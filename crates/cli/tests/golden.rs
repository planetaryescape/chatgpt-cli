//! Golden output, frozen from the TS CLI: the TS parity harnesses
//! (`parity_ts.rs`, `parity_export_search.rs`, in tag v0.1.5) ran the TS
//! CLI and the Rust CLI over the same data and required identical output.
//! Their last run recorded what the TS CLI printed into `golden/*.jsonl`,
//! one command a line: `args`, then `stdout`, `stderr`, or a `file` written
//! with `contents`, and `code` (0 for success, else a failure). These tests
//! build the same data for the Rust CLI alone and require the same output,
//! so it stays pinned without bun.
//!
//! The data: `list_stats` is 120 synthetic chats with judgments, follow-ups,
//! Luna reviews and local titles built to hit every policy branch (and the
//! number formats JS writes differently); `export_search` and `semantic`
//! are rich conversation trees (`fake_chatgpt::fixtures`), searched
//! lexically, semantically and in hybrid with the stand-in embedder, and
//! remotely against a fake `global/search`. Two `list` filters relative to
//! today (`--older-than`, `--newer-than`) were left out.
//!
//! An intended change to one of these outputs updates its line
//! (`CHATGPT_UPDATE_GOLDEN=1` rewrites them all from this build), with the
//! reason in the commit.

#![allow(clippy::unwrap_used)]

mod support;

use fake_chatgpt::Chat;
use fake_chatgpt::fixtures::{KINDS, rich_chats};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use support::{Env, QUESTIONS_VERSION};

const DEEP_VERSION: &str = "2026-09-27.2";
const LUNA_VERSION: i64 = 8;

/// A small deterministic generator, so a failure reproduces.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[(self.next() as usize) % items.len()]
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

/// Probabilities around every threshold in policy.ts; exact binary ties,
/// which `toFixed` rounds differently from Rust's formatter; and floats
/// whose shortest form needs 17 digits, as Jev returns some (serde_json's
/// default parser reads 0.9400000000000001 as 0.94).
const NOULS: &[f64] = &[
    0.02,
    0.1,
    0.125,
    0.25,
    0.3,
    0.35,
    0.375,
    0.4,
    0.45,
    0.47000000000000003,
    0.5,
    0.55,
    0.6,
    0.625,
    0.65,
    0.7,
    0.75,
    0.8,
    0.875,
    0.9,
    0.9299999999999999,
    0.9400000000000001,
    0.97,
];
const SCORES: &[f64] = &[0.2, 0.5, 0.95, 1.25, 1.5, 1.75, 1.9, 2.25, 2.8];
const TARGETS: &[&str] = &["none", "writing", "sermon", "product", "other"];
const TOPICS: &[&str] = &[
    "employer_work",
    "side_projects",
    "coding_general",
    "writing_creativity",
    "faith",
    "family_relationships",
    "health",
    "home_money_admin",
    "career_employment",
    "travel_transport",
    "learning_culture",
    "other",
];
const SUGGESTIONS: &[&str] = &["delete", "archive", "keep"];
const WORDS: &[&str] = &[
    "Love", "Book", "outline", "taxes", "trip", "sermon", "app", "pricing", "baby", "garden",
];

fn answers(rng: &mut Lcg) -> Value {
    let mut answers = json!({
        "worth_keeping": { "type": "score", "score": rng.pick(SCORES), "confidence": rng.pick(NOULS) },
        "nothing_there": { "type": "noul", "noul": rng.pick(NOULS) },
        "unfinished": { "type": "noul", "noul": rng.pick(NOULS) },
        "personal_record": { "type": "noul", "noul": rng.pick(NOULS) },
        "re_askable": { "type": "noul", "noul": rng.pick(NOULS) },
        "brainstorming": { "type": "noul", "noul": rng.pick(NOULS) },
        "brainstorm_for": { "type": "choice", "choice": rng.pick(TARGETS) },
        "topic": { "type": "choice", "choice": rng.pick(TOPICS) },
    });
    // Numbers JSON.stringify writes differently from serde_json.
    answers["extremes"] = json!({
        "tiny": 1e-6, "tinier": 1e-7, "small": 0.000001234, "huge": 1e21,
        "big": 123456789012345680000.0_f64,
    });
    for optional in ["product_idea", "time_bound", "overtaken_by_time"] {
        if rng.chance(60) {
            answers[optional] = json!({ "type": "noul", "noul": rng.pick(NOULS) });
        }
    }
    answers
}

