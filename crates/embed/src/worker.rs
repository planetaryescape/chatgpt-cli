//! The embedding worker: a child process the daemon starts at low CPU
//! priority to run the model, so the daemon itself never competes with the
//! user's work, and stops when idle, which unloads the model.
//!
//! The daemon writes one request per text and reads one response:
//!
//! - request: `u32` little-endian length, then that many bytes of UTF-8;
//! - response: [`OK`] then [`DIM`] little-endian `f32`s; or [`FAILED`] (this
//!   text) or [`UNUSABLE`] (the model won't load), then a `u32` length and a
//!   UTF-8 message that never holds the text.
//!
//! The worker exits when its stdin closes.

use std::io::{BufReader, BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use crate::embedder::{DIM, EmbedError, Embedder, FakeEmbedder, TractEmbedder};

pub const OK: u8 = 0;
pub const FAILED: u8 = 1;
pub const UNUSABLE: u8 = 2;
/// The longest text a request may carry. Chunks are a few kilobytes.
pub const MAX_TEXT_BYTES: u32 = 1 << 20;

/// Which embedder the worker runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Model {
    /// The pinned revision's directory.
    Files(PathBuf),
    /// [`FakeEmbedder`], for tests (the daemon asks for it only in debug
    /// builds), taking `delay` per text (or only per text containing
    /// `slow_only`).
    Fake {
        delay: std::time::Duration,
        slow_only: Option<String>,
    },
}

/// A request's bytes.
pub fn request(text: &str) -> Vec<u8> {
    let mut frame = Vec::with_capacity(4 + text.len());
    frame.extend_from_slice(&(text.len() as u32).to_le_bytes());
    frame.extend_from_slice(text.as_bytes());
    frame
}

/// A vector's bytes as the search index stores them (`Float32Array`'s
/// little-endian layout, as the TS CLI writes them).
pub fn vector_bytes(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// The vector in `bytes`, if it is [`DIM`] little-endian `f32`s.
pub fn vector_from_bytes(bytes: &[u8]) -> Option<Vec<f32>> {
    if bytes.len() != DIM * 4 {
        return None;
    }
    Some(
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|four| f32::from_le_bytes(*four))
            .collect(),
    )
}

/// Run the worker on stdin and stdout until stdin closes. The model loads
/// on the first request; a model that won't load fails every request with
/// the reason.
pub fn serve(model: Model) -> ExitCode {
    let mut input = BufReader::new(std::io::stdin().lock());
    let mut output = BufWriter::new(std::io::stdout().lock());
    let mut embedder: Option<Box<dyn Embedder>> = None;
    loop {
        let mut length = [0u8; 4];
        match input.read_exact(&mut length) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                return ExitCode::SUCCESS;
            }
            Err(_) => return ExitCode::FAILURE,
        }
        let length = u32::from_le_bytes(length);
        if length > MAX_TEXT_BYTES {
            return ExitCode::FAILURE;
        }
        let mut text = vec![0; length as usize];
        if input.read_exact(&mut text).is_err() {
            return ExitCode::FAILURE;
        }
        let answer = match String::from_utf8(text) {
            Ok(text) => load(&mut embedder, &model).and_then(|embedder| embedder.embed(&text)),
            Err(_) => Err(EmbedError::Run("the text isn't UTF-8".to_owned())),
        };
        let written = match answer {
            Ok(vector) => output
                .write_all(&[OK])
                .and_then(|()| output.write_all(&vector_bytes(&vector))),
            Err(error) => {
                let message = error.to_string();
                let tag = match error {
                    EmbedError::Run(_) => FAILED,
                    EmbedError::Model(_) | EmbedError::Load(_) => UNUSABLE,
                };
                output
                    .write_all(&[tag])
                    .and_then(|()| output.write_all(&(message.len() as u32).to_le_bytes()))
                    .and_then(|()| output.write_all(message.as_bytes()))
            }
        };
        if written.and_then(|()| output.flush()).is_err() {
            return ExitCode::FAILURE;
        }
    }
}

fn load<'a>(
    embedder: &'a mut Option<Box<dyn Embedder>>,
    model: &Model,
) -> Result<&'a mut Box<dyn Embedder>, EmbedError> {
    Ok(match embedder {
        Some(loaded) => loaded,
        None => embedder.insert(match model {
            Model::Files(dir) => Box::new(TractEmbedder::load(dir)?),
            Model::Fake { delay, slow_only } => Box::new(FakeEmbedder {
                delay: *delay,
                slow_only: slow_only.clone(),
            }),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        assert_eq!(
            request("héllo"),
            [6, 0, 0, 0, b'h', 0xC3, 0xA9, b'l', b'l', b'o']
        );
        let vector: Vec<f32> = (0..DIM).map(|i| i as f32 / 7.0).collect();
        let bytes = vector_bytes(&vector);
        assert_eq!(bytes.len(), DIM * 4);
        assert_eq!(&bytes[4..8], &(1.0f32 / 7.0).to_le_bytes());
        assert_eq!(vector_from_bytes(&bytes), Some(vector));
        assert_eq!(vector_from_bytes(&bytes[1..]), None);
    }
}
