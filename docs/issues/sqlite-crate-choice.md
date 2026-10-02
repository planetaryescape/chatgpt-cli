# Only one SQLite binding crate can be in the Rust build

**Resolved: decided 2026-10-01 (stage 1), rusqlite 0.40 everywhere.** The store (`crates/store`) uses the same rusqlite as the cookie readers: one SQLite binding, synchronous queries the daemon runs in `spawn_blocking`, and a writer and a reader connection over WAL.

Recorded on 2026-10-01 for the stage 1 store. Stage 1 decides; F1 changes nothing.

- `crates/chatgpt` uses `rusqlite` 0.40 with `bundled`. That version requires `libsqlite3-sys` `^0.38.2`; `Cargo.lock` has 0.38.2.
- `libsqlite3-sys` declares `links = "sqlite3"`. Cargo allows only one crate with a given `links` value in a build, so every SQLite user in the workspace must resolve to the same `libsqlite3-sys`.
- `sqlx-sqlite` 0.9.0, the latest, requires `libsqlite3-sys` `>=0.30.1, <0.38.0`. It can't share a build with `rusqlite` 0.40. ms-todo (the pattern repo) uses sqlx 0.9 and locks `libsqlite3-sys` 0.37.0.
- `rusqlite` 0.39 requires `libsqlite3-sys` `^0.37.0`, which is compatible with sqlx 0.9.

The store therefore either uses `rusqlite` as well, or uses sqlx 0.9 and moves the cookie readers to `rusqlite` 0.39. The cookie readers need only read-only `open_with_flags`, `busy_timeout`, `query_row` and `query_map`.
