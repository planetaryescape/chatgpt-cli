// Adapted from spotuify crates/spotuify-daemon/src/logging.rs @ b21ab30dac8450e36f753e9a0e0c4ecdaa9b7b3e
// (daily rotation keeping 7 files, a private log directory, `RUST_LOG`
// overriding the default filter). Changes: plain text only; chatgpt's own
// filter variable and paths.

//! The daemon's log: `<data_dir>/logs/daemon.log.YYYY-MM-DD`, one file a
//! day, the last 7 kept. Never cookies, tokens or response bodies: the HTTP
//! client's errors drop bodies before they reach a log line
//! (`api::ApiError`), and of the TS sync's output only the TS CLI's own
//! progress and summary lines are kept (`ts_sync::Output`).

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

const FILTER_ENV: &str = "CHATGPT_LOG";

pub fn init(dir: &Path) -> Result<WorkerGuard, String> {
    std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("{}: {error}", dir.display()))?;
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix(chatgpt_core::DAEMON_LOG_PREFIX)
        .max_log_files(7)
        .build(dir)
        .map_err(|error| format!("{}: {error}", dir.display()))?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_env(FILTER_ENV)
        .or_else(|_| EnvFilter::try_from_env("RUST_LOG"))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .try_init();
    Ok(guard)
}
