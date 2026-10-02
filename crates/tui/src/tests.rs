//! The TUI driven by keys, as the TS CLI's `src/tui/app.test.tsx` drives
//! it, with its screens drawn on ratatui's `TestBackend` and kept as
//! snapshots (`src/snapshots/`). The daemon is played by hand: each test
//! checks the effects a key asks for and answers them.

#![allow(clippy::unwrap_used)]

use chatgpt_protocol::{Row, Target};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use chatgpt_protocol::ChatTranscript;

use crate::app::{App, Effect, IndexStamp, Mode, Outcome, Transcript};
use crate::model::Mark;
use crate::model::tests::{judged, row};

const T: &str = "2024-05-01T10:00:00Z";
// 2026-09-27T00:00:00Z.
fn clock() -> i64 {
    1_790_467_200_000
}

fn rows() -> Vec<Row> {
    let mut long = judged(
        row("b2", "Postgres backup plan", false, T),
        "keep",
        "coding_general",
        None,
        false,
    );
    long.project_id = Some("g-p-1".into());
    vec![
        judged(
            row("a1", "Empty test chat", false, T),
            "delete",
            "coding_general",
            None,
            false,
        ),
        long,
        row("c3", "Unjudged third chat", false, T),
        judged(
            row("d4", "Archived idea", true, "2023-01-01T10:00:00Z"),
            "keep",
            "writing_projects",
            Some("writing"),
            false,
        ),
    ]
}

fn topics() -> Vec<String> {
    vec!["coding_general".into(), "writing_projects".into()]
}

/// Chat `id` at the revision the fixtures have.
fn key(id: &str) -> (String, String) {
    (id.to_owned(), T.to_owned())
}

/// The ticket of the one request among `effects`.
fn ticket_of(effects: &[Effect]) -> u64 {
    effects
        .iter()
        .find_map(|effect| match effect {
            Effect::Apply { ticket, .. }
            | Effect::SaveTitle { ticket, .. }
            | Effect::Reload { ticket } => Some(*ticket),
            _ => None,
        })
        .unwrap()
}

fn transcript(markdown: Option<&str>, summary: Option<&str>) -> ChatTranscript {
    ChatTranscript {
        markdown: markdown.map(str::to_owned),
        summary: summary.map(str::to_owned),
        ..ChatTranscript::default()
    }
}

struct Harness {
    app: App,
    terminal: Terminal<TestBackend>,
    effects: Vec<Effect>,
}

impl Harness {
    fn new() -> Self {
        let mut app = App::new(rows(), topics(), clock);
        let terminal = Terminal::new(TestBackend::new(140, 20)).unwrap();
        let effects = app.first_effects();
        let mut harness = Self {
            app,
            terminal,
            effects,
        };
        harness.draw();
        harness
    }

    fn draw(&mut self) -> String {
        self.terminal
            .draw(|frame| crate::ui::draw(frame, &mut self.app))
            .unwrap();
        self.terminal.backend().to_string()
    }

    fn key(&mut self, code: KeyCode) -> Vec<Effect> {
        self.key_with(code, KeyModifiers::NONE)
    }

