//! The browser sessions the daemon holds. Cookies are read once per
//! browser choice (the read can raise a Keychain prompt), exchanged for an
//! access token that stays in memory, and read again only when ChatGPT
//! rejects them or the session expired.

use std::collections::HashMap;
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

/// The sessions the daemon holds, one per browser choice, so a client
/// asking for another browser (`stats --browser chrome`) never changes the
/// session a running sync uses. Each choice has its own lock: reading one
/// browser's cookies, which can wait on a Keychain prompt, holds up only
/// calls for that browser.
pub struct Sessions {
    slots: Mutex<HashMap<SessionChoice, Arc<Slot>>>,
    reporter: Reporter,
}

type Slot = tokio::sync::Mutex<Option<Arc<Session>>>;

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
            slots: Mutex::new(HashMap::new()),
            reporter,
        }
    }

    fn slot(&self, choice: &SessionChoice) -> Arc<Slot> {
        let mut slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(slots.entry(choice.clone()).or_default())
    }

    /// Where the session for `choice` came from, for `daemon status`.
    /// `None` while one is being read: `Status` must answer at once, even
    /// while a Keychain prompt waits.
    pub fn source(&self, choice: &SessionChoice) -> Option<String> {
        self.slot(choice)
            .try_lock()
            .ok()?
            .as_ref()
            .map(|session| session.source.clone())
    }

    /// The session held for `choice`, or a new one read from the browser.
    pub async fn current(&self, choice: &SessionChoice) -> Result<Arc<Session>, ApiError> {
        let slot = self.slot(choice);
        let mut held = slot.lock().await;
        if let Some(session) = held.as_ref() {
            return Ok(Arc::clone(session));
        }
        let session = Arc::new(self.open(choice).await?);
        *held = Some(Arc::clone(&session));
        Ok(session)
    }

    /// Read `choice`'s cookies again and exchange them for a new token,
    /// unless another call already replaced `stale`.
    pub async fn renew(
        &self,
        choice: &SessionChoice,
        stale: &Arc<Session>,
    ) -> Result<Arc<Session>, ApiError> {
        let slot = self.slot(choice);
        let mut held = slot.lock().await;
        if let Some(session) = held.as_ref()
            && !Arc::ptr_eq(session, stale)
        {
            return Ok(Arc::clone(session));
        }
        tracing::info!(source = %stale.source, "ChatGPT rejected the session; reading cookies again");
        *held = None;
        let session = Arc::new(self.open(choice).await?);
        *held = Some(Arc::clone(&session));
        Ok(session)
    }

    async fn open(&self, choice: &SessionChoice) -> Result<Session, ApiError> {
        let (cookie_header, source) = match test_cookie(choice) {
            Some(cookie) => (cookie, "test cookie".to_owned()),
            None => read_cookies(choice).await?,
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

/// In debug builds: `CHATGPT_TEST_COOKIE_<BROWSER>` for a chosen browser,
/// else `CHATGPT_TEST_COOKIE`, instead of a real browser's cookies.
fn test_cookie(choice: &SessionChoice) -> Option<String> {
    choice
        .browser
        .as_deref()
        .and_then(|browser| debug_env(&format!("{TEST_COOKIE_ENV}_{}", browser.to_uppercase())))
        .or_else(|| debug_env(TEST_COOKIE_ENV))
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
