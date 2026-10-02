//! The daemon's lifecycle through the real binary: auto-start, a stale
//! socket, a lock held by another daemon, stop, and which daemons a client
//! keeps across an upgrade.

#![allow(clippy::unwrap_used)]

mod support;

use std::os::unix::net::UnixListener;

use nix::unistd::{Pid, getsid};
use support::{Env, mode, pid_exists};

#[test]
fn status_starts_a_detached_private_daemon_and_stop_ends_it() {
    let env = Env::new();
    let status = env.status();
    assert_eq!(status["instance"], "dev");
    assert_eq!(status["protocol_version"], 1);
    let pid = status["pid"].as_u64().unwrap();
    assert!(pid_exists(pid));

    let socket = env.socket();
    assert_eq!(mode(&socket), 0o600, "socket");
    assert_eq!(mode(socket.parent().unwrap()), 0o700, "run dir");
    assert_eq!(mode(&env.data_dir()), 0o700, "data dir");
    assert_eq!(mode(&env.data_dir().join("chatgpt.db")), 0o600, "index");

    // It left the client's session, so the terminal's hangup can't end it.
    let daemon = Pid::from_raw(i32::try_from(pid).unwrap());
    assert_ne!(getsid(Some(daemon)).unwrap(), getsid(None).unwrap());

    // A second client finds the same daemon.
    assert_eq!(env.status()["pid"], pid);

    let stopped = env.cmd().args(["daemon", "stop"]).output().unwrap();
    assert!(stopped.status.success());
    assert!(String::from_utf8_lossy(&stopped.stderr).contains(&format!("pid {pid}")));
    assert!(!pid_exists(pid), "the daemon outlived stop");
    assert!(!socket.exists(), "stop left the socket behind");
    let again = env.cmd().args(["daemon", "stop"]).output().unwrap();
    assert!(String::from_utf8_lossy(&again.stderr).contains("wasn't running"));
}

#[test]
fn a_stale_socket_is_replaced_by_a_new_daemon() {
    let env = Env::new();
    let socket = env.socket();
    std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
    // A socket file nobody listens on, as a crashed daemon leaves behind.
    drop(UnixListener::bind(&socket).unwrap());
    let status = env.status();
    assert!(pid_exists(status["pid"].as_u64().unwrap()));
}

