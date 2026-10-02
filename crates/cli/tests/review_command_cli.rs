//! `review`, natively, in a pseudo-terminal: one key per chat (keep,
//! archive, delete, accept, undo, view, open, quit), and nothing changes in
//! ChatGPT until `apply` is typed at the end.

#![allow(clippy::unwrap_used)]

mod support;

use fake_chatgpt::Chat;
use support::{
    Env, Pty, delete_answers, fake_chat_states, indexed_update_time, judge, recording_tool,
};

fn chats() -> Vec<Chat> {
    vec![
        Chat::new("a-one", "One", "2026-09-27T10:00:00.000000Z"),
        Chat::new("b-two", "Two", "2026-09-26T10:00:00.000000Z"),
        Chat::new("c-three", "Three", "2026-09-25T10:00:00.000000Z"),
    ]
}

/// Synced, with Jev saying delete for `a-one`.
fn synced() -> Env {
    let env = Env::with_fake(chats());
    env.cmd().arg("sync").assert().success();
    let update_time = indexed_update_time(&env, "a-one");
    judge(
        &env.index_db(),
        "a-one",
        &update_time,
        &delete_answers("coding_general"),
    );
    env
}

/// Keep, archive and delete, with an undo on the way: the summary, the
/// list of changes, and the question at the end.
fn triage_three(pty: &mut Pty) {
    let first = pty.wait_for("[1/3]  One");
    assert!(first.contains("\x1b[H\x1b[2J"), "the screen is cleared");
    let shown = pty.wait_for("[enter] accept suggestion  [k]eep");
    assert!(
        shown.contains("Jev suggests: DELETE · coding_general\n"),
        "{shown}"
    );
    assert!(
        shown.contains("1 turns\n\nYou: Hello from a-one\n"),
        "{shown}"
    );
    pty.send("a");
    pty.wait_for("[2/3]  Two");
    let shown =
        pty.wait_for("[k]eep  [a]rchive  [d]elete  [v]iew  [o]pen in browser  [u]ndo  [q]uit");
    assert!(!shown.contains("suggests"), "no verdict for Two: {shown}");
    pty.send("x");
    pty.send("d");
    pty.wait_for("[3/3]  Three");
    pty.send("u");
    pty.wait_for("[2/3]  Two");
    pty.send(" ");
    pty.wait_for("[3/3]  Three");
    pty.send("d");
    let summary = pty.wait_for("Type apply to carry these out: ");
    assert!(
        summary.contains(
            "Reviewed 3: keep 1, archive 1, delete 1.\ndelete   Three\narchive  One\nType apply"
        ),
        "{summary}"
    );
}

#[test]
fn anything_but_apply_changes_nothing() {
    let env = synced();
    let before = fake_chat_states(&env);
    let mut pty = Pty::spawn(&env, &["review"], None, 40, 120);
    triage_three(&mut pty);
    pty.send("yes\n");
    pty.wait_for("Nothing changed.");
    assert_eq!(pty.finish(), Some(0));
    assert_eq!(fake_chat_states(&env), before);
}

#[test]
fn apply_archives_and_deletes_what_was_decided() {
    let env = synced();
    let mut pty = Pty::spawn(&env, &["review"], None, 40, 120);
    triage_three(&mut pty);
    pty.send("apply\n");
    let done = pty.wait_for("Deleted 1 conversation(s)");
    assert!(done.contains("Archived 1 conversation(s)"), "{done}");
    assert_eq!(pty.finish(), Some(0));
    assert_eq!(
        fake_chat_states(&env),
        [("a-one".to_owned(), true), ("b-two".to_owned(), false)]
    );
    // The index followed.
    let ids = env.stdout(&["list", "--all", "--format", "ids"]);
    assert_eq!(ids, "a-one\nb-two\n");
}

