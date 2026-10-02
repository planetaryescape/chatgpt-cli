#![allow(dead_code, reason = "each test binary uses a different subset")]
#![allow(clippy::unwrap_used)]

//! The environment for driving the real binary and the daemon it starts:
//! HOME and the XDG directories in a temp dir, so nothing touches the
//! developer's own browser, daemon or TS index. Every test stops its daemon,
//! even when it fails.

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use fake_chatgpt::{COOKIE, FakeChatGpt};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

pub struct Env {
    pub home: tempfile::TempDir,
    pub fake: Option<FakeChatGpt>,
    /// The TS index to import from (it need not exist).
    pub legacy: PathBuf,
    /// A TS CLI stand-in: `CHATGPT_TS_CLI`, run by `/bin/sh`.
    pub ts_cli: Option<PathBuf>,
    pub extra_env: Vec<(String, String)>,
}

impl Env {
    pub fn new() -> Self {
        // Under /tmp, not the platform temp dir: macOS caps a socket path
        // at 104 bytes, and $TMPDIR plus "Library/Application Support/…"
        // is over.
        let home = tempfile::Builder::new()
            .prefix("cg")
            .tempdir_in("/tmp")
            .unwrap();
        // As the TS CLI lays it out: `$XDG_DATA_HOME/chatgpt-cli/index.db`.
        let legacy = home.path().join("ts/chatgpt-cli/index.db");
        Self {
            home,
            fake: None,
            legacy,
            ts_cli: None,
            extra_env: Vec::new(),
        }
    }

    pub fn with_fake(chats: Vec<fake_chatgpt::Chat>) -> Self {
        let mut env = Self::new();
        env.fake = Some(FakeChatGpt::start(chats));
        env
    }

    pub fn fake(&self) -> &FakeChatGpt {
        self.fake.as_ref().expect("a fake chatgpt.com")
    }

    pub fn cmd(&self) -> Command {
        Command::from_std(self.std_cmd())
    }

    /// The binary with this environment, as a plain `std::process::Command`
    /// (for `spawn`).
    pub fn std_cmd(&self) -> std::process::Command {
        let home = self.home.path();
        let mut command = std::process::Command::new(assert_cmd::cargo::cargo_bin!("chatgpt"));
        command
            .env("HOME", home)
            .env("XDG_DATA_HOME", home.join("xdg-data"))
            .env("XDG_CONFIG_HOME", home.join("xdg-config"))
            // The embedding model's cache: never the developer's.
            .env("XDG_CACHE_HOME", home.join("xdg-cache"))
            .env("CHATGPT_LEGACY_DB", &self.legacy)
            .env("CHATGPT_TEST_FAST_RETRY", "1")
            // Vectors without the model, and never a download from the
            // internet (a test that wants the real path overrides both).
            .env("CHATGPT_TEST_EMBEDDER", "fake")
            .env("CHATGPT_MODEL_BASE_URL", "http://127.0.0.1:9")
            .env_remove("CHATGPT_INSTANCE")
            .env_remove("CHATGPT_BROWSER")
            .env_remove("CHATGPT_BROWSER_PROFILE")
            .env_remove("CHATGPT_BRIDGED")
            .env_remove("CHATGPT_DAEMON_VERSION")
            .env_remove("CHATGPT_REQUEST_TIMEOUT_MS")
            .env_remove("CHATGPT_TS_SYNC");
        match &self.fake {
            Some(fake) => {
                command
                    .env("CHATGPT_BASE_URL", &fake.url)
                    .env("CHATGPT_TEST_COOKIE", COOKIE);
            }
            // No cookie and no browser in this HOME: any session read
            // fails fast instead of reaching chatgpt.com.
            None => {
                command
                    .env("CHATGPT_BASE_URL", "http://127.0.0.1:9")
                    .env_remove("CHATGPT_TEST_COOKIE");
            }
        }
        match &self.ts_cli {
            Some(script) => {
                command
                    .env("CHATGPT_TS_CLI", script)
                    .env("CHATGPT_BUN", "/bin/sh");
            }
            None => {
                command
                    .env_remove("CHATGPT_TS_CLI")
                    .env_remove("CHATGPT_BUN");
            }
        }
        for (name, value) in &self.extra_env {
            command.env(name, value);
        }
        command
    }

    /// Run `args`, expect success, return stdout.
    pub fn stdout(&self, args: &[&str]) -> String {
        let output = self.cmd().args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{args:?} failed: {}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    pub fn json(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.stdout(args)).unwrap()
    }

    pub fn status(&self) -> Value {
        self.json(&["daemon", "status", "--json"])
    }

