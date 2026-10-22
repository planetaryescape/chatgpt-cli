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

The CLI began as a TypeScript one, and the Rust port matched its output byte for byte. The parity harnesses that proved it ran both CLIs; most of them still live only in tag `v0.1.5` (see [The parity harness](#the-parity-harness) below). What the TS CLI printed in their last run is frozen in `crates/cli/tests/golden/*.jsonl`, and `crates/cli/tests/golden.rs` builds the same data for the Rust CLI alone and requires the same output: `list` and `stats` over 120 synthetic chats with every kind of verdict, and `export` plus lexical, semantic, hybrid and remote `search` over rich conversation trees (`crates/fake-chatgpt/src/fixtures.rs`). For an intended change, `CHATGPT_UPDATE_GOLDEN=1 cargo nextest run -p chatgpt-cli --test golden` rewrites the files; read the diff and give the reason in the commit. Deliberate changes so far: search chunks and snippets never hold half an emoji (`CHUNK_VERSION` 2); equal semantic scores are ordered by chat id (stage 7).

`crates/cli/tests/upgrade_cli.rs` opens an index made by v0.1.5 (`fixtures/v0.1.5.db`, synthetic chats only) with the current daemon and checks every row survives, `native_rows` is dropped, and the transcripts (rendered at version 2) are fetched again from the fake chatgpt.com and chunked.

### The parity harness

Two harnesses run write commands in both CLIs and compare what they did. `crates/cli/tests/parity_write.rs` runs one write command — `archive` — in the TypeScript CLI and in the Rust CLI, each against its own fake chatgpt.com, and their stdout, stderr, exit status and the HTTP writes they sent are compared. Durations and rates are left out of the comparison; everything else on stderr is compared byte for byte. Exit codes are compared as success or failure, because the Rust CLI's invalid-input errors exit 2 where the TS CLI exits 1. A difference fails the test with both sides printed.

`crates/cli/tests/parity_write_path.rs` widens that to the whole write path, with the scenarios the v0.1.5 harness ran (`git show v0.1.5:crates/cli/tests/parity_mutations.rs`): previews and dry runs (`archive`, `unarchive`, `delete` by filter, `--brainstorm`, `--suggest`), the selection errors (no ids, an unknown id, an archived chat, an ambiguous prefix, a bad age, an unknown suggestion), the changes themselves (`archive`, `unarchive`, `delete`, `rename`, `title`), the project moves (`project list`, `add`, `remove`, a read-only project, an unknown one) and the memory deletes (`memory delete`, a prefix, an unconfirmed delete). Both CLIs also get a fake TypeSafe, so neither reaches the real one.

`crates/cli/tests/parity_classify.rs` covers the model path: the part that spends money and decides what gets deleted. The TS CLI and the Rust CLI run the same `configure`, `classify`, `titles`, `memory classify` and Jev guard commands, in the same order (the v0.1.5 harness's scenarios, `git show v0.1.5:crates/cli/tests/parity_classify.rs`), each against its own fake chatgpt.com, fake TypeSafe, fake OpenAI and fake Anthropic, and their own stand-in `codex` and `claude` on `PATH`. The harness compares their stdout, stderr and exit status, the model requests they send (the Jev bodies, the OpenAI and Anthropic bodies, and the subscription CLIs' arguments and stdin, byte for byte), the config file each side writes, and the verdicts, titles and memory classifications they end with. It also runs the question before a large batch of summaries (`Go ahead? [y/N]`) at a terminal on both sides. One scenario makes the stand-in `codex` fail on a long chat's summary: each CLI must try `codex` first, then `claude` with the same chat and the summary instructions, save `claude`'s summary and have Jev judge the chat from it, without a failure line. Another makes the fake OpenAI fail the same summary: each CLI must ask OpenAI first, then Anthropic with the same chat, save Anthropic's summary and have Jev judge the chat from it. Two more make every summariser fail, once the APIs (OpenAI, then Anthropic) and once the subscription CLIs (`codex`, then `claude`): each CLI must try both in order, save no summary, keep the chat's previous judgment, ask Jev nothing, and print its own `failed:` line.

Each CLI is also held to the **expected** end state — the chat archived in ChatGPT and in each index, the chat deleted from both, the title and project changed, the memory gone, the manual local title saved, and exactly the requests each CLI must send — not only to the other's. Two CLIs that agree on doing nothing would pass a comparison on its own, so every scenario in the widened write path must say what it expects to have happened. The requests are sorted before they are compared, because the Rust CLI runs a bulk change with three workers at once.

The differences the harness asserts as expected are the ones the repository documents: the exit code (1 in the TS CLI, 2 in the Rust CLI, for invalid input) and the failure line (the TS CLI quotes the body, `404 from /backend-api/…: <body>`, the Rust one does not, `404 Not Found from /backend-api/…`), both in `docs/issues/mutations-followups.md`. Nothing else differs today: every other scenario's stdout, stderr and requests are byte for byte equal.

The model path's harness asserts the differences `docs/issues/classify-followups.md` documents:

- the exit code (1 in the TS CLI, 2 in the Rust CLI, for invalid input);
- the subscription CLIs' environment: the Rust CLI runs `codex` and `claude` with only what they need to find their own login, the TS CLI with its whole environment, API keys included. The harness compares the calls' arguments and stdin, not their environment, and asserts the difference instead;
- the `failed:` line when every summariser fails: the TS CLI quotes each provider's response body and each tool's stderr, the Rust CLI says only how each failed. The harness expects each side's exact line, compares the rest of stderr as usual, and fails if the Rust CLI's stderr holds the fakes' `secret` body or `SENTINEL` stderr in any scenario;
- at a terminal, a step opens with its label where the TS CLI draws its bar at zero, and the TS CLI colours its `failed:` lines. The terminal transcript is compared without the step lines, the colours and the durations.

Nothing else differs: all 35 commands print the same notes, steps, cost lines and failures (but for those two `failed:` lines), send the same Jev, OpenAI and Anthropic requests and the same subscription CLI calls byte for byte, and end with the same verdicts, titles and memory classifications.

A write that answers 404 has its own test, `a_404_drops_the_chat_from_both_indexes`: both CLIs fail, both drop the chat from their index (the Rust daemon reads a 404 as "the chat is gone"), and both leave it in ChatGPT's own list, so an expected-state assertion for a failing command has to expect the row's absence.

The TypeScript CLI was this repository's own code until the Rust port replaced it: commit `1b8c950` ("feat: create ChatGPT projects from CLI") is its last state — the commit the port was made from — and an ancestor of `main`. So the harness needs a checkout of it, which is a worktree of this repository:

```sh
git worktree add ../chatgpt-ts-port-base 1b8c950
cd ../chatgpt-ts-port-base && bun install
```

Then point the test at it and run it:

```sh
CHATGPT_TS_REPO=$PWD/../chatgpt-ts-port-base cargo nextest run -p chatgpt-cli --test parity_write --no-capture
CHATGPT_TS_REPO=$PWD/../chatgpt-ts-port-base cargo nextest run -p chatgpt-cli --test parity_write_path --no-capture
CHATGPT_TS_REPO=$PWD/../chatgpt-ts-port-base cargo nextest run -p chatgpt-cli --test parity_classify --no-capture
```

Without `CHATGPT_TS_REPO`, or without a `bun` that runs, the test skips with a note, so CI (which has neither) stays green. The TS CLI reads its own index (`$XDG_DATA_HOME/chatgpt-cli/index.db`), which the Rust CLI's index is copied into, and reaches its fake through a Bun plugin that points its HTTP client there. The plugin throws, rather than let the TS CLI reach the real chatgpt.com, if `src/api/client.ts` no longer holds the string it replaces; the test also reads that file first and fails if its base-URL line changed at all, because the plugin's guard alone lets a different base through. The same is true of the model APIs: `src/classify/model-api.ts` holds the hard-coded OpenAI and Anthropic URLs, and the plugin points them at their fakes (or at an address nothing answers), throwing rather than let the TS CLI reach a real provider. The TS CLI runs with a synthetic `HOME` and its own `XDG_*` directories, so it reads neither the developer's config nor the developer's cookies; that is why the harness looks for a `bun` on `PATH` that runs with that `HOME` (a version manager's shim can need the real one).

Both CLIs are sealed off from every real service, the way the tests' `Env` seals the Rust one: only the stand-in `codex` and `claude` are on the `PATH`, the TypeSafe base URL always points at the fake (or at `http://127.0.0.1:9`, which nothing answers — never at the SDK's default, `api.typesafe.ai`), and no OpenAI or Anthropic key is carried unless the scenario asks for that provider's API — then it is the fake's key, and it can only reach the URL the plugin patched (the fake, or `http://127.0.0.1:9`). The TypeScript CLI has no base URL for either (both are hard-coded), so without the patch it would call the real one. A proxy would carry the fake's traffic off the machine, so none is passed on either side. `the_ts_cli_cannot_reach_a_real_service` checks this: the environment as built, and a Jev-asking command that reaches the fake TypeSafe rather than a real service. `the_ts_cli_cannot_reach_a_real_model_service` (in `parity_classify.rs`) does the same for OpenAI and Anthropic.

`CHATGPT_PARITY_SEED_DIFFERENCE=1` breaks the TS side before the scenarios (its projects, in the write path; a chat's title and, in the first scenario with a fallback, the fallback too in the model path, so the TS side has no summariser left), to show that the harness fails with both sides printed when the two CLIs differ.

The write path is proved for `archive` alone in `parity_write.rs`, and for the whole path in `parity_write_path.rs`. The model path — Jev's questions, the summaries, Luna's review, the verdicts, the local titles and the memory classifications — is proved in `parity_classify.rs`. The export and search reads are still only in the tag (`v0.1.5`); they are pinned by the goldens instead.

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
