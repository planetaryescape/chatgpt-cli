//! The Chrome-impersonating HTTP client, with the TS CLI's retry policy.
//!
//! Ported from the TS CLI's `src/api/client.ts` @ 1b8c950.
//!
//! Cloudflare challenges any client whose TLS/HTTP2 fingerprint isn't a real
//! browser's (plain reqwest and curl both get 403). impit impersonates
//! Chrome's. Even then some fresh connections get a challenge; a new
//! connection usually passes, so challenges retry on a fresh client.

use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use impit::impit::{Impit, PSEUDOHEADERS_ORDER_ENV, RedirectBehavior};
use impit::request::{ImpitBody, RequestOptions};
use reqwest::StatusCode;
use reqwest::cookie::Jar;
use reqwest::header::{HeaderMap, HeaderValue};

pub const CHATGPT_BASE: &str = "https://chatgpt.com";

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("Cloudflare kept challenging requests to {path}. Wait a minute and retry.")]
    Challenged { path: String },
    /// `snippet` is the start of the response body, for the user's own
    /// terminal. It is always `None` for `/api/auth/` paths, whose bodies carry
    /// tokens. A long-running process must still not log it verbatim: it can
    /// hold conversation content.
    #[error("{status} from {path}{}", snippet.as_deref().map(|text| format!(": {text}")).unwrap_or_default())]
    Status {
        status: StatusCode,
        path: String,
        snippet: Option<String>,
    },
    /// The server asked for a longer pause than `RetryPolicy::max_retry_after`,
    /// so the client stops instead of retrying early.
    #[error("rate limited by ChatGPT; it asked to wait {}s before retrying {path}", retry_after.as_secs())]
    RateLimited { path: String, retry_after: Duration },
    #[error("Request to {path} failed: {source}")]
    Transport {
        path: String,
        source: impit::errors::ImpitError,
    },
    #[error("Could not build the HTTP client: {0}")]
    Build(impit::errors::ImpitError),
    /// impit's own error would echo the value, which may be the cookie or a token.
    #[error("A request header for {path} has characters HTTP does not allow")]
    InvalidHeaderValue { path: String },
    /// Building a client would write the process environment
    /// (docs/issues/impit-set-var-race.md). Call [`prepare_environment`]'s
    /// check before any thread starts.
    #[error(
        "{PSEUDO_HEADER_ORDER_ENV} must be set to Chrome's HTTP/2 pseudo-header order before the HTTP client is built"
    )]
    EnvironmentNotPrepared,
}

/// The variable apify's h2 fork reads for the HTTP/2 pseudo-header order.
pub const PSEUDO_HEADER_ORDER_ENV: &str = PSEUDOHEADERS_ORDER_ENV;

/// Chrome 124's pseudo-header order, the value impit would write into
/// [`PSEUDO_HEADER_ORDER_ENV`] when it builds a client.
pub fn pseudo_header_order() -> String {
    chrome_fingerprint().http2.pseudo_header_order.join(",")
}

/// Whether the environment already holds [`pseudo_header_order`], so that
/// building a client only reads it. A process sets it before it starts any
/// thread (the daemon re-executes itself with it), never afterwards.
pub fn environment_prepared() -> bool {
    std::env::var(PSEUDO_HEADER_ORDER_ENV).ok() == Some(pseudo_header_order())
}

fn chrome_fingerprint() -> impit::fingerprint::BrowserFingerprint {
    impit::fingerprint::database::chrome_124::fingerprint()
}

const SNIPPET_CHARS: usize = 300;

/// Auth endpoint bodies carry session and access tokens, so errors never show them.
fn shows_snippet(path: &str) -> bool {
    !path.starts_with("/api/auth/")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryReason {
    Challenge,
    RateLimited,
    Gateway(StatusCode),
}

/// One retry about to happen, for callers that report progress.
#[derive(Debug, Clone, Copy)]
pub struct RetryEvent {
    pub reason: RetryReason,
    /// Zero-based: the attempt that just failed.
    pub attempt: u32,
    pub wait: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_retries: u32,
    /// Doubles each attempt, for gateway errors and challenges.
    pub base_backoff: Duration,
    /// Doubles each attempt, for 429. ChatGPT's 429s outlast a gateway blip.
    pub rate_limit_backoff: Duration,
    /// The longest `retry-after` the client will wait out. A longer one ends
    /// the call with `HttpError::RateLimited` (on 429) rather than retrying
    /// before the server allows it.
    pub max_retry_after: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            max_retries: 4,
            base_backoff: Duration::from_secs(1),
            rate_limit_backoff: Duration::from_secs(5),
            max_retry_after: Duration::from_secs(60),
        }
    }
}

impl RetryPolicy {
    /// Fails on the first non-2xx response, to measure raw challenge rates.
    pub fn none() -> Self {
        RetryPolicy {
            max_retries: 0,
            ..RetryPolicy::default()
        }
    }

