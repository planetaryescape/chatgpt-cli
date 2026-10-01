//! Firefox cookie stores.
//!
//! Ported from the TS CLI's `src/auth/firefox-cookies.ts` and the profile
//! ordering in `browser-cookies.ts` @ 1b8c950.

use std::path::Path;

use rusqlite::params;

use super::{BrowserCookie, CookieError, open_cookie_db, subdirectories, unix_now};

/// Profile directories, with the `profiles.ini` default first, then alphabetical.
pub(crate) fn firefox_profiles(root: &Path) -> Vec<String> {
    let mut profiles = subdirectories(root);
    // A missing profiles.ini does not prevent scanning profile directories.
    let preferred = root
        .parent()
        .and_then(|parent| std::fs::read_to_string(parent.join("profiles.ini")).ok())
        .and_then(|ini| preferred_profile(&ini));
    profiles.sort_by(|a, b| {
        (Some(a) != preferred.as_ref())
            .cmp(&(Some(b) != preferred.as_ref()))
            .then_with(|| a.cmp(b))
    });
    profiles
}

/// The install's `Default=Profiles/<dir>`, else the section marked `Default=1`.
fn preferred_profile(ini: &str) -> Option<String> {
    let profile_dir = |line: &str, key: &str| {
        line.strip_prefix(key)?
            .strip_prefix("Profiles/")
            .map(|dir| dir.trim().to_owned())
    };
    if let Some(dir) = ini.lines().find_map(|line| profile_dir(line, "Default=")) {
        return Some(dir);
    }
    ini.split("\n[")
        .find(|section| section.lines().any(|line| line == "Default=1"))
        .and_then(|section| section.lines().find_map(|line| profile_dir(line, "Path=")))
}

/// Unexpired cookies for `host` and `.host` from the default container only.
pub(crate) fn read_firefox_cookies(
    profile_dir: &Path,
    host: &str,
) -> Result<Vec<BrowserCookie>, CookieError> {
    let source = profile_dir.join("cookies.sqlite");
    if !source.exists() {
        return Ok(Vec::new());
    }
    let sqlite_error = |error| CookieError::Sqlite {
        path: source.clone(),
        source: error,
    };
    let db = open_cookie_db(&source)?;
    let now = i64::try_from(unix_now().as_secs()).unwrap_or(i64::MAX);
    let mut statement = db
        .prepare(
            "select host, name, value from moz_cookies where host in (?1, ?2) and (expiry = 0 or expiry > ?3) and coalesce(originAttributes, '') = ''",
        )
        .map_err(sqlite_error)?;
    statement
        .query_map(params![host, format!(".{host}"), now], |row| {
            Ok(BrowserCookie {
                domain: row.get(0)?,
                name: row.get(1)?,
                value: row.get(2)?,
            })
        })
        .map_err(sqlite_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sqlite_error)
}
