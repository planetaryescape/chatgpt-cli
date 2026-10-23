#![allow(clippy::unwrap_used)]

use std::cell::Cell;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use aes::cipher::{BlockEncryptMut, KeyIvInit};
use rusqlite::{Connection, params};

use super::*;
use crate::cookies::{
    BrowserSelection, CHATGPT_HOST, SESSION_COOKIE,
    chromium::{Encryption, read_chromium_cookies},
    platform::{Platform, linux_root},
    read_browser_session_with,
};

const PASSWORD: &str = "synthetic-password";
const V10_KEY: [u8; 16] = [
    0xfd, 0x62, 0x1f, 0xe5, 0xa2, 0xb4, 0x02, 0x53, 0x9d, 0xfa, 0x14, 0x7c, 0xa9, 0x27, 0x27, 0x78,
];

fn no_keyring(_: &str) -> Result<String, CookieError> {
    Err(CookieError::SecretService(
        "the test expected no keyring lookup",
    ))
}

fn encrypted(prefix: &[u8; 3], key: &[u8; 16], host: Option<&str>, value: &[u8]) -> Vec<u8> {
    let plain = match host {
        Some(host) => [Sha256::digest(host.as_bytes()).as_slice(), value].concat(),
        None => value.to_vec(),
    };
    let mut buffer = plain.clone();
    buffer.resize(plain.len() + 16, 0);
    let cipher = cbc::Encryptor::<aes::Aes128>::new(key.into(), &[b' '; 16].into())
        .encrypt_padded_mut::<Pkcs7>(&mut buffer, plain.len())
        .unwrap();
    [prefix.as_slice(), cipher].concat()
}

fn profile(root: &Path, name: &str, version: Option<&str>) -> Connection {
    let path = root.join(name).join("Network/Cookies");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let db = Connection::open(path).unwrap();
    db.execute_batch("create table cookies (host_key text, name text, value text, encrypted_value blob, expires_utc integer);")
        .unwrap();
    if let Some(version) = version {
        db.execute_batch("create table meta (key text, value text);")
            .unwrap();
        db.execute("insert into meta values ('version', ?1)", [version])
            .unwrap();
    }
    db
}

fn insert(db: &Connection, host: &str, name: &str, value: &str, cipher: &[u8], expires: i64) {
    db.execute(
        "insert into cookies values (?1, ?2, ?3, ?4, ?5)",
        params![host, name, value, cipher, expires],
    )
    .unwrap();
}

fn read(
    root: &Path,
    password: &dyn Fn(&str) -> Result<String, CookieError>,
) -> Result<Vec<crate::cookies::BrowserCookie>, CookieError> {
    read_chromium_cookies(root, "Default", Encryption::Linux, CHATGPT_HOST, password)
}

fn selection(profile: Option<&str>) -> BrowserSelection {
    BrowserSelection {
        browser: Some("chromium".into()),
        profile: profile.map(str::to_owned),
    }
}

#[test]
fn maps_only_known_chromium_desktop_handlers() {
    for name in ["chromium.desktop", "chromium-browser.desktop"] {
        assert_eq!(desktop_handler(name), Some(Browser::Chromium));
    }
    for name in [
        "google-chrome.desktop",
        "firefox.desktop",
        "org.chromium.Chromium.desktop",
        "/usr/share/applications/chromium.desktop",
        "chromium.desktop; evil",
        "",
        "CHROMIUM.desktop",
    ] {
        assert_eq!(desktop_handler(name), None, "{name}");
    }
    assert_eq!(Platform::Linux.browsers(), [Browser::Chromium]);
    assert_eq!(
        Platform::MacOs
            .browsers()
            .iter()
            .map(|browser| browser.name())
            .collect::<Vec<_>>(),
        ["dia", "chrome", "safari", "firefox", "arc", "brave", "edge"]
    );
    assert_eq!(
        Platform::Linux.select("chromium").unwrap(),
        Browser::Chromium
    );
    assert!(Platform::MacOs.select("chromium").is_err());
}

#[test]
fn uses_only_absolute_xdg_config_home() {
    let home = Path::new("/synthetic-home");
    assert_eq!(linux_root(home, None), home.join(".config/chromium"));
    assert_eq!(
        linux_root(home, Some(Path::new(""))),
        home.join(".config/chromium")
    );
    assert_eq!(
        linux_root(home, Some(Path::new("relative"))),
        home.join(".config/chromium")
    );
    assert_eq!(
        linux_root(home, Some(Path::new("/custom"))),
        Path::new("/custom/chromium")
    );
}

