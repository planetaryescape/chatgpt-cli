//! While the bridge exists (D2), bridged commands such as `classify` read
//! the TS CLI's own index. After a pass the daemon runs the TS CLI's `sync`
//! so that index is as fresh, then imports what the TS CLI wrote: Jev and
//! Luna judgments, local titles, summaries and transcripts.
//!
//! The TS sync runs niced, with its output in the daemon's log. Its failures
//! show in `daemon status` and the sync report, and never fail the pass.

use std::path::Path;
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use chatgpt_core::legacy::legacy_index_path;
use chatgpt_core::ts_cli::{self, BRIDGED_ENV, TsCli};
use chatgpt_protocol::{ImportReport, SessionChoice, SyncReport, TableImport, TsSyncOutcome};
use regex::Regex;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::handlers::Failure;
use crate::progress::Step;
use crate::state::State;

/// Far past a full TS sync (about a minute); a hung one is killed.
const TS_SYNC_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Run after a successful pass when the TS sync is due: the TS sync if the
/// TS CLI is installed, then the import. Results go into `report`.
pub async fn after_pass(
    state: &State,
    choice: &SessionChoice,
    full: bool,
    report: &mut SyncReport,
) {
    state.syncer.ts_sync_ran();
    match ts_cli::locate() {
        Ok(ts) => {
            let step = state
                .reporter
                .step("Syncing the TS CLI's index for bridged commands", None);
            let outcome = run(&ts, choice, full, &step).await;
            step.finish(&outcome.message);
            state.record_ts_sync(Some(&ts), &outcome);
            report.ts_sync = Some(outcome);
        }
        Err(error) => {
            tracing::info!("no TS sync: {error}");
            state.record_ts_unavailable(error.to_string());
        }
    }
    match import(state).await {
        Ok(imported) => report.import = Some(imported),
        Err(failure) => tracing::warn!("importing the TS index failed: {}", failure.message),
    }
}

/// `bun <cli.ts> [--browser …] [--profile …] sync [--full]`, niced. Every
/// line it prints moves `step` on (a count, never the text); the server's
/// heartbeat covers quiet stretches.
async fn run(ts: &TsCli, choice: &SessionChoice, full: bool, step: &Step) -> TsSyncOutcome {
    let started = Instant::now();
    let nice = Path::new("/usr/bin/nice");
    let mut command = if nice.is_file() {
        let mut command = Command::new(nice);
        command.args(["-n", "10"]).arg(&ts.bun);
        command
    } else {
        Command::new(&ts.bun)
    };
    command.arg(&ts.entry);
    if let Some(browser) = &choice.browser {
        command.args(["--browser", browser]);
    }
    if let Some(profile) = &choice.profile {
        command.args(["--profile", profile]);
    }
    command.arg("sync");
    if full {
        command.arg("--full");
    }
    command
        .env(BRIDGED_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let outcome = |ok: bool, message: String| TsSyncOutcome {
        ok,
        message,
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(0),
    };
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return outcome(false, format!("couldn't start the TS sync: {error}")),
    };
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return outcome(false, "the TS sync's output couldn't be read".into());
    };
    let mut stdout = BufReader::new(stdout).lines();
    let mut stderr = BufReader::new(stderr).lines();
    let (mut stdout_open, mut stderr_open) = (true, true);
    let mut output = Output::default();
    let deadline = tokio::time::sleep(TS_SYNC_TIMEOUT);
    tokio::pin!(deadline);
    let status = loop {
        tokio::select! {
            line = stdout.next_line(), if stdout_open => match line {
                Ok(Some(line)) => {
                    output.line(&line);
                    step.update(output.lines);
                }
                _ => stdout_open = false,
            },
            line = stderr.next_line(), if stderr_open => match line {
                Ok(Some(line)) => {
                    output.line(&line);
                    step.update(output.lines);
                }
                _ => stderr_open = false,
            },
            status = child.wait(), if !stdout_open && !stderr_open => break status,
            () = &mut deadline => {
                let _ = child.kill().await;
                output.flush();
                return outcome(false, format!(
                    "the TS sync took over {} minutes and was stopped",
                    TS_SYNC_TIMEOUT.as_secs() / 60
                ));
            }
        }
    };
    output.flush();
    let summary = output.summary();
    match status {
        Ok(status) if status.success() => outcome(true, format!("TS sync: {summary}")),
        Ok(status) => outcome(false, format!("the TS sync failed ({status}): {summary}")),
        Err(error) => outcome(false, format!("the TS sync couldn't be watched: {error}")),
    }
}

/// The TS sync's output, kept only where it's one of the TS CLI's own
/// progress or summary lines. Anything else (an error with a response
/// body, which can hold chat content and span many lines) is counted,
/// never kept: not in the log, `daemon status` or the client's progress.
#[derive(Default)]
struct Output {
    /// Lines seen, kept or not.
    lines: usize,
    /// Withheld lines not yet logged as a count.
    withheld: usize,
    withheld_total: usize,
    last_safe: Option<String>,
}

impl Output {
    fn line(&mut self, line: &str) {
        self.lines += 1;
        if line.trim().is_empty() {
            return;
        }
        if is_safe(line) {
            self.flush();
            tracing::info!(target: "ts_sync", "{line}");
            self.last_safe = Some(line.to_owned());
        } else {
            self.withheld += 1;
            self.withheld_total += 1;
        }
    }

    fn flush(&mut self) {
        if self.withheld > 0 {
            tracing::info!(target: "ts_sync", "{}", withheld(self.withheld));
            self.withheld = 0;
        }
    }

