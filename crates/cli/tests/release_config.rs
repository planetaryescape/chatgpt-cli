//! release-please bumps each workspace crate's version in Cargo.lock by name.
//! A crate missing from that list leaves the lockfile at the old version, and
//! a vendored crate on the list gets bumped past its own manifest. Either way
//! the release build's `cargo build --locked` fails, after the tag is created.
//! Release PRs are opened with GITHUB_TOKEN, so CI never runs on them, which
//! is why this check runs on every PR instead.
//!
//! release-please parses TOML into `{start, end, value}` objects, so a filter
//! must compare `@.name.value`. A bare `@.name` never equals a string, which
//! once made `@.name != 'impit'` match the vendored impit as well.

use std::collections::BTreeSet;
use std::path::Path;

#[test]
fn release_please_bumps_exactly_the_workspace_crates_in_the_lockfile() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let config: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("release-please-config.json")).expect("config"),
    )
    .expect("config is JSON");
    let bumped: BTreeSet<String> = config["packages"]["."]["extra-files"]
        .as_array()
        .expect("extra-files")
        .iter()
        .filter(|file| file["path"] == "Cargo.lock")
        .map(|file| {
            let path = file["jsonpath"].as_str().expect("jsonpath");
            path.strip_prefix("$.package[?(@.name.value=='")
                .and_then(|rest| rest.strip_suffix("')].version"))
                .expect("each Cargo.lock jsonpath names one crate by @.name.value")
                .to_owned()
        })
        .collect();

    let manifest: toml::Table = std::fs::read_to_string(root.join("Cargo.toml"))
        .expect("manifest")
        .parse()
        .expect("manifest is TOML");
    let members: BTreeSet<String> = manifest["workspace"]["members"]
        .as_array()
        .expect("members")
        .iter()
        .map(|member| {
            let crate_manifest: toml::Table = std::fs::read_to_string(
                root.join(member.as_str().expect("member"))
                    .join("Cargo.toml"),
            )
            .expect("crate manifest")
            .parse()
            .expect("crate manifest is TOML");
            crate_manifest["package"]["name"]
                .as_str()
                .expect("name")
                .to_owned()
        })
        .collect();

    assert_eq!(bumped, members);
}

#[test]
fn linux_ci_tests_the_workspace_before_uploading_the_linked_candidate() {
    let workflow = include_str!("../../../.github/workflows/ci.yml");
    let (macos, linux) = workflow.split_once("\n  rust-linux:\n").expect("Linux job");
    assert!(macos.contains("runs-on: macos-latest"));
    assert!(!macos.contains("upload-artifact@"));
    for required in [
        "runs-on: ubuntu-22.04",
        "test \"$(uname -m)\" = x86_64",
        "test \"$(getconf GNU_LIBC_VERSION)\" = 'glibc 2.35'",
        "run: cargo clippy --all-targets --locked -- -D warnings",
        "run: cargo build --locked --bin chatgpt",
        "target/debug/chatgpt --version",
        "run: cargo nextest run --locked\n",
        "run: python3 -m unittest discover -s tests -p test_install.py -v",
        "uses: actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9",
        "name: linux-x86_64-candidate",
        "path: target/debug/chatgpt",
        "retention-days: 1\n",
        "if-no-files-found: error",
    ] {
        assert!(linux.contains(required), "Linux CI must contain {required}");
    }
    let upload = linux
        .find("uses: actions/upload-artifact@")
        .expect("upload");
    let tests = linux
        .find("run: cargo nextest run --locked")
        .expect("tests");
    let installer = linux.find("run: python3 -m unittest").expect("installer");
    assert!(tests < upload && installer < upload);
    assert!(
        !linux.contains("if:"),
        "candidate requires successful preceding steps"
    );
    assert!(!linux.contains("continue-on-error:"));
}

#[test]
fn releases_include_all_three_targets_and_publish_only_after_successful_builds() {
    let workflow = include_str!("../../../.github/workflows/release-please.yml");
    for entry in [
        "- os: macos-15\n            target: aarch64-apple-darwin\n            platform: macos\n            arch: aarch64",
        "- os: macos-15-intel\n            target: x86_64-apple-darwin\n            platform: macos\n            arch: x86_64",
        "- os: ubuntu-22.04\n            target: x86_64-unknown-linux-gnu\n            platform: linux\n            arch: x86_64",
        "run: cargo build --locked --release --target \"$TARGET\" --bin chatgpt",
        "test \"$(getconf GNU_LIBC_VERSION)\" = 'glibc 2.35'",
        "readelf --version-info \"target/${TARGET}/release/chatgpt\"",
        "ARCHIVE=\"chatgpt-${TAG}-${PLATFORM}-${ARCH}.tar.gz\"",
        "sha256sum \"$ARCHIVE\" > \"${ARCHIVE}.sha256\"",
        "shasum -a 256 \"$ARCHIVE\" > \"${ARCHIVE}.sha256\"",
        "gh release upload \"$TAG\" \"$ARCHIVE\" \"${ARCHIVE}.sha256\"",
    ] {
        assert!(
            workflow.contains(entry),
            "release workflow must contain {entry}"
        );
    }
    let install = workflow
        .find("sh install.sh --archive")
        .expect("installer smoke");
    let upload = workflow.find("gh release upload").expect("upload");
    assert!(
        install < upload,
        "each archive must install before it uploads"
    );
    let (_, publish) = workflow.split_once("\n  publish:\n").expect("publish job");
    assert!(publish.contains("needs: [release-please, build]"));
    assert!(publish.contains("if: needs.release-please.outputs.release_created == 'true' && needs.build.result == 'success'\n"));
    assert!(!workflow.contains("continue-on-error:"));
}
