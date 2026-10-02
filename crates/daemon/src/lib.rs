//! The chatgpt daemon. It holds the browser session (cookies read once, the
//! access token in memory only), keeps the index fresh with background
//! delta syncs, classifies chats (Jev on new ones in the background),
//! imports the TS CLI's titles and caches while the bridge exists, and
//! answers the CLI over a Unix socket in the instance's 0700 run directory.
//!
//! Started by any client that finds no daemon, as a detached
//! `chatgpt daemon run --instance <name>`.

mod api;
mod classify;
mod export;
mod filters;
mod handlers;
mod jev;
mod js;
mod logging;
mod memories;
mod mutate;
mod policy;
mod progress;
mod projects;
mod reads;
mod render;
mod search;
mod select;
mod server;
mod session;
mod state;
mod sync;
mod ts_sync;

use std::os::unix::process::CommandExt;
use std::process::ExitCode;

use chatgpt::http::{PSEUDO_HEADER_ORDER_ENV, environment_prepared, pseudo_header_order};
use chatgpt_core::Paths;

/// Run the daemon in the foreground until `Shutdown`, SIGTERM or SIGINT.
pub fn run(paths: Paths) -> ExitCode {
    // The HTTP client refuses to build unless this already holds Chrome's
    // pseudo-header order, so that no build ever writes the environment
    // while other threads read it (docs/issues/impit-set-var-race.md). No
    // thread exists yet, but setting it here would need `unsafe`: start
    // again with it set instead. `exec` keeps the PID, so the launcher and
    // the pid file see the same process.
    if !environment_prepared() {
        let error = match std::env::current_exe() {
            Ok(exe) => std::process::Command::new(exe)
                .args(std::env::args_os().skip(1))
                .env(PSEUDO_HEADER_ORDER_ENV, pseudo_header_order())
                .exec(),
            Err(error) => error,
        };
        eprintln!("chatgpt daemon: cannot restart with {PSEUDO_HEADER_ORDER_ENV} set: {error}");
        return ExitCode::FAILURE;
    }
    let _log = match logging::init(&paths.log_dir()) {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("chatgpt daemon: cannot open the log: {error}");
            return ExitCode::FAILURE;
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("chatgpt daemon: cannot start the async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(server::serve(paths)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(server::Fatal::DatabaseTooNew(message)) => {
            tracing::error!("{message}");
            eprintln!("chatgpt daemon: {message}");
            ExitCode::from(chatgpt_protocol::EXIT_DATABASE_TOO_NEW)
        }
        Err(server::Fatal::Other(message)) => {
            tracing::error!("{message}");
            eprintln!("chatgpt daemon: {message}");
            ExitCode::FAILURE
        }
    }
}
