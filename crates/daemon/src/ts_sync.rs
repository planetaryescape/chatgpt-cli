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
use crate::state::State;

/// Far past a full TS sync (about a minute); a hung one is killed.
const TS_SYNC_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Run after a successful pass when the TS sync is due: the TS sync if the
/// TS CLI is installed, then the import. Results go into `report`.
pub async fn after_pass(state: &State, full: bool, report: &mut SyncReport) {
    state.syncer.ts_sync_ran();
    match ts_cli::locate() {
        Ok(ts) => {
            let choice = state.sessions_choice();
            let step = state
                .reporter
                .step("Syncing the TS CLI's index for bridged commands", None);
            let outcome = run(&ts, &choice, full).await;
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

/// `bun <cli.ts> [--browser …] [--profile …] sync [--full]`, niced.
async fn run(ts: &TsCli, choice: &SessionChoice, full: bool) -> TsSyncOutcome {
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
    let elapsed = |started: Instant| u64::try_from(started.elapsed().as_millis()).unwrap_or(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return TsSyncOutcome {
                ok: false,
                message: format!("couldn't start the TS sync: {error}"),
                elapsed_ms: elapsed(started),
            };
        }
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out = async {
        if let Some(stdout) = stdout {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::info!(target: "ts_sync", "{}", redact(&line));
            }
        }
    };
    // The TS CLI writes its status to stderr; the last line says how it went.
    let err = async {
        let mut last = String::new();
        if let Some(stderr) = stderr {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = redact(&line);
                tracing::info!(target: "ts_sync", "{line}");
                if !line.trim().is_empty() {
                    last = line;
                }
            }
        }
        last
    };
    // Both pipes drain together, so neither fills and stalls the child.
    let waited = tokio::time::timeout(TS_SYNC_TIMEOUT, async {
        let ((), last, status) = tokio::join!(out, err, child.wait());
        (last, status)
    })
    .await;
    match waited {
        Err(_) => TsSyncOutcome {
            ok: false,
            message: format!(
                "the TS sync took over {} minutes and was stopped",
                TS_SYNC_TIMEOUT.as_secs() / 60
            ),
            elapsed_ms: elapsed(started),
        },
        Ok((last, Ok(status))) if status.success() => TsSyncOutcome {
            ok: true,
            message: if last.is_empty() {
                "TS sync done".into()
            } else {
                format!("TS sync: {last}")
            },
            elapsed_ms: elapsed(started),
        },
        Ok((last, Ok(status))) => TsSyncOutcome {
            ok: false,
            message: format!(
                "the TS sync failed ({status}){}",
                if last.is_empty() {
                    String::new()
                } else {
                    format!(": {last}")
                }
            ),
            elapsed_ms: elapsed(started),
        },
        Ok((_, Err(error))) => TsSyncOutcome {
            ok: false,
            message: format!("the TS sync couldn't be watched: {error}"),
            elapsed_ms: elapsed(started),
        },
    }
}

#[allow(clippy::unwrap_used, reason = "a constant pattern")]
static RESPONSE_BODY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\b\d{3} from \S+?): .*$").unwrap());

/// The TS CLI's errors end with up to 300 characters of the response body
/// (`<status> from <path>: <body>`), which can hold chat content. Logs keep
/// the status and path only.
fn redact(line: &str) -> String {
    RESPONSE_BODY
        .replace(line, "$1: [response body redacted]")
        .into_owned()
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
    use super::redact;

    #[test]
    fn response_bodies_never_reach_the_log() {
        assert_eq!(
            redact(
                "failed: abc: 500 from /backend-api/conversations/batch: {\"detail\":\"secret chat\"}"
            ),
            "failed: abc: 500 from /backend-api/conversations/batch: [response body redacted]"
        );
        assert_eq!(
            redact("Sync done in 2.1s: 1 new, 0 updated"),
            "Sync done in 2.1s: 1 new, 0 updated"
        );
    }
}