#[test]
fn enter_accepts_jevs_suggestion_and_q_stops_early() {
    let env = synced();
    let mut pty = Pty::spawn(&env, &["review", "--oldest-first"], None, 40, 120);
    pty.wait_for("[1/3]  Three");
    // No suggestion to accept: enter does nothing.
    pty.send("\r");
    pty.send("k");
    pty.wait_for("[2/3]  Two");
    pty.send("k");
    pty.wait_for("[3/3]  One");
    pty.send("\r");
    let summary = pty.wait_for("Type apply to carry these out: ");
    assert!(
        summary.contains("Reviewed 3: keep 2, archive 0, delete 1.\ndelete   One\n"),
        "{summary}"
    );
    pty.send("apply\n");
    pty.wait_for("Deleted 1 conversation(s)");
    assert_eq!(pty.finish(), Some(0));
    assert!(fake_chat_states(&env).iter().all(|(id, _)| id != "a-one"));

    let mut pty = Pty::spawn(&env, &["review"], None, 40, 120);
    pty.wait_for("[1/2]  Two");
    pty.send("q");
    pty.wait_for("Reviewed 0: keep 0, archive 0, delete 0.");
    // Nothing to apply: no question.
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(!pty.text().contains("Type apply"));
    assert_eq!(pty.finish(), Some(0));
}

#[test]
fn ids_on_stdin_and_keys_from_the_terminal() {
    let env = synced();
    let mut pty = Pty::spawn(
        &env,
        &["review", "-"],
        Some("b-two  2026-09-26  Two"),
        40,
        120,
    );
    pty.wait_for("[1/1]  Two");
    pty.send("a");
    pty.wait_for("Type apply to carry these out: ");
    pty.send("apply\n");
    pty.wait_for("Archived 1 conversation(s)");
    assert_eq!(pty.finish(), Some(0));
    assert!(fake_chat_states(&env).contains(&("b-two".to_owned(), true)));
}

#[test]
fn view_pages_the_transcript_and_open_hands_the_link_to_the_browser() {
    let mut env = synced();
    let pager = recording_tool(&env, "pager");
    let open = recording_tool(&env, "open");
    env.extra_env.push((
        "PAGER".into(),
        env.tools.join("pager").display().to_string(),
    ));
    let mut pty = Pty::spawn(&env, &["review", "a-one"], None, 40, 120);
    pty.wait_for("[1/1]  One");
    pty.send("v");
    // Shown again after the pager.
    pty.wait_for("[1/1]  One");
    pty.send("o");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !std::fs::read_to_string(&open).is_ok_and(|log| log.ends_with('\n'))
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    pty.send("q");
    pty.wait_for("Reviewed 0");
    assert_eq!(pty.finish(), Some(0));
    let paged = std::fs::read_to_string(pager).unwrap();
    assert!(
        paged.starts_with(
            "args:-R\n# One\n\nhttps://chatgpt.com/c/a-one · 2026-01-01 · unknown model\n\n---\n\n## Me\n\nHello from a-one\n"
        ),
        "{paged}"
    );
    assert_eq!(
        std::fs::read_to_string(open).unwrap(),
        "args:https://chatgpt.com/c/a-one\n"
    );
}

#[test]
fn a_chat_chatgpt_no_longer_has_says_why_and_can_still_be_decided() {
    let env = synced();
    env.fake().state().chats.retain(|chat| chat.id != "b-two");
    let mut pty = Pty::spawn(&env, &["review", "b-two"], None, 40, 120);
    pty.wait_for("(could not load: ChatGPT didn't return this conversation (deleted?))");
    pty.send("k");
    pty.wait_for("Reviewed 1: keep 1, archive 0, delete 0.");
    assert_eq!(pty.finish(), Some(0));
}

