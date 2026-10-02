//! The bridge: a command this build hasn't ported runs in the TS CLI as
//! `bun <cli.ts> <args…>`. The process becomes bun (`exec`), so stdin,
//! stdout, stderr, the terminal, signals and the exit code are the TS CLI's,
//! unchanged. The TS CLI is found by path (`chatgpt_core::ts_cli`), never as
//! `chatgpt` on PATH, which may be this binary.

use std::ffi::OsString;
use std::os::unix::process::{CommandExt, ExitStatusExt};
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
        .args(ts_args(args))
        .env(BRIDGED_ENV, "1");
    Ok((command, ts))
}

/// The arguments for the TS CLI: everything after the program name except
/// `--instance <name>` (or `--instance=<name>`), which only this build
/// knows and the TS CLI would reject. `--` ends the options.
fn ts_args(args: &[OsString]) -> Vec<OsString> {
    let mut out = Vec::new();
    let mut words = args.iter().skip(1);
    while let Some(word) = words.next() {
        if word == "--" {
            out.push(word.clone());
            out.extend(words.cloned());
            break;
        }
        if word == "--instance" {
            words.next();
            continue;
        }
        if word.to_string_lossy().starts_with("--instance=") {
            continue;
        }
        out.push(word.clone());
    }
    out
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
        // A child killed by a signal exits as a shell reports it: 128 + n.
        Ok(status) => {
            let code = status
                .code()
                .or_else(|| status.signal().map(|signal| 128 + signal))
                .unwrap_or(1);
            ExitCode::from(u8::try_from(code).unwrap_or(1))
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rust_only_instance_option_stays_behind() {
        let args =
            |line: &str| -> Vec<OsString> { line.split_whitespace().map(OsString::from).collect() };
        assert_eq!(
            ts_args(&args("chatgpt --instance work export abc -o")),
            args("export abc -o")
        );
        assert_eq!(
            ts_args(&args("chatgpt --browser dia --instance=work show x")),
            args("--browser dia show x")
        );
        assert_eq!(
            ts_args(&args("chatgpt search -- --instance")),
            args("search -- --instance")
        );
    }
}
