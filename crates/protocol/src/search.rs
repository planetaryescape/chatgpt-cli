//! Lexical `search` and the background search indexer.

use serde::{Deserialize, Serialize};

/// One chat that matched, with its best chunk (the TS CLI's `SearchHit`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub id: String,
    /// The display title: the local title when there is one.
    pub title: String,
    pub updated: String,
    pub archived: bool,
    /// Negated bm25: higher is better.
    pub score: f64,
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
}

/// The background indexer, for `daemon status`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchIndexStatus {
    /// Every chat in the index, and how many have current search chunks.
    #[serde(default)]
    pub chats: u64,
    #[serde(default)]
    pub indexed: u64,
    #[serde(default)]
    pub in_progress: bool,
    /// Transcripts fetched from ChatGPT since the daemon started.
    #[serde(default)]
    pub fetched: u64,
    /// Chats the current or last run couldn't fetch; retried later.
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
