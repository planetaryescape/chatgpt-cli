//! The parity harness for classification: the TS CLI and the Rust CLI run
//! the same `configure`, `classify`, `titles`, `memory classify` and Jev
//! guard commands, in the same order, against twin fakes that start alike
//! (chatgpt.com, TypeSafe, OpenAI, Anthropic, and stand-in `codex` and
//! `claude` on `PATH`), and must print the same notes, steps, cost lines,
//! prompts, failures and data, send the same model requests (Jev bodies,
//! OpenAI and Anthropic bodies, and the subscription CLIs' arguments and
//! stdin, byte for byte), and end with the same verdicts, titles and
//! memory classifications.
//!
//! The TS CLI (`bun src/cli.ts`) reads its own index, a copy of the Rust
//! daemon's chats, through a `--preload` plugin that points its HTTP
//! client and model APIs at its fakes. Durations and rates are left out of
//! the comparison; so are the config file's path, which each side has its
//! own of. Exit codes are compared as success or failure.
//!
//! Needs bun and `bun install` in the repository; skips without them.

#![allow(clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use fake_chatgpt::models::{ANTHROPIC_KEY, FakeModelApi, OPENAI_KEY};
use fake_chatgpt::typesafe::{API_KEY, FakeTypeSafe};
use fake_chatgpt::{COOKIE, Chat, FakeChatGpt};
use serde_json::{Value, json};
use support::{Env, bun, copy_chats, fake_tools, model_calls, repo, tools_path};

/// Points the TS CLI's HTTP client at its fake chatgpt.com, with its
/// cookie, and its model APIs at its fake OpenAI and Anthropic.
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
		build.onLoad({ filter: /\/src\/classify\/model-api\.ts$/ }, async ({ path }) => {
			const source = await Bun.file(path).text();
			const patched = source
				.replace('"https://api.openai.com/v1/responses"', JSON.stringify(`${process.env.PARITY_OPENAI_BASE}/v1/responses`))
				.replace('"https://api.anthropic.com/v1/messages"', JSON.stringify(`${process.env.PARITY_ANTHROPIC_BASE}/v1/messages`));
			if (patched.includes("https://api.")) throw new Error("src/classify/model-api.ts changed: update the parity plugin");
			return { contents: patched, loader: "ts" };
		});
	},
});
"#;

fn chats() -> Vec<Chat> {
    let mut long = Chat::new("e-long", "Long essay", "2024-03-01T05:00:00.000000Z");
    long.text = "word ".repeat(12_000);
    let mut long_guard = Chat::new("g-long", "Long junk diary", "2024-03-01T01:00:00.000000Z");
    long_guard.text = "line ".repeat(13_000);
    let mut pinned = Chat::new("f-pinned", "Pinned junk", "2024-03-01T04:00:00.000000Z");
    pinned.pinned = true;
    let mut project = Chat::new(
        "d-product",
        "My product plan",
        "2024-03-01T07:00:00.000000Z",
    );
    project.gizmo_id = Some("g-p-1".into());
    let mut huge = Chat::new("h-huge", "Huge log", "2024-03-01T03:00:00.000000Z");
    huge.text = "word ".repeat(400_100);
    huge.archived = true;
    vec![
        Chat::new("a-junk", "Junk ping", "2024-03-01T10:00:00.000000Z"),
        Chat::new("b-maybe", "Maybe receipt", "2024-03-01T09:00:00.000000Z"),
        Chat::new("c-idea", "Book idea", "2024-03-01T08:00:00.000000Z"),
        project,
        long,
        pinned,
        Chat::new(
            "i-notitle",
            "A notitle junk chat",
            "2024-03-01T02:00:00.000000Z",
        ),
        long_guard,
        huge,
    ]
}