#[test]
fn nothing_matched_needs_no_terminal() {
    let env = synced();
    let output = env
        .cmd()
        .args(["review", "--title", "nothing like this"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "Nothing matched.\n"
    );
}

/// A transcript many frames long reaches the pager whole, byte for byte
/// as the cache holds it. A lowered frame cap (debug builds only) stands in
/// for 16 MiB.
#[test]
fn a_transcript_larger_than_a_frame_streams_whole() {
    let mut big = Chat::new("d-big", "Huge", "2026-09-28T10:00:00.000000Z");
    big.text = "word café 😀 naïve ".repeat(15_000);
    let mut all = chats();
    all.push(big);
    let mut env = Env::with_fake(all);
    env.extra_env
        .push(("CHATGPT_TEST_MAX_FRAME_BYTES".into(), "50000".into()));
    let pager = recording_tool(&env, "pager");
    env.extra_env.push((
        "PAGER".into(),
        env.tools.join("pager").display().to_string(),
    ));
    env.cmd().arg("sync").assert().success();
    let mut pty = Pty::spawn(&env, &["review", "d-big"], None, 40, 120);
    pty.wait_for("[1/1]  Huge");
    pty.send("v");
    pty.wait_for("[1/1]  Huge");
    pty.send("q");
    assert_eq!(pty.finish(), Some(0));
    let cached: String = env
        .index_db()
        .query_row(
            "select markdown from transcripts where id = 'd-big'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(cached.len() > 300_000);
    assert_eq!(
        std::fs::read_to_string(pager).unwrap(),
        format!("args:-R\n{cached}")
    );
}

/// A fetch a sync overtook: the chat moved on while its batch read was out.
/// The chat is still shown, but its older revision isn't cached over the
/// newer one.
#[test]
fn a_fetch_overtaken_by_a_sync_is_shown_but_not_cached() {
    let env = synced();
    env.wait_for_indexer();
    let before = indexed_update_time(&env, "b-two");
    env.index_db()
        .execute("delete from transcripts where id = 'b-two'", [])
        .unwrap();
    env.fake().state().batch_delay_ms = 3000;
    let mut pty = Pty::spawn(&env, &["review", "b-two"], None, 40, 120);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !env
        .fake()
        .state()
        .batch_bodies
        .iter()
        .any(|ids| ids == &["b-two".to_owned()])
    {
        assert!(std::time::Instant::now() < deadline, "never fetched b-two");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // While that read is out: the chat changes, and a sync records it.
    {
        let mut fake = env.fake().state();
        fake.batch_delay_ms = 0;
        // Later batch reads (the indexer's) fail, so only the review's
        // fetch could cache b-two.
        fake.fail_batch = 1000;
        let chat = fake
            .chats
            .iter_mut()
            .find(|chat| chat.id == "b-two")
            .unwrap();
        chat.update_time = "2026-09-29T10:00:00.000000Z".into();
    }
    env.cmd().arg("sync").output().unwrap();
    assert_ne!(
        indexed_update_time(&env, "b-two"),
        before,
        "the sync moved it"
    );
    let shown = pty.wait_for("[k]eep");
    assert!(shown.contains("You: Hello from b-two"), "{shown}");
    pty.send("q");
    assert_eq!(pty.finish(), Some(0));
    let cached: i64 = env
        .index_db()
        .query_row(
            "select count(*) from transcripts where id = 'b-two' and update_time = ?",
            [&before],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(cached, 0, "the overtaken fetch was cached");
}

/// The children of process `pid`.
fn children(pid: u32) -> Vec<u32> {
    let output = std::process::Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

/// SIGTERM or SIGHUP while waiting for a key: the terminal is cooked again
/// (echo and line editing back) and the exit status is 128 + the signal.
#[test]
fn a_signal_while_waiting_for_a_key_gives_the_terminal_back() {
    let env = synced();
    for (signal, status) in [("TERM", 143), ("HUP", 129)] {
        let mut pty = Pty::spawn_then(
            &env,
            &["review", "b-two"],
            40,
            120,
            "echo \"exit:$?\"; stty -a; echo end-of-stty",
        );
        pty.wait_for("[k]eep");
        // script → sh → chatgpt.
        let shell = children(pty.pid());
        let review = children(*shell.first().unwrap());
        let review = *review.first().unwrap();
        std::process::Command::new("kill")
            .args([&format!("-{signal}"), &review.to_string()])
            .status()
            .unwrap();
        pty.wait_for(&format!("exit:{status}"));
        let settings = pty.wait_for("end-of-stty");
        assert!(
            !settings.contains("-icanon") && !settings.contains("-echo "),
            "SIG{signal} left the terminal raw:\n{settings}"
        );
        pty.finish();
    }
}
