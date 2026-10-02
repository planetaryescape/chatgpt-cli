//! Running the TUI: the terminal, the keyboard, and the daemon.
//!
//! Three threads: this one draws and owns the [`App`]; one reads the
//! keyboard; one runs the daemon requests (each on its own connection, so
//! a slow fetch never holds up a cache lookup) and the clipboard, and
//! watches for SIGTERM, SIGHUP and SIGINT. They meet in one channel, which
//! this thread blocks on: nothing is drawn, and no CPU is used, until a key,
//! an answer, a resize or the transcript debounce arrives.
//!
//! The terminal is restored on every way out: quitting, an error, a
//! signal, and a panic on any thread (whose hook restores it first; the
//! loop then stops).

use std::io::{IsTerminal, Stdout};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Instant;

use chatgpt_core::{ErrorKind, Paths};
use chatgpt_launcher::ClientError;
use chatgpt_protocol::{
    ChatAction, Event as DaemonEvent, ProgressKind, Request, ResponseData, Row, SessionChoice,
    Target, TranscriptSource,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event as TermEvent, KeyCode, KeyEventKind};
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::crossterm::{cursor, execute};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::app::{App, DEBOUNCE, Effect, Outcome, Transcript};

/// Set by the panic hook, so the loop stops instead of drawing on a
/// terminal that's been restored.
static PANICKED: AtomicBool = AtomicBool::new(false);

enum Event {
    Input(TermEvent),
    Outcome(Outcome),
    /// SIGTERM, SIGHUP or SIGINT.
    Signal,
    /// A request's task panicked.
    Crashed,
}

pub fn run(paths: &Paths, session: SessionChoice) -> Result<ExitCode, ClientError> {
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        return Err(ClientError::new(
            ErrorKind::InvalidInput,
            "chatgpt tui needs a terminal",
        ));
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            ClientError::new(
                ErrorKind::Internal,
                format!("cannot start the async runtime: {error}"),
            )
        })?;
    // Before the screen is taken: starting the daemon, and a missing index,
    // print as any command's would.
    let (topics, rows) = runtime.block_on(first_load(paths))?;

    let (events, inbox) = std::sync::mpsc::channel();
    let jobs = start_worker(runtime, paths.clone(), session, events.clone());
    std::thread::spawn({
        let events = events.clone();
        move || {
            while let Ok(input) = event::read() {
                if events.send(Event::Input(input)).is_err() {
                    break;
                }
            }
        }
    });
    install_panic_hook();
    let mut screen = Screen::enter().map_err(|error| terminal_error(&error))?;
    let mut app = App::new(rows, topics, now_ms);
    let result = event_loop(&mut screen.terminal, &mut app, &inbox, &jobs);
    drop(screen);
    result
}

