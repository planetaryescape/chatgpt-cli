//! The daemon's answer to a request.

use serde::{Deserialize, Serialize};

use crate::{
    ChatTranscript, ClassifiedMemories, ClassifyOutcome, DaemonStatus, ExportedChat, ImportReport,
    ListRows, Outcome, Project, SearchIndexReport, SearchResults, StatsReport, SyncReport,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok {
        data: ResponseData,
    },
    Error {
        error: ErrorPayload,
    },
    /// The answer was too large for one frame and came as `Part` events
    /// before this: their text, joined, is the `Response` (0.1.5 and
    /// newer; an older client can't read it). The client checks it got
    /// exactly `count` parts holding `bytes` bytes before reading them.
    Parted {
        count: u64,
        bytes: u64,
    },
    #[serde(other)]
    Unknown,
}

impl From<Result<ResponseData, ErrorPayload>> for Response {
    fn from(result: Result<ResponseData, ErrorPayload>) -> Self {
        match result {
            Ok(data) => Self::Ok { data },
            Err(error) => Self::Error { error },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResponseData {
    Status(Box<DaemonStatus>),
    Sync(Box<SyncReport>),
    Rows(ListRows),
    Stats(Box<StatsReport>),
    Imported(ImportReport),
    Ack,
    Exported(Box<ExportedChat>),
    Transcript(Box<ChatTranscript>),
    SearchHits(SearchResults),
    SearchIndexed(SearchIndexReport),
    /// `JevCheck`: the ids Jev backs the action for, in the order asked.
    Approved {
        ids: Vec<String>,
    },
    /// `Mutate`, `MoveToProject`, `DeleteMemories`.
    Outcome(Outcome),
    Renamed {
        id: String,
        /// The ChatGPT title before the rename.
        old_title: String,
        /// When the index last synced, for the stale-index note.
        synced_at: String,
    },
    TitleSaved {
        id: String,
        synced_at: String,
    },
    Projects {
        projects: Vec<Project>,
    },
    ProjectCreated(Project),
    /// ChatGPT's saved-memory objects, as it sent them.
    Memories {
        memories: Vec<serde_json::Value>,
    },
    MemorySummary {
        summary: serde_json::Value,
    },
    /// `Classify`, `Titles`.
    Classified(ClassifyOutcome),
    /// `MemoryClassify`.
    ClassifiedMemories(ClassifiedMemories),
    #[serde(other)]
    Unknown,
}

/// A failed request. `kind` is a `chatgpt_core::ErrorKind` string; a kind
/// the client doesn't know is treated as `internal`. `message` is shown as
/// is: the daemon never puts a response body, cookie or token in it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorPayload {
    pub kind: String,
    pub message: String,
}
