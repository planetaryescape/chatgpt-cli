//! `chatgpt tui`: browse, filter and triage chats in a terminal UI, ported
//! from the TS CLI's OpenTUI app (`src/tui/` @ 1b8c950) to ratatui.
//!
//! A client of the daemon only (tests/workspace_boundaries.rs): the rows
//! come from `List`, transcripts from `Transcript` (the cache, else the
//! batch endpoint after a 200 ms pause), local titles go through
//! `SetTitle`, and marks are applied through `Mutate`, as `archive` and
//! `delete` apply them, after the user types `apply`. Marks aren't kept.
//!
//! - [`model`]: the filters and the list's window;
//! - [`app`]: the state and what each key does;
//! - [`ui`]: drawing it;
//! - [`run`]: the terminal, the keyboard and the daemon requests.

mod app;
mod input;
mod model;
mod run;
mod ui;

/// Run the TUI until the user quits.
pub use run::run;

#[cfg(test)]
mod tests;
