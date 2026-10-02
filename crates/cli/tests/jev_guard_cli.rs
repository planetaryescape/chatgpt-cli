//! The Jev guard behind `archive`/`delete --check` and `--suggest <action>`,
//! through the real binary, the fake chatgpt.com and a fake TypeSafe: what
//! it judges and reuses, what it prints, what it saves, and how it holds
//! back anything it can't judge.

#![allow(clippy::unwrap_used)]

mod support;

use fake_chatgpt::Chat;
use fake_chatgpt::typesafe::{API_KEY, FakeTypeSafe};
use serde_json::Value;
use support::{Env, delete_answers, judge};

const OLD: &str = "2024-03-01T10:00:00.000000Z";

fn chats() -> Vec<Chat> {
    let mut project = Chat::new("c-idea", "Book idea", "2024-03-01T08:00:00.000000Z");
    project.gizmo_id = Some("g-p-1".into());
    vec![
        Chat::new("a-junk", "Junk ping", OLD),
        Chat::new("b-maybe", "Maybe receipt", "2024-03-01T09:00:00.000000Z"),
        project,
        Chat::new("d-stale", "Stale trip", "2024-03-01T07:00:00.000000Z"),
    ]
}

fn with_jev(typesafe: &FakeTypeSafe, key: Option<&str>) -> Env {
    let mut env = Env::with_fake(chats());
    env.extra_env
        .push(("TYPESAFE_BASE_URL".into(), typesafe.url.clone()));
    if let Some(key) = key {
        env.extra_env.push(("TYPESAFE_API_KEY".into(), key.into()));
    }
    env
}

