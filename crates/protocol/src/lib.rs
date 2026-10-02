//! The IPC protocol between the chatgpt CLI and its daemon: length-delimited
//! JSON over a Unix socket, each frame a [`Message`] envelope `{ id, payload }`.
//! The pattern is ms-todo's (`crates/protocol` @ 72a4064), itself mxr's.
//!
//! Compatibility rules, so an older client and a newer daemon (or the other
//! way round) fail clearly instead of mis-reading each other:
//!
//! - Every tagged enum has an `Unknown` variant that a tag this build doesn't
//!   know decodes to.
//! - Fields added after the first release carry `#[serde(default)]`.
//! - A change that can't follow those rules bumps [`PROTOCOL_VERSION`].
//!   Clients check it through `Status` before anything else.

mod classify;
mod codec;
mod events;
mod export;
mod mutations;
mod reads;
mod request;
mod response;
mod search;
mod status;
mod sync;

pub use classify::{AutoJevStatus, ClassifiedMemories, ClassifyOutcome, ModelAccess};
pub use codec::{Codec, FrameTooLarge, MAX_FRAME_BYTES};
pub use events::{Event, Progress, ProgressKind};
pub use export::ExportedChat;
pub use mutations::{ChatAction, Outcome, Project, Secret, Selection, Target};
pub use reads::{Filter, Jev, ListRows, MemoryCounts, Row, StatsReport, TopicCounts};
pub use request::{Request, SessionChoice};
pub use response::{ErrorPayload, Response, ResponseData};
pub use search::{
    EmbeddingStatus, REMOTE_SEARCH_MAX, SearchHit, SearchIndexReport, SearchIndexStatus,
    SearchMode, SearchResults,
};
use serde::{Deserialize, Serialize};
pub use status::{
    Backoff, ClassificationInfo, DaemonStatus, ImportStatus, SyncStatus, TsSyncStatus,
};
pub use sync::{ImportReport, ReconcileReport, SyncMode, SyncReport, TableImport, TsSyncOutcome};

/// Bumped on any change an older peer can't read.
pub const PROTOCOL_VERSION: u32 = 1;

/// The daemon's exit status when its database was upgraded by a newer
/// chatgpt (a migration this build doesn't know). The client that started it
/// reports `database_too_new` instead of the daemon's log (78 is sysexits'
/// `EX_CONFIG`).
pub const EXIT_DATABASE_TOO_NEW: u8 = 78;

/// The socket buffer both ends ask for: room for a full `list --json` in a
/// few writes. macOS gives a Unix socket 8 KiB by default.
pub const SOCKET_BUFFER_BYTES: usize = 1024 * 1024;

/// One frame. A client picks `id`, and the daemon echoes it on the response
/// and on the progress events it sends for that request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: u64,
    pub payload: Payload,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Payload {
    Request(Request),
    Response(Response),
    Event(Event),
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(message: &Message) -> Message {
        let json = serde_json::to_string(message).expect("encode");
        serde_json::from_str(&json).expect("decode")
    }

    #[test]
    fn requests_and_responses_round_trip() {
        let request = Message {
            id: 7,
            payload: Payload::Request(Request::List {
                filter: Box::new(Filter {
                    title: Some("idea".into()),
                    brainstorm: Some(String::new()),
                    ..Filter::default()
                }),
            }),
        };
        assert_eq!(round_trip(&request), request);
        let response = Message {
            id: 7,
            payload: Payload::Response(Response::Error {
                error: ErrorPayload {
                    kind: "invalid_input".into(),
                    message: "--limit must be a positive whole number.".into(),
                },
            }),
        };
        assert_eq!(round_trip(&response), response);
    }

    #[test]
    fn tags_from_a_newer_peer_decode_as_unknown() {
        let message: Message = serde_json::from_str(
            r#"{"id":1,"payload":{"type":"request","method":"teleport","to":"mars"}}"#,
        )
        .expect("decode");
        assert_eq!(message.payload, Payload::Request(Request::Unknown));
        let event: Message =
            serde_json::from_str(r#"{"id":1,"payload":{"type":"event","event":"fireworks"}}"#)
                .expect("decode");
        assert_eq!(event.payload, Payload::Event(Event::Unknown));
        let payload: Message =
            serde_json::from_str(r#"{"id":1,"payload":{"type":"hologram"}}"#).expect("decode");
        assert_eq!(payload.payload, Payload::Unknown);
    }

    #[test]
    fn search_messages_from_0_1_1_still_decode() {
        let request: Request =
            serde_json::from_str(r#"{"method":"search","query":"rust","limit":5}"#)
                .expect("decode");
        assert!(matches!(
            request,
            Request::Search {
                mode: SearchMode::Lexical,
                ..
            }
        ));
        let unknown: Request = serde_json::from_str(
            r#"{"method":"search","query":"rust","limit":5,"mode":"telepathic"}"#,
        )
        .expect("decode");
        assert!(matches!(
            unknown,
            Request::Search {
                mode: SearchMode::Unknown,
                ..
            }
        ));
        let results: SearchResults = serde_json::from_str(
            r#"{"hits":[{"id":"a","title":"T","updated":"u","archived":false,"score":1.5,"snippet":"s"}],
                "synced_at":"t","chats":1,"indexed":1}"#,
        )
        .expect("decode");
        assert_eq!(results.hits[0].score, Some(1.5));
        assert_eq!(results.hits[0].snippet_cut, None);
        assert_eq!(results.embedded, 0);
        let status: DaemonStatus = serde_json::from_str(
            r#"{"protocol_version":1,"version":"0.1.1","pid":4,"instance":"dev","started_at":0}"#,
        )
        .expect("decode");
        assert_eq!(status.embeddings, EmbeddingStatus::default());
    }

    #[test]
    fn missing_additive_fields_take_their_defaults() {
        let status: DaemonStatus = serde_json::from_str(
            r#"{"protocol_version":1,"version":"0.1.0","pid":4,"instance":"dev","started_at":0}"#,
        )
        .expect("decode");
        assert_eq!(status.sync, SyncStatus::default());
        assert!(status.backoff.is_none());
    }
}
