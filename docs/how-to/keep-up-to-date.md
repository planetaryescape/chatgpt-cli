# Keep the index and judgments up to date

Pick up new chats and changes since your last run, classify them, and generate local display titles.

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
- Changes to the thresholds in `src/classify/policy.ts` need no re-run at all, because suggestions are recalculated every time they're read.

Cached transcripts and summaries are reused either way. Scope it with any filter, for example `chatgpt classify --redo --older-than 2y`.

## What gets reused

| Cached | Invalidated when |
|---|---|
| Transcript | the chat changes, or the rendering changes (`RENDER_VERSION`) |
| Summary of a long chat | the chat changes, or the summary prompt changes (`SUMMARY_PROMPT_VERSION`) |
| Jev judgment | the chat changes, the questions change (`QUESTIONS_VERSION`), a time-bound judgment comes due for review, or you pass `--redo` |
| Jev follow-up | the chat or regular judgment changes, the follow-up questions change (`DEEP_QUESTIONS_VERSION`), or you pass `--redo` |
| Luna final judgment | the chat or Jev judgment changes, the final prompt changes (`LUNA_JUDGMENT_VERSION`), or you pass `--redo` |
| Luna local title and theme | the chat changes, the title prompt changes (`LOCAL_TITLE_VERSION`), or you pass `titles --redo`; manual titles remain until edited |

An interrupted `classify` or `titles` run loses nothing it finished; the next run carries on.

## Watch the cost

Every `classify` run ends with what it spent:

```text
Cost this run:
  Jev             $0.0027  15 call(s), 64k tokens (billed)
  total           $0.0027  of which billed: $0.0027
```

- **Jev** is billed to your TypeSafe account per input token; see `src/classify/costs.ts` for the current estimate.
- **Summaries** run on your Codex subscription, or your Claude subscription as a fallback. They're listed at their API-equivalent price (about $0.003 each with `gpt-6-luna`) so you can compare them with Jev, and aren't included in "billed".
- Before summarising more than about 500k tokens, `classify` shows a dollar estimate and asks. Pass `-y` to skip the question.

Prices live in `src/classify/costs.ts`, with their sources.
