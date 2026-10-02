//! The parity harness for changing chats, projects and memories: the TS CLI
//! and the Rust CLI run the same commands, in the same order, against two
//! fake chatgpt.coms that start alike (and two fake TypeSafes), and must
//! print the same previews, prompts, notes and summaries, send the same
//! writes and Jev requests, and leave ChatGPT and their indexes alike.
//!
//! The Rust daemon syncs its fake and imports a TS index (a long chat's
//! summary), and gets the TS index's judgments copied in; its chats are
//! then copied into that TS index, which the
//! TS CLI (`bun src/cli.ts`, with a `--preload` plugin pointing its HTTP
//! client at its fake) reads and writes. Durations and rates are left out
//! of the comparison; everything else on stderr is compared byte for byte.
//! Exit codes are compared as success or failure: the Rust CLI's
//! invalid-input errors exit 2 where the TS CLI exits 1.
//!
//! Needs bun and `bun install` in the repository; skips without them.

#![allow(clippy::unwrap_used)]

mod support;

use std::path::PathBuf;
use std::process::Command;

use fake_chatgpt::typesafe::{API_KEY, FakeTypeSafe};
use fake_chatgpt::{COOKIE, Chat, FakeChatGpt, Project};
use rusqlite::params;
use serde_json::{Value, json};
use support::{
    Env, QUESTIONS_VERSION, bun, copy_chats, copy_judgments, delete_answers, in_terminal, repo,
};

/// Points the TS CLI's HTTP client at its fake chatgpt.com, with its cookie.
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

const OLD_DAY: &str = "2024-02";

fn chats() -> Vec<Chat> {
    let mut chats = Vec::new();
    // Enough old chats for a preview's "… and N more".
    for n in 0..32 {
        let mut chat = Chat::new(
            &format!("n-{n:02}"),
            &format!("Old note {n}"),
            &format!("{OLD_DAY}-{:02}T10:{n:02}:00.000000Z", 1 + n % 28),
        );
        chat.pinned = n % 7 == 3;
        if n % 5 == 0 {
            chat.gizmo_id = Some("g-p-writing".into());
        }
        chats.push(chat);
    }
    for (id, title, minute) in [
        ("j-junk", "Junk ping", 1),
        ("j-maybe", "Maybe receipt", 2),
        ("j-idea", "Book idea", 3),
        ("j-stale", "Stale trip", 4),
    ] {
        chats.push(Chat::new(
            id,
            title,
            &format!("2023-05-01T10:0{minute}:00.000000Z"),
        ));
    }
    let mut long = Chat::new("j-long", "Long junk essay", "2023-05-01T09:00:00.000000Z");
    long.text = "word ".repeat(12_000);
    chats.push(long);
    chats.push(Chat::new(
        "r-legacy",
        "Legacy chat",
        "2022-01-01T10:00:00.000000Z",
    ));
    for n in 0..2 {
        let mut archived = Chat::new(
            &format!("z-arch{n}"),
            &format!("Archived {n}"),
            &format!("2025-03-0{}T10:00:00.000000Z", n + 1),
        );
        archived.archived = true;
        chats.push(archived);
    }
    chats.push(Chat::new(
        "f-fresh",
        "Fresh chat",
        "2026-09-30T10:00:00.000000Z",
    ));
    chats
}

