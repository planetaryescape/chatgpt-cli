# The Rust daemon caches a 2 MB chat in a minute; the TS CLI in a fifth of a second

**Resolved on 2026-10-22, by rung 8 of the hardening round.** The classification pass no
longer does the search index's work: it caches the transcript it downloaded and lets the
indexer cut the passages in the background. The chunker also stopped scanning the whole
text for every cut, which is where the time had gone.

Recorded on 2026-10-21, from rung 7 of the hardening round: the model path, both CLIs, side by
side (`crates/cli/tests/parity_classify.rs`).

## What was seen

The two CLIs read the same fake chatgpt.com, the same 2 MB chat (`h-huge`, 400,100 words) and
the same transcript, and the same fake served both. The TypeScript CLI downloaded and cached the
transcript in **0.2 s**. The Rust daemon took **42 s**, **78 s** and **1 m 18 s** on three runs of
the same harness on the same machine (`Downloaded 1 transcript(s) (42s)` against
`Downloaded 1 transcript(s) (0.2s)`).

The output is identical either way: the harness compares the notes without their durations, so
this shows up as a run that takes a minute rather than as a difference between the two CLIs.

## Where it came from

The Rust daemon saved a transcript **with its search chunks**; the TypeScript CLI saved only the
transcript.

- `crates/daemon/src/classify/pipeline.rs`: `download` — "The transcripts of `to_fetch` not
  cached for the chat's `update_time`, through the batch endpoint ten at a time, saved (with
  their search chunks) batch by batch, so an interrupted run resumes." It ends with
  `save_transcripts(state, &saved).await?`.
- `git show v0.1.5:src/classify/pipeline.ts`: `saveTranscript` — `this.store.saveTranscript(transcript)`,
  and nothing else.

So the Rust CLI pays for the search index while it classifies, and the TS CLI does not. The
cost lands on the command the user is waiting for (`classify`, and the Jev guard's `--check`,
which classifies each chat it is asked about).

## Where the time actually went

Measured on 2026-10-22, in the parity harness itself: the download step's parts were timed
where the step runs them (`crates/daemon/src/classify/pipeline.rs`, `download` and `save`), and
the same transcript was cut and written on its own, in a test, for a second measurement. The
chat is `h-huge`: one user message of 400,100 words, 2,000,500 characters, no line break in it.

The download step of the 2 MB chat, on an idle machine (`Downloaded 1 transcript(s) (18s)`):

| Part | Time |
|---|---|
| The session exchange (`pinned_api`) | 48 µs |
| The batch read of the chat (`api.batch`) | 59 ms |
| Rendering the transcript (`cached_transcript`) | 7.9 ms |
| **Cutting it into passages (`transcript_chunks`)** | **18.05 s** |
| Waiting for the sync pass lock | 7 µs |
| Writing the transcript, the 2,779 passages and their FTS rows | 98 ms |

The same numbers from the isolated measurement (the same transcript, cut and written in a
test): **17.2 s** to cut it, **97 ms** to write the transcript, the passages and their FTS
rows, and **46 ms** to write the transcript alone.

So the cutting was 99% of the step, and everything else — the read, the render, the write —
was under 200 ms. The 42 s, 78 s and 1 m 18 s of the original runs are the same 18 s of CPU
work on a loaded machine: the cutting is pure computation, so it slows with the box.

**Why cutting was quadratic.** `transcript_chunks` looks for the cut to fall on: the last line
break at or before the cut, or the last space, whichever is later
(`crates/daemon/src/search/chunks.rs`). The space is a few characters back, but the line break
is nowhere: `last_index_of` looked for it from the cut back to the start of the text, every
time. A 2 MB chat with no line break in it is 2,779 cuts × an average of a million
comparisons — three billion of them, for a cut that only ever needs to look a little further
back.

## The fix

Two changes, one to each of the two costs.

1. **The classification pass saves the transcript alone.** `save_transcripts` writes the
   transcript and no passages; the indexer cuts them in the background
   (`crates/daemon/src/search/indexer.rs`: a chat whose cached transcript is current but whose
   passages aren't is in the `cached` group, and it is cut from the cache, without a fetch).
   The classification pass wakes the indexer once it has saved, so the index knows there is
   something to cut.
2. **The chunker scans the text once, not once per cut.** The last line break and the last
   space are remembered as the cut moves forward, which is the same answer in a fraction of
   the time: **17.2 s → 108 ms** for the same 2,779 passages, byte for byte the same ones.

### The index still works

- The passages still land in the index. The transcript's own write drops the chat's passages
  with it (the trigger in `crates/store/migrations/0003_search_follows_transcripts.sql`), so the
  chat is "current transcript, no passages" and the indexer cuts it from the cache.
- The indexer learns about it: `download` calls `state.indexer.wake()` after the save, and
  `unindexed` (the index's own test for "chats whose passages aren't current") reports the
  chat as cached, which is the group the indexer cuts without fetching.
- The cache's staleness rules still hold. `CHUNK_VERSION` and `RENDER_VERSION` are untouched:
  the passages the chunker cuts are the same ones it cut before, so none of them is stale, and
  the index format does not change at all. An index made by an older build is read the same
  way, and a rollback to an older build finds the same rows.
- It is not a race. The classification pass writes under the sync pass lock and on the writer
  connection, as every other write does; the indexer writes under the same lock and the same
  connection, and its own guard (`the transcript it read is still the one there`) refuses to
  write passages for a transcript that has moved on. The indexer's `foreground` flag still
  keeps it from fetching while a classification pass reads ChatGPT.

### What it costs, and what it buys

- `classify` on a long chat: **18 s → 0.1 s**, the same as the TypeScript CLI's 0.1 s. The
  parity harness's whole model-path run went from 26.3 s to 7.4 s.
- `search` after a `classify`: the indexer cuts the passages in the background. It is quick
  now that the chunker is linear: the end-to-end test
  (`a_classified_chat_is_searchable_once_the_indexer_has_chunked_it`) finds the chat **117 ms**
  after the classification finished, and the whole test takes 2.6 s.

## What pins it

- `crates/daemon/src/classify/pipeline.rs`:
  `a_classified_transcript_is_cached_for_the_indexer_to_chunk` — the property that matters,
  and it is not a timing: the classification pass writes no passages, the transcript is
  cached, and the indexer sees a chat to cut. It fails on the old code
  ("the classification pass wrote search chunks").
- `crates/cli/tests/parity_classify.rs`: the Rust side's download of the 2 MB chat must take
  at most ten seconds. The TypeScript side takes 0.1 s for the same work; the regression cost
  18 s on an idle machine and 42 s to 1 m 18 s on a loaded one. The bound fails on the old
  code ("the Rust side's download took 18000ms").
- `crates/cli/tests/classify_cli.rs`:
  `a_classified_chat_is_searchable_once_the_indexer_has_chunked_it` — after a `classify`,
  `chatgpt search` still finds the chat's content.
- `crates/daemon/src/search/chunks.rs`: `the_cut_falls_on_the_later_break_or_space` — the cut
  still falls where it always did. The goldens (`crates/cli/tests/golden.rs`) pin the search
  results over rich conversation trees, so a passage that moved would show there.

## What the harness does about it

`crates/cli/tests/parity_classify.rs` compares the terminal transcript without the durations
(`Done in <t>.`, a step's own duration), so the two sides' notes compare equal. It now also
compares the durations against the bound above.
