// Adapted from ms-todo crates/cli/src/daemon_client.rs @ 72a406042a4d46c2f8cc7c3afe6bda12e11f923b
// (request/response correlation, stall timeouts, `daemon launch` as the
// middle of a double fork, the lock-file liveness probe, stop = socket gone
// and PID exited, zombie detection) and spotuify crates/spotuify-launcher/src/lib.rs
// @ b21ab30dac8450e36f753e9a0e0c4ecdaa9b7b3e (a separate launcher crate, so
// the CLI never links the daemon; the compatibility rule that keeps a newer
// daemon on the same protocol). Changes: chatgpt paths and errors; no
// mutation or subscription requests.

//! Talking to the daemon, and starting and stopping it. Any client that
//! finds the socket missing or dead starts a detached daemon and waits until
//! `Status` answers with a compatible protocol version.
//!
//! A request gives up only after the daemon has sent nothing for it for a
//! while (a stall), never on total time: each progress event restarts the
//! clock, so a long `sync` runs as long as it keeps moving.

use std::fs::File;
use std::io::{ErrorKind as IoErrorKind, Read, Seek, SeekFrom};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::process::{Child, ExitCode, Stdio};
use std::time::Duration;

use chatgpt_core::{ErrorKind, Paths};
use chatgpt_protocol::{
    Codec, DaemonStatus, EXIT_DATABASE_TOO_NEW, Event, Message, PROTOCOL_VERSION, Payload,
    ProgressKind, Request, Response, ResponseData,
};
use fs2::FileExt;
use futures_util::{SinkExt, StreamExt};
use nix::errno::Errno;
use nix::sys::signal::{Signal, kill};
use nix::unistd::{Pid, setsid};
use tokio::net::UnixStream;
use tokio::time::Instant;
use tokio_util::codec::Framed;

/// How long a daemon may take to answer `Status` or `Shutdown`.
const QUICK_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a request may go without an answer or a progress event. Longer
/// than the daemon's worst bounded step: a rate-limit wait the HTTP client
/// sits out (60 s) on top of its retries.
const STALL_TIMEOUT: Duration = Duration::from_secs(300);
const STALL_TIMEOUT_ENV: &str = "CHATGPT_REQUEST_TIMEOUT_MS";
const READY_TIMEOUT: Duration = Duration::from_secs(15);
/// In debug builds, a shorter `READY_TIMEOUT` for tests (milliseconds).
const READY_TIMEOUT_ENV: &str = "CHATGPT_TEST_READY_TIMEOUT_MS";
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long `daemon launch` relays the daemon's exit: past `READY_TIMEOUT`,
/// so it outlasts its client's wait, but bounded, so a launcher whose client
/// died doesn't stay the daemon's parent.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(READY_TIMEOUT.as_secs() * 2);
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const LOG_TAIL_LINES: usize = 5;

/// A failure talking to the daemon, or one the daemon reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientError {
    pub kind: ErrorKind,
    pub message: String,
}

impl ClientError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ClientError {}

/// Asks the user the daemon's question; the answer, or why there's none.
type AskUser<'a> = dyn FnMut(&str) -> Result<bool, ClientError> + 'a;

pub struct DaemonClient {
    framed: Framed<UnixStream, Codec>,
    next_id: u64,
}

impl DaemonClient {
    async fn connect(socket: &Path) -> std::io::Result<Self> {
        let stream = connect_socket(socket).await?;
        Ok(Self {
            framed: Framed::new(stream, Codec::new()),
            next_id: 0,
        })
    }

    /// Send `request` and pass each event the daemon sends for it to
    /// `on_event` until the answer arrives.
    pub async fn request_with_events(
        &mut self,
        request: Request,
        on_event: impl FnMut(Event),
    ) -> Result<ResponseData, ClientError> {
        self.request_within(request, stall_timeout(), on_event)
            .await
    }

    /// [`Self::request_with_events`] for a request that may ask the user
    /// something mid-run (a progress line of kind `Ask`): `on_ask` asks it
    /// and its answer goes back to the daemon. An error from `on_ask` (no
    /// terminal to ask at) ends the request, and the daemon, finding the
    /// client gone, gets no answer.
    pub async fn request_asking(
        &mut self,
        request: Request,
        on_event: impl FnMut(Event),
        mut on_ask: impl FnMut(&str) -> Result<bool, ClientError>,
    ) -> Result<ResponseData, ClientError> {
        let stall = stall_timeout();
        let id = self.send(request, stall).await?;
        into_data(self.reply(id, stall, on_event, Some(&mut on_ask)).await?)
    }

