//! The two model APIs the TS CLI calls with a configured key
//! (`src/classify/model-api.ts` @ 1b8c950), as it calls them:
//!
//! - OpenAI's Responses API: `POST {base}/v1/responses`, `gpt-6-luna` at
//!   medium reasoning, not stored, with a strict JSON schema when the
//!   caller wants JSON back. Used for summaries and for Luna.
//! - Anthropic's Messages API: `POST {base}/v1/messages`,
//!   `claude-haiku-4-5`, at most 1800 tokens. Used for summaries.
//!
//! One attempt each, as the TS CLI's `fetch` makes. Unlike it, errors never
//! quote a response body (it can echo the request, which holds a chat),
//! and keys never show in `Debug` output. Each call gets
//! [`REQUEST_TIMEOUT`]; the TS CLI waits as long as it takes.

use std::fmt;
use std::time::Duration;

use impit::impit::{Impit, RedirectBehavior};
use impit::request::{ImpitBody, RequestOptions};
use reqwest::cookie::Jar;
use serde_json::{Value, json};

pub const OPENAI_BASE_URL: &str = "https://api.openai.com";
pub const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
/// The model both summaries and Luna use through OpenAI.
pub const OPENAI_MODEL: &str = "gpt-6-luna";
pub const ANTHROPIC_MODEL: &str = "claude-haiku-4-5";
/// A long chat's summary at medium reasoning can take minutes.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// An API key. `Debug` never prints it, and it has no `Display`.
#[derive(Clone)]
pub struct ApiKey(String);

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

/// A key that can't go in an HTTP header. The message never shows it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the {0} API key has characters an HTTP header can't carry (a line break?)")]
pub struct InvalidKey(pub &'static str);

impl ApiKey {
    /// The key, trimmed; `Ok(None)` when blank. `provider` names it in the
    /// error.
    pub fn new(value: &str, provider: &'static str) -> Result<Option<Self>, InvalidKey> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        reqwest::header::HeaderValue::from_str(&format!("Bearer {trimmed}"))
            .map_err(|_| InvalidKey(provider))?;
        Ok(Some(Self(trimmed.to_owned())))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The API answered with an error status. Its body is never kept.
    #[error("{host} returned {status}")]
    Status { host: String, status: u16 },
    #[error("{host} didn't answer within {} seconds", .timeout.as_secs())]
    Timeout { host: String, timeout: Duration },
    #[error("couldn't reach {host}: {why}")]
    Connection { host: String, why: String },
    /// The answer wasn't the API's shape, or said the call failed. Worded
    /// as the TS CLI words it, without the API's own message.
    #[error("{0}")]
    Answer(String),
    #[error("couldn't build the HTTP client: {0}")]
    Build(String),
}

/// What a call produced and what it used.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reply {
    pub text: String,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
}

struct Http {
    base: String,
    host: String,
    http: Impit<Jar>,
}

impl Http {
    fn new(base: &str) -> Result<Self, Error> {
        let base = base.trim_end_matches('/').to_owned();
        let host = base
            .split_once("://")
            .map_or(base.as_str(), |(_, rest)| rest)
            .split(['/', ':'])
            .next()
            .unwrap_or_default()
            .to_owned();
        // Without a browser fingerprint, building it never writes the
        // environment (docs/issues/impit-set-var-race.md).
        let http = Impit::<Jar>::builder()
            .with_redirect(RedirectBehavior::ManualRedirect)
            .with_default_timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| Error::Build(error.to_string()))?;
        Ok(Self { base, host, http })
    }

    /// `postJson`: one POST of `body`, the answer's JSON.
    async fn post(
        &self,
        path: &str,
        headers: Vec<(String, String)>,
        body: &Value,
    ) -> Result<Value, Error> {
        let mut all = vec![("content-type".to_owned(), "application/json".to_owned())];
        all.extend(headers);
        let options = RequestOptions {
            headers: all,
            ..RequestOptions::default()
        };
        let bytes = bytes::Bytes::from(body.to_string());
        let response = self
            .http
            .post(
                format!("{}{path}", self.base),
                Some(ImpitBody::Bytes(bytes)),
                Some(options),
            )
            .await
            .map_err(|error| self.transport(&error))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(Error::Status {
                host: self.host.clone(),
                status,
            });
        }
        let text = response.text().await.map_err(|error| Error::Connection {
            host: self.host.clone(),
            why: error.to_string(),
        })?;
        // serde's message can quote the body; only its category is kept.
        serde_json::from_str(&text).map_err(|error| {
            Error::Answer(format!(
                "{} answered with unreadable JSON ({:?} error)",
                self.host,
                error.classify()
            ))
        })
    }

    fn transport(&self, error: &impit::errors::ImpitError) -> Error {
        use impit::errors::ImpitError;
        match error {
            ImpitError::TimeoutException(_)
            | ImpitError::ConnectTimeout
            | ImpitError::ReadTimeout
            | ImpitError::WriteTimeout
            | ImpitError::PoolTimeout => Error::Timeout {
                host: self.host.clone(),
                timeout: REQUEST_TIMEOUT,
            },
            // Its message would quote the header, which holds the key.
            ImpitError::InvalidHeaderValue(_) => Error::Connection {
                host: self.host.clone(),
                why: "a request header has characters HTTP does not allow".to_owned(),
            },
            other => Error::Connection {
                host: self.host.clone(),
                why: other.to_string(),
            },
        }
    }
}

