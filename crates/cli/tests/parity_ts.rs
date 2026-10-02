//! The parity harness: the TS CLI and the Rust CLI answer `list` and
//! `stats` from the SAME data, and must print the same thing.
//!
//! The Rust daemon syncs synthetic chats from the fake chatgpt.com and
//! imports judgments, follow-ups, Luna reviews and local titles (built to
//! hit every policy branch, `toFixed` ties included) from a TS index. Its
//! chats are then copied into that TS index, and this repository's TS CLI
//! (`bun src/cli.ts`, pointed at it with `XDG_DATA_HOME`) answers the same
//! commands. stdout is compared: JSON by value, text byte for byte.
//!
//! Needs bun and `bun install` in the repository; skips (with a note)
//! without them.

#![allow(clippy::unwrap_used)]

mod support;

use std::path::PathBuf;
use std::process::Command;

use fake_chatgpt::Chat;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use support::{Env, QUESTIONS_VERSION, bun, copy_chats, repo};

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

fn fill_ts_index(db: &Connection, chats: &[Chat], rng: &mut Lcg) {
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

struct Ts {
    bun: PathBuf,
    data_home: PathBuf,
    home: PathBuf,
}

impl Ts {
    fn run(&self, args: &[&str]) -> (String, Option<i32>) {
        let output = Command::new(&self.bun)
            .arg(repo().join("src/cli.ts"))
            .args(args)
            .env("XDG_DATA_HOME", &self.data_home)
            // No browser here: the saved-memory read fails without a prompt.
            .env("HOME", &self.home)
            .env_remove("CHATGPT_BROWSER")
            .env_remove("CHATGPT_BROWSER_PROFILE")
            .current_dir(repo())
            .output()
            .unwrap();
        (
            String::from_utf8(output.stdout).unwrap(),
            output.status.code(),
        )
    }
}

#[test]
fn list_and_stats_match_the_ts_cli_on_the_same_data() {
    let Some(bun) = bun() else {
        eprintln!("skipping the TS parity harness: bun isn't on PATH");
        return;
    };
    if !repo().join("node_modules/commander").is_dir() {
        eprintln!("skipping the TS parity harness: run `bun install` in the repository first");
        return;
    }
    let mut rng = Lcg(0x5eed);
    let chats = chats(&mut rng);
    let env = Env::with_fake(chats.clone());
    // As in the TS run, the saved memories can't be read.
    env.fake().state().memories = None;
    let ts_db = env.legacy_db();
    fill_ts_index(&ts_db, &chats, &mut rng);
    drop(ts_db);
    env.cmd().arg("sync").assert().success();
    assert_eq!(
        copy_chats(&env.data_dir().join("chatgpt.db"), &env.legacy),
        120
    );

    let ts = Ts {
        bun,
        data_home: env.legacy.parent().unwrap().parent().unwrap().to_path_buf(),
        home: env.home.path().join("ts-home"),
    };
    // The TS CLI resolves `$XDG_DATA_HOME/chatgpt-cli/index.db`.
    assert_eq!(
        env.legacy.parent().unwrap().file_name().unwrap(),
        "chatgpt-cli"
    );
    std::fs::create_dir_all(&ts.home).unwrap();

    let filters: &[&[&str]] = &[
        &[],
        &["--all"],
        &["--archived"],
        &["--suggest", "delete"],
        &["--suggest", "archive"],
        &["--suggest", "keep"],
        &["--brainstorm"],
        &["--brainstorm", "writing"],
        &["--brainstorm", "product"],
        &["--topic", "faith"],
        &["--topic", "employer_work", "--all"],
        &["--title", "love"],
        &["--title", "^local"],
        &["--limit", "7"],
        &["--older-than", "300d"],
        &["--newer-than", "2y", "--before", "2026-06-01"],
        &[
            "--after",
            "2025-06-01",
            "--all",
            "--suggest",
            "delete",
            "--limit",
            "5",
        ],
    ];
    let mut compared = 0;
    for filter in filters {
        let json_args: Vec<&str> = ["list", "--json"]
            .iter()
            .chain(filter.iter())
            .copied()
            .collect();
        let (ts_json, ts_code) = ts.run(&json_args);
        assert_eq!(ts_code, Some(0), "TS {json_args:?}");
        let rust_json = env.stdout(&json_args);
        let ts_value: Value = serde_json::from_str(&ts_json).unwrap();
        let rust_value: Value = serde_json::from_str(&rust_json).unwrap();
        assert_eq!(rust_value, ts_value, "list --json {filter:?}");
        assert_eq!(rust_json, ts_json, "list --json {filter:?}, byte for byte");

        for extra in [&[][..], &["--count"][..], &["--format", "ids"][..]] {
            let args: Vec<&str> = ["list"]
                .iter()
                .chain(filter.iter())
                .chain(extra.iter())
                .copied()
                .collect();
            let (ts_out, _) = ts.run(&args);
            assert_eq!(env.stdout(&args), ts_out, "{args:?}");
        }

        let stats_args: Vec<&str> = ["stats"].iter().chain(filter.iter()).copied().collect();
        if filter.contains(&"--limit") || filter.is_empty() || filter.contains(&"--all") {
            let (ts_stats, ts_code) = ts.run(&stats_args);
            let rust = env.cmd().args(&stats_args).output().unwrap();
            assert_eq!(rust.status.code(), ts_code, "{stats_args:?} exit code");
            assert_eq!(
                String::from_utf8(rust.stdout).unwrap(),
                ts_stats,
                "{stats_args:?}"
            );
        }
        compared += 1;
    }
    // The data reaches the branches it was built for, so matching means
    // something.
    let rows: Vec<Value> = serde_json::from_str(&env.stdout(&["list", "--json", "--all"])).unwrap();
    let count = |test: &dyn Fn(&Value) -> bool| rows.iter().filter(|row| test(row)).count();
    let judged = count(&|row| row.get("jev").is_some());
    let luna = count(&|row| row["jev"]["luna"] == true);
    let deep = count(&|row| row["jev"]["deep"] == true);
    let unsure = count(&|row| row["jev"]["unsure"] == true);
    let titled = count(&|row| !row["local_title"].is_null());
    let brainstorms = count(&|row| row["jev"]["brainstorm"].is_string());
    eprintln!(
        "TS parity: {compared} filter combinations matched on {} chats ({judged} judged, {luna} Luna, {deep} follow-up, {unsure} unsure, {brainstorms} brainstorms, {titled} local titles)",
        rows.len()
    );
    for (name, found) in [
        ("judged", judged),
        ("Luna", luna),
        ("follow-up", deep),
        ("unsure", unsure),
        ("titled", titled),
        ("brainstorm", brainstorms),
    ] {
        assert!(
            found >= 3,
            "only {found} {name} rows: the fixture misses that branch"
        );
    }
}
