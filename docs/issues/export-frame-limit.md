# An export larger than one IPC frame falls back to the TS CLI

**Must land before the TS CLI is removed (stage 6).** Recorded on 2026-10-02 from the stage 2 review.

The daemon answers `export` with the whole markdown in one IPC frame, and frames are capped at 16 MiB (`MAX_FRAME_BYTES` in `crates/protocol/src/codec.rs`). The TS CLI has no such limit. Today a chat whose JSON-encoded export would exceed the cap is answered with `ResponseData::ExportTooLarge`, and the CLI hands the command, with the same arguments and stdin, to the TS CLI (`crates/cli/src/export_cmd.rs`, `crates/daemon/src/server.rs` `send`). Output stays the TS CLI's, but only while the TS CLI exists. The largest chat seen live was 380 KB, so this is rare.

Fix: stream the export. Either send the markdown as a run of `ExportChunk` events followed by a small final answer (title, size), which the CLI writes out as they come, or have the daemon write the markdown to a 0600 temporary file in the instance's run directory and answer with its path. Then drop `ExportTooLarge` and the fallback. `CHATGPT_TEST_MAX_FRAME_BYTES` (debug builds) lowers the cap for the test in `crates/cli/tests/export_search_cli.rs`.