    fn backoff(&self, reason: RetryReason, attempt: u32) -> Duration {
        let base = match reason {
            RetryReason::RateLimited => self.rate_limit_backoff,
            _ => self.base_backoff,
        };
        base.saturating_mul(2u32.saturating_pow(attempt))
    }
}

/// A `retry-after` in seconds. HTTP dates, zero and garbage count as absent;
/// a value too large for `Duration` saturates.
fn parse_retry_after(value: &str) -> Option<Duration> {
    let seconds = value.trim().parse::<f64>().ok()?;
    (seconds.is_finite() && seconds > 0.0)
        .then(|| Duration::try_from_secs_f64(seconds).unwrap_or(Duration::MAX))
}

/// A secret header value: Debug never prints it.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: String) -> Self {
        Secret(value)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

type RetryObserver = Box<dyn Fn(&RetryEvent) + Send + Sync>;

/// The methods the client sends. POST is also a read (the batch read and
/// ChatGPT's search); PATCH, DELETE and the other POSTs change chats,
/// projects and memories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Patch,
    Delete,
}

/// Whether a request may go out again after ChatGPT may have acted on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resend {
    /// Reads and idempotent writes: challenges, 429s and gateway errors are
    /// all retried.
    Allowed,
    /// Writes that mustn't happen twice (a delete, a rename, a new
    /// project): only answers that show the request was turned away before
    /// ChatGPT acted on it are retried, a Cloudflare challenge or a 429. A
    /// gateway error or a dropped connection comes back as it is, since the
    /// write may have applied.
    OnlyIfRefused,
}

/// Whether a non-2xx response is worth retrying, and why.
///
/// A challenge is a 403 with `cf-mitigated: challenge`. A 500 is an
/// application error (e.g. legacy-chat rename) and repeats, so only gateway
/// errors count as transient.
pub fn retry_reason(status: StatusCode, headers: &HeaderMap) -> Option<RetryReason> {
    match status {
        StatusCode::FORBIDDEN
            if headers
                .get("cf-mitigated")
                .is_some_and(|value| value == "challenge") =>
        {
            Some(RetryReason::Challenge)
        }
        StatusCode::TOO_MANY_REQUESTS => Some(RetryReason::RateLimited),
        StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT => {
            Some(RetryReason::Gateway(status))
        }
        _ => None,
    }
}

/// Sends every request with the browser's cookies, impersonating Chrome.
///
/// Shareable across tasks: a challenge swaps in a fresh impit client, and
/// requests already in flight finish on the old one.
pub struct HttpClient {
    base: String,
    cookie_header: Secret,
    policy: RetryPolicy,
    impit: RwLock<Arc<Impit<Jar>>>,
    on_retry: Option<RetryObserver>,
}

impl fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpClient")
            .field("base", &self.base)
            .field("cookie_header", &self.cookie_header)
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

/// Chrome 124, the fingerprint the npm `impit` picks for `browser: "chrome"`.
///
/// Refuses unless the environment is prepared: the vendored impit then only
/// reads the variable, so no build (the first, or one after a challenge)
/// writes the environment while other threads may read it.
fn build_impit() -> Result<Arc<Impit<Jar>>, HttpError> {
    if !environment_prepared() {
        return Err(HttpError::EnvironmentNotPrepared);
    }
    Impit::<Jar>::builder()
        .with_fingerprint(chrome_fingerprint())
        .with_redirect(RedirectBehavior::ManualRedirect)
        .build()
        .map(Arc::new)
        .map_err(HttpError::Build)
}

impl HttpClient {
    pub fn new(
        base: impl Into<String>,
        cookie_header: Secret,
        policy: RetryPolicy,
    ) -> Result<Self, HttpError> {
        let base = base.into();
        if HeaderValue::from_str(cookie_header.expose()).is_err() {
            return Err(HttpError::InvalidHeaderValue { path: base });
        }
        Ok(HttpClient {
            base,
            cookie_header,
            policy,
            impit: RwLock::new(build_impit()?),
            on_retry: None,
        })
    }

    pub fn on_retry(mut self, observer: impl Fn(&RetryEvent) + Send + Sync + 'static) -> Self {
        self.on_retry = Some(Box::new(observer));
        self
    }

