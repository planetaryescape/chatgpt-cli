# How the Rust CLI works

The Rust `chatgpt` (in `crates/`) is replacing the TS CLI one group of commands at a time. Until it covers everything, it hands the rest to the TS CLI unchanged.

## Native commands and the bridge

`configure`, `sync`, `list`, `stats`, `export` (and its alias `show`), `search` (full-text, `--semantic`, `--hybrid` and `--remote`), `search-index`, `archive`, `unarchive`, `delete`, `rename`, `title`, `titles`, `classify`, `project` (`create`, `list`, `add`, `remove`), `memory` (`list`, `summary`, `delete`, `classify`), `daemon` and `import-legacy` run in Rust. `review` and `tui`, including their `--help`, run as `bun <cli.ts> <args…>`: the Rust process replaces itself with bun, so arguments, stdin, stdout, stderr, the terminal and the exit code are the TS CLI's. The TS CLI is found by path, never as `chatgpt` on PATH (which may be the Rust binary):

1. `CHATGPT_TS_CLI`, the TS CLI's `src/cli.ts`;
2. `~/.bun/install/global/node_modules/chatgpt-cli/src/cli.ts`, where `bun link` puts it.

Bun comes from PATH, else `~/.bun/bin/bun`. A bridged process carries `CHATGPT_BRIDGED=1`; a Rust `chatgpt` that sees it refuses to bridge again, so a misconfigured `CHATGPT_TS_CLI` can't loop.

## The daemon

Native commands ask a background daemon over a Unix socket. A native command that finds no daemon starts one (bridged commands and `configure` don't need it), detached from the terminal, and waits until it answers. The daemon:

- reads the browser's cookies once per `--browser`/`--profile` choice (the read can raise a Keychain prompt) and keeps the access token in memory only. It reads them again only when ChatGPT rejects the token (401, or a 403 that isn't a Cloudflare challenge);
- syncs one account per index. A pass pins its browser choice and the session's account from start to finish, so a command choosing another browser meanwhile can't change what it reads. A sync whose session belongs to another account than the one the index was built from is refused: keep each account in its own instance (`CHATGPT_INSTANCE=<name>`);
- keeps its own index, `~/Library/Application Support/chatgpt-cli/chatgpt.db`, fresh (a daemon started on an index synced within the interval waits out the rest of it, so a cold `list` sends nothing; on an older index the first background pass starts at once, while `list` still answers from the index without waiting for it): a delta sync every 2 minutes while a command ran in the last 10 minutes, otherwise every 15. The archived-list sweep and the cache reconcile run at most hourly in the background, and on every `chatgpt sync`. `sync --full` is only ever run on request;
- backs off for as long as ChatGPT's rate limit asks (at least a minute, at most an hour) and shows it in `chatgpt daemon status`. A rate limit in any step, the cache reconcile included, ends the pass, and the TS sync waits for the next one;
- logs to `~/Library/Application Support/chatgpt-cli/logs/daemon.log.<date>`, one file a day, seven kept. Logs never hold cookies, tokens or response bodies.

`list` and `search` never touch the network, except `search --remote`, which asks ChatGPT's search because you asked for it. `stats` reads the saved memories live, as the TS CLI does. `export` always fetches the chat from the single-chat endpoint, as the TS CLI does, rather than using a cached transcript: the cache is filled from the batch endpoint, which doesn't name the model, so its header would differ. An export too large for one IPC frame (16 MiB) is handed to the TS CLI with the same arguments until exports are streamed (`docs/issues/export-frame-limit.md`).

## The search index

