//! One write, both CLIs, the same fake chatgpt.com: `archive` runs in the
//! TypeScript CLI and in the Rust CLI, each against its own fake, and their
//! stdout, stderr, exit status and the writes they sent are compared.
//!
//! The TS CLI reads its own index (`$XDG_DATA_HOME/chatgpt-cli/index.db`),
//! which the Rust CLI's index is copied into, and reaches its fake through
//! a Bun plugin that points its HTTP client there. Durations and rates are
//! left out of the comparison; everything else on stderr is compared byte
//! for byte. Exit codes are compared as success or failure, because the
//! Rust CLI's invalid-input errors exit 2 where the TS CLI exits 1.
//!
//! The two CLIs are also held to the expected end state — the chat archived
//! in ChatGPT and in each index, and one write each — because two CLIs that
//! agree on doing nothing would pass a comparison alone.
//!
//! Needs bun and the TypeScript CLI's checkout (`CHATGPT_TS_REPO`); skips
//! without them. This is the first rung of the write path; the rest of it
//! (unarchive, delete, rename, title, projects, memories) is not here yet.

#![allow(clippy::unwrap_used)]

mod support;

use std::path::PathBuf;
use std::process::Command;

use fake_chatgpt::{COOKIE, Chat, FakeChatGpt};
use support::{
    Env, bun_for, copy_chats, copy_judgments, delete_answers, judge, normalize, ts_index, ts_repo,
    writes,
};

/// Points the TS CLI's HTTP client at its fake chatgpt.com, with its cookie.
/// Verbatim from the v0.1.5 harness: its guard throws rather than let the
/// TS CLI talk to the real chatgpt.com if `src/api/client.ts` changed.
const PARITY_PLUGIN: &str = r#"
import { plugin } from "bun";
const base = process.env.PARITY_FAKE_BASE;
const cookie = process.env.PARITY_FAKE_COOKIE;
const eq = cookie?.indexOf("=") ?? -1;
if (!base || !cookie || eq < 1) throw new Error("PARITY_FAKE_BASE and PARITY_FAKE_COOKIE (name=value) must be set");
plugin({
	name: "parity",
	setup(build) {
		build.onLoad({ filter: /\/src\/api\/client\.ts$/ }, async ({ path }) => {
			const source = await Bun.file(path).text();
			const session = `({ browser: "fake", profile: undefined, cookies: [{ name: ${JSON.stringify(cookie.slice(0, eq))}, value: ${JSON.stringify(cookie.slice(eq + 1))} }] })`;
			const patched = source
				.replace('const BASE = "https://chatgpt.com";', `const BASE = ${JSON.stringify(base)};`)
				.replace("readBrowserSession(this.browserSelection())", session);
			if (!patched.includes(session) || patched.includes("https://chatgpt.com")) {
				throw new Error("src/api/client.ts changed: update the parity plugin");
			}
			return { contents: patched, loader: "ts" };
		});
	},
});
"#;

fn chats() -> Vec<Chat> {
    let mut chats = vec![
        Chat::new("a-new", "New idea", "2026-09-27T10:00:00.000000Z"),
        Chat::new("b-old", "Old taxes", "2024-01-01T10:00:00.000000Z"),
        Chat::new("c-mid", "Mid trip", "2025-06-01T10:00:00.000000Z"),
    ];
    let mut archived = Chat::new("z-archived", "Archived chat", "2024-01-03T10:00:00.000000Z");
    archived.archived = true;
    chats.push(archived);
    chats
}

/// The TS CLI, run against its own fake chatgpt.com.
struct Ts {
    bun: PathBuf,
    repo: PathBuf,
    /// Where the TS CLI's own home, config and cache live.
    work: PathBuf,
    data_home: PathBuf,
    home: PathBuf,
    plugin: PathBuf,
    fake_url: String,
}