fn deep_answers(rng: &mut Lcg) -> Value {
    json!({
        "personal_record_lost": { "noul": rng.pick(NOULS) },
        "reusable_artifact_lost": { "noul": rng.pick(NOULS) },
        "original_thinking_lost": { "noul": rng.pick(NOULS) },
        "work_to_resume": { "noul": rng.pick(NOULS) },
        "creative_idea_lost": { "noul": rng.pick(NOULS) },
        "reaskable_without_loss": { "noul": rng.pick(NOULS) },
        "worth_finding_again": { "score": rng.pick(SCORES), "confidence": rng.pick(NOULS) },
    })
}

/// 120 chats, unique update times, some archived, pinned or in a project.
fn chats(rng: &mut Lcg) -> Vec<Chat> {
    (0..120)
        .map(|n| {
            let day = 1 + (n % 28);
            let month = 1 + (n / 28) % 12;
            let year = 2024 + n / (28 * 12 / 4);
            let update = format!(
                "{year}-{month:02}-{day:02}T{:02}:{:02}:00.{n:06}Z",
                n % 24,
                n % 60
            );
            let title = format!("{} {} {n}", rng.pick(WORDS), rng.pick(WORDS));
            let mut chat = Chat::new(&format!("chat-{n:03}"), &title, &update);
            chat.archived = rng.chance(15);
            chat.pinned = rng.chance(8);
            if rng.chance(12) {
                chat.gizmo_id = Some(format!("g-p-{}", n % 3));
            }
            chat
        })
        .collect()
}

fn fill_index(db: &Connection, chats: &[Chat], rng: &mut Lcg) {
    for chat in chats {
        if rng.chance(15) {
            continue; // not judged yet
        }
        // A few judgments are stale: for an older update time or questions.
        let update_time = if rng.chance(5) {
            "2020-01-01T00:00:00Z".to_owned()
        } else {
            chat.update_time.clone()
        };
        let version = if rng.chance(5) {
            "2026-09-27.8"
        } else {
            QUESTIONS_VERSION
        };
        db.execute(
            "insert into judgments (id, update_time, version, content_kind, answers, classified_at)
             values (?, ?, ?, 'full', ?, '2026-09-28T00:00:00Z')",
            params![chat.id, update_time, version, answers(rng).to_string()],
        )
        .unwrap();
        let deep_version = if rng.chance(45) {
            let deep_version = if rng.chance(85) {
                DEEP_VERSION
            } else {
                "2026-09-01.1"
            };
            db.execute(
                "insert into deep_judgments values (?, ?, ?, ?, ?, '2026-09-28T00:00:00Z')",
                params![
                    chat.id,
                    update_time,
                    version,
                    deep_version,
                    deep_answers(rng).to_string()
                ],
            )
            .unwrap();
            deep_version
        } else {
            ""
        };
        if rng.chance(50) {
            let brainstorm: Option<&str> = if rng.chance(40) {
                Some(*rng.pick(&["writing", "sermon", "product", "other"]))
            } else {
                None
            };
            db.execute(
                "insert into luna_judgments values (?, ?, ?, ?, ?, ?, ?, ?, '2026-09-28T00:00:00Z')",
                params![
                    chat.id,
                    update_time,
                    version,
                    deep_version,
                    if rng.chance(85) { LUNA_VERSION } else { 7 },
                    rng.pick(SUGGESTIONS),
                    brainstorm,
                    if rng.chance(80) { "Luna's reason" } else { "" },
                ],
            )
            .unwrap();
        }
        if rng.chance(40) {
            let (version, source, time) = match rng.next() % 4 {
                0 => (2, "luna", chat.update_time.as_str()),
                1 => (1, "luna", chat.update_time.as_str()),
                2 => (2, "luna", "2020-01-01T00:00:00Z"),
                _ => (2, "manual", "2020-01-01T00:00:00Z"),
            };
            db.execute(
                "insert into local_titles values (?, ?, ?, ?, ?, 'theme', 'now')",
                params![
                    chat.id,
                    time,
                    version,
                    source,
                    format!("Local {}", chat.title)
                ],
            )
            .unwrap();
        }
    }
}

/// ChatGPT's search results for the remote comparison: odd whitespace,
/// emoji where the 160-unit cut falls, archived chats, a repeat, a chat the
/// index doesn't know and a result that isn't a conversation.
fn remote_items(chats: &[Chat]) -> Vec<Value> {
    let snippets = [
        "plain words".to_owned(),
        "  lead\u{a0}and\u{2028}odd\u{feff} spaces \t".to_owned(),
        format!("{}👍 after the cut", "a".repeat(159)),
        format!("{}👍 whole emoji", "b".repeat(158)),
        "日本語 Привет café".repeat(12),
        String::new(),
    ];
    let mut items: Vec<Value> = chats
        .iter()
        .take(24)
        .enumerate()
        .map(|(n, chat)| {
            json!({
                "source_type": "conversation",
                "title": format!("Remote {}", chat.title),
                "snippet": snippets[n % snippets.len()],
                "update_time": 1_758_000_000.0 + n as f64 * 3_600.123_456,
                "payload": { "conversation_id": chat.id, "is_archived": chat.archived },
            })
        })
        .collect();
    items.insert(3, json!({ "source_type": "project", "title": "A project" }));
    items.insert(5, items[1].clone());
    items.insert(
        7,
        json!({
            "source_type": "conversation", "title": "Never synced", "snippet": "elsewhere",
            "update_time": 1_700_000_000.000_9,
            "payload": { "conversation_id": "zz-unknown", "is_archived": false },
        }),
    );
    items
}