Lexical `search` needs no `search-index` step. The daemon keeps a full-text index (SQLite FTS5, built as the TS CLI's `src/search/` builds it) in its own database:

- When it starts, it chunks every cached transcript that's current for its chat. After every successful sync pass, and after an import, it also fetches the transcripts it lacks through the batch endpoint, 10 chats per call with half a second between calls, as `search-index` does. The indexer itself fetches nothing until the daemon's first successful pass, so it never reads cookies on its own (the startup sync of an empty or stale index still does).
- Chunks count as current only while they were built from the transcript the cache holds: any write that brings a transcript new content (the TS import, the indexer) drops that chat's chunks in the same transaction (triggers in `crates/store/migrations/0003_search_follows_transcripts.sql`), so the cache reconcile can only move forward chunks of the content it verified.
- Before the indexer replaces a transcript that is only older than its chat, it runs the cache reconcile's check on it: unchanged content moves every cache (judgments, summaries, titles, chunks) forward, as `sync` would have; changed content replaces the transcript and leaves the rest stale.
- Each chat's transcript and chunks are written in one short transaction, so `list` and `search` never wait and an interrupted run continues where it stopped. Active, archived and pinned chats are all indexed; `search` filters by scope.
- It steps aside while a `sync` or `export` runs. A rate limit stops fetching for as long as ChatGPT asked (between a minute and an hour) without holding up syncs, a chat ChatGPT doesn't return (or whose batch it answers with an error) is asked for again after an hour, and a timeout or dropped connection ends the run until the next pass.
- `chatgpt daemon status` shows `search index: N of M chats indexed`, whether it's indexing, how many transcripts it fetched, and why it's waiting. While the index is incomplete, `search` answers from what's indexed and says `N of M chats indexed` on stderr.

## Embeddings

Semantic and hybrid `search` need no `search-index` step either. After the indexer writes chunks, the daemon's embedder gives each chunk that's current for its chat a 384-dimension vector, as the TS CLI's `search-index` does. It uses the same model, but a different runtime ([why, with measurements](embeddings.md)):

- The model runs in a worker process, `chatgpt daemon embed-worker`, started under macOS's utility QoS (`taskpolicy -c utility`) on one thread, so embedding uses at most one core and gives way to your work. The worker is stopped after ten idle minutes, which unloads the model.
- One chunk at a time, saved 16 at a time, so an interrupted run resumes where it stopped. It steps aside while a `sync` or `export` runs, and a semantic search's query goes before the next chunk.
- Each text gets a minute. A worker that takes longer is stopped and replaced, and that text (or query) fails with a clear error, so a hung worker can't hold up semantic search. A worker whose answer was abandoned midway (a client that went away) is replaced too, so its late answer can never be read as another text's vector.
- Every run checks the model is in the TS CLI's model cache, `~/.cache/chatgpt-cli/models`, and downloads what's missing (23 MB, checked against a pinned SHA-256), even when every chunk already has its vector, since queries need the model too. When the download fails, lexical search is unaffected; `daemon status` and semantic search say why, and the daemon tries again 15 minutes later on its own. A search never downloads anything; one that finds the model gone, or damaged (the worker checks its SHA-256 when it loads it), says so and wakes the embedder to fetch it again, at most once per retry window.
- A chunk's vector is deleted with it (a trigger in `crates/store/migrations/0004_search_vectors.sql`), and a vector is saved only if its chunk still holds the text it was made from.
- A search reads in one snapshot (a read transaction), so a hit's chat, score, scope and snippet always belong together even while the indexer replaces chunks.
- `chatgpt daemon status` shows `embeddings: N of M chunks embedded` and why it waits. While embedding is incomplete, semantic and hybrid search answer from the embedded chunks and say `N of M chunks embedded` on stderr; with none yet, they say so instead of answering.

`search-index` asks the indexer to fetch now (without waiting for a sync) and the embedder to retry a failed download, shows both making progress, and reports on its scope (`--archived`, `--all`) as the TS CLI's does once neither has work left. It exits 1 if a chat couldn't be fetched, the index is still incomplete, or the model is unavailable.

The vectors are tagged with the Rust `MODEL_VERSION` and live only in the daemon's index. An index from 0.1.1 gains the vectors table on the daemon's first start, and the existing chunks are embedded in the background while `list` and `search` keep answering.

## Changing chats, projects and memories

`archive`, `unarchive`, `delete`, `project add` and `project remove` run in three steps, so the chats a preview shows are exactly the ones changed:

1. The daemon resolves the chats from the ids, id prefixes, the ids on stdin (`-`), or the filters, with the TS CLI's checks and messages (pinned chats only with `--pinned`, archived ones only with `--archived` or `--all`; `unarchive` picks archived chats). It never touches the network for this.
2. The CLI prints the preview (the first 25, then how many more), stops for `-n`, and otherwise asks: `[y/N]`, or the typed count for `delete` and `memory delete`. The answer comes from the terminal; when the ids came in on stdin, from `/dev/tty`. Without a terminal to ask, it fails and says to pass `-y`.
3. The CLI sends back the ids it showed, and the daemon changes each one in ChatGPT first, then in its index, with the TS CLI's pacing (a quarter second apart; three deletes at once) and progress lines. A delete drops the chat's row, and its search chunks and vectors go with the indexer's next run; an archive change flips `is_archived`; a rename changes the title (the indexer rebuilds the chat's chunks); a project move sets `project_id`. If the CLI goes away (Ctrl-C), the chat in hand is finished, ChatGPT and the index both, and no further one is started. A change and a sync pass never run at the same time: a change waits for a running pass (and the pass for the change), so a pass that listed a chat before the change can't write its old state back. Every change, and every project and memory read, uses the session pinned to the index's account: a session for another account, or one renewed into another, is refused before anything is sent.

