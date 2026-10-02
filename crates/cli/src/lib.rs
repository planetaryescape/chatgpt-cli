//! The `chatgpt` command. `sync`, `list`, `stats`, `export`, lexical
//! `search`, `daemon` and `import-legacy` are native: they ask the daemon
//! over IPC and print its answer. Every other command, and `search` with
//! `--semantic`, `--hybrid` or `--remote`, is handed, unchanged, to the TS
//! CLI (the bridge).
//!
//! This crate never touches the index or chatgpt.com itself: only the
//! daemon does (tests/workspace_boundaries.rs). `main.rs` passes the
//! daemon's entry point in for `chatgpt daemon run`.

mod args;
mod bridge;
mod daemon_cmd;
mod export_cmd;
mod launch_agent;
mod output;
mod reads;
mod search_cmd;
mod sync_cmd;

use std::ffi::OsString;
use std::process::ExitCode;

use chatgpt_core::{ErrorKind, Instance, Paths};
use chatgpt_launcher::ClientError;
use chatgpt_protocol::SessionChoice;
use clap::Parser;

use args::{Cli, Command, DaemonCommand};

/// The daemon's foreground entry point, from the daemon crate.
pub type DaemonEntry = fn(Paths) -> ExitCode;

pub fn main(daemon: DaemonEntry) -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();
    if !is_native(&args) {
        return bridge::exec(&args);
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

/// Global options that take a value, which come before the command.
const VALUE_OPTIONS: &[&str] = &["--browser", "--profile", "--instance"];

/// Whether this build runs the command itself: a native command, `help`
/// for one, top-level `--help`/`--version`, or no command at all.
/// Anything else, unknown commands and options included, goes to the TS CLI,
/// which knows what to say about them, and so does a `search` in a mode
/// only the TS CLI has.
fn is_native(args: &[OsString]) -> bool {
    let mut words = args.iter().skip(1).map(|arg| arg.to_string_lossy());
    let mut command = None;
    while let Some(word) = words.next() {
        if VALUE_OPTIONS.contains(&word.as_ref()) {
            words.next();
            continue;
        }
        if VALUE_OPTIONS
            .iter()
            .any(|option| word.starts_with(&format!("{option}=")))
        {
            continue;
        }
        if word.starts_with('-') {
            return matches!(word.as_ref(), "-h" | "--help" | "-V" | "--version");
        }
        command = Some(word.into_owned());
        break;
    }
    match command.as_deref() {
        None => true,
        Some("help") => words
            .next()
            .is_none_or(|topic| args::NATIVE.contains(&topic.as_ref())),
        Some("search") => !words
            .take_while(|word| word != "--")
            .any(|word| args::BRIDGED_SEARCH_FLAGS.contains(&word.as_ref())),
        Some(command) => args::NATIVE.contains(&command),
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
        Command::Daemon(DaemonCommand::Logs { follow, lines }) => {
            daemon_cmd::logs(&paths, lines, follow)
        }
        command => block_on(async move {
            match command {
                Command::Sync { full } => sync_cmd::sync(&paths, full, session).await,
                Command::List(list) => reads::list(&paths, list).await,
                Command::Stats(filters) => reads::stats(&paths, filters, session).await,
                Command::Export(export) => export_cmd::export(&paths, export, session).await,
                Command::Search(search) => search_cmd::search(&paths, search).await,
                Command::ImportLegacy => sync_cmd::import_legacy(&paths).await,
                Command::Daemon(DaemonCommand::Status { json }) => {
                    daemon_cmd::status(&paths, json).await
                }
                Command::Daemon(DaemonCommand::Stop) => daemon_cmd::stop(&paths).await,
                Command::Daemon(_) => Ok(ExitCode::SUCCESS),
            }
        })?,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn native(line: &str) -> bool {
        let args: Vec<OsString> = std::iter::once("chatgpt")
            .chain(line.split_whitespace())
            .map(OsString::from)
            .collect();
        is_native(&args)
    }

    #[test]
    fn only_ported_commands_stay_native() {
        assert!(native(""));
        assert!(native("--help"));
        assert!(native("--version"));
        assert!(native("list --json"));
        assert!(native("--browser chrome stats"));
        assert!(native("--browser=chrome --profile Default sync --full"));
        assert!(native("daemon status"));
        assert!(native("help list"));
        assert!(native("help"));
        assert!(native("export abc -o"));
        assert!(native("show abc"));
        assert!(native("help export"));
        assert!(native("search rust --format json --limit 5"));
        assert!(native("search -- --semantic"));
        assert!(!native("search rust --semantic"));
        assert!(!native("search --hybrid rust"));
        assert!(!native("--browser chrome search rust --remote --limit 5"));
        assert!(!native("search-index"));
        assert!(!native("--browser chrome classify"));
        assert!(!native("help classify"));
        assert!(!native("frobnicate"));
        assert!(!native("--frobnicate list"));
    }
}
