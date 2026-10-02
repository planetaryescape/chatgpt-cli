# TUI and review follow-ups

Recorded on 2026-10-02 from stage 6a (native `tui` and `review`). None blocks a user journey.

## Should-fix

- **No live refresh after a background sync.** The list changes on `r`, after an apply and after a title, as in the TS TUI. A cheap improvement without a subscription: poll `Status` every 30 s while idle and show `index updated, r to reload` in the header when `sync.last_finished_at` moved. A real opt-in `Subscribe` (spotuify's, snapshot first) needs server plumbing the daemon left out on purpose (`crates/daemon/src/server.rs` header). Decided against for 6a: the TS TUI never refreshed itself, and reshuffling rows under the cursor while marking is a trust risk.
- **One connection per request.** Each TUI request (`chatgpt_launcher::ask`) connects and checks `Status` first. Cheap on a Unix socket, but a daemon restarted mid-session prints `Restarting the chatgpt daemon: …` over the screen (until the next redraw). A long-lived client in `run.rs`'s worker would avoid both.

- **Two age parsers.** The TUI's `model::cutoff` repeats the daemon's `filters::parse_age` and `js::iso_from_millis` for its fixed ages (`30d` … `3y`). Moving both into `chatgpt_core::js` would leave one.
- **`review` loads one chat at a time.** Each chat's batch fetch starts after the previous decision; prefetching the next while waiting for a key would hide the round trip.
- **`review`'s verdicts** come from a `List` of every chat, since `Select` rows carry none. A flag on `Select` for verdicts would save the second query.
- **The preview wraps the whole transcript** (once per text and width, cached). For a multi-megabyte chat, wrapping lazily up to the scroll position would make the first frame faster.

## Nits

- `chatgpt_core::desktop::copy_to_clipboard` returns `(ErrorKind, &str)`; a small error type with `Display` would save each caller its mapping.
- `chatgpt_store::get` looks a chat up with `like 'id%'`, which can't use the primary key; `Transcript` asks for exact ids and could use `=`.

- The help box is 84 columns wide, as the TS TUI's; on a narrower terminal it's cut at the right edge.
- `review` redraws after `u` on the first chat (the TS CLI did too).