fn memories() -> Value {
    json!([
        { "id": "mem-tea", "content": "Prefers green tea, \"not\" coffee", "updated_at": "2026-09-01T00:00:00Z", "status": "active", "conversation_id": null, "labels": null },
        { "id": "mem-old", "content": "Is planning last year's trip to Rome", "updated_at": "2026-09-02T00:00:00Z", "status": "active", "conversation_id": "c-idea" },
        { "id": "mem-move", "content": "Will move house soon", "updated_at": "2026-09-03T00:00:00Z", "status": "active", "conversation_id": null },
        { "id": "mem-trip", "content": "Is planning a trip to Rome", "updated_at": "2026-09-04T00:00:00Z", "status": "active", "conversation_id": null },
    ])
}

fn set_up(fake: &FakeChatGpt) {
    let mut state = fake.state();
    state.chats = chats();
    state.memories = Some(memories().as_array().unwrap().clone());
}

/// One side's model fakes.
struct Models {
    typesafe: FakeTypeSafe,
    openai: FakeModelApi,
    anthropic: FakeModelApi,
    tools: PathBuf,
    log: PathBuf,
}

impl Models {
    fn start(dir: &Path) -> Self {
        let tools = dir.join("tools");
        let log = dir.join("model-calls");
        fake_tools(&tools, &log, "ok", &["codex", "claude"]);
        Self {
            typesafe: FakeTypeSafe::start(),
            openai: FakeModelApi::openai(),
            anthropic: FakeModelApi::anthropic(),
            tools,
            log,
        }
    }

    /// The subscription CLIs' calls, as comparable text, sorted (they run
    /// concurrently).
    fn tool_calls(&self) -> Vec<String> {
        let mut calls: Vec<String> = model_calls(&self.log)
            .into_iter()
            .map(|call| {
                json!({ "tool": call["tool"], "args": call["args"], "stdin": call["stdin"] })
                    .to_string()
            })
            .collect();
        calls.sort();
        calls
    }
}

fn sorted(bodies: &[String]) -> Vec<String> {
    let mut bodies = bodies.to_vec();
    bodies.sort();
    bodies
}

struct Ts {
    bun: PathBuf,
    data_home: PathBuf,
    home: PathBuf,
    config_home: PathBuf,
    plugin: PathBuf,
    fake_url: String,
    models: Models,
    api_keys: bool,
}

impl Ts {
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.bun);
        command
            .arg("--preload")
            .arg(&self.plugin)
            .arg(repo().join("src/cli.ts"))
            .args(args)
            .env_clear()
            .env("PATH", tools_path(&self.models.tools))
            .env("XDG_DATA_HOME", &self.data_home)
            .env("XDG_CONFIG_HOME", &self.config_home)
            .env("HOME", &self.home)
            .env("PARITY_FAKE_BASE", &self.fake_url)
            .env("PARITY_FAKE_COOKIE", COOKIE)
            .env("PARITY_OPENAI_BASE", &self.models.openai.url)
            .env("PARITY_ANTHROPIC_BASE", &self.models.anthropic.url)
            .env("TYPESAFE_API_KEY", API_KEY)
            .env("TYPESAFE_BASE_URL", &self.models.typesafe.url)
            .current_dir(repo());
        if self.api_keys {
            command
                .env("OPENAI_API_KEY", OPENAI_KEY)
                .env("ANTHROPIC_API_KEY", ANTHROPIC_KEY);
        }
        command
    }

    fn run(&self, args: &[&str], stdin: &str) -> (bool, String, String) {
        use std::io::Write;
        let mut child = self
            .command(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
        let output = child.wait_with_output().unwrap();
        (
            output.status.success(),
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap(),
        )
    }
}

