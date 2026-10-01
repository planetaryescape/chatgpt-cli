//! The daemon's answer to a request.

use serde::{Deserialize, Serialize};

use crate::{DaemonStatus, ImportReport, ListRows, StatsReport, SyncReport};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Ok {
        data: ResponseData,
    },
    Error {
        error: ErrorPayload,
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
