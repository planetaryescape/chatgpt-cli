# chatgpt-cli

> **Unofficial API disclaimer:** This CLI uses private ChatGPT web app APIs discovered by reverse engineering. It works with the current web app as of 28 September 2026, but OpenAI can change those APIs without notice. There is no guarantee it will keep working. This project is not affiliated with or endorsed by OpenAI. Contributions that help keep it working are welcome.

Search, export and manage your ChatGPT history from the terminal. `chatgpt` uses your existing browser session on macOS, keeps a local SQLite index, and offers a terminal UI for reviewing conversations. Search can use full text, a local embedding model, or both. Optional Jev and Luna classification suggests what to keep, archive or delete.

## Quick start

You need macOS and a browser logged in to [chatgpt.com](https://chatgpt.com). Supported browsers are Safari, Chrome, Firefox, Dia, Arc, Brave and Edge. Install the latest release into `~/.local/bin`:

```sh
curl -fsSL https://raw.githubusercontent.com/planetaryescape/chatgpt-cli/main/install.sh | sh
chatgpt sync
chatgpt list --limit 10
```

The installer checks the release's SHA-256 before installing, and warns when another `chatgpt` comes earlier on your PATH. To pick a release or a place, pipe into `sh -s -- --version v0.1.5 --prefix <dir>` (installs into `<dir>/bin`).

The CLI uses your macOS default browser. To choose another browser or profile, put the flags before the command: `chatgpt --browser chrome --profile "Profile 1" sync`. `CHATGPT_BROWSER` and `CHATGPT_BROWSER_PROFILE` also work. Chromium browsers may ask for access to their **Safe Storage** Keychain item; Safari may need Full Disk Access for your terminal. Commands operate on active chats by default. Add `--archived` or `--all` when you intend to include archived chats.

Every command talks to a background daemon, which the first command starts. It reads your browser session once and keeps the token in memory (no Keychain prompt per command), keeps the local index, the search index and its embeddings fresh, and judges new chats with Jev once you configure a Jev key. `list` and `search` answer from the index without touching the network. `chatgpt daemon status` shows what it's doing, and `chatgpt daemon install` starts it at login. See [how it works](docs/explanation/rust-daemon.md).

## What you can do

```sh
chatgpt search "garden lighting"                      # full-text search, indexed in the background
chatgpt search-index                                  # wait for the search index to catch up
chatgpt search --semantic "ideas for a small garden"   # local embedding search
chatgpt search --hybrid "garden lighting"              # combine both
chatgpt export <chat-id> > conversation.md            # export a chat as Markdown
chatgpt project list                                  # inspect ChatGPT projects
chatgpt memory list --limit 10                        # read saved memories
```

Search, list and memory commands offer pipeable `--format json`, `csv` and `ids` output where applicable. For example:

```sh
chatgpt search "garden lighting" --format json | jq -r '.[].title'
```

The search index downloads a local embedding model on first use. Conversation text and search queries stay on your machine during embedding. [Search your history](docs/how-to/search-history.md) covers indexing, ranking and output formats.

### Review and clean up

Classification is optional. It requires a [TypeSafe](https://typesafe.ai) API key for Jev, plus either an OpenAI API key or the `codex` CLI for Luna review and titles. Configure keys with `chatgpt configure jev` and, if needed, `chatgpt configure openai`. An Anthropic key or `claude` can serve as a fallback for long-chat summaries. Model API usage may incur charges.

```sh
chatgpt classify                         # judge new or changed chats; deepen unsure cases
chatgpt stats                            # see suggestions and topic counts
chatgpt tui                              # read and decide one chat at a time
chatgpt archive --suggest archive --dry-run
chatgpt delete --suggest delete --dry-run
```

Suggestions are advisory. The dry runs show the selected chats before anything changes; the real archive and delete commands ask for confirmation. Deleting a chat is irreversible. Saved memories have a separate `chatgpt memory classify` command and are never deleted by chat cleanup. See [your first cleanup](docs/tutorial.md), [bulk cleanup](docs/how-to/clean-up-with-jev.md) and [saved memories](docs/how-to/manage-memories.md).

## Data and privacy

The CLI reads the ChatGPT session from your browser and exchanges it for an access token in memory. The daemon stores the conversation index, cached transcripts, judgments and search vectors in `~/Library/Application Support/chatgpt-cli/chatgpt.db`; optional model keys live in `~/.config/chatgpt-cli/config.json`. Neither location belongs in Git.

Local embedding does not send transcript text to Hugging Face. Classification and summarisation **do** send the relevant chat or saved-memory content to the configured model providers. Read [data, files and credentials](docs/reference/data.md) before using those features on sensitive conversations.

## Documentation

- [CLI commands and flags](docs/reference/cli.md)
- [TUI keys](docs/reference/tui.md)
- [Configuration](docs/how-to/configure.md)
- [Exporting conversations](docs/how-to/export-a-conversation.md)
- [Moving chats into projects](docs/how-to/add-chats-to-a-project.md)
- [How the private API was observed](docs/explanation/chatgpt-api.md)
- [Maintainer guide](docs/maintainers.md)

## Contributing

Bug reports and pull requests are welcome, especially when a ChatGPT web app change breaks an observed endpoint. Include the command, expected and actual behaviour, and a redacted response shape or error. Keep cookies, tokens, HAR files and conversation text out of issues and commits.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo nextest run
```

Tests use fixtures and do not need a ChatGPT account. API, rendering and classification changes also need a live check against your own account; record the observed behaviour without publishing private data. See the [maintainer guide](docs/maintainers.md) for the change procedure.
