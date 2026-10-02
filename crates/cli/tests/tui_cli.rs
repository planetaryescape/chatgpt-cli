//! `chatgpt tui` in a pseudo-terminal, against the fake chatgpt.com through
//! a test daemon: filters, a transcript fetched after the debounce and
//! cached, copy, marks, the apply dialog (only `apply` applies, exactly the
//! marked chats, through `Mutate`), quitting with marks, and the terminal
//! given back on quit, Ctrl-C and a panic.

#![allow(clippy::unwrap_used)]

mod support;

use fake_chatgpt::Chat;
use support::{
    Env, Pty, delete_answers, fake_chat_states, indexed_update_time, judge, recording_tool,
};

const ROWS: u16 = 30;
const COLS: u16 = 140;
/// Leaving the alternate screen, and showing the cursor again.
const RESTORED: [&str; 2] = ["\x1b[?1049l", "\x1b[?25h"];

fn chats() -> Vec<Chat> {
    vec![
        Chat::new("a-one", "One", "2026-09-27T10:00:00.000000Z"),
        Chat::new("b-two", "Two", "2026-09-26T10:00:00.000000Z"),
        Chat::new("c-three", "Three", "2026-09-25T10:00:00.000000Z"),
    ]
}

/// Synced and indexed, Jev saying delete for `a-one`, and `b-two`'s
/// transcript not cached.
fn synced() -> Env {
    let env = Env::with_fake(chats());
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    let db = env.index_db();
    let update_time = indexed_update_time(&env, "a-one");
    judge(
        &db,
        "a-one",
        &update_time,
        &delete_answers("coding_general"),
    );
    db.execute("delete from transcripts where id = 'b-two'", [])
        .unwrap();
    env.fake().state().batch_bodies.clear();
    env
}