    /// How it ended, for `daemon status` and the client.
    fn summary(&self) -> String {
        let withheld = (self.withheld_total > 0).then(|| withheld(self.withheld_total));
        match (&self.last_safe, withheld) {
            (Some(last), Some(withheld)) => format!("{last} {withheld}"),
            (Some(last), None) => last.clone(),
            (None, Some(withheld)) => withheld,
            (None, None) => "no output".into(),
        }
    }
}

fn withheld(count: usize) -> String {
    let lines = if count == 1 { "line" } else { "lines" };
    format!("[{count} {lines} of TS output withheld]")
}

/// The TS CLI's sync progress and summary lines (`src/cli.ts` and
/// `src/progress.ts` @ 1b8c950): counts, durations and fixed words only.
#[allow(clippy::unwrap_used, reason = "constant patterns")]
static SAFE_LINES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    let duration = r"(?:\d+\.\d+s|\d+s|\d+m\d{2}s|\d+h\d{2}m)";
    let label = r"(?:Checking for new and updated chats|Reading archived chats|Listing (?:active|archived) chats|Checking changed chat content)";
    [
        format!(r"^{label}…$"),
        format!(r"^{label}  \d+ so far · {duration}$"),
        format!(r"^{label}  [█░]{{20}} \d+/\d+ · \d+\.\d/s(?: · {duration} left)? · {duration}$"),
        format!(r"^\d+ new, \d+ updated \({duration}\)$"),
        format!(r"^\d+ archived \({duration}\)$"),
        format!(r"^Listed \d+ (?:active|archived) chat\(s\) \({duration}\)$"),
        format!(r"^Preserved \d+ unchanged cache\(s\); \d+ content or metadata change\(s\) left stale(?:, \d+ failed)? \({duration}\)$"),
        r"^Recovered \d+ chat\(s\) omitted from the conversation lists after individual checks\.$".to_owned(),
        format!(r"^Sync done in {duration}: \d+ new, \d+ updated, \d+ newly archived, \d+ unarchived, \d+ deleted\. `sync --full` also drops chats deleted in the browser\.$"),
        format!(r"^Full sync: \d+ chats \(\d+ active, \d+ archived\), [+-]?\d+ vs before, in {duration}\.$"),
        r"^rate limited by ChatGPT; waiting \d+(?:\.\d+)?s$".to_owned(),
    ]
    .iter()
    .map(|pattern| Regex::new(pattern).unwrap())
    .collect()
});

fn is_safe(line: &str) -> bool {
    SAFE_LINES.iter().any(|pattern| pattern.is_match(line))
}

/// Import the TS index now. A missing index is not an error: there's
/// nothing to import, and nothing is deleted.
pub async fn import(state: &State) -> Result<ImportReport, Failure> {
    // The TS CLI may have been updated, with new classification versions.
    state.reload_profile();
    let Some(path) = legacy_index_path().filter(|path| path.is_file()) else {
        let message = "no TS index to import".to_owned();
        state.record_import(Err(message.clone()));
        return Err(Failure::new(chatgpt_core::ErrorKind::NotSynced, message));
    };
    let display = path.display().to_string();
    let imported = state
        .db_write(move |db| chatgpt_store::import_legacy(db, &path))
        .await;
    match imported {
        Ok(counts) => {
            let report = ImportReport {
                path: display,
                tables: counts
                    .tables
                    .into_iter()
                    .map(|table| TableImport {
                        table: table.table,
                        rows: table.rows,
                        inserted: table.inserted,
                        updated: table.updated,
                        deleted: table.deleted,
                        skipped: table.skipped,
                    })
                    .collect(),
            };
            tracing::info!(
                path = %report.path,
                changed = report.tables.iter().map(|t| t.inserted + t.updated + t.deleted).sum::<u64>(),
                "imported the TS index"
            );
            state.record_import(Ok(report.clone()));
            Ok(report)
        }
        Err(failure) => {
            state.record_import(Err(failure.message.clone()));
            Err(failure)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SENTINEL: &str = "SENTINEL-chat-content";

    #[test]
    fn only_the_ts_clis_own_lines_are_kept() {
        let mut output = Output::default();
        let multiline_body = format!("  \"{SENTINEL}\"");
        for line in [
            "Checking for new and updated chats…",
            "Checking for new and updated chats  12 so far · 1.2s",
            "error: 500 from /backend-api/conversations/batch: {\"detail\":",
            &multiline_body,
            "}",
            "failed: abc: 429 from /backend-api/x: retry",
            "Checking changed chat content  █████████░░░░░░░░░░░ 10/22 · 1.1/s · 11s left · 9.1s",
            "Sync done in 8.4s: 7 new, 1 updated, 0 newly archived, 0 unarchived, 0 deleted. `sync --full` also drops chats deleted in the browser.",
            SENTINEL,
        ] {
            output.line(line);
        }
        let summary = output.summary();
        assert!(!summary.contains(SENTINEL), "{summary}");
        assert!(summary.starts_with("Sync done in 8.4s: 7 new"), "{summary}");
        assert!(
            summary.ends_with("[5 lines of TS output withheld]"),
            "{summary}"
        );
        assert!(is_safe(
            "Full sync: 811 chats (662 active, 149 archived), +0 vs before, in 35s."
        ));
        assert!(is_safe("rate limited by ChatGPT; waiting 5s"));
        assert!(!is_safe(&format!("Sync done in 8.4s: {SENTINEL}")));
    }
}
