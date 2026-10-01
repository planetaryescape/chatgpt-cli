//! Connectivity probe for the Rust port: browser session -> access token ->
//! one page of conversations. Every call is a read-only GET.

use std::process::ExitCode;
use std::time::Duration;

use chatgpt::cookies::{BrowserSelection, BrowserSession, default_browser, read_browser_session};
use chatgpt::http::{
    CHATGPT_BASE, HttpClient, HttpError, RetryPolicy, RetryReason, Secret, retry_reason,
};
use chatgpt::session::{SESSION_PATH, SessionError, exchange_session};
use clap::{Parser, Subcommand, ValueEnum};
use reqwest::StatusCode;
use reqwest::header::HeaderMap;
use serde::Deserialize;

const LIST_PATH: &str = "/backend-api/conversations?offset=0&limit=100&order=updated&is_archived=false&hide_snorlax=false";
const TITLE_CHARS: usize = 30;

#[derive(Parser)]
#[command(about = "Probe chatgpt.com access from the Rust port")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Read the browser session, exchange it for a token and list one page of chats.
    Probe {
        /// dia, chrome, safari, firefox, arc, brave or edge. Defaults to the macOS default browser.
        #[arg(long)]
        browser: Option<String>,
        /// A profile directory, such as "Default" or "Profile 1". Needs --browser.
        #[arg(long)]
        profile: Option<String>,
        /// Open this many fresh clients, without retries, and count how each fares.
        #[arg(long)]
        connections: Option<u32>,
        /// Pause between fresh clients, in milliseconds.
        #[arg(long, default_value_t = 1000)]
        pace_ms: u64,
        /// `reqwest` is the un-impersonated control, which Cloudflare should challenge.
        #[arg(long, value_enum, default_value_t = Transport::Impit)]
        transport: Transport,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Transport {
    Impit,
    Reqwest,
}

#[derive(Deserialize)]
struct ConversationPage {
    items: Vec<ConversationItem>,
}

#[derive(Deserialize)]
struct ConversationItem {
    title: Option<String>,
}

#[tokio::main]
async fn main() -> ExitCode {
    let Command::Probe {
        browser,
        profile,
        connections,
        pace_ms,
        transport,
    } = Cli::parse().command;
    let selection = BrowserSelection { browser, profile }.with_env_defaults();
    let session = match dirs::home_dir()
        .ok_or("Could not find your home directory.".to_owned())
        .and_then(|home| {
            read_browser_session(&selection, &home, default_browser(&home))
                .map_err(|error| error.to_string())
        }) {
        Ok(session) => session,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let chunks = session.session_token().map_or(0, |token| token.chunks);
    eprintln!("session: {} ({chunks} token chunk(s))", session.source());
    let result = match (connections, transport) {
        (None, Transport::Impit) => list_once(&session).await,
        (Some(count), Transport::Impit) => {
            fresh_connections(count, Duration::from_millis(pace_ms), || {
                impit_attempt(&session)
            })
            .await;
            Ok(())
        }
        (count, Transport::Reqwest) => {
            let count = count.unwrap_or(1);
            fresh_connections(count, Duration::from_millis(pace_ms), || {
                reqwest_attempt(&session)
            })
            .await;
            Ok(())
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

async fn list_once(session: &BrowserSession) -> Result<(), String> {
    let http = HttpClient::new(
        CHATGPT_BASE,
        Secret::new(session.cookie_header()),
        RetryPolicy::default(),
    )
    .map_err(|error| error.to_string())?
    .on_retry(|event| {
        eprintln!(
            "retrying after {:?}: waiting {:?}",
            event.reason, event.wait
        )
    });
    let token = exchange_session(&http, &session.source())
        .await
        .map_err(|error| error.to_string())?;
    eprintln!("session exchange: ok");
    let body = http
        .get(LIST_PATH, &list_headers(&token.bearer()))
        .await
        .map_err(|error| error.to_string())?;
    let page: ConversationPage = serde_json::from_str(&body)
        .map_err(|error| format!("Unexpected conversation list: {error}"))?;
    println!("conversations on first page: {}", page.items.len());
    for item in page.items.iter().take(3) {
        println!(
            "  {}",
            truncate_title(item.title.as_deref().unwrap_or("(untitled)"))
        );
    }
    Ok(())
}

fn list_headers(bearer: &Secret) -> [(&str, &str); 2] {
    [
        ("authorization", bearer.expose()),
        ("accept", "application/json"),
    ]
}

fn truncate_title(title: &str) -> String {
    match title.char_indices().nth(TITLE_CHARS) {
        Some((end, _)) => format!("{}…", &title[..end]),
        None => title.to_owned(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Success,
    Challenged,
    RateLimited,
    Other,
}

impl Outcome {
    fn from_response(status: StatusCode, headers: &HeaderMap) -> Outcome {
        match retry_reason(status, headers) {
            _ if status.is_success() => Outcome::Success,
            Some(RetryReason::Challenge) => Outcome::Challenged,
            Some(RetryReason::RateLimited) => Outcome::RateLimited,
            _ => Outcome::Other,
        }
    }
}

async fn fresh_connections<F, Fut>(count: u32, pace: Duration, attempt: F)
where
    F: Fn() -> Fut,
    Fut: Future<Output = (Outcome, String)>,
{
    let mut outcomes = Vec::new();
    for index in 0..count {
        if index > 0 {
            tokio::time::sleep(pace).await;
        }
        let (outcome, detail) = attempt().await;
        eprintln!("connection {:>3}: {outcome:?} {detail}", index + 1);
        outcomes.push(outcome);
    }
    let tally = |kind| outcomes.iter().filter(|outcome| **outcome == kind).count();
    println!(
        "connections: {count}, success: {}, challenged: {}, rate limited (429): {}, other: {}",
        tally(Outcome::Success),
        tally(Outcome::Challenged),
        tally(Outcome::RateLimited),
        tally(Outcome::Other),
    );
}

/// One fresh client, no retries: the session exchange, then one list call.
async fn impit_attempt(session: &BrowserSession) -> (Outcome, String) {
    let http = match HttpClient::new(
        CHATGPT_BASE,
        Secret::new(session.cookie_header()),
        RetryPolicy::none(),
    ) {
        Ok(http) => http,
        Err(error) => return (Outcome::Other, error.to_string()),
    };
    let token = match exchange_session(&http, &session.source()).await {
        Ok(token) => token,
        Err(SessionError::Http(error)) => return classify(&error, "session"),
        Err(error) => return (Outcome::Other, format!("session: {error}")),
    };
    match http.get(LIST_PATH, &list_headers(&token.bearer())).await {
        Ok(_) => (Outcome::Success, String::new()),
        Err(error) => classify(&error, "list"),
    }
}

fn classify(error: &HttpError, step: &str) -> (Outcome, String) {
    match error {
        HttpError::Challenged { .. } => (Outcome::Challenged, format!("at {step}")),
        HttpError::RateLimited { retry_after, .. } => (
            Outcome::RateLimited,
            format!("at {step}: retry-after {}s", retry_after.as_secs()),
        ),
        HttpError::Status { status, .. } if *status == StatusCode::TOO_MANY_REQUESTS => {
            (Outcome::RateLimited, format!("at {step}: {status}"))
        }
        HttpError::Status { status, .. } => (Outcome::Other, format!("at {step}: {status}")),
        other => (Outcome::Other, format!("at {step}: {other}")),
    }
}

/// The control: plain reqwest with a Chrome user agent but reqwest's own
/// TLS and HTTP/2 fingerprint. Only the session exchange is attempted.
async fn reqwest_attempt(session: &BrowserSession) -> (Outcome, String) {
    const CHROME_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
    let client = match reqwest::Client::builder().user_agent(CHROME_UA).build() {
        Ok(client) => client,
        Err(error) => return (Outcome::Other, error.to_string()),
    };
    match client
        .get(format!("{CHATGPT_BASE}{SESSION_PATH}"))
        .header("cookie", session.cookie_header())
        .send()
        .await
    {
        Ok(response) => {
            let cf = response
                .headers()
                .get("cf-mitigated")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let status = response.status();
            (
                Outcome::from_response(status, response.headers()),
                format!(
                    "at session: {status} cf-mitigated={}",
                    cf.as_deref().unwrap_or("-")
                ),
            )
        }
        Err(error) => (
            Outcome::Other,
            format!("at session: {}", error.without_url()),
        ),
    }
}