    fn key_with(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Vec<Effect> {
        let effects = self.app.on_key(KeyEvent::new(code, modifiers));
        self.draw();
        effects
    }

    fn keys(&mut self, text: &str) -> Vec<Effect> {
        let mut effects = Vec::new();
        for c in text.chars() {
            effects.extend(self.key(KeyCode::Char(c)));
        }
        effects
    }

    fn answer(&mut self, outcome: Outcome) -> Vec<Effect> {
        let effects = self.app.on_outcome(outcome);
        self.draw();
        effects
    }

    /// The cache's answer for the selected chat.
    fn cached(&mut self, id: &str, markdown: &str, summary: Option<&str>) {
        self.answer(Outcome::Transcript {
            key: key(id),
            fetched: false,
            result: Ok(transcript(Some(markdown), summary)),
        });
    }

    fn screen(&mut self) -> String {
        self.draw()
    }
}

#[test]
fn lists_chats_with_suggestions_and_previews_the_selection() {
    let mut h = Harness::new();
    assert_eq!(
        h.effects,
        [Effect::Transcript {
            key: key("a1"),
            fetch: false
        }],
        "the first chat's transcript, from the cache"
    );
    h.cached("a1", "# chat a1\n\nhello from a1", None);
    let screen = h.screen();
    assert!(screen.contains("3 of 4"), "{screen}");
    assert!(screen.contains("updated 2024-05-01"));
    assert!(screen.contains("Jev: DELETE · coding_general"));
    assert!(screen.contains("hello from a1"));
    insta::assert_snapshot!("list_and_preview", screen);
}

#[test]
fn an_uncached_transcript_is_fetched_after_the_debounce() {
    let mut h = Harness::new();
    h.answer(Outcome::Transcript {
        key: key("a1"),
        fetched: false,
        result: Ok(transcript(None, None)),
    });
    assert!(h.screen().contains("Loading transcript…"));
    let effects = h.app.on_outcome(Outcome::Transcript {
        key: key("a1"),
        fetched: false,
        result: Ok(transcript(None, None)),
    });
    assert_eq!(effects, [Effect::FetchLater { key: key("a1") }]);
    // Moving on first: the old chat's answer is ignored.
    let effects = h.key(KeyCode::Char('j'));
    assert_eq!(
        effects,
        [Effect::Transcript {
            key: key("b2"),
            fetch: false
        }]
    );
    h.answer(Outcome::Transcript {
        key: key("a1"),
        fetched: true,
        result: Ok(transcript(Some("stale"), None)),
    });
    assert_eq!(h.app.preview.transcript, Transcript::Loading);
    h.answer(Outcome::Transcript {
        key: key("b2"),
        fetched: true,
        result: Ok(ChatTranscript {
            fetch_error: Some("ChatGPT didn't return this conversation (deleted?)".into()),
            ..ChatTranscript::default()
        }),
    });
    assert!(
        h.screen()
            .contains("Couldn't load: ChatGPT didn't return this conversation (deleted?)")
    );
}

#[test]
fn a_long_chat_shows_the_summary_it_was_judged_from() {
    let mut h = Harness::new();
    h.key(KeyCode::Char('j'));
    h.cached(
        "b2",
        "# Postgres backup plan\n\n## Me\n\nhow do I back up postgres",
        Some("Planning nightly pg_dump backups."),
    );
    let screen = h.screen();
    assert!(screen.contains("in a project"));
    insta::assert_snapshot!("preview_with_summary", screen);
}

#[test]
fn filters_by_suggestion_age_topic_brainstorm_title_and_archive() {
    let mut h = Harness::new();
    h.key(KeyCode::Char('2'));
    let screen = h.screen();
    assert!(screen.contains("1 of 4 · active · delete"), "{screen}");
    assert!(screen.contains("Empty test chat"));
    assert!(!screen.contains("Postgres backup plan"));
    h.keys("1yy");
    assert!(
        h.screen()
            .contains("3 of 4 · active · all · all topics · older than 6m")
    );
    h.key(KeyCode::Char('Y'));
    h.key(KeyCode::Char('Y'));
    h.key(KeyCode::Char('5'));
    assert!(h.screen().contains("1 of 4 · active · unjudged"));
    h.keys("1t");
    assert!(
        h.screen()
            .contains("2 of 4 · active · all · coding_general")
    );
    h.key(KeyCode::Char('T'));
    h.key(KeyCode::Char('A'));
    assert!(h.screen().contains("1 of 4 · archived · all · all topics"));
    h.key(KeyCode::Char('b'));
    assert!(h.screen().contains("brainstorms: any"));
    assert!(h.screen().contains("idea:writing"));
    h.keys("bB");
    h.key(KeyCode::Char('B'));
    h.key(KeyCode::Char('A'));
    h.key(KeyCode::Char('/'));
    h.keys("POSTGRES");
    let screen = h.screen();
    assert!(screen.contains("1 of 4 · active · all · all topics · \"POSTGRES\""));
    insta::assert_snapshot!("filter_typing", screen);
    h.key(KeyCode::Enter);
    assert!(
        h.screen().contains("\"POSTGRES\""),
        "enter keeps the filter"
    );
    h.key(KeyCode::Esc);
    assert!(h.screen().contains("3 of 4 · active · all · all topics"));
    assert!(!h.screen().contains("POSTGRES"), "esc clears it");
    h.key(KeyCode::Char('2'));
    h.key(KeyCode::Char('j'));
    insta::assert_snapshot!("filters_no_match", {
        h.key(KeyCode::Char('A'));
        h.key(KeyCode::Char('2'));
        h.screen()
    });
}

#[test]
fn keys_pressed_before_a_draw_all_apply() {
    let mut h = Harness::new();
    h.app
        .on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    h.app
        .on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    h.app
        .on_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    h.app
        .on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    h.app
        .on_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
    let screen = h.screen();
    assert!(screen.contains("1 of 4 · active · delete"), "{screen}");
}

#[test]
fn help_shows_every_key_and_any_key_closes_it() {
    let mut h = Harness::new();
    h.key(KeyCode::Char('?'));
    let screen = h.screen();
    assert!(screen.contains("j/k"));
    assert!(screen.contains("quit"));
    insta::assert_snapshot!("help", screen);
    assert!(h.key(KeyCode::Char('q')).is_empty(), "q only closes help");
    assert_eq!(h.app.mode, Mode::Browse);
}

#[test]
fn help_wraps_on_a_narrow_terminal_and_says_when_it_cant_fit() {
    let mut h = Harness::new();
    h.terminal.backend_mut().resize(60, 30);
    h.key(KeyCode::Char('?'));
    let screen = h.screen();
    for words in [
        "ctrl-d/u",
        "clear filter",
        "unsure",
        "archived view",
        "3y",
        "other",
        "unmark",
        "apply marks",
        "reload data",
        "unchanged)",
        "close help",
    ] {
        assert!(screen.contains(words), "{words} is cut off:\n{screen}");
    }
    insta::assert_snapshot!("help_narrow", screen);

    h.key(KeyCode::Char('q'));
    h.terminal.backend_mut().resize(60, 12);
    h.key(KeyCode::Char('?'));
    let screen = h.screen();
    assert!(screen.contains("j/k"), "{screen}");
    assert!(
        screen.contains("… (a taller terminal shows every key)"),
        "{screen}"
    );
}

#[test]
fn marks_toggle_accept_and_count() {
    let mut h = Harness::new();
    h.key(KeyCode::Char('d'));
    assert_eq!(h.app.marks.get("a1"), Some(&Mark::Delete));
    h.key(KeyCode::Char('d'));
    assert!(h.app.marks.is_empty(), "d again unmarks");
    h.key(KeyCode::Enter);
    assert_eq!(
        h.app.marks.get("a1"),
        Some(&Mark::Delete),
        "Jev said delete"
    );
    assert_eq!(h.app.selected, 1, "and moved on");
    h.key(KeyCode::Char('a'));
    h.key(KeyCode::Enter);
    assert_eq!(h.app.marks.get("b2"), None, "keep unmarks");
    h.key(KeyCode::Enter);
    assert!(
        h.screen()
            .contains("Not classified yet; nothing to accept.")
    );
    h.key(KeyCode::Char('a'));
    h.key(KeyCode::Char('d'));
    h.key(KeyCode::Char('u'));
    assert_eq!(h.app.marks.len(), 1);
    h.key(KeyCode::Char('k'));
    h.key(KeyCode::Char('a'));
    let screen = h.screen();
    assert!(
        screen.contains("marked: 1 delete, 1 archive (x to apply)"),
        "{screen}"
    );
    insta::assert_snapshot!("marks", screen);
}

#[test]
fn apply_needs_apply_typed_and_sends_exactly_the_marks() {
    let mut h = Harness::new();
    h.key(KeyCode::Char('x'));
    assert!(
        h.screen()
            .contains("Nothing marked. d marks for delete, a for archive.")
    );
    h.key(KeyCode::Char('d'));
    h.key(KeyCode::Char('j'));
    h.key(KeyCode::Char('a'));
    h.key(KeyCode::Char('x'));
    let screen = h.screen();
    // The box wraps the message.
    assert!(screen.contains("Archive 1 and permanently delete 1 conversation(s)."));
    assert!(screen.contains("be undone."), "{screen}");
    insta::assert_snapshot!("apply_dialog", screen);
    // Anything but `apply` changes nothing.
    assert!(h.keys("yes").is_empty());
    assert!(h.key(KeyCode::Enter).is_empty());
    assert!(h.screen().contains("Not applied: type apply exactly."));
    assert_eq!(h.app.marks.len(), 2);
    h.key(KeyCode::Char('x'));
    h.keys("apply");
    h.key(KeyCode::Esc);
    assert_eq!(h.app.mode, Mode::Browse, "esc cancels");
    h.key(KeyCode::Char('x'));
    h.keys(" apply ");
    let effects = h.key(KeyCode::Enter);
    let ticket = ticket_of(&effects);
    assert_eq!(
        effects,
        [Effect::Apply {
            ticket,
            archive: vec![Target {
                id: "b2".into(),
                title: "Postgres backup plan".into()
            }],
            delete: vec![Target {
                id: "a1".into(),
                title: "Empty test chat".into()
            }],
        }]
    );
    assert!(
        h.key(KeyCode::Char('q')).is_empty(),
        "keys wait while applying"
    );
    h.answer(Outcome::ApplyProgress { ticket, done: 1 });
    insta::assert_snapshot!("applying", h.screen());
    let effects = h.answer(Outcome::Applied {
        ticket,
        result: Ok(Vec::new()),
    });
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, Effect::Reload { .. }))
    );
    assert!(h.app.marks.is_empty());
    assert!(h.screen().contains("Applied 2 change(s)."));
}

