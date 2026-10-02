//! Progress lines for the client waiting on a sync, worded as the TS CLI's
//! `src/progress.ts` words them. The daemon runs one pass at a time, so one
//! slot holds the waiting client's channel; a background pass has none and
//! its progress only reaches the log.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use chatgpt_core::format_duration;
use chatgpt_protocol::{Progress, ProgressKind};
use tokio::sync::mpsc::UnboundedSender;

/// How often the server tells a client waiting on any request that it's
/// still working, well inside the client's 300-second stall timeout.
pub const HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Clone, Default)]
pub struct Reporter {
    sink: Arc<Mutex<Option<UnboundedSender<Progress>>>>,
    /// Lines only for the client: they can name chats (a preview, a
    /// failure, Jev's held-back list), which the log never holds.
    quiet: bool,
}

/// Detaches the client's channel when dropped.
pub struct Attached {
    reporter: Reporter,
}

impl Drop for Attached {
    fn drop(&mut self) {
        *self
            .reporter
            .sink
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }
}

impl Reporter {
    /// A reporter of one request's own lines, sent to its client and never
    /// logged.
    pub fn for_client(sender: Option<UnboundedSender<Progress>>) -> Self {
        Self {
            sink: Arc::new(Mutex::new(sender)),
            quiet: true,
        }
    }

    /// Send progress to `sender` until the returned guard drops.
    pub fn attach(&self, sender: UnboundedSender<Progress>) -> Attached {
        *self.sink.lock().unwrap_or_else(PoisonError::into_inner) = Some(sender);
        Attached {
            reporter: self.clone(),
        }
    }

    fn send(&self, kind: ProgressKind, line: String) {
        if !self.quiet && !matches!(kind, ProgressKind::Update) {
            tracing::info!(target: "progress", "{line}");
        }
        if let Some(sender) = self
            .sink
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            let _ = sender.send(Progress { kind, line });
        }
    }

    /// `note()`: a line the client always prints.
    pub fn note(&self, line: String) {
        self.send(ProgressKind::Note, line);
    }

    /// Whether the client this reports to went away (its connection
    /// closed), so a long run can stop starting paid calls. A reporter
    /// without a client never went away.
    pub fn client_gone(&self) -> bool {
        self.sink
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(UnboundedSender::is_closed)
    }

    pub fn step(&self, label: &str, total: Option<usize>) -> Step {
        self.send(ProgressKind::Start, format!("{label}…"));
        Step {
            reporter: self.clone(),
            label: label.to_owned(),
            total,
            started: Instant::now(),
        }
    }
}

/// `Step` in progress.ts.
pub struct Step {
    reporter: Reporter,
    label: String,
    total: Option<usize>,
    started: Instant,
}

impl Step {
    pub fn update(&self, done: usize) {
        self.update_with(done, "");
    }

    /// `step.update(done, detail)`: the count, then `· detail` when there
    /// is one.
    pub fn update_with(&self, done: usize, detail: &str) {
        let elapsed = self.started.elapsed().as_millis() as f64;
        let mut line = self.label.clone();
        match self.total {
            Some(total) => {
                let rate = if elapsed > 0.0 {
                    done as f64 / (elapsed / 1000.0)
                } else {
                    0.0
                };
                let eta = if rate > 0.0 && done < total {
                    format!(
                        " · {} left",
                        format_duration((total - done) as f64 / rate * 1000.0)
                    )
                } else {
                    String::new()
                };
                line.push_str(&format!(
                    "  {} {done}/{total} · {}/s{eta}",
                    bar(done, total),
                    crate::js::to_fixed(rate, 1)
                ));
            }
            None => line.push_str(&format!("  {done} so far")),
        }
        line.push_str(&format!(" · {}", format_duration(elapsed)));
        if !detail.is_empty() {
            line.push_str(&format!(" · {detail}"));
        }
        self.reporter.send(ProgressKind::Update, line);
    }

    pub fn finish(self, summary: &str) {
        let elapsed = self.started.elapsed().as_millis() as f64;
        self.reporter.send(
            ProgressKind::Finish,
            format!("{summary} ({})", format_duration(elapsed)),
        );
    }
}

fn bar(done: usize, total: usize) -> String {
    const WIDTH: usize = 20;
    let filled = if total == 0 {
        0
    } else {
        ((done as f64 / total as f64) * WIDTH as f64).round() as usize
    }
    .min(WIDTH);
    format!("{}{}", "█".repeat(filled), "░".repeat(WIDTH - filled))
}

/// Asks the waiting client a yes-or-no question mid-request (the TS CLI's
/// `ask`): the prompt goes out as an `Ask` progress line, and the client's
/// `Answer` comes back through `answers`.
pub struct Asker {
    reporter: Reporter,
    answers: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<bool>>,
}

impl Asker {
    pub fn new(reporter: Reporter, answers: tokio::sync::mpsc::UnboundedReceiver<bool>) -> Self {
        Self {
            reporter,
            answers: tokio::sync::Mutex::new(answers),
        }
    }

    /// The client's answer; `None` when it went away without one.
    pub async fn ask(&self, prompt: &str) -> Option<bool> {
        let mut answers = self.answers.lock().await;
        self.reporter.send(ProgressKind::Ask, prompt.to_owned());
        answers.recv().await
    }
}
