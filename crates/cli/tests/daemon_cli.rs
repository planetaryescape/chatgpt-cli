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
