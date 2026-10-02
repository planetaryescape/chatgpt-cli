//! `export` and lexical `search` through the real binary, and the daemon's
//! background search indexer, against the fake chatgpt.com.

#![allow(clippy::unwrap_used)]

mod support;

use std::time::{Duration, Instant};

use fake_chatgpt::Chat;
use fake_chatgpt::fixtures::{Tree, text};
use serde_json::Value;
use support::Env;

const UUID: &str = "6a1b2c3d-0000-4000-8000-00000000abcd";

fn chats() -> Vec<Chat> {
    let mut tree = Tree::new();
    tree.push(text("user", "How do I write async rust with tokio?"));
    tree.push(text(
        "assistant",
        "Use **tokio** for async rust. Caf\u{e9} au lait helps.",
    ));
    let mut rust = Chat::new(UUID, "Rust: async & tokio!", "2026-09-27T10:00:00.000000Z");
    rust.tree = Some(tree.finish());
    let mut archived = Chat::new(
        "b-archived",
        "Old garden plan",
        "2026-09-20T10:00:00.000000Z",
    );
    archived.archived = true;
    archived.text = "garden beds and compost".into();
    let mut other = Chat::new("c-other", "Taxes", "2026-09-26T10:00:00.000000Z");
    other.text = "rust on the garden gate".into();
    vec![rust, archived, other]
}

fn status(env: &Env) -> Value {
    env.status()["search_index"].clone()
}

/// Wait until the indexer has nothing running or pending, and check it
/// indexed every chat.
fn wait_indexed(env: &Env) -> Value {
    let index = env.wait_for_indexer();
    assert_eq!(index["indexed"], index["chats"], "{index}");
    index
}

fn synced(chats: Vec<Chat>) -> Env {
    let env = Env::with_fake(chats);
    env.cmd().arg("sync").assert().success();
    env
}

