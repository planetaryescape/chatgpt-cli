//! The chatgpt.com endpoints the daemon reads, ported from the TS CLI's
//! `src/api/conversations.ts` and `memories.ts` @ 1b8c950: GETs, and the
//! batch read and search, which are POSTs. The writes are in `api/writes.rs`.

mod writes;

use std::sync::Arc;
use std::time::Duration;

use chatgpt::cookies::CookieError;
use chatgpt::http::{HttpError, HttpMethod, Resend};
use chatgpt::session::SessionError;
use chatgpt_core::ErrorKind;
use chatgpt_protocol::SessionChoice;
use chatgpt_store::NewConversation;
use serde::Deserialize;
use serde_json::Value;

use crate::policy::memory::SavedMemory;
use crate::render::Conversation;
use crate::session::{Session, Sessions};

pub const PAGE_SIZE: usize = 100;
pub const BATCH_MAX: usize = 10;

/// A failed call, already worded for people. Never holds a response body,
/// a cookie or a token: it goes to the log, `daemon status` and the CLI.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct ApiError {
    pub kind: ErrorKind,
    pub message: String,
    /// The HTTP status, when the server answered.
    pub status: Option<u16>,
    /// For a rate limit: how long the server asked to wait.
    pub retry_after: Option<Duration>,
}

impl ApiError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            status: None,
            retry_after: None,
        }
    }

    pub fn is_rate_limit(&self) -> bool {
        self.kind == ErrorKind::RateLimited
    }
}

impl From<crate::handlers::Failure> for ApiError {
    fn from(failure: crate::handlers::Failure) -> Self {
        Self::new(failure.kind, failure.message)
    }
}

impl From<CookieError> for ApiError {
    fn from(error: CookieError) -> Self {
        // "No ChatGPT session in …" and the Keychain prompt's message: the
        // user's own browser, nothing to redact.
        Self::new(ErrorKind::AuthRequired, error.to_string())
    }
}

impl From<SessionError> for ApiError {
    fn from(error: SessionError) -> Self {
        match error {
            SessionError::Http(http) => http.into(),
            // "ChatGPT session in … has expired. Open chatgpt.com there …"
            expired @ SessionError::Expired { .. } => {
                Self::new(ErrorKind::AuthRequired, expired.to_string())
            }
            malformed @ SessionError::Malformed { .. } => {
                Self::new(ErrorKind::Decode, malformed.to_string())
            }
        }
    }
}

impl From<HttpError> for ApiError {
    fn from(error: HttpError) -> Self {
        match error {
            HttpError::Status { status, path, .. } => {
                // The snippet can hold conversation content: never kept.
                let kind = match status.as_u16() {
                    429 => ErrorKind::RateLimited,
                    401 | 403 => ErrorKind::AuthRequired,
                    _ => ErrorKind::Api,
                };
                let message = if kind == ErrorKind::RateLimited {
                    format!("rate limited by ChatGPT ({status} from {path}); try again in a minute")
                } else {
                    format!("{status} from {path}")
                };
                Self {
                    status: Some(status.as_u16()),
                    ..Self::new(kind, message)
                }
            }
            HttpError::RateLimited { retry_after, .. } => Self {
                status: Some(429),
                retry_after: Some(retry_after),
                ..Self::new(ErrorKind::RateLimited, error.to_string())
            },
            HttpError::Challenged { .. } | HttpError::Transport { .. } => {
                Self::new(ErrorKind::Network, error.to_string())
            }
            HttpError::Build(_)
            | HttpError::InvalidHeaderValue { .. }
            | HttpError::EnvironmentNotPrepared => {
                Self::new(ErrorKind::Internal, error.to_string())
            }
        }
    }
}

fn decode_error(path: &str, error: &serde_json::Error) -> ApiError {
    // serde's message can quote the body; only its position is kept.
    ApiError::new(
        ErrorKind::Decode,
        format!(
            "ChatGPT's answer to {path} wasn't what the CLI expects ({:?} error at line {}, column {})",
            error.classify(),
            error.line(),
            error.column()
        ),
    )
}

/// A chat in the conversation list. Shapes observed 2026-09-27.
#[derive(Clone, Debug, Deserialize)]
pub struct ConversationSummary {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    pub create_time: String,
    pub update_time: String,
    #[serde(default)]
    pub is_archived: bool,
    #[serde(default)]
    pub pinned_time: Option<String>,
    #[serde(default)]
    pub gizmo_id: Option<String>,
}

