# Keep the index and judgments up to date

The background daemon syncs on its own: every 2 minutes while you're using `chatgpt`, otherwise every 15 (`chatgpt daemon status` shows when it last ran and when it runs next). Run `sync` yourself to pick up changes now, including the archived list and the cache check, which the daemon otherwise runs at most hourly. Then classify new chats and generate local display titles.

```sh
chatgpt sync
chatgpt classify
```

```text
Checking for new and updated chats…
0 new, 0 updated (2.4s)
Reading archived chats…
3 archived (0.7s)
Sync done in 5.5s: 0 new, 0 updated, 0 newly archived, 0 unarchived, 0 deleted. `sync --full` also drops chats deleted in the browser.
```

`sync` reads your most recent chats until it reaches ones it already has, then re-reads the archived list. `classify` judges chats that are new, have changed, were judged under older questions, or had a still-current time-bound purpose last judged at least seven UTC days ago. Chats with strong evidence of a lasting personal record are rechecked in a later UTC month. It asks Jev follow-up questions for unsure judgments, then uses `gpt-6-luna` for any still unsure, every product-brainstorm label, borderline product ideas or conflicting time-expired cases. Finally it generates missing local titles and topic themes from cached transcripts or long-chat summaries. It never renames a chat in ChatGPT. Run `chatgpt titles` separately to fill or regenerate titles without reclassifying.

With a Jev key in the config, the background daemon also gives new and changed chats Jev's first-pass verdict after each sync ([configure](configure.md#jev-on-new-chats-in-the-background)); `classify` still does the follow-up, Luna's review, summaries and titles.

## When to use `sync --full`

```sh
chatgpt sync --full
```

Use it when you've deleted chats in the ChatGPT app or website. A normal sync can't see deletions made elsewhere, because deleted chats stop appearing in the list. The full sync rebuilds the index, checking previously indexed chats omitted from the server's conversation lists individually before removing them. The archived list can be incomplete; an initial sync with no prior index still depends on what that list returns. It takes longer as the index grows; individual checks add time when lists are incomplete.

Deletes you make with `chatgpt` update the index immediately, so they don't need `--full`.

## When to use `classify --redo`

```sh
chatgpt classify --redo
```

This re-judges every matching chat, including its unsure follow-up, even when both are cached. You rarely need it:

- Changes to Jev's questions already trigger re-judging, because judgments are tied to a version number.
- Changes to the policy thresholds (`crates/daemon/src/policy/`) need no re-run at all, because suggestions are recalculated every time they're read.

Cached transcripts and summaries are reused either way. Scope it with any filter, for example `chatgpt classify --redo --older-than 2y`.

## What gets reused

| Cached | Invalidated when |
|---|---|
| Transcript | the chat changes, or the rendering changes (`render_version`) |
| Summary of a long chat | the chat changes, or the summary prompt changes (`SUMMARY_PROMPT_VERSION`) |
| Jev judgment | the chat changes, the questions change (`questions_version`), a time-bound judgment comes due for review, or you pass `--redo` |
| Jev follow-up | the chat or regular judgment changes, the follow-up questions change (`deep_questions_version`), or you pass `--redo` |
| Luna final judgment | the chat or Jev judgment changes, the final prompt changes (`luna_version`), or you pass `--redo` |
| Search passages and their embeddings | the transcript changes, the chunking changes (`CHUNK_VERSION`) or the embedding model changes (`MODEL_VERSION`); the daemon rebuilds them in the background, from cached transcripts |
| Luna local title and theme | the chat changes, the title prompt changes (`local_title_version`), or you pass `titles --redo`; manual titles remain until edited |

An interrupted `classify` or `titles` run loses nothing it finished; the next run carries on.

## Watch the cost

Every `classify` run ends with what it spent:

```text
Cost this run:
  Jev             $0.0027  15 call(s), 64k tokens (billed)
  total           $0.0027  of which billed: $0.0027
```

- **Jev** is billed to your TypeSafe account per input token; see `crates/daemon/src/classify/costs.rs` for the current estimate.
- **Summaries** run on your Codex subscription, or your Claude subscription as a fallback. They're listed at their API-equivalent price (about $0.003 each with `gpt-6-luna`) so you can compare them with Jev, and aren't included in "billed".
- Before summarising more than about 500k tokens, `classify` shows a dollar estimate and asks. Pass `-y` to skip the question.

Prices live in `crates/daemon/src/classify/costs.rs`, with their sources.

## After upgrading from 0.2.0

The rendering of canvas edits changed (render version 3: they match by UTF-16 code unit, as JavaScript does), so every cached transcript is out of date. The daemon fetches each chat once more in the background through ChatGPT's batch endpoint, then chunks and embeds it again: about six minutes for 800 chats, plus the embedding. Judgments, titles and summaries are kept. Search keeps answering meanwhile, from the chats already re-fetched; `daemon status` shows the progress. Chunks that could hold invalid text from before 0.1.6 are dropped, and chunks the embedding model fails on are now set aside for a day rather than until the daemon restarts.

## After upgrading from 0.1.5

0.1.5 was the last release with the old TS CLI in the repository. The first daemon of a newer release keeps your index, judgments, titles and summaries, and rebuilds the search passages from cached transcripts without the network (they no longer split an emoji), then embeds them again: about half an hour on one core for a large history. Lexical search works throughout; semantic and hybrid search say `N of M chunks embedded` until it's done. `chatgpt import-legacy` is gone. The TS CLI's database, `~/.local/share/chatgpt-cli/index.db`, is no longer read; delete it when you no longer want it.
