//! The `chatgpt` binary: the CLI, and the daemon it starts.

use std::process::ExitCode;

fn main() -> ExitCode {
    chatgpt_cli::main(chatgpt_daemon::run)
}