#[test]
fn selects_profiles_in_existing_order_and_honours_explicit_selection() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("custom-config");
    let root = config.join("chromium");
    for name in ["Default", "Profile 10", "Profile 2"] {
        let db = profile(&root, name, Some("24"));
        insert(&db, CHATGPT_HOST, SESSION_COOKIE, name, &[], 0);
    }
    std::fs::write(
        root.join("Local State"),
        r#"{"profile":{"last_used":"Profile 2"}}"#,
    )
    .unwrap();
    assert_eq!(
        crate::cookies::chromium_profiles(&root),
        ["Profile 2", "Default", "Profile 10"]
    );
    let get = |selection: BrowserSelection| {
        read_browser_session_with(
            &selection,
            home.path(),
            Some(Browser::Chromium),
            Platform::Linux,
            Some(&config),
            &no_keyring,
        )
    };
    let session = get(BrowserSelection::default()).unwrap();
    assert_eq!(session.profile.as_deref(), Some("Profile 2"));
    assert_eq!(session.session_token().unwrap().value.expose(), "Profile 2");
    let pinned = get(selection(Some("Default"))).unwrap();
    assert_eq!(pinned.session_token().unwrap().value.expose(), "Default");
    let missing = get(selection(Some("Profile 99"))).unwrap_err().to_string();
    assert_eq!(
        missing,
        "Profile \"Profile 99\" was not found in chromium. Available profiles: Profile 2, Default, Profile 10."
    );
    let relative = read_browser_session_with(
        &selection(None),
        home.path(),
        None,
        Platform::Linux,
        Some(Path::new("relative")),
        &no_keyring,
    );
    assert!(matches!(
        relative,
        Err(CookieError::NoSession {
            browser: Browser::Chromium,
            ..
        })
    ));
    let db = profile(&home.path().join(".config/chromium"), "Default", None);
    insert(&db, CHATGPT_HOST, SESSION_COOKIE, "fallback-root", &[], 0);
    assert_eq!(
        read_browser_session_with(
            &selection(None),
            home.path(),
            None,
            Platform::Linux,
            Some(Path::new("relative")),
            &no_keyring
        )
        .unwrap()
        .session_token()
        .unwrap()
        .value
        .expose(),
        "fallback-root"
    );
}

#[test]
fn unsupported_or_missing_default_never_falls_back_to_chromium() {
    let home = tempfile::tempdir().unwrap();
    let db = profile(&home.path().join(".config/chromium"), "Default", None);
    insert(&db, CHATGPT_HOST, SESSION_COOKIE, "exists", &[], 0);
    assert!(matches!(
        read_browser_session_with(
            &BrowserSelection::default(),
            home.path(),
            None,
            Platform::Linux,
            None,
            &no_keyring
        ),
        Err(CookieError::NoDefaultBrowser)
    ));
    for browser in [Browser::Firefox, Browser::Chrome] {
        assert!(
            matches!(read_browser_session_with(&BrowserSelection::default(), home.path(), Some(browser), Platform::Linux, None, &no_keyring), Err(CookieError::UnsupportedBrowser(name)) if name == browser.name())
        );
    }
    for name in ["chrome", "firefox", "safari", "brave", "edge", "arc", "dia"] {
        assert!(
            matches!(read_browser_session_with(&BrowserSelection { browser: Some(name.into()), profile: None }, home.path(), None, Platform::Linux, None, &no_keyring), Err(CookieError::UnsupportedBrowser(rejected)) if rejected == name)
        );
    }
    assert!(matches!(
        read_browser_session_with(
            &BrowserSelection {
                browser: None,
                profile: Some("Default".into())
            },
            home.path(),
            Some(Browser::Chromium),
            Platform::Linux,
            None,
            &no_keyring
        ),
        Err(CookieError::ProfileWithoutBrowser)
    ));
}

