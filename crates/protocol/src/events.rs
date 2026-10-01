//! What the daemon sends while a request runs.

use serde::{Deserialize, Serialize};

/// Sent with a request's message ID while it runs. Each one is progress, so
/// it restarts the client's stall clock. A client skips events it doesn't
/// know.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Progress(Progress),
    /// Sent every few seconds while a request runs, so a long step that
    /// has nothing to report never trips the client's stall timeout.
    Heartbeat,
    #[serde(other)]
    Unknown,
}

/// One status line for stderr, already worded the way the TS CLI words it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub kind: ProgressKind,
    pub line: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressKind {
    /// A step began: its label, which a terminal's live line shows anyway
    /// and plain output prints once.
    Start,
    /// A step's running count: a live line on a terminal, an occasional
    /// line otherwise.
    Update,
    /// A step finished: always printed.
    Finish,
    /// A notice such as a rate-limit wait: always printed.
    Note,
    #[serde(other)]
    Unknown,
}