impl Ts {
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.bun);
        command
            .arg("--preload")
            .arg(&self.plugin)
            .arg(self.repo.join("src/cli.ts"))
            .args(args)
            .env("XDG_DATA_HOME", &self.data_home)
            .env("XDG_CONFIG_HOME", self.work.join("config"))
            .env("XDG_CACHE_HOME", self.work.join("cache"))
            .env("HOME", &self.home)
            .env("PARITY_FAKE_BASE", &self.fake_url)
            .env("PARITY_FAKE_COOKIE", COOKIE)
            .env_remove("CHATGPT_BROWSER")
            .env_remove("CHATGPT_BROWSER_PROFILE")
            .current_dir(&self.repo);
        command
    }

    /// The exit code, stdout and stderr of one run.
    fn run(&self, args: &[&str]) -> (Option<i32>, String, String) {
        let output = self.command(args).output().unwrap();
        (
            output.status.code(),
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap(),
        )
    }
}

/// The exit code, stdout and stderr of one Rust CLI run.
fn rust(env: &Env, args: &[&str]) -> (Option<i32>, String, String) {
    let output = env.cmd().args(args).output().unwrap();
    (
        output.status.code(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

/// Every chat the fake has, and whether it's archived.
fn chat_states(fake: &FakeChatGpt) -> Vec<(String, bool)> {
    let mut chats: Vec<_> = fake
        .state()
        .chats
        .iter()
        .map(|chat| (chat.id.clone(), chat.archived))
        .collect();
    chats.sort();
    chats
}

/// Every chat an index has, and whether it says the chat is archived.
fn index_archived(db: &rusqlite::Connection) -> Vec<(String, bool)> {
    let mut rows = db
        .prepare("select id, is_archived from conversations order by id")
        .unwrap();
    rows.query_map([], |row| Ok((row.get(0)?, row.get::<_, i64>(1)? != 0)))
        .unwrap()
        .map(|row| row.unwrap())
        .collect()
}

#[test]
fn archiving_a_chat_prints_and_writes_as_the_ts_cli_does() {
    let Some(repo) = ts_repo() else {
        eprintln!(
            "skipping the TS parity harness: set CHATGPT_TS_REPO to the TypeScript CLI's checkout"
        );
        return;
    };
    // The TS CLI's own home, so it reads neither the developer's config nor
    // the developer's cookies.
    let env = Env::with_fake(chats());
    let work = env.home.path().join("parity");
    let ts_home = work.join("home");
    std::fs::create_dir_all(&ts_home).unwrap();
    let Some(bun) = bun_for(&ts_home) else {
        eprintln!("skipping the TS parity harness: no bun on PATH runs with a synthetic HOME");
        return;
    };
    // The TS CLI's own fake chatgpt.com, seeded alike.
    let ts_fake = FakeChatGpt::start(chats());

    // The Rust CLI's index, synced from its own fake.
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    let rust_index = env.data_dir().join("chatgpt.db");

    // The TS CLI's index, with the same chats and the same `synced_at`, and
    // the same verdicts, so both CLIs read the same data.
    let ts_data_home = work.join("data");
    let ts_db_path = ts_index(&ts_data_home);
    assert_eq!(copy_chats(&rust_index, &ts_db_path), chats().len());
    {
        let db = rusqlite::Connection::open(&ts_db_path).unwrap();
        judge(
            &db,
            "b-old",
            "2024-01-01T10:00:00.000000Z",
            &delete_answers("other"),
        );
    }
    assert_eq!(copy_judgments(&ts_db_path, &rust_index), 1);

    let plugin = work.join("plugin.ts");
    std::fs::write(&plugin, PARITY_PLUGIN).unwrap();
    // The plugin only patches `src/api/client.ts`. If the TS CLI's HTTP
    // client moved, the plugin would not fire at all, and the TS CLI would
    // read the developer's cookies and reach the real chatgpt.com: check
    // the file it patches is still the one it patches.
    let client = std::fs::read_to_string(repo.join("src/api/client.ts")).unwrap();
    assert!(
        client.contains("const BASE = \"https://chatgpt.com\";"),
        "src/api/client.ts changed: update the parity plugin"
    );
    let ts = Ts {
        bun,
        repo,
        work,
        data_home: ts_data_home,
        home: ts_home,
        plugin,
        fake_url: ts_fake.url.clone(),
    };

    // The write: one chat, archived in ChatGPT and in each index.
    let args = ["archive", "b-old", "-y"];
    let rust_seen = writes(env.fake()).len();
    let ts_seen = writes(&ts_fake).len();
    let (ts_code, ts_out, ts_err) = ts.run(&args);
    let (rust_code, rust_out, rust_err) = rust(&env, &args);
    env.wait_for_indexer();

    eprintln!("--- TS CLI (bun, {})", ts.repo.display());
    eprintln!("exit: {ts_code:?}");
    eprintln!("stdout: {ts_out:?}");
    eprintln!("stderr:\n{ts_err}");
    eprintln!(
        "stderr without durations and rates:\n{}",
        normalize(&ts_err)
    );
    eprintln!("--- Rust CLI");
    eprintln!("exit: {rust_code:?}");
    eprintln!("stdout: {rust_out:?}");
    eprintln!("stderr:\n{rust_err}");
    eprintln!(
        "stderr without durations and rates:\n{}",
        normalize(&rust_err)
    );
    eprintln!("--- writes the TS fake saw");
    for call in writes(&ts_fake).iter().skip(ts_seen) {
        eprintln!("    {call}");
    }
    eprintln!("--- writes the Rust fake saw");
    for call in writes(env.fake()).iter().skip(rust_seen) {
        eprintln!("    {call}");
    }
    eprintln!("--- every request the TS fake saw");
    for call in ts_fake.calls() {
        eprintln!("    {call}");
    }

    assert_eq!(
        rust_code == Some(0),
        ts_code == Some(0),
        "{args:?} exit: TS {ts_code:?}, Rust {rust_code:?}\nTS stderr: {ts_err}\nRust stderr: {rust_err}"
    );
    assert_eq!(
        rust_out, ts_out,
        "{args:?} stdout\nTS: {ts_out:?}\nRust: {rust_out:?}"
    );
    assert_eq!(
        normalize(&rust_err),
        normalize(&ts_err),
        "{args:?} stderr\nTS raw: {ts_err}\nRust raw: {rust_err}"
    );
    let rust_writes_all = writes(env.fake());
    let ts_writes_all = writes(&ts_fake);
    let rust_writes: Vec<&str> = rust_writes_all
        .iter()
        .skip(rust_seen)
        .map(String::as_str)
        .collect();
    let ts_writes: Vec<&str> = ts_writes_all
        .iter()
        .skip(ts_seen)
        .map(String::as_str)
        .collect();
    assert_eq!(rust_writes, ts_writes, "{args:?} requests");
    // ChatGPT and both indexes end up alike.
    let rust_fake_states = chat_states(env.fake());
    let ts_fake_states = chat_states(&ts_fake);
    let ts_db = rusqlite::Connection::open(&ts_db_path).unwrap();
    let rust_index_states = index_archived(&env.index_db());
    let ts_index_states = index_archived(&ts_db);
    assert_eq!(rust_fake_states, ts_fake_states);
    assert_eq!(rust_index_states, ts_index_states);

    // The end state, not only that the two CLIs agree: agreeing is not
    // enough, because both could have done nothing at all. The chat is
    // archived in ChatGPT and in each index.
    let archived = ("b-old".to_owned(), true);
    assert!(
        rust_fake_states.contains(&archived),
        "the Rust fake did not archive b-old: {rust_fake_states:?}"
    );
    assert!(
        ts_fake_states.contains(&archived),
        "the TS fake did not archive b-old: {ts_fake_states:?}"
    );
    assert!(
        rust_index_states.contains(&archived),
        "the Rust index did not archive b-old: {rust_index_states:?}"
    );
    assert!(
        ts_index_states.contains(&archived),
        "the TS index did not archive b-old: {ts_index_states:?}"
    );
    // And each CLI sent that one write, and no other.
    assert_eq!(
        rust_writes.len(),
        1,
        "the Rust CLI sent {rust_writes:?} writes, not one"
    );
    assert_eq!(
        ts_writes.len(),
        1,
        "the TS CLI sent {ts_writes:?} writes, not one"
    );
    assert_eq!(
        rust_writes,
        ["PATCH /backend-api/conversation/b-old"],
        "the Rust CLI's write"
    );
    assert_eq!(
        ts_writes,
        ["PATCH /backend-api/conversation/b-old"],
        "the TS CLI's write"
    );
}
