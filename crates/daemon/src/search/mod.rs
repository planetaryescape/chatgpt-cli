//! Lexical search: transcript chunks in an FTS5 index the daemon keeps
//! current in the background ([`indexer`]), and the `search` read over it
//! ([`query`]). Ported from the TS CLI's `src/search/` @ 1b8c950, without
//! embeddings.

pub mod chunks;
pub mod indexer;
pub mod query;

use chatgpt_store::ChunkVersions;

use crate::policy::Profile;

/// The versions current chunks carry: the transcript render and the chunker.
pub fn versions(profile: &Profile) -> ChunkVersions {
    ChunkVersions {
        render: profile.render_version,
        chunk: chunks::CHUNK_VERSION,
    }
}
