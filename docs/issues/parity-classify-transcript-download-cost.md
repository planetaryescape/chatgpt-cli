# The Rust daemon caches a 2 MB chat in a minute; the TS CLI in a fifth of a second

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

## Where it comes from

The Rust daemon saves a transcript **with its search chunks**; the TypeScript CLI saves only the
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

## Why it is recorded and not fixed here

The output is the same, so it is not a parity difference the harness must assert: it is a cost
the Rust CLI carries and the TS CLI did not. Whether the chunking should happen during a
classification pass, or be left to the indexer, is a design decision for the next rung; this
rung produces the evidence.

## What the harness does about it

`crates/cli/tests/parity_classify.rs` compares the terminal transcript without the durations
(`Done in <t>.`, a step's own duration), so the two sides' notes compare equal. The harness's
own timeout is raised for this binary (`.config/nextest.toml`), because the Rust side's 2 MB
read can take minutes on a loaded machine.
