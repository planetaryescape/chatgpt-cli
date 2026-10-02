//! `sync`.

use std::process::ExitCode;

use chatgpt_core::Paths;
use chatgpt_launcher::ClientError;
use chatgpt_protocol::{Request, ResponseData, SessionChoice, SyncMode, SyncReport};

use crate::output::{ProgressLines, note, unexpected};

pub async fn sync(
    paths: &Paths,
    full: bool,
    session: SessionChoice,
) -> Result<ExitCode, ClientError> {
    let mut progress = ProgressLines::new();
    let answer = chatgpt_launcher::ask(paths, Request::Sync { full, session }, |event| {
        progress.show(event);
    })
    .await;
    progress.close();
    let ResponseData::Sync(report) = answer? else {
        return Err(unexpected());
    };
    let failures = report
        .reconcile
        .as_ref()
        .map_or(&[][..], |reconcile| reconcile.failures.as_slice());
    for failure in failures {
        eprintln!("failed: {failure}");
    }
    note(&summary(&report));
    if failures.is_empty() {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}

/// The TS CLI's last line for each kind of sync.
fn summary(report: &SyncReport) -> String {
    let elapsed = chatgpt_core::format_duration(report.elapsed_ms as f64);
    match report.mode {
        SyncMode::Full => {
            let change = i128::from(report.total) - i128::from(report.before);
            format!(
                "Full sync: {} chats ({} active, {} archived), {}{change} vs before, in {elapsed}.",
                report.total,
                report.active,
                report.archived,
                if change >= 0 { "+" } else { "" }
            )
        }
        SyncMode::Delta => format!(
            "Sync done in {elapsed}: {} new, {} updated, {} newly archived, {} unarchived, {} deleted. `sync --full` also drops chats deleted in the browser.",
            report.added, report.updated, report.newly_archived, report.unarchived, report.deleted
        ),
        // A newer daemon's kind of pass: its counts may mean something else.
        SyncMode::Unknown => format!("Sync done in {elapsed}."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_of_sync_this_version_doesnt_know_gets_a_plain_summary() {
        let report = SyncReport {
            mode: SyncMode::Unknown,
            elapsed_ms: 1500,
            added: 3,
            ..SyncReport::default()
        };
        assert_eq!(summary(&report), "Sync done in 1.5s.");
    }
}
