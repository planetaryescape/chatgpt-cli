# The TUI clears every mark when an apply fails for some chats

**Fixed.** After an apply, the chats that failed keep their marks, so the list still shows them as marked and `x` retries exactly those; the footer names every failure, not only the first.

Recorded on 2026-10-21, from an adversarial review of the Rust TUI (rung 2 of the hardening round).

`crates/tui/src/app.rs`, `Outcome::Applied`, cleared the marks unconditionally and named only the first failure:

```rust
Ok(failures) => {
    self.marks.clear();
    self.status = match failures.first() {
        None => format!("Applied {total} change(s)."),
        Some(first) => format!("{} failed: {first}", failures.len()),
    };
}
```

The daemon already returns every failure, each with the chat's id and title (`crates/daemon/src/mutate.rs`: `format!("{} {}: {}", target.id, target.title, error.message)`), and the TUI threw the rest away.

Impact: the user was left with `3 failed: <first>` and no way to tell which of the other two failed, and no way to retry them without marking every chat again by hand. The neighbouring transport-error branch deliberately kept the marks ("Marks stay, to try again"), so the more likely failure — one chat out of several refusing the change — was the one that lost them. The TypeScript CLI had the same shape (`git show v0.1.5:src/tui/app.tsx`), but it is gone, so parity was not a constraint.

Fix: the marks of the chats that did not succeed are kept, and the footer's status area grows to name every failure, one per line, so a failed chat stays identifiable even when a filter hides it from the list. The status line says how many failed and how many were applied.

Tests: `tui::only_the_failed_chats_keep_their_marks_and_every_failure_is_named` (with the `failed_apply` snapshot), `tui::a_failed_apply_says_how_many_failed_and_keeps_the_failed_chats_marked`, `tui::a_failed_chat_the_list_hides_is_still_named_and_marked`, and `tui_cli::a_failed_apply_keeps_the_failed_chat_marked_and_names_it` (the real binary against the fake chatgpt.com, with one write turned away).

## The TUI's age filter failed open

The same review found that the TUI re-implemented the daemon's age parsing (`crates/tui/src/model.rs`, `cutoff`) and was worse than it in two ways: an age it could not parse applied no cutoff at all, so it silently showed **every** chat, and a count of about 1.07e11 or more overflowed `i64` (a panic in a debug build, a wrapped cutoff in a release build).

The daemon's `parse_age` clamps, but the TUI's did not. Both are now one implementation in `chatgpt_core::js` (`parse_age` and `iso_from_millis`), which the daemon and the TUI both call, and the TUI's filter fails **closed**: an age it cannot read shows no chats, never every chat, and the footer says why.

Reachability: `view.older_than` has exactly one writer (`crates/tui/src/app.rs`, the `y` key) and it cycles a fixed list (`AGE_CYCLE`: `any`, `30d`, `6m`, `1y`, `2y`, `3y`). All five values parse identically in both parsers, so the divergence was unreachable; the fix removes a fail-open landmine for the first person who lets a user type an age.

Tests: `chatgpt-core js::tests::ages_are_read_as_js_reads_them`, `chatgpt-core js::tests::a_count_too_large_for_a_date_is_clamped_and_writes_no_date`, `tui model::tests::an_age_the_cli_cannot_read_shows_nothing`.
