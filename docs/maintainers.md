# Maintainer guide

How to change `chatgpt` safely: where things live, how to verify a change, and which decisions are settled.

## Layout

| Path | Job |
|---|---|
| `crates/cli/` | The `chatgpt` binary: command definitions (`args.rs`, clap) and one module per command group (`review` in `review_cmd.rs`); stdout and stderr in `output.rs` |
| `crates/chatgpt/` | Browser cookies, the session exchange, and the Chrome-impersonating HTTP client (impit) |
| `crates/core/` | File locations and instances, error kinds and exit codes, the user config |
| `crates/protocol/` | The CLI–daemon IPC messages and codec |
| `crates/launcher/` | Finding, starting, restarting and stopping the daemon, and talking to it |
| `crates/store/` | The daemon's SQLite index: chats, cached transcripts, summaries, judgments, local titles, search chunks and vectors; migrations in `migrations/` |
| `crates/daemon/` | The daemon: sync (`sync/`), policy (`policy/`), `list`/`stats` reads, rendering (`render.rs`), search indexing, embedding and search (`search/`), changes, and classification with the background Jev (`classify/`) |
| `crates/embed/` | The embedding model's pinned files, the tract embedder, and the worker process the daemon runs it in |
| `crates/tui/` | `chatgpt tui`, a ratatui client of the daemon (`model.rs` filters, `app.rs` keys and state, `ui.rs` drawing, `run.rs` terminal and requests, `connections.rs` the daemon connections they share); snapshots in `src/snapshots/` |
| `crates/typesafe/` | A small client for TypeSafe's System One API (Jev), used by the daemon's classification |
| `crates/model-api/` | Small clients for OpenAI's Responses and Anthropic's Messages APIs (summaries and Luna with a configured key) |
| `crates/fake-chatgpt/` | A fake chatgpt.com (reads, writes and their quirks), fake TypeSafe, OpenAI and Anthropic APIs, and the answers the debug binary's stand-in `codex` and `claude` give (`model_answers.rs`), for tests |
| `third_party/impit/` | impit with one patch: no environment writes after startup (`third_party/README.md`) |
| `install.sh` | Installs a release into `~/.local/bin`, checking its SHA-256 |

