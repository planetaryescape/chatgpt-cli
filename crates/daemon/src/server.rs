// Skeleton adapted from ms-todo crates/daemon/src/server.rs @ 72a406042a4d46c2f8cc7c3afe6bda12e11f923b
// (itself from mxr's server.rs: the singleton lock before the socket is
// touched, the store opened before the bind, a 0600 socket and a pid file,
// ordered teardown on every exit path, progress events with the request's
// ID). Left out: subscriptions, out-of-order answers, the outbox.

use std::fs::File;
use std::io::ErrorKind as IoErrorKind;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::Arc;

use chatgpt_core::{ErrorKind, Paths};
use chatgpt_protocol::{
    Codec, Event, FrameTooLarge, Message, Payload, Request, Response, ResponseData,
    SOCKET_BUFFER_BYTES,
};
use chatgpt_store::{Store, StoreError};
use fs2::FileExt;
use futures_util::{SinkExt, StreamExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinSet;
use tokio_util::codec::Framed;

use crate::handlers::{error_payload, handle};
use crate::state::State;

/// `sun_path` is 104 bytes on macOS (108 on Linux), including the final NUL.
const MAX_SOCKET_PATH_BYTES: usize = 103;

/// Why the daemon stopped for good.
pub(crate) enum Fatal {
    /// The index has a migration this build doesn't know.
    DatabaseTooNew(String),
    Other(String),
}

impl From<String> for Fatal {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}

pub(crate) async fn serve(paths: Paths) -> Result<(), Fatal> {
    ensure_private_dir(&paths.run_dir).map_err(|error| describe(&paths.run_dir, &error))?;

    // The lock, not the socket, decides who the daemon is. Taking it first
    // means a second daemon started by a racing client exits here, before it
    // can touch the first one's socket.
    let Some(_lock) = take_singleton_lock(&paths.daemon_lock_file())? else {
        tracing::info!("another daemon already runs for this instance; exiting");
        eprintln!("chatgpt daemon: another daemon already runs for this instance; exiting");
        return Ok(());
    };

    // We hold the lock, so any socket file left here belongs to a daemon
    // that died without cleaning up.
    let socket = paths.socket_path();
    match std::fs::remove_file(&socket) {
        Ok(()) => tracing::info!(socket = %socket.display(), "removed a stale socket"),
        Err(error) if error.kind() == IoErrorKind::NotFound => {}
        Err(error) => return Err(describe(&socket, &error).into()),
    }
    if socket.as_os_str().len() > MAX_SOCKET_PATH_BYTES {
        return Err(format!(
            "the socket path {} is longer than the {MAX_SOCKET_PATH_BYTES} bytes Unix sockets allow; \
             use a shorter home directory",
            socket.display()
        )
        .into());
    }

    // The start time lets a client make sure the PID is still this daemon
    // before it signals it.
    let pid = std::process::id();
    let started = chatgpt_core::process_start_time(pid).unwrap_or_default();
    write_private(
        &paths.pid_file(),
        chatgpt_core::pid_file_contents(pid, &started).as_bytes(),
    )
    .map_err(|error| describe(&paths.pid_file(), &error))?;
    // Open the index before binding: a daemon that can't start (an index a
    // newer chatgpt migrated) then never has a socket, and the client that
    // started it reads its exit status instead.
    let state = match open_state(&paths) {
        Ok(state) => Arc::new(state),
        Err(fatal) => {
            let _ = std::fs::remove_file(paths.pid_file());
            return Err(fatal);
        }
    };
    let listener = match UnixListener::bind(&socket) {
        Ok(listener) => listener,
        Err(error) => {
            let _ = std::fs::remove_file(paths.pid_file());
            return Err(describe(&socket, &error).into());
        }
    };
    let served = async {
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| describe(&socket, &error))?;
        tracing::info!(
            version = %state.version,
            pid = std::process::id(),
            socket = %socket.display(),
            "chatgpt daemon listening"
        );
        accept_until_shutdown(listener, state)
            .await
            .map_err(Fatal::Other)
    }
    .await;

    // Every exit after the bind comes through here, so none leaves a socket
    // behind. We still hold the lock, so both files are ours to remove.
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_file(paths.pid_file());
    tracing::info!("chatgpt daemon stopped");
    served
}

fn open_state(paths: &Paths) -> Result<State, Fatal> {
    // The index holds chat titles and Jev's notes: keep its directory private.
    ensure_private_dir(&paths.data_dir).map_err(|error| describe(&paths.data_dir, &error))?;
    let database = paths.database_file();
    match Store::open(&database) {
        Ok(store) => Ok(State::new(paths.clone(), store)),
        Err(error @ StoreError::NewerDatabase { .. }) => Err(Fatal::DatabaseTooNew(format!(
            "{}: {error}",
            database.display()
        ))),
        Err(error) => Err(format!("{}: {error}", database.display()).into()),
    }
}

