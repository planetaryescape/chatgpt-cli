//! The bridge: unported commands reach the TS CLI with their arguments,
//! stdin, stdout, stderr and exit code untouched. A shell script stands in
//! for the TS CLI (`CHATGPT_BUN=/bin/sh`).

#![allow(clippy::unwrap_used)]

mod support;

use support::Env;

const ECHO_CLI: &str = r#"printf 'argv:'
for arg in "$@"; do printf ' [%s]' "$arg"; done
printf '\n'
printf 'stdin:%s\n' "$(cat)"
printf 'bridged:%s\n' "$CHATGPT_BRIDGED"
echo 'to stderr' >&2
exit 3
"#;

#[test]
fn an_unported_command_runs_in_the_ts_cli_unchanged() {
    let mut env = Env::new();
    env.fake_ts_cli(ECHO_CLI);
    let output = env
        .cmd()
        .args(["--browser", "dia", "rename", "abc def", "-o", "--copy"])
        .write_stdin("piped ids")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3), "the TS CLI's exit code");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "argv: [--browser] [dia] [rename] [abc def] [-o] [--copy]\nstdin:piped ids\nbridged:1\n"
    );
    assert_eq!(String::from_utf8_lossy(&output.stderr), "to stderr\n");
}

#[test]
fn no_search_goes_to_the_ts_cli() {
    let mut env = Env::new();
    env.fake_ts_cli(ECHO_CLI);
    for args in [
        &["search", "kids", "--semantic", "--limit", "5"][..],
        &["search", "kids", "--hybrid"],
        &["search-index", "--all"],
    ] {
        let output = env.cmd().args(args).output().unwrap();
        assert!(output.stdout.is_empty(), "{args:?} was bridged");
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            "error: No local index yet. Run `chatgpt sync` first.\n",
            "{args:?}"
        );
    }
    // --remote needs no index; here it has no session to read either.
    let output = env
        .cmd()
        .args(["search", "kids", "--remote"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "--remote was bridged");
}

#[test]
fn an_unported_commands_help_comes_from_the_ts_cli() {
    let mut env = Env::new();
    env.fake_ts_cli(ECHO_CLI);
    let output = env.cmd().args(["classify", "--help"]).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).lines().next(),
        Some("argv: [classify] [--help]")
    );
}

#[test]
fn top_level_help_lists_native_and_bridged_commands() {
    let env = Env::new();
    let help = env.stdout(&["--help"]);
    for command in [
        "sync", "list", "stats", "daemon", "export", "classify", "tui",
    ] {
        assert!(help.contains(command), "{command} missing from:\n{help}");
    }
    assert_eq!(
        env.stdout(&["--version"]),
        format!("chatgpt {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn the_bridge_refuses_to_loop_and_says_when_the_ts_cli_is_missing() {
    let mut env = Env::new();
    let missing = env.cmd().arg("rename").output().unwrap();
    assert_eq!(missing.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("TS chatgpt CLI, which isn't installed"),
        "{}",
        String::from_utf8_lossy(&missing.stderr)
    );

    env.fake_ts_cli(ECHO_CLI);
    let looped = env
        .cmd()
        .arg("rename")
        .env("CHATGPT_BRIDGED", "1")
        .output()
        .unwrap();
    assert_eq!(looped.status.code(), Some(1));
    assert!(looped.stdout.is_empty());
    assert!(String::from_utf8_lossy(&looped.stderr).contains("ran this chatgpt again"));
}
