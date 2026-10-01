//! Terminal output. Stdout is data and stderr is status, as in the TS CLI.

use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use chatgpt_core::ErrorKind;
use chatgpt_launcher::ClientError;
use chatgpt_protocol::{Event, ProgressKind};
use serde::Serialize;

/// `error: <message>` on stderr, as the TS CLI reports a failure.
pub fn error_line(message: &str) {
    eprintln!("error: {message}");
}

/// A status line on stderr (`note()` in the TS CLI).
pub fn note(message: &str) {
    eprintln!("{message}");
}

/// Write `text` to stdout. A reader that went away (`| head`) ends the
/// command quietly, as it would a shell tool.
pub fn data(text: &str) {
    let mut stdout = std::io::stdout().lock();
    let written = stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush());
    if let Err(error) = written {
        if error.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(0);
        }
        error_line(&format!("cannot write the output: {error}"));
        std::process::exit(1);
    }
}

/// `value` as pretty JSON on stdout, two-space indented as JSON.stringify.
pub fn json(value: &impl Serialize) -> Result<(), ClientError> {
    let text = serde_json::to_string_pretty(value).map_err(|error| {
        ClientError::new(ErrorKind::Internal, format!("cannot write JSON: {error}"))
    })?;
    data(&format!("{text}\n"));
    Ok(())
}

/// The daemon answered a request with another request's kind of answer.
pub fn unexpected() -> ClientError {
    ClientError::new(
        ErrorKind::DaemonUnavailable,
        "the daemon answered with something else; run `chatgpt daemon stop` and try again",
    )
}

pub fn io_error(path: &std::path::Path, error: &std::io::Error) -> ClientError {
    ClientError::new(ErrorKind::Internal, format!("{}: {error}", path.display()))
}

/// Draws the daemon's progress lines as the TS CLI's `Step` does: one live
/// line on a terminal, and otherwise plain lines at most every 5 seconds.
pub struct ProgressLines {
    terminal: bool,
    live: bool,
    last_plain: Option<Instant>,
}

const PLAIN_EVERY: Duration = Duration::from_secs(5);

impl ProgressLines {
    pub fn new() -> Self {
        Self {
            terminal: std::io::stderr().is_terminal(),
            live: false,
            last_plain: None,
        }
    }

    pub fn show(&mut self, event: Event) {
        let Event::Progress(progress) = event else {
            return;
        };
        let mut stderr = std::io::stderr().lock();
        if self.terminal {
            let _ = match progress.kind {
                ProgressKind::Start | ProgressKind::Update => {
                    self.live = true;
                    write!(stderr, "\r\x1b[2K{}", progress.line)
                }
                _ => {
                    self.live = false;
                    writeln!(stderr, "\r\x1b[2K{}", progress.line)
                }
            };
            return;
        }
        let print = match progress.kind {
            ProgressKind::Update => self.last_plain.is_none_or(|at| at.elapsed() > PLAIN_EVERY),
            _ => true,
        };
        if print {
            if progress.kind == ProgressKind::Update {
                self.last_plain = Some(Instant::now());
            }
            let _ = writeln!(stderr, "{}", progress.line);
        }
    }

    /// End a live line left open, before other output.
    pub fn close(&mut self) {
        if self.live {
            eprintln!();
            self.live = false;
        }
    }
}
