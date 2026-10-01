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

pub fn exec(args: &[OsString]) -> ExitCode {
    if std::env::var_os(BRIDGED_ENV).is_some() {
        error_line(&format!(
            "the TS CLI ran this chatgpt again ({BRIDGED_ENV} is set); check that {} points at the TS CLI's src/cli.ts, not this binary",
            ts_cli::TS_CLI_ENV
        ));
        return ExitCode::FAILURE;
    }
    let ts = match ts_cli::locate() {
        Ok(ts) => ts,
        Err(error) => {
            error_line(&error.to_string());
            return ExitCode::FAILURE;
        }
    };
    let error = std::process::Command::new(&ts.bun)
        .arg(&ts.entry)
        .args(args.iter().skip(1))
        .env(BRIDGED_ENV, "1")
        .exec();
    error_line(&format!(
        "couldn't run the TS CLI ({} {}): {error}",
        ts.bun.display(),
        ts.entry.display()
    ));
    ExitCode::FAILURE
}
