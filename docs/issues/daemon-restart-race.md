# Two clients can restart the same old daemon in turn

**Resolved after 0.2.0.** Stopping is compare-and-stop: a client stops only the daemon whose PID it saw, sending `Shutdown` on the connection that reported that PID or signalling only after the PID file still names it with a matching start time. A daemon another client started meanwhile is kept and used (`a_client_stops_only_the_old_daemon_it_saw_not_one_started_since` in `crates/cli/tests/daemon_cli.rs`).

Recorded on 2026-10-02 from the stage 1 review (`crates/launcher/src/lib.rs`, `connect`).

When two clients both find an older or incompatible daemon, each calls `stop()` and then starts a new one. If client A has already started its compatible daemon when client B's `stop()` runs, B stops A's new daemon, and A's request fails with a daemon-unavailable error. Running the command again works.

Impact: only right after an upgrade, with two commands started at the same moment, and recoverable by retrying.

Fix: compare-and-stop. `stop()` takes the PID and start time it observed as incompatible and stops only that daemon; if the PID file now names another process, it connects to that one instead.
