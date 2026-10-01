# The daemon never runs a full sync on its own

Recorded on 2026-10-01 for stage 1 of the Rust port.

The daemon's background passes are deltas: new and changed active chats, plus an hourly archived-list sweep with a batch check of chats that left it. A chat deleted while active, in the browser or the app, stays in the index until someone runs `chatgpt sync --full`, as with the TS CLI.

Proposed: a nightly full sync while the machine is idle. It reads every chat list page (about 10 requests for 800 chats) and checks omitted chats one by one through the single-chat endpoint, which rate-limits at bulk pace, so it needs the backoff to hold and should skip a night after a rate limit.
