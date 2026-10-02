#![allow(dead_code, reason = "each test binary uses a different subset")]
#![allow(clippy::unwrap_used)]

//! The environment for driving the real binary and the daemon it starts:
//! HOME and the XDG directories in a temp dir, so nothing touches the
//! developer's own browser or daemon. Every test stops its daemon,
//! even when it fails.

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use fake_chatgpt::{COOKIE, FakeChatGpt};
use serde_json::Value;

pub struct Env {
    pub home: tempfile::TempDir,
    pub fake: Option<FakeChatGpt>,
    pub extra_env: Vec<(String, String)>,
    /// Stand-in `codex` and `claude` (the debug binary playing them), first
    /// on `PATH`, so no test ever reaches the real ones.
    pub tools: PathBuf,
    /// Where they record their calls.
    pub model_log: PathBuf,
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
        let tools = home.path().join("tools");
        let model_log = home.path().join("model-calls");
        fake_tools(&tools, &model_log, "ok", &["codex", "claude"]);
        Self {
            home,
            fake: None,
            extra_env: Vec::new(),
            tools,
            model_log,
        }
    }

    /// The stand-in tools: `mode` `ok` or `fail`, and only `tools` of
    /// `codex` and `claude` on `PATH`.
    pub fn set_tools(&self, mode: &str, tools: &[&str]) {
        let _ = std::fs::remove_dir_all(&self.tools);
        fake_tools(&self.tools, &self.model_log, mode, tools);
    }

    /// The calls the stand-in tools recorded, in the order they started.
    pub fn model_calls(&self) -> Vec<Value> {
        model_calls(&self.model_log)
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
            .env("PATH", tools_path(&self.tools))
            .env("XDG_DATA_HOME", home.join("xdg-data"))
            .env("XDG_CONFIG_HOME", home.join("xdg-config"))
            // The embedding model's cache: never the developer's.
            .env("XDG_CACHE_HOME", home.join("xdg-cache"))
            .env("CHATGPT_TEST_FAST_RETRY", "1")
            // Vectors without the model, and never a download from the
            // internet (a test that wants the real path overrides both).
            .env("CHATGPT_TEST_EMBEDDER", "fake")
            .env("CHATGPT_MODEL_BASE_URL", "http://127.0.0.1:9")
            .env_remove("CHATGPT_INSTANCE")
            .env_remove("CHATGPT_BROWSER")
            .env_remove("CHATGPT_BROWSER_PROFILE")
            .env_remove("CHATGPT_DAEMON_VERSION")
            .env_remove("CHATGPT_REQUEST_TIMEOUT_MS")
            // Jev: never the developer's key or TypeSafe itself.
            .env_remove("TYPESAFE_API_KEY")
            .env("TYPESAFE_BASE_URL", "http://127.0.0.1:9")
            // Nor OpenAI's or Anthropic's.
            .env_remove("OPENAI_API_KEY")
            .env_remove("ANTHROPIC_API_KEY")
            .env("CHATGPT_TEST_OPENAI_URL", "http://127.0.0.1:9")
            .env("CHATGPT_TEST_ANTHROPIC_URL", "http://127.0.0.1:9")
            .env_remove("CHATGPT_TEST_AUTO_JEV_LIMIT")
            .env_remove("CHATGPT_TEST_AUTO_JEV_SINCE");
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
        // Generous: a test's 2 MB chat through a debug build's HTTP stack
        // on a busy machine can take most of a minute.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
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
            let status = self.status();
            let index = status["search_index"].clone();
            let embeddings = status["embeddings"].clone();
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

    /// The daemon's own index, to write fixtures into. It exists once the
    /// daemon started.
    pub fn index_db(&self) -> rusqlite::Connection {
        let db = rusqlite::Connection::open(self.data_dir().join("chatgpt.db")).unwrap();
        db.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
        db
    }
}

