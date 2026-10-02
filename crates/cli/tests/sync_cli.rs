//! `sync`, `list` and `stats` through the real binary and a daemon synced
//! from the fake chatgpt.com, with judgments in its index.

#![allow(clippy::unwrap_used)]

mod support;

use std::time::{Duration, Instant};

use fake_chatgpt::Chat;
use serde_json::Value;
use support::{Env, brainstorm_answers, delete_answers, judge};

fn chats() -> Vec<Chat> {
    let mut pinned = Chat::new("b-pinned", "Pinned chat", "2026-09-26T10:00:00.000000Z");
    pinned.pinned = true;
    let mut archived = Chat::new("c-archived", "Archived chat", "2026-09-20T10:00:00.000000Z");
    archived.archived = true;
    let mut project = Chat::new("d-project", "Project chat", "2026-09-25T10:00:00.000000Z");
    project.gizmo_id = Some("g-p-1".into());
    vec![
        Chat::new(
            "a-outline",
            "Love Book Outline",
            "2026-09-27T10:00:00.000000Z",
        ),
        pinned,
        archived,
        project,
        Chat::new("e-old", "Old taxes", "2025-01-01T10:00:00.000000Z"),
    ]
}

/// A daemon synced from the fake, with judgments in its index.
fn synced() -> Env {
    let env = Env::with_fake(chats());
    env.cmd().arg("sync").assert().success();
    let db = env.index_db();
    judge(
        &db,
        "a-outline",
        "2026-09-27T10:00:00.000000Z",
        &brainstorm_answers("writing_creativity"),
    );
    judge(
        &db,
        "e-old",
        "2025-01-01T10:00:00.000000Z",
        &delete_answers("home_money_admin"),
    );
    db.execute(
        "insert into local_titles values ('a-outline', '2026-09-27T10:00:00.000000Z', 2, 'luna', 'Developing the Love Book', '', 'now')",
        [],
    )
    .unwrap();
    env
}

fn ids(env: &Env, args: &[&str]) -> Vec<String> {
    let mut args = args.to_vec();
    args.extend(["--format", "ids"]);
    env.stdout(&args).lines().map(str::to_owned).collect()
}

#[test]
fn list_before_any_sync_says_no_local_index_yet() {
    let env = Env::new();
    let output = env.cmd().arg("list").output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty(), "errors never go to stdout");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "error: No local index yet. Run `chatgpt sync` first.\n"
    );
}

#[test]
fn list_json_has_the_ts_fields_and_the_judgments() {
    let env = synced();
    let rows: Vec<Value> = serde_json::from_str(&env.stdout(&["list", "--json"])).unwrap();
    let ids: Vec<&str> = rows.iter().map(|row| row["id"].as_str().unwrap()).collect();
    assert_eq!(
        ids,
        ["a-outline", "b-pinned", "d-project", "e-old"],
        "active, newest first, pinned included"
    );
    let outline = &rows[0];
    let keys: Vec<&str> = outline
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "id",
            "title",
            "create_time",
            "update_time",
            "is_archived",
            "pinned",
            "project_id",
            "local_title",
            "display_title",
            "topic",
            "jev"
        ]
    );
    assert_eq!(outline["title"], "Love Book Outline");
    assert_eq!(outline["local_title"], "Developing the Love Book");
    assert_eq!(outline["display_title"], "Developing the Love Book");
    assert_eq!(outline["topic"], "writing_creativity");
    assert_eq!(outline["update_time"], "2026-09-27T10:00:00.000000Z");
    assert_eq!(outline["jev"]["suggestion"], "keep");
    assert_eq!(outline["jev"]["unsure"], false);
    assert_eq!(outline["jev"]["brainstorm"], "writing");
    assert_eq!(
        outline["jev"]["answers"]["brainstorm_for"]["choice"],
        "writing"
    );
    assert!(
        outline["jev"]["reason"]
            .as_str()
            .unwrap()
            .starts_with("nothing 0.02 · re-askable 0.10 · worth 2.8/3")
    );
    assert_eq!(rows[1]["pinned"], 1);
    assert_eq!(rows[1]["topic"], Value::Null);
    assert!(rows[1].get("jev").is_none(), "no judgment, no jev key");
    assert_eq!(rows[2]["project_id"], "g-p-1");
    assert_eq!(rows[3]["jev"]["suggestion"], "delete");
}

