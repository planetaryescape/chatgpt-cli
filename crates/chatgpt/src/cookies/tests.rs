//! Mirrors the TS CLI's `src/auth/browser-cookies.test.ts` @ 1b8c950, plus
//! the cases it leaves implicit.

#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use aes::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};

use super::chromium::{decrypt_chromium_cookie, derive_chromium_key};
use super::safari::parse_safari_cookies;
use super::*;

const TOKEN: &str = SESSION_COOKIE;
const PASSWORD: &str = "test-password";

fn test_password(_service: &str) -> Result<String, CookieError> {
    Ok(PASSWORD.to_owned())
}

fn no_keychain(service: &str) -> Result<String, CookieError> {
    Err(CookieError::Keychain {
        service: service.to_owned(),
        detail: "the test expected no Keychain read".to_owned(),
    })
}

fn encrypt(plain: &[u8], key: &[u8; 16]) -> Vec<u8> {
    let mut buffer = plain.to_vec();
    buffer.resize(plain.len() + 16, 0);
    let ciphertext = cbc::Encryptor::<aes::Aes128>::new(key.into(), &[b' '; 16].into())
        .encrypt_padded_mut::<Pkcs7>(&mut buffer, plain.len())
        .unwrap();
    [b"v10".as_slice(), ciphertext].concat()
}

/// Encrypts `value` the way schema 24+ stores it: SHA-256(host) first.
fn encrypt_v24(host: &str, value: &str) -> Vec<u8> {
    let plain = [Sha256::digest(host.as_bytes()).as_slice(), value.as_bytes()].concat();
    encrypt(&plain, &derive_chromium_key(PASSWORD))
}

fn chrome_root(home: &Path) -> PathBuf {
    home.join("Library/Application Support/Google/Chrome")
}

/// A Chrome profile whose Cookies DB holds a session cookie plus a lookalike-domain one.
fn chrome_fixture(home: &Path, profile: &str, value: &str, encrypted: bool) -> PathBuf {
    let root = chrome_root(home);
    let path = root.join(profile).join("Cookies");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let db = Connection::open(&path).unwrap();
    db.execute_batch(
        "create table meta (key text, value text);
         insert into meta values ('version', '24');
         create table cookies (host_key text, name text, value text, encrypted_value blob, expires_utc integer);",
    )
    .unwrap();
    let (plain, cipher) = match encrypted {
        true => (String::new(), encrypt_v24(".chatgpt.com", value)),
        false => (value.to_owned(), Vec::new()),
    };
    let insert = "insert into cookies values (?1, ?2, ?3, ?4, ?5)";
    db.execute(insert, params![".chatgpt.com", TOKEN, plain, cipher, 0])
        .unwrap();
    db.execute(
        insert,
        params![
            "evilchatgpt.com",
            TOKEN,
            "wrong-domain",
            Vec::<u8>::new(),
            0
        ],
    )
    .unwrap();
    root
}

fn firefox_fixture(home: &Path) -> PathBuf {
    let profile = home.join("Library/Application Support/Firefox/Profiles/test.default");
    std::fs::create_dir_all(&profile).unwrap();
    let db = Connection::open(profile.join("cookies.sqlite")).unwrap();
    db.execute_batch("create table moz_cookies (host text, name text, value text, expiry integer, originAttributes text);")
        .unwrap();
    let future = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        + 3600;
    let insert = "insert into moz_cookies values (?1, ?2, ?3, ?4, ?5)";
    db.execute(
        insert,
        params![".chatgpt.com", TOKEN, "firefox-session", future, ""],
    )
    .unwrap();
    db.execute(
        insert,
        params![
            ".chatgpt.com",
            TOKEN,
            "container-session",
            future,
            "^userContextId=1"
        ],
    )
    .unwrap();
    db.execute(insert, params![".chatgpt.com", "expired", "old", 1, ""])
        .unwrap();
    profile
}

fn cookie(name: &str, value: &str) -> BrowserCookie {
    BrowserCookie {
        name: name.to_owned(),
        value: value.to_owned(),
        domain: ".chatgpt.com".to_owned(),
    }
}

