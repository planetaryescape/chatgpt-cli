# Troubleshooting

Find the message you're seeing and follow its fix.

## macOS asks for Keychain access

The daemon reads the selected Chromium browser's **Safe Storage** Keychain entry to decrypt its cookies: when it starts, and again when ChatGPT rejects its token. Choose **Always Allow** in the prompt. If you chose **Allow**, it asks again next time. Firefox and Safari do not use this Keychain step.

## `Could not read "Dia Safe Storage" from the Keychain`

You denied the Keychain prompt, or Dia isn't installed. Run the command again and choose **Always Allow**. The error names the browser's Keychain item when you select another Chromium browser.

## `No ChatGPT session in <browser>`

The selected browser profile has no usable chatgpt.com session.

1. Open chatgpt.com in your browser and log in.
2. Run `chatgpt sync`. If you use several browsers or profiles, choose one: `chatgpt --browser chrome --profile "Profile 1" sync`.

## Safari cookies are blocked by macOS

Grant Full Disk Access to the terminal app running `chatgpt`, then retry `chatgpt --browser safari sync`. Safari stores cookies in its protected application container.

## `ChatGPT session has expired`

Open chatgpt.com in the browser named in the error so it refreshes the session cookie, then retry.

## `403 from /api/auth/session` or `Cloudflare kept challenging requests`

Cloudflare challenged the request. The client already retries on a fresh connection up to four times. If it still fails:

1. Wait a minute and retry.
2. If it keeps failing, Cloudflare or the API has changed. See [The chatgpt.com web API](../explanation/chatgpt-api.md#when-it-breaks).

## Rate limited by ChatGPT

```text
rate limited by ChatGPT; waiting 5s
```

Fetching single chats quickly triggers ChatGPT's rate limit. The client waits 5, 10, 20 and 40 seconds before giving up. Bulk commands (`classify`, `sync`) use the batch endpoint, which doesn't hit this limit. If `export` or the TUI hits it, wait a minute.

## `No local index yet. Run chatgpt sync first.`

Run:

```sh
chatgpt sync
```

## `No conversation matching "<id>" in the index`

The chat isn't in your local index. It's new, or it was deleted. Run `chatgpt sync`, or pass the full chat link, which doesn't need the index.

## `"<prefix>" matches N conversations`

Use a longer id prefix, or the full id.

## `TYPESAFE_API_KEY is not configured`

Chat and memory classification need a TypeSafe key. Run `chatgpt configure jev` or set an environment variable:

```sh
export TYPESAFE_API_KEY=<key>
```

## No summariser available or long chats skipped in step 3

Summaries need an OpenAI or Anthropic API key, `codex`, or `claude`. Luna's final review and local titles need an OpenAI API key or `codex`. Run `chatgpt configure` to inspect stored-key status. Short chats can receive Jev judgments while no Luna provider is available, but the command reports unfinished review or title generation.

## `No summariser succeeded. gpt-6-luna: … | claude-haiku: …`

Both summary providers failed for that chat. The message includes each one's error. Check configured API keys or CLI logins (`codex login`, `claude`), then re-run `chatgpt classify`; only the failed chats are retried.

## `ChatGPT returned a server error` on `rename`

Renaming chats from before 2025 returns a server error even though the rename usually takes effect. Check:

```sh
chatgpt sync
chatgpt list --title "<new title>"
```

## `Shared links (/share/…) aren't supported`

Open the shared chat in ChatGPT and use its own `/c/<id>` link.

## `--copy uses pbcopy and only works on macOS`

Use `-o <file>` instead of `--copy`.

## The TUI shows `Loading transcript…` for a while

The chat's transcript isn't cached yet, so the TUI fetches it. If it stays, you've probably hit the rate limit; wait a minute. The daemon caches every transcript in the background after each sync; `chatgpt search-index` shows how far it has got.

## Counts look wrong after deleting in the browser

Run `chatgpt sync --full`. A normal sync can't see deletions made outside `chatgpt`.

## `this daemon doesn't know that request`

A `chatgpt` and a daemon from different releases are talking, and the daemon doesn't have what the command asked for. Run `chatgpt daemon stop`; the next command starts the daemon that matches it. Commands from 0.1.5 that no longer exist, such as `import-legacy`, get this message from a newer daemon.

## `unrecognized subcommand` (exit code 2)

The command doesn't exist in this release. `chatgpt --help` lists the commands. `import-legacy` was removed with the TS CLI's index after 0.1.5.

## Semantic search says `N of M chunks embedded`

The daemon is still embedding, for example after the first sync or after an upgrade rebuilt the search passages. Semantic and hybrid search answer from what's embedded so far; lexical search is complete. `chatgpt daemon status` shows progress, and `chatgpt search-index` waits until it's done.

## `chatgpt` still runs the old TS CLI

An old `bun link` left a `chatgpt` in `~/.bun/bin`, earlier on your PATH than `~/.local/bin`. Remove it (`rm ~/.bun/bin/chatgpt`) or put `~/.local/bin` first; `install.sh` warns about this. The TS CLI's database, `~/.local/share/chatgpt-cli/index.db`, is no longer read and can be deleted.