fn set_up(fake: &FakeChatGpt) {
    let mut state = fake.state();
    state.chats = chats();
    let mut shared = Project::new("g-p-shared", "Shared");
    shared.can_write = false;
    state.projects = vec![
        Project::new("g-p-writing", "Writing"),
        Project::new("g-p-ideas", "Ideas"),
        shared,
    ];
    state.legacy_rename.insert("r-legacy".into());
    state.project_500.insert("n-01".into());
    state.unconfirmed_memories.insert("mem-ccc".into());
    state.memories = Some(vec![
        json!({ "id": "mem-aaa", "content": "Prefers tea,\n not \"coffee\"", "updated_at": "2026-09-01T00:00:00Z",
                "status": "active", "conversation_id": null, "gizmo_id": null, "created_timestamp": 1.5,
                "last_updated": null, "labels": null }),
        json!({ "id": "mem-bbb", "content": "y ".repeat(80), "updated_at": "2026-09-02T00:00:00Z",
                "status": "active", "conversation_id": "n-01" }),
        json!({ "id": "mem-ccc", "content": "Unconfirmable", "updated_at": "2026-09-03T00:00:00Z",
                "status": "active", "conversation_id": null }),
    ]);
    state.memory_summary = json!({
        "sections": [{ "id": "1", "title": "Work", "description": "Builds CLIs" },
                     { "id": "2", "title": "Home", "description": "Two kids" }],
        "generatedAtIso": "2026-09-28T00:00:00.000Z", "emptyStateMessage": null, "sourceChecksum": "c",
    });
}

/// Judgments, and the long chat's summary, in the TS index both CLIs read.
fn fill_ts_index(db: &rusqlite::Connection) {
    let unsure = {
        let mut answers: Value = serde_json::from_str(&delete_answers("other")).unwrap();
        answers["personal_record"]["noul"] = json!(0.45);
        answers["nothing_there"]["noul"] = json!(0.5);
        answers["re_askable"]["noul"] = json!(0.75);
        answers.to_string()
    };
    for chat in chats().iter().filter(|chat| chat.id.starts_with("n-")) {
        let n: usize = chat.id[2..].parse().unwrap();
        let answers = match n % 3 {
            0 => delete_answers("other"),
            1 => unsure.clone(),
            _ => continue,
        };
        db.execute(
            "insert into judgments (id, update_time, version, content_kind, answers, classified_at)
             values (?, ?, ?, 'full', ?, '2026-09-30T00:00:00.000Z')",
            params![chat.id, chat.update_time, QUESTIONS_VERSION, answers],
        )
        .unwrap();
    }
    db.execute(
        "insert into summaries values ('j-long', '2023-05-01T09:00:00.000000Z', 9, 'The user asked a junk question.', 'm')",
        [],
    )
    .unwrap();
}

struct Ts {
    bun: PathBuf,
    data_home: PathBuf,
    home: PathBuf,
    config_home: PathBuf,
    plugin: PathBuf,
    fake_url: String,
    typesafe_url: String,
}

impl Ts {
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.bun);
        command
            .arg("--preload")
            .arg(&self.plugin)
            .arg(repo().join("src/cli.ts"))
            .args(args)
            .env("XDG_DATA_HOME", &self.data_home)
            .env("XDG_CONFIG_HOME", &self.config_home)
            .env("HOME", &self.home)
            .env("PARITY_FAKE_BASE", &self.fake_url)
            .env("PARITY_FAKE_COOKIE", COOKIE)
            .env("TYPESAFE_API_KEY", API_KEY)
            .env("TYPESAFE_BASE_URL", &self.typesafe_url)
            .env_remove("CHATGPT_BROWSER")
            .env_remove("CHATGPT_BROWSER_PROFILE")
            .current_dir(repo());
        command
    }

    fn run(&self, args: &[&str]) -> (bool, String, String) {
        let output = self.command(args).output().unwrap();
        (
            output.status.success(),
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap(),
        )
    }
}

