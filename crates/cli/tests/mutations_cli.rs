//! `archive`, `unarchive`, `delete`, `rename` and `title` through the real
//! binary and a daemon synced from the fake chatgpt.com: selection and its
//! errors, previews, dry runs, `-y`, the confirmation prompts in a
//! pseudo-terminal (`script`), ids on stdin, ChatGPT's quirks, and no
//! second try of a write that mustn't happen twice.

#![allow(clippy::unwrap_used)]

mod support;

use fake_chatgpt::{Chat, WriteFailure};
use support::{Env, delete_answers, in_terminal, judge};

const OLD: &str = "2024-01-01T10:00:00.000000Z";

fn chats() -> Vec<Chat> {
    let mut pinned = Chat::new("p-pinned", "Pinned junk", "2024-01-02T10:00:00.000000Z");
    pinned.pinned = true;
    let mut archived = Chat::new("z-archived", "Archived chat", "2024-01-03T10:00:00.000000Z");
    archived.archived = true;
    vec![
        Chat::new("a-new", "New idea", "2026-09-27T10:00:00.000000Z"),
        Chat::new("b-old", "Old taxes", OLD),
        Chat::new("c-old", "Old trip", "2024-01-01T09:00:00.000000Z"),
        pinned,
        archived,
    ]
}

