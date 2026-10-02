# How it works

How `chatgpt` gets from your browser login to a delete suggestion, and why each piece is built the way it is.

## The path of a request

```text
Browser cookie store ──▶ session cookie ──▶ /api/auth/session ──▶ bearer token
                                                                      │
                             impit (Chrome TLS fingerprint) ◀─────────┘
                                       │
                                chatgpt.com/backend-api
```

1. **`crates/chatgpt/src/cookies/`** selects the macOS default browser, or one named with `--browser`. Chromium profiles use a read-only SQLite connection and decrypt chatgpt.com cookies with a key derived from that browser's Safe Storage Keychain password. Firefox reads its SQLite cookie store; Safari reads its binary cookie store. `--profile` selects a specific Chromium or Firefox profile. There is no separate sign-in or stored ChatGPT token.
2. **`crates/chatgpt/src/http.rs`** and the session exchange turn the session cookie into a bearer token, which the daemon keeps in memory, then call the API. Every request goes through `impit`, an HTTP client that impersonates Chrome's TLS fingerprint. Plain `fetch` and `curl` are challenged by Cloudflare, and so is headless Chrome. A visible browser gets through, but opening one for every command isn't workable. It also retries Cloudflare challenges on a fresh connection, and 429s and gateway errors with backoff.
3. **`crates/daemon/src/api.rs`** (and `api/writes.rs`) holds one function per endpoint. [The chatgpt.com web API](chatgpt-api.md) records each endpoint's observed behaviour.

## The local index

The daemon (`crates/daemon`) keeps one row per chat in its SQLite index (`crates/store`), `~/Library/Application Support/chatgpt-cli/chatgpt.db`. Filters (`--older-than`, `--title`, …) run against it, so listing a large history is fast and works offline. Every command asks the daemon, which a command starts when none is running, and the daemon syncs in the background: every 2 minutes while you're using the CLI, otherwise every 15. [How the CLI works](rust-daemon.md) covers the daemon.

A sync is a delta by default. It reads the most recent chats until it reaches ones it already has. It also reads the full archived list (hourly in the background, and on every `chatgpt sync`), because archiving doesn't change a chat's update time. Chats that drop off the archived list are checked one by one before the index changes. `sync --full` rebuilds everything, which is the only way to notice chats deleted in the ChatGPT app.

## Searching locally

The daemon's indexer (`crates/daemon/src/search/`) fetches transcripts it lacks after each sync and splits them into short overlapping passages. SQLite FTS5 indexes each passage's title and text, so plain `search` ranks exact-word matches without loading a model. The embedder then computes a 384-dimensional embedding for each passage with a local quantized MiniLM model, in a worker process limited to one core. Active, archived and pinned chats are all indexed; `search` filters by scope. `search-index` only waits for both to catch up. `search --semantic` compares a query embedding with the stored vectors; `--hybrid` merges text and semantic ranks. The vectors live in the same SQLite database as the transcript cache, avoiding a second index to synchronize.

Both search paths join against the current conversation update time, so changed or removed chats do not appear with stale text. Reindexing replaces changed passages, and embedding work is saved in batches so an interrupted run resumes. [Search your history](../how-to/search-history.md) has the commands and model details.

## Rendering a transcript

A ChatGPT conversation is a tree: editing or regenerating a message creates a branch. `crates/daemon/src/render.rs` walks from `current_node` up to the root, which gives the thread you see in the UI, and renders it as markdown. It handles voice transcripts, images, generated images, file attachments, web-citation markers and canvas documents. Canvas edits are regex replacements, so it replays them to reach the final document.

## Classifying

Classification runs in the daemon. `crates/daemon/src/classify/pipeline.rs` runs three steps:

1. **Download.** Transcripts come from `POST /conversations/batch`, 10 per call. Single-chat fetches get rate-limited within minutes; the batch endpoint doesn't at this pace. Each batch is cached as it arrives.
2. **Judge short chats.** Chats up to ~12,000 tokens go to Jev whole, four at a time.
3. **Summarise and judge long chats.** Longer chats are first summarised by Codex (`gpt-6-luna`), or by `claude -p` with Haiku if Codex fails, then judged from the summary. Summarising beats cutting the transcript short, which would lose how the chat ended. Jev's accuracy also drops as its input grows, which is why the cutoff is well under its 32k-token limit.

Short chats go first so results appear in seconds, and each long chat is summarised and judged in one go so an interrupted run keeps everything it finished.

After those steps, `classify/deep.rs` asks Jev a second set of questions about every unsure result in the selected chats. It reuses cached transcripts and summaries, and caches the follow-up answers separately. `classify/review.rs` sends any still unsure chat, plus borderline or conflicting product-brainstorm labels and possible time-expired chats with conflicting evidence, to Luna for a deeper review. An incomplete final review leaves an unsure result marked `?`. A later `classify` resumes missing reviews even if the regular judgment was cached. A chat with a finite purpose that has not yet expired is re-judged after seven UTC days when `classify` runs; strong evidence of a lasting personal record delays that recheck until a later UTC month. Age alone never changes a suggestion.

`classify/titles.rs` uses Luna to give each chat a local display title and an open-ended theme from its transcript or summary. The title appears in the CLI and TUI while the ChatGPT title remains in `conversations`. A manual local title takes precedence; a generated title is regenerated when the chat changes.

Every Jev call and summary adds to a cost meter (`classify/costs.rs`). The run ends with a breakdown separating billed spend (Jev) from subscription-covered spend shown at API prices (summaries).

Jev is TypeSafe's System One model. It answers typed questions with probabilities rather than generating text. The regular judgment sends one request per chat; an unsure result gets a second request containing the follow-up questions. Luna supplies the slower judgment when those answers cannot settle the decision.

## From answers to suggestions

`crates/daemon/src/policy/` turns Jev's raw probabilities into delete, archive or keep. The database stores the regular and follow-up raw answers, and the rules run every time suggestions are read. So changing a threshold never requires another run, and every command and the TUI agree. [Classification reference](../reference/classification.md) lists the rules.

## Caching and versions

Every cached value records the chat's `update_time` and a version constant:

| Cache | Version constant |
|---|---|
| Transcript | `render_version` in `Profile::builtin` (`crates/daemon/src/policy/profile.rs`) |
| Search passages | `CHUNK_VERSION` in `crates/daemon/src/search/chunks.rs`, with the transcript's render version |
| Summary | `SUMMARY_PROMPT_VERSION` in `crates/daemon/src/classify/summarise.rs` |
| Judgment | `questions_version` in `Profile::builtin` |
| Follow-up judgment | `deep_questions_version`, tied to the regular questions version |
| Luna final review | `luna_version`, tied to the Jev judgment and follow-up version |
| Local display title | `local_title_version` |

A chat that changed, or a version that moved, makes the cached row stale, and the next `classify` redoes it. That's how "judge only what's new" works without separate bookkeeping.

## Acting on chats

The daemon (`crates/daemon/src/mutate.rs`) updates ChatGPT before the local index. Bulk delete sends up to three requests at a time, with a 250ms pause per worker. A 404 on delete means the chat is already gone, so the index is updated and the retry counts as complete. Bulk commands preview first and need confirmation: `y` for archive, and the typed count for delete. The TUI collects marks and applies them only after you type `apply`.

## Why a CLI and a TUI

The CLI suits bulk work you trust and anything you pipe into another command. The TUI (ratatui, in `crates/tui`) suits reading and deciding one chat at a time. Both ask the same daemon, so they share the index, the policy and the code for archiving and deleting.
