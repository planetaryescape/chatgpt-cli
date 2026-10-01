//! A fake chatgpt.com for tests: the session exchange, the conversation
//! lists (paged, newest update first), single and batch conversation reads,
//! and saved memories, all answered from shared state a test can change
//! between calls. It runs on its own runtime, so blocking tests (driving the
//! real binary) can use it.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

pub const ACCESS_TOKEN: &str = "fake-access-token";
pub const COOKIE: &str = "__Secure-next-auth.session-token=fake-session";
/// A second ChatGPT account, with chats of its own
/// (`State::other_account_chats`).
pub const OTHER_COOKIE: &str = "__Secure-next-auth.session-token=other-session";
const OTHER_ACCESS_TOKEN: &str = "other-access-token";

/// A chat the fake serves.
#[derive(Clone, Debug)]
pub struct Chat {
    pub id: String,
    pub title: String,
    /// ISO 8601, as the list returns it.
    pub create_time: String,
    pub update_time: String,
    pub archived: bool,
    pub pinned: bool,
    pub gizmo_id: Option<String>,
    /// The user's one message.
    pub text: String,
}

impl Chat {
    pub fn new(id: &str, title: &str, update_time: &str) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            create_time: "2026-01-01T00:00:00.000000Z".into(),
            update_time: update_time.into(),
            archived: false,
            pinned: false,
            gizmo_id: None,
            text: format!("Hello from {id}"),
        }
    }

    fn list_item(&self) -> Value {
        json!({
            "id": self.id, "title": self.title, "create_time": self.create_time,
            "update_time": self.update_time, "is_archived": self.archived,
            "pinned_time": self.pinned.then_some(self.update_time.as_str()),
            "gizmo_id": self.gizmo_id,
        })
    }

    fn mapping(&self) -> Value {
        json!({
            "root": { "id": "root", "parent": null, "children": ["m1"], "message": null },
            "m1": { "id": "m1", "parent": "root", "children": [], "message": {
                "author": { "role": "user" }, "create_time": 1,
                "content": { "content_type": "text", "parts": [self.text] } } },
        })
    }

    /// The single-chat endpoint: epoch seconds.
    fn detail(&self) -> Value {
        json!({
            "conversation_id": self.id, "title": self.title,
            "create_time": seconds(&self.create_time), "update_time": seconds(&self.update_time),
            "is_archived": self.archived, "pinned_time": null, "gizmo_id": self.gizmo_id,
            "default_model_slug": "gpt-4", "mapping": self.mapping(), "current_node": "m1",
        })
    }

    /// A batch item: `id`, ISO times, no model.
    fn batch_item(&self) -> Value {
        json!({
            "id": self.id, "title": self.title, "create_time": self.create_time,
            "update_time": self.update_time, "is_archived": self.archived,
            "mapping": self.mapping(), "current_node": "m1",
        })
    }
}

fn seconds(iso: &str) -> f64 {
    DateTime::parse_from_rfc3339(iso)
        .map(|time| time.with_timezone(&Utc).timestamp_micros() as f64 / 1e6)
        .unwrap_or(0.0)
}

#[derive(Default)]
pub struct State {
    pub chats: Vec<Chat>,
    /// Ids the archived list leaves out although the chat exists.
    pub omitted_from_lists: HashSet<String>,
    /// `None`: the memories endpoint fails with a 500.
    pub memories: Option<Vec<Value>>,
    /// Answer this many next requests with 429 (`retry-after` seconds).
    pub rate_limit: Option<(u32, u64)>,
    /// `METHOD path` of every request, in order.
    pub calls: Vec<String>,
    /// How many session exchanges happened.
    pub session_exchanges: u32,
    /// Reject the next request's token with a 401, as an expired one is.
    pub expire_token: bool,
    /// The second account's chats.
    pub other_account_chats: Vec<Chat>,
    /// Hold every conversation-list answer this long.
    pub list_delay_ms: u64,
    /// Answer this many next batch requests with 429 (`retry-after`
    /// seconds).
    pub rate_limit_batch: Option<(u32, u64)>,
}

pub struct FakeChatGpt {
    pub url: String,
    state: Arc<Mutex<State>>,
    // Owns the server's tasks; dropping it stops the server.
    _runtime: tokio::runtime::Runtime,
    _server: MockServer,
}

struct Handler {
    state: Arc<Mutex<State>>,
    route: Route,
}

