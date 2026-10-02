# TUI and review: what pins their behaviour

`chatgpt tui` and `review` replaced the TS CLI's OpenTUI React app and `review` command (sources in tag `v0.1.5`), whose screens couldn't be compared byte for byte with ratatui's. Instead, every key and state of the TS TUI (`src/tui/app.tsx` with its model, panes and transcript hook) and of `src/commands/review.ts` @ 1b8c950 was mapped to a Rust test, and the TS TUI's own tests were ported one for one. The tables below are that map: the left column is the behaviour, as the TS CLI had it, and the right the test that holds it now.

Test locations:

- `tui::…`: `crates/tui/src/tests.rs`, the app driven by keys and drawn on ratatui's `TestBackend`, with insta snapshots in `crates/tui/src/snapshots/`;
- `model::…`: `crates/tui/src/model.rs`;
- `input::…`: `crates/tui/src/input.rs`;
- `tui_cli::…`: `crates/cli/tests/tui_cli.rs`, the real binary in a pseudo-terminal against the fake chatgpt.com through a test daemon;
- `review_command_cli::…`: `crates/cli/tests/review_command_cli.rs`, `review` in a pseudo-terminal.

## TUI

### Layout

| TS | Rust test |
|---|---|
| Header: `chatgpt · <shown> of <total> · active/archived · suggestion · topic`, then age, brainstorm and `"query"` | `tui::filters_by_suggestion_age_topic_brainstorm_title_and_archive`, snapshot `list_and_preview` |
| Header right: `marked: N delete, M archive (x to apply)` | `tui::marks_toggle_accept_and_count`, snapshot `marks` |
| List row: mark, date, suggestion (`?` when unsure, `·` unjudged), topic or `idea:<kind>`, display title | snapshots `list_and_preview`, `marks`; `tui::filters_…` (`idea:writing`) |
| `No conversations match.` and `Nothing selected.` | snapshot `filters_no_match` |
| Preview: dates, project, pinned, archived; `Jev:`/`Luna:` verdict, `(unsure)`, topic, reason; or `not classified yet` | `tui::lists_chats_with_suggestions_and_previews_the_selection`, `tui::a_long_chat_shows_the_summary_it_was_judged_from` |
| Summary of a long chat judged from it, above the transcript | snapshot `preview_with_summary` |
| `Loading transcript…`, `Couldn't load: …` | `tui::an_uncached_transcript_is_fetched_after_the_debounce` |
| Footer: key hints, or the last action's status | snapshots `list_and_preview`, `quit_with_marks` |
| Help box, apply box, title box, applying box | snapshots `help`, `apply_dialog`, `title_edit`, `applying` |
| A resize redraws at the new size | `tui::a_resize_redraws_at_the_new_size`, snapshot `small_terminal` |

### Keys

| TS key | Rust test |
|---|---|
| `j`/`↓`, `k`/`↑` | `tui::keys_pressed_before_a_draw_all_apply`, `tui_cli::browse_filter_preview_copy_mark_and_quit_without_applying` |
| `g`/`G`, `ctrl-d`/`ctrl-u`, `pagedown`/`pageup` | `tui::copy_open_reload_and_preview_scrolling` |
| `J`/`K` (3 lines), `space` (a page), clamped, reset on a new chat | `tui::copy_open_reload_and_preview_scrolling` |
| `/` filter as typed; `enter` keeps; `esc` clears (in the filter and in the list) | `tui::filters_…`, snapshot `filter_typing`, `tui_cli::browse_…` |
| `1`–`6` suggestion filter | `tui::filters_…`, `tui_cli::browse_…` |
| `t`/`T` topic, `y`/`Y` age, `b`/`B` brainstorm, `A` archived | `tui::filters_…`, `model::tests::*` |
| `d`, `a` (again unmarks), `u` | `tui::marks_toggle_accept_and_count` |
| `enter`: delete/archive marks, keep unmarks, then moves on; unjudged says so | `tui::marks_toggle_accept_and_count`, `tui_cli::apply_changes_exactly_the_marked_chats` |
| `x` with no marks says so | `tui::apply_needs_apply_typed_and_sends_exactly_the_marks` |
| `x`: only `apply` (trimmed) applies; anything else says `Not applied: type apply exactly.`; `esc` cancels | `tui::apply_needs_…`, `tui_cli::browse_…` |
| Applying: `n/total done…`, keys wait; then `Applied N change(s).` or `N failed: …`, marks cleared, reloaded | `tui::apply_needs_…`, `tui::a_failed_apply_…`, `tui_cli::apply_changes_exactly_the_marked_chats` (fake chatgpt.com changed for exactly the marked chats) |
| `c` copies the transcript markdown (`Copied "<title>" (N KB).`), or `Transcript not loaded yet.` | `tui::copy_open_…`, `tui_cli::browse_…` (stand-in `pbcopy`) |
| `o` opens `https://chatgpt.com/c/<id>` | `tui::copy_open_…` |
| `r` reloads from the index | `tui::copy_open_…` |
| `n` edits the local title; `enter` saves, `esc` cancels; an invalid title says why and stays open; ChatGPT's title is unchanged | `tui::edits_a_local_title`, `tui_cli::a_local_title_goes_to_the_index_and_a_refused_one_says_why` |
| `?` help; any key closes it (and does nothing else) | `tui::help_shows_every_key_and_any_key_closes_it`, `tui::keys_pressed_before_a_draw_all_apply`; on a narrow or short terminal it wraps, moves to the corner, and says when keys are cut off: `tui::help_wraps_on_a_narrow_terminal_and_says_when_it_cant_fit` |
| `q`; with marks, `q` twice; another key disarms it | `tui::quitting_with_marks_takes_two_presses`, snapshot `quit_with_marks`, `tui_cli::browse_…` |
| `ctrl-c` quits at once, marks discarded | `tui::quitting_…`, `tui_cli::the_terminal_is_given_back_on_quit_ctrl_c_and_a_panic` |

