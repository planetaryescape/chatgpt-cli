//! Text to a 384-dimension unit vector: the TS CLI's `LocalEmbedder`
//! (`src/search/embeddings.ts` @ 1b8c950) on tract instead of ONNX Runtime.
//!
//! The same quantised ONNX file, tokenised as Transformers.js tokenises
//! (truncated at the model's 512 tokens; `tokenizer.json`'s own 128-token
//! truncation and fixed padding are ignored, as Transformers.js ignores
//! them), mean-pooled over the tokens and normalised. One text at a time:
//! the model quantises activations over the whole batch, so a vector would
//! otherwise depend on the texts beside it (docs/explanation/embeddings.md).

use std::path::Path;

use tokenizers::{Tokenizer, TruncationParams, TruncationStrategy};
use tract_onnx::prelude::{
    Framework, InferenceModelExt, IntoRunnable, IntoTValue, TValue, TVec, Tensor,
    TypedRunnableModel,
};

use crate::model::{self, MODEL_FILE, TOKENIZER_FILE};

pub const DIM: usize = 384;
/// `model_max_length` in the tokenizer config, where Transformers.js cuts.
pub const MAX_TOKENS: usize = 512;

/// Why a text couldn't be embedded. Never holds the text.
#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("the embedding model can't be used: {0}")]
    Model(#[from] model::FileProblem),
    #[error("the embedding model didn't load: {0}")]
    Load(String),
    #[error("the embedding model failed on a text: {0}")]
    Run(String),
}

pub trait Embedder {
    fn embed(&mut self, text: &str) -> Result<Vec<f32>, EmbedError>;
}

/// The real model.
pub struct TractEmbedder {
    tokenizer: Tokenizer,
    plan: std::sync::Arc<TypedRunnableModel>,
    /// For each of the model's inputs, in its order: 0 ids, 1 mask, 2 types.
    inputs: Vec<usize>,
}

impl TractEmbedder {
    /// Load the files in the pinned revision's directory, checking them
    /// against their checksums first.
    pub fn load(dir: &Path) -> Result<Self, EmbedError> {
        for file in model::FILES {
            model::verify_file(dir, &file)?;
        }
        let mut tokenizer = Tokenizer::from_file(dir.join(TOKENIZER_FILE.path))
            .map_err(|error| EmbedError::Load(format!("tokenizer: {error}")))?;
        tokenizer.with_padding(None);
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: MAX_TOKENS,
                strategy: TruncationStrategy::LongestFirst,
                ..TruncationParams::default()
            }))
            .map_err(|error| EmbedError::Load(format!("tokenizer: {error}")))?;
        let load = |error: tract_onnx::prelude::TractError| EmbedError::Load(error.to_string());
        let graph = tract_onnx::onnx()
            .model_for_path(dir.join(MODEL_FILE.path))
            .map_err(load)?;
        let inputs = graph
            .input_outlets()
            .map_err(load)?
            .iter()
            .map(|outlet| match graph.node(outlet.node).name.as_str() {
                "input_ids" => Ok(0),
                "attention_mask" => Ok(1),
                "token_type_ids" => Ok(2),
                other => Err(EmbedError::Load(format!("unexpected model input {other}"))),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let plan = graph
            .into_optimized()
            .and_then(|typed| typed.into_runnable())
            .map_err(load)?;
        Ok(Self {
            tokenizer,
            plan,
            inputs,
        })
    }
}

impl Embedder for TractEmbedder {
    fn embed(&mut self, text: &str) -> Result<Vec<f32>, EmbedError> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|error| EmbedError::Run(format!("tokenizer: {error}")))?;
        let tokens = encoding.get_ids().len();
        let columns = |values: &[u32]| values.iter().map(|&value| i64::from(value)).collect();
        let tensors: [Vec<i64>; 3] = [
            columns(encoding.get_ids()),
            columns(encoding.get_attention_mask()),
            columns(encoding.get_type_ids()),
        ];
        let run = |error: tract_onnx::prelude::TractError| EmbedError::Run(error.to_string());
        let inputs: TVec<TValue> = self
            .inputs
            .iter()
            .map(|&which| {
                Tensor::from_shape(&[1, tokens], &tensors[which]).map(IntoTValue::into_tvalue)
            })
            .collect::<Result<_, _>>()
            .map_err(run)?;
        let outputs = self.plan.run(inputs).map_err(run)?;
        let hidden = outputs
            .first()
            .ok_or_else(|| EmbedError::Run("the model returned nothing".to_owned()))?
            .to_plain_array_view::<f32>()
            .map_err(run)?;
        if hidden.shape() != [1, tokens, DIM] {
            return Err(EmbedError::Run(format!(
                "the model returned unexpected dimensions: {:?}",
                hidden.shape()
            )));
        }
        let hidden: Vec<f32> = hidden.iter().copied().collect();
        Ok(mean_normalized(&hidden, tokens))
    }
}

