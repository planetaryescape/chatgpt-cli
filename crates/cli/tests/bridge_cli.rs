//! Nothing reaches the TS CLI any more: every command, `review` and `tui`
//! included, runs here, and an unknown one is this CLI's error. A shell
//! script stands in for the TS CLI (`CHATGPT_BUN=/bin/sh`) and records
//! any run.

#![allow(clippy::unwrap_used)]

mod support;

use support::Env;

/// The TS CLI stand-in: it records each run's arguments.
fn recording_ts_cli(env: &mut Env) -> std::path::PathBuf {
    let calls = env.home.path().join("ts-cli-calls");
    env.fake_ts_cli(&format!(
        "printf '%s\\n' \"$*\" >> '{}'\nexit 3\n",
        calls.display()
    ));
    calls
}

/// Runs other than the daemon's own TS sync, which runs while the TS CLI
/// exists (and is deleted with it).
fn bridged_runs(calls: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(calls)
        .unwrap_or_default()
        .lines()
        .filter(|line| *line != "sync")
        .map(str::to_owned)
        .collect()
}

#[test]
fn no_command_goes_to_the_ts_cli() {
    let mut env = Env::new();
    let calls = recording_ts_cli(&mut env);
    for args in [
        &["review", "--suggest", "delete"][..],
        &["review", "--help"],
        &["tui", "--help"],
        &["help", "review"],
        &["frobnicate"],
        &["--frobnicate", "list"],
        &["search", "kids", "--semantic", "--limit", "5"],
        &["classify"],
    ] {
        let output = env.cmd().args(args).write_stdin("").output().unwrap();
        assert_ne!(output.status.code(), Some(3), "{args:?} was bridged");
        assert!(bridged_runs(&calls).is_empty(), "{args:?} ran the TS CLI");
    }
    let unknown = env.cmd().arg("frobnicate").output().unwrap();
    assert!(
        String::from_utf8_lossy(&unknown.stderr).contains("unrecognized subcommand 'frobnicate'"),
        "{}",
        String::from_utf8_lossy(&unknown.stderr)
    );
    let review = env.cmd().arg("review").output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&review.stderr),
        "error: No local index yet. Run `chatgpt sync` first.\n"
    );
}

#[test]
fn review_and_tui_have_their_own_help() {
    let env = Env::new();
    let review = env.stdout(&["review", "--help"]);
    for flag in ["--oldest-first", "--pinned", "--suggest", "--older-than"] {
        assert!(review.contains(flag), "{flag} missing from:\n{review}");
    }
    assert!(env.stdout(&["tui", "--help"]).contains("terminal UI"));
}

#[test]
fn top_level_help_lists_every_command() {
    let env = Env::new();
    let help = env.stdout(&["--help"]);
    for command in [
        "sync",
        "list",
        "stats",
        "daemon",
        "export",
        "archive",
        "project",
        "memory",
        "classify",
        "titles",
        "configure",
        "review",
        "tui",
    ] {
        assert!(help.contains(command), "{command} missing from:\n{help}");
    }
    assert!(!help.contains("TS chatgpt CLI"), "{help}");
    assert_eq!(
        env.stdout(&["--version"]),
        format!("chatgpt {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn the_tui_needs_a_terminal() {
    let env = Env::new();
    let output = env.cmd().arg("tui").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "error: chatgpt tui needs a terminal\n"
    );
}
