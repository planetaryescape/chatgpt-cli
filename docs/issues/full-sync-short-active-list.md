# A full sync can record single-chat timestamps for chats the list skipped

Observed on 2026-10-01 during the stage 1 live demo (Rust `sync --full`, 811 chats).

The active conversation list ended early: 536 entries (some repeated across pages) where a pass five minutes earlier and the TS CLI's full sync a minute later both listed all 662. The full sync's omission check (ported from the TS CLI's `reconcileFullSyncOmissions`) then read the 159 missing chats one by one and indexed them with the single-chat endpoint's times, `toISOString` milliseconds that can be seconds later than the list's microsecond `update_time` (`…14:06:05.279Z` against `…14:05:59.206844Z`).

Nothing was lost: the cache reconcile matched all 159 by content and kept their judgments, titles and summaries current. But `list --json` showed those 159 chats with different `update_time` and `create_time` values from the TS CLI's, and a delta sync never corrects an active chat older than the watermark, so they stay until the next full sync. The TS CLI's index had 21 such rows from an earlier run of the same path.

Proposed: when a full listing comes back with fewer chats than the index holds, list once more before falling back to single-chat reads, and prefer list values when a later listing has the chat. That departs from the TS CLI, so decide it once the TS sync is retired.