fn run(env: &Env, args: &[&str]) -> (Option<i32>, String, String) {
    let output = env.cmd().args(args).output().unwrap();
    (
        output.status.code(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

#[test]
fn export_resolves_links_ids_and_prefixes_and_prints_markdown() {
    let env = synced(chats());
    let expected = "# Rust: async & tokio!\n\nhttps://chatgpt.com/c/6a1b2c3d-0000-4000-8000-00000000abcd · 2026-01-01 · gpt-4\n\n---\n\n\
        ## Me\n\nHow do I write async rust with tokio?\n\n---\n\n## ChatGPT\n\nUse **tokio** for async rust. Café au lait helps.\n";
    for reference in [
        UUID.to_owned(),
        UUID.to_uppercase(),
        format!("https://chatgpt.com/c/{UUID}"),
        format!("https://chatgpt.com/g/g-p-68d2fabc/project/c/{UUID}?model=gpt-5"),
        "6a1b".to_owned(),
    ] {
        assert_eq!(env.stdout(&["export", &reference]), expected, "{reference}");
    }
    assert_eq!(env.stdout(&["show", "6a1b"]), expected, "show is export");
    assert!(
        env.fake()
            .calls()
            .contains(&format!("GET /backend-api/conversation/{UUID}")),
        "exports read the chat live"
    );

    // An archived chat needs --archived or --all, by prefix and live.
    let (code, _, stderr) = run(&env, &["export", "b-arch"]);
    assert_eq!(code, Some(2));
    assert_eq!(
        stderr,
        "error: Conversation \"b-arch\" is archived; pass --archived or --all to include it.\n"
    );
    assert!(
        env.stdout(&["export", "b-arch", "--archived"])
            .starts_with("# Old garden plan\n")
    );
    assert!(
        env.stdout(&["export", "b-arch", "--all"])
            .starts_with("# Old garden plan\n")
    );
    let (_, _, stderr) = run(&env, &["export", "c-other", "--archived"]);
    assert_eq!(
        stderr,
        "error: Conversation \"c-other\" is active; omit --archived or pass --all to include it.\n"
    );
}

#[test]
fn an_id_prefix_needs_an_index_and_a_full_id_does_not() {
    let env = Env::new();
    let (code, _, stderr) = run(&env, &["export", "c-ot"]);
    assert_eq!(code, Some(1));
    assert_eq!(
        stderr,
        "error: No local index yet. Run `chatgpt sync` first.\n"
    );
    // With no index, a full id goes straight to ChatGPT (unreachable here).
    let (_, _, stderr) = run(&env, &["export", UUID]);
    assert!(!stderr.contains("No local index"), "{stderr}");
}

#[test]
fn export_errors_match_the_ts_clis() {
    let env = synced(chats());
    for (reference, message) in [
        (
            "https://chatgpt.com/share/abc",
            "Shared links (/share/…) aren't supported; use the chat's own /c/… link.",
        ),
        (
            "zz",
            "No conversation matching \"zz\" in the index. Run `chatgpt sync`?",
        ),
        ("%", "\"%\" matches 3 conversations; use a longer prefix."),
    ] {
        let (code, stdout, stderr) = run(&env, &["export", reference]);
        assert_eq!(code, Some(2), "{reference}");
        assert!(stdout.is_empty());
        assert_eq!(stderr, format!("error: {message}\n"));
    }
    // Gone from ChatGPT since the last sync: no body in the message.
    env.fake().state().chats.retain(|chat| chat.id != "c-other");
    let (_, _, stderr) = run(&env, &["export", "c-other"]);
    assert_eq!(
        stderr,
        "error: 404 from /backend-api/conversation/c-other\n"
    );
}

#[test]
fn export_writes_files_named_from_the_title() {
    let env = synced(chats());
    let dir = tempfile::tempdir().unwrap();
    let output = env
        .cmd()
        .current_dir(dir.path())
        .args(["export", UUID, "-o"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty(), "-o prints nothing on stdout");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "wrote rust-async-tokio.md (0 KB)\n"
    );
    let written = std::fs::read_to_string(dir.path().join("rust-async-tokio.md")).unwrap();
    assert!(written.starts_with("# Rust: async & tokio!\n"));

    let output = env
        .cmd()
        .current_dir(dir.path())
        .args(["export", UUID, "--output", "named.md"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "wrote named.md (0 KB)\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("named.md")).unwrap(),
        written
    );
}

#[test]
fn search_finds_indexed_chats_without_a_search_index_step() {
    let env = synced(chats());
    let index = wait_indexed(&env);
    assert_eq!(index["chats"], 3);
    assert_eq!(index["fetched"], 3, "{index}");
    assert!(
        env.fake()
            .calls()
            .iter()
            .any(|call| call == "POST /backend-api/conversations/batch"),
    );

    let (code, stdout, stderr) = run(&env, &["search", "rust"]);
    assert_eq!(code, Some(0), "{stderr}");
    let ids: Vec<&str> = stdout
        .lines()
        .filter(|line| !line.starts_with("    "))
        .map(|line| line.split("  ").next().unwrap())
        .collect();
    assert_eq!(ids, [UUID, "c-other"], "active only, best first");
    assert!(stdout.starts_with(&format!(
        "{UUID}  2026-09-27     Rust: async & tokio!\n    "
    )));
    assert_eq!(stderr, "2 conversation(s) found locally.\n");

    // Diacritics fold; archived chats need --archived or --all.
    assert_eq!(
        env.stdout(&["search", "cafe", "--format", "ids"]),
        format!("{UUID}\n")
    );
    assert_eq!(env.stdout(&["search", "compost", "--format", "ids"]), "");
    assert_eq!(
        env.stdout(&["search", "compost", "--archived", "--format", "ids"]),
        "b-archived\n"
    );
    assert_eq!(
        env.stdout(&[
            "search", "garden", "--all", "--format", "ids", "--limit", "1"
        ])
        .lines()
        .count(),
        1
    );
    let hits: Vec<Value> =
        serde_json::from_str(&env.stdout(&["search", "tokio", "--format", "json"])).unwrap();
    let keys: Vec<&str> = hits[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["id", "title", "updated", "archived", "score", "snippet"]
    );
    assert!(hits[0]["score"].as_f64().unwrap() > 0.0);
    assert_eq!(
        env.stdout(&["search", "nothingmatches", "--format", "json"]),
        "[]\n"
    );

    let (code, _, stderr) = run(&env, &["search", "!!"]);
    assert_eq!(code, Some(2));
    assert_eq!(
        stderr,
        "error: Search query needs at least one letter or number.\n"
    );
    let (_, _, stderr) = run(&env, &["search", "x", "--format", "yaml"]);
    assert_eq!(
        stderr,
        "error: --format must be json, csv, table, or ids.\n"
    );
    let (_, _, stderr) = run(&env, &["search", "x", "--limit", "0"]);
    assert_eq!(stderr, "error: --limit must be a positive integer.\n");
}

#[test]
fn search_before_any_sync_says_no_local_index_yet() {
    let env = Env::new();
    let (code, _, stderr) = run(&env, &["search", "rust"]);
    assert_eq!(code, Some(1));
    assert_eq!(
        stderr,
        "error: No local index yet. Run `chatgpt sync` first.\n"
    );
}

/// 35 chats, each its own batch-sized slice of work.
fn many() -> Vec<Chat> {
    (0..35)
        .map(|n| {
            let mut chat = Chat::new(
                &format!("chat-{n:02}"),
                &format!("Chat {n}"),
                &format!("2026-09-{:02}T10:00:00.000000Z", n % 28 + 1),
            );
            chat.update_time = format!("2026-09-{:02}T10:{n:02}:00.000000Z", n % 28 + 1);
            chat.text = format!("needle number{n}");
            chat
        })
        .collect()
}

#[test]
fn search_answers_from_what_is_indexed_while_the_indexer_runs() {
    let env = Env::with_fake(many());
    env.fake().state().batch_delay_ms = 700;
    env.cmd().arg("sync").assert().success();
    // The indexer holds a batch at the fake now; reads don't wait for it.
    std::thread::sleep(Duration::from_millis(300));
    let started = Instant::now();
    let (code, stdout, stderr) = run(
        &env,
        &["search", "needle", "--format", "ids", "--limit", "50"],
    );
    assert_eq!(code, Some(0));
    assert!(
        started.elapsed() < Duration::from_millis(600),
        "{:?}",
        started.elapsed()
    );
    let found = stdout.lines().count();
    assert!(found < 35, "found {found}");
    assert!(
        stderr.starts_with(&format!(
            "{found} of 35 chats indexed; the daemon is indexing the rest in the background.\n"
        )),
        "{stderr}"
    );
    let started = Instant::now();
    env.stdout(&["list", "--count"]);
    assert!(
        started.elapsed() < Duration::from_millis(600),
        "list waited on the indexer"
    );

    let index = status(&env);
    assert_eq!(index["in_progress"], true, "{index}");
    let text = env.stdout(&["daemon", "status"]);
    assert!(text.contains("search index: "), "{text}");
    assert!(
        text.contains(" of 35 chats indexed, indexing now"),
        "{text}"
    );

    wait_indexed(&env);
    assert_eq!(
        env.stdout(&["search", "needle", "--format", "ids", "--limit", "50"])
            .lines()
            .count(),
        35
    );
}

#[test]
fn an_interrupted_indexer_resumes_without_fetching_again() {
    let env = Env::with_fake(many());
    env.fake().state().batch_delay_ms = 400;
    env.cmd().arg("sync").assert().success();
    // Stop the daemon once some batches are in.
    let deadline = Instant::now() + Duration::from_secs(20);
    while status(&env)["fetched"].as_u64().unwrap() < 10 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(50));
    }
    env.cmd().args(["daemon", "stop"]).assert().success();
    let asked_before: Vec<String> = batch_ids(&env);
    env.fake().state().batch_delay_ms = 0;

    // A new daemon chunks nothing it has and fetches only the rest, after
    // its first pass.
    env.cmd().arg("sync").assert().success();
    wait_indexed(&env);
    let asked = batch_ids(&env);
    let again: Vec<&String> = asked[asked_before.len()..]
        .iter()
        .filter(|id| asked_before[..asked_before.len().saturating_sub(10)].contains(id))
        .collect();
    assert!(again.is_empty(), "fetched again: {again:?}");
    assert_eq!(status(&env)["indexed"], 35);
}

/// Every id the batch endpoint was asked for, in order.
fn batch_ids(env: &Env) -> Vec<String> {
    env.fake()
        .state()
        .batch_bodies
        .iter()
        .flat_map(|ids| ids.clone())
        .collect()
}

#[test]
fn a_rate_limited_indexer_backs_off_and_sync_and_export_still_work() {
    let env = Env::with_fake(many());
    // Longer than the client waits out: the batch read fails at once.
    env.fake().state().rate_limit_batch = Some((1, 600));
    env.cmd().arg("sync").assert().success();
    let deadline = Instant::now() + Duration::from_secs(20);
    let index = loop {
        let index = status(&env);
        if index["waiting"]
            .as_str()
            .is_some_and(|why| why.starts_with("rate limited"))
        {
            break index;
        }
        assert!(Instant::now() < deadline, "{index}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(index["indexed"], 0);
    assert!(
        index["last_error"]
            .as_str()
            .unwrap()
            .contains("rate limited"),
        "{index}"
    );
    let batches = env.fake().state().batch_bodies.len();
    // A sync and an export don't wait for the backoff, and a new pass
    // doesn't make the indexer try again before it ends.
    env.cmd().arg("sync").assert().success();
    assert!(
        env.stdout(&["export", "chat-01"])
            .contains("needle number1\n")
    );
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        env.fake().state().batch_bodies.len(),
        batches,
        "no batch read during the backoff"
    );
    assert!(
        env.stdout(&["daemon", "status"])
            .contains("rate limited; fetching again in ")
    );
}

#[test]
fn the_indexer_steps_aside_for_sync_and_export() {
    let env = Env::with_fake(many());
    env.fake().state().batch_delay_ms = 300;
    env.cmd().arg("sync").assert().success();
    std::thread::sleep(Duration::from_millis(100));
    // An export runs beside the batch in flight rather than after the run
    // (35 chats: 4 batches, about 3 s).
    let started = Instant::now();
    assert!(
        env.stdout(&["export", "chat-30"])
            .contains("needle number30")
    );
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "{:?}",
        started.elapsed()
    );
    let started = Instant::now();
    env.cmd().arg("sync").assert().success();
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "{:?}",
        started.elapsed()
    );
    wait_indexed(&env);
}

#[test]
fn an_empty_output_name_prints_to_stdout_as_the_ts_cli_does() {
    let env = synced(chats());
    let dir = tempfile::tempdir().unwrap();
    let output = env
        .cmd()
        .current_dir(dir.path())
        .args(["export", UUID, "--output="])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("# Rust: async & tokio!\n"));
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "no file");
}

