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
}

/// `reconcileMetadataChanges`: cached work kept current when only a chat's
/// metadata changed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconcileReport {
    pub preserved: u64,
    pub changed: u64,
    /// Unchanged chats whose cache another write replaced while they were
    /// checked, so nothing moved.
    #[serde(default)]
    pub superseded: u64,
    /// `<id>: <why>`, without any response body.
    pub failures: Vec<String>,
}
