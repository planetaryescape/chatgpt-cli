//! A small client for TypeSafe's System One API, the one call the TS CLI
//! makes through `@typesafe-ai/sdk` 0.6.0 (`TypeSafeClient.systemOne`, read
//! from its `dist/index.mjs`):
//!
//! - `POST {base}/v1/systemone` with `Authorization: Bearer <key>` and the
//!   JSON body `{ "state": …, "questions": …, "model": "jev-latest" }`, in
//!   that key order;
//! - the answer is `{ "answers": { <question>: … }, "usage": { "input_tokens": n }, "model": … }`;
//! - each attempt gets 10 seconds; 408, 429 and 5xx answers, timeouts and
//!   dropped connections are retried twice, after `retry-after-ms` or
//!   `retry-after` when the server asks for at most a minute, otherwise
//!   after 0.5 s, then 1 s.
//!
//! Unlike the SDK, errors never quote the response body, and the API key
//! never shows in `Debug` output.

use std::fmt;
use std::time::Duration;

use impit::impit::{Impit, RedirectBehavior};
use impit::request::{ImpitBody, RequestOptions};
use reqwest::cookie::Jar;
use serde::Serialize;
use serde_json::Value;

pub const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
/// The SDK's default model.
pub const DEFAULT_MODEL: &str = "jev-latest";
const PATH: &str = "/v1/systemone";

/// A TypeSafe API key. `Debug` never prints it, and it has no `Display`.
#[derive(Clone)]
pub struct ApiKey(String);

impl ApiKey {
    /// The key, trimmed; `None` when blank.
    pub fn new(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| Self(trimmed.to_owned()))
    }

    fn bearer(&self) -> String {
        format!("Bearer {}", self.0)
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// TypeSafe answered with an error status after the retries. The body,
    /// which can echo the request, is never kept.
    #[error("TypeSafe answered {status}{}", hint(*status))]
    Status { status: u16 },
    #[error("TypeSafe didn't answer within {} seconds", .0.as_secs())]
    Timeout(Duration),
    #[error("couldn't reach TypeSafe: {0}")]
    Connection(String),
    /// The answer wasn't System One's shape.
    #[error("TypeSafe's answer wasn't what the CLI expects ({0})")]
    Decode(String),
    #[error("couldn't build the HTTP client for TypeSafe: {0}")]
    Build(String),
}

fn hint(status: u16) -> &'static str {
    match status {
        401 | 403 => " (check the Jev API key: `chatgpt configure jev` or TYPESAFE_API_KEY)",
        429 => " (rate limited; try again in a minute)",
        _ => "",
    }
}

/// When to try a failed call again, as the SDK's `DEFAULT_RETRY_POLICY`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_retries: u32,
    /// Doubles each attempt, up to `max_backoff`.
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    /// The longest server-requested wait honoured; a longer one falls back
    /// to the backoff.
    pub max_retry_after: Duration,
    /// Per attempt, including reading the body.
    pub timeout: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 2,
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(5),
            max_retry_after: Duration::from_secs(60),
            timeout: Duration::from_secs(10),
        }
    }
}

impl RetryPolicy {
    fn backoff(&self, attempt: u32) -> Duration {
        self.initial_backoff
            .saturating_mul(2u32.saturating_pow(attempt))
            .min(self.max_backoff)
    }
}

/// System One's answer: Jev's answers as sent (key order kept, numbers
/// exact), and the input tokens it billed.
#[derive(Clone, Debug, PartialEq)]
pub struct SystemOneResult {
    pub answers: Value,
    pub input_tokens: u64,
}

#[derive(Serialize)]
struct Body<'a> {
    state: &'a Value,
    questions: &'a Value,
    model: &'a str,
}

pub struct Client {
    base: String,
    key: ApiKey,
    policy: RetryPolicy,
    http: Impit<Jar>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base", &self.base)
            .field("key", &self.key)
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// A client for the API at `base` (no trailing slash needed). Without a
    /// browser fingerprint, building it never writes the environment.
    pub fn new(base: &str, key: ApiKey, policy: RetryPolicy) -> Result<Self, Error> {
        let http = Impit::<Jar>::builder()
            .with_redirect(RedirectBehavior::ManualRedirect)
            .with_default_timeout(policy.timeout)
            .build()
            .map_err(|error| Error::Build(error.to_string()))?;
        Ok(Self {
            base: base.trim_end_matches('/').to_owned(),
            key,
            policy,
            http,
        })
    }