#[test]
fn browse_filter_preview_copy_mark_and_quit_without_applying() {
    let env = synced();
    let pbcopy = recording_tool(&env, "pbcopy");
    let before = fake_chat_states(&env);
    let mut pty = Pty::spawn(&env, &["tui"], None, ROWS, COLS);
    let screen = pty.wait_for_screen("Hello from a-one");
    assert!(
        screen.contains("chatgpt · 3 of 3 · active · all · all topics"),
        "{screen}"
    );
    assert!(screen.contains("Jev: DELETE · coding_general"), "{screen}");

    pty.send("2");
    pty.wait_for_screen("chatgpt · 1 of 3 · active · delete · all topics");
    pty.send("1/thr");
    pty.wait_for_screen("1 of 3 · active · all · all topics · \"thr\"");
    pty.send("\x1b");
    pty.wait_for_screen("3 of 3 · active · all · all topics ");

    // Not cached: fetched through the batch endpoint after the pause, and
    // cached for next time.
    pty.send("j");
    pty.wait_for_screen("Hello from b-two");
    assert_eq!(env.fake().state().batch_bodies, [vec!["b-two".to_owned()]]);
    let cached: i64 = env
        .index_db()
        .query_row(
            "select count(*) from transcripts where id = 'b-two'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(cached, 1);

    pty.send("c");
    pty.wait_for_screen("Copied \"Two\" (0 KB).");
    assert!(
        std::fs::read_to_string(&pbcopy)
            .unwrap()
            .starts_with("args:\n# Two\n\nhttps://chatgpt.com/c/b-two")
    );

    pty.send("ka");
    pty.wait_for_screen("marked: 0 delete, 1 archive (x to apply)");
    pty.send("x");
    pty.wait_for_screen("Archive 1 and permanently delete 0 conversation(s).");
    pty.send("nope\r");
    pty.wait_for_screen("Not applied: type apply exactly.");
    pty.send("x");
    pty.wait_for_screen("Type apply and press enter. Esc cancels.");
    pty.send("\x1b");
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(!pty.screen().contains("Type apply and press enter"));

    pty.send("q");
    pty.wait_for_screen("1 unapplied mark(s). Press q again to quit without applying.");
    pty.send("q");
    assert_eq!(pty.finish(), Some(0));
    assert_eq!(fake_chat_states(&env), before, "nothing changed in ChatGPT");
}

#[test]
fn apply_changes_exactly_the_marked_chats() {
    let env = synced();
    let mut pty = Pty::spawn(&env, &["tui"], None, ROWS, COLS);
    pty.wait_for_screen("Hello from a-one");
    // Jev's delete, accepted; then an archive mark on Two.
    pty.send("\r");
    pty.wait_for_screen("marked: 1 delete, 0 archive");
    pty.send("a");
    pty.wait_for_screen("marked: 1 delete, 1 archive");
    pty.send("x");
    pty.wait_for_screen("Archive 1 and permanently delete 1 conversation(s).");
    pty.send("apply\r");
    let screen = pty.wait_for_screen("Applied 2 change(s).");
    assert!(!screen.contains("marked:"), "{screen}");
    pty.wait_for_screen("chatgpt · 1 of 2 · active");
    pty.send("q");
    assert_eq!(pty.finish(), Some(0));
    assert_eq!(
        fake_chat_states(&env),
        [("b-two".to_owned(), true), ("c-three".to_owned(), false)]
    );
}

#[test]
fn a_local_title_goes_to_the_index_and_a_refused_one_says_why() {
    let env = synced();
    let mut pty = Pty::spawn(&env, &["tui"], None, ROWS, COLS);
    pty.wait_for_screen("Hello from a-one");
    pty.send("n");
    pty.wait_for_screen("Edit local title");
    // Cleared: the daemon refuses an empty title, and the box stays.
    pty.send("\x01\x0b\r");
    pty.wait_for_screen("Local title must be 1–100 characters.");
    pty.send("Renamed here\r");
    pty.wait_for_screen("Local title saved.");
    pty.wait_for_screen("Renamed here");
    pty.send("q");
    assert_eq!(pty.finish(), Some(0));
    assert_eq!(
        env.stdout(&["list", "--title", "Renamed", "--format", "ids"]),
        "a-one\n"
    );
    assert_eq!(
        env.fake().state().chats[0].title,
        "One",
        "ChatGPT's title stays"
    );
}

#[test]
fn the_terminal_is_given_back_on_quit_ctrl_c_and_a_panic() {
    let mut env = synced();
    let mut pty = Pty::spawn(&env, &["tui"], None, ROWS, COLS);
    pty.wait_for_screen("3 of 3");
    // Ctrl-C quits at once, marks and all.
    pty.send("d\x03");
    let (code, text) = pty.finish_keeping_text();
    assert_eq!(code, Some(0));
    for sequence in RESTORED {
        assert!(text.contains(sequence), "{sequence:?}");
    }

    env.extra_env
        .push(("CHATGPT_TEST_TUI_PANIC".into(), "z".into()));
    let mut pty = Pty::spawn(&env, &["tui"], None, ROWS, COLS);
    pty.wait_for_screen("3 of 3");
    pty.send("z");
    let (code, text) = pty.finish_keeping_text();
    assert_eq!(code, Some(101));
    let panicked = text.find("CHATGPT_TEST_TUI_PANIC").unwrap();
    for sequence in RESTORED {
        let restored = text.find(sequence).unwrap();
        assert!(
            restored < panicked,
            "restored before the message: {sequence:?}"
        );
    }
}

#[test]
fn no_index_says_so_before_taking_the_screen() {
    let env = Env::new();
    let mut pty = Pty::spawn(&env, &["tui"], None, ROWS, COLS);
    let (code, text) = {
        pty.wait_for("No local index yet. Run `chatgpt sync` first.");
        pty.finish_keeping_text()
    };
    assert_eq!(code, Some(1));
    assert!(!text.contains("\x1b[?1049h"), "never took the screen");
}

/// SIGTERM or SIGHUP: the TUI leaves the screen, puts the terminal back
/// (echo and line editing), and exits 128 + the signal.
#[test]
fn a_signal_gives_the_terminal_back() {
    let env = synced();
    for (signal, status) in [("TERM", 143), ("HUP", 129)] {
        let mut pty = Pty::spawn_then(
            &env,
            &["tui"],
            ROWS,
            COLS,
            "echo \"exit:$?\"; stty -a; echo end-of-stty",
        );
        pty.wait_for_screen("3 of 3");
        // script → sh → chatgpt.
        let shell = children(pty.pid());
        let tui = *children(*shell.first().unwrap()).first().unwrap();
        std::process::Command::new("kill")
            .args([&format!("-{signal}"), &tui.to_string()])
            .status()
            .unwrap();
        pty.wait_for(&format!("exit:{status}"));
        let settings = pty.wait_for("end-of-stty");
        assert!(
            !settings.contains("-icanon") && !settings.contains("-echo "),
            "SIG{signal} left the terminal raw:\n{settings}"
        );
        let (_, text) = pty.finish_keeping_text();
        assert!(text.contains(RESTORED[0]), "left the alternate screen");
    }
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