fn session_for(
    selection: BrowserSelection,
    home: &Path,
    preferred: Option<Browser>,
) -> Result<BrowserSession, CookieError> {
    read_browser_session_with(&selection, home, preferred, &test_password)
}

fn chrome(profile: Option<&str>) -> BrowserSelection {
    BrowserSelection {
        browser: Some("chrome".to_owned()),
        profile: profile.map(str::to_owned),
    }
}

#[test]
fn maps_macos_default_browser_bundle_ids() {
    assert_eq!(
        Browser::for_bundle_id("com.google.Chrome"),
        Some(Browser::Chrome)
    );
    assert_eq!(
        Browser::for_bundle_id("com.apple.Safari"),
        Some(Browser::Safari)
    );
    assert_eq!(
        Browser::for_bundle_id("company.thebrowser.dia"),
        Some(Browser::Dia)
    );
    assert_eq!(
        Browser::for_bundle_id("company.thebrowser.Browser"),
        Some(Browser::Arc)
    );
    assert_eq!(Browser::for_bundle_id("unknown.browser"), None);
}

#[test]
fn picks_the_most_recently_set_https_handler() {
    let handler = |scheme: &str, bundle: &str, date: plist::Value| {
        let mut dict = plist::Dictionary::new();
        dict.insert("LSHandlerURLScheme".into(), scheme.into());
        dict.insert("LSHandlerRoleAll".into(), bundle.into());
        dict.insert("LSHandlerModificationDate".into(), date);
        plist::Value::Dictionary(dict)
    };
    let mut root = plist::Dictionary::new();
    root.insert(
        "LSHandlers".into(),
        plist::Value::Array(vec![
            handler("https", "com.google.chrome", plist::Value::Real(100.0)),
            handler(
                "https",
                "company.thebrowser.dia",
                plist::Value::Integer(200.into()),
            ),
            // A newer handler for another scheme must not win.
            handler("mailto", "com.apple.safari", plist::Value::Real(300.0)),
        ]),
    );
    assert_eq!(
        default_browser_from_plist(&plist::Value::Dictionary(root)),
        Some(Browser::Dia)
    );
}

#[test]
fn keeps_the_first_https_handler_when_dates_tie() {
    let handler = |bundle: &str| {
        let mut dict = plist::Dictionary::new();
        dict.insert("LSHandlerURLScheme".into(), "https".into());
        dict.insert("LSHandlerRoleAll".into(), bundle.into());
        plist::Value::Dictionary(dict)
    };
    let mut root = plist::Dictionary::new();
    root.insert(
        "LSHandlers".into(),
        plist::Value::Array(vec![
            handler("company.thebrowser.dia"),
            handler("com.google.chrome"),
        ]),
    );
    assert_eq!(
        default_browser_from_plist(&plist::Value::Dictionary(root)),
        Some(Browser::Dia)
    );
}

#[test]
fn decrypts_chrome_session_cookies_and_ignores_lookalike_domains() {
    let home = tempfile::tempdir().unwrap();
    let root = chrome_fixture(home.path(), "Default", "chrome-session", true);
    let cookies = read_chromium_cookies(
        &root,
        "Default",
        "Chrome Safe Storage",
        CHATGPT_HOST,
        &test_password,
    )
    .unwrap();
    assert_eq!(cookies, vec![cookie(TOKEN, "chrome-session")]);
}

#[test]
fn strips_the_host_hash_only_from_schema_24_cookies() {
    let key = derive_chromium_key(PASSWORD);
    let v24 = encrypt_v24(".chatgpt.com", "value");
    assert_eq!(decrypt_chromium_cookie(&v24, &key, true).unwrap(), "value");
    let older = encrypt(b"plain-value", &key);
    assert_eq!(
        decrypt_chromium_cookie(&older, &key, false).unwrap(),
        "plain-value"
    );
}

