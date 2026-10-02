//! Semantic search's embedding model, ported from the TS CLI's
//! `src/search/embeddings.ts` @ 1b8c950: the same pinned
//! `Xenova/all-MiniLM-L6-v2` quantised ONNX file, run with tract (pure Rust,
//! so both Mac release builds need no ONNX Runtime library). See
//! docs/explanation/embeddings.md for the choice and its measurements.
//!
//! This crate never touches the network or the index: the daemon downloads
//! the files ([`model`] says which, and checks them) and runs the model in
//! a [`worker`] process.

pub mod embedder;
pub mod model;
pub mod worker;

pub use embedder::{DIM, EmbedError, Embedder, FakeEmbedder, TractEmbedder, fake_vector};

/// What the vectors in the daemon's index were made with. A vector made any
/// other way doesn't count and is made again. It differs from the TS CLI's
/// `MODEL_VERSION` (`…:q8:mean-normalized`) in naming the runtime and the
/// batch size: the vectors agree with the TS CLI's to a cosine of 0.99, not
/// bit for bit. They live only in the daemon's index, never the TS CLI's.
pub const MODEL_VERSION: &str = "Xenova/all-MiniLM-L6-v2@751bff37182d3f1213fa05d7196b954e230abad9:q8:mean-normalized:tract-batch1";
