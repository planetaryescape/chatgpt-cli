# chatgpt-cli

A CLI and TUI for managing ChatGPT history through chatgpt.com's private web API. Start with `docs/maintainers.md`: layout, verification, the classification change procedure, locked decisions, and out-of-scope work.

## Every change

1. `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo nextest run` pass. Never set `RUSTFLAGS`: it replaces `.cargo/config.toml`'s `reqwest_unstable` cfg and the build fails. The `cli_reference` test fails when `docs/reference/cli.md` is stale; `CHATGPT_UPDATE_CLI_REFERENCE=1 cargo nextest run -p chatgpt-cli --test cli_reference` regenerates it.
2. For anything touching the API, rendering, or classification, run the real command against your own account (`cargo build`, then `target/debug/chatgpt <command>`, which uses the `dev` instance). Tests only use fixtures; the private API is verified live.
3. Update the page in `docs/` that describes what you changed.
4. Commit as `type: description`, using your own Git identity without agent attribution.

## Private data

Transcripts can contain private conversations about other people. When debugging, print ids, synthetic titles, counts, scores, and turn/token numbers; read transcript text only when the bug is in the text itself. Test destructive actions on a throwaway chat or on already-archived chats you restore. Cookies, HARs, tokens, and real conversation details stay out of git and public issues.

## Before you change

- An API call, or something chatgpt.com returns: read `docs/explanation/chatgpt-api.md`, then re-observe the live site rather than guessing.
- A Jev question, the policy, or the summariser: read `docs/reference/classification.md` and the classification section of `docs/maintainers.md`, and bump the right version constant.
- The TUI (`crates/tui`): tests drive keys and screens against ratatui's `TestBackend` with insta snapshots, and the real binary in a pseudo-terminal (`crates/cli/tests/tui_cli.rs`); see `docs/maintainers.md`.
- Commands, flags or output formats: update the relevant page in `docs/` and regenerate `docs/reference/cli.md`. Output the TS CLI printed is pinned in `crates/cli/tests/golden/`; change a golden line only on purpose, and say why in the commit.
- Terminal output: stdout carries data and stderr carries status. Send status lines through `output::note` and failures through `output::error_line` in `crates/cli/src/output.rs`.