    async fn request_within(
        &mut self,
        request: Request,
        stall: Duration,
        on_event: impl FnMut(Event),
    ) -> Result<ResponseData, ClientError> {
        let id = self.send(request, stall).await?;
        into_data(self.reply(id, stall, on_event, None).await?)
    }

    async fn send(&mut self, request: Request, stall: Duration) -> Result<u64, ClientError> {
        self.next_id += 1;
        let message = Message {
            id: self.next_id,
            payload: Payload::Request(request),
        };
        tokio::time::timeout(stall, self.framed.send(message))
            .await
            .map_err(|_| unavailable("the daemon didn't take the request in time".into()))?
            .map_err(ipc_error)?;
        Ok(self.next_id)
    }

    /// Wait for the answer to request `id`. Each frame for it (an answer
    /// or a progress event) restarts the `stall` clock; other frames don't.
    /// A question (`Ask`) goes to `on_ask`, whose answer is sent back; the
    /// clock restarts once it's answered, however long the user took.
    async fn reply(
        &mut self,
        id: u64,
        stall: Duration,
        mut on_event: impl FnMut(Event),
        mut on_ask: Option<&mut AskUser<'_>>,
    ) -> Result<Response, ClientError> {
        let mut deadline = Instant::now() + stall;
        // An answer too large for one frame, as it arrives in parts.
        let mut parts = Parts::default();
        loop {
            let frame = tokio::time::timeout_at(deadline, self.framed.next())
                .await
                .map_err(|_| {
                    unavailable(format!(
                        "the daemon sent nothing for {} seconds",
                        stall.as_secs_f64().round()
                    ))
                })?
                .ok_or_else(|| mismatch("the daemon closed the connection without answering"))?;
            let message = frame.map_err(ipc_error)?;
            match message.payload {
                Payload::Response(Response::Parted { count, bytes }) if message.id == id => {
                    return parts.finish(count, bytes);
                }
                Payload::Event(Event::Part(part)) if message.id == id => {
                    deadline = Instant::now() + stall;
                    parts.push(part)?;
                }
                // Id 0 is the daemon rejecting a frame it couldn't read.
                Payload::Response(response) if message.id == id || message.id == 0 => {
                    return Ok(response);
                }
                Payload::Event(Event::Progress(progress))
                    if message.id == id && progress.kind == ProgressKind::Ask =>
                {
                    // A daemon only asks a client that sent a request
                    // which may ask; anything else declines.
                    let yes = match on_ask.as_mut() {
                        Some(ask) => ask(&progress.line)?,
                        None => false,
                    };
                    let answer = Message {
                        id,
                        payload: Payload::Request(Request::Answer { yes }),
                    };
                    tokio::time::timeout(stall, self.framed.send(answer))
                        .await
                        .map_err(|_| {
                            unavailable("the daemon didn't take the answer in time".into())
                        })?
                        .map_err(ipc_error)?;
                    deadline = Instant::now() + stall;
                }
                Payload::Event(event) if message.id == id => {
                    deadline = Instant::now() + stall;
                    on_event(event);
                }
                // Anything else, including what a newer daemon adds, isn't
                // progress on this request.
                _ => {}
            }
        }
    }
}

fn into_data(response: Response) -> Result<ResponseData, ClientError> {
    match response {
        Response::Ok { data } => Ok(data),
        Response::Error { error } => Err(ClientError::new(
            ErrorKind::parse(&error.kind).unwrap_or(ErrorKind::Internal),
            error.message,
        )),
        // `Parted` only ever closes the parts `reply` joins.
        Response::Parted { .. } | Response::Unknown => Err(mismatch(
            "the daemon sent an answer this version can't read",
        )),
    }
}

/// [`STALL_TIMEOUT`], or in debug builds `CHATGPT_REQUEST_TIMEOUT_MS`, so
/// tests can reach it without waiting five minutes.
fn stall_timeout() -> Duration {
    if cfg!(debug_assertions)
        && let Some(millis) = std::env::var(STALL_TIMEOUT_ENV)
            .ok()
            .and_then(|value| value.parse().ok())
    {
        return Duration::from_millis(millis);
    }
    STALL_TIMEOUT
}

