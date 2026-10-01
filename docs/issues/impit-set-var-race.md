# impit sets an environment variable on every client build

Observed on 2026-10-01 while porting the HTTP client to Rust (impit rev `152e7db`, tag `js-0.14.5`).

`Impit::new` calls `std::env::set_var("IMPIT_H2_PSEUDOHEADERS_ORDER", …)` each time a client is built (`impit/src/impit.rs:343` at that rev). Apify's h2 fork reads the variable to order HTTP/2 pseudo-headers. `set_var` is unsound while other threads read the environment: Rust 2024 makes it `unsafe` for that reason, and impit, on edition 2021, calls it without the marker. `crates/chatgpt/src/http.rs` builds a client in `HttpClient::new` and again after every Cloudflare challenge. Inside a multi-threaded tokio runtime, other threads may be reading the environment at the time. Examples are DNS resolution, TLS certificate loading and `std::env::var` anywhere in the process.

The F1 probe is effectively single-threaded at those moments, so this has not caused a failure. The sync daemon will run challenge rebuilds alongside other requests.

## Mitigations

- Stage 1: in `main`, before the tokio runtime starts, set `IMPIT_H2_PSEUDOHEADERS_ORDER` once, to the Chrome 124 fingerprint's order. Every later `set_var` then writes the same value, before any other thread exists to race with that first write.
- Stage 1: rebuild clients one at a time. `HttpClient` already swaps its client behind a lock. Build the replacement while holding that lock, or a dedicated lock, so that two challenges never call `set_var` at once.
- Consider an upstream issue or PR to apify/impit: pass the pseudo-header order through the h2 builder instead of the process environment.
