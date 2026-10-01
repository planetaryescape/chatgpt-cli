# How the Rust CLI works

The Rust `chatgpt` (in `crates/`) is replacing the TS CLI one group of commands at a time. Until it covers everything, it hands the rest to the TS CLI unchanged.

## Native commands and the bridge

`sync`, `list`, `stats`, `daemon` and `import-legacy` run in Rust. Every other command, including its `--help`, runs as `bun <cli.ts> <args…>`: the Rust process replaces itself with bun, so arguments, stdin, stdout, stderr, the terminal and the exit code are the TS CLI's. The TS CLI is found by path, never as `chatgpt` on PATH (which may be the Rust binary):

1. `CHATGPT_TS_CLI`, the TS CLI's `src/cli.ts`;
2. `~/.bun/install/global/node_modules/chatgpt-cli/src/cli.ts`, where `bun link` puts it.

Bun comes from PATH, else `~/.bun/bin/bun`. A bridged process carries `CHATGPT_BRIDGED=1`; a Rust `chatgpt` that sees it refuses to bridge again, so a misconfigured `CHATGPT_TS_CLI` can't loop.

## The daemon

Native commands ask a background daemon over a Unix socket. Any command that finds no daemon starts one, detached from the terminal, and waits until it answers. The daemon:

- reads the browser's cookies once (the read can raise a Keychain prompt) and keeps the access token in memory only. It reads them again only when ChatGPT rejects the token (401, or a 403 that isn't a Cloudflare challenge), or when a command passes another `--browser`/`--profile`;
- keeps its own index, `~/Library/Application Support/chatgpt-cli/chatgpt.db`, fresh: a delta sync every 2 minutes while a command ran in the last 10 minutes, otherwise every 15. The archived-list sweep and the cache reconcile run at most hourly in the background, and on every `chatgpt sync`. `sync --full` is only ever run on request;
- backs off for as long as ChatGPT's rate limit asks (at least a minute, at most an hour) and shows it in `chatgpt daemon status`;
- logs to `~/Library/Application Support/chatgpt-cli/logs/daemon.log.<date>`, one file a day, seven kept. Logs never hold cookies, tokens or response bodies.

`list` never touches the network. `stats` reads the saved memories live, as the TS CLI does.

A client keeps a running daemon that speaks its protocol and is at least as new as itself, and restarts an older one. `chatgpt daemon install` writes a LaunchAgent that starts the installed daemon at login (it doesn't load it; the command prints how).

Debug builds and binaries under `target/` use the `dev` instance (`chatgpt-cli-dev`), so a local build never touches the installed daemon. `CHATGPT_INSTANCE=<name>` picks another.

## Data from the TS CLI

While the bridge exists the TS CLI still writes every Jev judgment, follow-up, Luna review, local title, summary and cached transcript, into `~/.local/share/chatgpt-cli/index.db`. The daemon imports those tables into its own index at startup, after every TS sync and on `chatgpt import-legacy`. The import opens the TS index read-only, mirrors each table by its key (rows the TS index dropped are dropped), and keeps a newer `update_time` the daemon's reconcile wrote when nothing else differs.

After a pass that's due for it (every 15 minutes, and on every `chatgpt sync`), the daemon also runs the TS CLI's own `sync`, niced, so bridged commands such as `classify` see the same chats. Its output goes to the daemon's log with response bodies redacted. Its failures show in `daemon status` and never fail the Rust sync.

## Which versions count

Whether a judgment, follow-up, Luna review or local title is current depends on version constants in the TS sources (`QUESTIONS_VERSION`, `DEEP_QUESTIONS_VERSION`, `LUNA_JUDGMENT_VERSION`, `LOCAL_TITLE_VERSION`, `MEMORY_CLASSIFICATION_VERSION`) and on the topic names. Since the installed TS CLI writes the judgments, the daemon reads those constants, the topics and the employer topic from the TS CLI it bridges to. Without one it uses the values built in, which a test keeps equal to this repository's `src/`. `chatgpt daemon status` shows which it uses.
