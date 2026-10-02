//! `search`, `search-index` and the background search indexer.

use serde::{Deserialize, Serialize};

/// The most results ChatGPT's search (`--remote`) returns per request.
pub const REMOTE_SEARCH_MAX: u64 = 40;

/// How a local `search` ranks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    /// Full text (`bm25`).
    #[default]
    Lexical,
    /// Embedding similarity (`--semantic`).
    Semantic,
    /// Both, fused by reciprocal rank (`--hybrid`).
    Hybrid,
    #[serde(other)]
    Unknown,
}

/// One chat that matched, with its best chunk (the TS CLI's `SearchHit`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub id: String,
    /// The display title: the local title when there is one.
    pub title: String,
    pub updated: String,
    pub archived: bool,
    /// Higher is better: negated bm25, a dot product or a fused rank.
    /// `None` for `--remote`, which ChatGPT doesn't score.
    pub score: Option<f64>,
    pub snippet: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    /// When the index last synced, for the stale-index note.
    pub synced_at: String,
    /// Chats in the searched scope, and how many of them are indexed: the
    /// answer covers only those while the indexer catches up.
    pub chats: u64,
    pub indexed: u64,
    /// For `--semantic` and `--hybrid`: chunks in scope, and how many of
    /// them have vectors.
    #[serde(default)]
    pub chunks: u64,
    #[serde(default)]
    pub embedded: u64,
}

/// What `search-index` found once the daemon caught up.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchIndexReport {
    pub chats: u64,
    pub indexed: u64,
    pub chunks: u64,
    pub embedded: u64,
    /// `<id> <title>: <why>` for each chat whose transcript couldn't be had.
    #[serde(default)]
    pub failures: Vec<String>,
    /// Why the index isn't complete although the daemon stopped working on
    /// it, such as a rate limit or a model that couldn't be downloaded.
    #[serde(default)]
    pub waiting: Vec<String>,
    pub elapsed_ms: u64,
}

/// The background embedder, for `daemon status`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingStatus {
    /// Chunks current for their chat, and how many have vectors.
    #[serde(default)]
    pub chunks: u64,
    #[serde(default)]
    pub embedded: u64,
    /// Embedding now, or asked to and about to start.
    #[serde(default)]
    pub in_progress: bool,
    /// Chunks the model failed on, skipped until the daemon restarts.
    #[serde(default)]
    pub failed: u64,
    /// Why embedding waits: the model is downloading, or couldn't be.
    #[serde(default)]
    pub waiting: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    /// Unix seconds.
    #[serde(default)]
    pub last_finished_at: Option<i64>,
}

/// The background indexer, for `daemon status`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchIndexStatus {
    /// Every chat in the index, and how many have current search chunks.
    #[serde(default)]
    pub chats: u64,
    #[serde(default)]
    pub indexed: u64,
    /// Indexing now, or asked to and about to start.
    #[serde(default)]
    pub in_progress: bool,
    /// Transcripts fetched from ChatGPT since the daemon started.
    #[serde(default)]
    pub fetched: u64,
    /// Chats set aside because ChatGPT left them out of a batch or
    /// answered it with an error; each is asked for again after an hour.
    #[serde(default)]
    pub failed: u64,
    /// Unix seconds.
    #[serde(default)]
    pub last_finished_at: Option<i64>,
    #[serde(default)]
    pub last_error: Option<String>,
    /// Why fetching waits, e.g. for the first sync or a rate limit.
    #[serde(default)]
    pub waiting: Option<String>,
}
