//! Terminal output. Stdout is data and stderr is status, as in the TS CLI.

use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use chatgpt_core::ErrorKind;
use chatgpt_launcher::ClientError;
use chatgpt_protocol::{Event, ProgressKind};
use serde::Serialize;
use serde_json::ser::{Formatter, PrettyFormatter};

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

/// `value` as `JSON.stringify(value, null, 2)` writes it, on stdout.
pub fn json(value: &impl Serialize) -> Result<(), ClientError> {
    let mut text = to_js_json(value).map_err(|error| {
        ClientError::new(ErrorKind::Internal, format!("cannot write JSON: {error}"))
    })?;
    text.push(b'\n');
    data(&String::from_utf8_lossy(&text));
    Ok(())
}

fn to_js_json(value: &impl Serialize) -> serde_json::Result<Vec<u8>> {
    let mut text = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut text, JsFormatter(PrettyFormatter::new()));
    value.serialize(&mut serializer)?;
    Ok(text)
}

/// serde_json's two-space pretty layout, which matches JSON.stringify's,
/// with floats printed by JS's rules (`0.000001`, `1e-7`, `1e+21`) where
/// serde_json would write `1e-6` and `1e21`.
struct JsFormatter<'a>(PrettyFormatter<'a>);

macro_rules! delegate {
    ($($name:ident),*) => {$(
        fn $name<W: ?Sized + std::io::Write>(&mut self, writer: &mut W) -> std::io::Result<()> {
            self.0.$name(writer)
        }
    )*};
    (first: $($name:ident),*) => {$(
        fn $name<W: ?Sized + std::io::Write>(
            &mut self,
            writer: &mut W,
            first: bool,
        ) -> std::io::Result<()> {
            self.0.$name(writer, first)
        }
    )*};
}

impl Formatter for JsFormatter<'_> {
    delegate!(
        begin_array,
        end_array,
        end_array_value,
        begin_object,
        end_object,
        begin_object_value,
        end_object_value
    );

    delegate!(first: begin_array_value, begin_object_key);

    fn write_f64<W: ?Sized + std::io::Write>(
        &mut self,
        writer: &mut W,
        value: f64,
    ) -> std::io::Result<()> {
        writer.write_all(chatgpt_core::js_number_string(value).as_bytes())
    }

    fn write_f32<W: ?Sized + std::io::Write>(
        &mut self,
        writer: &mut W,
        value: f32,
    ) -> std::io::Result<()> {
        self.write_f64(writer, f64::from(value))
    }
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
            // Each step keeps its own clock, as each TS `Step` does: its
            // first count prints at once.
            ProgressKind::Start => {
                self.last_plain = None;
                true
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_numbers_and_layout_match_json_stringify() {
        let value =
            serde_json::json!([{"a": 1e-6, "b": 1e-7, "c": [], "d": {}, "e": [1, 2.5], "f": 1e21}]);
        let text = to_js_json(&value).expect("json");
        assert_eq!(
            String::from_utf8(text).expect("utf-8"),
            "[\n  {\n    \"a\": 0.000001,\n    \"b\": 1e-7,\n    \"c\": [],\n    \"d\": {},\n    \"e\": [\n      1,\n      2.5\n    ],\n    \"f\": 1e+21\n  }\n]"
        );
    }
}