#[test]
fn rejects_unsupported_encryption_and_wrong_keys() {
    let key = derive_chromium_key(PASSWORD);
    let mut v11 = encrypt(b"value", &key);
    v11[2] = b'1';
    v11[1] = b'1';
    assert!(matches!(
        decrypt_chromium_cookie(&v11, &key, false),
        Err(CookieError::UnsupportedEncryption(prefix)) if prefix == "v11"
    ));
    let wrong = derive_chromium_key("another-password");
    assert!(matches!(
        decrypt_chromium_cookie(&encrypt(b"value", &key), &wrong, false),
        Err(CookieError::Decrypt(_))
    ));
}

#[test]
fn skips_the_keychain_when_a_profile_has_no_session() {
    let home = tempfile::tempdir().unwrap();
    let root = chrome_fixture(home.path(), "Default", "chrome-session", true);
    let cookies = read_chromium_cookies(
        &root,
        "Default",
        "Chrome Safe Storage",
        "other.example",
        &no_keychain,
    )
    .unwrap();
    assert!(cookies.is_empty());
}

#[test]
fn reads_a_browser_login_still_in_sqlites_write_ahead_log() {
    let home = tempfile::tempdir().unwrap();
    let root = chrome_fixture(home.path(), "Default", "old-session", false);
    let db = Connection::open(root.join("Default/Cookies")).unwrap();
    let mode: String = db
        .query_row("pragma journal_mode = WAL", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    db.execute_batch("pragma wal_autocheckpoint = 0;").unwrap();
    db.execute(
        "update cookies set value = ?1 where host_key = ?2",
        params!["fresh-session", ".chatgpt.com"],
    )
    .unwrap();
    // The writer stays open, so the update lives only in the WAL.
    let cookies = read_chromium_cookies(
        &root,
        "Default",
        "Chrome Safe Storage",
        CHATGPT_HOST,
        &no_keychain,
    )
    .unwrap();
    assert_eq!(cookies[0].value, "fresh-session");
    drop(db);
}

#[test]
fn drops_expired_chromium_cookies() {
    let home = tempfile::tempdir().unwrap();
    let root = chrome_fixture(home.path(), "Default", "chrome-session", false);
    let db = Connection::open(root.join("Default/Cookies")).unwrap();
    // 1 µs after 1601: long expired.
    db.execute("update cookies set expires_utc = 1", [])
        .unwrap();
    drop(db);
    let cookies = read_chromium_cookies(
        &root,
        "Default",
        "Chrome Safe Storage",
        CHATGPT_HOST,
        &no_keychain,
    )
    .unwrap();
    assert!(cookies.is_empty());
}

#[test]
fn reads_only_the_default_firefox_container_and_unexpired_cookies() {
    let home = tempfile::tempdir().unwrap();
    let cookies = read_firefox_cookies(&firefox_fixture(home.path()), CHATGPT_HOST).unwrap();
    assert_eq!(cookies, vec![cookie(TOKEN, "firefox-session")]);
}

fn safari_record(domain: &str, value: &str, expiry: f64) -> Vec<u8> {
    let strings: Vec<Vec<u8>> = [domain, TOKEN, "/", value]
        .iter()
        .map(|part| format!("{part}\0").into_bytes())
        .collect();
    let mut record = vec![0u8; 56];
    let total = 56 + strings.iter().map(Vec::len).sum::<usize>();
    record[0..4].copy_from_slice(&(total as u32).to_le_bytes());
    record[8..12].copy_from_slice(&5u32.to_le_bytes());
    let mut offset = 56u32;
    for (i, part) in strings.iter().enumerate() {
        record[16 + i * 4..20 + i * 4].copy_from_slice(&offset.to_le_bytes());
        offset += part.len() as u32;
    }
    record[40..48].copy_from_slice(&expiry.to_le_bytes());
    record.extend(strings.concat());
    record
}

fn safari_file(records: &[Vec<u8>]) -> Vec<u8> {
    let header = 8 + records.len() * 4 + 4;
    let mut page = vec![0u8; header];
    page[0..4].copy_from_slice(&256u32.to_be_bytes());
    page[4..8].copy_from_slice(&(records.len() as u32).to_le_bytes());
    let mut offset = header;
    for (i, record) in records.iter().enumerate() {
        page[8 + i * 4..12 + i * 4].copy_from_slice(&(offset as u32).to_le_bytes());
        offset += record.len();
    }
    page.extend(records.concat());
    let mut file = b"cook".to_vec();
    file.extend(1u32.to_be_bytes());
    file.extend((page.len() as u32).to_be_bytes());
    file.extend(page);
    file
}

#[test]
fn parses_safari_cookies_without_other_domains_or_expired_ones() {
    let in_an_hour = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
        - 978_307_200.0
        + 3600.0;
    let file = safari_file(&[
        safari_record(".chatgpt.com", "safari-session", in_an_hour),
        safari_record("other.example", "wrong-domain", in_an_hour),
        safari_record("chatgpt.com", "expired", 1.0),
    ]);
    assert_eq!(
        parse_safari_cookies(&file, CHATGPT_HOST).unwrap(),
        vec![cookie(TOKEN, "safari-session")]
    );
}

#[test]
fn rejects_corrupt_safari_files() {
    assert!(parse_safari_cookies(b"nope", CHATGPT_HOST).is_err());
    let mut file = safari_file(&[safari_record(".chatgpt.com", "v", 0.0)]);
    file.truncate(file.len() - 10);
    assert!(parse_safari_cookies(&file, CHATGPT_HOST).is_err());
}

#[test]
fn orders_chromium_profiles_by_last_used_then_default_then_name() {
    let root = tempfile::tempdir().unwrap();
    for dir in [
        "Profile 2",
        "Default",
        "Profile 1",
        "System Profile",
        "Guest Profile",
    ] {
        std::fs::create_dir_all(root.path().join(dir)).unwrap();
    }
    assert_eq!(
        chromium_profiles(root.path()),
        ["Default", "Profile 1", "Profile 2"]
    );
    std::fs::write(
        root.path().join("Local State"),
        r#"{"profile":{"last_used":"Profile 2"}}"#,
    )
    .unwrap();
    assert_eq!(
        chromium_profiles(root.path()),
        ["Profile 2", "Default", "Profile 1"]
    );
}

#[test]
fn orders_firefox_profiles_by_the_install_default() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("Profiles");
    for dir in ["a.default", "b.default-release"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    assert_eq!(firefox_profiles(&root), ["a.default", "b.default-release"]);
    std::fs::write(
        home.path().join("profiles.ini"),
        "[Install4F96D1932A9F858E]\nDefault=Profiles/b.default-release\n\n[Profile0]\nPath=Profiles/a.default\nDefault=1\n",
    )
    .unwrap();
    assert_eq!(firefox_profiles(&root), ["b.default-release", "a.default"]);
    std::fs::write(
        home.path().join("profiles.ini"),
        "[Profile1]\nPath=Profiles/b.default-release\nDefault=1\n",
    )
    .unwrap();
    assert_eq!(firefox_profiles(&root), ["b.default-release", "a.default"]);
}

#[test]
fn chooses_the_default_browser_and_the_most_recently_used_chrome_profile() {
    let home = tempfile::tempdir().unwrap();
    firefox_fixture(home.path());
    let chrome_dir = chrome_fixture(home.path(), "Default", "old-profile", false);
    chrome_fixture(home.path(), "Profile 1", "current-profile", false);
    std::fs::write(
        chrome_dir.join("Local State"),
        r#"{"profile":{"last_used":"Profile 1"}}"#,
    )
    .unwrap();

    let firefox = session_for(
        BrowserSelection::default(),
        home.path(),
        Some(Browser::Firefox),
    )
    .unwrap();
    assert_eq!(firefox.browser, Browser::Firefox);
    let current = session_for(chrome(None), home.path(), None).unwrap();
    assert_eq!(
        (
            current.profile.as_deref(),
            current.cookies[0].value.as_str()
        ),
        (Some("Profile 1"), "current-profile")
    );
    let pinned = session_for(chrome(Some("Default")), home.path(), None).unwrap();
    assert_eq!(pinned.cookies[0].value, "old-profile");
}

#[test]
fn scans_past_profiles_without_a_session() {
    let home = tempfile::tempdir().unwrap();
    let root = chrome_fixture(home.path(), "Profile 1", "second-profile", false);
    std::fs::create_dir_all(root.join("Default")).unwrap();
    let session = session_for(chrome(None), home.path(), None).unwrap();
    assert_eq!(session.profile.as_deref(), Some("Profile 1"));
}

#[test]
fn does_not_switch_accounts_by_falling_back_to_another_browser() {
    let home = tempfile::tempdir().unwrap();
    chrome_fixture(home.path(), "Default", "chrome-session", false);
    let error = session_for(
        BrowserSelection::default(),
        home.path(),
        Some(Browser::Firefox),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("No ChatGPT session in firefox"),
        "{error}"
    );
}

#[test]
fn validates_the_selection() {
    let home = tempfile::tempdir().unwrap();
    let profile_only = BrowserSelection {
        browser: None,
        profile: Some("Default".to_owned()),
    };
    assert!(matches!(
        session_for(profile_only, home.path(), None),
        Err(CookieError::ProfileWithoutBrowser)
    ));
    let unknown = BrowserSelection {
        browser: Some("netscape".to_owned()),
        profile: None,
    };
    assert_eq!(
        session_for(unknown, home.path(), None)
            .unwrap_err()
            .to_string(),
        "Unsupported browser \"netscape\". Choose dia, chrome, safari, firefox, arc, brave, edge."
    );
    assert!(matches!(
        session_for(BrowserSelection::default(), home.path(), None),
        Err(CookieError::NoDefaultBrowser)
    ));
    chrome_fixture(home.path(), "Default", "chrome-session", false);
    assert_eq!(
        session_for(chrome(Some("Profile 9")), home.path(), None)
            .unwrap_err()
            .to_string(),
        "Profile \"Profile 9\" was not found in chrome. Available profiles: Default."
    );
}

#[test]
fn joins_chunked_session_tokens_in_numeric_order() {
    let chunked = [
        cookie(&format!("{TOKEN}.1"), "BBB"),
        cookie("other", "x"),
        cookie(&format!("{TOKEN}.10"), "K"),
        cookie(&format!("{TOKEN}.0"), "AAA"),
        cookie(&format!("{TOKEN}.2"), "CCC"),
    ];
    let token = join_session_token(&chunked).unwrap();
    assert_eq!((token.value.expose(), token.chunks), ("AAABBBCCCK", 4));
    let whole = [
        cookie(&format!("{TOKEN}.0"), "stale"),
        cookie(TOKEN, "whole"),
    ];
    assert_eq!(join_session_token(&whole).unwrap().value.expose(), "whole");
    assert!(join_session_token(&[cookie("other", "x")]).is_none());
}

#[test]
fn debug_output_never_contains_cookie_values() {
    let session = BrowserSession {
        browser: Browser::Dia,
        profile: Some("Default".to_owned()),
        cookies: vec![cookie(TOKEN, "super-secret-value")],
    };
    let printed = format!("{session:?} {:?}", session.session_token().unwrap());
    assert!(!printed.contains("super-secret-value"), "{printed}");
    assert_eq!(
        session.cookie_header(),
        format!("{TOKEN}=super-secret-value")
    );
}

#[test]
fn recognises_only_whole_or_numbered_session_cookies() {
    assert!(is_session_cookie_name(TOKEN));
    assert!(is_session_cookie_name(&format!("{TOKEN}.0")));
    assert!(is_session_cookie_name(&format!("{TOKEN}.12")));
    assert!(!is_session_cookie_name(&format!("{TOKEN}.")));
    assert!(!is_session_cookie_name(&format!("{TOKEN}.legacy")));
    assert!(!is_session_cookie_name(&format!("{TOKEN}-old")));
    assert!(!is_session_cookie_name("__Secure-next-auth.callback-url"));
}
