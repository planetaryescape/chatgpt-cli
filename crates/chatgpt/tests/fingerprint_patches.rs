//! impit's Chrome fingerprint depends on apify's forks of these crates. Cargo
//! drops a `[patch.crates-io]` entry with only a warning when the graph wants a
//! newer version (as `cargo update` did with hyper 1.11 pulling h2 0.4.19), and
//! the build still passes. This keeps that from shipping unnoticed.

#![allow(clippy::unwrap_used)]

#[test]
fn the_lockfile_resolves_every_fingerprint_fork() {
    let lock =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock")).unwrap();
    for name in ["h2", "rustls", "hyper-util", "tower-http"] {
        let entry = format!("name = \"{name}\"\n");
        let sources: Vec<&str> = lock
            .split("[[package]]")
            .filter(|package| package.trim_start().starts_with(entry.as_str()))
            .filter_map(|package| package.lines().find(|line| line.starts_with("source = ")))
            .collect();
        assert!(!sources.is_empty(), "{name} is missing from Cargo.lock");
        for source in sources {
            assert!(
                source.contains("git+https://github.com/apify/"),
                "{name} resolves to {source}, not apify's fork. Pin hyper/reqwest back to impit's lockfile versions."
            );
        }
    }
}
