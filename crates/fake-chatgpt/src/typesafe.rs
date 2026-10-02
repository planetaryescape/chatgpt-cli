//! A fake TypeSafe System One API (`POST /v1/systemone`) for the Jev
//! guard: it answers from the chat's title, so a test decides each verdict
//! by naming its chats, and it keeps every request body to compare.
//!
//! A title containing `junk` gets answers that make a confident delete,
//! `maybe` an unsure one, `stale` a time-bound chat whose moment is still
//! current; `product` a product brainstorm (which Luna reviews); anything
//! else is a kept writing brainstorm. The follow-up questions
//! (`personal_record_lost`, …) settle an unsure chat as archive; the
//! saved-memory questions keep a memory mentioning `tea` and send the
//! rest to Luna.

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
    /// Answer with a score that isn't a number (it echoes text instead).
    pub malformed: bool,
    /// Hold every answer this long.
    pub delay_ms: u64,
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
    // A product brainstorm, which Luna reviews.
    let product = lower.contains("product");
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
        "product_idea": noul(if product { 0.8 } else { 1e-7 }),
        "brainstorm_for": { "type": "choice", "choice": if product { "product" } else if brainstorming > 0.5 { "writing" } else { "none" }, "confidence": 0.8 },
        "topic": { "type": "choice", "choice": if product { "side_projects" } else { "other" }, "confidence": 0.7 },
    })
}

/// The follow-up's answers: nothing would be lost, so an unsure chat
/// settles as archive.
pub fn deep_answers() -> Value {
    json!({
        "personal_record_lost": noul(0.1), "reusable_artifact_lost": noul(0.1),
        "original_thinking_lost": noul(0.05), "work_to_resume": noul(0.05),
        "creative_idea_lost": noul(0.05), "reaskable_without_loss": noul(0.6),
        "worth_finding_again": { "type": "score", "score": 1.0, "confidence": 0.8 },
    })
}

/// A saved memory's quick answers, from its content.
pub fn memory_answers(content: &str) -> Value {
    let lasting = if content.to_lowercase().contains("tea") {
        3.0
    } else {
        1.0
    };
    json!({
        "lasting_value": { "type": "score", "score": lasting, "confidence": 0.9 },
        "expired": noul(0.1), "superseded": noul(0.05), "redundant": noul(0.05),
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
        let mut answers = if body.pointer("/questions/personal_record_lost").is_some() {
            deep_answers()
        } else if body.pointer("/questions/lasting_value").is_some() {
            memory_answers(
                body.pointer("/state/memory/content")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
        } else {
            answers_for(title)
        };
        if state.malformed {
            answers["worth_keeping"]["score"] = json!("SENTINEL private transcript fragment");
        }
        ResponseTemplate::new(200)
            .set_delay(std::time::Duration::from_millis(state.delay_ms))
            .set_body_json(json!({
                "answers": answers,
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
