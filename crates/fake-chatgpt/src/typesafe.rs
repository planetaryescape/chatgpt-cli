//! A fake TypeSafe System One API (`POST /v1/systemone`) for the Jev
//! guard: it answers from the chat's title, so a test decides each verdict
//! by naming its chats, and it keeps every request body to compare.
//!
//! A title containing `junk` gets answers that make a confident delete,
//! `maybe` an unsure one, `stale` a time-bound chat whose moment is still
//! current; anything else is a kept brainstorm.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

pub const API_KEY: &str = "ts-test-key";
/// The tokens every answer bills, so cost lines are the same whatever
/// order the calls finish in.
pub const INPUT_TOKENS: u64 = 1234;

#[derive(Default)]
pub struct TypeSafeState {
    /// Every request body, as sent.
    pub bodies: Vec<String>,
    /// Answer this many next requests with this status.
    pub fail: Option<(u32, u16)>,
}

pub struct FakeTypeSafe {
    pub url: String,
    state: Arc<Mutex<TypeSafeState>>,
    _runtime: tokio::runtime::Runtime,
    _server: MockServer,
}

fn noul(value: f64) -> Value {
    json!({ "type": "noul", "noul": value })
}

/// Jev's answers for a chat titled `title`.
pub fn answers_for(title: &str) -> Value {
    let lower = title.to_lowercase();
    let (worth, nothing, re_askable, brainstorming, personal, time_bound, overtaken) =
        if lower.contains("junk") {
            (0.2, 0.95, 0.9, 0.05, 0.05, 0.1, 0.1)
        } else if lower.contains("maybe") {
            (0.9, 0.5, 0.75, 0.05, 0.45, 0.1, 0.1)
        } else if lower.contains("stale") {
            (1.2, 0.3, 0.6, 0.05, 0.1, 0.9, 0.1)
        } else {
            (2.8, 0.02, 0.1, 0.95, 0.2, 0.1, 0.1)
        };
    json!({
        "worth_keeping": { "type": "score", "score": worth, "confidence": 0.9400000000000001,
            "legend": { "0": "None" }, "probabilities": { "0": 0.1 } },
        "nothing_there": noul(nothing),
        "unfinished": noul(0.05),
        "personal_record": noul(personal),
        "re_askable": noul(re_askable),
        "time_bound": noul(time_bound),
        "overtaken_by_time": noul(overtaken),
        "brainstorming": noul(brainstorming),
        "product_idea": noul(1e-7),
        "brainstorm_for": { "type": "choice", "choice": if brainstorming > 0.5 { "writing" } else { "none" }, "confidence": 0.8 },
        "topic": { "type": "choice", "choice": "other", "confidence": 0.7 },
    })
}

struct Handler(Arc<Mutex<TypeSafeState>>);

fn lock(state: &Mutex<TypeSafeState>) -> MutexGuard<'_, TypeSafeState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Respond for Handler {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let mut state = lock(&self.0);
        state
            .bodies
            .push(String::from_utf8_lossy(&request.body).into_owned());
        if request
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            != Some(&format!("Bearer {API_KEY}"))
        {
            return ResponseTemplate::new(401)
                .set_body_json(json!({ "detail": "secret: invalid key" }));
        }
        if let Some((left, status)) = state.fail
            && left > 0
        {
            state.fail = (left > 1).then_some((left - 1, status));
            return ResponseTemplate::new(status).set_body_string("{\"detail\":\"secret\"}");
        }
        let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
        let title = body
            .pointer("/state/conversation/title")
            .and_then(Value::as_str)
            .unwrap_or_default();
        ResponseTemplate::new(200).set_body_json(json!({
            "answers": answers_for(title),
            "usage": { "input_tokens": INPUT_TOKENS },
            "model": "jev-test",
        }))
    }
}

impl FakeTypeSafe {
    pub fn start() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("fake runtime");
        let state = Arc::new(Mutex::new(TypeSafeState::default()));
        let server = runtime.block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/v1/systemone"))
                .respond_with(Handler(Arc::clone(&state)))
                .mount(&server)
                .await;
            server
        });
        Self {
            url: server.uri(),
            state,
            _runtime: runtime,
            _server: server,
        }
    }

    pub fn state(&self) -> MutexGuard<'_, TypeSafeState> {
        lock(&self.state)
    }

    /// How many calls it answered (or refused).
    pub fn calls(&self) -> usize {
        self.state().bodies.len()
    }
}
