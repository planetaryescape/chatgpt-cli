# Indexer and TS import can race on a stale transcript

Found by independent review on 2026-10-02, at c414e8e.

After a metadata-only change moves a chat from `t1` to `t2`, the indexer snapshots the stale `t1` transcript and calls `reconcile::check`. If a TS import replaces that transcript before `preserve` runs, the in-transaction guard correctly refuses. This can happen even when the replacement has identical content and only the model header differs. `save_batch` then treats the refusal as "content changed" and saves the fetched transcript at `t2`. The judgments, summaries and local titles stay at `t1`, and no later reconcile picks the chat up again, because its transcript is now current.

## Why it is not fixed now

- Hitting it needs a TS import to land inside the indexer's window between its snapshot and its preserve. Imports only run after a TS sync, which happens at most every 15 minutes.
- While the bridge exists, it corrects itself: the TS CLI reconciles its own caches, and the next import brings the advanced judgments across.
- The TS import is removed at stage 6 (cutover), and the race disappears with it.

## Fix, if it is still needed before stage 6

`reconcile::check` should return a distinct "snapshot superseded" result. On that result, the indexer retries the check against the current row instead of falling back to a replace. The final replacement should be guarded on the snapshot it read, so it doesn't overwrite an intervening write.