/// A daemon that's ready for requests, and the status it reported: started
/// now if it wasn't running, and restarted if it's older or speaks another
/// protocol.
pub async fn connect(paths: &Paths) -> Result<(DaemonClient, DaemonStatus), ClientError> {
    match probe(paths).await {
        Probe::Ready(client, status) => return Ok((*client, *status)),
        Probe::Incompatible { why, pid } => {
            eprintln!("Restarting the chatgpt daemon: {why}.");
            // Only the daemon found incompatible: another client may already
            // have replaced it with one this client can use.
            stop_daemon(paths, pid).await?;
        }
        Probe::Unreachable => {}
    }
    start(paths).await
}

/// [`ask`] for a request that may ask the user something mid-run: see
/// [`DaemonClient::request_asking`].
pub async fn ask_asking(
    paths: &Paths,
    request: Request,
    on_event: impl FnMut(Event),
    on_ask: impl FnMut(&str) -> Result<bool, ClientError>,
) -> Result<ResponseData, ClientError> {
    let (mut client, _) = connect(paths).await?;
    client.request_asking(request, on_event, on_ask).await
}

/// Send one request, starting the daemon first if needed, and pass the
/// events the daemon sends for it to `on_event`.
pub async fn ask(
    paths: &Paths,
    request: Request,
    on_event: impl FnMut(Event),
) -> Result<ResponseData, ClientError> {
    let (mut client, _) = connect(paths).await?;
    client.request_with_events(request, on_event).await
}

/// The running daemon's status, without starting one.
pub async fn inspect(paths: &Paths) -> Inspection {
    match probe(paths).await {
        Probe::Ready(_, status) => Inspection::Ready(status),
        Probe::Incompatible { why, .. } => Inspection::Unhealthy {
            pid: read_pid_file(paths).map(|(pid, _)| pid),
            why,
        },
        Probe::Unreachable if daemon_lock_held(paths) => Inspection::Unhealthy {
            pid: read_pid_file(paths).map(|(pid, _)| pid),
            why: "it's running but not answering on its socket".into(),
        },
        Probe::Unreachable => Inspection::Stopped,
    }
}

pub enum Inspection {
    Ready(Box<DaemonStatus>),
    Unhealthy { pid: Option<u32>, why: String },
    Stopped,
}

/// Stop the daemon. Done only when the socket is unreachable and the
/// daemon's PID has exited (spotuify's rule). Returns the PID it stopped.
pub async fn stop(paths: &Paths) -> Result<Option<u32>, ClientError> {
    stop_daemon(paths, None).await
}

/// Compare-and-stop: [`stop`], but only the daemon with PID `seen`, the
/// one this client observed. A daemon another client started since is
/// left running (`Ok(None)`), so two clients restarting the same old
/// daemon never stop each other's new one.
pub async fn stop_if_still(paths: &Paths, seen: u32) -> Result<Option<u32>, ClientError> {
    stop_daemon(paths, Some(seen)).await
}

async fn stop_daemon(paths: &Paths, seen: Option<u32>) -> Result<Option<u32>, ClientError> {
    let another = |pid: u32| seen.is_some_and(|seen| seen != pid);
    let pid = match probe(paths).await {
        Probe::Ready(mut client, status) => {
            if another(status.pid) {
                return Ok(None);
            }
            // Asked on the connection that just named its PID, so it's that
            // daemon that stops. It answers, then exits; a lost answer is
            // fine, since the wait below is what counts.
            let _ = client
                .request_within(Request::Shutdown, QUICK_TIMEOUT, |_| {})
                .await;
            status.pid
        }
        Probe::Incompatible { pid: answered, .. } => {
            match signal_lock_holder(paths, answered.or(seen))? {
                Some(pid) => pid,
                None => return Ok(None),
            }
        }
        Probe::Unreachable => match signal_lock_holder(paths, seen)? {
            Some(pid) => pid,
            None => return Ok(None),
        },
    };
    if wait_until_gone(paths, pid, EXIT_TIMEOUT).await {
        return Ok(Some(pid));
    }
    verify_daemon_pid(paths, pid)?;
    terminate(pid)?;
    if wait_until_gone(paths, pid, EXIT_TIMEOUT).await {
        return Ok(Some(pid));
    }
    Err(unavailable(format!(
        "the daemon (pid {pid}) didn't exit; check {}",
        paths.log_dir().display()
    )))
}

