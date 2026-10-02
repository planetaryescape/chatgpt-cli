# A full sync can record single-chat timestamps for chats the list skipped

**Resolved after 0.2.0.** A full sync also lists again (up to twice, while that finds chats) when a listing comes back more than 5 chats shorter than the index's count for that list, so the chats it skipped keep the list's times; only what two more listings still miss is read one by one (`a_short_active_list_is_read_again_before_checking_chats_one_by_one` in `crates/cli/tests/sync_cli.rs`).

**Partly mitigated 2026-10-02.** A second live full sync, on an empty index, listed 662 active entries but only 657 distinct chats: the pages repeated some chats and skipped five others, which had no shared timestamps. With nothing indexed, the omission check had nothing to recover them from. A full sync now reads a list again (up to twice) when it repeated chats, while that still finds new ones (`crates/cli/tests/review_cli.rs`). The rest below still applies when a list is short without repeating.

Observed on 2026-10-01 during the stage 1 live demo (Rust `sync --full`, 811 chats).

The active conversation list ended early: 536 entries (some repeated across pages) where a pass five minutes earlier and the TS CLI's full sync a minute later both listed all 662. The full sync's omission check (ported from the TS CLI's `reconcileFullSyncOmissions`) then read the 159 missing chats one by one and indexed them with the single-chat endpoint's times, `toISOString` milliseconds that can be seconds later than the list's microsecond `update_time` (`…14:06:05.279Z` against `…14:05:59.206844Z`).

Nothing was lost: the cache reconcile matched all 159 by content and kept their judgments, titles and summaries current. But `list --json` showed those 159 chats with different `update_time` and `create_time` values from the TS CLI's, and a delta sync never corrects an active chat older than the watermark, so they stay until the next full sync. The TS CLI's index had 21 such rows from an earlier run of the same path.

Proposed: when a full listing comes back with fewer chats than the index holds, list once more before falling back to single-chat reads, and prefer list values when a later listing has the chat. That departs from the TS CLI, so decide it once the TS sync is retired.