impl ConversationSummary {
    /// `ConversationIndex.row`.
    pub fn to_index(&self) -> NewConversation {
        NewConversation {
            id: self.id.clone(),
            title: self.title.clone().unwrap_or_else(|| "(untitled)".into()),
            create_time: self.create_time.clone(),
            update_time: self.update_time.clone(),
            is_archived: self.is_archived,
            pinned: self
                .pinned_time
                .as_deref()
                .is_some_and(|time| !time.is_empty()),
            project_id: self
                .gizmo_id
                .clone()
                .filter(|gizmo| gizmo.starts_with("g-p-")),
        }
    }
}

#[derive(Deserialize)]
struct ListPage {
    items: Vec<ConversationSummary>,
}

/// A batch item: a full conversation with `id`, ISO times and archive state.
#[derive(Clone, Debug, Deserialize)]
pub struct BatchItem {
    pub id: String,
    #[serde(default)]
    pub create_time: Option<String>,
    #[serde(default)]
    pub is_archived: Option<bool>,
    #[serde(flatten)]
    pub conversation: Conversation,
}

/// A conversation result of ChatGPT's search (the TS CLI's `SearchHit`).
#[derive(Clone, Debug, Deserialize)]
pub struct GlobalSearchHit {
    pub title: String,
    pub snippet: String,
    /// Unix seconds.
    pub update_time: f64,
    pub payload: GlobalSearchPayload,
}

#[derive(Clone, Debug, Deserialize)]
pub struct GlobalSearchPayload {
    pub conversation_id: String,
    #[serde(default)]
    pub is_archived: Option<bool>,
}

/// Calls made with one browser choice's session, which the daemon reads
/// once and renews only when ChatGPT rejects it. A sync pins the account
/// too: a renewed session for another account fails the call rather than
/// mixing two accounts' chats in one pass.
#[derive(Clone)]
pub struct Api {
    sessions: Arc<Sessions>,
    choice: SessionChoice,
    account: Option<String>,
}

impl Api {
    pub fn new(sessions: Arc<Sessions>, choice: SessionChoice) -> Self {
        Self {
            sessions,
            choice,
            account: None,
        }
    }

    pub fn choice(&self) -> &SessionChoice {
        &self.choice
    }

    /// Open this choice's session if need be and pin its account: later
    /// calls fail if a renewal brings another account.
    pub async fn pinned(mut self) -> Result<Self, ApiError> {
        let session = self.sessions.current(&self.choice).await?;
        self.account = session.token.account().map(str::to_owned);
        Ok(self)
    }

    /// The account a pinned client keeps to.
    pub fn account(&self) -> Option<&str> {
        self.account.as_deref()
    }

    fn same_account(&self, session: &Session) -> Result<(), ApiError> {
        match (&self.account, session.token.account()) {
            (Some(pinned), Some(now)) if pinned != now => Err(ApiError::new(
                ErrorKind::AuthRequired,
                format!(
                    "the session in {} now belongs to another ChatGPT account; stopped this sync so it doesn't mix accounts",
                    session.source
                ),
            )),
            _ => Ok(()),
        }
    }

    async fn send(
        &self,
        method: HttpMethod,
        path: &str,
        body: Option<&str>,
    ) -> Result<String, ApiError> {
        Ok(self.send_raw(method, path, body, Resend::Allowed).await?)
    }

    /// The call, with the session renewed once if ChatGPT rejected it. A
    /// rejected token means the request was refused, so sending it again is
    /// safe even for a write that mustn't happen twice.
    async fn send_raw(
        &self,
        method: HttpMethod,
        path: &str,
        body: Option<&str>,
        resend: Resend,
    ) -> Result<String, SendError> {
        let session = self.sessions.current(&self.choice).await?;
        self.same_account(&session)?;
        match call(&session, method, path, body, resend).await {
            // 401, or a 403 that isn't a Cloudflare challenge (that's a
            // separate error): the token or the cookies went stale. Read
            // them again once.
            Err(HttpError::Status { status, .. })
                if status.as_u16() == 401 || status.as_u16() == 403 =>
            {
                let renewed = self.sessions.renew(&self.choice, &session).await?;
                self.same_account(&renewed)?;
                Ok(call(&renewed, method, path, body, resend).await?)
            }
            other => Ok(other?),
        }
    }