/// SIGTERM the daemon holding the lock, one that can't be asked to stop:
/// only if its PID file names `seen` (when given) and the PID's start time
/// still matches it. `None` when no daemon holds the lock, or another one
/// than `seen` does now.
fn signal_lock_holder(paths: &Paths, seen: Option<u32>) -> Result<Option<u32>, ClientError> {
    if !daemon_lock_held(paths) {
        return Ok(None);
    }
    // The PID is safe to signal: the daemon wrote it under the lock it
    // still holds, and its start time is checked, so it can't be reused.
    let (pid, _) = read_pid_file(paths).ok_or_else(|| {
        unavailable(format!(
            "a daemon holds {} but wrote no PID to {}",
            paths.daemon_lock_file().display(),
            paths.pid_file().display()
        ))
    })?;
    if seen.is_some_and(|seen| seen != pid) {
        return Ok(None);
    }
    verify_daemon_pid(paths, pid)?;
    terminate(pid)?;
    Ok(Some(pid))
}

enum Probe {
    Ready(Box<DaemonClient>, Box<DaemonStatus>),
    /// Something answers, but not in a way this build can use. `pid`: the
    /// one it reported, else the one its PID file named.
    Incompatible { why: String, pid: Option<u32> },
    Unreachable,
}

async fn probe(paths: &Paths) -> Probe {
    let recorded = || read_pid_file(paths).map(|(pid, _)| pid);
    let mut client = match DaemonClient::connect(&paths.socket_path()).await {
        Ok(client) => client,
        // Something holds the socket but never accepts: not missing.
        Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {
            return Probe::Incompatible {
                why: error.to_string(),
                pid: recorded(),
            };
        }
        // Missing, or a stale file nobody listens on. A new daemon removes
        // a stale socket itself, under its lock.
        Err(_) => return Probe::Unreachable,
    };
    match client
        .request_within(Request::Status, QUICK_TIMEOUT, |_| {})
        .await
    {
        Ok(ResponseData::Status(status)) => {
            match incompatibility(&status, env!("CARGO_PKG_VERSION")) {
                None => Probe::Ready(Box::new(client), status),
                Some(why) => Probe::Incompatible {
                    why,
                    pid: Some(status.pid),
                },
            }
        }
        Ok(_) => Probe::Incompatible {
            why: "its status answer isn't one this version can read".into(),
            pid: recorded(),
        },
        Err(error) => Probe::Incompatible {
            why: error.message,
            pid: recorded(),
        },
    }
}

/// spotuify's rule: a daemon on this protocol that is at least as new as
/// this client is kept. Restarting a newer one would only start this older
/// binary in its place (or, after an upgrade, the same newer one again, which
/// in spotuify bounced the daemon every few seconds). An older daemon is
/// restarted, which starts this binary.
fn incompatibility(status: &DaemonStatus, ours: &str) -> Option<String> {
    if status.protocol_version != PROTOCOL_VERSION {
        Some(format!(
            "it speaks protocol {}, this chatgpt speaks {PROTOCOL_VERSION}",
            status.protocol_version
        ))
    } else if !version_at_least(&status.version, ours) {
        Some(format!(
            "it's version {}, older than this chatgpt ({ours})",
            status.version
        ))
    } else {
        None
    }
}

/// True when dotted version `candidate` >= `baseline`. Tolerates a leading
/// `v`; an unparseable part compares as 0, so a malformed daemon version
/// reads as older (restart, the safe direction).
fn version_at_least(candidate: &str, baseline: &str) -> bool {
    fn parts(version: &str) -> Vec<u64> {
        version
            .trim()
            .trim_start_matches('v')
            .split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    }
    let (a, b) = (parts(candidate), parts(baseline));
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return x > y;
        }
    }
    true
}

async fn start(paths: &Paths) -> Result<(DaemonClient, DaemonStatus), ClientError> {
    match spawn_and_wait(paths).await {
        // A daemon that took the lock but never bound its socket (stuck in
        // startup) would turn every client away until someone stopped it.
        // Stop it, PID-checked, and start once more.
        Err(Startup::NotReady(error)) => {
            if stop_stuck(paths).await {
                eprintln!("Restarted a chatgpt daemon that was stuck starting up.");
                spawn_and_wait(paths).await.map_err(Startup::into_error)
            } else {
                Err(error)
            }
        }
        started => started.map_err(Startup::into_error),
    }
}

/// Why a daemon this client launched isn't answering.
enum Startup {
    /// It wasn't ready in time.
    NotReady(ClientError),
    Failed(ClientError),
}

impl Startup {
    fn into_error(self) -> ClientError {
        match self {
            Self::NotReady(error) | Self::Failed(error) => error,
        }
    }
}