#[test]
fn mixed_versions_plaintext_domains_expiry_and_chunks_share_one_lazy_lookup() {
    let root = tempfile::tempdir().unwrap();
    let db = profile(root.path(), "Default", Some("24"));
    insert(
        &db,
        ".chatgpt.com",
        &format!("{SESSION_COOKIE}.1"),
        "",
        &encrypted(
            b"v11",
            &derive_key(PASSWORD),
            Some(".chatgpt.com"),
            b"second",
        ),
        0,
    );
    insert(
        &db,
        CHATGPT_HOST,
        &format!("{SESSION_COOKIE}.0"),
        "",
        &encrypted(b"v10", &V10_KEY, Some(CHATGPT_HOST), b"first"),
        0,
    );
    insert(
        &db,
        CHATGPT_HOST,
        "another",
        "",
        &encrypted(b"v11", &derive_key(PASSWORD), Some(CHATGPT_HOST), b"other"),
        i64::MAX,
    );
    insert(&db, CHATGPT_HOST, "plain", "plaintext", &[], 0);
    insert(&db, CHATGPT_HOST, "empty", "", &[], 0);
    for host in ["evilchatgpt.com", "chatgpt.com.evil", ".sub.chatgpt.com"] {
        insert(&db, host, SESSION_COOKIE, "wrong-domain", b"bad", 0);
    }
    insert(&db, CHATGPT_HOST, SESSION_COOKIE, "expired", b"bad", 1);
    let calls = Cell::new(0);
    let cookies = read(root.path(), &|service| {
        assert_eq!(service, "chromium");
        calls.set(calls.get() + 1);
        Ok(PASSWORD.into())
    })
    .unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(cookies.len(), 5);
    assert_eq!(
        crate::cookies::join_session_token(&cookies)
            .unwrap()
            .value
            .expose(),
        "firstsecond"
    );
    assert_eq!(
        cookies.iter().find(|c| c.name == "plain").unwrap().value,
        "plaintext"
    );
    assert_eq!(
        cookies.iter().find(|c| c.name == "empty").unwrap().value,
        ""
    );
}

#[test]
fn v10_uses_upstream_key_without_reading_secret_service() {
    assert_eq!(derive_key("peanuts"), V10_KEY);
    for version in [None, Some("23"), Some("not-a-number")] {
        let root = tempfile::tempdir().unwrap();
        let db = profile(root.path(), "Default", version);
        insert(
            &db,
            CHATGPT_HOST,
            SESSION_COOKIE,
            "",
            &encrypted(b"v10", &V10_KEY, None, b"legacy"),
            0,
        );
        let cookies = read(root.path(), &no_keyring).unwrap();
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].value, "legacy");
    }
}

#[test]
fn missing_v11_secret_never_tries_the_v10_key_or_another_profile() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".config/chromium");
    let db = profile(&root, "Default", Some("24"));
    insert(
        &db,
        CHATGPT_HOST,
        SESSION_COOKIE,
        "",
        &encrypted(
            b"v11",
            &V10_KEY,
            Some(CHATGPT_HOST),
            b"would-decrypt-with-peanuts",
        ),
        0,
    );
    let other = profile(&root, "Profile 1", Some("24"));
    insert(
        &other,
        CHATGPT_HOST,
        SESSION_COOKIE,
        "other-account",
        &[],
        0,
    );
    let calls = Cell::new(0);
    let error = read_browser_session_with(
        &selection(None),
        home.path(),
        None,
        Platform::Linux,
        None,
        &|_| {
            calls.set(calls.get() + 1);
            Err(CookieError::SecretService(
                "no existing secret was returned",
            ))
        },
    )
    .unwrap_err();
    assert!(matches!(error, CookieError::SecretService(_)));
    assert_eq!(calls.get(), 1);
}

#[test]
fn sessionless_and_expired_profiles_do_not_prompt_for_the_keyring() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".config/chromium");
    let db = profile(&root, "Default", Some("24"));
    insert(&db, CHATGPT_HOST, "unrelated", "", b"v11bad", 0);
    insert(&db, CHATGPT_HOST, SESSION_COOKIE, "", b"v11bad", 1);
    assert!(read(&root, &no_keyring).unwrap().is_empty());
    let next = profile(&root, "Profile 1", None);
    insert(&next, CHATGPT_HOST, SESSION_COOKIE, "next-profile", &[], 0);
    let session = read_browser_session_with(
        &selection(None),
        home.path(),
        None,
        Platform::Linux,
        None,
        &no_keyring,
    )
    .unwrap();
    assert_eq!(session.profile.as_deref(), Some("Profile 1"));
    assert_eq!(
        session.session_token().unwrap().value.expose(),
        "next-profile"
    );
}

