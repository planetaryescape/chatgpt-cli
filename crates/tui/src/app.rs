//! The TUI's state and what each key does, ported from the TS CLI's
//! `src/tui/app.tsx` @ 1b8c950. Pure: a key or a daemon answer changes the
//! state and returns the [`Effect`]s to carry out (requests to the daemon,
//! the clipboard, the browser), which `run.rs` runs and answers with an
//! [`Outcome`]. Tests drive it directly.

use std::collections::HashMap;
use std::time::Duration;

use chatgpt_core::js::{trim, utf16_len};
use chatgpt_protocol::{ChatTranscript, Row, Target};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::input::LineInput;
use crate::model::{
    AGE_CYCLE, BRAINSTORM_CYCLE, Mark, Suggestion, View, cycle, visible_rows, window_start,
};

/// Waiting before a fetch keeps fast `j`/`k` scrolling from firing a
/// request per row.
pub const DEBOUNCE: Duration = Duration::from_millis(200);

pub const HELP: &str = "j/k ↑/↓  move            g/G  top/bottom       ctrl-d/u  page
J/K      scroll preview  /    filter titles    esc  clear filter
1-6      all · delete · archive · keep · unjudged · unsure
t/T      next/previous topic                    A    archived view
y/Y      older than: any · 30d · 6m · 1y · 2y · 3y
b/B      brainstorms: off · any · writing · sermon · product · other
d / a    mark delete / archive (again to unmark)   u  unmark
enter    take suggestion and move on             x    apply marks
c        copy transcript    o  open in browser  r    reload data
n        edit local title (ChatGPT title stays unchanged)
q        quit               ?  close help";

pub const HINTS: &str = "? help · / filter · 1-6 suggestion · t topic · y age · b brainstorms · n title · d/a mark · enter accept · x apply · q quit";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Browse,
    /// Typing the title filter, which applies as it's typed.
    Filter(LineInput),
    Help,
    /// The apply dialog: the user must type `apply`.
    Confirm(LineInput),
    /// Editing chat `id`'s local title; `saving` while the save asked for
    /// with that ticket is out.
    Title {
        id: String,
        input: LineInput,
        saving: Option<u64>,
    },
    /// Until the apply asked for with `ticket` answers: nothing else
    /// unlocks it.
    Applying {
        ticket: u64,
        done: usize,
        total: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Transcript {
    Idle,
    Loading,
    Ready(String),
    Failed(String),
}

/// A chat at one revision: its id and `update_time`.
pub type ChatKey = (String, String);

/// The selected chat's transcript and summary, for the revision the list
/// shows: a reload that brings a newer one fetches it again.
#[derive(Clone, Debug)]
pub struct Preview {
    pub key: Option<ChatKey>,
    pub transcript: Transcript,
    pub summary: Option<String>,
    /// Lines scrolled past.
    pub scroll: usize,
    /// Bumped whenever the text changes, so the wrapped lines are redone.
    pub version: u64,
}

/// What the runner does for the app. A `ticket` comes back on the
/// outcome, so a late answer is matched with what asked for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// A chat's transcript: from the cache only, or (`fetch`) fetched when
    /// the cache lacks it.
    Transcript {
        key: ChatKey,
        fetch: bool,
    },
    /// Fetch the transcript after [`DEBOUNCE`], if it's still selected.
    FetchLater {
        key: ChatKey,
    },
    /// Every chat again, from the index.
    Reload {
        ticket: u64,
    },
    /// Archive, then delete, exactly these chats.
    Apply {
        ticket: u64,
        archive: Vec<Target>,
        delete: Vec<Target>,
    },
    SaveTitle {
        ticket: u64,
        id: String,
        title: String,
    },
    Copy {
        markdown: String,
        title: String,
    },
    Open {
        id: String,
    },
    Quit,
}

/// What came back from an [`Effect`].
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Transcript {
        key: ChatKey,
        fetched: bool,
        result: Result<ChatTranscript, String>,
    },
    Reloaded {
        ticket: u64,
        result: Result<Vec<Row>, String>,
    },
    /// Chats attempted so far in an apply.
    ApplyProgress { ticket: u64, done: usize },
    /// Each failure as `<id> <title>: <why>`, or why nothing was applied.
    Applied {
        ticket: u64,
        result: Result<Vec<String>, String>,
    },
    TitleSaved {
        ticket: u64,
        result: Result<(), String>,
    },
    /// The status line to show.
    Copied(Result<String, String>),
}