## Verify a change

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo nextest run        # or cargo test
```

Don't set `RUSTFLAGS`: it replaces the `reqwest_unstable` cfg in `.cargo/config.toml` and the build fails.

Tests use temporary SQLite fixtures and never call chatgpt.com, TypeSafe or a summariser. `PATH` holds only stand-in `codex` and `claude` (the debug binary's hidden `__fake-model-cli`) and the system's own directories, and the API base URLs point at fakes (`TYPESAFE_BASE_URL`, `CHATGPT_TEST_OPENAI_URL`, `CHATGPT_TEST_ANTHROPIC_URL` and the other `CHATGPT_TEST_*` overrides, honoured by debug builds only). Debug builds run as the `dev` instance (`~/Library/Application Support/chatgpt-cli-dev`), so a local build never touches the installed daemon.

For anything touching the API, rendering or classification, also run the real command against your own account:

```sh
cargo build
target/debug/chatgpt sync
target/debug/chatgpt list --limit 5
target/debug/chatgpt export <link>
target/debug/chatgpt classify --title "<a few known chats>"
```

Before a change to the TUI, run `cargo nextest run -p chatgpt-tui` (keys and screens against ratatui's `TestBackend`, with insta snapshots: `INSTA_UPDATE=always` rewrites them, then read the diff) and `cargo nextest run -p chatgpt-cli --test tui_cli` (the real binary in a pseudo-terminal against the fake chatgpt.com, read through a vt100 screen). Then open `target/debug/chatgpt tui` yourself.

### Output pinned from the TS CLI

The CLI began as a TypeScript one, and the Rust port matched its output byte for byte. The parity harnesses that proved it ran both CLIs; they live in tag `v0.1.5`. What the TS CLI printed in their last run is frozen in `crates/cli/tests/golden/*.jsonl`, and `crates/cli/tests/golden.rs` builds the same data for the Rust CLI alone and requires the same output: `list` and `stats` over 120 synthetic chats with every kind of verdict, and `export` plus lexical, semantic, hybrid and remote `search` over rich conversation trees (`crates/fake-chatgpt/src/fixtures.rs`). For an intended change, `CHATGPT_UPDATE_GOLDEN=1 cargo nextest run -p chatgpt-cli --test golden` rewrites the files; read the diff and give the reason in the commit. Deliberate changes so far: search chunks and snippets never hold half an emoji (`CHUNK_VERSION` 2); equal semantic scores are ordered by chat id (stage 7).

`crates/cli/tests/upgrade_cli.rs` opens an index made by v0.1.5 (`fixtures/v0.1.5.db`, synthetic chats only) with the current daemon and checks every row survives, `native_rows` is dropped, and the transcripts (rendered at version 2) are fetched again from the fake chatgpt.com and chunked.

### impit

impit is a git dependency that only impersonates Chrome when apify's forks of `h2`, `rustls`, `hyper-util` and `tower-http` are in the graph (`[patch.crates-io]` in `Cargo.toml`), built with `--cfg reqwest_unstable` (`.cargo/config.toml`). Cargo drops a patch with only a warning when the graph wants a newer version, so `hyper` and `reqwest` stay pinned in `Cargo.lock` to impit's own lockfile versions, and `crates/chatgpt/tests/fingerprint_patches.rs` fails if a fork falls out. Re-check that test after any `cargo update`.

## Regenerate docs

```sh
CHATGPT_UPDATE_CLI_REFERENCE=1 cargo nextest run -p chatgpt-cli --test cli_reference
```

This rewrites `docs/reference/cli.md` from the binary's `--help`. Without the variable the same test fails when that file is out of date, so run this after changing any command, flag or description in `crates/cli/src/args.rs`. A new command also goes in the test's `COMMANDS` list, in the order the docs introduce it; another test fails until it does.

Search index changes that alter passage text bump `CHUNK_VERSION` in `crates/daemon/src/search/chunks.rs`: the indexer then rebuilds every chat's chunks from the cached transcripts, without fetching, and the embedder embeds them again (about 30 minutes for 47k chunks on one core). Changes to the model, revision, pooling, quantization, runtime or batch size change `MODEL_VERSION` in `crates/embed/src/lib.rs` ([Embeddings](explanation/embeddings.md)). Then check lexical, semantic and hybrid results on the real account; fixture tests do not measure retrieval quality or runtime.

## Change classification

| You changed | Then |
|---|---|
| A threshold in `crates/daemon/src/policy/` | Nothing to re-run; suggestions are recalculated on read |
| A question, its criteria, or the set of questions (`crates/daemon/src/classify/questions.json`) | Bump `questions_version` in `Profile::builtin` (`crates/daemon/src/policy/profile.rs`); next `classify` re-judges everything |
| A follow-up question or its criteria (`deep_questions.json`) | Bump `deep_questions_version`; next `classify` re-asks unsure chats |
| A saved-memory question (`memory_questions.json`) | Bump `memory_version` |
| Luna's final-review prompt (`classify/prompts/luna_review.txt`) or result meaning | Bump `luna_version`; next `classify` re-reviews chats still unsure, every product label, borderline product ideas and conflicting time-expired cases |
| Luna's local-title prompt (`prompts/titles.txt`) or result meaning | Bump `local_title_version`; next `titles` regenerates titles and themes |
| The summary prompt (`prompts/summary.txt`) | Bump `SUMMARY_PROMPT_VERSION` in `classify/summarise.rs`; long chats are re-summarised on your subscriptions |
| The memory review prompt (`prompts/memory_review.txt`) | Bump `memory_version` |
| Transcript rendering (`crates/daemon/src/render.rs`) | Bump `RENDER_VERSION` there; every reader looks transcripts up at it, so the indexer re-downloads them all through the batch endpoint (about six minutes for 800 chats) |
| A provider's prices | Update the `Price` constants in `crates/daemon/src/classify/costs.rs` with the new source and date |

The question files are the questions' source: edit them directly. They were first written out from the TS CLI's sources.

Then:

1. Check the change against a labelled handful of real chats. Print titles and scores, not transcripts:

   ```sh
   target/debug/chatgpt classify --title '^(Chat A|Chat B|Chat C)$' --pinned -y
   target/debug/chatgpt list --title '^(Chat A|Chat B|Chat C)$' --json | jq -r '.[] | "\(.jev.suggestion)\t\(.jev.reason)\t\(.title)"'
   ```

2. Update [Classification reference](reference/classification.md).

## Handle personal data

The database holds private conversations that mention other people.

- When debugging, print ids, titles, counts, scores and token counts. Read transcript text only when the bug is in the text.
- Test destructive actions on a throwaway chat you create for the purpose, or on already-archived chats you restore afterwards.
- Keep cookies, HAR files and access tokens out of the repo. `.gitignore` covers `*.har`.

## Locked decisions

Settle these with the owner before changing them.

- **Auth comes from a local browser session.** Use the macOS default browser unless `--browser` and `--profile` select another source. Do not silently fall back to another browser, which may be logged into a different account. No separate login flow or stored ChatGPT tokens.
- **HTTP goes through `impit`.** Plain fetch and curl are blocked by Cloudflare. Headless Chrome was challenged too; a visible browser works but can't run for every command.
- **Model credentials.** Environment variables take precedence over `~/.config/chatgpt-cli/config.json`. A configured OpenAI or Anthropic key replaces that provider's subscription CLI for model work. Without those keys, summaries try Codex `gpt-6-luna`, then `claude -p --model haiku`.
- **Memory classification.** The saved-memory rubric and version are separate from chat classification; see `docs/reference/memory-classification.md`. Quick decisions can only keep or route to Luna. Every delete suggestion needs Luna review. Do not infer memory staleness from an archived source chat.
- **Brainstorms are always keep.** That includes drafting talks and articles.
- **Faith influences do not make a sermon brainstorm.** Use `sermon` when religious teaching or reflection is the piece's main purpose; broader writing stays `writing` even when faith informs it.
- **Product brainstorms are the user's own concepts or substantial new directions.** Generic coding, settled implementation and work on someone else's product do not qualify by themselves.
- **Re-askable quick help is delete.** The test is whether anything would be lost, not the number of turns.
- **Age alone doesn't affect suggestions.** One-off help may become delete when its useful moment has passed and it leaves no lasting record or artifact. Keep meaningful decisions, personal history, claims, drafts and work to resume even after a deadline or event.
- **A few misses are acceptable.** Don't tune question wording to flip a single chat. Borderline chats surface as `keep?`.

## Out of scope

- Other operating systems for browser-session auth. macOS supports Safari, Chrome, Firefox, Dia, Arc, Brave and Edge.
- Sending messages or starting chats. Verified 2026-09-28: a send without sentinel tokens gets `403 Unusual activity has been detected from your device`, and the tokens need Turnstile and fingerprinting programs that only a real browser runs. See [The chatgpt.com web API](explanation/chatgpt-api.md#sending-messages).
- Shared links (`/share/…`).
- A web UI.

## Commits

`type: description`, with an optional body and no generated attribution footers. Use your own Git identity.
