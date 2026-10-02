//! `daemon status|stop|logs`.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use chatgpt_core::{DAEMON_LOG_PREFIX, Paths};
use chatgpt_launcher::ClientError;
use chatgpt_protocol::DaemonStatus;

use crate::output::{self, data, io_error, note};

/// The daemon's status, starting it if it isn't running.
pub async fn status(paths: &Paths, json: bool) -> Result<ExitCode, ClientError> {
    let (_, status) = chatgpt_launcher::connect(paths).await?;
    if json {
        output::json(&status)?;
    } else {
        data(&status_text(&status, chrono::Utc::now().timestamp()));
    }
    Ok(ExitCode::SUCCESS)
}

fn ago(at: i64, now: i64) -> String {
    let seconds = (now - at).max(0);
    match seconds {
        0..60 => format!("{seconds}s ago"),
        60..3600 => format!("{}m ago", seconds / 60),
        _ => format!("{}h{:02}m ago", seconds / 3600, seconds % 3600 / 60),
    }
}

fn status_text(status: &DaemonStatus, now: i64) -> String {
    let mut lines = vec![
        format!(
            "daemon: running, pid {}, version {}, instance {}, up {}",
            status.pid,
            status.version,
            status.instance,
            ago(status.started_at, now).trim_end_matches(" ago")
        ),
        format!("socket: {}", status.socket),
        format!("index: {}", status.database),
    ];
    let sync = &status.sync;
    lines.push(match &sync.synced_at {
        Some(at) => format!("synced: {at}"),
        None => "synced: never (No local index yet)".to_owned(),
    });
    if sync.in_progress {
        lines.push("sync: running now".to_owned());
    }
    if let (Some(at), Some(summary)) = (sync.last_finished_at, &sync.last_summary) {
        lines.push(format!("last pass: {} ({summary})", ago(at, now)));
    }
    if let Some(error) = &sync.last_error {
        lines.push(format!("last error: {error}"));
    }
    if let Some(next) = sync.next_at {
        lines.push(format!("next pass: in {}s", (next - now).max(0)));
    }
    if let Some(backoff) = &status.backoff {
        lines.push(format!(
            "backing off: {}s more ({})",
            (backoff.until - now).max(0),
            backoff.reason
        ));
    }
    lines.push(format!(
        "session: {}",
        status.session.as_deref().unwrap_or("not read yet")
    ));
    let ts = &status.ts_sync;
    lines.push(match (&ts.cli, &ts.unavailable) {
        (Some(cli), _) => format!("TS CLI: {cli}"),
        (None, Some(why)) => format!("TS CLI: unavailable ({why})"),
        (None, None) => "TS CLI: unknown".to_owned(),
    });
    if let (Some(at), Some(message)) = (ts.last_run_at, &ts.last_message) {
        let outcome = match ts.last_ok {
            Some(true) => "ok",
            Some(false) => "failed",
            None => "not run",
        };
        lines.push(format!("TS sync: {outcome} {} ({message})", ago(at, now)));
    }
    let import = &status.legacy_import;
    if let Some(at) = import.last_at {
        let detail = match (&import.last_error, &import.last) {
            (Some(error), _) => format!("failed: {error}"),
            (None, Some(report)) => {
                let changed: u64 = report
                    .tables
                    .iter()
                    .map(|table| table.inserted + table.updated + table.deleted)
                    .sum();
                format!("{changed} row(s) changed from {}", report.path)
            }
            (None, None) => String::new(),
        };
        lines.push(format!("TS import: {} {detail}", ago(at, now)));
    }
    let classification = &status.classification;
    lines.push(format!(
        "classification: questions {}, Luna {}, from {}",
        classification.questions_version, classification.luna_version, classification.source
    ));
    if let Some(problem) = &classification.problem {
        lines.push(format!("classification problem: {problem}"));
    }
    lines.push(search_index_line(&status.search_index));
    if let Some(error) = &status.search_index.last_error {
        lines.push(format!("search index error: {error}"));
    }
    lines.join("\n") + "\n"
}

/// How far the background search indexer has got.
fn search_index_line(index: &chatgpt_protocol::SearchIndexStatus) -> String {
    let mut parts = vec![format!(
        "{} of {} chats indexed",
        index.indexed, index.chats
    )];
    if index.in_progress {
        parts.push("indexing now".to_owned());
    }
    parts.push(format!("{} transcript(s) fetched", index.fetched));
    if index.failed > 0 {
        parts.push(format!("{} not returned by ChatGPT", index.failed));
    }
    if let Some(waiting) = &index.waiting {
        parts.push(waiting.clone());
    }
    format!("search index: {}", parts.join(", "))
}

pub async fn stop(paths: &Paths) -> Result<ExitCode, ClientError> {
    match chatgpt_launcher::stop(paths).await? {
        Some(pid) => note(&format!("Stopped the chatgpt daemon (pid {pid}).")),
        None => note("The chatgpt daemon wasn't running."),
    }
    Ok(ExitCode::SUCCESS)
}

/// The newest `daemon.log.YYYY-MM-DD`.
fn newest_log(dir: &Path) -> Option<PathBuf> {
    let prefix = format!("{DAEMON_LOG_PREFIX}.");
    let mut logs: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix))
        })
        .collect();
    // The date suffix sorts in time order.
    logs.sort();
    logs.pop()
}

/// Print the last `lines` lines of the daemon's log; with `follow`, keep
/// printing what's appended, moving to the next day's file when it appears.
pub fn logs(paths: &Paths, lines: usize, follow: bool) -> Result<ExitCode, ClientError> {
    let dir = paths.log_dir();
    let mut current: Option<(PathBuf, u64)> = None;
    match newest_log(&dir) {
        Some(path) => {
            let text = std::fs::read_to_string(&path).map_err(|error| io_error(&path, &error))?;
            let tail: Vec<&str> = text.lines().rev().take(lines).collect();
            data(
                &tail
                    .into_iter()
                    .rev()
                    .map(|line| format!("{line}\n"))
                    .collect::<String>(),
            );
            current = Some((path, text.len() as u64));
        }
        None => note(&format!("No daemon log yet in {}.", dir.display())),
    }
    if !follow {
        return Ok(ExitCode::SUCCESS);
    }
    loop {
        std::thread::sleep(Duration::from_millis(500));
        if let Some(newer) = newest_log(&dir)
            && current.as_ref().is_none_or(|(path, _)| *path != newer)
        {
            current = Some((newer, 0));
        }
        let Some((path, offset)) = current.as_mut() else {
            continue;
        };
        let Ok(mut file) = std::fs::File::open(&*path) else {
            continue;
        };
        let length = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        if length < *offset {
            *offset = 0;
        }
        if length == *offset || file.seek(SeekFrom::Start(*offset)).is_err() {
            continue;
        }
        let mut appended = String::new();
        let read = file.read_to_string(&mut appended).unwrap_or(0);
        *offset += read as u64;
        data(&appended);
    }
}