pub struct App {
    pub rows: Vec<Row>,
    /// `all`, then every topic.
    topics: Vec<String>,
    pub view: View,
    /// Indices into `rows`.
    pub visible: Vec<usize>,
    pub selected: usize,
    pub start: usize,
    pub marks: HashMap<String, Mark>,
    pub mode: Mode,
    pub status: String,
    quit_armed: bool,
    pub preview: Preview,
    clock: fn() -> i64,
    /// The list's rows, as last drawn.
    pub list_height: usize,
    /// The preview's scrolling area, as last drawn: its height, and how
    /// many lines its text wraps to.
    pub preview_height: usize,
    pub preview_lines: usize,
    /// The preview's text wrapped: for which `Preview::version` and
    /// width. Wrapping a long transcript on every keypress would lag.
    pub wrapped: Option<(u64, usize, Vec<ratatui::text::Line<'static>>)>,
    /// The last ticket handed out, and the reload whose answer counts.
    tickets: u64,
    reloading: u64,
    /// Per chat: the title save that's out, and the latest title typed
    /// while it was.
    titles_saving: HashMap<String, u64>,
    titles_waiting: HashMap<String, (u64, String)>,
}

impl App {
    /// `topics`: the daemon's, in its order. `clock`: now, in Unix
    /// milliseconds, for the age filter.
    pub fn new(rows: Vec<Row>, topics: Vec<String>, clock: fn() -> i64) -> Self {
        let mut app = Self {
            rows,
            topics: std::iter::once("all".to_owned()).chain(topics).collect(),
            view: View::default(),
            visible: Vec::new(),
            selected: 0,
            start: 0,
            marks: HashMap::new(),
            mode: Mode::Browse,
            status: String::new(),
            quit_armed: false,
            preview: Preview {
                key: None,
                transcript: Transcript::Idle,
                summary: None,
                scroll: 0,
                version: 0,
            },
            clock,
            list_height: 1,
            preview_height: 1,
            preview_lines: 0,
            wrapped: None,
            tickets: 0,
            reloading: 0,
            titles_saving: HashMap::new(),
            titles_waiting: HashMap::new(),
        };
        app.visible = visible_rows(&app.rows, &app.view, clock());
        app
    }

    /// The effects the first screen needs: the selected chat's transcript.
    pub fn first_effects(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.sync_preview(&mut effects);
        effects
    }

    pub fn current(&self) -> Option<&Row> {
        let last = self.visible.len().checked_sub(1)?;
        self.visible
            .get(self.selected.min(last))
            .and_then(|&index| self.rows.get(index))
    }