#[test]
fn a_failed_apply_says_how_many_failed_and_a_refused_one_keeps_the_marks() {
    let mut h = Harness::new();
    h.key(KeyCode::Char('d'));
    h.key(KeyCode::Char('x'));
    h.keys("apply");
    let ticket = ticket_of(&h.key(KeyCode::Enter));
    h.answer(Outcome::Applied {
        ticket,
        result: Ok(vec![
            "a1 Empty test chat: 500 from /backend-api/conversation/a1".into(),
        ]),
    });
    assert!(
        h.screen()
            .contains("1 failed: a1 Empty test chat: 500 from /backend-api/conversation/a1")
    );
    h.key(KeyCode::Char('d'));
    h.key(KeyCode::Char('x'));
    h.keys("apply");
    let ticket = ticket_of(&h.key(KeyCode::Enter));
    h.answer(Outcome::Applied {
        ticket,
        result: Err("the daemon isn't running".into()),
    });
    assert_eq!(h.app.marks.len(), 1);
}

#[test]
fn quitting_with_marks_takes_two_presses() {
    let mut h = Harness::new();
    h.key(KeyCode::Char('d'));
    assert!(h.key(KeyCode::Char('q')).is_empty());
    let screen = h.screen();
    assert!(screen.contains("1 unapplied mark(s). Press q again to quit without applying."));
    insta::assert_snapshot!("quit_with_marks", screen);
    // Another key disarms it.
    h.key(KeyCode::Char('j'));
    assert!(h.key(KeyCode::Char('q')).is_empty());
    assert_eq!(h.key(KeyCode::Char('q')), [Effect::Quit]);
    // Without marks, once; ctrl-c always.
    let mut h = Harness::new();
    assert_eq!(h.key(KeyCode::Char('q')), [Effect::Quit]);
    h.key(KeyCode::Char('d'));
    assert_eq!(
        h.key_with(KeyCode::Char('c'), KeyModifiers::CONTROL),
        [Effect::Quit]
    );
}

