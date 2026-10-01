# impit sets an environment variable on every client build

Observed on 2026-10-01 while porting the HTTP client to Rust (impit rev `152e7db`, tag `js-0.14.5`).

`Impit::new` calls `std::env::set_var("IMPIT_H2_PSEUDOHEADERS_ORDER", …)` each time a client is built (`impit/src/impit.rs:343` at that rev). Apify's h2 fork reads the variable to order HTTP/2 pseudo-headers. `set_var` is unsound while other threads read the environment: Rust 2024 makes it `unsafe` for that reason, and impit, on edition 2021, calls it without the marker. `crates/chatgpt/src/http.rs` builds a client in `HttpClient::new` and again after every Cloudflare challenge. Inside a multi-threaded tokio runtime, other threads may be reading the environment at the time. Examples are DNS resolution, TLS certificate loading and `std::env::var` anywhere in the process.

The F1 probe is effectively single-threaded at those moments, so this has not caused a failure. The sync daemon will run challenge rebuilds alongside other requests.

## Mitigations

Pre-seeding the variable and serialising rebuilds narrows the race but does not remove it, because each build still writes to the environment while other threads may read it (independent review, 2026-10-01). Stage 1 must make sure no environment write happens once other threads exist. Options:

- Pin a patched impit that skips `set_var` when the variable already holds the wanted value, or that passes the order through the h2 builder. Set the variable once in `main`, before the tokio runtime starts. The repo already pins apify forks, so one more `[patch]` is in keeping.
- Pre-build a pool of impit clients in `main`, before the runtime starts. On a challenge, switch to an unused client instead of building a new one. Replenishing the pool still needs a build, so this only delays the problem.
- Upstream: an issue or PR to apify/impit that passes the pseudo-header order through the h2 builder instead of the process environment.