async fn accept_until_shutdown(listener: UnixListener, state: Arc<State>) -> Result<(), String> {
    let shutdown = Arc::new(Notify::new());
    let mut terminate = signal(SignalKind::terminate()).map_err(|error| error.to_string())?;
    let mut interrupt = signal(SignalKind::interrupt()).map_err(|error| error.to_string())?;
    // Dropping the set when we return aborts open connections and the
    // background work with them.
    let mut tasks = JoinSet::new();
    // Chunk what the cache already holds (also when there is nothing to
    // import); fetching waits for a pass. Asked for before the first client
    // is answered, so `daemon status` shows indexing as pending from the
    // start.
    state.indexer.wake();
    // Chunks from an earlier run (or an index from before vectors) may
    // still need vectors.
    state.embedder.wake();
    let background = Arc::clone(&state);
    tasks.spawn(async move {
        // The TS CLI's judgments and titles show from the first `list`.
        if let Err(failure) = crate::ts_sync::import(&background).await {
            tracing::info!("no import at startup: {}", failure.message);
        }
        crate::sync::run_scheduled(background).await;
    });
    tasks.spawn(crate::search::indexer::run(Arc::clone(&state)));
    tasks.spawn(crate::search::embedder::run(Arc::clone(&state)));
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    tasks.spawn(serve_connection(stream, Arc::clone(&state), Arc::clone(&shutdown)));
                }
                // One failed accept (out of file descriptors) mustn't take
                // the daemon down.
                Err(error) => tracing::warn!("accept failed: {error}"),
            },
            () = shutdown.notified() => break,
            _ = terminate.recv() => break,
            _ = interrupt.recv() => break,
            // Reap finished connections so the set doesn't grow forever.
            Some(_) = tasks.join_next(), if tasks.len() > 1 => {}
        }
    }
    tracing::info!("shutting down");
    Ok(())
}

async fn serve_connection(stream: UnixStream, state: Arc<State>, shutdown: Arc<Notify>) {
    // See SOCKET_BUFFER_BYTES. Best effort: a small buffer is only slower.
    let _ = socket2::SockRef::from(&stream).set_send_buffer_size(SOCKET_BUFFER_BYTES);
    let mut framed = Framed::new(stream, codec());
    while let Some(frame) = framed.next().await {
        let message = match frame {
            Ok(message) => message,
            Err(error) => {
                // Framing can't recover after a bad frame; say why, then close.
                let error = error_payload(
                    ErrorKind::InvalidInput,
                    format!("unreadable IPC frame: {error}"),
                );
                let _ = send(&mut framed, 0, Response::Error { error }).await;
                return;
            }
        };
        let Payload::Request(request) = message.payload else {
            // Clients only send requests; ignore anything else.
            continue;
        };
        let stopping = request == Request::Shutdown;
        if request != Request::Status && !stopping {
            state.syncer.client_active();
        }
        // Progress of a sync goes back as events with the request's ID,
        // each resetting the client's stall deadline.
        let (progress, mut updates) = mpsc::unbounded_channel();
        let work = handle(&state, request, Some(progress));
        tokio::pin!(work);
        let mut heartbeat = tokio::time::interval(crate::progress::HEARTBEAT);
        heartbeat.tick().await;
        let response = loop {
            tokio::select! {
                response = &mut work => break response,
                _ = heartbeat.tick() => {
                    let beat = Message { id: message.id, payload: Payload::Event(Event::Heartbeat) };
                    if framed.send(beat).await.is_err() {
                        return;
                    }
                }
                Some(update) = updates.recv() => {
                    let event = Message {
                        id: message.id,
                        payload: Payload::Event(Event::Progress(update)),
                    };
                    if framed.send(event).await.is_err() {
                        // The client went away; nobody to answer.
                        return;
                    }
                }
            }
        };
        // Lines sent just before the answer still go first.
        while let Ok(update) = updates.try_recv() {
            let event = Message {
                id: message.id,
                payload: Payload::Event(Event::Progress(update)),
            };
            if framed.send(event).await.is_err() {
                return;
            }
        }
        if send(&mut framed, message.id, response).await.is_err() {
            return;
        }
        if stopping {
            shutdown.notify_one();
            return;
        }
    }
}

/// Lowers the frame cap in debug builds only, so tests can reach it.
const MAX_FRAME_ENV: &str = "CHATGPT_TEST_MAX_FRAME_BYTES";

fn codec() -> Codec {
    let lowered = std::env::var(MAX_FRAME_ENV)
        .ok()
        .filter(|_| cfg!(debug_assertions))
        .and_then(|bytes| bytes.parse().ok());
    lowered.map_or_else(Codec::new, Codec::with_max_frame)
}

async fn send(
    framed: &mut Framed<UnixStream, Codec>,
    id: u64,
    response: Response,
) -> Result<(), std::io::Error> {
    let export = matches!(
        &response,
        Response::Ok {
            data: ResponseData::Exported(_)
        }
    );
    let message = Message {
        id,
        payload: Payload::Response(response),
    };
    match framed.send(message).await {
        Err(error)
            if error
                .get_ref()
                .is_some_and(|inner| inner.is::<FrameTooLarge>()) =>
        {
            // An export too large to send goes to the TS CLI instead; any
            // other answer says why rather than leaving the client waiting.
            let response = if export {
                Response::Ok {
                    data: ResponseData::ExportTooLarge,
                }
            } else {
                Response::Error {
                    error: error_payload(
                        ErrorKind::Internal,
                        format!("the response was too large to send: {error}"),
                    ),
                }
            };
            framed
                .send(Message {
                    id,
                    payload: Payload::Response(response),
                })
                .await
        }
        other => other,
    }
}

/// `Ok(None)` when another daemon holds the lock.
fn take_singleton_lock(path: &Path) -> Result<Option<File>, String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .map_err(|error| describe(path, &error))?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(file)),
        Err(error) if error.kind() == IoErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(describe(path, &error)),
    }
}

/// Create `dir` if needed and make it 0700 either way.
fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// Write `bytes` to `path` as a 0600 file, replacing it atomically.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let temporary = path.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)
}

fn describe(path: &Path, error: &std::io::Error) -> String {
    format!("{}: {error}", path.display())
}