#[test]
fn edits_a_local_title() {
    let mut h = Harness::new();
    h.key(KeyCode::Char('n'));
    assert!(h.screen().contains("Edit local title"));
    h.key_with(KeyCode::Char('a'), KeyModifiers::CONTROL);
    h.key_with(KeyCode::Char('k'), KeyModifiers::CONTROL);
    h.keys("Better chat title");
    insta::assert_snapshot!("title_edit", h.screen());
    let effects = h.key(KeyCode::Enter);
    let refused = ticket_of(&effects);
    assert_eq!(
        effects,
        [Effect::SaveTitle {
            ticket: refused,
            id: "a1".into(),
            title: "Better chat title".into()
        }]
    );
    h.answer(Outcome::TitleSaved {
        ticket: refused,
        result: Err("Local title must be 1–100 characters.".into()),
    });
    assert!(matches!(h.app.mode, Mode::Title { .. }), "stays open");
    let saved = ticket_of(&h.key(KeyCode::Enter));
    let effects = h.answer(Outcome::TitleSaved {
        ticket: saved,
        result: Ok(()),
    });
    let reload = ticket_of(&effects);
    assert_eq!(effects, [Effect::Reload { ticket: reload }]);
    let mut renamed = rows();
    renamed[0].display_title = "Better chat title".into();
    renamed[0].local_title = Some("Better chat title".into());
    h.answer(Outcome::Reloaded {
        ticket: reload,
        result: Ok(renamed),
    });
    let screen = h.screen();
    assert!(screen.contains("Better chat title"));
    assert!(screen.contains("Local title saved."));
}