#[test]
fn filters_combine_as_in_the_ts_cli() {
    let env = synced();
    assert_eq!(ids(&env, &["list", "--archived"]), ["c-archived"]);
    assert_eq!(env.stdout(&["list", "--all", "--count"]), "5\n");
    assert_eq!(ids(&env, &["list", "--title", "OUTLINE"]), ["a-outline"]);
    assert_eq!(
        ids(&env, &["list", "--title", "^developing"]),
        ["a-outline"]
    );
    assert_eq!(ids(&env, &["list", "--suggest", "delete"]), ["e-old"]);
    assert_eq!(ids(&env, &["list", "--brainstorm"]), ["a-outline"]);
    assert_eq!(
        ids(&env, &["list", "--brainstorm", "writing"]),
        ["a-outline"]
    );
    assert!(ids(&env, &["list", "--brainstorm", "product"]).is_empty());
    assert_eq!(
        ids(&env, &["list", "--topic", "home_money_admin"]),
        ["e-old"]
    );
    assert_eq!(
        ids(&env, &["list", "--limit", "2"]),
        ["a-outline", "b-pinned"]
    );
    assert_eq!(ids(&env, &["list", "--older-than", "1y"]), ["e-old"]);
    assert_eq!(ids(&env, &["list", "--before", "2025-06-01"]), ["e-old"]);
    assert_eq!(
        ids(&env, &["list", "--after", "2026-09-26"]),
        ["a-outline", "b-pinned"]
    );

    let text = env.stdout(&["list", "--limit", "1"]);
    assert_eq!(
        text,
        "a-outline  2026-09-27       keep     writing_creativity   idea:writing  Developing the Love Book\n"
    );

    for (args, message) in [
        (
            vec!["list", "--suggest", "burn"],
            "error: --suggest must be one of delete, archive, keep.\n",
        ),
        (
            vec!["list", "--limit", "0"],
            "error: --limit must be a positive whole number.\n",
        ),
        (
            vec!["list", "--format", "csv"],
            "error: --format must be ids.\n",
        ),
        (
            vec!["list", "--json", "--format", "ids"],
            "error: Choose only one of --json or --format.\n",
        ),
        (
            vec!["list", "--older-than", "3x"],
            "error: Invalid age \"3x\". Use a number and unit, e.g. 30d, 12w, 6m, 2y.\n",
        ),
    ] {
        let output = env.cmd().args(&args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert_eq!(String::from_utf8_lossy(&output.stderr), message, "{args:?}");
    }
}

#[test]
fn list_answers_from_the_index_without_the_network() {
    let env = synced();
    // The sync woke the search indexer, which reads transcripts.
    env.wait_for_indexer();
    let before = env.fake().calls().len();
    let started = Instant::now();
    env.stdout(&["list"]);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(env.fake().calls().len(), before, "list made a request");
}

#[test]
fn stats_counts_chats_and_live_memories() {
    let env = synced();
    let output = env.stdout(&["stats"]);
    assert!(
        output.starts_with("4 chat(s), 2 judged, 2 not yet judged\n\n"),
        "{output}"
    );
    assert!(output.contains("\ndelete           1\n"), "{output}");
    assert!(
        output.contains("\nwriting                       1\n"),
        "{output}"
    );
    assert!(output.contains("\n0 saved memories\n"), "{output}");

    env.fake().state().memories = None;
    let failed = env.cmd().arg("stats").output().unwrap();
    assert_eq!(failed.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&failed.stderr);
    assert!(
        stderr.contains(
            "Saved-memory stats unavailable: 500 Internal Server Error from /backend-api/memories"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains("secret body"),
        "a response body reached the terminal"
    );
    assert!(String::from_utf8_lossy(&failed.stdout).starts_with("4 chat(s)"));
}

#[test]
fn delta_sync_sees_new_changed_archived_unarchived_and_deleted_chats() {
    let mut chats = chats();
    let mut returning = Chat::new("g-returning", "Was archived", "2026-09-01T10:00:00.000000Z");
    returning.archived = true;
    chats.push(returning);
    let env = Env::with_fake(chats);
    env.cmd().arg("sync").assert().success();
    {
        let mut state = env.fake().state();
        state.chats.push(Chat::new(
            "f-new",
            "Brand new",
            "2026-09-30T10:00:00.000000Z",
        ));
        let outline = state
            .chats
            .iter_mut()
            .find(|c| c.id == "a-outline")
            .unwrap();
        outline.update_time = "2026-09-29T10:00:00.000000Z".into();
        let pinned = state.chats.iter_mut().find(|c| c.id == "b-pinned").unwrap();
        pinned.archived = true;
        let returning = state
            .chats
            .iter_mut()
            .find(|c| c.id == "g-returning")
            .unwrap();
        returning.archived = false;
        state.chats.retain(|c| c.id != "c-archived");
    }
    let output = env.cmd().arg("sync").output().unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(": 1 new, 1 updated, 1 newly archived, 1 unarchived, 1 deleted. `sync --full` also drops chats deleted in the browser."),
        "{stderr}"
    );
    assert_eq!(
        ids(&env, &["list", "--all"]),
        [
            "f-new",
            "a-outline",
            "b-pinned",
            "d-project",
            "g-returning",
            "e-old"
        ]
    );
    assert_eq!(ids(&env, &["list", "--archived"]), ["b-pinned"]);
}

#[test]
fn full_sync_recovers_chats_the_lists_omit_and_drops_confirmed_deletions() {
    let env = Env::with_fake(chats());
    env.cmd().arg("sync").assert().success();
    {
        let mut state = env.fake().state();
        state.omitted_from_lists.insert("c-archived".into());
        state.chats.retain(|c| c.id != "e-old");
    }
    let output = env.cmd().args(["sync", "--full"]).output().unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "Recovered 1 chat(s) omitted from the conversation lists after individual checks."
        ),
        "{stderr}"
    );
    assert!(
        stderr.contains("Full sync: 4 chats (3 active, 1 archived), -1 vs before, in "),
        "{stderr}"
    );
    assert_eq!(
        ids(&env, &["list", "--all"]),
        ["a-outline", "b-pinned", "d-project", "c-archived"]
    );
}