#[test]
fn export_dash_takes_the_first_id_piped_in_after_checking_them_all() {
    let env = synced(chats());
    let piped = |stdin: &str, extra: &[&str]| {
        let output = env
            .cmd()
            .args(["export", "-"])
            .args(extra)
            .write_stdin(stdin)
            .output()
            .unwrap();
        (
            output.status.code(),
            String::from_utf8(output.stdout).unwrap(),
            String::from_utf8(output.stderr).unwrap(),
        )
    };
    // `list` output pipes straight in: the id is the first word.
    let (code, stdout, _) = piped("c-other  2026-09-26       Taxes\n6a1b  x\n", &[]);
    assert_eq!(code, Some(0));
    assert!(stdout.starts_with("# Taxes\n"), "{stdout}");
    for (stdin, extra, message) in [
        ("", &[][..], "No conversation matching \"-\"."),
        ("\n  \n", &[], "No conversation matching \"-\"."),
        (
            "c-other\nzz\n",
            &[],
            "No conversation matching \"zz\" in the index. Run `chatgpt sync`?",
        ),
        (
            "b-arch\n",
            &[],
            "Conversation \"b-arch\" is archived; pass --archived or --all to include it.",
        ),
    ] {
        let (code, stdout, stderr) = piped(stdin, extra);
        assert_ne!(code, Some(0), "{stdin:?}");
        assert!(stdout.is_empty());
        assert_eq!(stderr, format!("error: {message}\n"), "{stdin:?}");
    }
    let (code, stdout, _) = piped("b-arch\n", &["--archived"]);
    assert_eq!(code, Some(0));
    assert!(stdout.starts_with("# Old garden plan\n"));
}