#[test]
fn copy_open_reload_and_preview_scrolling() {
    let mut h = Harness::new();
    h.key(KeyCode::Char('c'));
    assert!(h.screen().contains("Transcript not loaded yet."));
    let long: String = (1..=60).map(|n| format!("line {n}\n")).collect();
    h.cached("a1", &long, None);
    assert_eq!(
        h.key(KeyCode::Char('c')),
        [Effect::Copy {
            markdown: long.clone(),
            title: "Empty test chat".into()
        }]
    );
    assert_eq!(
        App::copied("Empty test chat", &"x".repeat(1536)),
        "Copied \"Empty test chat\" (2 KB)."
    );
    assert_eq!(
        h.key(KeyCode::Char('o')),
        [Effect::Open { id: "a1".into() }]
    );
    assert!(h.screen().contains("Opened in the browser."));
    assert!(matches!(
        h.key(KeyCode::Char('r'))[..],
        [Effect::Reload { .. }]
    ));
    assert!(h.screen().contains("Reloaded from the local index."));
    h.key(KeyCode::Char('J'));
    assert_eq!(h.app.preview.scroll, 3);
    h.key(KeyCode::Char(' '));
    assert!(h.app.preview.scroll > 3);
    h.key(KeyCode::Char('K'));
    for _ in 0..40 {
        h.key(KeyCode::Char(' '));
    }
    let max = h.app.preview_lines - h.app.preview_height;
    assert_eq!(h.app.preview.scroll, max, "never past the end");
    h.key(KeyCode::Char('G'));
    assert_eq!(h.app.selected, 2);
    assert_eq!(h.app.preview.scroll, 0, "a new chat starts at the top");
    h.key(KeyCode::Char('g'));
    assert_eq!(h.app.selected, 0);
    h.key_with(KeyCode::Char('d'), KeyModifiers::CONTROL);
    assert_eq!(h.app.selected, 2, "a page down, clamped");
    h.key(KeyCode::PageUp);
    assert_eq!(h.app.selected, 0);
}

#[test]
fn a_resize_redraws_at_the_new_size() {
    let mut h = Harness::new();
    h.cached("a1", "hello from a1", None);
    h.terminal.backend_mut().resize(100, 12);
    let screen = h.screen();
    assert_eq!(
        h.app.list_height, 8,
        "12 rows less header, footer and borders"
    );
    assert!(screen.contains("3 of 4"));
    insta::assert_snapshot!("small_terminal", screen);
}