#[test]
fn a_metadata_only_move_keeps_the_judgment_current() {
    let env = Env::with_fake(chats());
    let old = "2026-09-27T10:00:00.000000Z";
    env.cmd().arg("sync").assert().success();
    // The indexer caches the transcript the reconcile checks.
    assert_eq!(env.wait_for_indexer()["indexed"], 5);
    judge(
        &env.index_db(),
        "a-outline",
        old,
        &brainstorm_answers("writing_creativity"),
    );
    {
        let mut state = env.fake().state();
        let outline = state
            .chats
            .iter_mut()
            .find(|c| c.id == "a-outline")
            .unwrap();
        outline.update_time = "2026-09-28T10:00:00.000000Z".into();
        outline.gizmo_id = Some("g-p-books".into());
    }
    let output = env.cmd().arg("sync").output().unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr
            .contains("Preserved 1 unchanged cache(s); 0 content or metadata change(s) left stale"),
        "{stderr}"
    );
    let rows: Vec<Value> =
        serde_json::from_str(&env.stdout(&["list", "--json", "--limit", "1"])).unwrap();
    assert_eq!(rows[0]["project_id"], "g-p-books");
    assert_eq!(
        rows[0]["jev"]["brainstorm"], "writing",
        "the judgment stayed current"
    );

    // An edit to the text leaves it stale.
    {
        let mut state = env.fake().state();
        let outline = state
            .chats
            .iter_mut()
            .find(|c| c.id == "a-outline")
            .unwrap();
        outline.update_time = "2026-09-29T10:00:00.000000Z".into();
        outline.text = "Rewritten".into();
    }
    env.cmd().arg("sync").assert().success();
    let rows: Vec<Value> =
        serde_json::from_str(&env.stdout(&["list", "--json", "--limit", "1"])).unwrap();
    assert!(
        rows[0].get("jev").is_none(),
        "a changed chat's judgment is stale"
    );
}

#[test]
fn an_expired_token_is_exchanged_again_once() {
    let env = Env::with_fake(chats());
    env.cmd().arg("sync").assert().success();
    let exchanges = env.fake().state().session_exchanges;
    env.fake().state().expire_token = true;
    env.cmd().arg("sync").assert().success();
    assert_eq!(env.fake().state().session_exchanges, exchanges + 1);

    // Another browser choice reads the session again too.
    env.cmd()
        .args(["--browser", "chrome", "sync"])
        .assert()
        .success();
    assert_eq!(env.fake().state().session_exchanges, exchanges + 2);
}