/// A project move: ChatGPT bumps `update_time` without changing the chat.
/// A sync whose cache reconcile couldn't run leaves the transcript, the
/// judgment and the local title at the old time; the indexer must check the
/// content before it replaces the transcript, and move them all forward.
#[test]
fn a_metadata_only_change_keeps_judgments_current_after_indexing() {
    let mut env = Env::with_fake(chats());
    env.extra_env
        .push(("CHATGPT_INSTANCE".into(), "s2move".into()));
    let data = env
        .home
        .path()
        .join("Library/Application Support/chatgpt-cli-s2move/chatgpt.db");
    env.cmd().arg("sync").assert().success();
    wait_indexed(&env);
    let old = "2026-09-26T10:00:00.000000Z";
    {
        let db = rusqlite::Connection::open(&data).unwrap();
        support::judge(
            &db,
            "c-other",
            old,
            &support::delete_answers("home_money_admin"),
        );
        db.execute(
            "insert into local_titles values ('c-other', ?, 2, 'luna', 'Local taxes', '', 'now')",
            [old],
        )
        .unwrap();
    }
    let jev = |env: &Env| {
        let rows: Vec<Value> = serde_json::from_str(&env.stdout(&["list", "--json"])).unwrap();
        let row = rows.into_iter().find(|row| row["id"] == "c-other").unwrap();
        (
            row["jev"]["suggestion"].clone(),
            row["display_title"].clone(),
        )
    };
    assert_eq!(
        jev(&env),
        (Value::from("delete"), Value::from("Local taxes"))
    );

    // The move; the sync's reconcile read fails, so nothing moves forward.
    {
        let mut fake = env.fake().state();
        let chat = fake
            .chats
            .iter_mut()
            .find(|chat| chat.id == "c-other")
            .unwrap();
        chat.update_time = "2026-09-29T10:00:00.000000Z".into();
        chat.gizmo_id = Some("g-p-1".into());
        fake.fail_batch = 1;
    }
    let sync = env.cmd().arg("sync").output().unwrap();
    assert!(
        String::from_utf8_lossy(&sync.stderr).contains("failed: c-other: 500"),
        "the sync couldn't reconcile"
    );
    wait_indexed(&env);
    assert_eq!(
        jev(&env),
        (Value::from("delete"), Value::from("Local taxes")),
        "the indexer's check moved the judgment and title forward"
    );
    assert_eq!(
        env.stdout(&["search", "garden", "--format", "ids"])
            .lines()
            .filter(|id| *id == "c-other")
            .count(),
        1
    );
}