    /// Wait until the search indexer has nothing running or pending, so a
    /// test that counts requests to the fake counts only its own. Returns
    /// the indexer's status.
    pub fn wait_for_indexer(&self) -> Value {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let index = self.status()["search_index"].clone();
            if index["in_progress"] == false {
                return index;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the indexer never went idle: {index}"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    /// Wait until the indexer and then the embedder have nothing running
    /// or pending. Returns the embedder's status.
    pub fn wait_for_embedder(&self) -> Value {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
        loop {
            let index = self.status()["search_index"].clone();
            let embeddings = index["embeddings"].clone();
            assert!(
                embeddings["embedded"].as_u64() <= embeddings["chunks"].as_u64(),
                "more chunks embedded than there are: {embeddings}"
            );
            if index["in_progress"] == false && embeddings["in_progress"] == false {
                return embeddings;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the embedder never went idle: {index}"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    pub fn data_dir(&self) -> PathBuf {
        self.home
            .path()
            .join("Library/Application Support/chatgpt-cli-dev")
    }

    pub fn socket(&self) -> PathBuf {
        self.data_dir().join("run/daemon.sock")
    }

    /// A TS index at `self.legacy` with the TS CLI's schema.
    pub fn legacy_db(&self) -> rusqlite::Connection {
        std::fs::create_dir_all(self.legacy.parent().unwrap()).unwrap();
        let db = rusqlite::Connection::open(&self.legacy).unwrap();
        db.execute_batch(TS_SCHEMA).unwrap();
        db
    }

    /// A TS CLI stand-in script with `body` as its shell code.
    pub fn fake_ts_cli(&mut self, body: &str) -> PathBuf {
        let dir = self.home.path().join("ts-cli/src");
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("cli.ts");
        std::fs::write(&script, body).unwrap();
        self.ts_cli = Some(script.clone());
        script
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = self.cmd().args(["daemon", "stop"]).output();
    }
}

pub fn pid_exists(pid: u64) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

pub fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// The TS CLI's tables, which the daemon's first migration copies column
/// for column.
pub const TS_SCHEMA: &str = include_str!("../../../store/migrations/0001_index.sql");

/// Jev answers that make a confident delete.
pub fn delete_answers(topic: &str) -> String {
    serde_json::json!({
        "worth_keeping": { "score": 0.2, "confidence": 0.9 }, "nothing_there": { "noul": 0.95 },
        "unfinished": { "noul": 0.05 }, "personal_record": { "noul": 0.05 }, "re_askable": { "noul": 0.9 },
        "brainstorming": { "noul": 0.05 }, "brainstorm_for": { "choice": "none" }, "topic": { "choice": topic },
    })
    .to_string()
}

/// Jev answers that make a kept writing brainstorm.
pub fn brainstorm_answers(topic: &str) -> String {
    serde_json::json!({
        "worth_keeping": { "score": 2.8, "confidence": 0.9 }, "nothing_there": { "noul": 0.02 },
        "unfinished": { "noul": 0.4 }, "personal_record": { "noul": 0.2 }, "re_askable": { "noul": 0.1 },
        "brainstorming": { "noul": 0.95 }, "brainstorm_for": { "choice": "writing" }, "topic": { "choice": topic },
    })
    .to_string()
}

/// This repository's QUESTIONS_VERSION: the daemon reads the built-in
/// versions when no TS CLI is installed.
pub const QUESTIONS_VERSION: &str = "2026-09-28.9";

pub fn judge(db: &rusqlite::Connection, id: &str, update_time: &str, answers: &str) {
    db.execute(
        "insert or replace into judgments (id, update_time, version, content_kind, answers, classified_at)
         values (?, ?, ?, 'full', ?, '2026-09-28T00:00:00Z')",
        rusqlite::params![id, update_time, QUESTIONS_VERSION, answers],
    )
    .unwrap();
}

/// This repository, for the TS CLI the parity harnesses run.
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// bun on PATH, if any: the parity harnesses skip without it.
pub fn bun() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("bun"))
        .find(|candidate| candidate.is_file())
}

/// Copy the Rust index's chats into the TS index (the same rows in the
/// same order, and the same `synced_at`), so both CLIs read the same data.
/// Returns how many chats it copied.
pub fn copy_chats(rust_index: &Path, ts_index: &Path) -> usize {
    let rust = Connection::open_with_flags(rust_index, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let ts = Connection::open(ts_index).unwrap();
    ts.execute("delete from conversations", []).unwrap();
    let mut rows = rust
        .prepare("select id, title, create_time, update_time, is_archived, pinned, project_id from conversations order by rowid")
        .unwrap();
    let mut insert = ts
        .prepare("insert into conversations values (?, ?, ?, ?, ?, ?, ?)")
        .unwrap();
    let mut copied = 0;
    let mut query = rows.query([]).unwrap();
    while let Some(row) = query.next().unwrap() {
        let values: Vec<rusqlite::types::Value> = (0..7).map(|i| row.get(i).unwrap()).collect();
        insert.execute(rusqlite::params_from_iter(values)).unwrap();
        copied += 1;
    }
    let synced: String = rust
        .query_row(
            "select value from meta where key = 'synced_at'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    ts.execute(
        "insert or replace into meta values ('synced_at', ?)",
        [synced],
    )
    .unwrap();
    copied
}