fn run(env: &Env, args: &[&str]) -> (Option<i32>, String) {
    let output = env.cmd().args(args).output().unwrap();
    assert!(output.stdout.is_empty(), "status only on stderr");
    (
        output.status.code(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

/// Without the running counts and durations, which vary.
fn steady(stderr: &str) -> String {
    stderr
        .lines()
        // A running count draws a bar; a summary line doesn't.
        .filter(|line| !line.contains(['█', '░']))
        .map(|line| match line.rfind(" (") {
            Some(at) if line.ends_with("s)") => &line[..at],
            _ => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn check_judges_each_chat_and_acts_only_on_confident_ones() {
    let typesafe = FakeTypeSafe::start();
    let env = with_jev(&typesafe, Some(API_KEY));
    env.cmd().arg("sync").assert().success();
    // The indexer has cached every transcript, so the guard reads them.
    env.wait_for_indexer();
    let (code, stderr) = run(
        &env,
        &["delete", "a-junk", "b-maybe", "c-idea", "--check", "-n"],
    );
    assert_eq!(code, Some(0), "{stderr}");
    let steady = steady(&stderr);
    let lines: Vec<&str> = steady.lines().collect();
    assert_eq!(
        lines[..5],
        [
            "3 chat(s): 0 already judged, 3 new or changed to judge.",
            "Steps: [1/3] download transcripts → [2/3] judge short chats → [3/3] summarise and judge long chats",
            "[1/3] Download transcripts: all 3 cached, nothing to download.",
            "[2/3] Judging short chats…",
            "[2/3] Judging short chats: 3 judged, $0.0002 so far",
        ],
        "{stderr}"
    );
    assert_eq!(
        lines[5..10],
        [
            "[3/3] Summarising and judging long chats: nothing to do.",
            "Cost this run:",
            "  Jev             $0.0002  3 call(s), 4k tokens (billed)",
            "  total           $0.0002  of which billed: $0.0002",
            "Classification backs delete for 1 of 3.",
        ]
    );
    assert_eq!(lines[10], "Held back (suggestion, then the title):");
    assert!(
        lines[11].starts_with("  delete?     Maybe receipt  (nothing 0.50 · "),
        "{}",
        lines[11]
    );
    assert!(
        lines[12].starts_with("  keep        Book idea  (nothing 0.02 · "),
        "{}",
        lines[12]
    );
    assert_eq!(
        lines[13..],
        [
            "a-junk  2024-03-01       Junk ping",
            "dry run: would delete 1 conversation(s)."
        ]
    );
    assert_eq!(typesafe.calls(), 3);
    let body: Value = serde_json::from_str(&typesafe.state().bodies[0]).unwrap();
    let keys: Vec<&str> = body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, ["state", "questions", "model"]);
    assert_eq!(body["model"], "jev-latest");
    let idea = typesafe
        .state()
        .bodies
        .iter()
        .map(|body| serde_json::from_str::<Value>(body).unwrap())
        .find(|body| body["state"]["conversation"]["title"] == "Book idea")
        .unwrap();
    let conversation = &idea["state"]["conversation"];
    assert_eq!(conversation["created"], "2026-01-01");
    assert_eq!(conversation["last_updated"], "2024-03-01");
    assert_eq!(conversation["turns"], 1);
    assert_eq!(conversation["in_a_project"], true);
    assert_eq!(conversation["pinned"], false);
    assert_eq!(idea["state"]["content_kind"], "full transcript");
    assert!(
        idea["state"]["content"]
            .as_str()
            .unwrap()
            .contains("Hello from c-idea")
    );

    // Saved: `list` shows the verdicts, and a second run asks Jev nothing.
    let rows: Vec<Value> =
        serde_json::from_str(&env.stdout(&["list", "--json", "--title", "junk"])).unwrap();
    assert_eq!(rows[0]["jev"]["suggestion"], "delete");
    assert_eq!(
        rows[0]["jev"]["answers"]["product_idea"]["noul"].to_string(),
        "1e-7"
    );
    let (_, again) = run(
        &env,
        &["archive", "a-junk", "b-maybe", "c-idea", "--check", "-n"],
    );
    assert!(
        again.starts_with("3 chat(s): 3 already judged, 0 new or changed to judge.\nClassification backs archive for 1 of 3.\n"),
        "{again}"
    );
    assert_eq!(typesafe.calls(), 3);
    assert!(
        env.fake()
            .calls()
            .iter()
            .all(|call| !call.starts_with("DELETE"))
    );
}

#[test]
fn without_a_key_nothing_is_judged_so_nothing_is_approved() {
    let typesafe = FakeTypeSafe::start();
    let env = with_jev(&typesafe, None);
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    let (code, stderr) = run(&env, &["delete", "a-junk", "--check", "-y"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stderr.contains("failed: a-junk Junk ping: TYPESAFE_API_KEY is not configured. Run `chatgpt configure jev` or set TYPESAFE_API_KEY.\n"), "{stderr}");
    assert!(stderr.ends_with("Classification backs delete for 0 of 1.\nHeld back (suggestion, then the title):\n  not judged  Junk ping\nNothing matched.\n"), "{stderr}");
    assert_eq!(typesafe.calls(), 0);

    // The user config's key, read at the time of the call.
    let config = env.home.path().join("xdg-config/chatgpt-cli");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("config.json"),
        format!("{{\"jev\": \"{API_KEY}\"}}"),
    )
    .unwrap();
    let (code, stderr) = run(&env, &["delete", "a-junk", "--check", "-y"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stderr.contains("Deleted 1 conversation(s)"), "{stderr}");
}

#[test]
fn a_refused_call_is_a_failure_without_its_body() {
    let typesafe = FakeTypeSafe::start();
    let env = with_jev(&typesafe, Some("wrong-key"));
    env.cmd().arg("sync").assert().success();
    let (_, stderr) = run(&env, &["archive", "a-junk", "--check", "-n"]);
    assert!(
        stderr.contains("failed: a-junk Junk ping: TypeSafe answered 401 (check the Jev API key: `chatgpt configure jev` or TYPESAFE_API_KEY)\n"),
        "{stderr}"
    );
    assert!(!stderr.contains("secret"), "{stderr}");
    assert!(!stderr.contains("wrong-key"), "{stderr}");
    assert!(stderr.ends_with("Nothing matched.\n"));
}

#[test]
fn suggest_reuses_current_judgments_and_leaves_unsure_ones_out() {
    let typesafe = FakeTypeSafe::start();
    let env = with_jev(&typesafe, Some(API_KEY));
    let db = env.legacy_db();
    judge(&db, "a-junk", OLD, &delete_answers("other"));
    let mut unsure: Value = serde_json::from_str(&delete_answers("other")).unwrap();
    unsure["personal_record"]["noul"] = serde_json::json!(0.45);
    unsure["nothing_there"]["noul"] = serde_json::json!(0.5);
    unsure["re_askable"]["noul"] = serde_json::json!(0.75);
    judge(
        &db,
        "b-maybe",
        "2024-03-01T09:00:00.000000Z",
        &unsure.to_string(),
    );
    // A time-bound chat judged long ago is due for another look.
    let mut stale: Value = serde_json::from_str(&delete_answers("other")).unwrap();
    stale["time_bound"] = serde_json::json!({ "noul": 0.9 });
    stale["overtaken_by_time"] = serde_json::json!({ "noul": 0.1 });
    db.execute(
        "insert into judgments (id, update_time, version, content_kind, answers, classified_at)
         values ('d-stale', '2024-03-01T07:00:00.000000Z', ?, 'full', ?, '2026-01-01T00:00:00.000Z')",
        rusqlite::params![support::QUESTIONS_VERSION, stale.to_string()],
    )
    .unwrap();
    drop(db);
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();

    let (code, stderr) = run(&env, &["delete", "--suggest", "delete", "-n"]);
    assert_eq!(code, Some(0), "{stderr}");
    let steady = steady(&stderr);
    assert!(
        steady.starts_with("2 chat(s): 1 already judged, 1 new or changed to judge.\n"),
        "unsure b-maybe left out; d-stale due again: {stderr}"
    );
    assert!(
        steady.contains("\n[1/3] Download transcripts: all 1 cached, nothing to download.\n"),
        "{stderr}"
    );
    assert!(
        steady.ends_with("Classification backs delete for 1 of 2.\nHeld back (suggestion, then the title):\n  archive     Stale trip  (nothing 0.30 · re-askable 0.60 · worth 1.2/3 · unfinished 0.05 · personal 0.10 · brainstorm 0.05 · product idea 0.00 · time-bound 0.90 · overtaken 0.10)\na-junk  2024-03-01       Junk ping\ndry run: would delete 1 conversation(s)."),
        "{stderr}"
    );
    assert_eq!(typesafe.calls(), 1, "only the stale one");
}