/// An export many times larger than one IPC frame comes through natively,
/// streamed in slices, and byte for byte as it renders when it fits in one
/// frame: emoji and accents across the cuts included. The TS CLI never
/// runs. A lowered frame cap (debug builds only) stands in for 16 MiB.
#[test]
fn an_export_larger_than_a_frame_streams_byte_identical() {
    let big = || {
        let mut big = Chat::new("d-big", "Huge", "2026-09-28T10:00:00.000000Z");
        big.text = "word café 😀 naïve\ttab ".repeat(20_000);
        big
    };
    let with_big = || {
        let mut all = chats();
        all.push(big());
        all
    };
    // Rendered in one frame, under the normal cap.
    let reference = Env::with_fake(with_big());
    reference.cmd().arg("sync").assert().success();
    let whole = reference.stdout(&["export", "d-big"]);
    assert!(whole.len() > 400_000, "{}", whole.len());

    let mut env = Env::with_fake(with_big());
    env.extra_env
        .push(("CHATGPT_TEST_MAX_FRAME_BYTES".into(), "50000".into()));
    env.cmd().arg("sync").assert().success();

    assert_eq!(env.stdout(&["export", "d-big"]), whole, "stdout");
    let piped = env
        .cmd()
        .args(["export", "-"])
        .write_stdin("d-big  2026-09-28  Huge\n")
        .output()
        .unwrap();
    assert!(piped.status.success());
    assert_eq!(
        String::from_utf8(piped.stdout).unwrap(),
        whole,
        "ids on stdin"
    );
    let written = env
        .cmd()
        .current_dir(env.home.path())
        .args(["export", "d-big", "-o"])
        .output()
        .unwrap();
    assert!(written.status.success());
    assert_eq!(
        std::fs::read_to_string(env.home.path().join("huge.md")).unwrap(),
        whole,
        "-o"
    );
}

#[test]
fn a_session_refused_mid_indexing_ends_the_run_without_setting_chats_aside() {
    let env = Env::with_fake(chats());
    env.fake().state().reject_batch = 100;
    env.cmd().arg("sync").assert().success();
    let index = env.wait_for_indexer();
    assert_eq!(index["failed"], 0, "{index}");
    assert!(
        index["last_error"].as_str().is_some_and(|error| error.contains("401")),
        "{index}"
    );
    // Once ChatGPT takes the session again, every chat is fetched at once:
    // none waits out an hour.
    env.fake().state().reject_batch = 0;
    env.cmd().arg("search-index").assert().success();
    wait_indexed(&env);
}
