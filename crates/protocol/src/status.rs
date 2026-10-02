//! How the daemon stands: `daemon status`.

use serde::{Deserialize, Serialize};

use crate::{EmbeddingStatus, ImportReport, SearchIndexStatus};

/// What `Status` reports. Ready means this answers with a compatible
/// `protocol_version`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    pub ts_sync: TsSyncStatus,
    #[serde(default)]
    pub legacy_import: ImportStatus,
    #[serde(default)]
    pub classification: ClassificationInfo,
    /// The background search indexer.
    #[serde(default)]
    pub search_index: SearchIndexStatus,
    /// The background embedder.
    #[serde(default)]
    pub embeddings: EmbeddingStatus,
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

/// The TS CLI's sync, which the daemon runs while the bridge exists.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TsSyncStatus {
    /// The TS CLI's `cli.ts`, or why it wasn't found.
    #[serde(default)]
    pub cli: Option<String>,
    #[serde(default)]
    pub unavailable: Option<String>,
    /// Unix seconds.
    #[serde(default)]
    pub last_run_at: Option<i64>,
    #[serde(default)]
    pub last_ok: Option<bool>,
    #[serde(default)]
    pub last_message: Option<String>,
}

/// The import of the TS index's judgments and titles.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportStatus {
    /// Unix seconds.
    #[serde(default)]
    pub last_at: Option<i64>,
    #[serde(default)]
    pub last: Option<ImportReport>,
    #[serde(default)]
    pub last_error: Option<String>,
}

/// The classification versions verdicts are read with. While the bridge
/// exists the TS CLI writes the judgments, so its sources decide them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassificationInfo {
    /// `builtin`, or the TS sources they were read from.
    pub source: String,
    pub questions_version: String,
    pub deep_questions_version: String,
    pub luna_version: u32,
    pub local_title_version: u32,
    pub memory_version: String,
    pub topics: Vec<String>,
    /// Why the TS sources couldn't be used, if they couldn't.
    #[serde(default)]
    pub problem: Option<String>,
}
