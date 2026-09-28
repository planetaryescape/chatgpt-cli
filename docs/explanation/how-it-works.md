# How it works

How `chatgpt` gets from your Dia login to a delete suggestion, and why each piece is built the way it is.

## The path of a request

```text
Dia cookie store ──(Keychain key)──▶ session cookie ──▶ /api/auth/session ──▶ bearer token
                                                                                 │
                                        impit (Chrome TLS fingerprint) ◀─────────┘
                                                  │
                                           chatgpt.com/backend-api
```

1. **`src/auth/dia-cookies.ts`** copies Dia's cookie database and decrypts the chatgpt.com cookies. It uses Chromium's macOS scheme: AES-128-CBC with a key derived from the "Dia Safe Storage" Keychain password. Reading Dia's own login means there's no separate sign-in, no copied tokens, and nothing to refresh by hand.
2. **`src/api/client.ts`** exchanges the session cookie for a bearer token, then calls the API. Every request goes through `impit`, an HTTP client that impersonates Chrome's TLS fingerprint. Plain `fetch` and `curl` are challenged by Cloudflare, and so is headless Chrome. A visible browser gets through, but opening one for every command isn't workable. It also retries Cloudflare challenges on a fresh connection, and 429s and gateway errors with backoff.
3. **`src/api/conversations.ts`** holds one function per endpoint. [The chatgpt.com web API](chatgpt-api.md) records each endpoint's observed behaviour.

## The local index

`src/index/store.ts` keeps one row per chat in SQLite (`bun:sqlite`). Filters (`--older-than`, `--title`, …) run against it, so listing a large history is fast and works offline.

`sync` is a delta by default. It reads the most recent chats until it reaches ones it already has. It also reads the full archived list, because archiving doesn't change a chat's update time. Chats that drop off the archived list are checked one by one before the index changes. `sync --full` rebuilds everything, which is the only way to notice chats deleted in the ChatGPT app.

## Searching locally

`src/search/` splits cached transcripts into short overlapping passages. SQLite FTS5 indexes each passage's title and text, so plain `search` ranks exact-word matches without loading a model. `search-index` fetches uncached active transcripts by default and computes a 384-dimensional embedding for each passage with a local quantized MiniLM model. `--archived` and `--all` opt into indexing archived chats. `search --semantic` compares a query embedding with the stored vectors; `--hybrid` merges text and semantic ranks. The vectors live in the same SQLite database as the transcript cache, avoiding a second index to synchronize.

Both search paths join against the current conversation update time, so changed or removed chats do not appear with stale text. Reindexing replaces changed passages, and embedding work is saved in batches so a later run resumes. [Search your history](../how-to/search-history.md) has the commands and model details.

## Rendering a transcript

A ChatGPT conversation is a tree: editing or regenerating a message creates a branch. `src/render/transcript.ts` walks from `current_node` up to the root, which gives the thread you see in the UI, and renders it as markdown. It handles voice transcripts, images, generated images, file attachments, web-citation markers and canvas documents. Canvas edits are regex replacements, so it replays them to reach the final document.

## Classifying

`src/classify/pipeline.ts` runs three steps:

1. **Download.** Transcripts come from `POST /conversations/batch`, 10 per call. Single-chat fetches get rate-limited within minutes; the batch endpoint doesn't at this pace. Each batch is cached as it arrives.
2. **Judge short chats.** Chats up to ~12,000 tokens go to Jev whole, four at a time.
3. **Summarise and judge long chats.** Longer chats are first summarised by Codex (`gpt-6-luna`), or by `claude -p` with Haiku if Codex fails, then judged from the summary. Summarising beats cutting the transcript short, which would lose how the chat ended. Jev's accuracy also drops as its input grows, which is why the cutoff is well under its 32k-token limit.

Short chats go first so results appear in seconds, and each long chat is summarised and judged in one go so an interrupted run keeps everything it finished.

After those steps, `src/classify/deep-pipeline.ts` asks Jev a second set of questions about every unsure result in the selected chats. It reuses cached transcripts and summaries, and caches the follow-up answers separately. `src/classify/luna-pipeline.ts` sends any still unsure chat, plus borderline or conflicting product-brainstorm labels and possible time-expired chats with conflicting evidence, to Luna for a deeper review. An incomplete final review leaves an unsure result marked `?`. A later `classify` resumes missing reviews even if the regular judgment was cached. A chat with a finite purpose that has not yet expired is re-judged after seven UTC days when `classify` runs; strong evidence of a lasting personal record delays that recheck until a later UTC month. Age alone never changes a suggestion.

`src/classify/titles.ts` uses Luna to give each chat a local display title and an open-ended theme from its transcript or summary. The title appears in the CLI and TUI while the ChatGPT title remains in `conversations`. A manual local title takes precedence; a generated title is regenerated when the chat changes.

Every Jev call and summary adds to a cost meter (`src/classify/costs.ts`). The run ends with a breakdown separating billed spend (Jev) from subscription-covered spend shown at API prices (summaries).

Jev is TypeSafe's System One model. It answers typed questions with probabilities rather than generating text. The regular judgment sends one request per chat; an unsure result gets a second request containing the follow-up questions. Luna supplies the slower judgment when those answers cannot settle the decision.

## From answers to suggestions

`src/classify/policy.ts` and `src/classify/deep-policy.ts` turn Jev's raw probabilities into delete, archive or keep. The database stores the regular and follow-up raw answers, and the rules run every time suggestions are read. So changing a threshold never requires another run, and every command and the TUI agree. [Classification reference](../reference/classification.md) lists the rules.

## Caching and versions

Every cached value records the chat's `update_time` and a version constant:

| Cache | Version constant |
|---|---|
| Transcript | `RENDER_VERSION` in `src/render/transcript.ts` |
| Summary | `SUMMARY_PROMPT_VERSION` in `src/classify/summarise.ts` |
| Judgment | `QUESTIONS_VERSION` in `src/classify/questions.ts` |
| Follow-up judgment | `DEEP_QUESTIONS_VERSION` in `src/classify/deep-questions.ts`, tied to the regular questions version |
| Luna final review | `LUNA_JUDGMENT_VERSION` in `src/classify/luna-version.ts`, tied to the Jev judgment and follow-up version |
| Local display title | `LOCAL_TITLE_VERSION` in `src/index/store.ts` |

A chat that changed, or a version that moved, makes the cached row stale, and the next `classify` redoes it. That's how "judge only what's new" works without separate bookkeeping.

## Acting on chats

`src/commands/mutate.ts` updates ChatGPT before the local index. Bulk delete sends up to three requests at a time, with a 250ms pause per worker. A 404 on delete means the chat is already gone, so the index is updated and the retry counts as complete. Bulk commands preview first and need confirmation: `y` for archive, and the typed count for delete. The TUI collects marks and applies them only after you type `apply`.

## Why a CLI and a TUI

The CLI suits bulk work you trust and anything you pipe into another command. The TUI (OpenTUI with React, in `src/tui/`) suits reading and deciding one chat at a time. Both use the same index, the same policy and the same code for archiving and deleting.