    /// Delete and archive marks.
    pub fn mark_counts(&self) -> (usize, usize) {
        let delete = self
            .marks
            .values()
            .filter(|mark| **mark == Mark::Delete)
            .count();
        (delete, self.marks.len() - delete)
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let mut effects = Vec::new();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            return vec![Effect::Quit];
        }
        // Taken out while the key is handled, and put back unless the key
        // changed it.
        match std::mem::replace(&mut self.mode, Mode::Browse) {
            applying @ Mode::Applying { .. } => self.mode = applying,
            Mode::Help => {}
            Mode::Filter(mut input) => match key.code {
                KeyCode::Esc => self.change_view(&mut effects, |view| view.query.clear()),
                KeyCode::Enter => {}
                _ => {
                    let changed = input.edit(key);
                    let query = input.text().to_owned();
                    self.mode = Mode::Filter(input);
                    if changed {
                        self.change_view(&mut effects, |view| view.query = query);
                    }
                }
            },
            Mode::Confirm(mut input) => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    if trim(input.text()) == "apply" {
                        effects.push(self.apply());
                    } else {
                        "Not applied: type apply exactly.".clone_into(&mut self.status);
                    }
                }
                _ => {
                    input.edit(key);
                    self.mode = Mode::Confirm(input);
                }
            },
            Mode::Title {
                id,
                mut input,
                saving,
            } => match key.code {
                // A save still out lands in the index, but no longer
                // touches the screen.
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    let ticket = self.save_title(&id, input.text(), &mut effects);
                    // Until the daemon answers: a refused title stays open.
                    self.mode = Mode::Title {
                        id,
                        input,
                        saving: Some(ticket),
                    };
                }
                _ => {
                    input.edit(key);
                    self.mode = Mode::Title { id, input, saving };
                }
            },
            Mode::Browse => self.browse_key(key, &mut effects),
        }
        effects
    }

    fn browse_key(&mut self, key: KeyEvent, effects: &mut Vec<Effect>) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // As OpenTUI names keys: a letter by its lower case, with shift.
        let (name, shifted) = match key.code {
            KeyCode::Char(c) if c.is_alphabetic() => (
                c.to_lowercase().next().unwrap_or(c),
                c.is_uppercase() || key.modifiers.contains(KeyModifiers::SHIFT),
            ),
            KeyCode::Char(c) => (c, false),
            _ => ('\0', false),
        };
        if name != 'q' || ctrl {
            self.quit_armed = false;
        }
        let list_height = self.list_height;
        match key.code {
            KeyCode::Esc => return self.change_view(effects, |view| view.query.clear()),
            KeyCode::Down => return self.move_by(effects, 1),
            KeyCode::Up => return self.move_by(effects, -1),
            KeyCode::PageDown => return self.move_by(effects, list_height as isize),
            KeyCode::PageUp => return self.move_by(effects, -(list_height as isize)),
            KeyCode::Enter => return self.accept(effects),
            _ => {}
        }
        if ctrl {
            match name {
                'd' => self.move_by(effects, list_height as isize),
                'u' => self.move_by(effects, -(list_height as isize)),
                _ => {}
            }
            return;
        }
        if let Some(suggestion) = Suggestion::for_key(name) {
            return self.change_view(effects, |view| view.suggestion = suggestion);
        }
        match (name, shifted) {
            ('q', _) => {
                if !self.marks.is_empty() && !self.quit_armed {
                    self.quit_armed = true;
                    self.status = format!(
                        "{} unapplied mark(s). Press q again to quit without applying.",
                        self.marks.len()
                    );
                } else {
                    effects.push(Effect::Quit);
                }
            }
            ('?', _) => self.mode = Mode::Help,
            ('/', _) => self.mode = Mode::Filter(LineInput::new(&self.view.query)),
            ('j', false) => self.move_by(effects, 1),
            ('k', false) => self.move_by(effects, -1),
            ('j', true) => self.scroll_by(3),
            ('k', true) => self.scroll_by(-3),
            (' ', _) => self.scroll_by(self.preview_height as isize),
            ('g', false) => self.select(effects, 0),
            ('g', true) => self.select(effects, self.visible.len().saturating_sub(1)),
            ('t', back) => {
                let topic = cycle(&self.topics, &self.view.topic, back);
                self.change_view(effects, |view| view.topic = topic);
            }
            ('a', true) => self.change_view(effects, |view| view.archived = !view.archived),
            ('y', back) => {
                let age = cycle(&AGE_CYCLE, &self.view.older_than, back);
                self.change_view(effects, |view| view.older_than = age);
            }
            ('b', back) => {
                let kind = cycle(&BRAINSTORM_CYCLE, &self.view.brainstorm, back);
                self.change_view(effects, |view| view.brainstorm = kind);
            }
            ('r', _) => {
                effects.push(self.reload());
                "Reloaded from the local index.".clone_into(&mut self.status);
            }
            ('x', _) => {
                if self.marks.is_empty() {
                    "Nothing marked. d marks for delete, a for archive."
                        .clone_into(&mut self.status);
                } else {
                    self.mode = Mode::Confirm(LineInput::default());
                }
            }
            (name, shifted) => self.row_key(name, shifted, effects),
        }
    }

    /// The keys that act on the selected chat.
    fn row_key(&mut self, name: char, shifted: bool, effects: &mut Vec<Effect>) {
        let Some(row) = self.current() else {
            return;
        };
        let id = row.id.clone();
        match (name, shifted) {
            ('n', _) => {
                let input = LineInput::new(&row.display_title);
                self.mode = Mode::Title {
                    id,
                    input,
                    saving: None,
                };
            }
            ('d', false) => self.toggle_mark(&id, Some(Mark::Delete)),
            ('a', false) => self.toggle_mark(&id, Some(Mark::Archive)),
            ('u', _) => self.toggle_mark(&id, None),
            ('o', _) => {
                effects.push(Effect::Open { id });
                "Opened in the browser.".clone_into(&mut self.status);
            }
            ('c', _) => match &self.preview.transcript {
                Transcript::Ready(markdown)
                    if self
                        .preview
                        .key
                        .as_ref()
                        .is_some_and(|(shown, _)| *shown == id) =>
                {
                    effects.push(Effect::Copy {
                        markdown: markdown.clone(),
                        title: row.display_title.clone(),
                    });
                }
                _ => "Transcript not loaded yet.".clone_into(&mut self.status),
            },
            _ => {}
        }
    }

    /// `enter`: take Jev's suggestion (delete or archive marks it, keep
    /// unmarks it) and move on.
    fn accept(&mut self, effects: &mut Vec<Effect>) {
        let Some(row) = self.current() else {
            return;
        };
        let id = row.id.clone();
        let Some(jev) = &row.jev else {
            "Not classified yet; nothing to accept.".clone_into(&mut self.status);
            return;
        };
        match Mark::for_suggestion(&jev.suggestion) {
            Some(mark) => self.marks.insert(id, mark),
            None => self.marks.remove(&id),
        };
        self.move_by(effects, 1);
    }

    fn toggle_mark(&mut self, id: &str, mark: Option<Mark>) {
        match mark {
            Some(mark) if self.marks.get(id) != Some(&mark) => {
                self.marks.insert(id.to_owned(), mark);
            }
            _ => {
                self.marks.remove(id);
            }
        }
    }

    /// The confirmed marks, as targets in the list's order: archives, then
    /// deletes.
    fn apply(&mut self) -> Effect {
        let mut archive = Vec::new();
        let mut delete = Vec::new();
        for row in &self.rows {
            let target = || Target {
                id: row.id.clone(),
                title: row.title.clone(),
            };
            match self.marks.get(&row.id) {
                Some(Mark::Archive) => archive.push(target()),
                Some(Mark::Delete) => delete.push(target()),
                None => {}
            }
        }
        let ticket = self.ticket();
        self.mode = Mode::Applying {
            ticket,
            done: 0,
            total: archive.len() + delete.len(),
        };
        Effect::Apply {
            ticket,
            archive,
            delete,
        }
    }

    /// A new request's ticket.
    fn ticket(&mut self) -> u64 {
        self.tickets += 1;
        self.tickets
    }

    /// Save `title` for chat `id`, or, while a save for it is out, after
    /// that one answers: only the latest title typed waits, so the last
    /// one typed is the one that lands. Its ticket.
    fn save_title(&mut self, id: &str, title: &str, effects: &mut Vec<Effect>) -> u64 {
        let ticket = self.ticket();
        if self.titles_saving.contains_key(id) {
            self.titles_waiting
                .insert(id.to_owned(), (ticket, title.to_owned()));
        } else {
            self.titles_saving.insert(id.to_owned(), ticket);
            effects.push(Effect::SaveTitle {
                ticket,
                id: id.to_owned(),
                title: title.to_owned(),
            });
        }
        ticket
    }

    /// Every chat again; only the latest reload's answer is used.
    fn reload(&mut self) -> Effect {
        let ticket = self.ticket();
        self.reloading = ticket;
        Effect::Reload { ticket }
    }

    pub fn on_outcome(&mut self, outcome: Outcome) -> Vec<Effect> {
        let mut effects = Vec::new();
        match outcome {
            Outcome::Transcript {
                key,
                fetched,
                result,
            } => {
                // For another chat, or another revision of this one.
                if self.preview.key.as_ref() != Some(&key) {
                    return effects;
                }
                match result {
                    Ok(chat) => {
                        self.preview.summary = chat.summary;
                        match (chat.markdown, chat.fetch_error) {
                            (Some(markdown), _) => {
                                self.preview.transcript = Transcript::Ready(markdown);
                            }
                            (None, Some(why)) => self.preview.transcript = Transcript::Failed(why),
                            (None, None) if !fetched => effects.push(Effect::FetchLater { key }),
                            (None, None) => {
                                self.preview.transcript =
                                    Transcript::Failed("not in the cache".to_owned());
                            }
                        }
                    }
                    Err(why) => self.preview.transcript = Transcript::Failed(why),
                }
                self.preview.version += 1;
            }
            Outcome::Reloaded { ticket, result } => {
                if ticket != self.reloading {
                    return effects;
                }
                match result {
                    Ok(rows) => {
                        self.rows = rows;
                        self.visible = visible_rows(&self.rows, &self.view, (self.clock)());
                        self.sync_preview(&mut effects);
                    }
                    Err(why) => self.status = why,
                }
            }
            Outcome::ApplyProgress { ticket, done } => {
                if let Mode::Applying {
                    ticket: running,
                    done: shown,
                    ..
                } = &mut self.mode
                    && *running == ticket
                {
                    *shown = done;
                }
            }
            Outcome::Applied { ticket, result } => {
                let total = match &self.mode {
                    Mode::Applying {
                        ticket: running,
                        total,
                        ..
                    } if *running == ticket => *total,
                    // Not the apply on screen: nothing to unlock.
                    _ => return effects,
                };
                self.mode = Mode::Browse;
                match result {
                    Ok(failures) => {
                        self.marks.clear();
                        self.status = match failures.first() {
                            None => format!("Applied {total} change(s)."),
                            Some(first) => format!("{} failed: {first}", failures.len()),
                        };
                    }
                    // Marks stay, to try again.
                    Err(why) => self.status = why,
                }
                self.select(&mut effects, 0);
                effects.push(self.reload());
            }
            Outcome::TitleSaved { ticket, result } => {
                // The chat's next title goes out only now, so saves to one
                // chat land in the order they were typed.
                if let Some(id) = self
                    .titles_saving
                    .iter()
                    .find_map(|(id, saving)| (*saving == ticket).then(|| id.clone()))
                {
                    self.titles_saving.remove(&id);
                    if let Some((next, title)) = self.titles_waiting.remove(&id) {
                        self.titles_saving.insert(id.clone(), next);
                        effects.push(Effect::SaveTitle {
                            ticket: next,
                            id,
                            title,
                        });
                    }
                }
                let open = matches!(&self.mode, Mode::Title { saving: Some(saving), .. } if *saving == ticket);
                match result {
                    Ok(()) => {
                        if open {
                            "Local title saved.".clone_into(&mut self.status);
                            self.mode = Mode::Browse;
                        }
                        // Saved either way: the list shows it.
                        effects.push(self.reload());
                    }
                    Err(why) if open => {
                        self.status = why;
                        if let Mode::Title { saving, .. } = &mut self.mode {
                            *saving = None;
                        }
                    }
                    // A refusal for a box already closed changed nothing.
                    Err(_) => {}
                }
            }
            Outcome::Copied(Ok(status) | Err(status)) => self.status = status,
        }
        effects
    }

    /// The status line after a copy.
    pub fn copied(title: &str, markdown: &str) -> String {
        let kb = (utf16_len(markdown) as f64 / 1024.0).round();
        format!("Copied \"{title}\" ({kb} KB).")
    }

    fn select(&mut self, effects: &mut Vec<Effect>, next: usize) {
        let total = self.visible.len();
        let clamped = next.min(total.saturating_sub(1));
        self.selected = clamped;
        self.start = window_start(clamped, self.start, self.list_height, total);
        self.preview.scroll = 0;
        self.sync_preview(effects);
    }

    fn move_by(&mut self, effects: &mut Vec<Effect>, delta: isize) {
        let next = self.selected.saturating_add_signed(delta);
        self.select(effects, next);
    }

    fn scroll_by(&mut self, delta: isize) {
        let max = self.preview_lines.saturating_sub(self.preview_height);
        self.preview.scroll = self.preview.scroll.saturating_add_signed(delta).min(max);
    }

    fn change_view(&mut self, effects: &mut Vec<Effect>, patch: impl FnOnce(&mut View)) {
        patch(&mut self.view);
        self.visible = visible_rows(&self.rows, &self.view, (self.clock)());
        self.selected = 0;
        self.start = 0;
        self.sync_preview(effects);
    }

    /// A newly selected chat's transcript, or a new revision of the same
    /// chat's (after a reload): from the cache now, fetched after the
    /// debounce if it isn't there.
    fn sync_preview(&mut self, effects: &mut Vec<Effect>) {
        let key = self
            .current()
            .map(|row| (row.id.clone(), row.update_time.clone()));
        if self.preview.key == key {
            return;
        }
        let same_chat = matches!((&self.preview.key, &key), (Some((a, _)), Some((b, _))) if a == b);
        self.preview = Preview {
            transcript: if key.is_some() {
                Transcript::Loading
            } else {
                Transcript::Idle
            },
            key: key.clone(),
            summary: None,
            scroll: if same_chat { self.preview.scroll } else { 0 },
            version: self.preview.version + 1,
        };
        if let Some(key) = key {
            effects.push(Effect::Transcript { key, fetch: false });
        }
    }
}