/// The topics to cycle through (from the daemon's status) and every chat.
async fn first_load(paths: &Paths) -> Result<(Vec<String>, Vec<Row>), ClientError> {
    let (mut client, status) = chatgpt_launcher::connect(paths).await?;
    let ResponseData::Rows(answer) = client
        .request_with_events(Request::list_every_chat(), |_| {})
        .await?
    else {
        return Err(chatgpt_launcher::unexpected());
    };
    Ok((status.classification.topics, answer.rows))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    inbox: &Receiver<Event>,
    jobs: &UnboundedSender<Effect>,
) -> Result<ExitCode, ClientError> {
    // The chat whose transcript to fetch once the debounce passes.
    let mut pending: Option<(String, Instant)> = None;
    let panic_key = panic_key_for_tests();
    let mut effects = app.first_effects();
    loop {
        for effect in effects.drain(..) {
            match effect {
                Effect::Quit => return Ok(ExitCode::SUCCESS),
                // Scrolled past already (keys handled together): only the
                // chat now selected is looked up.
                Effect::Transcript { id, .. } if app.preview.id.as_ref() != Some(&id) => {}
                Effect::FetchLater { id } => pending = Some((id, Instant::now() + DEBOUNCE)),
                // `open` returns at once; a failure to start it shows
                // nowhere, as in the TS TUI.
                Effect::Open { id } => {
                    let _ = chatgpt_core::desktop::open_chat(&id);
                }
                effect => {
                    if jobs.send(effect).is_err() {
                        return Err(crashed());
                    }
                }
            }
        }
        if PANICKED.load(Ordering::SeqCst) {
            return Err(crashed());
        }
        terminal
            .draw(|frame| crate::ui::draw(frame, app))
            .map_err(|error| terminal_error(&error))?;

        let first = match &pending {
            Some((_, due)) => {
                match inbox.recv_timeout(due.saturating_duration_since(Instant::now())) {
                    Ok(event) => Some(event),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return Err(crashed()),
                }
            }
            None => Some(inbox.recv().map_err(|_| crashed())?),
        };
        let Some(first) = first else {
            // The debounce passed with the chat still selected and not
            // yet loaded: fetch it.
            if let Some((id, _)) = pending.take()
                && app.preview.id.as_ref() == Some(&id)
                && app.preview.transcript == Transcript::Loading
            {
                effects.push(Effect::Transcript { id, fetch: true });
            }
            continue;
        };
        // Everything already queued (key repeat outruns drawing) before
        // the next draw.
        for event in std::iter::once(first).chain(inbox.try_iter()) {
            match event {
                Event::Input(TermEvent::Key(key))
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    #[allow(clippy::panic, reason = "a test asks for it")]
                    if panic_key.is_some_and(|wanted| key.code == KeyCode::Char(wanted)) {
                        panic!("CHATGPT_TEST_TUI_PANIC");
                    }
                    effects.extend(app.on_key(key));
                }
                // A resize redraws at the new size.
                Event::Input(_) => {}
                Event::Outcome(outcome) => effects.extend(app.on_outcome(outcome)),
                Event::Signal => return Ok(ExitCode::from(130)),
                Event::Crashed => return Err(crashed()),
            }
        }
    }
}

/// Debug builds only: `CHATGPT_TEST_TUI_PANIC=<key>` panics on that key,
/// so a test can see the terminal restored after a panic.
fn panic_key_for_tests() -> Option<char> {
    chatgpt_core::debug_env("CHATGPT_TEST_TUI_PANIC").and_then(|key| key.chars().next())
}

/// The thread that runs daemon requests and the clipboard, each in its own
/// task, and watches for signals.
fn start_worker(
    runtime: tokio::runtime::Runtime,
    paths: Paths,
    session: SessionChoice,
    events: Sender<Event>,
) -> UnboundedSender<Effect> {
    let (jobs, mut queue): (UnboundedSender<Effect>, UnboundedReceiver<Effect>) =
        unbounded_channel();
    std::thread::spawn(move || {
        // The launcher's requests aren't `Send`: they run as local tasks.
        let local = tokio::task::LocalSet::new();
        local.block_on(&runtime, async move {
            tokio::task::spawn_local(watch_signals(events.clone()));
            while let Some(effect) = queue.recv().await {
                let task = tokio::task::spawn_local(perform(
                    effect,
                    paths.clone(),
                    session.clone(),
                    events.clone(),
                ));
                let events = events.clone();
                tokio::task::spawn_local(async move {
                    if task.await.is_err() {
                        let _ = events.send(Event::Crashed);
                    }
                });
            }
        });
    });
    jobs
}

async fn watch_signals(events: Sender<Event>) {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut terminate), Ok(mut hangup), Ok(mut interrupt)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::hangup()),
        signal(SignalKind::interrupt()),
    ) else {
        return;
    };
    tokio::select! {
        _ = terminate.recv() => {}
        _ = hangup.recv() => {}
        _ = interrupt.recv() => {}
    }
    let _ = events.send(Event::Signal);
}