### Behaviour

| TS | Rust test |
|---|---|
| Cached transcripts show at once; others are fetched (batch endpoint) after 200 ms if still selected, and cached | `tui::an_uncached_transcript_is_fetched_after_the_debounce`, `tui_cli::browse_…` (one batch read, then cached) |
| A late transcript for a chat no longer selected is ignored | `tui::an_uncached_transcript_is_fetched_after_the_debounce` |
| A reload that brings a newer revision of the selected chat fetches it again, drops the older answer, and `c` copies the new text | `tui::a_reload_that_brings_a_newer_revision_refetches_the_preview` |
| A late answer (a title save, another apply, an older reload) never unlocks a running apply or replaces newer rows | `tui::a_late_answer_never_unlocks_a_running_apply`, `tui::only_the_latest_reload_counts` |
| Keys faster than a render all apply (the TS `live` ref) | `tui::keys_pressed_before_a_draw_all_apply` |
| `requireSynced` before the screen | `tui_cli::no_index_says_so_before_taking_the_screen` |
| The terminal is given back on quit, Ctrl-C and a panic | `tui_cli::the_terminal_is_given_back_on_quit_ctrl_c_and_a_panic` |
| SIGTERM and SIGHUP give the terminal back too (handlers in place before raw mode; exit 128 + the signal) | `tui_cli::a_signal_gives_the_terminal_back` |
| Titles typed for one chat land in the order typed (one save out at a time; the latest waits) | `tui::title_saves_to_one_chat_land_in_the_order_typed` |
| Marks aren't kept | by construction (`App::marks` lives in memory only) |
| (Rust only) A background sync or Jev run reloads the list, keeping the cursor, never under a box | `tui::a_changed_index_reloads_in_browse_only_and_keeps_the_cursor`, `tui_cli::a_background_sync_reloads_the_list_and_keeps_the_cursor` |
| (Rust only) A reload that answers while a box is open or an apply runs waits until it closes, so the rows and marks the user confirmed never change under them | `tui::a_reload_answering_under_the_apply_box_waits_for_it_to_close` |
| (Rust only) A long transcript is wrapped only as far as it's shown | `tui::a_long_transcript_is_wrapped_only_as_far_as_its_shown` |

## review

