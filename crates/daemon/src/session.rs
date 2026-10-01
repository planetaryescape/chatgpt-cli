//! The one browser session the daemon holds. Cookies are read once (the
//! read can raise a Keychain prompt), exchanged for an access token that
//! stays in memory, and read again only when ChatGPT rejects them, the
//! session expired, or a client asks for another browser or profile.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chatgpt::cookies::{BrowserSelection, default_browser, read_browser_session};
use chatgpt::http::{CHATGPT_BASE, HttpClient, RetryPolicy, RetryReason, Secret};
use chatgpt::session::{AccessToken, exchange_session};
use chatgpt_core::ErrorKind;
use chatgpt_protocol::SessionChoice;

use crate::api::ApiError;
use crate::progress::Reporter;

/// Overrides chatgpt.com in debug builds, so tests can point a real daemon
/// at a fake. Release builds ignore it: it would send the cookies elsewhere.
const BASE_URL_ENV: &str = "CHATGPT_BASE_URL";
/// A cookie header to use instead of a browser's, in debug builds only.
const TEST_COOKIE_ENV: &str = "CHATGPT_TEST_COOKIE";
/// Millisecond backoffs instead of seconds, in debug builds only.
const FAST_RETRY_ENV: &str = "CHATGPT_TEST_FAST_RETRY";

pub struct Session {
    /// Where it came from, e.g. `dia profile "Default"`.
    pub source: String,
    pub http: HttpClient,
    pub token: AccessToken,
}

pub struct Sessions {
    current: tokio::sync::Mutex<Option<Arc<Session>>>,
    /// What the next session should be read from: the last choice a client
    /// sent, which background syncs keep using.
    wanted: Mutex<SessionChoice>,
    reporter: Reporter,
}

fn debug_env(name: &str) -> Option<String> {
    if cfg!(debug_assertions) {
        std::env::var(name).ok().filter(|value| !value.is_empty())
    } else {
        None
    }
}

impl Sessions {
    pub fn new(reporter: Reporter) -> Self {
        Self {
            current: tokio::sync::Mutex::new(None),
            wanted: Mutex::new(SessionChoice::default()),
            reporter,
        }
    }

    /// Use `choice` from now on. A different choice drops the held session,
    /// so the next call reads the newly chosen browser's cookies.
    pub async fn choose(&self, choice: &SessionChoice) {
        let changed = {
            let mut wanted = self.wanted.lock().unwrap_or_else(PoisonError::into_inner);
            let changed = *wanted != *choice;
            *wanted = choice.clone();
            changed
        };
        if changed {
            *self.current.lock().await = None;
        }
    }

    /// The browser and profile sessions are read from.
    pub fn chosen(&self) -> SessionChoice {
        self.wanted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Where the held session came from, for `daemon status`. `None` while
    /// one is being read: `Status` must answer at once, even while a
    /// Keychain prompt waits.
    pub fn source(&self) -> Option<String> {
        self.current
            .try_lock()
            .ok()?
            .as_ref()
            .map(|session| session.source.clone())
    }

    /// The held session, or a new one read from the browser.
    pub async fn current(&self) -> Result<Arc<Session>, ApiError> {
        let mut current = self.current.lock().await;
        if let Some(session) = current.as_ref() {
            return Ok(Arc::clone(session));
        }
        let session = Arc::new(self.open().await?);
        *current = Some(Arc::clone(&session));
        Ok(session)
    }

    /// Read the cookies again and exchange them for a new token, unless
    /// another call already replaced `stale`.
    pub async fn renew(&self, stale: &Arc<Session>) -> Result<Arc<Session>, ApiError> {
        let mut current = self.current.lock().await;
        if let Some(session) = current.as_ref()
            && !Arc::ptr_eq(session, stale)
        {
            return Ok(Arc::clone(session));
        }
        tracing::info!(source = %stale.source, "ChatGPT rejected the session; reading cookies again");
        *current = None;
        let session = Arc::new(self.open().await?);
        *current = Some(Arc::clone(&session));
        Ok(session)
    }

    async fn open(&self) -> Result<Session, ApiError> {
        let choice = self.chosen();
        let (cookie_header, source) = match debug_env(TEST_COOKIE_ENV) {
            Some(cookie) => (cookie, "test cookie".to_owned()),
            None => read_cookies(&choice).await?,
        };
        let base = debug_env(BASE_URL_ENV).unwrap_or_else(|| CHATGPT_BASE.to_owned());
        let policy = if debug_env(FAST_RETRY_ENV).is_some() {
            RetryPolicy {
                base_backoff: Duration::from_millis(1),
                rate_limit_backoff: Duration::from_millis(5),
                ..RetryPolicy::default()
            }
        } else {
            RetryPolicy::default()
        };
        let reporter = self.reporter.clone();
        let http = HttpClient::new(base, Secret::new(cookie_header), policy)?.on_retry(
            move |event| {
                tracing::info!(reason = ?event.reason, attempt = event.attempt, wait_ms = event.wait.as_millis() as u64, "retrying a ChatGPT request");
                if event.reason == RetryReason::RateLimited {
                    // The skill's wording: "rate limited by ChatGPT; waiting …".
                    reporter.note(format!(
                        "rate limited by ChatGPT; waiting {}s",
                        event.wait.as_secs_f64().round()
                    ));
                }
            },
        );
        let token = exchange_session(&http, &source).await?;
        tracing::info!(%source, "exchanged the browser session for an access token");
        Ok(Session {
            source,
            http,
            token,
        })
    }
}

/// The browser's cookies, read off the async threads: it reads SQLite and
/// may run `security`, which can wait on a Keychain prompt.
async fn read_cookies(choice: &SessionChoice) -> Result<(String, String), ApiError> {
    let selection = BrowserSelection {
        browser: choice.browser.clone(),
        profile: choice.profile.clone(),
    };
    tokio::task::spawn_blocking(move || {
        let home = dirs::home_dir().ok_or_else(|| {
            ApiError::new(
                ErrorKind::AuthRequired,
                "Could not find your home directory.",
            )
        })?;
        let session = read_browser_session(&selection, &home, default_browser(&home))?;
        Ok((session.cookie_header(), session.source()))
    })
    .await
    .map_err(|error| {
        ApiError::new(
            ErrorKind::Internal,
            format!("reading cookies failed: {error}"),
        )
    })?
}
