//! `docs/reference/cli.md` is generated from the binary's own `--help`
//! output, so the reference can't drift from the code. This test fails when
//! the file on disk is stale; `CHATGPT_UPDATE_CLI_REFERENCE=1 cargo nextest
//! run -p chatgpt-cli --test cli_reference` rewrites it.
//!
//! Piped stdout isn't a terminal and clap doesn't wrap help text here, so
//! the output is the same on every machine.

#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

/// In the order the docs introduce them, not the CLI's registration order.
const COMMANDS: &[&str] = &[
    "configure",
    "sync",
    "list",
    "stats",
    "search",
    "search-index",
    "memory",
    "memory list",
    "memory classify",
    "memory summary",
    "memory delete",
    "project",
    "project create",
    "project list",
    "project add",
    "project remove",
    "project delete",
    "export",
    "classify",
    "titles",
    "title",
    "review",
    "tui",
    "archive",
    "unarchive",
    "delete",
    "rename",
    "daemon",
    "daemon status",
    "daemon stop",
    "daemon logs",
    "daemon install",
    "daemon uninstall",
    "daemon run",
];

const UPDATE_ENV: &str = "CHATGPT_UPDATE_CLI_REFERENCE";

fn reference_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/reference/cli.md")
}

fn help(args: &[&str]) -> String {
    let output = std::process::Command::new(assert_cmd::cargo::cargo_bin!("chatgpt"))
        .args(args)
        .arg("--help")
        .env_remove("COLUMNS")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "chatgpt {} --help failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .trim_end()
        .to_owned()
}

fn render() -> String {
    let sections: Vec<String> = COMMANDS
        .iter()
        .map(|command| {
            let args: Vec<&str> = command.split(' ').collect();
            format!("## `chatgpt {command}`\n\n```text\n{}\n```", help(&args))
        })
        .collect();
    format!(
        "<!-- Generated from the binary's --help output by crates/cli/tests/cli_reference.rs. Edit the command definitions in crates/cli/src/args.rs, then run `{UPDATE_ENV}=1 cargo nextest run -p chatgpt-cli --test cli_reference`. -->

# CLI reference

Every command and flag, taken from `chatgpt <command> --help`.

Shared behaviour:

- **Filters.** `--older-than`, `--newer-than`, `--before`, `--after`, `--title`, `--archived`, `--all`, `--limit`, `--suggest`, `--topic` and `--brainstorm` work the same on every command that lists them. They read the local index, which the daemon keeps fresh.
- **Targets.** Commands that take `[ids...]` accept full ids, unique id prefixes, or `-` to read ids from stdin (the first column of `list` output works). A repeated id counts once.
- **Pinned chats.** Bulk commands skip pinned chats unless you pass `--pinned`.
- **Recommendations.** Preview with `archive --suggest archive --dry-run` or `delete --suggest delete --dry-run`, then repeat without `--dry-run` to apply. Both commands skip unsure `?` chats. Deletion sends up to three requests at a time.
- **Output.** Results go to stdout; progress, prompts and errors go to stderr, so piping `list` into another command stays clean.
- **Exit codes.** `0` on success. `2` for invalid arguments or input (an unknown command or flag included), `4` when no browser session can be read, `6` when ChatGPT's rate limit holds, `7` when this build or platform can't do it, and `1` for any other failure, including any item in a bulk action failing.

## `chatgpt`

```text
{}
```

{}
",
        help(&[]),
        sections.join("\n\n")
    )
}

#[test]
fn the_cli_reference_matches_the_help_output() {
    let rendered = render();
    if std::env::var_os(UPDATE_ENV).is_some() {
        std::fs::write(reference_path(), &rendered).unwrap();
        return;
    }
    let on_disk = std::fs::read_to_string(reference_path()).unwrap();
    assert!(
        on_disk == rendered,
        "docs/reference/cli.md is stale: run `{UPDATE_ENV}=1 cargo nextest run -p chatgpt-cli --test cli_reference` and commit the result"
    );
}

/// Every command `chatgpt --help` lists is in the reference.
#[test]
fn the_reference_covers_every_command() {
    for (parent, prefix) in [
        (&[][..], ""),
        (&["memory"][..], "memory "),
        (&["project"][..], "project "),
        (&["daemon"][..], "daemon "),
    ] {
        let listed: Vec<String> = help(parent)
            .lines()
            .skip_while(|line| *line != "Commands:")
            .skip(1)
            .take_while(|line| line.starts_with("  "))
            .filter_map(|line| line.split_whitespace().next())
            .filter(|name| *name != "help")
            .map(|name| format!("{prefix}{name}"))
            .collect();
        assert!(!listed.is_empty(), "no commands under {parent:?}");
        for command in listed {
            assert!(
                COMMANDS.contains(&command.as_str()),
                "`chatgpt {command}` is missing from COMMANDS"
            );
        }
    }
}