fn count(value: &Value, pointer: &str) -> u64 {
    value
        .pointer(pointer)
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && *n > 0.0)
        .map_or(0, |n| n as u64)
}

/// OpenAI's Responses API.
pub struct OpenAi {
    http: Http,
    key: ApiKey,
}

impl fmt::Debug for OpenAi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAi")
            .field("base", &self.http.base)
            .field("key", &self.key)
            .finish()
    }
}

impl OpenAi {
    pub fn new(base: &str, key: ApiKey) -> Result<Self, Error> {
        Ok(Self {
            http: Http::new(base)?,
            key,
        })
    }

    /// The request body `openAIText` sends, in its key order.
    pub fn body(instructions: &str, input: &str, schema: Option<&Value>) -> Value {
        let mut body = json!({
            "model": OPENAI_MODEL,
            "reasoning": { "effort": "medium" },
            "store": false,
            "instructions": instructions,
            "input": input,
        });
        if let (Some(schema), Some(object)) = (schema, body.as_object_mut()) {
            object.insert(
                "text".to_owned(),
                json!({ "format": { "type": "json_schema", "name": "chatgpt_cli_result", "strict": true, "schema": schema } }),
            );
        }
        body
    }

    /// `openAIText`: the output text, which with `schema` is JSON in it.
    pub async fn text(
        &self,
        instructions: &str,
        input: &str,
        schema: Option<&Value>,
    ) -> Result<Reply, Error> {
        let headers = vec![("authorization".to_owned(), format!("Bearer {}", self.key.0))];
        let answer = self
            .http
            .post(
                "/v1/responses",
                headers,
                &Self::body(instructions, input, schema),
            )
            .await?;
        let status = answer.get("status").and_then(Value::as_str);
        if status != Some("completed") {
            // The TS CLI adds the API's error message, which may quote the
            // request; it's left out here.
            return Err(Error::Answer(format!(
                "OpenAI response {}: no completed output",
                status.unwrap_or("unknown")
            )));
        }
        let text: String = answer
            .get("output")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("message"))
            .flat_map(|item| {
                item.get("content")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            })
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("output_text"))
            .map(|part| {
                part.get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect();
        if text.trim().is_empty() {
            return Err(Error::Answer("OpenAI returned no text.".to_owned()));
        }
        Ok(Reply {
            text,
            input_tokens: count(&answer, "/usage/input_tokens"),
            cached_input_tokens: count(&answer, "/usage/input_tokens_details/cached_tokens"),
            output_tokens: count(&answer, "/usage/output_tokens"),
        })
    }
}

/// Anthropic's Messages API.
pub struct Anthropic {
    http: Http,
    key: ApiKey,
}

impl fmt::Debug for Anthropic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Anthropic")
            .field("base", &self.http.base)
            .field("key", &self.key)
            .finish()
    }
}

impl Anthropic {
    pub fn new(base: &str, key: ApiKey) -> Result<Self, Error> {
        Ok(Self {
            http: Http::new(base)?,
            key,
        })
    }

    /// The request body `anthropicText` sends, in its key order.
    pub fn body(instructions: &str, input: &str) -> Value {
        json!({
            "model": ANTHROPIC_MODEL,
            "max_tokens": 1800,
            "system": instructions,
            "messages": [{ "role": "user", "content": input }],
        })
    }

    /// `anthropicText`: the reply's text.
    pub async fn text(&self, instructions: &str, input: &str) -> Result<Reply, Error> {
        let headers = vec![
            ("x-api-key".to_owned(), self.key.0.clone()),
            ("anthropic-version".to_owned(), "2023-06-01".to_owned()),
        ];
        let answer = self
            .http
            .post("/v1/messages", headers, &Self::body(instructions, input))
            .await?;
        if answer.get("stop_reason").and_then(Value::as_str) == Some("max_tokens") {
            return Err(Error::Answer(
                "Anthropic summary was truncated at max_tokens.".to_owned(),
            ));
        }
        let text: String = answer
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .map(|part| {
                part.get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect();
        if text.trim().is_empty() {
            return Err(Error::Answer("Anthropic returned no text.".to_owned()));
        }
        Ok(Reply {
            text,
            input_tokens: count(&answer, "/usage/input_tokens"),
            cached_input_tokens: 0,
            output_tokens: count(&answer, "/usage/output_tokens"),
        })
    }
}

#[cfg(test)]
mod tests;
