//! The embedding worker process, from the daemon's side: started as
//! `chatgpt daemon embed-worker` at low CPU priority, one text in and one
//! vector out at a time (`chatgpt_embed::worker` has the frame format).
//!
//! macOS has no way to lower one thread's priority without `unsafe`, which
//! this workspace forbids, so the model runs in its own process: under
//! `taskpolicy -c utility` (or `nice` where that's missing), on one thread.
//! Dropping the [`Worker`] kills it, which is how the daemon unloads the
//! model.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use chatgpt_embed::DIM;
use chatgpt_embed::worker::{FAILED, MAX_TEXT_BYTES, OK, UNUSABLE, request, vector_from_bytes};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

/// Which embedder the worker runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerModel {
    /// The model files in this directory.
    Files(PathBuf),
    /// The fake embedder, for tests (debug builds only).
    Fake,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    /// The model couldn't embed this text; others may still work.
    #[error("{0}")]
    Text(String),
    /// The model won't load: no text will work until that's fixed.
    #[error("{0}")]
    Unusable(String),
    /// The process couldn't be started, or stopped answering.
    #[error("the embedding worker {0}")]
    Broken(String),
}

pub struct Worker {
    // Kept so the process is killed when the worker is dropped.
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    /// An exchange began and never finished (its caller was cancelled or
    /// timed out): the next answer on the pipe belongs to it, not to the
    /// next text. Such a worker must be replaced.
    interrupted: bool,
}

/// macOS's utility QoS: lower priority, and efficiency cores when the
/// performance cores are busy. Measured at two thirds of full speed.
const TASKPOLICY: &str = "/usr/sbin/taskpolicy";
const NICE: &str = "/usr/bin/nice";

fn command(program: &Path, args: &[&std::ffi::OsStr]) -> Command {
    let mut command = if Path::new(TASKPOLICY).is_file() {
        let mut command = Command::new(TASKPOLICY);
        command.args(["-c", "utility"]).arg(program);
        command
    } else if Path::new(NICE).is_file() {
        let mut command = Command::new(NICE);
        command.args(["-n", "10"]).arg(program);
        command
    } else {
        Command::new(program)
    };
    command.args(args);
    command
}

impl Worker {
    pub fn spawn(model: &WorkerModel) -> Result<Self, WorkerError> {
        let exe = std::env::current_exe()
            .map_err(|error| WorkerError::Broken(format!("can't be found: {error}")))?;
        let mut args: Vec<&std::ffi::OsStr> = vec!["daemon".as_ref(), "embed-worker".as_ref()];
        match model {
            WorkerModel::Files(dir) => {
                args.push("--model-dir".as_ref());
                args.push(dir.as_os_str());
            }
            WorkerModel::Fake => args.push("--fake".as_ref()),
        }
        let mut child = command(&exe, &args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| WorkerError::Broken(format!("didn't start: {error}")))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(WorkerError::Broken("has no pipes".to_owned()));
        };
        Ok(Self {
            _child: child,
            stdin,
            stdout: BufReader::new(stdout),
            interrupted: false,
        })
    }

    pub fn interrupted(&self) -> bool {
        self.interrupted
    }

    /// One text in, its answer out. If this future is dropped part way,
    /// the worker stays [`interrupted`](Self::interrupted).
    pub async fn embed(&mut self, text: &str) -> Result<Vec<f32>, WorkerError> {
        self.interrupted = true;
        let answer = self.exchange(text).await;
        // A broken pipe may be out of step too.
        self.interrupted = matches!(answer, Err(WorkerError::Broken(_)));
        answer
    }

    async fn exchange(&mut self, text: &str) -> Result<Vec<f32>, WorkerError> {
        let broken = |error: std::io::Error| WorkerError::Broken(format!("stopped: {error}"));
        if text.len() > MAX_TEXT_BYTES as usize {
            return Err(WorkerError::Text(
                "the text is too long to embed".to_owned(),
            ));
        }
        self.stdin.write_all(&request(text)).await.map_err(broken)?;
        self.stdin.flush().await.map_err(broken)?;
        match self.stdout.read_u8().await.map_err(broken)? {
            OK => {
                let mut bytes = vec![0; DIM * 4];
                self.stdout.read_exact(&mut bytes).await.map_err(broken)?;
                vector_from_bytes(&bytes)
                    .ok_or_else(|| WorkerError::Broken("sent a bad vector".to_owned()))
            }
            tag @ (FAILED | UNUSABLE) => {
                let length = self.stdout.read_u32_le().await.map_err(broken)?;
                if length > MAX_TEXT_BYTES {
                    return Err(WorkerError::Broken("sent a bad message".to_owned()));
                }
                let mut message = vec![0; length as usize];
                self.stdout.read_exact(&mut message).await.map_err(broken)?;
                let message = String::from_utf8_lossy(&message).into_owned();
                Err(if tag == FAILED {
                    WorkerError::Text(message)
                } else {
                    WorkerError::Unusable(message)
                })
            }
            other => Err(WorkerError::Broken(format!(
                "sent an unknown answer {other}"
            ))),
        }
    }
}
