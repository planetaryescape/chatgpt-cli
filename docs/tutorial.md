# Tutorial: your first cleanup

In this tutorial you index your ChatGPT history, have Jev judge every chat, review its delete suggestions in the terminal UI, and delete the ones you agree with. It takes about 20 minutes, most of it waiting on the first `classify`.

Before you start, finish the [quick start](../README.md#quick-start) and check you're logged in to chatgpt.com in a supported browser.

## 1. Index your chats

```sh
chatgpt sync
```

The first command starts the background daemon, which keeps the index fresh from then on. The first sync lists active and archived chats. This example uses fictional counts and titles:

```text
Listed 42 active chat(s) (3.2s)
Listed 3 archived chat(s) (0.7s)
Full sync: 45 chats (42 active, 3 archived), +45 vs before, in 3.9s.
```

If you use a Chromium browser, macOS may ask for Keychain access to that browser's Safe Storage item the first time. Choose **Always Allow**, or it will ask again each time the daemon starts. Safari may require Full Disk Access for your terminal.

Check the index:

```sh
chatgpt list --limit 3
```

```text
00000000-0000-4000-8000-000000000001  2026-09-27       Garden Lighting Notes
00000000-0000-4000-8000-000000000002  2026-09-26       Pasta Recipe Question
00000000-0000-4000-8000-000000000003  2026-09-25    J  Reading List Ideas
```

The flags column shows `A` for archived, `P` for pinned and `J` for a chat inside a project.

## 2. Classify every chat

```sh
chatgpt classify
```

`classify` announces its three regular steps, asks Jev follow-up questions for unsure results, then sends anything still unsure, with a product-brainstorm label or borderline product idea, or with conflicting evidence about an expired purpose to Luna. Luna also generates local display titles and themes. Each step shows progress; your counts and times will differ:

```text
43 chat(s): 0 already judged, 43 new or changed to judge.
Steps: [1/3] download transcripts → [2/3] judge short chats → [3/3] summarise and judge long chats
[1/3] Downloaded 43 transcript(s) (<time>)
5 long chat(s) for step 3; 5 need a new summary (gpt-6-luna, then claude-haiku), ~100k tokens.
Go ahead? [y/N] y
[2/3] Judging short chats: 38 judged, $<cost> so far (<time>)
[3/3] Summarising and judging long chats: 5 judged, $<cost> so far (<time>)
Cost this run:
  Jev              $<cost>  43 call(s), <n>k tokens (billed)
  summaries (gpt-6-luna)  $<cost>  5 call(s), <n>k tokens (API-equivalent, covered by your subscription)
  total            $<cost>  of which billed: $<cost>
<n> unsure chat(s): 0 already deep-classified, <n> to judge.
Deep-classified <n> chat(s) (<time>)
Cost this run:
  Jev              $<cost>  <n> call(s), <n>k tokens (billed)
  total            $<cost>  of which billed: $<cost>
<n> chat(s) need Luna's deeper review.
Luna reviewed <n> chat(s) (<time>)
Generated <n> local title(s) (<time>)
Done in <time>. 43 of 43 judged: delete <n>, archive <n>, keep <n> (<n> still unsure).
```

The first run can take several minutes or longer, depending on your chat count and the number of long conversations. The follow-up adds a Jev call for each unsure chat, so its time and billed cost vary with the results. Check the cost estimate before accepting a large summary batch.

- Step 1 downloads each transcript once and caches it. Later runs skip it.
- Step 2 is fast, and its results are usable straight away.
- Step 3 summarises chats over ~12k tokens with Codex (or Claude as a fallback) before Jev judges them. It asks first because it uses your subscription.
- The follow-up pass reuses those transcripts or summaries. Luna reviews anything still unclear. A chat keeps its `?` only if that review cannot finish.
- Local titles appear in the CLI and TUI; the original title and order in ChatGPT stay the same.

You can stop it with `ctrl-c` at any point. Every finished chat is saved, and the next `chatgpt classify` carries on where this one stopped.

## 3. See what Jev suggests

```sh
chatgpt stats
```

```text
45 chat(s), 43 judged, 2 not yet judged

suggestion   chats
──────────   ─────
delete           8
delete?          2
archive          4
archive?         1
keep            20
keep?            8
not judged       2

brainstorms (kept)   chats
──────────────────   ─────
writing                  4
product                  3
other                    1
total                    8
```

The `?` rows mean Jev's answers sat close to a threshold. Those chats are worth reading yourself. The two unjudged chats are pinned; `classify` skips pinned chats unless you pass `--pinned`.

## 4. Review the delete suggestions

```sh
chatgpt tui
```

1. Press `2` to show only chats Jev suggests deleting.
2. Read the preview on the right: Jev's verdict, its scores, and the transcript. `J`/`K` scroll it.
3. Press `enter` to accept Jev's suggestion and move to the next chat. Press `j` to skip one you want to keep.
4. Watch the header count your marks: `marked: 3 delete, 0 archive (x to apply)`.

Nothing has changed on ChatGPT yet. Marks live only in the TUI until you apply them.

## 5. Apply your marks

1. Press `x`. A dialog summarises what will happen: `Archive 0 and permanently delete 3 conversation(s). Deletes cannot be undone.`
2. Type `apply` and press `enter`. Anything else cancels.
3. The dialog counts progress, then the list reloads without the deleted chats.

## 6. Check the result

```sh
chatgpt sync
chatgpt stats --suggest delete
```

The delete count drops by the number you applied, and the chats are gone from the ChatGPT sidebar.

## Next steps

- Work through `6` (still unsure) and `3` (archive) the same way.
- Pull your brainstorms out for Claude Code: [Find your brainstorms](how-to/find-brainstorms.md).
- Every TUI key: [TUI reference](reference/tui.md).
