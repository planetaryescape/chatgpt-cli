# The daemon never runs a full sync on its own

**Resolved after 0.2.0.** The daemon runs a full sync about once a day, once nobody has used the CLI for 10 minutes and the last one is a day old. It checks at most 50 omitted chats one by one, a quarter second apart, and leaves the index as it was beyond that; a rate limit puts it off another day. `daemon status` shows the last and next full sync. Tests: `the_daily_full_sync_drops_a_chat_deleted_while_active`, `the_daily_full_sync_reads_only_a_few_left_out_chats_one_by_one` (`crates/cli/tests/sync_cli.rs`) and the schedule tests in `crates/daemon/src/sync/mod.rs`.

Recorded on 2026-10-01 for stage 1 of the Rust port.

The daemon's background passes are deltas: new and changed active chats, plus an hourly archived-list sweep with a batch check of chats that left it. A chat deleted while active, in the browser or the app, stays in the index until someone runs `chatgpt sync --full`, as with the TS CLI.

Proposed: a nightly full sync while the machine is idle. It reads every chat list page (about 10 requests for 800 chats) and checks omitted chats one by one through the single-chat endpoint, which rate-limits at bulk pace, so it needs the backoff to hold and should skip a night after a rate limit.