    async fn json<T: serde::de::DeserializeOwned>(
        &self,
        method: HttpMethod,
        path: &str,
        body: Option<&str>,
    ) -> Result<T, ApiError> {
        let text = self.send(method, path, body).await?;
        serde_json::from_str(&text).map_err(|error| decode_error(path, &error))
    }

    /// One page of the conversation list, newest update first.
    pub async fn list_page(
        &self,
        archived: bool,
        offset: usize,
    ) -> Result<Vec<ConversationSummary>, ApiError> {
        let path = format!(
            "/backend-api/conversations?offset={offset}&limit={PAGE_SIZE}&order=updated&is_archived={archived}&hide_snorlax=false"
        );
        Ok(self
            .json::<ListPage>(HttpMethod::Get, &path, None)
            .await?
            .items)
    }

    /// One whole conversation. A 404 comes back as `Ok(None)`: the chat was
    /// deleted.
    pub async fn conversation(&self, id: &str) -> Result<Option<Value>, ApiError> {
        let path = format!("/backend-api/conversation/{id}");
        match self.json::<Value>(HttpMethod::Get, &path, None).await {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.status == Some(404) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Up to [`BATCH_MAX`] whole conversations. Unknown ids are silently
    /// left out; some deleted ids still come back.
    pub async fn batch(&self, ids: &[String]) -> Result<Vec<BatchItem>, ApiError> {
        if ids.len() > BATCH_MAX {
            return Err(ApiError::new(
                ErrorKind::Internal,
                format!("At most {BATCH_MAX} ids per batch."),
            ));
        }
        let body = serde_json::json!({ "conversation_ids": ids }).to_string();
        self.json(
            HttpMethod::Post,
            "/backend-api/conversations/batch",
            Some(&body),
        )
        .await
    }

    /// `searchConversations`: ChatGPT's own search, conversation results
    /// only. A read, although it's a POST.
    pub async fn global_search(
        &self,
        query: &str,
        limit: u64,
    ) -> Result<Vec<GlobalSearchHit>, ApiError> {
        #[derive(Deserialize)]
        struct Page {
            items: Vec<Value>,
        }
        let path = "/backend-api/global/search";
        let body = serde_json::json!({
            "entrypoint": "global_search",
            "limit": limit,
            "query": query,
            "source_requests": [{ "type": "conversation" }],
            "cursor": null,
        })
        .to_string();
        self.json::<Page>(HttpMethod::Post, path, Some(&body))
            .await?
            .items
            .into_iter()
            .filter(|item| item["source_type"] == "conversation")
            .map(|item| serde_json::from_value(item).map_err(|error| decode_error(path, &error)))
            .collect()
    }

    /// The saved memories, for `stats`.
    pub async fn memories(&self) -> Result<Vec<SavedMemory>, ApiError> {
        #[derive(Deserialize)]
        struct Memories {
            memories: Option<Vec<SavedMemory>>,
        }
        let path = "/backend-api/memories?include_memory_entries=true";
        self.json::<Memories>(HttpMethod::Get, path, None)
            .await?
            .memories
            .ok_or_else(|| {
                ApiError::new(
                    ErrorKind::Decode,
                    "ChatGPT returned an unexpected saved-memory list.",
                )
            })
    }
}

/// A failed call: the HTTP client's error, which the writes inspect (a 404,
/// a 500, a dropped connection), or a session failure.
#[derive(Debug)]
enum SendError {
    Http(HttpError),
    Api(ApiError),
}

impl From<HttpError> for SendError {
    fn from(error: HttpError) -> Self {
        Self::Http(error)
    }
}

impl From<ApiError> for SendError {
    fn from(error: ApiError) -> Self {
        Self::Api(error)
    }
}

impl From<SendError> for ApiError {
    fn from(error: SendError) -> Self {
        match error {
            SendError::Http(http) => http.into(),
            SendError::Api(api) => api,
        }
    }
}

async fn call(
    session: &Session,
    method: HttpMethod,
    path: &str,
    body: Option<&str>,
    resend: Resend,
) -> Result<String, HttpError> {
    let bearer = session.token.bearer();
    let mut headers = vec![
        ("authorization", bearer.expose()),
        ("accept", "application/json"),
    ];
    if body.is_some() {
        headers.push(("content-type", "application/json"));
    }
    session
        .http
        .send_with(method, path, &headers, body, resend)
        .await
}