ChatGPT's quirks are the TS CLI's: a pre-2025 chat's rename answers 500 yet applies (the error says so), a project move answering 500 is checked with a read of the chat, a delete answering 404 has done its job, and a memory delete counts only when ChatGPT answers `success: true`.

Archiving is idempotent, so it's retried like a read. A delete, a rename, a project move, a new project and a memory delete mustn't happen twice: they're retried only after an answer that shows ChatGPT turned them away (a Cloudflare challenge, a 429). After a gateway error or a dropped connection the write may have applied, and the CLI says exactly that, with the path, and that `chatgpt sync` shows whether it did, rather than sending it again.

`title` writes a manual local title to the daemon's index only. While the bridge exists, the TS import keeps it (see below).

### The Jev guard

`archive`/`delete --check`, and `--suggest delete` on `delete` (or `--suggest archive` on `archive`), judge each selected chat that has no current judgment, during the command, as the TS CLI's `checkWithJev` does: `classify`'s first pass (below), with its steps, notes, summaries for long chats, the question before a large batch of them (`-y` skips it) and cost lines, and only chats Jev confidently backs are kept. Anything it couldn't judge is held back, and so is a chat whose `update_time` changed while it was being judged (a sync pass ran meanwhile): its verdict would rest on old content.

An answer of the wrong shape (a score that isn't a number in its range, a choice that isn't one of the question's) is never saved; the failure names the question, never the value. A stored judgment that can't be read counts as no judgment, in `list`, `stats` and the guard, which judges it again.

## Classification

`classify`, `titles`, `memory classify` and the guard run in the daemon (`crates/daemon/src/classify/`), ported from the TS CLI's `src/classify/` with its models, prompts, flags, fallback order, pacing, notes and cost lines, at this repository's question and policy versions (`Profile::builtin`, D9). The CLI resolves the chats first, as for `archive`, then hands their ids over; each run has its own task, so what it paid for is saved even if the client goes away, while a client that went away (Ctrl-C) stops it from starting further calls.

- **Jev** (TypeSafe's System One, `crates/typesafe`): the first pass, the follow-up for unsure chats (`classify` only) and the saved-memory quick pass.
- **Summaries** of chats over 12,000 tokens, in the TS CLI's order: OpenAI's API with an OpenAI key, else `codex exec -m gpt-6-luna`; then Anthropic's API (`claude-haiku-4-5`) with an Anthropic key, else `claude -p --model haiku`. `crates/model-api` calls the APIs, once each, with a 10-minute limit. Before summarising more than 500k tokens without `-y`, the daemon asks the client (`Go ahead? [y/N] `) and waits for its answer.
- **Luna** (`gpt-6-luna`): OpenAI's API with a strict JSON schema, else `codex exec --output-schema`. It reviews what Jev can't settle, writes local titles, and reviews saved memories Jev couldn't keep. Its answers are checked before anything is saved.

Keys come from the requesting command's environment (`TYPESAFE_API_KEY`, `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`), which it passes on with the request, else from the user config, read on every call; never from the daemon's own environment. The same goes for `PATH`, where `codex` and `claude` are found: a daemon started at login has a bare one. `codex` and `claude` run in an environment holding only `HOME`, the locale, `TMPDIR` and their own config directories (none of our keys), in their own process group, which is killed after 10 minutes or when the run is dropped; their output is captured and never logged or shown, so a failure says only how they exited.

Every judgment, follow-up, Luna review, summary, title and memory classification is saved as soon as it's made, under the sync pass lock (briefly), so no pass rewrites the chat or moves its caches forward halfway through a save.

`configure` writes `~/.config/chatgpt-cli/config.json` itself (0600, through a rename), in the TS CLI's layout; it keeps an `auto_jev` setting the TS CLI would drop.

### Jev in the background

After each successful sync pass, the daemon judges chats that are new or changed since it was first enabled and have no current judgment, as a bare `chatgpt classify` would pick them (active, unpinned), newest first, at most 50 a pass, with `classify`'s pacing. It runs Jev's first pass only: never the follow-up, Luna or a summary; a long chat without a cached summary waits for a `classify` someone runs. It needs a Jev key in the user config (a key in a command's environment doesn't count: no command asked for it), and `"auto_jev": false` there switches it off. The first time it runs, it notes the newest chat's time, so enabling it never judges the whole history. A chat Jev fails on waits six hours before another try. `chatgpt daemon status` shows whether it's on (and why not), what it judged and spent today (UTC), and its last run.

## Matching the TS CLI

`crates/cli/tests/parity_classify.rs` runs both CLIs through `configure`, `classify`, `titles`, `memory classify` and the guard against twin fakes (chatgpt.com, TypeSafe, OpenAI, Anthropic, and stand-in `codex` and `claude`): notes, steps, cost lines, the question before a large batch (in a pseudo-terminal) and failures match, as do every model request (Jev, OpenAI and Anthropic bodies, and the CLIs' arguments and stdin) and the verdicts, titles and memory classifications afterwards. `crates/cli/tests/parity_mutations.rs` runs both CLIs through the same changes, in order, against two fake chatgpt.coms and two fake TypeSafes that start alike: previews, prompts (in a pseudo-terminal), notes, summaries and errors match byte for byte (leaving out durations and rates), as do the requests each sends, the Jev request bodies, and ChatGPT's and both indexes' state afterwards.

The chunks reproduce the TS CLI's exactly, down to the bytes Bun stores when a chunk boundary splits an emoji, so ranking, scores and snippets match the TS CLI's over the same transcripts (`crates/cli/tests/parity_export_search.rs`). The same harness gives both CLIs a stand-in embedder, so their vectors are identical, and requires identical semantic and hybrid output; and it points both at one fake `global/search` for `--remote`. The TS CLI's own search index, in its own database, is never read or written.

A client keeps a running daemon that speaks its protocol and is at least as new as itself, and restarts an older one. `chatgpt daemon install` writes a LaunchAgent that starts the installed daemon at login (it doesn't load it; the command prints how).

Debug builds and binaries under `target/` use the `dev` instance (`chatgpt-cli-dev`), so a local build never touches the installed daemon. `CHATGPT_INSTANCE=<name>` picks another.

## Data from the TS CLI

While the bridge exists the TS CLI's `review` and `tui` read `~/.local/share/chatgpt-cli/index.db`, and the TUI can write manual titles there. The daemon imports the TS index's local titles, summaries and cached transcripts into its own index at startup, after every TS sync and on `chatgpt import-legacy`. The import opens the TS index read-only and mirrors local titles by their key (rows the TS index dropped are dropped), keeping a newer `update_time` the daemon's reconcile wrote when nothing else differs. Summaries and transcripts are caches both sides fill, so the import never deletes one and replaces the daemon's copy only when that copy isn't current for the chat.

Judgments, follow-ups, Luna reviews and memory classifications are no longer imported: the daemon makes them itself, at this build's versions, and the TS CLI's (made at the installed CLI's own versions) would only ever be stale here. Until stage 6 ports them, the bridged `review` and `tui` show the TS index's own, older verdicts.

The daemon also writes some titles itself: a `title`, and Luna's from `titles` and `classify`. The TS index never has them, so the import would drop them; instead it keeps a row the daemon wrote (listed in `native_rows`) until the TS CLI writes a newer manual one for that chat.

The TS index belongs to one account: whichever the TS CLI's default browser session holds. So the TS sync and the import run only for the installed (or dev) instance on a pass that reads the default browser choice, with no `--browser`, `--profile` or `CHATGPT_BROWSER*`. A named instance, or a pass on another browser, skips both and says so in `daemon status` ("TS sync skipped: …"), because a TS full sync from another account would replace the default account's chats in the shared index.

After a pass that's due for it (every 15 minutes, and on every `chatgpt sync`), the daemon also runs the TS CLI's own `sync`, niced, so the bridged `review` and `tui` see the same chats. Only the TS CLI's own progress and summary lines (counts and durations) reach the daemon's log, `daemon status` or the waiting client; every other line, such as an error that quotes a response body, is counted as withheld. Each line and a 10-second heartbeat keep a waiting `chatgpt sync` from timing out during a long TS sync, which is stopped after 10 minutes. Its failures show in `daemon status` and never fail the Rust sync.

## Which versions count

Whether a judgment, follow-up, Luna review, local title or memory classification is current depends on this build's versions (`Profile::builtin`: `QUESTIONS_VERSION`, `DEEP_QUESTIONS_VERSION`, `LUNA_JUDGMENT_VERSION`, `LOCAL_TITLE_VERSION`, `MEMORY_CLASSIFICATION_VERSION`), which are this repository's public TS sources' (`crates/daemon/src/classify/questions.rs` keeps them in step). Judgments made at another CLI's versions (the installed private TS checkout's) are simply stale, and `classify` makes them again (D9). `chatgpt daemon status` shows the versions.