    fn current_impit(&self) -> Arc<Impit<Jar>> {
        Arc::clone(&self.impit.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// GETs `path`, retrying challenges, 429s and gateway errors. Returns the 2xx body.
    pub async fn get(&self, path: &str, headers: &[(&str, &str)]) -> Result<String, HttpError> {
        self.send(HttpMethod::Get, path, headers, None).await
    }

    /// Sends `method` to `path` with an optional body, retrying challenges,
    /// 429s and gateway errors as [`HttpClient::get`] does. A retried
    /// request is sent again, so only reads and idempotent writes belong
    /// here; see [`HttpClient::send_with`] for the others.
    pub async fn send(
        &self,
        method: HttpMethod,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&str>,
    ) -> Result<String, HttpError> {
        self.send_with(method, path, headers, body, Resend::Allowed)
            .await
    }

    /// [`HttpClient::send`], retrying only as far as `resend` allows.
    pub async fn send_with(
        &self,
        method: HttpMethod,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&str>,
        resend: Resend,
    ) -> Result<String, HttpError> {
        let url = format!("{}{path}", self.base);
        let options = RequestOptions {
            headers: std::iter::once(("cookie", self.cookie_header.expose()))
                .chain(headers.iter().copied())
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect(),
            ..RequestOptions::default()
        };
        let mut attempt = 0;
        loop {
            let impit = self.current_impit();
            let request_body =
                body.map(|text| ImpitBody::Bytes(bytes::Bytes::copy_from_slice(text.as_bytes())));
            let sent = match method {
                HttpMethod::Get => {
                    impit
                        .get(url.clone(), request_body, Some(options.clone()))
                        .await
                }
                HttpMethod::Post => {
                    impit
                        .post(url.clone(), request_body, Some(options.clone()))
                        .await
                }
                HttpMethod::Patch => {
                    impit
                        .patch(url.clone(), request_body, Some(options.clone()))
                        .await
                }
                HttpMethod::Delete => {
                    impit
                        .delete(url.clone(), request_body, Some(options.clone()))
                        .await
                }
            };
            let response = sent.map_err(|error| transport_error(path, error))?;
            let status = response.status();
            let reason = match retry_reason(status, response.headers()) {
                // The gateway may have passed the write on before failing.
                Some(RetryReason::Gateway(_)) if resend == Resend::OnlyIfRefused => {
                    return finish(response, path).await;
                }
                Some(reason) if attempt < self.policy.max_retries => reason,
                Some(RetryReason::Challenge) => {
                    return Err(HttpError::Challenged {
                        path: path.to_owned(),
                    });
                }
                _ => return finish(response, path).await,
            };
            let server_wait = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(parse_retry_after);
            let wait = match server_wait {
                Some(wait) if wait > self.policy.max_retry_after => {
                    if reason == RetryReason::RateLimited {
                        return Err(HttpError::RateLimited {
                            path: path.to_owned(),
                            retry_after: wait,
                        });
                    }
                    return finish(response, path).await;
                }
                Some(wait) => wait,
                None => self.policy.backoff(reason, attempt),
            };
            // The retry doesn't need this body; dropping it frees the connection.
            drop(response);
            if reason == RetryReason::Challenge {
                *self.impit.write().unwrap_or_else(PoisonError::into_inner) = build_impit()?;
            }
            if let Some(observer) = &self.on_retry {
                observer(&RetryEvent {
                    reason,
                    attempt,
                    wait,
                });
            }
            tokio::time::sleep(wait).await;
            attempt += 1;
        }
    }
}

/// GET a public file, following redirects, with no cookies and no retries:
/// the search embedding model's files, which Hugging Face redirects to its
/// CDN. Errors name `url` without its query and never quote a body.
pub async fn download(url: &str, timeout: Duration) -> Result<bytes::Bytes, HttpError> {
    let shown = url.split('?').next().unwrap_or(url).to_owned();
    if !environment_prepared() {
        return Err(HttpError::EnvironmentNotPrepared);
    }
    let impit = Impit::<Jar>::builder()
        .with_fingerprint(chrome_fingerprint())
        .with_redirect(RedirectBehavior::FollowRedirect(10))
        .with_default_timeout(timeout)
        .build()
        .map_err(HttpError::Build)?;
    let response = impit
        .get(url.to_owned(), None, None)
        .await
        .map_err(|error| transport_error(&shown, error))?;
    let status = response.status();
    if !status.is_success() {
        return Err(HttpError::Status {
            status,
            path: shown,
            snippet: None,
        });
    }
    response
        .bytes()
        .await
        .map_err(|error| transport_error(&shown, error.into()))
}

/// The body of a final response: `Ok` for 2xx, otherwise `HttpError::Status`.
async fn finish(response: reqwest::Response, path: &str) -> Result<String, HttpError> {
    let status = response.status();
    if !status.is_success() && !shows_snippet(path) {
        // Never read an auth error body: it is not shown, and it may hold tokens.
        let path = path.to_owned();
        return Err(HttpError::Status {
            status,
            path,
            snippet: None,
        });
    }
    let body = response
        .text()
        .await
        .map_err(|error| transport_error(path, error.into()))?;
    if status.is_success() {
        return Ok(body);
    }
    Err(HttpError::Status {
        status,
        path: path.to_owned(),
        snippet: Some(body.chars().take(SNIPPET_CHARS).collect()),
    })
}

fn transport_error(path: &str, error: impit::errors::ImpitError) -> HttpError {
    let path = path.to_owned();
    match error {
        impit::errors::ImpitError::InvalidHeaderValue(_) => HttpError::InvalidHeaderValue { path },
        source => HttpError::Transport { path, source },
    }
}

#[cfg(test)]
mod tests;