/// A title save still out when the box is closed, the chat marked and an
/// apply started: its late answer mustn't unlock the apply, or a second
/// `x` would send the marks again.
#[test]
fn a_late_answer_never_unlocks_a_running_apply() {
    let mut h = Harness::new();
    h.key(KeyCode::Char('n'));
    let title = ticket_of(&h.key(KeyCode::Enter));
    h.key(KeyCode::Esc);
    h.key(KeyCode::Char('d'));
    h.key(KeyCode::Char('x'));
    h.keys("apply");
    let apply = ticket_of(&h.key(KeyCode::Enter));
    assert!(matches!(h.app.mode, Mode::Applying { .. }));
    let effects = h.answer(Outcome::TitleSaved {
        ticket: title,
        result: Ok(()),
    });
    assert!(
        matches!(h.app.mode, Mode::Applying { .. }),
        "still applying"
    );
    assert!(
        matches!(effects[..], [Effect::Reload { .. }]),
        "the title still shows"
    );
    // A late refusal, or another apply's answer, changes nothing either.
    h.answer(Outcome::TitleSaved {
        ticket: title,
        result: Err("refused".into()),
    });
    h.answer(Outcome::Applied {
        ticket: apply + 100,
        result: Ok(Vec::new()),
    });
    assert!(matches!(h.app.mode, Mode::Applying { .. }));
    assert!(h.key(KeyCode::Char('x')).is_empty(), "no second apply");
    h.answer(Outcome::Applied {
        ticket: apply,
        result: Ok(Vec::new()),
    });
    assert_eq!(h.app.mode, Mode::Browse);
}

/// Two reloads out at once: only the last one's rows are used.
#[test]
fn only_the_latest_reload_counts() {
    let mut h = Harness::new();
    let first = ticket_of(&h.key(KeyCode::Char('r')));
    let second = ticket_of(&h.key(KeyCode::Char('r')));
    let mut newer = rows();
    newer.truncate(1);
    h.answer(Outcome::Reloaded {
        ticket: second,
        result: Ok(newer),
    });
    h.answer(Outcome::Reloaded {
        ticket: first,
        result: Ok(rows()),
    });
    assert_eq!(
        h.app.rows.len(),
        1,
        "the older answer came last and was dropped"
    );
}

/// Chat A shown at t1, synced to t2 meanwhile: a reload fetches it again,
/// drops the t1 answer, and `c` copies the t2 text.
#[test]
fn a_reload_that_brings_a_newer_revision_refetches_the_preview() {
    let mut h = Harness::new();
    h.cached("a1", "old text", Some("old summary"));
    let reload = ticket_of(&h.key(KeyCode::Char('r')));
    let mut synced = rows();
    synced[0].update_time = "2024-06-01T10:00:00Z".into();
    let new_key = ("a1".to_owned(), "2024-06-01T10:00:00Z".to_owned());
    let effects = h.answer(Outcome::Reloaded {
        ticket: reload,
        result: Ok(synced),
    });
    assert_eq!(
        effects,
        [Effect::Transcript {
            key: new_key.clone(),
            fetch: false
        }]
    );
    assert_eq!(h.app.preview.transcript, Transcript::Loading);
    assert_eq!(h.app.preview.summary, None);
    assert!(
        h.key(KeyCode::Char('c')).is_empty(),
        "nothing to copy until the new text is in"
    );
    // A late answer for t1 is dropped.
    h.cached("a1", "old text again", None);
    assert_eq!(h.app.preview.transcript, Transcript::Loading);
    h.answer(Outcome::Transcript {
        key: new_key,
        fetched: false,
        result: Ok(transcript(Some("new text"), None)),
    });
    assert_eq!(
        h.key(KeyCode::Char('c')),
        [Effect::Copy {
            markdown: "new text".into(),
            title: "Empty test chat".into()
        }]
    );
}

