//! Safari's `Cookies.binarycookies`.
//!
//! Ported from the TS CLI's `src/auth/safari-cookies.ts` @ 1b8c950.

use std::path::Path;

use super::{BrowserCookie, CookieError, unix_now};

const SAFARI_PATHS: [&str; 2] = [
    "Library/Containers/com.apple.Safari/Data/Library/Cookies/Cookies.binarycookies",
    "Library/Cookies/Cookies.binarycookies",
];
/// Seconds between the Unix epoch and Apple's (2001-01-01).
const MAC_EPOCH_SECONDS: f64 = 978_307_200.0;
const RECORD_HEADER: usize = 56;

fn invalid(message: &'static str) -> CookieError {
    CookieError::InvalidSafariFile(message)
}

fn bytes<const N: usize>(data: &[u8], at: usize) -> Result<[u8; N], CookieError> {
    data.get(at..at + N)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(invalid("Truncated Safari cookie file."))
}

fn u32_be(data: &[u8], at: usize) -> Result<usize, CookieError> {
    Ok(u32::from_be_bytes(bytes(data, at)?) as usize)
}

fn u32_le(data: &[u8], at: usize) -> Result<usize, CookieError> {
    Ok(u32::from_le_bytes(bytes(data, at)?) as usize)
}

fn f64_le(data: &[u8], at: usize) -> Result<f64, CookieError> {
    Ok(f64::from_le_bytes(bytes(data, at)?))
}

fn c_string(record: &[u8], offset: usize) -> Result<String, CookieError> {
    let tail = record
        .get(offset..)
        .filter(|tail| !tail.is_empty())
        .ok_or(invalid("Invalid Safari cookie string offset."))?;
    let end = tail
        .iter()
        .position(|&byte| byte == 0)
        .ok_or(invalid("Unterminated Safari cookie string."))?;
    Ok(String::from_utf8_lossy(&tail[..end]).into_owned())
}

/// Unexpired cookies for `host` and `.host`.
pub(crate) fn parse_safari_cookies(
    data: &[u8],
    host: &str,
) -> Result<Vec<BrowserCookie>, CookieError> {
    if data.len() < 8 || &data[..4] != b"cook" {
        return Err(invalid("Invalid Safari cookie file."));
    }
    let pages = u32_be(data, 4)?;
    if pages > (data.len() - 8) / 4 {
        return Err(invalid("Invalid Safari cookie page count."));
    }
    let now = unix_now().as_secs_f64();
    let dot_host = format!(".{host}");
    let mut position = 8 + pages * 4;
    let mut cookies = Vec::new();
    for page_index in 0..pages {
        let size = u32_be(data, 8 + page_index * 4)?;
        if size < 8 || position + size > data.len() {
            return Err(invalid("Invalid Safari cookie page size."));
        }
        let page = &data[position..position + size];
        position += size;
        if u32_be(page, 0)? != 256 {
            return Err(invalid("Invalid Safari cookie page header."));
        }
        let count = u32_le(page, 4)?;
        if count > (page.len() - 8) / 4 {
            return Err(invalid("Invalid Safari cookie record count."));
        }
        for i in 0..count {
            let offset = u32_le(page, 8 + i * 4)?;
            if offset + RECORD_HEADER > page.len() {
                return Err(invalid("Invalid Safari cookie record offset."));
            }
            let record_size = u32_le(page, offset)?;
            if record_size < RECORD_HEADER || offset + record_size > page.len() {
                return Err(invalid("Invalid Safari cookie record size."));
            }
            let record = &page[offset..offset + record_size];
            let domain = c_string(record, u32_le(record, 16)?)?;
            if domain != host && domain != dot_host {
                continue;
            }
            let expiry = f64_le(record, 40)?;
            if expiry != 0.0 && expiry + MAC_EPOCH_SECONDS <= now {
                continue;
            }
            cookies.push(BrowserCookie {
                name: c_string(record, u32_le(record, 20)?)?,
                value: c_string(record, u32_le(record, 28)?)?,
                domain,
            });
        }
    }
    Ok(cookies)
}

pub(crate) fn read_safari_cookies(
    host: &str,
    home: &Path,
) -> Result<Vec<BrowserCookie>, CookieError> {
    for relative in SAFARI_PATHS {
        let path = home.join(relative);
        match std::fs::read(&path) {
            Ok(data) => return parse_safari_cookies(&data, host),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err(CookieError::SafariAccessDenied);
            }
            Err(source) => return Err(CookieError::Io { path, source }),
        }
    }
    Ok(Vec::new())
}
