//! Chromium cookie stores (Dia, Chrome, Arc, Brave, Edge).
//!
//! Ported from the TS CLI's `src/auth/chromium-cookies.ts` and the profile
//! ordering in `browser-cookies.ts` @ 1b8c950.

use std::path::Path;
use std::process::Command;

use aes::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use rusqlite::params;
use serde::Deserialize;

use super::{
    BrowserCookie, CookieError, is_session_cookie_name, open_cookie_db, subdirectories, unix_now,
};

#[derive(Debug, Clone, Copy)]
pub(crate) struct ChromiumBrowser {
    /// Under `~/Library/Application Support`.
    pub(crate) directory: &'static str,
    /// The Keychain item holding the cookie encryption password.
    pub(crate) keychain_service: &'static str,
}

/// Seconds between the Windows epoch (1601), which Chromium timestamps use, and 1970.
const WINDOWS_EPOCH_OFFSET_SECS: u64 = 11_644_473_600;
const CBC_IV: [u8; 16] = [b' '; 16];

type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

/// Reads the cookie password, which can raise a macOS Keychain prompt.
pub(crate) fn keychain_password(service: &str) -> Result<String, CookieError> {
    let output = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-w", "-s", service])
        .output()
        .map_err(|error| CookieError::Keychain {
            service: service.to_owned(),
            detail: error.to_string(),
        })?;
    if !output.status.success() {
        return Err(CookieError::Keychain {
            service: service.to_owned(),
            detail: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    let password = String::from_utf8(output.stdout).map_err(|_| CookieError::Keychain {
        service: service.to_owned(),
        detail: "the password is not UTF-8".to_owned(),
    })?;
    Ok(password.trim().to_owned())
}

pub(crate) fn derive_chromium_key(password: &str) -> [u8; 16] {
    pbkdf2::pbkdf2_hmac_array::<sha1::Sha1, 16>(password.as_bytes(), b"saltysalt", 1003)
}

/// Decrypts a macOS Chromium `v10` cookie value.
///
/// `strip_host_hash` drops the SHA-256(host_key) that cookie DB schema 24+
/// puts before the plaintext.
pub(crate) fn decrypt_chromium_cookie(
    encrypted: &[u8],
    key: &[u8; 16],
    strip_host_hash: bool,
) -> Result<String, CookieError> {
    let (prefix, ciphertext) = encrypted.split_at(encrypted.len().min(3));
    if prefix != b"v10" {
        return Err(CookieError::UnsupportedEncryption(
            String::from_utf8_lossy(prefix).into_owned(),
        ));
    }
    let mut buffer = ciphertext.to_vec();
    let plain = Aes128CbcDec::new(key.into(), &CBC_IV.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buffer)
        .map_err(|_| CookieError::Decrypt("bad padding; the Keychain password may be wrong"))?;
    let plain = match strip_host_hash {
        true => plain
            .get(32..)
            .ok_or(CookieError::Decrypt("value is shorter than its host hash"))?,
        false => plain,
    };
    String::from_utf8(plain.to_vec()).map_err(|_| CookieError::Decrypt("value is not UTF-8"))
}

/// The one field of Chromium's large `Local State` file the scan needs.
#[derive(Deserialize)]
struct LocalState {
    profile: Option<LocalStateProfile>,
}

#[derive(Deserialize)]
struct LocalStateProfile {
    last_used: Option<String>,
}

/// Profiles in scan order: Local State's `last_used`, then `Default`, then alphabetical.
pub(crate) fn chromium_profiles(root: &Path) -> Vec<String> {
    let mut profiles: Vec<String> = subdirectories(root)
        .into_iter()
        .filter(|name| name == "Default" || is_numbered_profile(name))
        .collect();
    let last_used = std::fs::read_to_string(root.join("Local State"))
        .ok()
        .and_then(|text| serde_json::from_str::<LocalState>(&text).ok())
        .and_then(|state| state.profile?.last_used);
    profiles.sort_by(|a, b| {
        let rank = |name: &String| (Some(name) != last_used.as_ref(), name != "Default");
        rank(a).cmp(&rank(b)).then_with(|| a.cmp(b))
    });
    profiles
}

fn is_numbered_profile(name: &str) -> bool {
    name.strip_prefix("Profile ")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Reads unexpired cookies for `host` and `.host` from one profile.
///
/// It returns nothing, without touching the Keychain, unless the profile has a
/// session cookie.
pub(crate) fn read_chromium_cookies(
    user_data: &Path,
    profile: &str,
    keychain_service: &str,
    host: &str,
    read_password: &dyn Fn(&str) -> Result<String, CookieError>,
) -> Result<Vec<BrowserCookie>, CookieError> {
    let profile_dir = user_data.join(profile);
    let Some(source) = [
        profile_dir.join("Network").join("Cookies"),
        profile_dir.join("Cookies"),
    ]
    .into_iter()
    .find(|path| path.exists()) else {
        return Ok(Vec::new());
    };
    let sqlite_error = |error| CookieError::Sqlite {
        path: source.clone(),
        source: error,
    };
    let db = open_cookie_db(&source)?;
    let schema: i64 = db
        .query_row("select value from meta where key = 'version'", [], |row| {
            row.get::<_, String>(0)
        })
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let mut statement = db
        .prepare(
            "select host_key, name, value, encrypted_value from cookies where host_key in (?1, ?2) and (expires_utc = 0 or expires_utc > ?3)",
        )
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map(params![host, format!(".{host}"), chromium_now()], |row| {
            Ok(RawCookie {
                domain: row.get(0)?,
                name: row.get(1)?,
                value: row.get(2)?,
                encrypted: row.get(3)?,
            })
        })
        .map_err(sqlite_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sqlite_error)?;
    if !rows.iter().any(|row| is_session_cookie_name(&row.name)) {
        return Ok(Vec::new());
    }
    let mut key = None;
    rows.into_iter()
        .map(|row| {
            let value = if !row.value.is_empty() {
                row.value
            } else {
                let key = match key {
                    Some(key) => key,
                    None => *key.insert(derive_chromium_key(&read_password(keychain_service)?)),
                };
                decrypt_chromium_cookie(&row.encrypted, &key, schema >= 24)?
            };
            Ok(BrowserCookie {
                name: row.name,
                value,
                domain: row.domain,
            })
        })
        .collect()
}

struct RawCookie {
    domain: String,
    name: String,
    value: String,
    encrypted: Vec<u8>,
}

/// Now, in Chromium's microseconds since 1601.
fn chromium_now() -> i64 {
    let since_unix = unix_now();
    let micros = (since_unix.as_secs() + WINDOWS_EPOCH_OFFSET_SECS) as i128 * 1_000_000
        + i128::from(since_unix.subsec_micros());
    i64::try_from(micros).unwrap_or(i64::MAX)
}
