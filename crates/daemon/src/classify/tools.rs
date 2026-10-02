//! Running the subscription CLIs the TS CLI falls back on, `codex exec`
//! and `claude -p` (`exec` in `src/classify/summarise.ts` and `askLuna` in
//! `luna.ts` @ 1b8c950), more carefully than it does:
//!
//! - in an environment holding only what they need to find their own
//!   login (`HOME`, the locale, `TMPDIR`, their config directories) and the
//!   client's `PATH`: no API key, cookie or token of ours reaches them;
//! - in their own process group, killed whole on a timeout or when the
//!   run is dropped (a client that went away), so none outlives its run;
//! - with their output captured and never logged: it holds the chat, and
//!   their errors can quote it. Failures say only how they exited.

use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// How long one call may take: a long chat's summary at medium reasoning
/// takes minutes.
const TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// In debug builds, a shorter timeout for tests (milliseconds).
const TIMEOUT_ENV: &str = "CHATGPT_TEST_TOOL_TIMEOUT_MS";

/// The variables passed on from the daemon's environment: enough for the
/// CLIs to find their own login and config, nothing else.
const KEPT_ENV: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "CODEX_HOME",
    "CLAUDE_CONFIG_DIR",
    "XDG_CONFIG_HOME",
];

#[derive(Debug)]
pub struct Finished {
    /// The exit code; `None` when a signal ended it.
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub stdout: Vec<u8>,
}

impl Finished {
    /// How it exited, for an error: `exited 2`, `was killed by signal 9`.
    pub fn exit(&self) -> String {
        match (self.code, self.signal) {
            (Some(code), _) => format!("exited {code}"),
            (None, Some(signal)) => format!("was killed by signal {signal}"),
            (None, None) => "exited".to_owned(),
        }
    }
}

fn timeout() -> Duration {
    chatgpt_core::debug_env(TIMEOUT_ENV)
        .and_then(|millis| millis.parse().ok())
        .map_or(TIMEOUT, Duration::from_millis)
}

/// Kills the process group when dropped unless the run finished.
struct GroupGuard {
    pgid: Option<i32>,
}

impl GroupGuard {
    fn kill(&mut self) {
        if let Some(pgid) = self.pgid.take() {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(pgid),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Run `program` with `args` in `cwd`, `stdin` written to it, and wait.
/// `name` names it in errors (`codex`, `claude -p`).
pub async fn run(
    name: &str,
    program: &Path,
    args: &[String],
    stdin: &[u8],
    cwd: &Path,
    path_var: Option<&str>,
) -> Result<Finished, String> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Never read: it can quote the chat. Discarded, so a chatty child
        // can't fill a pipe and stall.
        .stderr(Stdio::null())
        .process_group(0)
        .kill_on_drop(true);
    for name in KEPT_ENV {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    if let Some(path) = path_var {
        command.env("PATH", path);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("couldn't start {name}: {error}"))?;
    let mut guard = GroupGuard {
        pgid: child.id().and_then(|pid| i32::try_from(pid).ok()),
    };
    let mut input = child.stdin.take();
    let mut output = child.stdout.take();
    let exchange = async {
        let write = async {
            if let Some(mut input) = input.take() {
                // A child that exits without reading all of it is judged by
                // its exit, not by the broken pipe.
                let _ = input.write_all(stdin).await;
                let _ = input.shutdown().await;
            }
        };
        let read = async {
            let mut stdout = Vec::new();
            if let Some(output) = output.as_mut() {
                let _ = output.read_to_end(&mut stdout).await;
            }
            stdout
        };
        let ((), stdout) = tokio::join!(write, read);
        let status = child.wait().await;
        (status, stdout)
    };
    let limit = timeout();
    let (status, stdout) = match tokio::time::timeout(limit, exchange).await {
        Ok(done) => done,
        Err(_) => {
            guard.kill();
            return Err(format!(
                "{name} didn't finish within {} seconds",
                limit.as_secs()
            ));
        }
    };
    let status = status.map_err(|error| format!("{name} failed to run: {error}"))?;
    // It exited; its group may still hold children it left behind.
    guard.kill();
    Ok(Finished {
        code: status.code(),
        signal: status.signal(),
        stdout,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> (std::path::PathBuf, Vec<String>) {
        ("/bin/sh".into(), vec!["-c".into(), script.into()])
    }

    #[tokio::test]
    async fn the_child_sees_only_the_kept_environment() {
        let dir = tempfile::tempdir().expect("dir");
        let (program, args) = sh("cat; env | sort");
        let finished = run(
            "sh",
            &program,
            &args,
            b"input\n",
            dir.path(),
            Some("/usr/bin:/bin"),
        )
        .await
        .expect("ran");
        let out = String::from_utf8(finished.stdout).expect("utf8");
        assert!(out.starts_with("input\n"), "{out}");
        assert!(out.contains("PATH=/usr/bin:/bin"), "{out}");
        for line in out.lines().skip(1) {
            let name = line.split('=').next().unwrap_or_default();
            assert!(
                name == "PATH"
                    || name == "PWD"
                    || name == "SHLVL"
                    || name == "_"
                    || KEPT_ENV.contains(&name),
                "leaked {name}"
            );
        }
        assert_eq!(finished.code, Some(0));
    }

    #[tokio::test]
    async fn exits_are_reported_without_output() {
        let dir = tempfile::tempdir().expect("dir");
        let (program, args) = sh("echo SENTINEL >&2; exit 3");
        let finished = run("sh", &program, &args, b"", dir.path(), None)
            .await
            .expect("ran");
        assert_eq!(finished.exit(), "exited 3");
    }
}
