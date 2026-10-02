//! The `chatgpt` command. Every command is native: it asks the daemon over
//! IPC and prints its answer (`configure` only writes the user config;
//! `tui` is a daemon client of its own, in `chatgpt-tui`). Nothing goes to
//! the TS CLI any more; the bridge to it is unreachable and goes in 6b.
//!
//! This crate never touches the index or chatgpt.com itself: only the
//! daemon does (tests/workspace_boundaries.rs). `main.rs` passes the
//! daemon's entry point in for `chatgpt daemon run`.

mod args;
#[allow(
    dead_code,
    reason = "unreachable since every command is native; deleted with the TS CLI"
)]
mod bridge;
mod change_cmd;
mod classify_cmd;
mod configure_cmd;
mod daemon_cmd;
mod export_cmd;
#[cfg(debug_assertions)]
mod fake_model;
mod launch_agent;
mod memory_cmd;
mod output;
mod project_cmd;
mod prompt;
mod reads;
mod review_cmd;
mod search_cmd;
mod sync_cmd;

use std::ffi::OsString;
use std::process::ExitCode;

use chatgpt_core::{ErrorKind, Instance, Paths};
use chatgpt_launcher::ClientError;
use chatgpt_protocol::{ChatAction, SessionChoice};
use clap::Parser;

use args::{Cli, Command, DaemonCommand};

/// The daemon's foreground entry point, from the daemon crate.
pub type DaemonEntry = fn(Paths) -> ExitCode;

pub fn main(daemon: DaemonEntry) -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();
    // Tests' stand-in `codex` and `claude` (debug builds only).
    #[cfg(debug_assertions)]
    if args.get(1).is_some_and(|arg| arg == "__fake-model-cli") {
        let rest: Vec<String> = args[2..]
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        return fake_model::run(&rest);
    }
    let cli = Cli::parse_from(&args);
    match run(cli, daemon) {
        Ok(code) => code,
        Err(error) => {
            output::error_line(&error.message);
            ExitCode::from(error.kind.exit_code())
        }
    }
}

fn run(cli: Cli, daemon: DaemonEntry) -> Result<ExitCode, ClientError> {
    let instance = Instance::detect(cli.instance.as_deref())
        .map_err(|error| ClientError::new(ErrorKind::InvalidInput, error.to_string()))?;
    let paths = Paths::resolve(instance)
        .map_err(|error| ClientError::new(ErrorKind::Internal, error.to_string()))?;
    let session = session_choice(cli.browser, cli.profile);
    match cli.command {
        Command::Daemon(DaemonCommand::Run) => Ok(daemon(paths)),
        Command::Daemon(DaemonCommand::Launch) => Ok(chatgpt_launcher::launch(&paths)),
        Command::Daemon(DaemonCommand::Install) => launch_agent::install(&paths),
        Command::Daemon(DaemonCommand::Uninstall) => launch_agent::uninstall(),
        Command::Daemon(DaemonCommand::EmbedWorker { model_dir, fake }) => {
            Ok(embed_worker(model_dir, fake))
        }
        Command::Daemon(DaemonCommand::Logs { follow, lines }) => {
            daemon_cmd::logs(&paths, lines, follow)
        }
        Command::Configure { provider, remove } => configure_cmd::configure(provider, remove),
        Command::Tui => chatgpt_tui::run(&paths, session),
        command => block_on(async move {
            match command {
                Command::Sync { full } => sync_cmd::sync(&paths, full, session).await,
                Command::List(list) => reads::list(&paths, list).await,
                Command::Stats(filters) => reads::stats(&paths, filters, session).await,
                Command::Export(export) => export_cmd::export(&paths, export, session).await,
                Command::Search(search) => search_cmd::search(&paths, search, session).await,
                Command::SearchIndex(scope) => search_cmd::search_index(&paths, scope).await,
                Command::ImportLegacy => sync_cmd::import_legacy(&paths).await,
                Command::Archive(change) => {
                    change_cmd::change(&paths, ChatAction::Archive, change, session).await
                }
                Command::Unarchive(change) => {
                    change_cmd::change(&paths, ChatAction::Unarchive, change, session).await
                }
                Command::Delete(change) => {
                    change_cmd::change(&paths, ChatAction::Delete, change, session).await
                }
                Command::Rename(rename) => change_cmd::rename(&paths, rename, session).await,
                Command::Title(title) => change_cmd::title(&paths, title).await,
                Command::Titles(titles) => classify_cmd::titles(&paths, titles).await,
                Command::Classify(classify) => {
                    classify_cmd::classify(&paths, classify, session).await
                }
                Command::Project(project) => project_cmd::run(&paths, project, session).await,
                Command::Memory(memory) => memory_cmd::run(&paths, memory, session).await,
                Command::Daemon(DaemonCommand::Status { json }) => {
                    daemon_cmd::status(&paths, json).await
                }
                Command::Daemon(DaemonCommand::Stop) => daemon_cmd::stop(&paths).await,
                Command::Review(review) => review_cmd::review(&paths, review, session).await,
                Command::Daemon(_) | Command::Configure { .. } | Command::Tui => {
                    Ok(ExitCode::SUCCESS)
                }
            }
        })?,
    }
}

/// The embedding worker the daemon starts. The fake embedder is for tests
/// of debug builds only; `CHATGPT_TEST_EMBED_DELAY_MS` slows it down, for
/// texts containing `CHATGPT_TEST_EMBED_SLOW_TEXT` only if that's set.
fn embed_worker(model_dir: Option<std::path::PathBuf>, fake: bool) -> ExitCode {
    use chatgpt_embed::worker::{Model, serve};
    if fake {
        if !cfg!(debug_assertions) {
            output::error_line("--fake is only for debug builds");
            return ExitCode::FAILURE;
        }
        let delay = std::env::var("CHATGPT_TEST_EMBED_DELAY_MS")
            .ok()
            .and_then(|ms| ms.parse().ok())
            .map_or(std::time::Duration::ZERO, std::time::Duration::from_millis);
        let slow_only = std::env::var("CHATGPT_TEST_EMBED_SLOW_TEXT")
            .ok()
            .filter(|marker| !marker.is_empty());
        return serve(Model::Fake { delay, slow_only });
    }
    match model_dir {
        Some(dir) => serve(Model::Files(dir)),
        // clap requires --model-dir without --fake.
        None => ExitCode::FAILURE,
    }
}

/// `--browser` and `--profile`, else `CHATGPT_BROWSER` and
/// `CHATGPT_BROWSER_PROFILE`, as the TS CLI reads them.
fn session_choice(browser: Option<String>, profile: Option<String>) -> SessionChoice {
    let env = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    SessionChoice {
        browser: browser.or_else(|| env("CHATGPT_BROWSER")),
        profile: profile.or_else(|| env("CHATGPT_BROWSER_PROFILE")),
    }
}

fn block_on<T>(work: impl Future<Output = T>) -> Result<T, ClientError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            ClientError::new(
                ErrorKind::Internal,
                format!("cannot start the async runtime: {error}"),
            )
        })?;
    Ok(runtime.block_on(work))
}