#[test]
fn reads_readonly_database_with_uncheckpointed_wal_without_writing_cookies() {
    let root = tempfile::tempdir().unwrap();
    let db = profile(root.path(), "Default", Some("24"));
    assert_eq!(
        db.query_row("pragma journal_mode=WAL", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "wal"
    );
    db.execute_batch("pragma wal_autocheckpoint=0").unwrap();
    insert(&db, CHATGPT_HOST, SESSION_COOKIE, "wal-session", &[], 0);
    let path = root.path().join("Default/Network/Cookies");
    let wal = path.with_file_name("Cookies-wal");
    assert!(std::fs::metadata(&wal).unwrap().len() > 0);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
    let before = (std::fs::read(&path).unwrap(), std::fs::read(&wal).unwrap());
    let cookies = read(root.path(), &no_keyring).unwrap();
    assert_eq!(cookies.len(), 1);
    assert_eq!(cookies[0].value, "wal-session");
    assert_eq!(
        before,
        (std::fs::read(&path).unwrap(), std::fs::read(&wal).unwrap())
    );
}

#[test]
fn invalid_ciphertext_keys_host_hashes_and_utf8_fail_without_values_in_errors() {
    let key = derive_key(PASSWORD);
    let cases = [
        b"v12secret".to_vec(),
        b"v11".to_vec(),
        b"v1".to_vec(),
        vec![],
        b"v11bad-length".to_vec(),
        encrypted(
            b"v11",
            &derive_key("wrong-key"),
            Some(CHATGPT_HOST),
            b"PRIVATE-SENTINEL",
        ),
        encrypted(b"v11", &key, Some("wrong-host"), b"PRIVATE-SENTINEL"),
        encrypted(b"v11", &key, None, b"short"),
        encrypted(b"v11", &key, Some(CHATGPT_HOST), &[0xff]),
    ];
    for cipher in cases {
        let error = Decryptor::default()
            .decrypt(&cipher, CHATGPT_HOST, true, &|_| Ok(PASSWORD.into()))
            .unwrap_err();
        let printed = format!("{error} {error:?}");
        assert!(matches!(error, CookieError::Decrypt(_)));
        assert!(!printed.contains("PRIVATE-SENTINEL"));
        assert!(!printed.contains(PASSWORD));
    }
}

#[test]
fn conflicting_columns_and_invalid_rows_fail_the_read() {
    let root = tempfile::tempdir().unwrap();
    let db = profile(root.path(), "Default", Some("24"));
    insert(
        &db,
        CHATGPT_HOST,
        SESSION_COOKIE,
        "stale-plaintext",
        &encrypted(b"v10", &V10_KEY, Some(CHATGPT_HOST), b"encrypted-session"),
        0,
    );
    assert!(matches!(
        read(root.path(), &no_keyring),
        Err(CookieError::Decrypt(
            "cookie has both plaintext and encrypted values"
        ))
    ));
    db.execute("update cookies set value = ''", []).unwrap();
    assert_eq!(
        read(root.path(), &no_keyring).unwrap()[0].value,
        "encrypted-session"
    );
    insert(
        &db,
        CHATGPT_HOST,
        "broken",
        "",
        &encrypted(b"v10", &V10_KEY, Some("wrong-host"), b"PRIVATE-SENTINEL"),
        0,
    );
    let error = read(root.path(), &no_keyring).unwrap_err();
    assert!(matches!(error, CookieError::Decrypt(_)));
    assert!(!error.to_string().contains("PRIVATE-SENTINEL"));
}

#[test]
fn v11_before_schema_24_has_no_host_hash() {
    let root = tempfile::tempdir().unwrap();
    let db = profile(root.path(), "Default", Some("23"));
    insert(
        &db,
        CHATGPT_HOST,
        SESSION_COOKIE,
        "",
        &encrypted(b"v11", &derive_key(PASSWORD), None, b"older-session"),
        0,
    );
    assert_eq!(
        read(root.path(), &|_| Ok(PASSWORD.into())).unwrap()[0].value,
        "older-session"
    );
}