fn synced() -> Env {
    let env = Env::with_fake(chats());
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

fn ids(env: &Env, args: &[&str]) -> Vec<String> {
    let mut args = args.to_vec();
    args.extend(["--format", "ids"]);
    env.stdout(&args).lines().map(str::to_owned).collect()
}

fn writes(env: &Env) -> Vec<String> {
    env.fake()
        .calls()
        .into_iter()
        .filter(|call| {
            call.starts_with("PATCH") || call.starts_with("DELETE") || call.contains("/rename")
        })
        .collect()
}

/// Without durations, which vary run to run.
fn steady(stderr: &str) -> String {
    stderr
        .lines()
        // A running count draws a bar; a summary line doesn't.
        .filter(|line| !line.contains(['█', '░']))
        .map(|line| match line.rfind(" (") {
            Some(at) if line.ends_with("s)") => &line[..at],
            _ => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_dry_run_previews_filtered_chats_and_skips_pinned_ones() {
    let env = synced();
    let (code, stdout, stderr) = run(&env, &["archive", "--older-than", "1y", "-n"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.is_empty(), "previews go to stderr");
    assert_eq!(
        stderr,
        "b-old  2024-01-01       Old taxes\nc-old  2024-01-01       Old trip\ndry run: would archive 2 conversation(s).\n"
    );
    let (_, _, pinned) = run(
        &env,
        &["archive", "--older-than", "1y", "--pinned", "--dry-run"],
    );
    assert!(
        pinned.starts_with("p-pinned  2024-01-02   P   Pinned junk\n"),
        "{pinned}"
    );
    assert!(pinned.ends_with("dry run: would archive 3 conversation(s).\n"));
    assert!(writes(&env).is_empty());
}

#[test]
fn yes_archives_in_chatgpt_then_the_index_and_unarchive_undoes_it() {
    let env = synced();
    let (code, _, stderr) = run(&env, &["archive", "b-old", "c-o", "-y"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        steady(&stderr),
        "b-old  2024-01-01       Old taxes\nc-old  2024-01-01       Old trip\nArchiving…\nArchived 2 conversation(s)"
    );
    assert_eq!(
        writes(&env),
        [
            "PATCH /backend-api/conversation/b-old",
            "PATCH /backend-api/conversation/c-old"
        ]
    );
    assert!(
        env.fake()
            .state()
            .chats
            .iter()
            .filter(|c| c.archived)
            .count()
            == 3
    );
    assert_eq!(
        ids(&env, &["list", "--archived"]),
        ["z-archived", "b-old", "c-old"]
    );

    // Unarchive selects archived chats by default; an active one is refused.
    let (code, _, stderr) = run(&env, &["unarchive", "a-new", "-y"]);
    assert_eq!(code, Some(2));
    assert_eq!(
        stderr,
        "error: Conversation \"a-new\" is active; omit --archived or pass --all to include it.\n"
    );
    let (code, _, stderr) = run(&env, &["unarchive", "b-old", "-y"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stderr.contains("Unarchived 1 conversation(s)"), "{stderr}");
    assert_eq!(ids(&env, &["list", "--archived"]), ["z-archived", "c-old"]);
}

#[test]
fn selection_errors_read_as_the_ts_clis() {
    let env = synced();
    for (args, message) in [
        (
            &["delete", "-y"][..],
            "Pass conversation ids, `-` to read ids from stdin, or a filter such as --older-than 1y or --title.",
        ),
        (
            &["archive", "z-archived", "-y"][..],
            "Conversation \"z-archived\" is archived; pass --archived or --all to include it.",
        ),
        (
            &["delete", "nope", "-y"][..],
            "No conversation matching \"nope\" in the index. Run `chatgpt sync`?",
        ),
        (
            &["archive", "-n", "--limit", "0"][..],
            "--limit must be a positive whole number.",
        ),
        (
            &["unarchive", "z-archived", "--check", "-n"][..],
            "--check applies to archive and delete only.",
        ),
    ] {
        let (code, _, stderr) = run(&env, args);
        assert_eq!(stderr, format!("error: {message}\n"), "{args:?}");
        assert_ne!(code, Some(0));
    }
    let (_, _, stderr) = run(&env, &["delete", "-", "-y"]);
    assert_eq!(stderr, "Nothing matched.\n", "empty stdin selects nothing");
    assert!(writes(&env).is_empty());
}

#[test]
fn a_delete_that_finds_the_chat_gone_counts_as_done() {
    let env = synced();
    // Deleted in the browser since the sync.
    env.fake().state().chats.retain(|chat| chat.id != "c-old");
    let (code, _, stderr) = run(&env, &["delete", "b-old", "c-old", "-y"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stderr.contains("Deleted 2 conversation(s)"), "{stderr}");
    assert_eq!(
        ids(&env, &["list", "--all"]),
        ["a-new", "z-archived", "p-pinned"]
    );
    // An archive of a gone chat fails, and the index drops it anyway.
    env.fake().state().chats.retain(|chat| chat.id != "a-new");
    let (code, _, stderr) = run(&env, &["archive", "a-new", "-y"]);
    assert_eq!(code, Some(1));
    assert!(
        stderr.contains(
            "failed: a-new New idea: 404 Not Found from /backend-api/conversation/a-new\n"
        ),
        "{stderr}"
    );
    assert!(!stderr.contains("secret"), "no response body: {stderr}");
    assert_eq!(ids(&env, &["list", "--all"]), ["z-archived", "p-pinned"]);
}

#[test]
fn a_delete_or_rename_is_never_sent_twice() {
    let env = synced();
    env.fake()
        .state()
        .fail_writes
        .extend([WriteFailure::Status(502), WriteFailure::Truncated]);
    let (code, _, stderr) = run(&env, &["delete", "b-old", "-y"]);
    assert_eq!(code, Some(1));
    assert!(
        stderr.contains("failed: b-old Old taxes: ChatGPT's gateway answered 502 Bad Gateway for /backend-api/conversation/id/b-old, so it may have applied; run `chatgpt sync` and check before trying again\n"),
        "{stderr}"
    );
    assert_eq!(writes(&env), ["DELETE /backend-api/conversation/id/b-old"]);
    assert!(
        ids(&env, &["list"]).contains(&"b-old".to_owned()),
        "unknown: kept"
    );

    // The rename applied, but the answer broke off.
    let (code, _, stderr) = run(&env, &["rename", "c-old", "Trip 2024"]);
    assert_eq!(code, Some(1));
    assert_eq!(
        stderr,
        "error: the connection failed before ChatGPT answered /backend-api/conversation/id/c-old/rename, so it may have applied; run `chatgpt sync` and check before trying again\n"
    );
    assert_eq!(
        writes(&env)
            .iter()
            .filter(|call| call.contains("/rename"))
            .count(),
        1
    );

    // An archive is idempotent: a gateway error is retried.
    env.fake()
        .state()
        .fail_writes
        .push_back(WriteFailure::Status(503));
    let (code, _, stderr) = run(&env, &["archive", "b-old", "-y"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        writes(&env)
            .iter()
            .filter(|call| call.starts_with("PATCH"))
            .count(),
        2
    );
}

#[test]
fn renames_go_to_chatgpt_and_the_index_and_a_legacy_500_says_it_likely_applied() {
    let env = synced();
    let (code, _, stderr) = run(&env, &["rename", "b-old", "Taxes 2023"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(stderr, "renamed \"Old taxes\" → \"Taxes 2023\"\n");
    assert_eq!(
        env.fake()
            .state()
            .chats
            .iter()
            .find(|chat| chat.id == "b-old")
            .unwrap()
            .title,
        "Taxes 2023"
    );
    assert!(
        env.stdout(&["list"])
            .contains("b-old  2024-01-01       Taxes 2023\n")
    );

    env.fake().state().legacy_rename.insert("c-old".into());
    let (code, _, stderr) = run(&env, &["rename", "c-old", "Trip"]);
    assert_eq!(code, Some(1));
    assert_eq!(
        stderr,
        "error: ChatGPT returned a server error. On older chats the rename usually applies anyway; run `chatgpt sync` and `chatgpt list --title` to check.\n"
    );
}

#[test]
fn a_local_title_shows_in_list_and_survives_the_ts_import() {
    let env = synced();
    let (code, _, stderr) = run(&env, &["title", "b-old", "  My   taxes  "]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(stderr, "Local title saved for b-old: My   taxes\n");
    assert!(
        env.stdout(&["list"])
            .contains("b-old  2024-01-01       My taxes\n")
    );
    assert!(writes(&env).is_empty(), "local only");
    // The TS index never has it; the import keeps it.
    drop(env.legacy_db());
    env.cmd().arg("import-legacy").assert().success();
    assert!(env.stdout(&["list"]).contains("My taxes"));
    let (_, _, stderr) = run(&env, &["title", "b-old", " "]);
    assert_eq!(stderr, "error: Local title must be 1–100 characters.\n");
    let (_, _, stderr) = run(&env, &["title", "z-archived", "x"]);
    assert!(stderr.contains("is archived; pass --archived"), "{stderr}");
}

#[test]
fn the_prompt_asks_at_the_terminal_and_only_yes_goes_ahead() {
    let env = synced();
    let (code, shown) = in_terminal(
        &env,
        &["archive", "b-old"],
        None,
        "archive 1 conversation(s)? [y/N] ",
        "n",
    );
    assert_eq!(code, Some(0));
    assert!(
        shown.ends_with("archive 1 conversation(s)? [y/N] n\nCancelled.\n"),
        "{shown}"
    );
    assert!(writes(&env).is_empty());

    let (code, shown) = in_terminal(&env, &["archive", "b-old"], None, "[y/N] ", "YES");
    assert_eq!(code, Some(0), "{shown}");
    assert!(shown.contains("Archived 1 conversation(s)"), "{shown}");
}

#[test]
fn a_delete_wants_the_count_typed_even_with_ids_from_stdin() {
    let env = synced();
    let prompt = "Permanently delete 2 conversation(s)? This cannot be undone. Type 2 to confirm: ";
    // With ids piped in, the answer comes from the terminal.
    let (code, shown) = in_terminal(&env, &["delete", "-"], Some("b-old\nc-old"), prompt, "y");
    assert_eq!(code, Some(0), "{shown}");
    assert!(shown.ends_with("Cancelled.\n"), "{shown}");
    assert!(writes(&env).is_empty());
    let (code, shown) = in_terminal(&env, &["delete", "-"], Some("b-old\nc-old"), prompt, "2");
    assert_eq!(code, Some(0), "{shown}");
    assert!(shown.contains("Deleted 2 conversation(s)"), "{shown}");
    assert_eq!(ids(&env, &["list"]), ["a-new", "p-pinned"]);
}

#[test]
fn without_a_terminal_a_prompt_fails_instead_of_hanging() {
    // Run from a terminal, /dev/tty is the developer's, and the prompt
    // would wait on it: only a run without one can check this.
    if std::fs::File::open("/dev/tty").is_ok() {
        return;
    }
    let env = synced();
    let output = env.cmd().args(["archive", "b-old"]).output().unwrap();
    assert_ne!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stderr).contains("pass -y"));
}

#[test]
fn suggest_applies_jev_suggestions_but_leaves_unsure_chats_out() {
    let env = Env::with_fake(chats());
    let db = env.legacy_db();
    judge(&db, "b-old", OLD, &delete_answers("other"));
    let mut unsure: serde_json::Value = serde_json::from_str(&delete_answers("other")).unwrap();
    unsure["personal_record"]["noul"] = serde_json::json!(0.45);
    unsure["nothing_there"]["noul"] = serde_json::json!(0.5);
    unsure["re_askable"]["noul"] = serde_json::json!(0.75);
    unsure["unfinished"]["noul"] = serde_json::json!(0.4);
    judge(
        &db,
        "c-old",
        "2024-01-01T09:00:00.000000Z",
        &unsure.to_string(),
    );
    drop(db);
    env.cmd().arg("sync").assert().success();
    assert_eq!(
        ids(&env, &["list", "--suggest", "delete"]),
        ["b-old", "c-old"]
    );
    // `archive --suggest delete` isn't applying Jev's own suggestion: no guard.
    let (_, _, stderr) = run(&env, &["archive", "--suggest", "delete", "-n"]);
    assert!(
        stderr.ends_with("dry run: would archive 2 conversation(s).\n"),
        "{stderr}"
    );
}

#[test]
fn another_accounts_session_changes_nothing() {
    // The daemon reads test cookies from its own environment.
    let mut env = Env::with_fake(chats());
    env.extra_env.push((
        "CHATGPT_TEST_COOKIE_CHROME".into(),
        fake_chatgpt::OTHER_COOKIE.into(),
    ));
    env.cmd().arg("sync").assert().success();
    for args in [
        &["--browser", "chrome", "delete", "b-old", "c-old", "-y"][..],
        &["--browser", "chrome", "rename", "b-old", "x"][..],
    ] {
        let (code, _, stderr) = run(&env, args);
        assert_ne!(code, Some(0), "{args:?}: {stderr}");
        assert!(
            stderr.contains("another ChatGPT account") || stderr.contains("account"),
            "{stderr}"
        );
    }
    assert!(writes(&env).is_empty());
    assert_eq!(
        ids(&env, &["list"]),
        ["a-new", "p-pinned", "b-old", "c-old"]
    );
}

#[test]
fn a_sync_running_during_a_change_never_undoes_it() {
    let env = synced();
    {
        let mut state = env.fake().state();
        // a-new changed since the sync, so the pass's listing has it as
        // active; the archived listing has z-archived. Both are read before
        // the changes below and land after them.
        let chat = state
            .chats
            .iter_mut()
            .find(|chat| chat.id == "a-new")
            .unwrap();
        chat.update_time = "2026-09-29T10:00:00.000000Z".into();
        state.list_delay_ms = 1500;
    }
    let sync = env
        .std_cmd()
        .arg("sync")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    // The active list is read; the archived list is in flight.
    std::thread::sleep(std::time::Duration::from_millis(1800));
    let (code, _, stderr) = run(&env, &["delete", "z-archived", "--archived", "-y"]);
    assert_eq!(code, Some(0), "{stderr}");
    let (code, _, stderr) = run(&env, &["archive", "a-new", "-y"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(sync.wait_with_output().unwrap().status.success());
    env.fake().state().list_delay_ms = 0;
    assert_eq!(
        ids(&env, &["list", "--archived"]),
        ["a-new"],
        "deleted stays deleted; archived stays archived"
    );
}

#[test]
fn a_chat_named_twice_is_changed_once() {
    let env = synced();
    let (code, _, stderr) = run(&env, &["delete", "b-old", "b-o", "b-old", "-n"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        stderr,
        "b-old  2024-01-01       Old taxes\ndry run: would delete 1 conversation(s).\n"
    );
    let (code, _, stderr) = run(&env, &["archive", "c-old", "c-old", "-y"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(writes(&env), ["PATCH /backend-api/conversation/c-old"]);
}
