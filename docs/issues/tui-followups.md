# TUI and review follow-ups

Recorded on 2026-10-02 from stage 6a (native `tui` and `review`). None blocks a user journey. Stage 7 (2026-10-02) closed every item but one; each line says how.

## Should-fix

- ~~**No live refresh after a background sync.**~~ Fixed: the TUI polls `Status` every 30 s and reloads when the last sync or background Jev run moved, keeping the cursor on its chat and never under an open box or a filter being typed. There's no index generation counter in `Status`; the sync and Jev timestamps stand in for one, so a change another client makes (`chatgpt archive` in another terminal) still waits for `r`. Tests: `tui::a_changed_index_reloads_in_browse_only_and_keeps_the_cursor`, `tui_cli::a_background_sync_reloads_the_list_and_keeps_the_cursor`.
- ~~**One connection per request.**~~ Fixed: requests share the first load's connection (`crates/tui/src/connections.rs`); a second opens only while a slow fetch holds the first. A read that fails on a reused connection is sent once more on a new one; a write first checks the reused connection with `Status`. A new connection repaints the screen, so a `Restarting the chatgpt daemon` line never stays on it. Covered by every `tui_cli` journey.
- **Two age parsers.** Open. The TUI's `model::cutoff` repeats the daemon's `filters::parse_age` and `js::iso_from_millis` for its fixed ages (`30d` … `3y`). Moving both into `chatgpt_core::js` would leave one. Deferred: `filters.rs` and `js.rs` belong to the search and JS-compatibility work, and the duplicate is correct today.
- ~~**`review` loads one chat at a time.**~~ Fixed: the next chat is fetched on a thread of its own while the current one is on screen. Test: `review_command_cli::the_next_chat_is_fetched_while_this_one_is_read`.
- ~~**`review`'s verdicts** come from a `List` of every chat.~~ Fixed: `Selection.verdicts` (protocol, additive) makes `Select` return each row with its verdict and topic, as `List` does. Covered by `review_command_cli::anything_but_apply_changes_nothing` (the verdict shows).
- ~~**The preview wraps the whole transcript.**~~ Fixed: it wraps only as far as it's shown, plus a page. Test: `tui::a_long_transcript_is_wrapped_only_as_far_as_its_shown`.

## Nits

- ~~`chatgpt_core::desktop::copy_to_clipboard` returns `(ErrorKind, &str)`.~~ Fixed: `DesktopError` (with `Display`) for both `copy_to_clipboard` and `open_chat`; the TUI now says when `open` can't run.
- ~~`chatgpt_store::get` looks a chat up with `like 'id%'`.~~ Fixed: a full UUID uses `=` on the primary key. Test: `chatgpt-store tests::a_full_uuid_is_looked_up_exactly_and_anything_shorter_by_prefix`.
- ~~The help box is 84 columns wide and cut on a narrower terminal.~~ Fixed: it wraps, moves to the corner when it's too tall for its place, and says so when it still can't fit. Test: `tui::help_wraps_on_a_narrow_terminal_and_says_when_it_cant_fit` (new snapshot `help_narrow`).
- ~~`review` redraws after `u` on the first chat.~~ Fixed: `u` there does nothing. Test: `review_command_cli::the_next_chat_is_fetched_while_this_one_is_read`.
