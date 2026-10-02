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

/// Wait until `pid`, a child not yet reaped, has exited: it shows as a
/// zombie. Without a `ps` to ask, it returns at once (the caller then kills
/// the group as soon as its output ended, as before).
async fn exited_unreaped(pid: u32) {
    const POLL: Duration = Duration::from_millis(20);
    loop {
        let state = tokio::task::spawn_blocking(move || chatgpt_core::ps_field(pid, "state"))
            .await
            .ok()
            .flatten();
        match state {
            Some(state) if !state.starts_with('Z') => tokio::time::sleep(POLL).await,
            _ => return,
        }
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
    let leader = child.id();
    let mut guard = GroupGuard {
        pgid: leader.and_then(|pid| i32::try_from(pid).ok()),
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
        // Kill what it left in its group before reaping it: until the
        // leader is reaped its PID, and so the group's ID, can't go to
        // another process, so the signal can only reach this run's.
        if let Some(pid) = leader {
            exited_unreaped(pid).await;
        }
        guard.kill();
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
    async fn what_a_run_leaves_in_its_group_is_killed_and_its_exit_kept() {
        let dir = tempfile::tempdir().expect("dir");
        // A child that outlives it, holding none of its output.
        let (program, args) = sh("sleep 30 >/dev/null 2>&1 & echo $!; exit 4");
        let finished = run("sh", &program, &args, b"", dir.path(), None)
            .await
            .expect("ran");
        assert_eq!(finished.exit(), "exited 4", "its own exit, not the kill");
        let left: i32 = String::from_utf8(finished.stdout)
            .expect("utf8")
            .trim()
            .parse()
            .expect("a pid");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while nix::sys::signal::kill(nix::unistd::Pid::from_raw(left), None).is_ok()
            && chatgpt_core::ps_field(u32::try_from(left).expect("pid"), "state")
                .is_some_and(|state| !state.starts_with('Z'))
        {
            assert!(std::time::Instant::now() < deadline, "pid {left} outlived the run");
            std::thread::sleep(Duration::from_millis(20));
        }
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
