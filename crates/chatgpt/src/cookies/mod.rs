//! Reads the chatgpt.com session cookie from the user's macOS browser.
//!
//! Ported from the TS CLI's `src/auth/browser-cookies.ts` @ 1b8c950.

mod chromium;
mod firefox;
mod safari;

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags};

use crate::http::Secret;

use chromium::{ChromiumBrowser, chromium_profiles, keychain_password, read_chromium_cookies};
use firefox::{firefox_profiles, read_firefox_cookies};
use safari::read_safari_cookies;

pub const CHATGPT_HOST: &str = "chatgpt.com";
pub const SESSION_COOKIE: &str = "__Secure-next-auth.session-token";

#[derive(Clone, PartialEq, Eq)]
pub struct BrowserCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
}

impl fmt::Debug for BrowserCookie {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BrowserCookie")
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .field("domain", &self.domain)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Browser {
    Dia,
    Chrome,
    Safari,
    Firefox,
    Arc,
    Brave,
    Edge,
}

impl Browser {
    pub const ALL: [Browser; 7] = [
        Browser::Dia,
        Browser::Chrome,
        Browser::Safari,
        Browser::Firefox,
        Browser::Arc,
        Browser::Brave,
        Browser::Edge,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Browser::Dia => "dia",
            Browser::Chrome => "chrome",
            Browser::Safari => "safari",
            Browser::Firefox => "firefox",
            Browser::Arc => "arc",
            Browser::Brave => "brave",
            Browser::Edge => "edge",
        }
    }

    pub fn for_bundle_id(bundle_id: &str) -> Option<Browser> {
        match bundle_id.to_lowercase().as_str() {
            "company.thebrowser.dia" => Some(Browser::Dia),
            "com.google.chrome" => Some(Browser::Chrome),
            "com.apple.safari" => Some(Browser::Safari),
            "org.mozilla.firefox" => Some(Browser::Firefox),
            "company.thebrowser.browser" => Some(Browser::Arc),
            "com.brave.browser" => Some(Browser::Brave),
            "com.microsoft.edgemac" => Some(Browser::Edge),
            _ => None,
        }
    }

    fn chromium(self) -> Option<ChromiumBrowser> {
        let (directory, keychain_service) = match self {
            Browser::Dia => ("Dia/User Data", "Dia Safe Storage"),
            Browser::Chrome => ("Google/Chrome", "Chrome Safe Storage"),
            Browser::Arc => ("Arc/User Data", "Arc Safe Storage"),
            Browser::Brave => ("BraveSoftware/Brave-Browser", "Brave Safe Storage"),
            Browser::Edge => ("Microsoft Edge", "Microsoft Edge Safe Storage"),
            Browser::Safari | Browser::Firefox => return None,
        };
        Some(ChromiumBrowser {
            directory,
            keychain_service,
        })
    }
}

impl fmt::Display for Browser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Browser {
    type Err = CookieError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Browser::ALL
            .into_iter()
            .find(|browser| browser.name() == name)
            .ok_or_else(|| CookieError::UnsupportedBrowser(name.to_owned()))
    }
}

fn browser_list() -> String {
    Browser::ALL.map(Browser::name).join(", ")
}

