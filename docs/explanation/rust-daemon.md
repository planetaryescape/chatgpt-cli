# How the Rust CLI works

The Rust `chatgpt` (in `crates/`) is replacing the TS CLI one group of commands at a time. Until it covers everything, it hands the rest to the TS CLI unchanged.

## Native commands and the bridge

`sync`, `list`, `stats`, `export` (and its alias `show`), lexical `search`, `daemon` and `import-legacy` run in Rust. `search` with `--semantic`, `--hybrid` or `--remote`, `search-index`, and every other command, including its `--help`, run as `bun <cli.ts> <args…>`: the Rust process replaces itself with bun, so arguments, stdin, stdout, stderr, the terminal and the exit code are the TS CLI's. The TS CLI is found by path, never as `chatgpt` on PATH (which may be the Rust binary):

1. `CHATGPT_TS_CLI`, the TS CLI's `src/cli.ts`;
2. `~/.bun/install/global/node_modules/chatgpt-cli/src/cli.ts`, where `bun link` puts it.

Bun comes from PATH, else `~/.bun/bin/bun`. A bridged process carries `CHATGPT_BRIDGED=1`; a Rust `chatgpt` that sees it refuses to bridge again, so a misconfigured `CHATGPT_TS_CLI` can't loop.

## The daemon

Native commands ask a background daemon over a Unix socket. A native command that finds no daemon starts one (bridged commands don't need it), detached from the terminal, and waits until it answers. The daemon:

- reads the browser's cookies once per `--browser`/`--profile` choice (the read can raise a Keychain prompt) and keeps the access token in memory only. It reads them again only when ChatGPT rejects the token (401, or a 403 that isn't a Cloudflare challenge);
- syncs one account per index. A pass pins its browser choice and the session's account from start to finish, so a command choosing another browser meanwhile can't change what it reads. A sync whose session belongs to another account than the one the index was built from is refused: keep each account in its own instance (`CHATGPT_INSTANCE=<name>`);
- keeps its own index, `~/Library/Application Support/chatgpt-cli/chatgpt.db`, fresh (a daemon started on an index synced within the interval waits out the rest of it, so a cold `list` sends nothing; on an older index the first background pass starts at once, while `list` still answers from the index without waiting for it): a delta sync every 2 minutes while a command ran in the last 10 minutes, otherwise every 15. The archived-list sweep and the cache reconcile run at most hourly in the background, and on every `chatgpt sync`. `sync --full` is only ever run on request;
- backs off for as long as ChatGPT's rate limit asks (at least a minute, at most an hour) and shows it in `chatgpt daemon status`. A rate limit in any step, the cache reconcile included, ends the pass, and the TS sync waits for the next one;
- logs to `~/Library/Application Support/chatgpt-cli/logs/daemon.log.<date>`, one file a day, seven kept. Logs never hold cookies, tokens or response bodies.

`list` and `search` never touch the network. `stats` reads the saved memories live, as the TS CLI does. `export` always fetches the chat from the single-chat endpoint, as the TS CLI does, rather than using a cached transcript: the cache is filled from the batch endpoint, which doesn't name the model, so its header would differ.

## The search index

Lexical `search` needs no `search-index` step. The daemon keeps a full-text index (SQLite FTS5, built as the TS CLI's `src/search/` builds it) in its own database:

- When it starts, it chunks every cached transcript that's current for its chat. After every successful sync pass, and after an import, it also fetches the transcripts it lacks through the batch endpoint, 10 chats per call with half a second between calls, as `search-index` does. A daemon that has not yet synced in its lifetime fetches nothing, so a cold start reads no cookies.
- Each chat's transcript and chunks are written in one short transaction, so `list` and `search` never wait and an interrupted run continues where it stopped. Active, archived and pinned chats are all indexed; `search` filters by scope.
- It steps aside while a `sync` or `export` runs. A rate limit stops fetching for as long as ChatGPT asked (between a minute and an hour) without holding up syncs, a chat ChatGPT doesn't return (or whose batch it answers with an error) is asked for again after an hour, and a timeout or dropped connection ends the run until the next pass.
- `chatgpt daemon status` shows `search index: N of M chats indexed`, whether it's indexing, how many transcripts it fetched, and why it's waiting. While the index is incomplete, `search` answers from what's indexed and says `N of M chats indexed` on stderr.

The chunks reproduce the TS CLI's exactly, down to the bytes Bun stores when a chunk boundary splits an emoji, so ranking, scores and snippets match the TS CLI's over the same transcripts (`crates/cli/tests/parity_export_search.rs`). The TS CLI's own search index, in its own database, is never read or written.

A client keeps a running daemon that speaks its protocol and is at least as new as itself, and restarts an older one. `chatgpt daemon install` writes a LaunchAgent that starts the installed daemon at login (it doesn't load it; the command prints how).

Debug builds and binaries under `target/` use the `dev` instance (`chatgpt-cli-dev`), so a local build never touches the installed daemon. `CHATGPT_INSTANCE=<name>` picks another.

## Data from the TS CLI

While the bridge exists the TS CLI still writes every Jev judgment, follow-up, Luna review, local title, summary and cached transcript, into `~/.local/share/chatgpt-cli/index.db`. The daemon imports those tables into its own index at startup, after every TS sync and on `chatgpt import-legacy`. The import opens the TS index read-only, mirrors each table by its key (rows the TS index dropped are dropped), and keeps a newer `update_time` the daemon's reconcile wrote when nothing else differs. Transcripts are the exception: the search indexer caches them too, so the import never deletes one and replaces the daemon's copy only when that copy isn't current for the chat.

The TS index belongs to one account: whichever the TS CLI's default browser session holds. So the TS sync and the import run only for the installed (or dev) instance on a pass that reads the default browser choice, with no `--browser`, `--profile` or `CHATGPT_BROWSER*`. A named instance, or a pass on another browser, skips both and says so in `daemon status` ("TS sync skipped: …"), because a TS full sync from another account would replace the default account's chats in the shared index.

After a pass that's due for it (every 15 minutes, and on every `chatgpt sync`), the daemon also runs the TS CLI's own `sync`, niced, so bridged commands such as `classify` see the same chats. Only the TS CLI's own progress and summary lines (counts and durations) reach the daemon's log, `daemon status` or the waiting client; every other line, such as an error that quotes a response body, is counted as withheld. Each line and a 10-second heartbeat keep a waiting `chatgpt sync` from timing out during a long TS sync, which is stopped after 10 minutes. Its failures show in `daemon status` and never fail the Rust sync.

## Which versions count

Whether a judgment, follow-up, Luna review or local title is current depends on version constants in the TS sources (`QUESTIONS_VERSION`, `DEEP_QUESTIONS_VERSION`, `LUNA_JUDGMENT_VERSION`, `LOCAL_TITLE_VERSION`, `MEMORY_CLASSIFICATION_VERSION`) and on the topic names. Since the installed TS CLI writes the judgments, the daemon reads those constants, the topics and the employer topic from the TS CLI it bridges to. Without one it uses the values built in, which a test keeps equal to this repository's `src/`. `chatgpt daemon status` shows which it uses.
