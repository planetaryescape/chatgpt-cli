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
