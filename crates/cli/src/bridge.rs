//! The bridge: a command this build hasn't ported runs in the TS CLI as
//! `bun <cli.ts> <args…>`. The process becomes bun (`exec`), so stdin,
//! stdout, stderr, the terminal, signals and the exit code are the TS CLI's,
//! unchanged. The TS CLI is found by path (`chatgpt_core::ts_cli`), never as
//! `chatgpt` on PATH, which may be this binary.

use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::process::ExitCode;

use chatgpt_core::ts_cli::{self, BRIDGED_ENV};

use crate::output::error_line;

/// `bun <cli.ts> <args…>`, or why it can't run (already said on stderr).
fn command(args: &[OsString]) -> Result<(std::process::Command, ts_cli::TsCli), ExitCode> {
    if std::env::var_os(BRIDGED_ENV).is_some() {
        error_line(&format!(
            "the TS CLI ran this chatgpt again ({BRIDGED_ENV} is set); check that {} points at the TS CLI's src/cli.ts, not this binary",
            ts_cli::TS_CLI_ENV
        ));
        return Err(ExitCode::FAILURE);
    }
    let ts = ts_cli::locate().map_err(|error| {
        error_line(&error.to_string());
        ExitCode::FAILURE
    })?;
    let mut command = std::process::Command::new(&ts.bun);
    command
        .arg(&ts.entry)
        .args(args.iter().skip(1))
        .env(BRIDGED_ENV, "1");
    Ok((command, ts))
}

pub fn exec(args: &[OsString]) -> ExitCode {
    let (mut command, ts) = match command(args) {
        Ok(found) => found,
        Err(code) => return code,
    };
    let error = command.exec();
    error_line(&format!(
        "couldn't run the TS CLI ({} {}): {error}",
        ts.bun.display(),
        ts.entry.display()
    ));
    ExitCode::FAILURE
}

/// The bridge for a command whose stdin this process already read: the TS
/// CLI gets `input` on its stdin, and the terminal for the rest.
pub fn run_with_stdin(args: &[OsString], input: &[u8]) -> ExitCode {
    let (mut command, ts) = match command(args) {
        Ok(found) => found,
        Err(code) => return code,
    };
    let ran = command
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            if let Some(mut stdin) = child.stdin.take() {
                // A TS CLI that stops reading early is fine.
                let _ = std::io::Write::write_all(&mut stdin, input);
            }
            child.wait()
        });
    match ran {
        Ok(status) => ExitCode::from(u8::try_from(status.code().unwrap_or(1)).unwrap_or(1)),
        Err(error) => {
            error_line(&format!(
                "couldn't run the TS CLI ({} {}): {error}",
                ts.bun.display(),
                ts.entry.display()
            ));
            ExitCode::FAILURE
        }
    }
}
