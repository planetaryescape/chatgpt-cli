//! Regressions from the stage 1 review: one account per pass, no network
//! on a cold start within the sync interval, and a rate limit that ends the
//! pass.

#![allow(clippy::unwrap_used)]

mod support;

use std::process::Stdio;
use std::time::Duration;

use fake_chatgpt::{Chat, OTHER_COOKIE};
use support::{Env, delete_answers, judge};

fn chats() -> Vec<Chat> {
    vec![
        Chat::new("a-one", "One", "2026-09-27T10:00:00.000000Z"),
        Chat::new("b-two", "Two", "2026-09-26T10:00:00.000000Z"),
        Chat::new("c-three", "Three", "2026-09-25T10:00:00.000000Z"),
    ]
}

fn ids(env: &Env, args: &[&str]) -> Vec<String> {
    let mut args = args.to_vec();
    args.extend(["--format", "ids"]);
    env.stdout(&args).lines().map(str::to_owned).collect()
}

/// `--browser chrome` reads the second account in these tests.
fn two_accounts() -> Env {
    let mut env = Env::with_fake(chats());
    env.extra_env
        .push(("CHATGPT_TEST_COOKIE_CHROME".into(), OTHER_COOKIE.into()));
    env.fake().state().other_account_chats = vec![Chat::new(
        "z-other",
        "Other account",
        "2026-09-30T10:00:00.000000Z",
    )];
    env
}

#[test]
fn another_browser_chosen_mid_pass_never_changes_what_the_pass_reads() {
    let env = two_accounts();
    env.cmd().arg("sync").assert().success();
    env.fake().state().list_delay_ms = 400;
    let sync = env
        .std_cmd()
        .args(["sync", "--full"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    // Reads the other account's saved memories, mid-pass.
    let _ = env
        .cmd()
        .args(["--browser", "chrome", "stats"])
        .output()
        .unwrap();
    let synced = sync.wait_with_output().unwrap();
    assert!(
        synced.status.success(),
        "{}",
        String::from_utf8_lossy(&synced.stderr)
    );
    assert_eq!(
        ids(&env, &["list", "--all"]),
        ["a-one", "b-two", "c-three"],
        "no chat of the first account was dropped"
    );
}

#[test]
fn a_sync_from_another_account_is_refused() {
    let env = two_accounts();
    env.cmd().arg("sync").assert().success();
    let refused = env
        .cmd()
        .args(["--browser", "chrome", "sync"])
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("another ChatGPT account"), "{stderr}");
    assert_eq!(ids(&env, &["list", "--all"]), ["a-one", "b-two", "c-three"]);
}

#[test]
fn a_cold_start_with_a_synced_index_waits_for_the_cadence() {
    let env = Env::with_fake(chats());
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    env.cmd().args(["daemon", "stop"]).assert().success();
    let before = env.fake().calls().len();
    assert_eq!(ids(&env, &["list"]).len(), 3);
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(
        env.fake().calls().len(),
        before,
        "the restarted daemon synced before the cadence was due"
    );
    let status = env.status();
    let next = status["sync"]["next_at"].as_i64().unwrap();
    let now = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap();
    assert!(next - now > 60, "next pass in {}s", next - now);
}

#[test]
fn a_rate_limit_during_the_reconcile_ends_the_pass() {
    let env = Env::with_fake(chats());
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    // A cached transcript for an older update time makes the reconcile
    // read the chat through the batch endpoint.
    env.index_db()
        .execute(
            "insert or replace into transcripts values ('a-one', '2026-09-01T00:00:00Z', 2, '# One', 1, 1)",
            [],
        )
        .unwrap();
    env.fake().state().rate_limit_batch = Some((50, 600));
    let synced = env.cmd().arg("sync").output().unwrap();
    assert_eq!(
        synced.status.code(),
        Some(6),
        "{}",
        String::from_utf8_lossy(&synced.stderr)
    );
    let status = env.status();
    assert!(status["backoff"]["until"].is_i64(), "{status}");
    assert!(status["sync"]["last_error"].is_string(), "{status}");
}

#[test]
fn a_listing_that_repeats_chats_is_read_again_so_none_is_skipped() {
    // More than one page, so the second page can shift.
    let chats: Vec<Chat> = (0..105)
        .map(|n| {
            Chat::new(
                &format!("chat-{n:03}"),
                "Chat",
                &format!("2026-09-{:02}T10:{:02}:00.000000Z", 1 + n / 60, n % 60),
            )
        })
        .collect();
    let env = Env::with_fake(chats);
    // The first listing of an empty index, with nothing indexed to recover
    // skipped chats from.
    env.fake().state().flaky_listings = 1;
    let synced = env.cmd().arg("sync").output().unwrap();
    assert!(
        synced.status.success(),
        "{}",
        String::from_utf8_lossy(&synced.stderr)
    );
    assert_eq!(env.stdout(&["list", "--all", "--count"]), "105\n");
}

#[test]
fn a_session_that_doesnt_name_its_account_is_refused_for_an_index_that_has_one() {
    let env = Env::with_fake(chats());
    env.cmd().arg("sync").assert().success();
    env.cmd().args(["daemon", "stop"]).assert().success();
    env.fake().state().omit_user_id = true;
    let refused = env.cmd().arg("sync").output().unwrap();
    assert_eq!(refused.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("doesn't say which ChatGPT account"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );

    // An index that never stored an account still syncs, as before.
    let fresh = Env::with_fake(chats());
    fresh.fake().state().omit_user_id = true;
    fresh.cmd().arg("sync").assert().success();
    assert_eq!(ids(&fresh, &["list"]).len(), 3);
}

#[test]
fn a_judgment_without_a_topic_has_none_and_fails_nothing() {
    let env = Env::with_fake(chats());
    env.cmd().arg("sync").assert().success();
    let db = env.index_db();
    let mut answers: serde_json::Value = serde_json::from_str(&delete_answers("other")).unwrap();
    answers.as_object_mut().unwrap().remove("topic");
    judge(
        &db,
        "a-one",
        "2026-09-27T10:00:00.000000Z",
        &answers.to_string(),
    );
    drop(db);
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(&env.stdout(&["list", "--json"])).unwrap();
    assert_eq!(rows[0]["topic"], serde_json::Value::Null);
    assert_eq!(rows[0]["jev"]["suggestion"], "delete");
    assert!(
        env.stdout(&["list"])
            .starts_with("a-one  2026-09-27       delete                        ")
    );
    assert!(
        env.stdout(&["stats"])
            .starts_with("3 chat(s), 1 judged, 2 not yet judged")
    );
    assert_eq!(ids(&env, &["list", "--topic", "other"]).len(), 0);
}

#[test]
fn stats_never_counts_another_accounts_saved_memories() {
    let env = two_accounts();
    env.cmd().arg("sync").assert().success();
    let output = env
        .cmd()
        .args(["--browser", "chrome", "stats"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Saved-memory stats unavailable: this index holds another ChatGPT account"),
        "{stderr}"
    );
    // The chat counts are this index's, as before.
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("3 chat(s)"));
}
