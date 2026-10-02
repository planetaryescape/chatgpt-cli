//! How the daemon stands: `daemon status`.

use serde::{Deserialize, Serialize};

use crate::{AutoJevStatus, EmbeddingStatus, SearchIndexStatus};

/// What `Status` reports. Ready means this answers with a compatible
/// `protocol_version`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub protocol_version: u32,
    /// The daemon's package version.
    pub version: String,
    pub pid: u32,
    pub instance: String,
    /// Unix seconds.
    pub started_at: i64,
    #[serde(default)]
    pub socket: String,
    #[serde(default)]
    pub database: String,
    #[serde(default)]
    pub sync: SyncStatus,
    /// Set while ChatGPT's rate limit holds syncs back.
    #[serde(default)]
    pub backoff: Option<Backoff>,
    /// Where the session the daemon holds came from, e.g. `dia profile
    /// "Default"`; `None` before it first needed one.
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub classification: ClassificationInfo,
    /// The background search indexer.
    #[serde(default)]
    pub search_index: SearchIndexStatus,
    /// The background embedder.
    #[serde(default)]
    pub embeddings: EmbeddingStatus,
    /// Jev on newly synced chats, in the background.
    #[serde(default)]
    pub auto_jev: AutoJevStatus,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncStatus {
    pub in_progress: bool,
    /// When the index last synced (ISO 8601); `None` before the first sync.
    #[serde(default)]
    pub synced_at: Option<String>,
    /// Unix seconds: when the last pass finished, and how.
    #[serde(default)]
    pub last_finished_at: Option<i64>,
    #[serde(default)]
    pub last_summary: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    /// Unix seconds: the archived sweep and cache reconcile, at most hourly
    /// in the background.
    #[serde(default)]
    pub last_sweep_at: Option<i64>,
    /// Unix seconds: when the next background pass is due.
    #[serde(default)]
    pub next_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backoff {
    /// Unix seconds.
    pub until: i64,
    pub reason: String,
}

/// The classification versions verdicts are read with: this build's (D9).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassificationInfo {
    /// `builtin` (once also the TS sources they were read from).
    pub source: String,
    pub questions_version: String,
    pub deep_questions_version: String,
    pub luna_version: u32,
    pub local_title_version: u32,
    pub memory_version: String,
    pub topics: Vec<String>,
    /// Kept for older clients; always `None` now.
    #[serde(default)]
    pub problem: Option<String>,
}