/// Title A, then B for the same chat (the box closed and opened again)
/// before A answers: B waits for A, so B is the title that lands; a C typed
/// meanwhile replaces B in the wait, and only C follows A.
#[test]
fn title_saves_to_one_chat_land_in_the_order_typed() {
    let mut h = Harness::new();
    let type_title = |h: &mut Harness, title: &str| -> Vec<Effect> {
        h.key(KeyCode::Char('n'));
        h.key_with(KeyCode::Char('a'), KeyModifiers::CONTROL);
        h.key_with(KeyCode::Char('k'), KeyModifiers::CONTROL);
        h.keys(title);
        let effects = h.key(KeyCode::Enter);
        h.key(KeyCode::Esc);
        effects
    };
    let first = type_title(&mut h, "Title A");
    let a = ticket_of(&first);
    assert!(type_title(&mut h, "Title B").is_empty(), "B waits for A");
    assert!(
        type_title(&mut h, "Title C").is_empty(),
        "C replaces B in the wait"
    );
    let effects = h.answer(Outcome::TitleSaved {
        ticket: a,
        result: Ok(()),
    });
    let next = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::SaveTitle { ticket, id, title } => Some((*ticket, id.clone(), title.clone())),
            _ => None,
        })
        .unwrap();
    assert_eq!((next.1.as_str(), next.2.as_str()), ("a1", "Title C"));
    // C's answer sends nothing more.
    let effects = h.answer(Outcome::TitleSaved {
        ticket: next.0,
        result: Ok(()),
    });
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::SaveTitle { .. }))
    );
    // Another chat's title never waits on this one's.
    h.key(KeyCode::Char('j'));
    let other = type_title(&mut h, "Other");
    assert!(matches!(other[..], [Effect::SaveTitle { .. }]));
}

/// A poll that finds the index moved reloads, but never under an open box;
/// the reload keeps the cursor on its chat and drops marks on chats gone.
#[test]
fn a_changed_index_reloads_in_browse_only_and_keeps_the_cursor() {
    let mut h = Harness::new();
    let first = IndexStamp {
        sync_finished_at: Some(1),
        ..IndexStamp::default()
    };
    let moved = IndexStamp {
        sync_finished_at: Some(2),
        ..IndexStamp::default()
    };
    h.app.index = Some(first.clone());
    assert!(h.answer(Outcome::Index(first)).is_empty(), "unchanged");

    h.key(KeyCode::Char('j'));
    h.key(KeyCode::Char('d'));
    h.key(KeyCode::Char('j'));
    h.key(KeyCode::Char('a'));
    assert_eq!(h.app.current().unwrap().id, "c3");
    h.key(KeyCode::Char('?'));
    assert!(
        h.answer(Outcome::Index(moved.clone())).is_empty(),
        "help is open"
    );
    h.key(KeyCode::Char('q'));
    h.key(KeyCode::Char('/'));
    assert!(
        h.answer(Outcome::Index(moved.clone())).is_empty(),
        "a filter is being typed"
    );
    h.key(KeyCode::Esc);
    h.key(KeyCode::Char('x'));
    assert!(
        h.answer(Outcome::Index(moved.clone())).is_empty(),
        "the apply box is open"
    );
    h.key(KeyCode::Esc);
    // Clearing the filter went back to the top.
    h.key(KeyCode::Char('G'));
    assert_eq!(h.app.current().unwrap().id, "c3");

    let reload = ticket_of(&h.answer(Outcome::Index(moved.clone())));
    assert!(
        h.screen()
            .contains("The index changed in the background; reloaded.")
    );
    assert_eq!(h.app.index, Some(moved));
    // A new chat on top, and b2 (marked) deleted elsewhere.
    let mut synced = rows();
    synced.retain(|row| row.id != "b2");
    synced.insert(0, row("e5", "Newest chat", false, "2024-06-01T10:00:00Z"));
    h.answer(Outcome::Reloaded {
        ticket: reload,
        result: Ok(synced),
    });
    assert_eq!(h.app.current().unwrap().id, "c3", "the cursor followed c3");
    assert_eq!(h.app.selected, 2);
    assert_eq!(h.app.mark_counts(), (0, 1), "b2's mark went with it");
}