    /// Ask `questions` about `state` (`systemOne({ state, questions })`).
    pub async fn system_one(
        &self,
        state: &Value,
        questions: &Value,
    ) -> Result<SystemOneResult, Error> {
        let body = serde_json::to_string(&Body {
            state,
            questions,
            model: DEFAULT_MODEL,
        })
        .map_err(|error| Error::Decode(error.to_string()))?;
        let text = self.post(&body).await?;
        parse(&text)
    }

    async fn post(&self, body: &str) -> Result<String, Error> {
        let url = format!("{}{PATH}", self.base);
        let mut attempt = 0;
        loop {
            let mut headers = vec![
                ("Authorization".to_owned(), self.key.bearer()),
                ("Accept".to_owned(), "application/json".to_owned()),
                ("Content-Type".to_owned(), "application/json".to_owned()),
                (
                    "User-Agent".to_owned(),
                    concat!("chatgpt-cli/", env!("CARGO_PKG_VERSION")).to_owned(),
                ),
            ];
            if attempt > 0 {
                headers.push(("X-TypeSafe-Retry-Count".to_owned(), attempt.to_string()));
            }
            let options = RequestOptions {
                headers,
                ..RequestOptions::default()
            };
            let request_body = ImpitBody::Bytes(bytes::Bytes::copy_from_slice(body.as_bytes()));
            let sent = self
                .http
                .post(url.clone(), Some(request_body), Some(options))
                .await;
            let left = attempt < self.policy.max_retries;
            let response = match sent {
                Ok(response) => response,
                Err(error) => {
                    let error = transport_error(&error, self.policy.timeout);
                    if !left {
                        return Err(error);
                    }
                    tokio::time::sleep(self.policy.backoff(attempt)).await;
                    attempt += 1;
                    continue;
                }
            };
            let status = response.status().as_u16();
            if (200..300).contains(&status) {
                return response
                    .text()
                    .await
                    .map_err(|error| Error::Connection(error.to_string()));
            }
            let retryable = status == 408 || status == 429 || (500..600).contains(&status);
            if !retryable || !left {
                return Err(Error::Status { status });
            }
            let wait = retry_after(response.headers())
                .filter(|wait| *wait <= self.policy.max_retry_after)
                .unwrap_or_else(|| self.policy.backoff(attempt));
            drop(response);
            tokio::time::sleep(wait).await;
            attempt += 1;
        }
    }
}

/// `retry-after-ms`, else `retry-after` in seconds. HTTP dates count as
/// absent: TypeSafe sends seconds.
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .and_then(|text| text.trim().parse::<f64>().ok())
            .filter(|number| number.is_finite() && *number >= 0.0)
    };
    if let Some(ms) = header("retry-after-ms") {
        return Duration::try_from_secs_f64(ms / 1000.0).ok();
    }
    header("retry-after").and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
}

fn transport_error(error: &impit::errors::ImpitError, timeout: Duration) -> Error {
    use impit::errors::ImpitError;
    match error {
        ImpitError::TimeoutException(_)
        | ImpitError::ConnectTimeout
        | ImpitError::ReadTimeout
        | ImpitError::WriteTimeout
        | ImpitError::PoolTimeout => Error::Timeout(timeout),
        // impit's messages name the failure, not the request.
        other => Error::Connection(other.to_string()),
    }
}

fn parse(text: &str) -> Result<SystemOneResult, Error> {
    // serde's message can quote the body; only its category is kept.
    let value: Value = serde_json::from_str(text)
        .map_err(|error| Error::Decode(format!("{:?} error", error.classify())))?;
    let answers = value
        .get("answers")
        .filter(|answers| answers.is_object())
        .cloned()
        .ok_or_else(|| Error::Decode("no answers".to_owned()))?;
    let input_tokens = value
        .pointer("/usage/input_tokens")
        .and_then(Value::as_f64)
        .ok_or_else(|| Error::Decode("no usage".to_owned()))?;
    Ok(SystemOneResult {
        answers,
        input_tokens: input_tokens.max(0.0) as u64,
    })
}

#[cfg(test)]
mod tests;