/// The local titles `search` prints for two of the rich chats.
fn add_local_titles(db: &Connection) {
    for (id, title) in [("chat-003", "Local three"), ("chat-012", "Local twelve")] {
        db.execute(
            "insert into local_titles values (?, '2020-01-01T00:00:00Z', 2, 'manual', ?, '', 'now')",
            [id, title],
        )
        .unwrap();
    }
}

/// Rewrites the golden files with this build's output instead of
/// comparing, for an intended change: review the diff before committing.
const UPDATE_ENV: &str = "CHATGPT_UPDATE_GOLDEN";

/// Run every command in `golden/<name>.jsonl` and compare. Returns how
/// many it compared.
fn replay(env: &Env, name: &str) -> usize {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.jsonl"));
    let update = std::env::var_os(UPDATE_ENV).is_some();
    let out_dir = env.home.path().join("golden-out");
    std::fs::create_dir_all(&out_dir).unwrap();
    let mut updated = Vec::new();
    for line in std::fs::read_to_string(&path).unwrap().lines() {
        let mut entry: Value = serde_json::from_str(line).unwrap();
        let args: Vec<String> = entry["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| arg.as_str().unwrap().to_owned())
            .collect();
        let mut command = env.cmd();
        command.current_dir(&out_dir).args(&args);
        if let Some(stdin) = entry["stdin"].as_str() {
            command.write_stdin(stdin);
        }
        let output = command.output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let written = entry["file"]
            .as_str()
            .map(|file| std::fs::read_to_string(out_dir.join(file)).unwrap());
        if let Some(code) = entry["code"].as_i64() {
            assert_eq!(
                output.status.success(),
                code == 0,
                "{args:?} exited {}: {stderr}",
                output.status
            );
        }
        if update {
            for (field, now) in [
                ("stdout", Some(stdout)),
                ("stderr", Some(stderr)),
                ("contents", written),
            ] {
                if let (Some(now), Some(old)) = (now, entry.get_mut(field)) {
                    *old = Value::from(now);
                }
            }
        } else {
            if let Some(expected) = entry["stdout"].as_str() {
                assert_eq!(stdout, expected, "{args:?}");
            }
            if let Some(expected) = entry["stderr"].as_str() {
                assert_eq!(stderr, expected, "{args:?} stderr");
            }
            if let Some(written) = written {
                assert_eq!(
                    written,
                    entry["contents"].as_str().unwrap(),
                    "{args:?} wrote {}",
                    entry["file"]
                );
            }
        }
        updated.push(entry.to_string());
    }
    if update {
        std::fs::write(&path, updated.join("\n") + "\n").unwrap();
    }
    updated.len()
}

#[test]
fn list_and_stats_print_what_the_ts_cli_printed() {
    let mut rng = Lcg(0x5eed);
    let chats = chats(&mut rng);
    let env = Env::with_fake(chats.clone());
    // As in the TS run, the saved memories can't be read.
    env.fake().state().memories = None;
    env.cmd().arg("sync").assert().success();
    fill_index(&env.index_db(), &chats, &mut rng);
    assert_eq!(replay(&env, "list_stats"), 65);
}

#[test]
fn export_and_search_print_what_the_ts_cli_printed() {
    let chats: Vec<Chat> = rich_chats(KINDS * 5, 0x5eed);
    let env = Env::with_fake(chats.clone());
    env.cmd().arg("sync").assert().success();
    let index = env.wait_for_indexer();
    assert_eq!(index["indexed"], chats.len(), "{index}");
    add_local_titles(&env.index_db());
    assert_eq!(replay(&env, "export_search"), 352);
}

#[test]
fn semantic_hybrid_and_remote_search_print_what_the_ts_cli_printed() {
    let chats: Vec<Chat> = rich_chats(KINDS * 5, 0x5eed);
    let env = Env::with_fake(chats.clone());
    env.cmd().arg("sync").assert().success();
    let embeddings = env.wait_for_embedder();
    assert_eq!(embeddings["embedded"], embeddings["chunks"], "{embeddings}");
    add_local_titles(&env.index_db());
    env.fake().state().search_items = remote_items(&chats);
    assert_eq!(replay(&env, "semantic"), 212);
}
