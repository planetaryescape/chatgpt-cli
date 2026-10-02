//! Answering requests.

use std::sync::Arc;

use chatgpt_core::ErrorKind;
use chatgpt_protocol::Progress;
use chatgpt_protocol::{
    DaemonStatus, ErrorPayload, Filter, PROTOCOL_VERSION, Request, Response, ResponseData,
    SearchMode, SessionChoice, StatsReport,
};
use rusqlite::Connection;
use tokio::sync::mpsc::UnboundedSender;

use crate::api::{Api, ApiError};
use crate::filters::InvalidFilter;
use crate::policy::PolicyError;
use crate::policy::memory::{Cached, memory_counts};
use crate::state::State;
use crate::sync::{PassOptions, run_pass};
use crate::{export, jev, memories, mutate, projects, reads, search, select, ts_sync};

/// A failed request, worded for people: no response body, cookie or token.
#[derive(Debug, Clone)]
pub struct Failure {
    pub kind: ErrorKind,
    pub message: String,
}

impl Failure {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn invalid(error: InvalidFilter) -> Self {
        Self::new(ErrorKind::InvalidInput, error.0)
    }

    pub fn store(error: chatgpt_store::StoreError) -> Self {
        let kind = match error {
            chatgpt_store::StoreError::NewerDatabase { .. } => ErrorKind::DatabaseTooNew,
            _ => ErrorKind::Internal,
        };
        Self::new(kind, format!("the index: {error}"))
    }

    pub fn policy(error: PolicyError) -> Self {
        Self::new(ErrorKind::Internal, error.to_string())
    }

    pub fn join(error: tokio::task::JoinError) -> Self {
        Self::new(
            ErrorKind::Internal,
            format!("a database task failed: {error}"),
        )
    }

    fn payload(self) -> ErrorPayload {
        ErrorPayload {
            kind: self.kind.as_str().to_owned(),
            message: self.message,
        }
    }
}

impl From<ApiError> for Failure {
    fn from(error: ApiError) -> Self {
        Self::new(error.kind, error.message)
    }
}

pub fn error_payload(kind: ErrorKind, message: String) -> ErrorPayload {
    Failure::new(kind, message).payload()
}

/// Answer `request`. A sync sends its progress lines to `progress`.
pub async fn handle(
    state: &Arc<State>,
    request: Request,
    progress: Option<UnboundedSender<Progress>>,
) -> Response {
    let answered = match request {
        Request::Status => Ok(ResponseData::Status(Box::new(status(state).await))),
        Request::Shutdown => Ok(ResponseData::Ack),
        Request::Sync { full, session } => {
            // Its own task: a client that goes away mid-sync mustn't cancel
            // the pass halfway.
            let state = Arc::clone(state);
            let options = PassOptions {
                explicit: true,
                full,
                choice: Some(session),
                progress,
            };
            tokio::spawn(async move { run_pass(&state, options).await })
                .await
                .map_err(Failure::join)
                .and_then(|result| result)
                .map(|report| ResponseData::Sync(Box::new(report)))
        }
        Request::List { filter } => read(state, move |db, profile, now| {
            reads::list(db, &filter, profile, now)
        })
        .await
        .map(ResponseData::Rows),
        Request::Stats { filter, session } => stats(state, *filter, &session)
            .await
            .map(|report| ResponseData::Stats(Box::new(report))),
        Request::ImportLegacy => ts_sync::import(state).await.map(ResponseData::Imported),
        Request::Export {
            reference,
            stdin_ids,
            archived,
            all,
            session,
        } => export::export(state, reference, stdin_ids, archived, all, session)
            .await
            .map(|chat| ResponseData::Exported(Box::new(chat))),
        Request::Search {
            query,
            limit,
            archived,
            all,
            mode,
        } => {
            let scope = (!all).then_some(archived);
            match mode {
                SearchMode::Lexical => {
                    read(state, move |db, profile, _| {
                        search::query::search(db, &query, limit, scope, profile)
                    })
                    .await
                }
                SearchMode::Semantic | SearchMode::Hybrid => {
                    search::semantic::search(state, query, limit, scope, mode).await
                }
                SearchMode::Unknown => Err(unknown_request()),
            }
            .map(ResponseData::SearchHits)
        }
        Request::RemoteSearch {
            query,
            limit,
            archived,
            all,
            session,
        } => search::remote::search(state, query, limit, (!all).then_some(archived), session)
            .await
            .map(ResponseData::SearchHits),
        Request::SearchIndex { archived, all } => {
            search::catch_up::search_index(state, archived, all, progress)
                .await
                .map(ResponseData::SearchIndexed)
        }
        Request::Select { selection } => read(state, move |db, profile, now| {
            select::rows(db, &selection, profile, now)
        })
        .await
        .map(ResponseData::Rows),
        Request::JevCheck {
            action,
            ids,
            api_key,
            session,
        } => jev::check(state, action, ids, api_key, session, progress)
            .await
            .map(|ids| ResponseData::Approved { ids }),
        Request::Mutate {
            action,
            targets,
            session,
        } => mutate::apply(state, action, targets, session, progress)
            .await
            .map(ResponseData::Outcome),
        Request::Rename {
            reference,
            title,
            archived,
            all,
            session,
        } => mutate::rename(state, reference, title, archived, all, session)
            .await
            .map(|(id, old_title, synced_at)| ResponseData::Renamed {
                id,
                old_title,
                synced_at,
            }),
        Request::SetTitle {
            reference,
            title,
            archived,
            all,
        } => mutate::set_title(state, reference, title, archived, all)
            .await
            .map(|(id, synced_at)| ResponseData::TitleSaved { id, synced_at }),
        Request::Projects { session } => Api::new(Arc::clone(&state.sessions), session)
            .projects()
            .await
            .map(|projects| ResponseData::Projects { projects })
            .map_err(Failure::from),
        Request::CreateProject { name, session } => Api::new(Arc::clone(&state.sessions), session)
            .create_project(&name)
            .await
            .map(ResponseData::ProjectCreated)
            .map_err(Failure::from),
        Request::MoveToProject {
            project,
            targets,
            remove,
            session,
        } => projects::move_chats(state, project, targets, remove, session, progress)
            .await
            .map(ResponseData::Outcome),
        Request::Memories { session } => Api::new(Arc::clone(&state.sessions), session)
            .memory_objects()
            .await
            .map(|memories| ResponseData::Memories { memories })
            .map_err(Failure::from),
        Request::MemorySummary { session } => Api::new(Arc::clone(&state.sessions), session)
            .memory_summary()
            .await
            .map(|summary| ResponseData::MemorySummary { summary })
            .map_err(Failure::from),
        Request::DeleteMemories { ids, session } => memories::delete(state, ids, session)
            .await
            .map(ResponseData::Outcome),
        Request::Unknown => Err(unknown_request()),
    };
    answered.map_err(Failure::payload).into()
}