fn rust(env: &Env, args: &[&str], stdin: &str, api_keys: bool) -> (bool, String, String) {
    let mut command = env.cmd();
    if api_keys {
        command
            .env("OPENAI_API_KEY", OPENAI_KEY)
            .env("ANTHROPIC_API_KEY", ANTHROPIC_KEY);
    }
    let output = command.args(args).write_stdin(stdin).output().unwrap();
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

/// stderr without what varies run to run: a running count keeps its label
/// and count; a summary loses its duration; `Done in …` its duration; the
/// config path its directory.
fn normalize(stderr: &str, config_home: &Path) -> String {
    let stderr = stderr.replace(&config_home.display().to_string(), "<config>");
    let mut out = String::new();
    for line in stderr.split('\n') {
        let line = line.trim_start_matches("\r\u{1b}[2K");
        if line.contains(['█', '░']) {
            let label = line.split("  ").next().unwrap_or("");
            out.push_str(&format!("{label}  <count>\n"));
            continue;
        }
        let line = match line.rfind(" (") {
            Some(at) if line.ends_with(')') && is_duration(&line[at + 2..line.len() - 1]) => {
                &line[..at]
            }
            _ => line,
        };
        let line = match line.strip_prefix("Done in ") {
            Some(rest) => format!(
                "Done in <t>. {}",
                rest.split_once(". ").map_or("", |(_, r)| r)
            ),
            None => line.to_owned(),
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

#[test]
fn classification_prints_asks_sends_and_saves_as_the_ts_cli_does() {
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
    let work = env.home.path().join("parity");
    let rust_models = Models::start(&work.join("rust"));
    let ts_models = Models::start(&work.join("ts"));
    env.tools = rust_models.tools.clone();
    env.model_log = rust_models.log.clone();
    env.extra_env.extend([
        ("TYPESAFE_API_KEY".to_owned(), API_KEY.to_owned()),
        (
            "TYPESAFE_BASE_URL".to_owned(),
            rust_models.typesafe.url.clone(),
        ),
        (
            "CHATGPT_TEST_OPENAI_URL".to_owned(),
            rust_models.openai.url.clone(),
        ),
        (
            "CHATGPT_TEST_ANTHROPIC_URL".to_owned(),
            rust_models.anthropic.url.clone(),
        ),
    ]);
    drop(env.legacy_db());
    // The background indexer's batch reads fail, so `classify` downloads
    // the transcripts itself, as the TS CLI (with none cached) does.
    env.fake().state().fail_batch = u32::try_from(chats().len().div_ceil(10)).unwrap();
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    assert_eq!(
        env.fake().state().fail_batch,
        0,
        "the indexer tried every chat"
    );
    assert_eq!(
        copy_chats(&env.data_dir().join("chatgpt.db"), &env.legacy),
        chats().len()
    );

    std::fs::write(work.join("plugin.ts"), PARITY_PLUGIN).unwrap();
    // The bun binary itself, not a version manager's shim, which needs the
    // environment the TS CLI runs without here.
    let bun = Command::new(&bun)
        .args(["-e", "process.stdout.write(process.execPath)"])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map_or(bun, PathBuf::from);
    let mut ts = Ts {
        bun,
        data_home: env.legacy.parent().unwrap().parent().unwrap().to_path_buf(),
        home: work.join("home"),
        config_home: work.join("ts-config"),
        plugin: work.join("plugin.ts"),
        fake_url: ts_fake.url.clone(),
        models: ts_models,
        api_keys: false,
    };
    std::fs::create_dir_all(&ts.home).unwrap();
    let rust_config = env.home.path().join("xdg-config");

    // (arguments, stdin, with API keys)
    let scenarios: &[(&[&str], &str, bool)] = &[
        // configure: status, a key from stdin, removal, the errors.
        (&["configure"], "", false),
        (&["configure", "anthropic"], "  ant-from-stdin \n", false),
        (&["configure", "openai"], "sk-from-stdin", false),
        (&["configure"], "", false),
        (&["configure", "openai", "--remove"], "", false),
        (&["configure", "jev", "--remove"], "", false),
        (&["configure", "--remove"], "", false),
        (&["configure", "grok"], "", false),
        (&["configure", "jev"], "   ", false),
        (&["configure", "anthropic", "--remove"], "", false),
        // classify: a filter, then everything (summaries, follow-up, Luna
        // and titles), then again (all cached), then --redo.
        (&["classify", "--title", "junk", "-y"], "", false),
        (&["classify", "-y"], "", false),
        (&["classify"], "", false),
        (&["classify", "c-idea", "e-long", "--redo"], "", false),
        (
            &["classify", "--title", "junk", "--pinned", "--limit", "1"],
            "",
            false,
        ),
        (&["classify", "nope"], "", false),
        (&["classify", "--older-than", "3x"], "", false),
        // titles.
        (&["titles"], "", false),
        (&["titles", "--title", "idea|maybe", "--redo"], "", false),
        (&["titles", "f-pinned"], "", false),
        // memory classify.
        (&["memory", "classify"], "", false),
        (&["memory", "classify", "--format", "json"], "", false),
        (
            &[
                "memory",
                "classify",
                "--format",
                "csv",
                "--suggest",
                "delete",
            ],
            "",
            false,
        ),
        (
            &["memory", "classify", "--format", "ids", "--limit", "2"],
            "",
            false,
        ),
        (&["memory", "classify", "--suggest", "review"], "", false),
        (&["memory", "classify", "--format", "xml"], "", false),
        // The Jev guard summarises a long chat it hasn't judged.
        (&["delete", "g-long", "a-junk", "--check", "-n"], "", false),
        // With API keys: OpenAI and Anthropic instead of the CLIs (the
        // long chat's summary is made again).
        (&["classify", "e-long", "--redo", "-y"], "", true),
        (&["titles", "c-idea", "d-product", "--redo"], "", true),
        (
            &["memory", "classify", "--redo", "--format", "ids"],
            "",
            true,
        ),
    ];
    let mut compared = 0;
    for (args, stdin, keys) in scenarios {
        if args.first() == Some(&"classify") && *keys {
            // The long chat is summarised again, through the API.
            for db in [
                env.index_db(),
                rusqlite::Connection::open(&env.legacy).unwrap(),
            ] {
                db.execute("delete from summaries where id = 'e-long'", [])
                    .unwrap();
            }
        }
        ts.api_keys = *keys;
        let (ts_ok, ts_out, ts_err) = ts.run(args, stdin);
        let (rust_ok, rust_out, rust_err) = rust(&env, args, stdin, *keys);
        assert_eq!(
            rust_ok, ts_ok,
            "{args:?} exit\nTS: {ts_err}\nRust: {rust_err}"
        );
        assert_eq!(
            rust_out.replace(&rust_config.display().to_string(), "<config>"),
            ts_out.replace(&ts.config_home.display().to_string(), "<config>"),
            "{args:?} stdout"
        );
        assert_eq!(
            normalize(&rust_err, &rust_config),
            normalize(&ts_err, &ts.config_home),
            "{args:?} stderr\nTS raw: {ts_err}\nRust raw: {rust_err}"
        );
        compared += 1;
    }
    // The config files, written the same way.
    assert_eq!(
        std::fs::read_to_string(rust_config.join("chatgpt-cli/config.json")).unwrap(),
        std::fs::read_to_string(ts.config_home.join("chatgpt-cli/config.json")).unwrap()
    );

    // The same model requests, byte for byte.
    let jev = sorted(&rust_models.typesafe.state().bodies);
    assert!(jev.len() > 10, "{}", jev.len());
    assert_eq!(
        jev,
        sorted(&ts.models.typesafe.state().bodies),
        "Jev requests"
    );
    let tool_calls = rust_models.tool_calls();
    assert!(tool_calls.len() > 5, "{}", tool_calls.len());
    assert_eq!(tool_calls, ts.models.tool_calls(), "codex and claude calls");
    let openai = sorted(&rust_models.openai.state().bodies);
    assert!(!openai.is_empty());
    assert_eq!(
        openai,
        sorted(&ts.models.openai.state().bodies),
        "OpenAI requests"
    );
    assert_eq!(
        sorted(&rust_models.anthropic.state().bodies),
        sorted(&ts.models.anthropic.state().bodies),
        "Anthropic requests"
    );

    // The same verdicts, titles and memory classifications.
    let (_, ts_list, _) = ts.run(&["list", "--json", "--all"], "");
    let (_, rust_list, _) = rust(&env, &["list", "--json", "--all"], "", false);
    let (ts_rows, rust_rows): (Value, Value) = (
        serde_json::from_str(&ts_list).unwrap(),
        serde_json::from_str(&rust_list).unwrap(),
    );
    assert_eq!(rust_rows, ts_rows, "list --json --all");
    assert!(
        rust_rows
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["jev"]["luna"] == true)
    );
    let (_, ts_memories, _) = ts.run(&["memory", "classify", "--format", "json"], "");
    let (_, rust_memories, _) = rust(&env, &["memory", "classify", "--format", "json"], "", false);
    assert_eq!(rust_memories, ts_memories, "memory classify --format json");

    // The question before a large batch of summaries, answered no at a
    // terminal.
    ts.api_keys = false;
    let args = ["classify", "h-huge", "--archived"];
    let (_, rust_shown) = support::in_terminal(&env, &args, None, "Go ahead? [y/N] ", "n");
    let ts_shown = ts_in_terminal(&ts, &args, "Go ahead? [y/N] ", "n");
    let (rust_shown, ts_shown) = (plain(&rust_shown), plain(&ts_shown));
    assert!(
        rust_shown.contains(
            "Go ahead? [y/N] n\nSkipping those in step 3; everything else will still be judged.\n"
        ),
        "{rust_shown}"
    );
    let steps = |shown: &str, config: &Path| -> String {
        normalize(shown, config)
            .lines()
            .filter(|line| !line.ends_with("<count>") && !line.ends_with('…'))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(
        steps(&rust_shown, &rust_config),
        steps(&ts_shown, &ts.config_home)
    );
    eprintln!(
        "TS parity: {compared} classification commands matched; {} Jev, {} codex/claude, {} OpenAI requests identical",
        jev.len(),
        tool_calls.len(),
        openai.len()
    );
}

/// The TS CLI in a pseudo-terminal, as `support::in_terminal` runs the
/// Rust one.
fn ts_in_terminal(ts: &Ts, args: &[&str], prompt: &str, answer: &str) -> String {
    use std::io::{Read, Write};
    let template = ts.command(args);
    let mut command = Command::new("script");
    command
        .arg("-q")
        .arg("/dev/null")
        .arg(template.get_program())
        .args(template.get_args())
        .current_dir(repo())
        .env_clear();
    for (name, value) in template.get_envs() {
        if let Some(value) = value {
            command.env(name, value);
        }
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while !String::from_utf8_lossy(&shown.lock().unwrap()).contains(prompt) {
        assert!(
            std::time::Instant::now() < deadline,
            "the TS CLI never prompted: {}",
            String::from_utf8_lossy(&shown.lock().unwrap())
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

/// A terminal transcript as text: no carriage returns or escape sequences
/// (the TS CLI colours `failed:` lines), a live line's redraws as separate
/// lines, and neither a step's opening line nor its running counts: at a
/// terminal the TS CLI opens a step with its zero count where the Rust CLI
/// shows the label (docs/issues/classify-followups.md).
fn plain(shown: &str) -> String {
    let mut out = String::new();
    let mut chars = shown.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {}
            '\u{1b}' => {
                let mut last = None;
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for next in chars.by_ref() {
                        if next.is_ascii_alphabetic() {
                            last = Some(next);
                            break;
                        }
                    }
                }
                // A cleared line starts a redraw.
                if last == Some('K') && !out.ends_with('\n') && !out.is_empty() {
                    out.push('\n');
                }
            }
            _ => out.push(c),
        }
    }
    out
}
