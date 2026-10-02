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
    lines.push(full_sync_line(sync, now));
    if let Some(backoff) = &status.backoff {
        lines.push(format!(
            "backing off: {}s more ({})",
            (backoff.until - now).max(0),
            backoff.reason
        ));
    }
    lines.push(format!(
        "session: {}",
        match status.session.as_deref() {
            Some(source) => source,
            None if status.session_reading => {
                "reading session… (a Keychain prompt may be waiting)"
            }
            None => "not read yet",
        }
    ));
    let classification = &status.classification;
    lines.push(format!(
        "classification: questions {}, Luna {}, from {}",
        classification.questions_version, classification.luna_version, classification.source
    ));
    if let Some(problem) = &classification.problem {
        lines.push(format!("classification problem: {problem}"));
    }
    lines.extend(auto_jev_lines(&status.auto_jev, now));
    lines.push(search_index_line(&status.search_index));
    if let Some(error) = &status.search_index.last_error {
        lines.push(format!("search index error: {error}"));
    }
    let embeddings = &status.embeddings;
    lines.push(embeddings_line(embeddings));
    if let Some(error) = &embeddings.last_error {
        lines.push(format!("embedding error: {error}"));
    }
    lines.join("\n") + "\n"
}

/// When the last full pass ran, and when the daily one may run again.
fn full_sync_line(sync: &chatgpt_protocol::SyncStatus, now: i64) -> String {
    let last = sync
        .last_full_at
        .map_or_else(|| "never".to_owned(), |at| ago(at, now));
    match sync.next_full_at {
        Some(next) if next > now => format!(
            "full sync: {last}; next once idle, in {}",
            ago(now - (next - now), now).trim_end_matches(" ago")
        ),
        Some(_) => format!("full sync: {last}; next once idle"),
        // A daemon from before the daily full sync.
        None => format!("full sync: {last}"),
    }
}

/// The background Jev: on or off, today's tally, the last run.
fn auto_jev_lines(auto: &chatgpt_protocol::AutoJevStatus, now: i64) -> Vec<String> {
    let mut lines = vec![if auto.enabled {
        format!(
            "background Jev: on, {} judged today ({}){}",
            auto.judged_today,
            usd(auto.cost_today_usd),
            if auto.in_progress {
                ", judging now"
            } else {
                ""
            }
        )
    } else {
        format!(
            "background Jev: off ({})",
            auto.off_reason.as_deref().unwrap_or("not configured")
        )
    }];
    if let (Some(at), Some(summary)) = (auto.last_run_at, &auto.last_summary) {
        lines.push(format!(
            "background Jev last run: {} ({summary})",
            ago(at, now)
        ));
    }
    if let Some(error) = &auto.last_error {
        lines.push(format!("background Jev error: {error}"));
    }
    lines
}

/// `formatUsd`: four places under a cent, else two.
fn usd(amount: f64) -> String {
    if amount > 0.0 && amount < 0.01 {
        format!("${amount:.4}")
    } else {
        format!("${amount:.2}")
    }
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
        parts.push(format!("{} set aside for an hour", index.failed));
    }
    if let Some(waiting) = &index.waiting {
        parts.push(waiting.clone());
    }
    format!("search index: {}", parts.join(", "))
}

/// How far the background embedder has got, and why it waits.
fn embeddings_line(embeddings: &chatgpt_protocol::EmbeddingStatus) -> String {
    let mut parts = vec![format!(
        "{} of {} chunks embedded",
        embeddings.embedded, embeddings.chunks
    )];
    if embeddings.in_progress {
        parts.push("embedding now".to_owned());
    }
    if embeddings.failed > 0 {
        parts.push(format!("{} skipped", embeddings.failed));
    }
    if let Some(waiting) = &embeddings.waiting {
        parts.push(waiting.clone());
    }
    format!("embeddings: {}", parts.join(", "))
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

/// The last `lines` lines of `file`, each ending in a newline, and the
/// file's length. Reads backwards a block at a time, so only about as much
/// as those lines take is read, however large the day's log is.
fn tail_lines(file: &mut std::fs::File, lines: usize) -> std::io::Result<(String, u64)> {
    const BLOCK: u64 = 64 * 1024;
    let length = file.metadata()?.len();
    let mut start = length;
    let mut bytes: Vec<u8> = Vec::new();
    // A last line without its newline counts as a line too.
    let wanted = |bytes: &[u8]| {
        let newlines = bytes.iter().filter(|&&byte| byte == b'\n').count();
        let unterminated = usize::from(bytes.last().is_some_and(|&byte| byte != b'\n'));
        newlines + unterminated > lines
    };
    while start > 0 && !wanted(&bytes) {
        let from = start.saturating_sub(BLOCK);
        let mut block = vec![0; usize::try_from(start - from).unwrap_or(0)];
        file.seek(SeekFrom::Start(from))?;
        file.read_exact(&mut block)?;
        block.extend_from_slice(&bytes);
        bytes = block;
        start = from;
    }
    let text = String::from_utf8_lossy(&bytes);
    let tail: Vec<&str> = text.lines().rev().take(lines).collect();
    let tail = tail
        .into_iter()
        .rev()
        .map(|line| format!("{line}\n"))
        .collect();
    Ok((tail, length))
}

/// Print the last `lines` lines of the daemon's log; with `follow`, keep
/// printing what's appended, moving to the next day's file when it appears.
pub fn logs(paths: &Paths, lines: usize, follow: bool) -> Result<ExitCode, ClientError> {
    let dir = paths.log_dir();
    let mut current: Option<(PathBuf, u64)> = None;
    match newest_log(&dir) {
        Some(path) => {
            let (tail, length) = std::fs::File::open(&path)
                .and_then(|mut file| tail_lines(&mut file, lines))
                .map_err(|error| io_error(&path, &error))?;
            data(&tail);
            current = Some((path, length));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tail_of(contents: &str, lines: usize) -> String {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("log");
        std::fs::write(&path, contents).expect("write");
        let mut file = std::fs::File::open(&path).expect("open");
        let (tail, length) = tail_lines(&mut file, lines).expect("tail");
        assert_eq!(length, contents.len() as u64);
        tail
    }

    #[test]
    fn the_tail_is_the_last_lines_across_blocks() {
        // Lines of 100 bytes, so the tail spans several 64 KiB blocks.
        let log: String = (0..5_000).map(|n| format!("{n:099}\n")).collect();
        let tail = tail_of(&log, 700);
        let expected: String = (4_300..5_000).map(|n| format!("{n:099}\n")).collect();
        assert_eq!(tail, expected);
        assert_eq!(tail_of("a\nb\nc", 2), "b\nc\n", "an unfinished last line");
        assert_eq!(tail_of("a\nb\n", 5), "a\nb\n");
        assert_eq!(tail_of("", 5), "");
        assert_eq!(tail_of("a\nb\n", 0), "");
    }
}
