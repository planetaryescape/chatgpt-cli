//! Progress lines for the client waiting on a sync, worded as the TS CLI's
//! `src/progress.ts` words them. The daemon runs one pass at a time, so one
//! slot holds the waiting client's channel; a background pass has none and
//! its progress only reaches the log.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use chatgpt_core::format_duration;
use chatgpt_protocol::{Progress, ProgressKind};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Clone, Default)]
pub struct Reporter {
    sink: Arc<Mutex<Option<UnboundedSender<Progress>>>>,
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
    /// Send progress to `sender` until the returned guard drops.
    pub fn attach(&self, sender: UnboundedSender<Progress>) -> Attached {
        *self.sink.lock().unwrap_or_else(PoisonError::into_inner) = Some(sender);
        Attached {
            reporter: self.clone(),
        }
    }

    fn send(&self, kind: ProgressKind, line: String) {
        if !matches!(kind, ProgressKind::Update) {
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
