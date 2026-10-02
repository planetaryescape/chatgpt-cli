# How the CLI works

`chatgpt` is a Rust CLI (`crates/`) and a background daemon it talks to. Every command asks the daemon over a Unix socket and prints its answer; `configure` only writes the user config, and `tui` is a client of the daemon of its own (`crates/tui`, ratatui on crossterm): the chats come from `List`, transcripts from `Transcript`, local titles go through `SetTitle`, and the marks are applied through `Mutate`, exactly as `archive -y` and `delete -y` apply them (the index's pinned account, the sync pass lock, no resend of a delete). `review` is a CLI command over `Select`, `List` (for the verdicts), `Transcript` and `Mutate`. An unknown command exits 2 with clap's usage error.

The CLI started as a TypeScript one, and the Rust port replaced it command by command, matching its output; [Matching the TS CLI](#matching-the-ts-cli) says what's pinned and what changed.

## The daemon

A command that finds no daemon starts one (`configure` doesn't need it), detached from the terminal, and waits until it answers. The daemon:

- reads the browser's cookies once per `--browser`/`--profile` choice (the read can raise a Keychain prompt) and keeps the access token in memory only. It reads them again only when ChatGPT rejects the token (401, or a 403 that isn't a Cloudflare challenge). A choice nobody has used for an hour gives up its session, except the one sync passes use, and a choice whose read failed keeps nothing. While a read waits (on a Keychain prompt, say), `chatgpt daemon status` says `session: reading session…`;
- syncs one account per index. A pass pins its browser choice and the session's account from start to finish, so a command choosing another browser meanwhile can't change what it reads. A sync whose session belongs to another account than the one the index was built from is refused: keep each account in its own instance (`CHATGPT_INSTANCE=<name>`);
- keeps its own index, `~/Library/Application Support/chatgpt-cli/chatgpt.db`, fresh (a daemon started on an index synced within the interval waits out the rest of it, so a cold `list` sends nothing; on an older index the first background pass starts at once, while `list` still answers from the index without waiting for it): a delta sync every 2 minutes while a command ran in the last 10 minutes, otherwise every 15. The archived-list sweep and the cache reconcile run at most hourly in the background, and on every `chatgpt sync`;
- runs a full sync about once a day, in the background, once nobody has used the CLI for 10 minutes and the last full sync is a day old. It's the only pass that drops a chat deleted while active. It checks at most 50 chats the lists left out one by one, a quarter second apart; with more, it leaves the index as it was and says to run `chatgpt sync --full`, which checks them all. A rate limit puts it off another day, any other failure an hour. `chatgpt daemon status` shows when the last full sync ran and when the next may (`full sync: …`). A full sync, either kind, lists a chat list again (up to twice, while that finds chats) when the listing repeated chats or came back more than 5 chats shorter than the index's count for that list, before checking the missing chats one by one;
- backs off for as long as ChatGPT's rate limit asks (at least a minute, at most an hour) and shows it in `chatgpt daemon status`. A rate limit in any step, the cache reconcile included, ends the pass;
- logs to `~/Library/Application Support/chatgpt-cli/logs/daemon.log.<date>`, one file a day, seven kept. Logs never hold cookies, tokens or response bodies.

`list` and `search` never touch the network, except `search --remote`, which asks ChatGPT's search because you asked for it. `stats` reads the saved memories live, with the session pinned to the index's account: another account's memories are refused (`Saved-memory stats unavailable: …`) rather than counted against this index's classifications. `export` always fetches the chat from the single-chat endpoint rather than using a cached transcript: the cache is filled from the batch endpoint, which doesn't name the model, so its header would differ. Any answer too large for one IPC frame (16 MiB), an export of a very long chat say, goes out as numbered `Part` events holding slices of its JSON, then `Parted` with their count and total size; the client refuses a missing, repeated or reordered part or a size that doesn't add up, before it reads anything, then joins them and reads the answer as if it had come whole, so an export of any size is written byte for byte as a small one is.

`Transcript` gives the TUI's preview and `review` a chat's transcript as the cache holds it (the batch endpoint's rendering): from the cache alone, from the cache else the batch endpoint (the TUI), or always from the batch endpoint (`review`). A fetched transcript is cached with its search chunks under the sync pass lock, as the index's account fetched it, and only while the index still has the chat at the `update_time` it was fetched for: a fetch a sync overtook is shown but never cached over the newer revision. With it come the summary a long chat's current judgment was made from (also when the fetch fails, which the answer says rather than failing), and for `review` the turn count and the first and last turns.

## The search index

Lexical `search` needs no `search-index` step. The daemon keeps a full-text index (SQLite FTS5) in its own database:

- When it starts, it chunks every cached transcript that's current for its chat. After every successful sync pass it also fetches the transcripts it lacks through the batch endpoint, 10 chats per call with half a second between calls, as `search-index` does too. The indexer itself fetches nothing until the daemon's first successful pass, so it never reads cookies on its own (the startup sync of an empty or stale index still does).
- Chunks count as current only while they were built from the transcript the cache holds: any write that brings a transcript new content drops that chat's chunks in the same transaction (triggers in `crates/store/migrations/0003_search_follows_transcripts.sql`), so the cache reconcile can only move forward chunks of the content it verified.
- Before the indexer replaces a transcript that is only older than its chat, it runs the cache reconcile's check on it: unchanged content moves every cache (judgments, summaries, titles, chunks) forward, as `sync` would have; changed content replaces the transcript and leaves the rest stale.
- A chat's transcript is cut into chunks of at most 800 UTF-16 code units, overlapping by 80, preferably at a line break or space and never inside an emoji. Raising `CHUNK_VERSION` (`crates/daemon/src/search/chunks.rs`) makes every chunk stale: the indexer rebuilds them from the cached transcripts, without fetching anything.
- Each chat's transcript and chunks are written in one short transaction, so `list` and `search` never wait and an interrupted run continues where it stopped. Active, archived and pinned chats are all indexed; `search` filters by scope.
- It steps aside while a `sync` or `export` runs. A rate limit stops fetching for as long as ChatGPT asked (between a minute and an hour) without holding up syncs, a chat ChatGPT doesn't return (or whose batch it answers with an error) is asked for again after an hour, and a timeout or dropped connection ends the run until the next pass.
- `chatgpt daemon status` shows `search index: N of M chats indexed`, whether it's indexing, how many transcripts it fetched, and why it's waiting. While the index is incomplete, `search` answers from what's indexed and says `N of M chats indexed` on stderr.

## Embeddings

Semantic and hybrid `search` need no `search-index` step either. After the indexer writes chunks, the daemon's embedder gives each chunk that's current for its chat a 384-dimension vector, with the model the TS CLI used, in a different runtime ([why, with measurements](embeddings.md)):

- The model runs in a worker process, `chatgpt daemon embed-worker`, started under macOS's utility QoS (`taskpolicy -c utility`) on one thread, so embedding uses at most one core and gives way to your work. The worker is stopped after ten idle minutes, which unloads the model.
- One chunk at a time, saved 16 at a time, so an interrupted run resumes where it stopped. It steps aside while a `sync` or `export` runs, and a semantic search's query goes before the next chunk.
- Each text gets a minute. A worker that takes longer is stopped and replaced, and that text (or query) fails with a clear error, so a hung worker can't hold up semantic search. A worker whose answer was abandoned midway (a client that went away) is replaced too, so its late answer can never be read as another text's vector.
- Every run checks the model is in the model cache, `~/.cache/chatgpt-cli/models`, and downloads what's missing (23 MB, checked against a pinned SHA-256), even when every chunk already has its vector, since queries need the model too. When the download fails, lexical search is unaffected; `daemon status` and semantic search say why, and the daemon tries again 15 minutes later on its own. A search never downloads anything; one that finds the model gone, or damaged (the worker checks its SHA-256 when it loads it), says so and wakes the embedder to fetch it again, at most once per retry window.
- A chunk's vector is deleted with it (a trigger in `crates/store/migrations/0004_search_vectors.sql`), and a vector is saved only if its chunk still holds the text it was made from.
- A search reads in one snapshot (a read transaction), so a hit's chat, score, scope and snippet always belong together even while the indexer replaces chunks.
- `chatgpt daemon status` shows `embeddings: N of M chunks embedded` and why it waits. While embedding is incomplete, semantic and hybrid search answer from the embedded chunks and say `N of M chunks embedded` on stderr; with none yet, they say so instead of answering.

`search-index` asks the indexer to fetch now (without waiting for a sync) and the embedder to retry a failed download, shows both making progress, and reports on its scope (`--archived`, `--all`) once neither has work left. It exits 1 if a chat couldn't be fetched, the index is still incomplete, or the model is unavailable.

The vectors are tagged with the Rust `MODEL_VERSION` and live only in the daemon's index. An index from 0.1.1 gains the vectors table on the daemon's first start, and the existing chunks are embedded in the background while `list` and `search` keep answering.

## Changing chats, projects and memories

`archive`, `unarchive`, `delete`, `project add` and `project remove` run in three steps, so the chats a preview shows are exactly the ones changed:

1. The daemon resolves the chats from the ids, id prefixes, the ids on stdin (`-`), or the filters (pinned chats only with `--pinned`, archived ones only with `--archived` or `--all`; `unarchive` picks archived chats). It never touches the network for this.
2. The CLI prints the preview (the first 25, then how many more), stops for `-n`, and otherwise asks: `[y/N]`, or the typed count for `delete` and `memory delete`. The answer comes from the terminal; when the ids came in on stdin, from `/dev/tty`. Without a terminal to ask, it fails and says to pass `-y`.
3. The CLI sends back the ids it showed, and the daemon changes each one in ChatGPT first, then in its index, a quarter second apart (three deletes at once) and progress lines. A delete drops the chat's row, and its search chunks and vectors go with the indexer's next run; an archive change flips `is_archived`; a rename changes the title (the indexer rebuilds the chat's chunks); a project move sets `project_id`. If the CLI goes away (Ctrl-C), the chat in hand is finished, ChatGPT and the index both, and no further one is started. A change and a sync pass never run at the same time: a change waits for a running pass (and the pass for the change), so a pass that listed a chat before the change can't write its old state back. Every change, and every project and memory read, uses the session pinned to the index's account: a session for another account, or one renewed into another, is refused before anything is sent.

ChatGPT's quirks, as observed: a pre-2025 chat's rename answers 500 yet applies (the error says so), a project move answering 500 is checked with a read of the chat, a delete answering 404 has done its job, and a memory delete counts only when ChatGPT answers `success: true`.

`project delete` has one target, so it skips the per-chat steps: the CLI resolves the project from `project list`, counts its chats from the index, and asks for the project's name. The daemon then deletes it under the same pass lock and pinned session, and clears `project_id` for its chats in the index. It counts only when ChatGPT answers `deleted: true`.

Archiving is idempotent, so it's retried like a read. A delete, a rename, a project move, a new or deleted project and a memory delete mustn't happen twice: they're retried only after an answer that shows ChatGPT turned them away (a Cloudflare challenge, a 429). After a gateway error or a dropped connection the write may have applied, and the CLI says exactly that, with the path, and that `chatgpt sync` shows whether it did, rather than sending it again.

`title` (and the TUI's `n`) writes a manual local title to the daemon's index only.

### The Jev guard

`archive`/`delete --check`, and `--suggest delete` on `delete` (or `--suggest archive` on `archive`), judge each selected chat that has no current judgment, during the command: `classify`'s first pass (below), with its steps, notes, summaries for long chats, the question before a large batch of them (`-y` skips it) and cost lines, and only chats Jev confidently backs are kept. Anything it couldn't judge is held back, and so is a chat whose `update_time` changed while it was being judged (a sync pass ran meanwhile): its verdict would rest on old content.

An answer of the wrong shape (a score that isn't a number in its range, a choice that isn't one of the question's) is never saved; the failure names the question, never the value. A stored judgment that can't be read counts as no judgment, in `list`, `stats` and the guard, which judges it again.

## Classification

`classify`, `titles`, `memory classify` and the guard run in the daemon (`crates/daemon/src/classify/`), ported from the TS CLI's classifier with its models, prompts, flags, fallback order, pacing, notes and cost lines, at this build's question and policy versions (`Profile::builtin`). The CLI resolves the chats first, as for `archive`, then hands their ids over; each run has its own task, so what it paid for is saved even if the client goes away, while a client that went away (Ctrl-C) stops it from starting further calls.

- **Jev** (TypeSafe's System One, `crates/typesafe`): the first pass, the follow-up for unsure chats (`classify` only) and the saved-memory quick pass.
- **Summaries** of chats over 12,000 tokens, in this order (each provider's client is built when it's reached, so a broken key fails that provider alone): OpenAI's API with an OpenAI key, else `codex exec -m gpt-6-luna`; then Anthropic's API (`claude-haiku-4-5`) with an Anthropic key, else `claude -p --model haiku`. `crates/model-api` calls the APIs, once each, with a 10-minute limit. Before summarising more than 500k tokens without `-y`, the daemon asks the client (`Go ahead? [y/N] `) and waits for its answer.
- **Luna** (`gpt-6-luna`): OpenAI's API with a strict JSON schema, else `codex exec --output-schema`. It reviews what Jev can't settle, writes local titles, and reviews saved memories Jev couldn't keep. Its answers are checked before anything is saved.

Keys come from the requesting command's environment (`TYPESAFE_API_KEY`, `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`), which it passes on with the request, else from the user config, read on every call; never from the daemon's own environment. The same goes for `PATH`, where `codex` and `claude` are found: a daemon started at login has a bare one. `codex` and `claude` run in an environment holding only `HOME`, the locale, `TMPDIR` and their own config directories (none of our keys), in their own process group, which is killed after 10 minutes or when the run is dropped; their output is captured and never logged or shown, so a failure says only how they exited.

Every judgment, follow-up, Luna review, summary, title, memory classification and downloaded transcript is saved as soon as it's made, under the sync pass lock (briefly), so no pass rewrites the chat or moves its caches forward halfway through a save. A result lands only if the chat's cached transcript is still the one the model was given, and then at the chat's current `update_time`: a sync that moved an unchanged chat's caches forward meanwhile neither hides the result nor has it displace newer rows, and a result from content that changed meanwhile isn't saved (the failure says the chat changed while it was being classified). Each stage reads the chats afresh. A saved memory's late result is dropped when another run saved that memory for other input after this run started. A first pass saved again for the same chat and question version keeps that judgment's follow-up and Luna review; only a newer judgment drops them. A Luna title never replaces a manual one, even one set while Luna was answering.

`classify` and the guard hold their chats for the whole run, after waiting for the background Jev to finish any of them, and the background Jev skips chats a command holds: no chat is judged by two runs at once. Every new summary passes the same question before more than 500k tokens (`-y`, or the client's yes), the follow-up's included. A client that can't answer one (it says so with `can_answer`) is never asked: the run fails before spending, saying to pass `-y`. The daemon reads from the client's connection while any request runs, so a client that went away is noticed at once, and no paid call (a model API, `codex`, `claude`, Jev, the next summariser in the fallback) starts after it.

`configure` writes `~/.config/chatgpt-cli/config.json` itself (0600, through a rename), keeping the other providers' keys and the `auto_jev` setting.

### Jev in the background

After each successful sync pass, the daemon judges chats that are new or changed since it was first enabled and have no current judgment, as a bare `chatgpt classify` would pick them (active, unpinned), newest first, at most 50 a pass, with `classify`'s pacing. It runs Jev's first pass only: never the follow-up, Luna or a summary; a long chat without a cached summary waits for a `classify` someone runs. It needs a Jev key in the user config (a key in a command's environment doesn't count: no command asked for it), and `"auto_jev": false` there switches it off. The first time it runs, it notes the newest chat's time, so enabling it never judges the whole history. A chat Jev fails on waits six hours before another try. `chatgpt daemon status` shows whether it's on (and why not), what it judged and spent today (UTC), and its last run.

## Clients, daemons and instances

A client keeps a running daemon that speaks its protocol and is at least as new as itself, and restarts an older one. It stops only the daemon it found too old (compare-and-stop on its PID, asked on the connection that named it or signalled after checking its start time), so when two clients restart the same old daemon at once, the second leaves the first one's new daemon running and uses it. Whichever way the daemon is found when it comes to stopping it (answering, timing out, or not listening yet), only the PID and start time first observed are stopped; anything else is judged afresh. A daemon that took the lock but isn't answering 15 seconds after a client started it (stuck opening its index) is stopped, but only the one first seen holding the lock during that wait, and a new one started once. A request a daemon doesn't know (one this build retired, such as 0.1.5's `import_legacy`) gets an error at once, never a hang. `chatgpt daemon install` writes a LaunchAgent that starts the installed daemon at login (it doesn't load it; the command prints how).

Debug builds and binaries in a Cargo build directory (`target/`, or wherever `CARGO_TARGET_DIR` points: Cargo's `.fingerprint` directory sits beside them) use the `dev` instance (`chatgpt-cli-dev`), so a local build never touches the installed daemon. `CHATGPT_INSTANCE=<name>` picks another.

## Which versions count

Whether a judgment, follow-up, Luna review, local title or memory classification is current depends on this build's versions (`Profile::builtin` in `crates/daemon/src/policy/profile.rs`: the questions, follow-up questions, Luna review, local title and memory classification versions). Judgments made at other versions (an older build's, or the TS CLI's) are stale, and `classify` makes them again. `chatgpt daemon status` shows the versions.

## Upgrading from 0.1.5

0.1.5 was the last release with the TS CLI in the repository. Opening its index, the daemon:

- keeps every chat, judgment, follow-up, Luna review, local title, summary, memory classification and cached transcript as it was;
- drops the `native_rows` table (migration 6), which only the import from the TS CLI's index read;
- rebuilds every search chunk at `CHUNK_VERSION` 2 from the cached transcripts, without the network, then embeds the new chunks. On a large history (about 47,000 chunks) the embedding takes about half an hour on one core. Lexical search answers throughout; semantic and hybrid search answer from what's embedded so far and say `N of M chunks embedded`.

The TS CLI's own index, `~/.local/share/chatgpt-cli/index.db`, is no longer read by anything. Delete it when you no longer want it.

## Matching the TS CLI

The Rust port matched the TS CLI's output byte for byte: list and stats, export, every search mode, the changes and their prompts, and classification down to the model requests. The parity harnesses that proved it ran both CLIs and live in tag `v0.1.5`. What the TS CLI printed in their last run is pinned in `crates/cli/tests/golden/` ([maintainer guide](../maintainers.md#output-pinned-from-the-ts-cli)).

Changed on purpose since then:

- Search chunks and snippets never hold half an emoji. The TS CLI cut chunks and snippets at UTF-16 offsets, which could split an emoji's surrogate pair, so a snippet could end in `U+FFFD` as text or `\ud83d` in JSON, or start with a stray character. Now the cut moves to keep the pair whole.
- `import-legacy` is gone with the TS CLI's index; like any unknown command it exits 2.