| TS | Rust test |
|---|---|
| `[i/n]  <title>`, dates, project, archived; `Jev/Luna suggests: …`, reason; `Summary:` clipped to 900 | `review_command_cli::anything_but_apply_changes_nothing` |
| `N turns`, `You:` (500), `ChatGPT (last):` (400), one line each | same; `clip` in `chatgpt_core::js` tests |
| Help line with and without `[enter] accept suggestion` | `review_command_cli::anything_but_…` |
| `k`/space keep, `a` archive, `d` delete, `u` undo | `review_command_cli::anything_but_…`, `…::apply_archives_and_deletes_what_was_decided` |
| `enter` accepts Jev's suggestion; does nothing without one | `review_command_cli::enter_accepts_jevs_suggestion_and_q_stops_early` |
| `q` (and Ctrl-C) stops; nothing decided means no question | same |
| `v` pages the transcript through `$PAGER -R`, then shows the chat again | `review_command_cli::view_pages_the_transcript_and_open_hands_the_link_to_the_browser` |
| `o` opens the chat | same |
| `--oldest-first` | `review_command_cli::enter_accepts_…` |
| Ids on stdin (`-`), keys from `/dev/tty` | `review_command_cli::ids_on_stdin_and_keys_from_the_terminal` |
| `Reviewed N: keep K, archive A, delete D.`, then the deletes and archives, then `Type apply to carry these out: `; anything else: `Nothing changed.` | `review_command_cli::anything_but_…` |
| `apply` archives, then deletes, as `archive -y`/`delete -y` (preview, progress, summary) | `review_command_cli::apply_archives_and_deletes_what_was_decided` |
| `(could not load: …)` for a chat ChatGPT doesn't return | `review_command_cli::a_chat_chatgpt_no_longer_has_says_why_and_can_still_be_decided` |
| `Nothing matched.` | `review_command_cli::nothing_matched_needs_no_terminal` |
| A transcript of any size | `review_command_cli::a_transcript_larger_than_a_frame_streams_whole` |
| SIGTERM or SIGHUP while waiting for a key gives the terminal back (exit 128 + signal) | `review_command_cli::a_signal_while_waiting_for_a_key_gives_the_terminal_back` |
| A fetch a sync overtook is shown but not cached | `review_command_cli::a_fetch_overtaken_by_a_sync_is_shown_but_not_cached` |
| Only the current chat and the 3 before it stay in memory; older ones are fetched again on `u` | `review_cmd::tests::only_the_undo_window_stays_in_memory` |
| (Rust only) The next chat is fetched while the current one is on screen; `u` on the first chat does nothing | `review_command_cli::the_next_chat_is_fetched_while_this_one_is_read` |

## Where the Rust port differs, on purpose

- **Applying marks** goes through `Mutate`, as `archive -y` and `delete -y` do: the index's pinned account and the sync pass lock, and no resend of a delete. So the archives go first, then the deletes (three at a time), each group in the list's order; the TS TUI went one chat at a time in the order marked. A failure line names the chat (`<id> <title>: <why>`). If the daemon refuses the whole apply (say, another account's session), the marks stay, to try again.
- **Live refresh.** Besides `r`, an apply and a title (as in the TS TUI), the list reloads when the daemon's `Status` (polled every 30 s) shows a sync or a background Jev run since the rows were loaded. The cursor stays on the chat it was on, and nothing reloads while a box is open or a filter is being typed (the next poll tries again). Polling, not a subscription: the daemon has none to offer.
- **One connection.** Requests reuse the connection the first load opened instead of connecting and asking `Status` each time; a second opens only while a slow fetch holds the first. A reused connection a write would go out on is checked with `Status` first, so a write never goes to a daemon that has since restarted and might not have seen it.
- **Transcripts** are looked up in the cache through the daemon, so a cached one shows a moment after the selection (`Loading transcript…` flashes) rather than in the same frame.
- **Text fields** (filter, `apply`, title) take left/right, home/end (`ctrl-a`/`ctrl-e`), `ctrl-k`/`ctrl-u`, delete and backspace. In the list, a `ctrl-` letter other than `ctrl-c`, `ctrl-d` and `ctrl-u` does nothing (in OpenTUI, `ctrl-a` also marked for archive).
- **Columns** are padded by terminal width, so wide characters (emoji, CJK) don't push a row out of line.
- **review** reads keys one at a time even when several arrive in one read (Node handed them over as one chunk that matched no key); an unknown key, or `u` on the first chat, neither redraws nor fetches the chat again; the next chat is prefetched while the current one is read; the verdicts come with the selected rows (`Select` with `verdicts`) instead of a second query of every chat; each chat is fetched once and `v` pages that copy (the TS CLI fetched it again); `v` on a chat that couldn't be loaded tries the load again instead of ending the review; `$PAGER` may carry arguments (`less -S`).
- **Unknown commands** are clap's error (`unrecognized subcommand`, exit 2) instead of commander's.
