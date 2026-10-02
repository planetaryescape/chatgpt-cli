//! Fake OpenAI Responses and Anthropic Messages APIs, answering from
//! [`crate::model_answers`] and keeping every request body to compare.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use crate::model_answers::{self, CACHED_INPUT_TOKENS, INPUT_TOKENS, OUTPUT_TOKENS};

pub const OPENAI_KEY: &str = "sk-test-openai";
pub const ANTHROPIC_KEY: &str = "ant-test-key";

#[derive(Default)]
pub struct ModelState {
    /// Every request body, as sent.
    pub bodies: Vec<String>,
    /// Answer every request with this status.
    pub fail: Option<u16>,
}

fn lock(state: &Mutex<ModelState>) -> MutexGuard<'_, ModelState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Clone, Copy)]
enum Api {
    OpenAi,
    Anthropic,
}

struct Handler(Api, Arc<Mutex<ModelState>>);

impl Respond for Handler {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let mut state = lock(&self.1);
        state
            .bodies
            .push(String::from_utf8_lossy(&request.body).into_owned());
        let header = |name: &str| {
            request
                .headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        let authorised = match self.0 {
            Api::OpenAi => header("authorization") == Some(format!("Bearer {OPENAI_KEY}")),
            Api::Anthropic => header("x-api-key").as_deref() == Some(ANTHROPIC_KEY),
        };
        if !authorised {
            return ResponseTemplate::new(401).set_body_string("{\"error\":\"secret\"}");
        }
        if let Some(status) = state.fail {
            return ResponseTemplate::new(status).set_body_string("{\"error\":\"secret\"}");
        }
        let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
        match self.0 {
            Api::OpenAi => {
                let input = body["input"].as_str().unwrap_or_default();
                let text = match body.pointer("/text/format/schema") {
                    Some(schema) => {
                        let parsed: Value = serde_json::from_str(input).unwrap_or(Value::Null);
                        model_answers::luna(schema, &parsed).to_string()
                    }
                    None => model_answers::summary(input),
                };
                ResponseTemplate::new(200).set_body_json(json!({
                    "status": "completed",
                    "output": [{ "type": "message", "content": [{ "type": "output_text", "text": text }] }],
                    "usage": { "input_tokens": INPUT_TOKENS, "output_tokens": OUTPUT_TOKENS,
                        "input_tokens_details": { "cached_tokens": CACHED_INPUT_TOKENS } },
                }))
            }
            Api::Anthropic => {
                let input = body
                    .pointer("/messages/0/content")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                ResponseTemplate::new(200).set_body_json(json!({
                    "content": [{ "type": "text", "text": model_answers::summary(input) }],
                    "usage": { "input_tokens": INPUT_TOKENS, "output_tokens": OUTPUT_TOKENS },
                    "stop_reason": "end_turn",
                }))
            }
        }
    }
}

/// One fake model API on its own runtime.
pub struct FakeModelApi {
    pub url: String,
    state: Arc<Mutex<ModelState>>,
    _runtime: tokio::runtime::Runtime,
    _server: MockServer,
}

impl FakeModelApi {
    fn start(api: Api) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("fake runtime");
        let state = Arc::new(Mutex::new(ModelState::default()));
        let route = match api {
            Api::OpenAi => "/v1/responses",
            Api::Anthropic => "/v1/messages",
        };
        let server = runtime.block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path(route))
                .respond_with(Handler(api, Arc::clone(&state)))
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

    pub fn openai() -> Self {
        Self::start(Api::OpenAi)
    }

    pub fn anthropic() -> Self {
        Self::start(Api::Anthropic)
    }

    pub fn state(&self) -> MutexGuard<'_, ModelState> {
        lock(&self.state)
    }

    pub fn calls(&self) -> usize {
        self.state().bodies.len()
    }
}
