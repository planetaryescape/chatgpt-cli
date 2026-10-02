// Adapted from ms-todo tests/workspace_boundaries.rs @ 72a406042a4d46c2f8cc7c3afe6bda12e11f923b
// (itself from mxr's). Changes: chatgpt's crates and rules; the binary
// lives in crates/cli, so its main.rs is the one file there that may name
// the daemon.
//
// Dependency direction: `core` stays free of I/O crates; `protocol` stands
// alone; the CLI reaches the index and chatgpt.com only through the daemon,
// over IPC. Only the daemon uses the store and the HTTP client.

#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn dependencies(manifest: &str) -> Vec<String> {
    let raw = std::fs::read_to_string(root().join(manifest)).unwrap();
    let manifest: toml::Table = toml::from_str(&raw).unwrap();
    manifest
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .map(|table| table.keys().cloned().collect())
        .unwrap_or_default()
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(rust_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    files
}

const IO_DEPENDENCIES: &[&str] = &[
    "tokio",
    "tokio-util",
    "futures-util",
    "nix",
    "reqwest",
    "hyper",
    "rusqlite",
    "fs2",
    "impit",
];

#[test]
fn core_has_no_io_dependencies() {
    for dependency in dependencies("crates/core/Cargo.toml") {
        assert!(
            !IO_DEPENDENCIES.contains(&dependency.as_str()) && !dependency.starts_with("chatgpt"),
            "crates/core must stay free of I/O, but depends on {dependency}"
        );
    }
}

#[test]
fn the_protocol_stands_alone() {
    for dependency in dependencies("crates/protocol/Cargo.toml") {
        assert!(
            !dependency.starts_with("chatgpt"),
            "crates/protocol must not depend on {dependency}"
        );
    }
}

#[test]
fn the_cli_never_touches_the_index_or_chatgpt_com() {
    let cli = dependencies("crates/cli/Cargo.toml");
    for forbidden in [
        "chatgpt",
        "chatgpt-store",
        "rusqlite",
        "impit",
        "reqwest",
        "typesafe-client",
    ] {
        assert!(
            !cli.iter().any(|dependency| dependency == forbidden),
            "crates/cli must reach {forbidden} through the daemon, not depend on it"
        );
    }
    let launcher = dependencies("crates/launcher/Cargo.toml");
    assert!(
        !launcher
            .iter()
            .any(|dependency| ["chatgpt", "chatgpt-store", "chatgpt-daemon"]
                .contains(&dependency.as_str())),
        "the launcher talks to the daemon over IPC only: {launcher:?}"
    );
    // The binary runs the daemon (`chatgpt daemon run`), so the package
    // depends on it, but only main.rs may name it.
    let mut offenders = Vec::new();
    for file in rust_files(&root().join("crates/cli/src")) {
        if file.file_name().is_some_and(|name| name == "main.rs") {
            continue;
        }
        let source = std::fs::read_to_string(&file).unwrap();
        for name in [
            "chatgpt_daemon",
            "chatgpt_store",
            "chatgpt::",
            "rusqlite",
            "typesafe_client",
        ] {
            if source.contains(name) {
                offenders.push(format!("{}: {name}", file.display()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "the CLI goes through IPC: {offenders:?}"
    );
}

#[test]
fn only_the_daemon_uses_the_store_and_the_http_client() {
    for manifest in [
        "crates/core/Cargo.toml",
        "crates/protocol/Cargo.toml",
        "crates/launcher/Cargo.toml",
        "crates/cli/Cargo.toml",
    ] {
        let found = dependencies(manifest);
        assert!(
            !found.iter().any(
                |name| ["chatgpt-store", "chatgpt", "typesafe-client"].contains(&name.as_str())
            ),
            "{manifest} must not depend on the store or the HTTP client: {found:?}"
        );
    }
    // The store knows neither chatgpt.com nor the wire.
    for dependency in dependencies("crates/store/Cargo.toml") {
        assert!(
            !dependency.starts_with("chatgpt"),
            "crates/store must not depend on {dependency}"
        );
    }
}

#[test]
fn every_crate_stays_unpublished() {
    for manifest in [
        "crates/chatgpt/Cargo.toml",
        "crates/core/Cargo.toml",
        "crates/protocol/Cargo.toml",
        "crates/launcher/Cargo.toml",
        "crates/store/Cargo.toml",
        "crates/daemon/Cargo.toml",
        "crates/cli/Cargo.toml",
        "crates/fake-chatgpt/Cargo.toml",
        "crates/typesafe/Cargo.toml",
    ] {
        let raw = std::fs::read_to_string(root().join(manifest)).unwrap();
        let manifest_table: toml::Table = toml::from_str(&raw).unwrap();
        let publish = manifest_table
            .get("package")
            .and_then(|package| package.get("publish"))
            .and_then(toml::Value::as_bool);
        assert_eq!(
            publish,
            Some(false),
            "{manifest} ships through GitHub Releases"
        );
    }
}