#[test]
fn racing_clients_share_one_daemon() {
    let env = Env::new();
    let children: Vec<_> = (0..4)
        .map(|_| {
            env.std_cmd()
                .args(["daemon", "status", "--json"])
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let pids: Vec<u64> = children
        .into_iter()
        .map(|child| {
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success());
            let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            status["pid"].as_u64().unwrap()
        })
        .collect();
    assert!(pids.windows(2).all(|pair| pair[0] == pair[1]), "{pids:?}");
}

#[test]
fn a_second_daemon_on_the_same_instance_exits_at_the_lock() {
    let env = Env::new();
    let first = env.status()["pid"].as_u64().unwrap();
    let second = env.cmd().args(["daemon", "run"]).output().unwrap();
    assert!(second.status.success());
    assert_eq!(env.status()["pid"], first);
}

/// A daemon run directly that reports `version`, as one of another
/// release would. A client only ever starts its own version.
fn daemon_reporting(env: &Env, version: &str) -> std::process::Child {
    let child = env
        .std_cmd()
        .args(["daemon", "run"])
        .env("CHATGPT_DAEMON_VERSION", version)
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::os::unix::net::UnixStream::connect(env.socket()).is_err() {
        assert!(
            std::time::Instant::now() < deadline,
            "the daemon never listened"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    child
}

#[test]
fn an_older_daemon_is_restarted_and_a_newer_one_kept() {
    let env = Env::new();
    let mut old = daemon_reporting(&env, "0.0.1");
    // This client is newer: it replaces the old daemon with its own.
    let current = env.status();
    assert_eq!(current["version"], env!("CARGO_PKG_VERSION"));
    assert_ne!(current["pid"], old.id());
    assert!(
        old.wait().unwrap().success(),
        "the old daemon stopped cleanly"
    );
    env.cmd().args(["daemon", "stop"]).assert().success();

    let mut newer = daemon_reporting(&env, "99.0.0");
    let kept = env.status();
    assert_eq!(kept["version"], "99.0.0");
    assert_eq!(kept["pid"], newer.id(), "a newer daemon is kept");
    env.cmd().args(["daemon", "stop"]).assert().success();
    assert!(newer.wait().unwrap().success());
}

#[test]
fn logs_show_the_daemons_lines() {
    let env = Env::new();
    env.status();
    // The log is written by a background thread: give it a moment.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let logs = env.stdout(&["daemon", "logs", "-n", "200"]);
        if logs.contains("chatgpt daemon listening") {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{logs}");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// A 0.1.5 client keeps a newer daemon, and its `import-legacy` sends a
/// request this daemon no longer has: it gets a clear error at once.
#[test]
fn a_retired_request_from_an_older_client_gets_an_error_not_a_hang() {
    use std::io::{Read, Write};
    let env = Env::new();
    env.status();
    let body = r#"{"id":1,"payload":{"type":"request","method":"import_legacy"}}"#;
    let mut socket = std::os::unix::net::UnixStream::connect(env.socket()).unwrap();
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    socket
        .write_all(&u32::try_from(body.len()).unwrap().to_be_bytes())
        .unwrap();
    socket.write_all(body.as_bytes()).unwrap();
    let mut length = [0u8; 4];
    socket
        .read_exact(&mut length)
        .expect("an answer, not a hang");
    let mut frame = vec![0u8; u32::from_be_bytes(length) as usize];
    socket.read_exact(&mut frame).unwrap();
    let answer: serde_json::Value = serde_json::from_slice(&frame).unwrap();
    assert_eq!(answer["payload"]["status"], "error", "{answer}");
    assert_eq!(
        answer["payload"]["error"]["kind"], "unsupported",
        "{answer}"
    );
}

fn paths(env: &Env) -> chatgpt_core::Paths {
    chatgpt_core::Paths::under(
        chatgpt_core::Instance::Named("dev".into()),
        &env.home.path().join("Library/Application Support"),
        None,
    )
}

fn block_on<T>(work: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(work)
}

/// The identity of a process that isn't the daemon: what a client saw
/// before another client replaced that daemon.
fn someone_else() -> (std::process::Child, chatgpt_launcher::DaemonIdentity) {
    let child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let identity = chatgpt_launcher::DaemonIdentity::of(child.id()).unwrap();
    (child, identity)
}

/// A daemon run directly that stalls before binding its socket, as one
/// stuck opening its index would. Its PID, once its PID file names it.
#[allow(
    clippy::zombie_processes,
    reason = "the caller kills and waits for it; a test that fails here ends the process"
)]
fn stuck_daemon(env: &Env, marker: &str) -> (std::process::Child, u32) {
    let marker = env.home.path().join(marker);
    std::fs::write(&marker, "").unwrap();
    let child = env
        .std_cmd()
        .args(["daemon", "run"])
        .env("CHATGPT_TEST_STALL_STARTUP", &marker)
        .spawn()
        .unwrap();
    let pid_file = env.data_dir().join("run/daemon.pid");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if marker.exists() {
            // Not stalled yet.
        } else if let Some(pid) = std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|text| text.lines().next()?.parse().ok())
        {
            return (child, pid);
        }
        assert!(std::time::Instant::now() < deadline, "it never stalled");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Clients A and B both find the same old daemon. A replaces it first; B's
/// stop, of the daemon it observed, must leave A's new one running.
#[test]
fn a_client_stops_only_the_old_daemon_it_saw_not_one_started_since() {
    let env = Env::new();
    let mut old = daemon_reporting(&env, "0.0.1");
    let seen = chatgpt_launcher::DaemonIdentity::of(old.id()).unwrap();
    let fresh = env.status()["pid"].as_u64().unwrap();
    assert!(old.wait().unwrap().success());

    let stopped = block_on(chatgpt_launcher::stop_if_still(&paths(&env), &seen)).unwrap();
    assert_eq!(stopped, None, "B stopped A's new daemon");
    assert!(pid_exists(fresh));
    assert_eq!(env.status()["pid"], fresh);

    let fresh = u32::try_from(fresh).unwrap();
    let identity = chatgpt_launcher::DaemonIdentity::of(fresh).unwrap();
    let stopped = block_on(chatgpt_launcher::stop_if_still(&paths(&env), &identity)).unwrap();
    assert_eq!(stopped, Some(fresh));
    assert!(!pid_exists(u64::from(fresh)));
}

/// The daemon a client saw was replaced by one that is itself too old (it
/// answers, incompatibly): the client must not stop the replacement.
#[test]
fn a_replacement_that_answers_incompatibly_is_not_the_one_stopped() {
    let env = Env::new();
    let (mut other, seen) = someone_else();
    let mut replacement = daemon_reporting(&env, "0.0.1");
    let stopped = block_on(chatgpt_launcher::stop_if_still(&paths(&env), &seen)).unwrap();
    assert_eq!(stopped, None);
    assert!(
        pid_exists(u64::from(replacement.id())),
        "the replacement was stopped"
    );
    env.cmd().args(["daemon", "stop"]).assert().success();
    assert!(replacement.wait().unwrap().success());
    let _ = other.kill();
}

/// The same when the replacement holds the lock but doesn't answer: one
/// whose socket times out, and one with no socket yet.
#[test]
fn a_replacement_that_times_out_or_isnt_listening_is_not_the_one_stopped() {
    let env = Env::new();
    let (mut other, seen) = someone_else();
    let (mut stuck, pid) = stuck_daemon(&env, "stall");

    // No socket yet.
    let stopped = block_on(chatgpt_launcher::stop_if_still(&paths(&env), &seen)).unwrap();
    assert_eq!(stopped, None);
    assert!(pid_exists(u64::from(pid)), "the stuck daemon was signalled");

    // A socket that takes connections but never answers: the probe times out.
    let _silent = UnixListener::bind(env.socket()).unwrap();
    let stopped = block_on(chatgpt_launcher::stop_if_still(&paths(&env), &seen)).unwrap();
    assert_eq!(stopped, None);
    assert!(pid_exists(u64::from(pid)), "the stuck daemon was signalled");

    let _ = stuck.kill();
    let _ = stuck.wait();
    let _ = other.kill();
}

/// A client waiting for its daemon first sees a stuck one holding the
/// lock. That one goes away and another stuck one takes its place before
/// the timeout: the client must not stop the newcomer.
#[test]
fn a_timeout_stops_only_the_daemon_first_seen_stuck() {
    let mut env = Env::new();
    env.extra_env.push((
        "CHATGPT_TEST_READY_TIMEOUT_MS".to_owned(),
        "4000".to_owned(),
    ));
    let (mut first, _) = stuck_daemon(&env, "stall-first");
    let client = env
        .std_cmd()
        .args(["daemon", "status", "--json"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1000));
    first.kill().unwrap();
    first.wait().unwrap();
    let (mut second, pid) = stuck_daemon(&env, "stall-second");
    let output = client.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("wasn't ready"), "{stderr}");
    assert!(pid_exists(u64::from(pid)), "the newcomer was signalled");
    let _ = second.kill();
    let _ = second.wait();
}

#[test]
fn a_daemon_stuck_starting_up_is_stopped_and_started_again() {
    let mut env = Env::new();
    let marker = env.home.path().join("stall-once");
    std::fs::write(&marker, "").unwrap();
    env.extra_env.extend([
        (
            "CHATGPT_TEST_STALL_STARTUP".to_owned(),
            marker.display().to_string(),
        ),
        (
            "CHATGPT_TEST_READY_TIMEOUT_MS".to_owned(),
            "2000".to_owned(),
        ),
    ]);
    let output = env
        .cmd()
        .args(["daemon", "status", "--json"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains("stuck starting up"), "{stderr}");
    assert!(!marker.exists(), "the first daemon stalled");
}