fn unknown_request() -> Failure {
    Failure::new(
        ErrorKind::Unsupported,
        "this daemon doesn't know that request; run `chatgpt daemon stop` and try again",
    )
}

/// Run a read on the store's reader connection with the current versions.
async fn read<T: Send + 'static>(
    state: &State,
    work: impl FnOnce(&Connection, &crate::policy::Profile, i64) -> Result<T, Failure> + Send + 'static,
) -> Result<T, Failure> {
    let profile = state.profile();
    let now_ms = chrono::Utc::now().timestamp_millis();
    state.db(move |db| Ok(work(db, &profile, now_ms))).await?
}

/// `stats`: the chat counts from the index, then the saved memories, read
/// live. A failed memory read is reported, not fatal, as in the TS CLI.
async fn stats(
    state: &State,
    filter: Filter,
    session: &SessionChoice,
) -> Result<StatsReport, Failure> {
    let mut report = read(state, move |db, profile, now| {
        reads::chat_stats(db, &filter, profile, now)
    })
    .await?;
    // Its own session for this choice: a sync running meanwhile keeps its.
    let api = Api::new(Arc::clone(&state.sessions), session.clone());
    match api.memories().await {
        Ok(memories) => {
            let as_of = chrono::Utc::now().format("%Y-%m-%d").to_string();
            let counted = read(state, move |db, profile, _| {
                memory_counts(&memories, &as_of, |id, hash| {
                    chatgpt_store::memory_judgment(db, id, hash, &profile.memory_version)
                        .map(|row| {
                            row.map(|row| Cached {
                                system_one: row.system_one,
                                system_two: row.system_two,
                            })
                        })
                        .map_err(|error| error.to_string())
                })
                .map_err(|message| Failure::new(ErrorKind::Internal, message))
            })
            .await;
            match counted {
                Ok(counts) => report.memory = Some(counts),
                Err(failure) => report.memory_error = Some(failure.message),
            }
        }
        Err(error) => report.memory_error = Some(error.message),
    }
    Ok(report)
}

async fn status(state: &State) -> DaemonStatus {
    let synced_at = state.db(chatgpt_store::synced_at).await.ok().flatten();
    let (sync, backoff) = state.syncer.status(synced_at);
    DaemonStatus {
        protocol_version: PROTOCOL_VERSION,
        version: state.version.clone(),
        pid: std::process::id(),
        instance: state.paths.instance.label().to_owned(),
        started_at: state.started_at,
        socket: state.paths.socket_path().display().to_string(),
        database: state.store.path().display().to_string(),
        sync,
        backoff,
        session: state.sessions.source(&state.syncer.choice()),
        ts_sync: state.ts_sync_status(),
        legacy_import: state.import_status(),
        classification: state.profile().info(),
        search_index: state.indexer.status(),
        embeddings: state.embedder.status(),
    }
}