/// Transformers.js's `mean_pooling` (one text, so every token counts) and
/// `normalize`.
fn mean_normalized(hidden: &[f32], tokens: usize) -> Vec<f32> {
    let mut sum = vec![0f32; DIM];
    for token in hidden.as_chunks::<DIM>().0.iter().take(tokens) {
        for (total, value) in sum.iter_mut().zip(token) {
            *total += value;
        }
    }
    let count = tokens.max(1) as f32;
    for value in &mut sum {
        *value /= count;
    }
    let norm = sum.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut sum {
            *value /= norm;
        }
    }
    sum
}

/// A stand-in for tests that need vectors but not the model: a hashed bag
/// of ASCII words, so texts sharing words point the same way. It's simple
/// enough to compute bit for bit in JS too, which the parity harness does
/// (`crates/cli/tests/parity_semantic.rs`).
pub struct FakeEmbedder {
    /// How long each text takes, to stand in for the model's speed.
    pub delay: std::time::Duration,
}

impl Embedder for FakeEmbedder {
    fn embed(&mut self, text: &str) -> Result<Vec<f32>, EmbedError> {
        std::thread::sleep(self.delay);
        Ok(fake_vector(text))
    }
}

/// FNV-1a over each word's bytes picks a dimension and a sign; the sums are
/// normalised in f64 and rounded to f32. Words are runs of ASCII letters
/// (lowercased) and digits; every other character separates them.
pub fn fake_vector(text: &str) -> Vec<f32> {
    let mut values = [0f64; DIM];
    let mut word = Vec::new();
    let mut words = 0;
    let mut add = |word: &mut Vec<u8>| {
        if word.is_empty() {
            return;
        }
        let hash = word.iter().fold(0x811c_9dc5_u32, |hash, &byte| {
            (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
        });
        let sign = if (hash >> 16) & 1 == 1 { 1.0 } else { -1.0 };
        values[hash as usize % DIM] += sign;
        words += 1;
        word.clear();
    };
    for c in text.chars() {
        match c {
            'a'..='z' | '0'..='9' => word.push(c as u8),
            'A'..='Z' => word.push(c.to_ascii_lowercase() as u8),
            _ => add(&mut word),
        }
    }
    add(&mut word);
    if words == 0 || values.iter().all(|value| *value == 0.0) {
        values[0] = 1.0;
    }
    let norm = values.iter().map(|value| value * value).sum::<f64>().sqrt();
    values.iter().map(|value| (value / norm) as f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_vectors_are_unit_length_and_follow_shared_words() {
        let rust = fake_vector("Rust async runtimes");
        assert_eq!(rust.len(), DIM);
        let norm: f32 = rust.iter().map(|value| value * value).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
        assert_eq!(rust, fake_vector("rust ASYNC, runtimes!"));
        let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
        let near = fake_vector("async rust");
        let far = fake_vector("garden sermon");
        assert!(dot(&rust, &near) > dot(&rust, &far));
        // No words at all still gives a unit vector.
        assert_eq!(fake_vector("!!! 👍")[0], 1.0);
    }

    #[test]
    fn fake_vectors_are_pinned() {
        // The parity harness's JS port must produce these exact values.
        let vector = fake_vector("Rust async");
        let nonzero: Vec<(usize, f32)> = vector
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, value)| *value != 0.0)
            .collect();
        assert_eq!(nonzero.len(), 2, "{nonzero:?}");
        for (_, value) in nonzero {
            assert_eq!(value.abs(), std::f32::consts::FRAC_1_SQRT_2);
        }
    }

    #[test]
    fn pooling_averages_tokens_then_normalises() {
        let mut hidden = vec![0f32; DIM * 2];
        hidden[0] = 3.0;
        hidden[DIM] = 1.0;
        hidden[DIM + 1] = 2.0;
        let pooled = mean_normalized(&hidden, 2);
        // Mean (2, 1, 0…), normalised.
        let norm = 5f32.sqrt();
        assert_eq!(&pooled[..3], &[2.0 / norm, 1.0 / norm, 0.0]);
    }

    /// The real model against the TS CLI's vectors, when the model is in
    /// the cache (`chatgpt search-index` in either CLI puts it there).
    #[test]
    fn the_model_matches_transformers_js_when_cached() {
        let Some(dir) = crate::model::cache_root().map(|root| crate::model::revision_dir(&root))
        else {
            return;
        };
        if !crate::model::present(&dir) {
            eprintln!("skipping: the embedding model isn't in {}", dir.display());
            return;
        }
        let mut embedder = TractEmbedder::load(&dir).expect("load");
        let vector = embedder
            .embed(
                "Rust async runtimes compared: tokio, smol and async-std for a small CLI daemon.",
            )
            .expect("embed");
        // `bun scripts/dump-embeddings.ts --batch 1` for the same text.
        let ts: Vec<f32> =
            serde_json::from_str(include_str!("../tests/data/ts_vector.json")).expect("vector");
        let cosine: f32 = vector.iter().zip(&ts).map(|(a, b)| a * b).sum();
        assert!(cosine >= 0.99, "cosine {cosine}");
    }
}
