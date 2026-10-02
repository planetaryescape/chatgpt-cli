//! scripts/render_homebrew_formula.sh: the release workflow renders the
//! tap's formula from a release's .sha256 files (adapted from ms-todo's
//! tests/homebrew_formula.rs @ 72a406042a4d46c2f8cc7c3afe6bda12e11f923b).

#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

const PLATFORMS: [&str; 2] = ["macos-aarch64", "macos-x86_64"];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn render(checksums: &Path, version: &str, output: &Path) -> std::process::Output {
    Command::new("bash")
        .arg(root().join("scripts/render_homebrew_formula.sh"))
        .arg(version)
        .arg(checksums)
        .arg(output)
        .output()
        .unwrap()
}

/// A .sha256 file as `shasum -a 256` writes it, for each platform.
fn checksums(dir: &Path, version: &str) {
    for (digit, platform) in ["a", "b"].iter().zip(PLATFORMS) {
        let archive = format!("chatgpt-v{version}-{platform}.tar.gz");
        std::fs::write(
            dir.join(format!("{archive}.sha256")),
            format!("{}  {archive}\n", digit.repeat(64)),
        )
        .unwrap();
    }
}

#[test]
fn the_formula_has_each_archives_url_and_checksum() {
    let dir = tempfile::tempdir().unwrap();
    checksums(dir.path(), "1.2.3");
    let output = dir.path().join("Formula/chatgpt.rb");
    let rendered = render(dir.path(), "v1.2.3", &output);
    assert!(rendered.status.success(), "{rendered:?}");
    let formula = std::fs::read_to_string(&output).unwrap();
    assert!(!formula.contains("__"), "a placeholder is left: {formula}");
    let template = std::fs::read_to_string(root().join("packaging/homebrew/chatgpt.rb")).unwrap();
    assert_eq!(
        formula,
        template
            .replace("__VERSION__", "1.2.3")
            .replace("__SHA256_MACOS_AARCH64__", &"a".repeat(64))
            .replace("__SHA256_MACOS_X86_64__", &"b".repeat(64)),
        "only the placeholders change"
    );
    for line in [
        "class Chatgpt < Formula",
        r##"  version "1.2.3""##,
        r##"      url "https://github.com/planetaryescape/chatgpt-cli/releases/download/v#{version}/chatgpt-v#{version}-macos-aarch64.tar.gz""##,
        r##"      url "https://github.com/planetaryescape/chatgpt-cli/releases/download/v#{version}/chatgpt-v#{version}-macos-x86_64.tar.gz""##,
        r##"    bin.install "chatgpt""##,
        r##"    assert_match "chatgpt #{version}", shell_output("#{bin}/chatgpt --version")"##,
    ] {
        assert!(
            formula.lines().any(|have| have == line),
            "no {line}:\n{formula}"
        );
    }
    // Each archive's checksum follows its own url.
    let aarch64 = formula.find("macos-aarch64.tar.gz").unwrap();
    let x86_64 = formula.find("macos-x86_64.tar.gz").unwrap();
    let a = formula.find(&"a".repeat(64)).unwrap();
    let b = formula.find(&"b".repeat(64)).unwrap();
    assert!(aarch64 < a && a < x86_64 && x86_64 < b, "{formula}");
}

#[test]
fn a_missing_or_malformed_checksum_fails_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    checksums(dir.path(), "1.2.3");
    let output = dir.path().join("chatgpt.rb");
    std::fs::remove_file(dir.path().join("chatgpt-v1.2.3-macos-x86_64.tar.gz.sha256")).unwrap();
    let rendered = render(dir.path(), "1.2.3", &output);
    assert!(!rendered.status.success());
    assert!(String::from_utf8_lossy(&rendered.stderr).contains("missing checksum file"));
    assert!(!output.exists());

    checksums(dir.path(), "1.2.3");
    std::fs::write(
        dir.path()
            .join("chatgpt-v1.2.3-macos-aarch64.tar.gz.sha256"),
        "Not Found\n",
    )
    .unwrap();
    let rendered = render(dir.path(), "1.2.3", &output);
    assert!(!rendered.status.success());
    assert!(String::from_utf8_lossy(&rendered.stderr).contains("not a sha256"));
    assert!(!output.exists());
}
