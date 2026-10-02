# Project moves make cached judgments and search results stale

**Resolved 2026-09-28.** `sync` now compares changed chats with cached transcripts through the batch endpoint. Matching title and rendered content keep transcript, summary, judgment, and search timestamps current; changed content stays stale. It also reconciles rows left stale by an interrupted move or earlier sync. A live repair preserved the affected project chats; after another `sync`, both projects had zero stale transcript and search timestamps. `bunx tsc --noEmit` and `bun test` passed.

Still resolved in the Rust daemon (checked after 0.2.0): `crates/daemon/src/sync/reconcile.rs` runs the same check on every explicit sync and hourly in the background, and the indexer runs it before replacing an older transcript; `a_metadata_only_move_keeps_the_judgment_current` (`crates/cli/tests/sync_cli.rs`) pins it.

Observed on 2026-09-28 while moving classified chats into projects.

ChatGPT changes `update_time` for many project moves without changing the conversation text. `sync` stores that new timestamp. `ClassificationStore.currentJudgments`, cached transcripts, and the search index all compare their stored timestamp with the conversation row, so a metadata-only move makes otherwise useful cached work appear stale. After project moves and `sync`, brainstorm counts fell sharply even though project membership was verified separately.

## Desired behaviour

Keep caches current when a project move changes only metadata. Preserve the existing invalidation when the chat's content also changed. Verify the content before advancing cache timestamps; the batch conversation endpoint can fetch up to 10 chats per request. Account for interrupted bulk moves and later `sync` runs.

The original post-move filter counts reflected stale judgments, not missing project chats.