/// Run `args` in a pseudo-terminal, as a person at a terminal would: once
/// `prompt` shows (or the command ends), type `answer`. With `stdin_ids`,
/// the command reads them from a pipe and the answer from `/dev/tty`, as
/// `… | chatgpt archive -` does. Returns the exit status and everything the
/// terminal showed, with `\r\n` as `\n`.
pub fn in_terminal(
    env: &Env,
    args: &[&str],
    stdin_ids: Option<&str>,
    prompt: &str,
    answer: &str,
) -> (Option<i32>, String) {
    let mut pty = Pty::spawn(env, args, stdin_ids, 40, 200);
    // Room for a 2 MB chat's download on a busy machine before the prompt.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while !pty.text().contains(prompt) && !pty.exited() {
        assert!(
            std::time::Instant::now() < deadline,
            "never prompted {prompt:?}: {}",
            pty.text()
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    pty.send(&format!("{answer}\n"));
    pty.finish_keeping_text()
}

/// The binary in a pseudo-terminal (through `script`), `rows` × `cols`,
/// driven key by key. What it shows is read both as a stream (for
/// line-by-line output such as `review`'s) and as a screen (a vt100
/// emulator, for the TUI, which redraws only the cells that changed).
pub struct Pty {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    shown: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    reader: Option<std::thread::JoinHandle<()>>,
    /// How far `wait_for` has read the stream.
    seen: usize,
    rows: u16,
    cols: u16,
}

impl Pty {
    /// With `stdin_ids`, they're piped to the command's stdin, as
    /// `… | chatgpt review -` does, and keys still come from the terminal.
    pub fn spawn(env: &Env, args: &[&str], stdin_ids: Option<&str>, rows: u16, cols: u16) -> Self {
        Self::spawn_with(env, args, stdin_ids, rows, cols, None)
    }

    /// [`Pty::spawn`], then the shell command `then` in the same terminal
    /// once the binary ends (`$?` is its exit status).
    pub fn spawn_then(env: &Env, args: &[&str], rows: u16, cols: u16, then: &str) -> Self {
        Self::spawn_with(env, args, None, rows, cols, Some(then))
    }

    fn spawn_with(
        env: &Env,
        args: &[&str],
        stdin_ids: Option<&str>,
        rows: u16,
        cols: u16,
        then: Option<&str>,
    ) -> Self {
        use std::io::Read;
        let template = env.std_cmd();
        let quote = |word: &str| format!("'{}'", word.replace('\'', "'\\''"));
        let command_line: Vec<String> =
            std::iter::once(template.get_program().to_string_lossy().into_owned())
                .chain(args.iter().map(|arg| (*arg).to_owned()))
                .map(|word| quote(&word))
                .collect();
        let pipe = stdin_ids.map_or_else(String::new, |ids| {
            format!("printf '%s\\n' {} | ", quote(ids))
        });
        let command = command_line.join(" ");
        let script = match then {
            Some(then) => format!("stty rows {rows} cols {cols}; {pipe}{command}; {then}"),
            None => format!("stty rows {rows} cols {cols}; {pipe}exec {command}"),
        };
        let mut command = std::process::Command::new("script");
        command.args(["-q", "/dev/null", "/bin/sh", "-c", &script]);
        for (name, value) in template.get_envs() {
            match value {
                Some(value) => command.env(name, value),
                None => command.env_remove(name),
            };
        }
        command.env("TERM", "xterm-256color");
        let mut child = command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let shown = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut stdout = child.stdout.take().unwrap();
        let reader = {
            let shown = std::sync::Arc::clone(&shown);
            std::thread::spawn(move || {
                let mut buffer = [0u8; 8192];
                while let Ok(read) = stdout.read(&mut buffer) {
                    if read == 0 {
                        break;
                    }
                    shown.lock().unwrap().extend_from_slice(&buffer[..read]);
                }
            })
        };
        Self {
            stdin: child.stdin.take(),
            child,
            shown,
            reader: Some(reader),
            seen: 0,
            rows,
            cols,
        }
    }

    /// Type `keys`. A command that already ended reads nothing.
    pub fn send(&mut self, keys: &str) {
        use std::io::Write;
        let stdin = self.stdin.as_mut().unwrap();
        let _ = stdin
            .write_all(keys.as_bytes())
            .and_then(|()| stdin.flush());
    }

    /// The `script` process the binary runs under.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    /// Everything shown so far, `\r\n` as `\n`.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.shown.lock().unwrap()).replace("\r\n", "\n")
    }

    /// Wait until `wanted` shows in the stream after what earlier waits
    /// matched; returns the text from there up to and including it.
    pub fn wait_for(&mut self, wanted: &str) -> String {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let text = self.text();
            if let Some(at) = text.get(self.seen..).and_then(|rest| rest.find(wanted)) {
                let end = self.seen + at + wanted.len();
                let upto = text[self.seen..end].to_owned();
                self.seen = end;
                return upto;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "never showed {wanted:?}; showed:\n{}",
                text.get(self.seen..).unwrap_or_default()
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// The screen as a terminal of this size shows it now.
    pub fn screen(&self) -> String {
        let mut parser = vt100::Parser::new(self.rows, self.cols, 0);
        parser.process(&self.shown.lock().unwrap());
        parser.screen().contents()
    }

    /// Wait until the screen shows `wanted`; returns the screen.
    pub fn wait_for_screen(&self, wanted: &str) -> String {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let screen = self.screen();
            if screen.contains(wanted) {
                return screen;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the screen never showed {wanted:?}:\n{screen}"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// Wait for the command to exit; its status code.
    pub fn finish(self) -> Option<i32> {
        self.finish_keeping_text().0
    }

    /// Wait for the command to exit (generously: classify tests summarise
    /// 2 MB chats after the answer); its status code and everything it
    /// showed.
    pub fn finish_keeping_text(mut self) -> (Option<i32>, String) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(150);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                drop(self.stdin.take());
                if let Some(reader) = self.reader.take() {
                    reader.join().unwrap();
                }
                return (status.code(), self.text());
            }
            assert!(
                std::time::Instant::now() < deadline,
                "never exited; showed:\n{}",
                self.text()
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// Every chat the fake chatgpt.com has, and whether it's archived.
pub fn fake_chat_states(env: &Env) -> Vec<(String, bool)> {
    env.fake()
        .state()
        .chats
        .iter()
        .map(|chat| (chat.id.clone(), chat.archived))
        .collect()
}

/// `id`'s `update_time` in the daemon's index.
pub fn indexed_update_time(env: &Env, id: &str) -> String {
    env.index_db()
        .query_row(
            "select update_time from conversations where id = ?",
            [id],
            |row| row.get(0),
        )
        .unwrap()
}

/// A stand-in `name` on the tools `PATH` (`pbcopy`, `open`, a pager) that
/// appends its arguments, then its stdin, to `<home>/<name>.log`.
pub fn recording_tool(env: &Env, name: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let log = env.home.path().join(format!("{name}.log"));
    let script = env.tools.join(name);
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf 'args:%s\\n' \"$*\" >> '{log}'\ncat >> '{log}'\n",
            log = log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    log
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

/// This build's QUESTIONS_VERSION (`Profile::builtin`).
pub const QUESTIONS_VERSION: &str = "2026-09-28.9";

pub fn judge(db: &rusqlite::Connection, id: &str, update_time: &str, answers: &str) {
    db.execute(
        "insert or replace into judgments (id, update_time, version, content_kind, answers, classified_at)
         values (?, ?, ?, 'full', ?, '2026-09-28T00:00:00Z')",
        rusqlite::params![id, update_time, QUESTIONS_VERSION, answers],
    )
    .unwrap();
}

/// Write stand-in `tools` into `dir`: scripts that run the debug binary as
/// the fake `codex` or `claude`, recording calls in `log`.
pub fn fake_tools(dir: &Path, log: &Path, mode: &str, tools: &[&str]) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).unwrap();
    let binary = assert_cmd::cargo::cargo_bin!("chatgpt");
    for tool in tools {
        let script = dir.join(tool);
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nexec '{}' __fake-model-cli '{}' '{mode}' {tool} \"$@\"\n",
                binary.display(),
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// `dir`, then only the system's own directories: never a real `codex`
/// or `claude` (Homebrew's, `~/.local/bin`'s) behind a stand-in a test
/// took away.
pub fn tools_path(dir: &Path) -> String {
    format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", dir.display())
}

/// The calls recorded in `log`, oldest first.
pub fn model_calls(log: &Path) -> Vec<Value> {
    let Ok(entries) = std::fs::read_dir(log) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    files.sort_by_key(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.split('-').nth(1))
            .and_then(|stamp| stamp.parse::<u128>().ok())
            .unwrap_or(0)
    });
    files
        .iter()
        .map(|path| serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap())
        .collect()
}
