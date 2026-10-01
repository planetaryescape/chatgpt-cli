//! What a sync pass did.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    /// New and changed chats since the newest one indexed.
    #[default]
    Delta,
    /// Every chat, dropping ones deleted in the browser.
    Full,
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncReport {
    pub mode: SyncMode,
    pub elapsed_ms: u64,
    /// Delta: chats new to the index, and known chats that changed.
    #[serde(default)]
    pub added: u64,
    #[serde(default)]
    pub updated: u64,
    /// Whether the archived list was read (every explicit sync; hourly in
    /// the background).
    #[serde(default)]
    pub swept: bool,
    #[serde(default)]
    pub newly_archived: u64,
    #[serde(default)]
    pub unarchived: u64,
    #[serde(default)]
    pub deleted: u64,
    /// Full: the chats now indexed, and how many there were before.
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub active: u64,
    #[serde(default)]
    pub archived: u64,
    #[serde(default)]
    pub before: u64,
    #[serde(default)]
    pub reconcile: Option<ReconcileReport>,
    /// The TS CLI's own sync, run after this one while the bridge exists.
    #[serde(default)]
    pub ts_sync: Option<TsSyncOutcome>,
    /// The import of the TS CLI's judgments and titles that followed it.
    #[serde(default)]
    pub import: Option<ImportReport>,
}

/// `reconcileMetadataChanges`: cached work kept current when only a chat's
/// metadata changed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconcileReport {
    pub preserved: u64,
    pub changed: u64,
    /// `<id>: <why>`, without any response body.
    pub failures: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TsSyncOutcome {
    pub ok: bool,
    /// For people: how it ended, without any response body.
    pub message: String,
    pub elapsed_ms: u64,
}

/// What an import of the TS index changed, per table.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReport {
    pub path: String,
    pub tables: Vec<TableImport>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableImport {
    pub table: String,
    /// Rows in the TS index.
    pub rows: u64,
    pub inserted: u64,
    pub updated: u64,
    pub deleted: u64,
    /// Why the table was skipped, if it was.
    #[serde(default)]
    pub skipped: Option<String>,
}