#[derive(Debug, thiserror::Error)]
pub enum CookieError {
    #[error("Browser session reading currently supports macOS only.")]
    UnsupportedPlatform,
    #[error("--profile requires --browser (or CHATGPT_BROWSER).")]
    ProfileWithoutBrowser,
    #[error("Unsupported browser \"{0}\". Choose {list}.", list = browser_list())]
    UnsupportedBrowser(String),
    #[error("Could not identify your macOS default browser. Choose one with --browser: {list}.", list = browser_list())]
    NoDefaultBrowser,
    #[error("Safari does not have selectable cookie profiles.")]
    SafariProfile,
    #[error("Profile \"{profile}\" was not found in {browser}. Available profiles: {available}.")]
    ProfileNotFound {
        browser: Browser,
        profile: String,
        available: String,
    },
    #[error(
        "No ChatGPT session in {browser}{profile}. Log in to chatgpt.com there and retry, or use --browser and --profile to select another session.",
        profile = profile.as_ref().map(|p| format!(" profile \"{p}\"")).unwrap_or_default()
    )]
    NoSession {
        browser: Browser,
        profile: Option<String>,
    },
    #[error(
        "Could not read \"{service}\" from the Keychain. Allow access when macOS prompts.\n{detail}"
    )]
    Keychain { service: String, detail: String },
    #[error("Unsupported cookie encryption version \"{0}\"")]
    UnsupportedEncryption(String),
    #[error("Could not decrypt a cookie: {0}")]
    Decrypt(&'static str),
    #[error(
        "macOS blocked access to Safari cookies. Grant Full Disk Access to your terminal, then retry."
    )]
    SafariAccessDenied,
    #[error("{0}")]
    InvalidSafariFile(&'static str),
    #[error("Could not read cookies from {path}: {source}")]
    Sqlite {
        path: PathBuf,
        source: rusqlite::Error,
    },
    #[error("Could not read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, Default)]
pub struct BrowserSelection {
    pub browser: Option<String>,
    pub profile: Option<String>,
}

impl BrowserSelection {
    /// Fills unset fields from `CHATGPT_BROWSER` and `CHATGPT_BROWSER_PROFILE`, as the TS CLI does.
    pub fn with_env_defaults(self) -> Self {
        let env = |name| {
            std::env::var(name)
                .ok()
                .filter(|value: &String| !value.is_empty())
        };
        BrowserSelection {
            browser: self.browser.or_else(|| env("CHATGPT_BROWSER")),
            profile: self.profile.or_else(|| env("CHATGPT_BROWSER_PROFILE")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BrowserSession {
    pub browser: Browser,
    pub profile: Option<String>,
    pub cookies: Vec<BrowserCookie>,
}

impl BrowserSession {
    /// Where the session came from, for messages: `dia profile "Default"`.
    pub fn source(&self) -> String {
        match &self.profile {
            Some(profile) => format!("{} profile \"{profile}\"", self.browser),
            None => self.browser.to_string(),
        }
    }

    /// Every chatgpt.com cookie, the way the browser would send them.
    pub fn cookie_header(&self) -> String {
        self.cookies
            .iter()
            .map(|cookie| format!("{}={}", cookie.name, cookie.value))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// The session token, with `.0`, `.1`, … chunks joined in numeric order.
    pub fn session_token(&self) -> Option<SessionToken> {
        join_session_token(&self.cookies)
    }
}

/// The joined session-token value. Debug never prints it.
#[derive(Debug, Clone)]
pub struct SessionToken {
    pub value: Secret,
    pub chunks: usize,
}

/// NextAuth splits a session token over 4 KB into `<name>.0`, `<name>.1`, ….
/// An unsplit cookie wins; otherwise the chunks join in index order.
pub fn join_session_token(cookies: &[BrowserCookie]) -> Option<SessionToken> {
    if let Some(whole) = cookies.iter().find(|cookie| cookie.name == SESSION_COOKIE) {
        return Some(SessionToken {
            value: Secret::new(whole.value.clone()),
            chunks: 1,
        });
    }
    let mut chunks: Vec<(u32, &str)> = cookies
        .iter()
        .filter_map(|cookie| {
            let index = cookie
                .name
                .strip_prefix(SESSION_COOKIE)?
                .strip_prefix('.')?;
            Some((index.parse().ok()?, cookie.value.as_str()))
        })
        .collect();
    if chunks.is_empty() {
        return None;
    }
    chunks.sort_by_key(|(index, _)| *index);
    Some(SessionToken {
        value: Secret::new(chunks.iter().map(|(_, value)| *value).collect()),
        chunks: chunks.len(),
    })
}

/// The whole session-token cookie or one of its `.N` chunks.
fn is_session_cookie_name(name: &str) -> bool {
    match name.strip_prefix(SESSION_COOKIE) {
        Some("") => true,
        Some(suffix) => suffix
            .strip_prefix('.')
            .is_some_and(|index| !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit())),
        None => false,
    }
}

fn has_session_cookie(cookies: &[BrowserCookie]) -> bool {
    cookies
        .iter()
        .any(|cookie| is_session_cookie_name(&cookie.name))
}

/// Opens a browser's cookie DB read-only. A read-only connection still sees
/// uncheckpointed WAL writes from a running browser; the busy timeout rides
/// out its checkpoints.
fn open_cookie_db(path: &Path) -> Result<Connection, CookieError> {
    let sqlite_error = |source| CookieError::Sqlite {
        path: path.to_owned(),
        source,
    };
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(sqlite_error)?;
    db.busy_timeout(Duration::from_secs(5))
        .map_err(sqlite_error)?;
    Ok(db)
}

/// Names of the directories directly under `root`; empty if it is unreadable.
fn subdirectories(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect()
}

/// Time since 1970; a clock before 1970 counts as 1970.
fn unix_now() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

const LAUNCH_SERVICES_PLIST: &str =
    "Library/Preferences/com.apple.LaunchServices/com.apple.launchservices.secure.plist";

/// The macOS default browser: the https handler with the newest modification date.
pub fn default_browser(home: &Path) -> Option<Browser> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let plist = plist::Value::from_file(home.join(LAUNCH_SERVICES_PLIST)).ok()?;
    default_browser_from_plist(&plist)
}

fn default_browser_from_plist(plist: &plist::Value) -> Option<Browser> {
    let handlers = plist.as_dictionary()?.get("LSHandlers")?.as_array()?;
    let modified = |handler: &plist::Dictionary| {
        handler
            .get("LSHandlerModificationDate")
            .and_then(|date| {
                date.as_real()
                    .or_else(|| date.as_signed_integer().map(|n| n as f64))
            })
            .unwrap_or(0.0)
    };
    handlers
        .iter()
        .filter_map(plist::Value::as_dictionary)
        .filter(|handler| {
            handler
                .get("LSHandlerURLScheme")
                .and_then(plist::Value::as_string)
                == Some("https")
        })
        // max_by keeps the last of equal maxima; reversing keeps the first,
        // matching the TS stable sort when dates tie or are missing.
        .rev()
        .max_by(|a, b| modified(a).total_cmp(&modified(b)))
        .and_then(|handler| handler.get("LSHandlerRoleAll")?.as_string())
        .and_then(Browser::for_bundle_id)
}

/// Reads the session from the requested browser, or the default one.
///
/// It never falls back to another browser: that could silently switch accounts.
pub fn read_browser_session(
    selection: &BrowserSelection,
    home: &Path,
    preferred: Option<Browser>,
) -> Result<BrowserSession, CookieError> {
    if !cfg!(target_os = "macos") {
        return Err(CookieError::UnsupportedPlatform);
    }
    read_browser_session_with(selection, home, preferred, &keychain_password)
}

fn read_browser_session_with(
    selection: &BrowserSelection,
    home: &Path,
    preferred: Option<Browser>,
    read_password: &dyn Fn(&str) -> Result<String, CookieError>,
) -> Result<BrowserSession, CookieError> {
    let profile = selection.profile.clone();
    if profile.is_some() && selection.browser.is_none() {
        return Err(CookieError::ProfileWithoutBrowser);
    }
    let browser = match &selection.browser {
        Some(name) => name.parse()?,
        None => preferred.ok_or(CookieError::NoDefaultBrowser)?,
    };
    read_from_browser(browser, profile.as_deref(), home, read_password)?
        .ok_or(CookieError::NoSession { browser, profile })
}

fn read_from_browser(
    browser: Browser,
    profile: Option<&str>,
    home: &Path,
    read_password: &dyn Fn(&str) -> Result<String, CookieError>,
) -> Result<Option<BrowserSession>, CookieError> {
    if browser == Browser::Safari {
        if profile.is_some() {
            return Err(CookieError::SafariProfile);
        }
        let cookies = read_safari_cookies(CHATGPT_HOST, home)?;
        return Ok(has_session_cookie(&cookies).then_some(BrowserSession {
            browser,
            profile: None,
            cookies,
        }));
    }
    let chromium = browser.chromium();
    let root = match &chromium {
        Some(chromium) => home
            .join("Library/Application Support")
            .join(chromium.directory),
        None => home.join("Library/Application Support/Firefox/Profiles"),
    };
    let profiles = match chromium {
        Some(_) => chromium_profiles(&root),
        None => firefox_profiles(&root),
    };
    let candidates = match profile {
        Some(profile) if !profiles.iter().any(|p| p == profile) => {
            return Err(CookieError::ProfileNotFound {
                browser,
                profile: profile.to_owned(),
                available: if profiles.is_empty() {
                    "none".to_owned()
                } else {
                    profiles.join(", ")
                },
            });
        }
        Some(profile) => vec![profile.to_owned()],
        None => profiles,
    };
    for candidate in candidates {
        let cookies = match &chromium {
            Some(chromium) => read_chromium_cookies(
                &root,
                &candidate,
                chromium.keychain_service,
                CHATGPT_HOST,
                read_password,
            )?,
            None => read_firefox_cookies(&root.join(&candidate), CHATGPT_HOST)?,
        };
        if has_session_cookie(&cookies) {
            return Ok(Some(BrowserSession {
                browser,
                profile: Some(candidate),
                cookies,
            }));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