#[derive(Clone, Copy)]
enum Route {
    Session,
    List,
    Detail,
    Batch,
    Memories,
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

fn query(request: &Request, name: &str) -> Option<String> {
    request
        .url
        .query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

impl Respond for Handler {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let mut state = lock(&self.state);
        state.calls.push(format!(
            "{} {}{}",
            request.method,
            request.url.path(),
            request
                .url
                .query()
                .map(|query| format!("?{query}"))
                .unwrap_or_default()
        ));
        if let Some((left, retry_after)) = state.rate_limit
            && left > 0
        {
            state.rate_limit = (left > 1).then_some((left - 1, retry_after));
            return ResponseTemplate::new(429)
                .insert_header("retry-after", retry_after.to_string());
        }
        let header = |name: &str| {
            request
                .headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        let (token, user, other) = match header("cookie").as_deref() {
            Some(COOKIE) => (ACCESS_TOKEN, "user-a", false),
            Some(OTHER_COOKIE) => (OTHER_ACCESS_TOKEN, "user-b", true),
            _ => return ResponseTemplate::new(401),
        };
        if let Route::Session = self.route {
            state.session_exchanges += 1;
            return ResponseTemplate::new(200)
                .set_body_json(json!({ "accessToken": token, "user": { "id": user } }));
        }
        if header("authorization").as_deref() != Some(&format!("Bearer {token}")) {
            return ResponseTemplate::new(401);
        }
        if let (Route::Batch, Some((left, retry_after))) = (self.route, state.rate_limit_batch)
            && left > 0
        {
            state.rate_limit_batch = (left > 1).then_some((left - 1, retry_after));
            return ResponseTemplate::new(429)
                .insert_header("retry-after", retry_after.to_string());
        }
        if state.expire_token {
            state.expire_token = false;
            return ResponseTemplate::new(401).set_body_string("{\"detail\":\"token expired\"}");
        }
        let delay = std::time::Duration::from_millis(state.list_delay_ms);
        let chats = if other {
            &state.other_account_chats
        } else {
            &state.chats
        };
        match self.route {
            Route::Session => ResponseTemplate::new(500),
            Route::List => {
                let archived = query(request, "is_archived").as_deref() == Some("true");
                let offset: usize = query(request, "offset")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let limit: usize = query(request, "limit")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(100);
                let mut listed: Vec<&Chat> = chats
                    .iter()
                    .filter(|chat| chat.archived == archived)
                    .filter(|chat| !state.omitted_from_lists.contains(&chat.id))
                    .collect();
                listed.sort_by(|a, b| b.update_time.cmp(&a.update_time));
                let items: Vec<Value> = listed
                    .into_iter()
                    .skip(offset)
                    .take(limit)
                    .map(Chat::list_item)
                    .collect();
                let total = offset + items.len() + 1;
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "items": items, "total": total }))
                    .set_delay(delay)
            }
            Route::Detail => {
                let id = request.url.path().rsplit('/').next().unwrap_or("");
                match chats.iter().find(|chat| chat.id == id) {
                    Some(chat) => ResponseTemplate::new(200).set_body_json(chat.detail()),
                    None => ResponseTemplate::new(404)
                        .set_body_json(json!({ "detail": "conversation_deleted" })),
                }
            }
            Route::Batch => {
                let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
                let ids: Vec<String> = body["conversation_ids"]
                    .as_array()
                    .map(|ids| {
                        ids.iter()
                            .filter_map(|id| id.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                let items: Vec<Value> = ids
                    .iter()
                    .filter_map(|id| chats.iter().find(|chat| &chat.id == id))
                    .map(Chat::batch_item)
                    .collect();
                ResponseTemplate::new(200).set_body_json(items)
            }
            Route::Memories => match &state.memories {
                Some(memories) => {
                    ResponseTemplate::new(200).set_body_json(json!({ "memories": memories }))
                }
                None => ResponseTemplate::new(500).set_body_string("{\"detail\":\"secret body\"}"),
            },
        }
    }
}

impl FakeChatGpt {
    pub fn start(chats: Vec<Chat>) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("fake runtime");
        let state = Arc::new(Mutex::new(State {
            chats,
            memories: Some(Vec::new()),
            ..State::default()
        }));
        let server = runtime.block_on(async {
            let server = MockServer::start().await;
            let routes = [
                (
                    Mock::given(method("GET")).and(path("/api/auth/session")),
                    Route::Session,
                ),
                (
                    Mock::given(method("GET")).and(path("/backend-api/conversations")),
                    Route::List,
                ),
                (
                    Mock::given(method("GET"))
                        .and(path_regex(r"^/backend-api/conversation/[^/]+$")),
                    Route::Detail,
                ),
                (
                    Mock::given(method("POST")).and(path("/backend-api/conversations/batch")),
                    Route::Batch,
                ),
                (
                    Mock::given(method("GET")).and(path("/backend-api/memories")),
                    Route::Memories,
                ),
            ];
            for (mock, route) in routes {
                mock.respond_with(Handler {
                    state: Arc::clone(&state),
                    route,
                })
                .mount(&server)
                .await;
            }
            server
        });
        Self {
            url: server.uri(),
            state,
            _runtime: runtime,
            _server: server,
        }
    }

    /// Change what the fake serves.
    pub fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    /// Requests seen so far, `METHOD /path?query`.
    pub fn calls(&self) -> Vec<String> {
        self.state().calls.clone()
    }
}