async fn spawn_and_wait(paths: &Paths) -> Result<(DaemonClient, DaemonStatus), Startup> {
    let mut launcher = spawn(paths).map_err(Startup::Failed)?;
    let started = wait_until_ready(paths, &mut launcher).await;
    // The launcher is the daemon's parent only while it relays an early
    // exit. Ending it hands the daemon to launchd or init, which reap it, so
    // this client never parents the daemon and can't leave it a zombie.
    let _ = launcher.kill();
    let _ = launcher.wait();
    started
}

/// Stop the daemon holding the lock without a socket that answers, if its
/// PID checks out. Whether it's gone.
async fn stop_stuck(paths: &Paths) -> bool {
    if matches!(probe(paths).await, Probe::Ready(..)) {
        return false;
    }
    match signal_lock_holder(paths, None) {
        Ok(Some(pid)) => {
            let deadline = Instant::now() + EXIT_TIMEOUT;
            while pid_alive(pid) {
                if Instant::now() >= deadline {
                    return false;
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            true
        }
        Ok(None) | Err(_) => false,
    }
}

fn ready_timeout() -> Duration {
    if cfg!(debug_assertions)
        && let Some(millis) = std::env::var(READY_TIMEOUT_ENV)
            .ok()
            .and_then(|value| value.parse().ok())
    {
        return Duration::from_millis(millis);
    }
    READY_TIMEOUT
}

async fn wait_until_ready(
    paths: &Paths,
    launcher: &mut Child,
) -> Result<(DaemonClient, DaemonStatus), Startup> {
    let deadline = Instant::now() + ready_timeout();
    loop {
        match probe(paths).await {
            Probe::Ready(client, status) => return Ok((*client, *status)),
            Probe::Incompatible { why, .. } => {
                return Err(Startup::Failed(unavailable(format!(
                    "another chatgpt daemon answered on {}: {why}; stop it with `chatgpt daemon stop`",
                    paths.socket_path().display()
                ))));
            }
            Probe::Unreachable => {}
        }
        // The launcher exits with the daemon's status. Exit 0 means another
        // daemon already held the lock (a racing client started it), so
        // keep waiting for that one.
        if let Ok(Some(exit)) = launcher.try_wait()
            && !exit.success()
        {
            if exit.code() == Some(i32::from(EXIT_DATABASE_TOO_NEW)) {
                return Err(Startup::Failed(ClientError::new(
                    ErrorKind::DatabaseTooNew,
                    format!(
                        "this index was upgraded by a newer chatgpt; install the latest version ({})",
                        paths.database_file().display()
                    ),
                )));
            }
            return Err(Startup::Failed(unavailable(format!(
                "the daemon exited during startup ({exit}){}",
                log_tail(&paths.daemon_stderr_file())
            ))));
        }
        if Instant::now() >= deadline {
            return Err(Startup::NotReady(unavailable(format!(
                "the daemon wasn't ready after {} seconds{}",
                ready_timeout().as_secs_f64().round(),
                log_tail(&paths.daemon_stderr_file())
            ))));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Spawn `chatgpt daemon launch`, which starts the daemon detached from
/// this client and relays its exit if it dies during startup.
fn spawn(paths: &Paths) -> Result<Child, ClientError> {
    let exe = std::env::current_exe()
        .map_err(|error| unavailable(format!("cannot find this executable: {error}")))?;
    let log = open_log(&paths.daemon_stderr_file())
        .map_err(|error| unavailable(format!("cannot open the daemon's log: {error}")))?;
    std::process::Command::new(exe)
        .args(["daemon", "launch", "--instance", paths.instance.label()])
        // Don't hold the caller's directory open for the daemon's lifetime.
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .spawn()
        .map_err(|error| unavailable(format!("cannot start the daemon: {error}")))
}

/// `chatgpt daemon launch`: the middle of a double fork, without `unsafe`
/// (the workspace forbids it, so no `pre_exec`). It leaves the client's
/// session, so the daemon outlives the terminal's SIGHUP, then runs
/// `chatgpt daemon run` and exits with its status if it dies during
/// startup. The client ends this process once the daemon is ready, so the
/// daemon's parent becomes launchd, never a client. A launcher whose client
/// died gives up after [`LAUNCH_TIMEOUT`].
pub fn launch(paths: &Paths) -> ExitCode {
    if let Err(error) = setsid() {
        eprintln!("chatgpt daemon launch: cannot start a new session: {error}");
        return ExitCode::FAILURE;
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("chatgpt daemon launch: cannot find this executable: {error}");
            return ExitCode::FAILURE;
        }
    };
    // Stdio and the working directory are inherited from the client's spawn.
    let mut daemon = match std::process::Command::new(exe)
        .args(["daemon", "run", "--instance", paths.instance.label()])
        .spawn()
    {
        Ok(daemon) => daemon,
        Err(error) => {
            eprintln!("chatgpt daemon launch: cannot start the daemon: {error}");
            return ExitCode::FAILURE;
        }
    };
    let deadline = std::time::Instant::now() + LAUNCH_TIMEOUT;
    while std::time::Instant::now() < deadline {
        match daemon.try_wait() {
            Ok(Some(exit)) => {
                // A daemon killed by a signal has no code; any failure will do.
                return exit
                    .code()
                    .and_then(|code| u8::try_from(code).ok())
                    .map_or(ExitCode::FAILURE, ExitCode::from);
            }
            Ok(None) => std::thread::sleep(POLL_INTERVAL),
            Err(error) => {
                eprintln!("chatgpt daemon launch: cannot watch the daemon: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}

pub fn open_log(path: &Path) -> std::io::Result<File> {
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
}

fn log_tail(path: &Path) -> String {
    let Ok(mut file) = File::open(path) else {
        return String::new();
    };
    // The file only grows; its last few KiB hold the last lines.
    let length = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(length.saturating_sub(4096)));
    let mut text = String::new();
    let _ = file.read_to_string(&mut text);
    let lines: Vec<&str> = text.lines().rev().take(LOG_TAIL_LINES).collect();
    let mut tail = format!(". Log: {}", path.display());
    for line in lines.into_iter().rev() {
        tail.push_str("\n  ");
        tail.push_str(line);
    }
    tail
}

async fn wait_until_gone(paths: &Paths, pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        // A socket that answers again belongs to a daemon another client
        // started since, once the PID file names it.
        let socket_gone = connect_socket(&paths.socket_path()).await.is_err()
            || read_pid_file(paths).is_some_and(|(recorded, _)| recorded != pid);
        if socket_gone && !pid_alive(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Connect to the daemon's socket, giving up after `QUICK_TIMEOUT`. On
/// macOS a connect that races the daemon closing its socket can wait
/// forever (seen in ms-todo's `daemon stop`).
async fn connect_socket(socket: &Path) -> std::io::Result<UnixStream> {
    tokio::time::timeout(QUICK_TIMEOUT, UnixStream::connect(socket))
        .await
        .map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "{} didn't accept a connection within {} seconds",
                    socket.display(),
                    QUICK_TIMEOUT.as_secs()
                ),
            )
        })?
}

fn pid_alive(pid: u32) -> bool {
    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    // EPERM means it exists but isn't ours to signal.
    if matches!(kill(Pid::from_raw(raw), None), Err(Errno::ESRCH)) {
        return false;
    }
    // `kill(pid, 0)` succeeds for a zombie, but a zombie has already exited:
    // there's nothing left to wait for, and only its parent can reap it.
    !process_is_zombie(pid)
}

/// macOS has no procfs, and `proc_pidinfo` needs `unsafe`, which the
/// workspace forbids; `ps` is mxr's probe. A `ps` that can't run says
/// "not a zombie", so the caller keeps waiting as before.
fn process_is_zombie(pid: u32) -> bool {
    chatgpt_core::ps_field(pid, "state").is_some_and(|state| state.starts_with('Z'))
}

fn terminate(pid: u32) -> Result<(), ClientError> {
    let raw = i32::try_from(pid).map_err(|_| unavailable(format!("invalid daemon PID {pid}")))?;
    match kill(Pid::from_raw(raw), Signal::SIGTERM) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(error) => Err(unavailable(format!(
            "cannot stop the daemon (pid {pid}): {error}"
        ))),
    }
}

/// Whether a daemon holds its lock, so is alive, even if it doesn't answer.
fn daemon_lock_held(paths: &Paths) -> bool {
    let Ok(file) = File::open(paths.daemon_lock_file()) else {
        return false;
    };
    match FileExt::try_lock_shared(&file) {
        // Closing the file releases the probe's shared lock at once.
        Ok(()) => false,
        Err(error) => error.kind() == IoErrorKind::WouldBlock,
    }
}

/// The PID file's PID and the start time recorded with it.
fn read_pid_file(paths: &Paths) -> Option<(u32, Option<String>)> {
    chatgpt_core::parse_pid_file(&std::fs::read_to_string(paths.pid_file()).ok()?)
}

/// Make sure `pid` is still the daemon that wrote the PID file: same PID,
/// same start time. A PID that can't be confirmed is never signalled.
fn verify_daemon_pid(paths: &Paths, pid: u32) -> Result<(), ClientError> {
    let confirmed = match read_pid_file(paths) {
        Some((recorded, Some(started))) if recorded == pid => {
            chatgpt_core::process_start_time(pid).as_deref() == Some(started.as_str())
        }
        _ => false,
    };
    if confirmed {
        Ok(())
    } else {
        Err(unavailable(format!(
            "won't signal pid {pid}: it can't be confirmed as the daemon that wrote {} \
             (the PID may have been reused). Stop the daemon yourself if it's still running",
            paths.pid_file().display()
        )))
    }
}

fn unavailable(message: String) -> ClientError {
    ClientError::new(ErrorKind::DaemonUnavailable, message)
}

/// An answer arriving in parts. Each part must be the next one; the end
/// must name exactly the parts and bytes that came. Anything else is
/// refused before the text is read, since a lost slice could still decode
/// (a gap inside a transcript's markdown) and be shown as the chat.
#[derive(Default)]
struct Parts {
    text: String,
    count: u64,
}

impl Parts {
    fn push(&mut self, part: chatgpt_protocol::Part) -> Result<(), ClientError> {
        if part.index != self.count {
            return Err(mismatch(&format!(
                "the daemon's answer came in parts out of order (part {} after {})",
                part.index, self.count
            )));
        }
        self.count += 1;
        self.text.push_str(&part.text);
        Ok(())
    }

    fn finish(self, count: u64, bytes: u64) -> Result<Response, ClientError> {
        let got = u64::try_from(self.text.len()).unwrap_or(u64::MAX);
        if count != self.count || bytes != got {
            return Err(mismatch(&format!(
                "the daemon's answer in parts is incomplete ({} of {count} parts, {got} of {bytes} bytes)",
                self.count
            )));
        }
        serde_json::from_str(&self.text).map_err(|error| {
            mismatch(&format!(
                "the daemon's answer in parts didn't decode ({:?} error)",
                error.classify()
            ))
        })
    }
}

/// The daemon answered a request with another request's kind of answer.
pub fn unexpected() -> ClientError {
    ClientError::new(
        ErrorKind::DaemonUnavailable,
        "the daemon answered with something else; run `chatgpt daemon stop` and try again",
    )
}

fn mismatch(what: &str) -> ClientError {
    unavailable(format!(
        "{what}; it may be another version. Run `chatgpt daemon stop` and try again"
    ))
}

fn ipc_error(error: std::io::Error) -> ClientError {
    // A frame that framed fine but didn't decode means the daemon's
    // messages have a shape this build doesn't know (mxr's hint).
    let undecodable = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<serde_json::Error>())
        .is_some_and(|json| json.classify() == serde_json::error::Category::Data);
    if undecodable {
        mismatch(&format!(
            "the daemon sent a message this version can't read ({error})"
        ))
    } else {
        unavailable(format!("IPC error talking to the daemon: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A transcript answer's JSON, cut into three parts.
    fn parted() -> (Vec<chatgpt_protocol::Part>, u64) {
        let response = Response::Ok {
            data: ResponseData::Exported(Box::new(chatgpt_protocol::ExportedChat {
                markdown: "# T\n\nfirst slice second slice third slice".into(),
                title: "T".into(),
                synced_at: None,
            })),
        };
        let json = serde_json::to_string(&response).expect("encode");
        let third = json.len() / 3;
        let pieces = [&json[..third], &json[third..2 * third], &json[2 * third..]];
        let parts = (0..)
            .zip(pieces)
            .map(|(index, text)| chatgpt_protocol::Part {
                index,
                text: text.to_owned(),
            })
            .collect();
        (parts, json.len() as u64)
    }

    fn join(
        parts: Vec<chatgpt_protocol::Part>,
        count: u64,
        bytes: u64,
    ) -> Result<Response, ClientError> {
        let mut joined = Parts::default();
        for part in parts {
            joined.push(part)?;
        }
        joined.finish(count, bytes)
    }

    #[test]
    fn parts_in_order_join_into_the_answer() {
        let (parts, bytes) = parted();
        let response = join(parts, 3, bytes).expect("joined");
        assert!(matches!(
            response,
            Response::Ok {
                data: ResponseData::Exported(_)
            }
        ));
    }

    #[test]
    fn a_missing_repeated_or_reordered_part_is_refused() {
        let (parts, bytes) = parted();
        // Only the middle part lost: what's left could still decode.
        let gap = vec![
            parts[0].clone(),
            chatgpt_protocol::Part {
                index: 2,
                ..parts[2].clone()
            },
        ];
        assert!(join(gap, 3, bytes).is_err(), "a gap");
        let repeated = vec![parts[0].clone(), parts[0].clone()];
        assert!(join(repeated, 3, bytes).is_err(), "a repeat");
        let swapped = vec![parts[1].clone(), parts[0].clone(), parts[2].clone()];
        assert!(join(swapped, 3, bytes).is_err(), "out of order");
        // Numbered right, but the end names another count or size.
        let renumbered = vec![
            parts[0].clone(),
            chatgpt_protocol::Part {
                index: 1,
                ..parts[2].clone()
            },
        ];
        let error = join(renumbered, 2, bytes).expect_err("short by a slice");
        assert!(error.message.contains("incomplete"), "{}", error.message);
        assert!(join(parts.clone(), 4, bytes).is_err(), "a part never came");
    }

    fn status(protocol_version: u32, version: &str) -> DaemonStatus {
        DaemonStatus {
            protocol_version,
            version: version.into(),
            pid: 1,
            instance: "dev".into(),
            started_at: 0,
            socket: String::new(),
            database: String::new(),
            sync: Default::default(),
            backoff: None,
            session: None,
            session_reading: false,
            classification: Default::default(),
            search_index: Default::default(),
            embeddings: Default::default(),
            auto_jev: Default::default(),
        }
    }

    #[test]
    fn a_newer_daemon_on_this_protocol_is_kept_and_an_older_one_restarted() {
        assert_eq!(
            incompatibility(&status(PROTOCOL_VERSION, "0.2.0"), "0.2.0"),
            None
        );
        assert_eq!(
            incompatibility(&status(PROTOCOL_VERSION, "0.3.1"), "0.2.0"),
            None
        );
        let older = incompatibility(&status(PROTOCOL_VERSION, "0.1.9"), "0.2.0");
        assert!(older.is_some_and(|why| why.contains("0.1.9")));
        let other_protocol = incompatibility(&status(PROTOCOL_VERSION + 1, "9.0.0"), "0.2.0");
        assert!(other_protocol.is_some_and(|why| why.contains("protocol")));
    }

    #[test]
    fn version_at_least_orders_dotted_versions() {
        assert!(version_at_least("0.1.62", "0.1.60"));
        assert!(version_at_least("0.1.60", "0.1.60"));
        assert!(!version_at_least("0.1.60", "0.1.62"));
        assert!(version_at_least("v0.2.0", "0.1.99"));
        assert!(version_at_least("1.0", "0.9.9"));
        assert!(!version_at_least("garbage", "0.1.0"));
    }

    #[test]
    fn only_a_pid_whose_start_time_matches_is_signalled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::under(chatgpt_core::Instance::Named("t".into()), dir.path(), None);
        std::fs::create_dir_all(&paths.run_dir).expect("mkdir");
        let pid = std::process::id();
        let started = chatgpt_core::process_start_time(pid).expect("start time");
        let write = |contents: String| std::fs::write(paths.pid_file(), contents).expect("write");

        write(chatgpt_core::pid_file_contents(pid, &started));
        assert!(verify_daemon_pid(&paths, pid).is_ok());
        // Same PID, another process's start time: a reused PID.
        write(chatgpt_core::pid_file_contents(
            pid,
            "Thu Jan  1 00:00:00 1970",
        ));
        assert!(verify_daemon_pid(&paths, pid).is_err());
        // An old PID file without a start time can't be confirmed.
        write(format!("{pid}\n"));
        assert!(verify_daemon_pid(&paths, pid).is_err());
    }

    #[test]
    fn a_zombie_counts_as_exited() {
        let child = std::process::Command::new("true")
            .spawn()
            .expect("spawn true");
        let pid = child.id();
        std::mem::forget(child);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !process_is_zombie(pid) {
            assert!(
                std::time::Instant::now() < deadline,
                "pid {pid} never became a zombie"
            );
            std::thread::sleep(POLL_INTERVAL);
        }
        assert!(!pid_alive(pid), "zombie pid {pid} counted as alive");
    }
}