#[test]
fn a_long_rate_limit_backs_off_and_status_shows_it() {
    let env = Env::with_fake(chats());
    env.cmd().arg("sync").assert().success();
    env.fake().state().rate_limit = Some((50, 600));
    let output = env.cmd().arg("sync").output().unwrap();
    assert_eq!(output.status.code(), Some(6));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("rate limited by ChatGPT"), "{stderr}");
    let status = env.status();
    let backoff = &status["backoff"];
    assert!(
        backoff["reason"].as_str().unwrap().contains("rate limited"),
        "{status}"
    );
    let left = backoff["until"].as_i64().unwrap() - chrono_now();
    assert!((500..=600).contains(&left), "backoff {left}s");
    // Lists still answer from the index meanwhile.
    assert_eq!(ids(&env, &["list"]).len(), 4);
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[test]
fn a_daemon_started_without_the_impit_variable_sets_it_and_syncs() {
    let env = Env::with_fake(chats());
    // The first command starts the daemon with this environment.
    env.cmd()
        .env_remove("IMPIT_H2_PSEUDOHEADERS_ORDER")
        .arg("sync")
        .assert()
        .success();
    assert_eq!(ids(&env, &["list", "--all"]).len(), 5);
}

#[test]
fn a_sync_hears_the_rate_limit_it_waits_on() {
    let env = Env::with_fake(chats());
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    env.fake().state().rate_limit = Some((1, 0));
    let output = env.cmd().arg("sync").output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains("rate limited by ChatGPT; waiting 0s"),
        "{stderr}"
    );
}

fn numbered(count: usize) -> Vec<Chat> {
    (0..count)
        .map(|n| {
            Chat::new(
                &format!("n-{n:03}"),
                &format!("Chat {n}"),
                &format!("2026-09-{:02}T10:{:02}:00.000000Z", 1 + n / 60, n % 60),
            )
        })
        .collect()
}

/// Single-chat reads the fake answered.
fn single_reads(env: &Env) -> usize {
    env.fake()
        .calls()
        .iter()
        .filter(|call| call.starts_with("GET /backend-api/conversation/"))
        .count()
}

fn indexed_count(env: &Env) -> i64 {
    env.index_db()
        .query_row("select count(*) from conversations", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn a_short_active_list_is_read_again_before_checking_chats_one_by_one() {
    let env = Env::with_fake(numbered(150));
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    let reads_before = single_reads(&env);
    // The next active listing ends after its first page of 100.
    env.fake().state().short_listings = 1;
    let output = env.cmd().args(["sync", "--full"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains(
            "The active list had 100 chat(s) where the index has 150; listing it again found 50 more."
        ),
        "{stderr}"
    );
    assert_eq!(single_reads(&env), reads_before, "no chat was read one by one");
    assert_eq!(indexed_count(&env), 150);
}

/// Background passes, full ones included, every half second once nobody
/// has used the CLI for half a second.
fn quick_schedule(chats: Vec<Chat>) -> Env {
    let mut env = Env::with_fake(chats);
    env.extra_env
        .push(("CHATGPT_TEST_SCHEDULE_MS".into(), "500".into()));
    env.cmd().arg("sync").assert().success();
    env
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn the_daily_full_sync_drops_a_chat_deleted_while_active() {
    let env = quick_schedule(chats());
    let first_full = env.status()["sync"]["last_full_at"].as_i64().unwrap();
    env.fake()
        .state()
        .chats
        .retain(|chat| chat.id != "e-old");
    // Only a full pass drops it; `daemon status` doesn't count as use.
    wait_until("the background full sync", || indexed_count(&env) == 4);
    let sync = env.status()["sync"].clone();
    assert!(sync["last_full_at"].as_i64().unwrap() >= first_full, "{sync}");
    assert!(sync["next_full_at"].as_i64().is_some(), "{sync}");
    assert!(
        env.stdout(&["daemon", "status"]).contains("full sync: "),
        "status shows it"
    );
}

#[test]
fn the_daily_full_sync_reads_only_a_few_left_out_chats_one_by_one() {
    let env = quick_schedule(numbered(60));
    env.wait_for_indexer();
    let reads_before = single_reads(&env);
    {
        let mut fake = env.fake().state();
        fake.omitted_from_lists = fake.chats.iter().map(|chat| chat.id.clone()).collect();
    }
    wait_until("a refused background full sync", || {
        env.status()["sync"]["last_error"]
            .as_str()
            .is_some_and(|error| error.contains("chatgpt sync --full"))
    });
    assert_eq!(single_reads(&env), reads_before, "nothing was read one by one");
    assert_eq!(indexed_count(&env), 60, "the index is unchanged");
}
