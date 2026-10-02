//! The parity harness for `export` and lexical `search`: the TS CLI and the
//! Rust CLI work from the SAME conversation trees and must print the same.
//!
//! The fake chatgpt.com serves rich trees (`fake_chatgpt::fixtures`). The
//! Rust daemon syncs them, fetches every transcript through the batch
//! endpoint and indexes it. A Bun script renders the same trees with this
//! repository's TS `renderTranscript`: the single-chat answers for
//! `export`, and the batch items, as the TS `search-index` caches them, into
//! a TS index holding the same chats. The TS CLI's own `search` then chunks
//! and indexes those (no network: lexical search reads cached transcripts).
//!
//! The TS CLI can't reach the fake (its base URL is fixed), so `export` is
//! compared at the render, and its index-only errors end to end.
//!
//! Needs bun and `bun install` in the repository; skips (with a note)
//! without them.

#![allow(clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use fake_chatgpt::Chat;
use fake_chatgpt::fixtures::{KINDS, rich_chats};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use support::{Env, bun, copy_chats, repo};

/// Renders with the TS sources: `{ id: { markdown, slug } }` for the
/// single-chat answers, and the batch items cached into the TS index.
const RENDER_SCRIPT: &str = r#"
import { Database } from "bun:sqlite";
const [repo, detailsPath, batchPath, dbPath, outPath] = process.argv.slice(2);
const { renderTranscript, toCachedTranscript } = await import(`${repo}/src/render/transcript.ts`);
// export.ts keeps slugify private: take its source as it is.
const exportSource = await Bun.file(`${repo}/src/commands/export.ts`).text();
const slugSource = exportSource.match(/function slugify\(title: string\): string \{[\s\S]*?\n\}/)[0];
const slugify = new Function(`${slugSource.replace(": string): string", ")")}; return slugify;`)();
const out = {};
for (const detail of JSON.parse(await Bun.file(detailsPath).text())) {
	out[detail.conversation_id] = { markdown: renderTranscript(detail), slug: `${slugify(detail.title)}.md` };
}
await Bun.write(outPath, JSON.stringify(out));
const db = new Database(dbPath);
const times = new Map(db.query("select id, update_time from conversations").all().map((r) => [r.id, r.update_time]));
const insert = db.query("insert or replace into transcripts values (?, ?, ?, ?, ?, ?)");
// getConversationsBatch's mapping, then the search indexer's cache row.
for (const { id, create_time, update_time, ...rest } of JSON.parse(await Bun.file(batchPath).text())) {
	const convo = { ...rest, conversation_id: id, create_time: Date.parse(create_time) / 1000,
		update_time: Date.parse(update_time) / 1000, default_model_slug: null };
	const t = toCachedTranscript(id, times.get(id), convo);
	insert.run(t.id, t.update_time, t.render_version, t.markdown, t.turns, t.approx_tokens);
}
"#;

struct Ts {
    bun: PathBuf,
    data_home: PathBuf,
    home: PathBuf,
}

impl Ts {
    fn command(&self, args: &[String]) -> Command {
        let mut command = Command::new(&self.bun);
        command
            .arg(repo().join("src/cli.ts"))
            .args(args)
            .env("XDG_DATA_HOME", &self.data_home)
            .env("HOME", &self.home)
            .env_remove("CHATGPT_BROWSER")
            .env_remove("CHATGPT_BROWSER_PROFILE")
            .current_dir(repo());
        command
    }