fn rust(env: &Env, args: &[&str]) -> (bool, String, String) {
    let output = env.cmd().args(args).output().unwrap();
    (
        output.status.success(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

/// `(0.3s)`, `(1m02s)`: a duration in brackets.
fn is_duration(text: &str) -> bool {
    !text.is_empty()
        && text.ends_with(['s', 'm'])
        && text
            .chars()
            .all(|c| c.is_ascii_digit() || ".hms".contains(c))
}

/// stderr without what varies run to run: a running count keeps its label,
/// its count and its cost or failures; a summary loses its duration.
fn normalize(stderr: &str) -> String {
    let mut out = String::new();
    for line in stderr.split('\n') {
        let line = line.trim_start_matches("\r\u{1b}[2K");
        if line.contains(['█', '░']) {
            let label = line.split("  ").next().unwrap_or("");
            let count = line
                .split_whitespace()
                .find(|word| {
                    word.contains('/')
                        && word.chars().next().is_some_and(|c| c.is_ascii_digit())
                        && !word.ends_with("/s")
                })
                .unwrap_or("");
            let detail = line
                .rsplit(" · ")
                .next()
                .filter(|last| last.starts_with('$') || last.ends_with("failed"))
                .unwrap_or("");
            out.push_str(&format!("{label}  {count} {detail}\n"));
            continue;
        }
        let line = match line.rfind(" (") {
            Some(at) if line.ends_with(')') && is_duration(&line[at + 2..line.len() - 1]) => {
                &line[..at]
            }
            _ => line,
        };
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The requests that change or read projects and memories, and the
/// single-chat reads a project move's check makes, in order.
fn writes(fake: &FakeChatGpt) -> Vec<String> {
    fake.calls()
        .into_iter()
        .filter(|call| {
            call.starts_with("PATCH")
                || call.starts_with("DELETE")
                || call.contains("/rename")
                || call.contains("/projects")
                || call.contains("/sidebar")
                || call.contains("/memories")
                || call.starts_with("GET /backend-api/conversation/")
        })
        .collect()
}

fn chat_states(fake: &FakeChatGpt) -> Vec<(String, String, bool, Option<String>)> {
    let mut chats: Vec<_> = fake
        .state()
        .chats
        .iter()
        .map(|chat| {
            (
                chat.id.clone(),
                chat.title.clone(),
                chat.archived,
                chat.gizmo_id.clone(),
            )
        })
        .collect();
    chats.sort();
    chats
}

#[test]
fn changes_print_and_apply_as_the_ts_clis_do() {
    let Some(bun) = bun() else {
        eprintln!("skipping the TS parity harness: bun isn't on PATH");
        return;
    };
    if !repo().join("node_modules/@typesafe-ai/sdk").is_dir() {
        eprintln!("skipping the TS parity harness: run `bun install` in the repository first");
        return;
    }
    let mut env = Env::with_fake(Vec::new());
    set_up(env.fake());
    let ts_fake = FakeChatGpt::start(Vec::new());
    set_up(&ts_fake);
    let rust_typesafe = FakeTypeSafe::start();
    let ts_typesafe = FakeTypeSafe::start();
    env.extra_env.extend([
        ("TYPESAFE_API_KEY".to_owned(), API_KEY.to_owned()),
        ("TYPESAFE_BASE_URL".to_owned(), rust_typesafe.url.clone()),
    ]);
    let ts_db = env.legacy_db();
    fill_ts_index(&ts_db);
    drop(ts_db);
    // The background indexer's batch reads fail, so the guard downloads
    // the transcripts it judges, as the TS CLI (with none cached) does.
    let batches = chats().len().div_ceil(10);
    env.fake().state().fail_batch = u32::try_from(batches).unwrap();
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    assert_eq!(
        env.fake().state().fail_batch,
        0,
        "the indexer tried every chat"
    );
    assert!(copy_judgments(&env.legacy, &env.data_dir().join("chatgpt.db")) > 0);
    assert_eq!(
        copy_chats(&env.data_dir().join("chatgpt.db"), &env.legacy),
        chats().len()
    );

    let work = env.home.path().join("parity");
    std::fs::create_dir_all(work.join("config")).unwrap();
    std::fs::write(work.join("plugin.ts"), PARITY_PLUGIN).unwrap();
    let ts = Ts {
        bun,
        data_home: env.legacy.parent().unwrap().parent().unwrap().to_path_buf(),
        home: work.join("home"),
        config_home: work.join("config"),
        plugin: work.join("plugin.ts"),
        fake_url: ts_fake.url.clone(),
        typesafe_url: ts_typesafe.url.clone(),
    };
    std::fs::create_dir_all(&ts.home).unwrap();

    let scenarios: &[&[&str]] = &[
        // Previews and dry runs.
        &["archive", "--older-than", "1y", "-n"],
        &["archive", "--older-than", "1y", "--pinned", "--dry-run"],
        &["delete", "--title", "^old note 1", "--limit", "3", "-n"],
        &["unarchive", "--older-than", "1y", "-n"],
        &["unarchive", "--all", "--title", "archived|fresh", "-n"],
        &["archive", "--brainstorm", "-n"],
        // Selection errors.
        &["delete", "-y"],
        &["archive", "nope", "-y"],
        &["archive", "z-arch0", "-y"],
        &["unarchive", "f-fresh", "-y"],
        &["archive", "n-0", "-y"],
        &["archive", "--older-than", "3x", "-n"],
        &["archive", "--suggest", "burn", "-n"],
        &["unarchive", "z-arch0", "--check", "-n"],
        // The Jev guard: current judgments, then fresh ones (downloads,
        // a long chat judged from its summary).
        &["delete", "--suggest", "delete", "-n"],
        &[
            "archive",
            "--suggest",
            "archive",
            "--older-than",
            "1y",
            "-n",
        ],
        &[
            "delete", "j-junk", "j-maybe", "j-idea", "j-long", "--check", "-n",
        ],
        &[
            "archive", "j-junk", "j-maybe", "j-idea", "j-stale", "j-long", "--check", "-n",
        ],
        // Changes.
        &["archive", "n-02", "n-04", "-y"],
        &["unarchive", "n-02", "-y"],
        &["delete", "n-05", "n-08", "n-11", "-y"],
        &[
            "delete",
            "--suggest",
            "delete",
            "--older-than",
            "1y",
            "--limit",
            "2",
            "-y",
        ],
        &["rename", "n-10", "Renamed note"],
        &["rename", "r-legacy", "Still legacy"],
        &["title", "n-13", "  A   local title  "],
        &["title", "n-13", " "],
        // Projects.
        &["project", "list"],
        &["project", "list", "--json", "--limit", "2"],
        &["project", "create", "  chatgpt-cli test (delete me) "],
        &["project", "create", "WRITING"],
        &[
            "project", "add", "Ideas", "n-14", "n-15", "n-15", "n-01", "-n",
        ],
        &["project", "add", "ideas", "n-14", "n-01", "-y"],
        &["project", "add", "Ideas", "n-14", "-y"],
        &["project", "remove", "g-p-ideas", "n-14", "n-16", "-y"],
        &["project", "remove", "Ideas", "n-16", "-y"],
        &["project", "add", "Shared", "n-16", "-y"],
        &["project", "add", "g-p", "n-16", "-y"],
        // Memories.
        &["memory", "list"],
        &["memory", "list", "--format", "csv", "--search", "TEA"],
        &["memory", "list", "--format", "json", "--limit", "2"],
        &["memory", "list", "--format", "ids"],
        &["memory", "summary"],
        &["memory", "summary", "--format", "json"],
        &["memory", "delete", "mem-a", "MEM-AAA", "-n"],
        &["memory", "delete", "mem", "-n"],
        &["memory", "delete", "mem-aaa", "mem-ccc", "-y"],
    ];
    let mut compared = 0;
    // Each command's requests, sorted: three deletes run at once.
    let since = |fake: &FakeChatGpt, seen: usize| {
        let mut new = writes(fake).split_off(seen);
        new.sort();
        new
    };
    for args in scenarios {
        let (rust_seen, ts_seen) = (writes(env.fake()).len(), writes(&ts_fake).len());
        let (ts_ok, ts_out, ts_err) = ts.run(args);
        let (rust_ok, rust_out, rust_err) = rust(&env, args);
        assert_eq!(
            rust_ok, ts_ok,
            "{args:?} exit\nTS: {ts_err}\nRust: {rust_err}"
        );
        assert_eq!(rust_out, ts_out, "{args:?} stdout");
        assert_eq!(
            normalize(&rust_err),
            normalize(&ts_err),
            "{args:?} stderr\nTS raw: {ts_err}\nRust raw: {rust_err}"
        );
        assert_eq!(
            since(env.fake(), rust_seen),
            since(&ts_fake, ts_seen),
            "{args:?} requests"
        );
        compared += 1;
    }

    // The same Jev requests, byte for byte (questions included), in
    // whatever order they finished.
    let bodies = |fake: &FakeTypeSafe| {
        let mut bodies = fake.state().bodies.clone();
        bodies.sort();
        bodies
    };
    assert!(!bodies(&rust_typesafe).is_empty());
    assert_eq!(bodies(&rust_typesafe), bodies(&ts_typesafe));

    // ChatGPT and both indexes end up alike.
    assert_eq!(chat_states(env.fake()), chat_states(&ts_fake));
    let ts_projects: Vec<String> = ts_fake
        .state()
        .projects
        .iter()
        .map(|p| p.name.clone())
        .collect();
    let rust_projects: Vec<String> = env
        .fake()
        .state()
        .projects
        .iter()
        .map(|p| p.name.clone())
        .collect();
    assert_eq!(rust_projects, ts_projects);
    assert_eq!(env.fake().state().memories, ts_fake.state().memories);
    let (_, ts_list, _) = ts.run(&["list", "--json", "--all"]);
    let (_, rust_list, _) = rust(&env, &["list", "--json", "--all"]);
    assert_eq!(rust_list, ts_list, "list --json --all after the changes");

    // A prompt, at a terminal, answered no.
    let (_, rust_shown) = in_terminal(&env, &["archive", "n-20"], None, "[y/N] ", "n");
    let ts_shown = ts_in_terminal(&ts, &["archive", "n-20"], "[y/N] ", "n");
    assert!(
        plain(&rust_shown).ends_with("archive 1 conversation(s)? [y/N] n\nCancelled.\n"),
        "{rust_shown:?}"
    );
    assert_eq!(plain(&rust_shown), plain(&ts_shown));
    eprintln!(
        "TS parity: {compared} change commands matched, {} Jev requests identical",
        bodies(&rust_typesafe).len()
    );
}

/// The TS CLI in a pseudo-terminal, as [`in_terminal`] runs the Rust one.
fn ts_in_terminal(ts: &Ts, args: &[&str], prompt: &str, answer: &str) -> String {
    use std::io::{Read, Write};
    let template = ts.command(args);
    let mut command = Command::new("script");
    command
        .arg("-q")
        .arg("/dev/null")
        .arg(template.get_program())
        .args(template.get_args())
        .current_dir(repo());
    for (name, value) in template.get_envs() {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let shown = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let reader = {
        let shown = std::sync::Arc::clone(&shown);
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while let Ok(read) = stdout.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                shown.lock().unwrap().extend_from_slice(&buffer[..read]);
            }
        })
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !String::from_utf8_lossy(&shown.lock().unwrap()).contains(prompt) {
        assert!(
            std::time::Instant::now() < deadline,
            "the TS CLI never prompted"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(format!("{answer}\n").as_bytes()).unwrap();
    child.wait().unwrap();
    drop(stdin);
    reader.join().unwrap();
    String::from_utf8_lossy(&shown.lock().unwrap()).into_owned()
}

/// A terminal transcript as text: no carriage returns or escape sequences.
fn plain(shown: &str) -> String {
    let mut out = String::new();
    let mut chars = shown.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {}
            '\u{1b}' => {
                // CSI: ESC [ parameters final-byte.
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for next in chars.by_ref() {
                        if next.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out
}