/// A long transcript's first frame wraps only what's shown (and a page
/// more); scrolling wraps on, and the end is found as before.
#[test]
fn a_long_transcript_is_wrapped_only_as_far_as_its_shown() {
    let mut h = Harness::new();
    let long: String = (1..=50_000).map(|n| format!("line {n}\n")).collect();
    h.cached("a1", &long, Some("short summary"));
    let wrapped = |h: &Harness| h.app.wrapped.as_ref().unwrap().lines.len();
    assert!(wrapped(&h) < 100, "{} lines wrapped", wrapped(&h));
    assert!(h.screen().contains("line 1"));
    assert_eq!(h.app.preview_lines, usize::MAX, "the end isn't known yet");
    for _ in 0..10 {
        h.key(KeyCode::Char(' '));
    }
    assert!(
        h.screen()
            .contains(&format!("line {}", h.app.preview.scroll - 4))
    );
    assert!(wrapped(&h) < 300);
    // Scrolled far past the end at once: all of it is wrapped, and the
    // scroll comes back to the last page.
    h.app.preview.scroll = 1_000_000;
    h.draw();
    // Summary (header, text, blank, rule, blank), then 50,000 lines and the
    // empty one after the last newline.
    assert_eq!(h.app.preview_lines, 5 + 50_001);
    for _ in 0..3 {
        h.key(KeyCode::Char(' '));
    }
    assert_eq!(
        h.app.preview.scroll,
        h.app.preview_lines - h.app.preview_height,
        "never past the end"
    );
    assert!(h.screen().contains("line 50000"));
}

/// A reload already out when `x` opens the apply box answers while it's
/// open, or while the apply runs: the rows and marks the user confirmed
/// stay as they were, and the held rows show once the box closes.
#[test]
fn a_reload_answering_under_the_apply_box_waits_for_it_to_close() {
    // Rows reordered, and b2 (marked) gone.
    let synced = || {
        let mut synced = rows();
        synced.retain(|row| row.id != "b2");
        synced.reverse();
        synced
    };
    let mut h = Harness::new();
    h.key(KeyCode::Char('j'));
    h.key(KeyCode::Char('d'));
    let reload = ticket_of(&h.key(KeyCode::Char('r')));
    h.key(KeyCode::Char('x'));
    assert!(matches!(h.app.mode, Mode::Confirm(_)));
    assert!(
        h.answer(Outcome::Reloaded {
            ticket: reload,
            result: Ok(synced()),
        })
        .is_empty()
    );
    assert_eq!(h.app.rows, rows(), "the rows under the box didn't change");
    assert_eq!(h.app.mark_counts(), (1, 0));
    assert!(h.screen().contains("permanently delete 1 conversation(s)"));
    // Esc: the held rows show, and b2's mark goes with it.
    h.key(KeyCode::Esc);
    assert_eq!(h.app.rows, synced());
    assert_eq!(h.app.mark_counts(), (0, 0));

    // The answer after `apply`, while it runs.
    let mut h = Harness::new();
    h.key(KeyCode::Char('j'));
    h.key(KeyCode::Char('d'));
    let reload = ticket_of(&h.key(KeyCode::Char('r')));
    h.key(KeyCode::Char('x'));
    let effects = h.keys("apply");
    assert!(effects.is_empty());
    let effects = h.key(KeyCode::Enter);
    assert_eq!(
        effects,
        [Effect::Apply {
            ticket: ticket_of(&effects),
            archive: Vec::new(),
            delete: vec![Target {
                id: "b2".into(),
                title: "Postgres backup plan".into(),
            }],
        }]
    );
    let apply = ticket_of(&effects);
    h.answer(Outcome::Reloaded {
        ticket: reload,
        result: Ok(synced()),
    });
    assert_eq!(h.app.rows, rows(), "the rows under the apply didn't change");
    assert!(matches!(h.app.mode, Mode::Applying { .. }));
    // The apply's own reload supersedes the held one.
    let effects = h.answer(Outcome::Applied {
        ticket: apply,
        result: Ok(Vec::new()),
    });
    let after = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::Reload { ticket } => Some(*ticket),
            _ => None,
        })
        .unwrap();
    h.key(KeyCode::Char('j'));
    assert_eq!(h.app.rows, rows(), "the stale held rows never showed");
    h.answer(Outcome::Reloaded {
        ticket: after,
        result: Ok(synced()),
    });
    assert_eq!(h.app.rows, synced());
}