    /// Run every command, several at a time; (exit code, stdout, stderr).
    fn run_all(&self, commands: &[Vec<String>]) -> Vec<(Option<i32>, String, String)> {
        commands
            .chunks(8)
            .flat_map(|group| {
                let children: Vec<_> = group
                    .iter()
                    .map(|args| {
                        self.command(args)
                            .stdout(std::process::Stdio::piped())
                            .stderr(std::process::Stdio::piped())
                            .spawn()
                            .unwrap()
                    })
                    .collect();
                children
                    .into_iter()
                    .map(|child| {
                        let output = child.wait_with_output().unwrap();
                        (
                            output.status.code(),
                            String::from_utf8(output.stdout).unwrap(),
                            String::from_utf8(output.stderr).unwrap(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}

fn transcripts(db: &Path) -> Vec<(String, String, String, i64, i64)> {
    let db = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut statement = db
        .prepare(
            "select id, update_time, markdown, turns, approx_tokens from transcripts order by id",
        )
        .unwrap();
    statement
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn wait_indexed(env: &Env, chats: usize) {
    let index = env.wait_for_indexer();
    assert_eq!(index["indexed"], chats, "{index}");
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

#[test]
fn export_and_search_match_the_ts_cli_on_the_same_chats() {
    let Some(bun) = bun() else {
        eprintln!("skipping the TS parity harness: bun isn't on PATH");
        return;
    };
    if !repo().join("node_modules/commander").is_dir() {
        eprintln!("skipping the TS parity harness: run `bun install` in the repository first");
        return;
    }
    let chats: Vec<Chat> = rich_chats(KINDS * 5, 0x5eed);
    let env = Env::with_fake(chats.clone());
    let ts_db = env.legacy_db();
    // Display titles: search prints the local title when there is one.
    for (id, title) in [("chat-003", "Local three"), ("chat-012", "Local twelve")] {
        ts_db
            .execute(
                "insert into local_titles values (?, '2020-01-01T00:00:00Z', 2, 'manual', ?, '', 'now')",
                [id, title],
            )
            .unwrap();
    }
    drop(ts_db);
    env.cmd().arg("sync").assert().success();
    wait_indexed(&env, chats.len());
    assert_eq!(
        copy_chats(&env.data_dir().join("chatgpt.db"), &env.legacy),
        chats.len()
    );

    // Render the same trees with the TS sources.
    let work = env.home.path().join("parity");
    std::fs::create_dir_all(&work).unwrap();
    let details: Vec<Value> = chats.iter().map(Chat::detail).collect();
    let batch: Vec<Value> = chats.iter().map(Chat::batch_item).collect();
    std::fs::write(work.join("details.json"), json!(details).to_string()).unwrap();
    std::fs::write(work.join("batch.json"), json!(batch).to_string()).unwrap();
    std::fs::write(work.join("render.ts"), RENDER_SCRIPT).unwrap();
    let rendered = Command::new(&bun)
        .arg(work.join("render.ts"))
        .arg(repo())
        .arg(work.join("details.json"))
        .arg(work.join("batch.json"))
        .arg(&env.legacy)
        .arg(work.join("rendered.json"))
        .output()
        .unwrap();
    assert!(
        rendered.status.success(),
        "{}",
        String::from_utf8_lossy(&rendered.stderr)
    );
    let ts_exports: Value =
        serde_json::from_str(&std::fs::read_to_string(work.join("rendered.json")).unwrap())
            .unwrap();

    // The transcripts the Rust indexer cached are the ones the TS CLI's
    // search-index would cache.
    let rust_cache = transcripts(&env.data_dir().join("chatgpt.db"));
    assert_eq!(rust_cache.len(), chats.len());
    assert_eq!(rust_cache, transcripts(&env.legacy), "cached transcripts");

    // export: byte for byte, by id, by link and as `show`; `-o` names the
    // file as the TS CLI would.
    let out_dir = work.join("out");
    std::fs::create_dir_all(&out_dir).unwrap();
    for chat in &chats {
        let expected = ts_exports[&chat.id]["markdown"].as_str().unwrap();
        assert_eq!(
            env.stdout(&["export", &chat.id, "--all"]),
            expected,
            "export {}",
            chat.id
        );
        let output = env
            .cmd()
            .current_dir(&out_dir)
            .args(["export", &chat.id, "--all", "-o"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let slug = ts_exports[&chat.id]["slug"].as_str().unwrap();
        assert_eq!(
            std::fs::read_to_string(out_dir.join(slug)).unwrap(),
            expected,
            "export -o {} as {slug}",
            chat.id
        );
    }
    assert_eq!(
        env.stdout(&["show", "chat-007"]),
        ts_exports["chat-007"]["markdown"].as_str().unwrap()
    );

    let ts = Ts {
        bun,
        data_home: env.legacy.parent().unwrap().parent().unwrap().to_path_buf(),
        home: env.home.path().join("ts-home"),
    };
    std::fs::create_dir_all(&ts.home).unwrap();

    // export's errors that need only the index.
    let failures = [
        vec!["export", "chat-0"],
        vec!["export", "nope"],
        vec!["export", "chat-010"],
        vec!["export", "chat-001", "--archived"],
        vec!["export", "https://chatgpt.com/share/abc"],
        vec!["show", "CHAT-0_"],
    ];
    let ts_failures = ts.run_all(
        &failures
            .iter()
            .map(|args| strings(args))
            .collect::<Vec<_>>(),
    );
    for (args, (code, _, ts_stderr)) in failures.iter().zip(&ts_failures) {
        assert_eq!(*code, Some(1), "TS {args:?}");
        let rust = env.cmd().args(args).output().unwrap();
        assert!(!rust.status.success());
        assert_eq!(
            String::from_utf8_lossy(&rust.stderr),
            *ts_stderr,
            "{args:?}"
        );
    }

    // `export -` reads the selector from stdin; these fail before any
    // request, so the TS CLI can answer them here.
    for stdin in [
        "nope\n",
        "",
        "\n \n",
        "chat-010  2026  title\n",
        "chat-001\nnope\n",
        "chat-0\n",
    ] {
        let mut ts_child = ts
            .command(&strings(&["export", "-"]))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(&mut ts_child.stdin.take().unwrap(), stdin.as_bytes()).unwrap();
        let ts_output = ts_child.wait_with_output().unwrap();
        assert_eq!(ts_output.status.code(), Some(1), "TS export - {stdin:?}");
        let rust = env
            .cmd()
            .args(["export", "-"])
            .write_stdin(stdin)
            .output()
            .unwrap();
        assert!(!rust.status.success());
        assert_eq!(
            String::from_utf8_lossy(&rust.stderr),
            String::from_utf8_lossy(&ts_output.stderr),
            "export - with {stdin:?}"
        );
    }

    // search: the TS CLI chunks every cached transcript on its first local
    // search with --all, as the Rust indexer did.
    let (code, _, stderr) = ts
        .run_all(&[strings(&["search", "--all", "rust"])])
        .remove(0);
    assert_eq!(code, Some(0), "{stderr}");
    let x_run = "x".repeat(787);
    let w_run = "w".repeat(299);
    let queries: Vec<&str> = vec![
        "rust",
        "rust async",
        "cafe",
        "café",
        "CAFÉ",
        "naive",
        "zurich",
        "Zürich",
        "\"pricing page\"",
        "C++ templates",
        "don't",
        "2026",
        "v1.2",
        "garden sermon",
        "日本語",
        "Привет",
        "running",
        "notes.pdf",
        "attached file",
        "image",
        "generated image",
        "final cut",
        "Example",
        "canvas final",
        "Thought",
        "old question",
        "hidden context",
        "zzzqqq",
        &x_run,
        &w_run,
        "the and of",
    ];
    let mut commands = Vec::new();
    for query in &queries {
        for format in [None, Some("json"), Some("csv"), Some("table"), Some("ids")] {
            let mut args = strings(&["search", query]);
            if let Some(format) = format {
                args.extend(strings(&["--format", format]));
            }
            commands.push(args);
        }
        for scope in [&["--all"][..], &["--archived"], &["--all", "--limit", "3"]] {
            let mut args = strings(&["search", query, "--format", "json"]);
            args.extend(strings(scope));
            commands.push(args);
        }
    }
    let ts_results = ts.run_all(&commands);
    let mut compared = 0;
    let mut hits = 0;
    for (args, (code, ts_stdout, _)) in commands.iter().zip(&ts_results) {
        assert_eq!(*code, Some(0), "TS {args:?}");
        let rust = env.stdout(&args.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(rust, *ts_stdout, "{args:?}");
        if args.contains(&"json".to_owned()) && ts_stdout != "[]\n" {
            hits += 1;
        }
        compared += 1;
    }
    for args in [
        strings(&["search", "👍"]),
        strings(&["search", "!!!", "--all"]),
    ] {
        let (code, _, ts_stderr) = ts.run_all(std::slice::from_ref(&args)).remove(0);
        assert_eq!(code, Some(1));
        let rust = env.cmd().args(&args).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&rust.stderr), ts_stderr, "{args:?}");
    }
    eprintln!(
        "TS parity: {} exports byte-identical; {compared} searches ({} queries, {hits} JSON answers with hits) identical",
        chats.len(),
        queries.len()
    );
    assert!(
        hits >= queries.len(),
        "too few queries found anything: {hits}"
    );
}