async fn perform(effect: Effect, paths: Paths, session: SessionChoice, events: Sender<Event>) {
    let outcome = match effect {
        Effect::Transcript { id, fetch } => {
            let source = if fetch {
                TranscriptSource::CacheOrFetch
            } else {
                TranscriptSource::Cache
            };
            let request = Request::Transcript {
                id: id.clone(),
                source,
                session,
            };
            let answer = chatgpt_launcher::ask(&paths, request, |_| {}).await;
            Outcome::Transcript {
                id,
                fetched: fetch,
                result: expect(answer, |data| match data {
                    ResponseData::Transcript(chat) => Some(*chat),
                    _ => None,
                }),
            }
        }
        Effect::Reload => {
            let answer = chatgpt_launcher::ask(&paths, Request::list_every_chat(), |_| {}).await;
            Outcome::Reloaded(expect(answer, |data| match data {
                ResponseData::Rows(answer) => Some(answer.rows),
                _ => None,
            }))
        }
        Effect::Apply { archive, delete } => {
            Outcome::Applied(apply(&paths, archive, delete, session, &events).await)
        }
        Effect::SaveTitle { id, title } => {
            let request = Request::SetTitle {
                reference: id,
                title,
                archived: false,
                all: true,
            };
            let answer = chatgpt_launcher::ask(&paths, request, |_| {}).await;
            Outcome::TitleSaved(expect(answer, |data| {
                matches!(data, ResponseData::TitleSaved { .. }).then_some(())
            }))
        }
        Effect::Copy { markdown, title } => {
            let copied = tokio::task::spawn_blocking(move || {
                chatgpt_core::desktop::copy_to_clipboard(&markdown)
                    .map(|()| App::copied(&title, &markdown))
                    .map_err(|(_, why)| format!("Copy failed: {why}"))
            })
            .await
            .unwrap_or_else(|error| Err(format!("Copy failed: {error}")));
            Outcome::Copied(copied)
        }
        // Handled on the main thread.
        Effect::FetchLater { .. } | Effect::Open { .. } | Effect::Quit => return,
    };
    let _ = events.send(Event::Outcome(outcome));
}

/// The answer `pick` wants, or what to tell the user.
fn expect<T>(
    answer: Result<ResponseData, ClientError>,
    pick: impl FnOnce(ResponseData) -> Option<T>,
) -> Result<T, String> {
    answer
        .and_then(|data| pick(data).ok_or_else(chatgpt_launcher::unexpected))
        .map_err(|error| error.message)
}

/// The marks through `Mutate`, as `archive -y` and `delete -y` send them:
/// the archives, then the deletes, counting each chat attempted.
async fn apply(
    paths: &Paths,
    archive: Vec<Target>,
    delete: Vec<Target>,
    session: SessionChoice,
    events: &Sender<Event>,
) -> Result<Vec<String>, String> {
    let mut attempted = 0;
    let mut failures = Vec::new();
    for (action, targets) in [(ChatAction::Archive, archive), (ChatAction::Delete, delete)] {
        if targets.is_empty() {
            continue;
        }
        let count = targets.len();
        let request = Request::Mutate {
            action,
            targets,
            session: session.clone(),
        };
        let mut done_here = 0;
        let answer = chatgpt_launcher::ask(paths, request, |event| {
            if let DaemonEvent::Progress(progress) = event
                && progress.kind == ProgressKind::Update
            {
                done_here += 1;
                let _ = events.send(Event::Outcome(Outcome::ApplyProgress(
                    attempted + done_here,
                )));
            }
        })
        .await;
        let outcome = expect(answer, |data| match data {
            ResponseData::Outcome(outcome) => Some(outcome),
            _ => None,
        })?;
        failures.extend(outcome.failures);
        attempted += count;
    }
    Ok(failures)
}

fn crashed() -> ClientError {
    ClientError::new(
        ErrorKind::Internal,
        "the TUI stopped after an internal error (see above)",
    )
}

fn terminal_error(error: &std::io::Error) -> ClientError {
    ClientError::new(ErrorKind::Internal, format!("the terminal: {error}"))
}

/// The alternate screen in raw mode, given back when dropped.
struct Screen {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl Screen {
    fn enter() -> std::io::Result<Self> {
        enable_raw_mode()?;
        let entered = execute!(std::io::stdout(), EnterAlternateScreen, cursor::Hide)
            .and_then(|()| Terminal::new(CrosstermBackend::new(std::io::stdout())));
        match entered {
            Ok(terminal) => Ok(Self { terminal }),
            Err(error) => {
                restore();
                Err(error)
            }
        }
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        restore();
    }
}

fn restore() {
    let _ = disable_raw_mode();
    let _ = execute!(std::io::stdout(), LeaveAlternateScreen, cursor::Show);
}

/// Restore the terminal before a panic's message prints, on any thread,
/// and stop the loop.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        PANICKED.store(true, Ordering::SeqCst);
        previous(info);
    }));
}
