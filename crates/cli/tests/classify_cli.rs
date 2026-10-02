//! `classify`, `titles`, `memory classify` and `configure`, through the
//! real binary against the fake chatgpt.com, a fake TypeSafe, fake OpenAI
//! and Anthropic APIs, and stand-in `codex` and `claude` on `PATH`: what
//! they judge, summarise and title, what they print and cost, what they
//! ask, and what reaches the subscription CLIs.

#![allow(clippy::unwrap_used)]

mod support;

use std::time::{Duration, Instant};

use fake_chatgpt::Chat;
use fake_chatgpt::models::{ANTHROPIC_KEY, FakeModelApi, OPENAI_KEY};
use fake_chatgpt::typesafe::{API_KEY, FakeTypeSafe};
use serde_json::Value;
use support::{Env, in_terminal};

fn chats() -> Vec<Chat> {
    let mut long = Chat::new("e-long", "Long essay", "2024-03-01T05:00:00.000000Z");
    long.text = "word ".repeat(12_000);
    let mut pinned = Chat::new("f-pinned", "Pinned junk", "2024-03-01T04:00:00.000000Z");
    pinned.pinned = true;
    vec![
        Chat::new("a-junk", "Junk ping", "2024-03-01T10:00:00.000000Z"),
        Chat::new("b-maybe", "Maybe receipt", "2024-03-01T09:00:00.000000Z"),
        Chat::new("c-idea", "Book idea", "2024-03-01T08:00:00.000000Z"),
        Chat::new(
            "d-product",
            "My product plan",
            "2024-03-01T07:00:00.000000Z",
        ),
        long,
        pinned,
    ]
}

fn with_models(typesafe: &FakeTypeSafe) -> Env {
    let mut env = Env::with_fake(chats());
    env.extra_env.extend([
        ("TYPESAFE_BASE_URL".to_owned(), typesafe.url.clone()),
        ("TYPESAFE_API_KEY".to_owned(), API_KEY.to_owned()),
    ]);
    env
}

fn run(env: &Env, args: &[&str]) -> (Option<i32>, String, String) {
    let output = env.cmd().args(args).output().unwrap();
    (
        output.status.code(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

/// Without the running counts and durations, which vary.
fn steady(stderr: &str) -> String {
    stderr
        .lines()
        .filter(|line| !line.contains(['█', '░']))
        .map(|line| {
            let line = match line.rfind(" (") {
                Some(at) if line.ends_with("s)") => &line[..at],
                _ => line,
            };
            match line.strip_prefix("Done in ") {
                Some(rest) => format!(
                    "Done in _. {}",
                    rest.split_once(". ").map_or("", |(_, r)| r)
                ),
                None => line.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn list_json(env: &Env) -> Vec<Value> {
    serde_json::from_str(&env.stdout(&["list", "--json", "--all"])).unwrap()
}

fn row<'a>(rows: &'a [Value], id: &str) -> &'a Value {
    rows.iter().find(|row| row["id"] == id).unwrap()
}

#[test]
fn classify_runs_jev_its_follow_up_luna_and_titles_with_the_ts_notes_and_costs() {
    let typesafe = FakeTypeSafe::start();
    let env = with_models(&typesafe);
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    let (code, stdout, stderr) = run(&env, &["classify"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.is_empty());
    assert_eq!(
        steady(&stderr),
        [
            "5 chat(s): 0 already judged, 5 new or changed to judge.",
            "Steps: [1/3] download transcripts → [2/3] judge short chats → [3/3] summarise and judge long chats",
            "[1/3] Download transcripts: all 5 cached, nothing to download.",
            "1 long chat(s) for step 3; 1 need a new summary (gpt-6-luna, then claude-haiku), ~15k tokens, about $0.0034 API-equivalent on your subscription.",
            "[2/3] Judging short chats…",
            "[2/3] Judging short chats: 4 judged, $0.0002 so far",
            "[3/3] Summarising and judging long chats…",
            "[3/3] Summarising and judging long chats: 1 judged, $0.0024 so far",
            "Cost this run:",
            "  Jev             $0.0003  5 call(s), 6k tokens (billed)",
            "  summaries (gpt-6-luna)  $0.0021  1 call(s), 20k tokens (API-equivalent, covered by your subscription)",
            "  total           $0.0024  of which billed: $0.0003",
            "1 unsure chat(s): 0 already deep-classified, 1 to judge.",
            "Deep-classifying…",
            "Deep-classified 1 chat(s)",
            "Cost this run:",
            "  Jev             $0.0001  1 call(s), 1k tokens (billed)",
            "  total           $0.0001  of which billed: $0.0001",
            "1 chat(s) need Luna's deeper review.",
            "Deeper Luna review…",
            "Luna reviewed 1 chat(s)",
            "5 chat(s): 0 local titles cached or manual, 5 to generate in 1 batch(es).",
            "Generating local titles…",
            "Generated 5 local title(s)",
            "Done in _. 5 of 5 judged: delete 1, archive 1, keep 3 (0 still unsure).",
            "Next: `chatgpt list --suggest delete`, then `chatgpt review --suggest delete`.",
        ]
        .join("\n"),
        "{stderr}"
    );
    // Jev: five first passes and one follow-up, each validated and saved.
    assert_eq!(typesafe.calls(), 6);
    let rows = list_json(&env);
    assert_eq!(row(&rows, "a-junk")["jev"]["suggestion"], "delete");
    assert_eq!(row(&rows, "b-maybe")["jev"]["suggestion"], "archive");
    assert_eq!(row(&rows, "b-maybe")["jev"]["deep"], true);
    // Luna dropped the product label; in a product review it can only make
    // the verdict more protective, so Jev's keep stands.
    assert_eq!(row(&rows, "d-product")["jev"]["luna"], true);
    assert_eq!(row(&rows, "d-product")["jev"]["brainstorm"], Value::Null);
    assert_eq!(row(&rows, "d-product")["jev"]["suggestion"], "keep");
    assert_eq!(row(&rows, "c-idea")["display_title"], "Luna: Book idea");
    assert_eq!(row(&rows, "e-long")["jev"]["suggestion"], "keep");
    assert!(
        row(&rows, "f-pinned").get("jev").is_none(),
        "pinned left out"
    );

    // What reached the subscription CLI: a summary call, one Luna review
    // and one titles batch, each in an environment without our keys.
    let calls = env.model_calls();
    assert_eq!(calls.len(), 3, "{calls:?}");
    for call in &calls {
        assert_eq!(call["tool"], "codex");
        let env_names: Vec<&str> = call["env"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        for secret in ["TYPESAFE_API_KEY", "OPENAI_API_KEY", "CHATGPT_TEST_COOKIE"] {
            assert!(
                !env_names.contains(&secret),
                "{secret} leaked: {env_names:?}"
            );
        }
    }
    assert!(
        calls[0]["args"]
            .as_array()
            .unwrap()
            .iter()
            .any(|arg| arg == "--json")
    );
    assert!(
        calls[0]["stdin"]
            .as_str()
            .unwrap()
            .starts_with("Title: Long essay\n\n")
    );

    // Everything is current now: nothing more is paid for.
    let (code, _, stderr) = run(&env, &["classify"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        steady(&stderr).starts_with("5 chat(s): 5 already judged, 0 new or changed to judge.\n1 unsure chat(s): 1 already deep-classified, 0 to judge.\n0 chat(s) need Luna's deeper review."),
        "{stderr}"
    );
    assert_eq!(typesafe.calls(), 6);
    assert_eq!(env.model_calls().len(), 3);

    // --redo judges again, reusing the cached transcripts and summary.
    let (code, _, stderr) = run(&env, &["classify", "a-junk", "e-long", "--redo"]);
    assert_eq!(code, Some(0), "{stderr}");
    let shown = steady(&stderr);
    assert!(
        shown.starts_with("Re-judging all 2 matching chat(s).\n"),
        "{stderr}"
    );
    assert!(!shown.contains("need a new summary"), "{stderr}");
    assert_eq!(typesafe.calls(), 8);
}

/// How many summaries the stand-in tools made (Luna's calls carry a
/// schema).
fn summaries(env: &Env) -> usize {
    env.model_calls()
        .iter()
        .filter(|call| {
            !call["args"]
                .as_array()
                .unwrap()
                .iter()
                .any(|arg| arg == "--output-schema")
        })
        .count()
}

/// A chat long enough that summarising it needs a yes (over 500k tokens).
fn huge() -> Chat {
    let mut huge = Chat::new("h-huge", "Huge log", "2024-03-01T03:00:00.000000Z");
    huge.text = "word ".repeat(400_100);
    huge
}

#[test]
fn a_large_batch_of_summaries_is_asked_about_first_unless_yes() {
    let typesafe = FakeTypeSafe::start();
    let env = with_models(&typesafe);
    env.fake().state().chats.push(huge());
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();

    // Answered no at a terminal: the long chats are skipped, the rest judged.
    let (code, shown) = in_terminal(
        &env,
        &["classify", "h-huge", "a-junk"],
        None,
        "Go ahead? [y/N] ",
        "n",
    );
    // Its title can't be made without the summary either, as in the TS CLI.
    assert_eq!(code, Some(1), "{shown}");
    let shown = shown.replace('\r', "").replace("\u{1b}[2K", "");
    assert!(
        shown.contains("1 long chat(s) for step 3; 1 need a new summary (gpt-6-luna, then claude-haiku), ~500k tokens, about $0.05 API-equivalent on your subscription.\nGo ahead? [y/N] n\nSkipping those in step 3; everything else will still be judged.\n"),
        "{shown}"
    );
    assert!(
        shown.contains("failed: h-huge Huge log: no current summary; run classify first.\n"),
        "{shown}"
    );
    assert!(
        shown.contains(" 1 long chat(s) skipped for lack of a summary."),
        "{shown}"
    );
    assert_eq!(summaries(&env), 0, "nothing summarised");

    // Without a terminal to ask at, it stops rather than spend.
    let (code, _, stderr) = run(&env, &["classify", "h-huge"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("pass -y"), "{stderr}");
    assert_eq!(summaries(&env), 0, "nothing summarised");

    // -y summarises without asking.
    let (code, _, stderr) = run(&env, &["classify", "h-huge", "-y"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(!stderr.contains("Go ahead?"), "{stderr}");
    assert_eq!(summaries(&env), 1);
    let rows = list_json(&env);
    assert_eq!(row(&rows, "h-huge")["jev"]["suggestion"], "keep");
}

#[test]
fn summarisers_fall_back_in_the_ts_order() {
    let typesafe = FakeTypeSafe::start();
    let env = with_models(&typesafe);
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();

    // codex fails: claude -p takes over, and its own cost is counted.
    env.set_tools("fail", &["codex"]);
    std::fs::write(
        env.tools.join("claude"),
        std::fs::read_to_string(env.tools.join("codex"))
            .unwrap()
            .replace("'fail' codex", "'ok' claude"),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            env.tools.join("claude"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    let (code, _, stderr) = run(&env, &["classify", "e-long"]);
    assert_eq!(code, Some(1), "the titles step's codex failed: {stderr}");
    assert!(
        stderr.contains("  summaries (claude-haiku)    $0.01  1 call(s), 20k tokens (API-equivalent, covered by your subscription)\n"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("SENTINEL"),
        "a tool's output is never shown: {stderr}"
    );
    assert!(
        stderr.contains("failed: e-long Long essay: gpt-6-luna exited 1\n"),
        "{stderr}"
    );
    let tools: Vec<String> = env
        .model_calls()
        .iter()
        .map(|call| call["tool"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(tools, ["codex", "claude", "codex"]);

    // Neither on PATH: long chats are held back, with the TS CLI's note,
    // and with no summary, no title either.
    env.set_tools("ok", &[]);
    env.index_db().execute("delete from summaries", []).unwrap();
    let (code, _, stderr) = run(&env, &["classify", "e-long", "--redo"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("1 long chat(s) need a summary but neither codex nor claude is on PATH; skipping them in step 3.\n"), "{stderr}");
    assert!(
        stderr.contains("failed: e-long Long essay: no current summary; run classify first.\n"),
        "{stderr}"
    );
}

#[test]
fn api_keys_take_over_from_the_subscription_clis() {
    let typesafe = FakeTypeSafe::start();
    let openai = FakeModelApi::openai();
    let anthropic = FakeModelApi::anthropic();
    let mut env = with_models(&typesafe);
    env.extra_env.extend([
        ("OPENAI_API_KEY".to_owned(), OPENAI_KEY.to_owned()),
        ("CHATGPT_TEST_OPENAI_URL".to_owned(), openai.url.clone()),
        ("ANTHROPIC_API_KEY".to_owned(), ANTHROPIC_KEY.to_owned()),
        (
            "CHATGPT_TEST_ANTHROPIC_URL".to_owned(),
            anthropic.url.clone(),
        ),
    ]);
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    let (code, _, stderr) = run(&env, &["classify", "e-long", "d-product"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stderr.contains("need a new summary (gpt-6-luna (API), then claude-haiku (API))"),
        "{stderr}"
    );
    assert!(stderr.contains("  summaries (gpt-6-luna (API))  $0.0021  1 call(s), 20k tokens (API-equivalent, covered by your subscription)\n"), "{stderr}");
    assert!(env.model_calls().is_empty(), "no subscription CLI ran");
    // A summary, a Luna review and a titles batch, as the TS CLI sends them.
    let bodies = openai.state().bodies.clone();
    assert_eq!(bodies.len(), 3, "{bodies:?}");
    let summary: Value = serde_json::from_str(&bodies[0]).unwrap();
    assert_eq!(summary["model"], "gpt-6-luna");
    assert_eq!(summary["reasoning"]["effort"], "medium");
    assert_eq!(summary["store"], false);
    assert!(summary.get("text").is_none());
    assert!(
        bodies[1..]
            .iter()
            .all(|body| body.contains("\"name\":\"chatgpt_cli_result\",\"strict\":true"))
    );
    assert_eq!(anthropic.calls(), 0, "OpenAI answered first");

    // OpenAI failing falls through to Anthropic, whose error names only
    // the status.
    openai.state().fail = Some(500);
    let (code, _, stderr) = run(&env, &["classify", "e-long", "--redo"]);
    assert_eq!(code, Some(0), "the cached summary is reused: {stderr}");
    assert_eq!(anthropic.calls(), 0);
    env.index_db().execute("delete from summaries", []).unwrap();
    let (code, _, stderr) = run(&env, &["classify", "e-long", "--redo"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stderr.contains("  summaries (claude-haiku (API))    $0.02  1 call(s), 20k tokens"),
        "{stderr}"
    );
    assert_eq!(anthropic.calls(), 1);
    let body: Value = serde_json::from_str(&anthropic.state().bodies[0]).unwrap();
    assert_eq!(body["model"], "claude-haiku-4-5");
    assert_eq!(body["max_tokens"], 1800);
}

#[test]
fn the_guard_now_summarises_a_long_chat_instead_of_holding_it_back() {
    let typesafe = FakeTypeSafe::start();
    let env = with_models(&typesafe);
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    let (code, _, stderr) = run(&env, &["archive", "e-long", "a-junk", "--check", "-n"]);
    assert_eq!(code, Some(0), "{stderr}");
    let shown = steady(&stderr);
    assert!(
        shown.contains(
            "1 long chat(s) for step 3; 1 need a new summary (gpt-6-luna, then claude-haiku)"
        ),
        "{stderr}"
    );
    assert!(
        shown.contains("Classification backs archive for 1 of 2."),
        "{stderr}"
    );
    assert!(
        shown.contains("  keep        Long essay  ("),
        "judged from its summary: {stderr}"
    );
    assert_eq!(env.model_calls().len(), 1, "one summary, no Luna or titles");
}

#[test]
fn filters_ids_and_pinned_choose_what_classify_and_titles_judge() {
    let typesafe = FakeTypeSafe::start();
    let env = with_models(&typesafe);
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    let (code, _, stderr) = run(&env, &["classify", "--title", "junk", "-y"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stderr.starts_with("1 chat(s): 0 already judged, 1 new or changed to judge.\n"),
        "{stderr}"
    );
    let (_, _, stderr) = run(&env, &["classify", "--title", "junk", "--pinned"]);
    assert!(
        stderr.starts_with("2 chat(s): 1 already judged, 1 new or changed to judge.\n"),
        "{stderr}"
    );
    let (code, _, stderr) = run(&env, &["classify", "nope"]);
    assert_eq!(code, Some(2));
    assert_eq!(
        stderr,
        "error: No conversation matching \"nope\" in the index. Run `chatgpt sync`?\n"
    );

    // titles: the cached and manual ones stay; --redo regenerates Luna's only.
    env.cmd()
        .args(["title", "c-idea", "My own"])
        .assert()
        .success();
    let (code, _, stderr) = run(&env, &["titles", "a-junk", "c-idea", "b-maybe"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        steady(&stderr).starts_with(
            "3 chat(s): 2 local titles cached or manual, 1 to generate in 1 batch(es).\n"
        ),
        "{stderr}"
    );
    let (code, _, stderr) = run(&env, &["titles", "a-junk", "c-idea", "b-maybe", "--redo"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        steady(&stderr).starts_with(
            "3 chat(s): 1 local titles cached or manual, 2 to generate in 1 batch(es).\n"
        ),
        "{stderr}"
    );
    let rows = list_json(&env);
    assert_eq!(row(&rows, "c-idea")["display_title"], "My own");
    assert_eq!(
        row(&rows, "b-maybe")["display_title"],
        "Luna: Maybe receipt"
    );
}

#[test]
fn titles_split_a_failing_batch_and_name_each_chat_that_fails() {
    let typesafe = FakeTypeSafe::start();
    let env = with_models(&typesafe);
    env.fake().state().chats.push(Chat::new(
        "g-notitle",
        "A notitle chat",
        "2024-03-01T02:00:00.000000Z",
    ));
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    let (code, _, stderr) = run(&env, &["titles", "a-junk", "g-notitle", "c-idea"]);
    assert_eq!(code, Some(1), "{stderr}");
    let shown = steady(&stderr);
    assert!(
        shown.contains("Generated 2 local title(s), 1 failed or missing content"),
        "{stderr}"
    );
    assert!(
        shown.ends_with(
            "failed: g-notitle A notitle chat: Luna returned an invalid title or theme."
        ),
        "{stderr}"
    );
    // The batch of three, then its halves (two, and one), then the two's
    // halves.
    assert_eq!(env.model_calls().len(), 5);

    // A chat with no cached transcript can't be titled.
    env.index_db()
        .execute("delete from transcripts where id = 'b-maybe'", [])
        .unwrap();
    let (code, _, stderr) = run(&env, &["titles", "b-maybe"]);
    assert_eq!(code, Some(1));
    assert!(
        stderr.ends_with(
            "failed: b-maybe Maybe receipt: no cached transcript; run classify first.\n"
        ),
        "{stderr}"
    );
}

fn memories() -> Vec<Value> {
    vec![
        serde_json::json!({ "id": "mem-tea", "content": "Prefers green tea, \"not\" coffee", "updated_at": "2026-09-01T00:00:00Z", "status": "active", "conversation_id": null }),
        serde_json::json!({ "id": "mem-old", "content": "Is planning last year's trip", "updated_at": "2026-09-02T00:00:00Z", "status": "active", "conversation_id": "c-idea" }),
        serde_json::json!({ "id": "mem-move", "content": "Will move house soon", "updated_at": "2026-09-03T00:00:00Z", "status": "active", "conversation_id": null }),
    ]
}

#[test]
fn memory_classify_keeps_reviews_and_prints_each_format() {
    let typesafe = FakeTypeSafe::start();
    let env = with_models(&typesafe);
    env.fake().state().memories = Some(memories());
    let (code, stdout, stderr) = run(&env, &["memory", "classify"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        steady(&stderr),
        "3 saved memories: 0 cached, 3 new or changed.\nQuick memory classification…\nQuick-classified 3 saved memories\n2 saved memories need Luna's deeper review.\nLuna reviewed 2/2 memories."
    );
    assert_eq!(
        stdout,
        "SUGGEST  STAGE  ID                                    REASON\n\
         keep     quick  mem-tea  Enduring context with no clear expiry, replacement, or full duplicate.  [Prefers green tea, \"not\" coffee]\n\
         delete   deep   mem-old  Fake memory reason.  [Is planning last year's trip]\n\
         review   deep   mem-move  Fake memory reason.  [Will move house soon]\n"
    );
    assert_eq!(typesafe.calls(), 3);
    assert_eq!(env.model_calls().len(), 1, "one Luna batch");

    // Cached now: the formats, --suggest and --limit cost nothing.
    let (_, json, stderr) = run(
        &env,
        &[
            "memory",
            "classify",
            "--format",
            "json",
            "--suggest",
            "delete",
        ],
    );
    assert!(
        stderr.starts_with("3 saved memories: 3 cached, 0 new or changed.\n"),
        "{stderr}"
    );
    let rows: Vec<Value> = serde_json::from_str(&json).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0]["conversation_id"], "c-idea",
        "ChatGPT's fields kept"
    );
    assert_eq!(rows[0]["stage"], "deep");
    assert_eq!(rows[0]["related_ids"], serde_json::json!([]));
    let (_, ids, _) = run(
        &env,
        &["memory", "classify", "--format", "ids", "--limit", "2"],
    );
    assert_eq!(ids, "mem-tea\nmem-old\n");
    let (_, csv, _) = run(
        &env,
        &["memory", "classify", "--format", "csv", "--suggest", "keep"],
    );
    assert_eq!(
        csv,
        "id,suggestion,stage,reason,content,updated_at,related_ids\nmem-tea,keep,quick,\"Enduring context with no clear expiry, replacement, or full duplicate.\",\"Prefers green tea, \"\"not\"\" coffee\",2026-09-01T00:00:00Z,\n"
    );
    assert_eq!(typesafe.calls(), 3);
    let (code, _, stderr) = run(&env, &["memory", "classify", "--redo", "--format", "ids"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(typesafe.calls(), 6);
    for (args, error) in [
        (
            &["memory", "classify", "--suggest", "burn"][..],
            "error: --suggest must be keep, delete, or review.\n",
        ),
        (
            &["memory", "classify", "--limit", "0"],
            "error: --limit must be a positive integer.\n",
        ),
    ] {
        let (code, _, stderr) = run(&env, args);
        assert_eq!(code, Some(2));
        assert_eq!(stderr, error);
    }
    assert_eq!(typesafe.calls(), 6, "a bad option costs nothing");
}

#[test]
fn configure_stores_keys_privately_and_never_echoes_them() {
    let env = Env::new();
    let status = env.stdout(&["configure"]);
    let path = env.home.path().join("xdg-config/chatgpt-cli/config.json");
    assert_eq!(
        status,
        format!(
            "Config: {}\njev: not configured\nopenai: not configured\nanthropic: not configured\n",
            path.display()
        )
    );
    let output = env
        .cmd()
        .args(["configure", "openai"])
        .write_stdin("  sk-SENTINEL-key \n")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(stderr, format!("Saved openai key to {}.\n", path.display()));
    assert_eq!(support::mode(&path), 0o600);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "{\n  \"openai\": \"sk-SENTINEL-key\"\n}\n"
    );
    assert!(env.stdout(&["configure"]).contains("openai: configured\n"));

    // At a terminal: a prompt, and nothing typed is shown.
    let (code, shown) = in_terminal(
        &env,
        &["configure", "jev"],
        None,
        "press Enter to save):",
        "ts-SENTINEL-typed",
    );
    assert_eq!(code, Some(0), "{shown}");
    assert!(
        shown.contains("Enter jev API key (hidden; press Enter to save):"),
        "{shown}"
    );
    assert!(!shown.contains("SENTINEL"), "the key was echoed: {shown}");
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("\"jev\": \"ts-SENTINEL-typed\"")
    );

    let output = env
        .cmd()
        .args(["configure", "openai", "--remove"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "Removed stored openai key.\n"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "{\n  \"jev\": \"ts-SENTINEL-typed\"\n}\n"
    );
    for (args, stdin, error) in [
        (
            &["configure", "--remove"][..],
            "",
            "error: Choose a provider to remove: jev, openai, or anthropic.\n",
        ),
        (
            &["configure", "grok"],
            "",
            "error: Provider must be jev, openai, or anthropic.\n",
        ),
        (
            &["configure", "anthropic"],
            "   \n",
            "error: API key cannot be empty.\n",
        ),
    ] {
        let output = env.cmd().args(args).write_stdin(stdin).output().unwrap();
        assert!(!output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stderr), error, "{args:?}");
    }
    std::fs::write(&path, "{ SENTINEL").unwrap();
    let output = env.cmd().arg("configure").output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!("error: Invalid JSON in {}.\n", path.display())
    );
}

#[test]
fn the_follow_up_asks_before_a_large_batch_of_summaries_too() {
    let typesafe = FakeTypeSafe::start();
    let env = with_models(&typesafe);
    let mut huge = huge();
    huge.title = "Maybe huge log".into();
    env.fake().state().chats.push(huge);
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    let (code, _, stderr) = run(&env, &["classify", "h-huge", "-y"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(summaries(&env), 1);
    // A current, unsure first pass; its follow-up and summary gone (as
    // when the summary prompt changes).
    env.index_db()
        .execute_batch(
            "delete from deep_judgments; delete from luna_judgments; delete from summaries;",
        )
        .unwrap();
    let (code, _, stderr) = run(&env, &["classify", "h-huge"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(
        stderr.contains("1 unsure long chat(s) need a new summary for the follow-up"),
        "{stderr}"
    );
    assert!(stderr.contains("pass -y"), "{stderr}");
    assert_eq!(summaries(&env), 1, "nothing summarised without a yes");
}

#[test]
fn a_client_that_goes_away_starts_no_further_paid_call() {
    let typesafe = FakeTypeSafe::start();
    let env = with_models(&typesafe);
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    // codex takes three seconds to fail; claude would be next, then Jev.
    env.set_tools("slowfail", &["codex"]);
    std::fs::write(
        env.tools.join("claude"),
        std::fs::read_to_string(env.tools.join("codex"))
            .unwrap()
            .replace("'slowfail' codex", "'ok' claude"),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            env.tools.join("claude"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    let mut child = env
        .std_cmd()
        .args(["classify", "e-long"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while env.model_calls().is_empty() {
        assert!(std::time::Instant::now() < deadline, "codex never started");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // Ctrl-C while codex works.
    child.kill().unwrap();
    child.wait().unwrap();
    std::thread::sleep(std::time::Duration::from_secs(5));
    let tools: Vec<String> = env
        .model_calls()
        .iter()
        .map(|call| call["tool"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(tools, ["codex"], "claude was never started");
    assert_eq!(typesafe.calls(), 0, "nor Jev");
}

#[test]
fn a_client_that_cant_answer_is_never_asked_and_nothing_is_spent() {
    use std::io::{Read, Write};
    let typesafe = FakeTypeSafe::start();
    let env = with_models(&typesafe);
    env.fake().state().chats.push(huge());
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    // A 0.1.3 client's JevCheck: no `can_answer`, no Answer support.
    let request = serde_json::json!({ "id": 1, "payload": { "type": "request", "method": "jev_check",
        "action": "delete", "ids": ["h-huge"], "api_key": API_KEY } });
    let body = request.to_string();
    let mut socket = std::os::unix::net::UnixStream::connect(env.socket()).unwrap();
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(20)))
        .unwrap();
    socket
        .write_all(&u32::try_from(body.len()).unwrap().to_be_bytes())
        .unwrap();
    socket.write_all(body.as_bytes()).unwrap();
    // Heartbeats keep the socket busy, so the read timeout alone can't
    // catch a daemon that waits forever for an answer.
    let deadline = Instant::now() + Duration::from_secs(20);
    let answer = loop {
        assert!(Instant::now() < deadline, "the daemon never answered");
        let mut length = [0u8; 4];
        socket
            .read_exact(&mut length)
            .expect("an answer, not a hang");
        let mut frame = vec![0u8; u32::from_be_bytes(length) as usize];
        socket.read_exact(&mut frame).unwrap();
        let message: Value = serde_json::from_slice(&frame).unwrap();
        assert_ne!(
            message["payload"]["event"]["kind"], "ask",
            "an old client was asked"
        );
        if message["payload"]["type"] == "response" {
            break message;
        }
    };
    assert_eq!(answer["payload"]["status"], "error", "{answer}");
    assert!(
        answer["payload"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("pass -y"),
        "{answer}"
    );
    assert_eq!(summaries(&env), 0);
    assert_eq!(typesafe.calls(), 0);
}

#[test]
fn transcript_and_memory_saves_wait_for_a_running_sync_pass() {
    let typesafe = FakeTypeSafe::start();
    let env = with_models(&typesafe);
    env.fake().state().memories = Some(memories());
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    env.index_db()
        .execute("delete from transcripts where id = 'a-junk'", [])
        .unwrap();
    // A slow pass holds the pass lock for about three seconds.
    env.fake().state().list_delay_ms = 1500;
    let mut sync = env.std_cmd().arg("sync").spawn().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    let started = Instant::now();
    // No Jev key: the guard only downloads (and saves) the transcript.
    let output = env
        .cmd()
        .env_remove("TYPESAFE_API_KEY")
        .args(["delete", "a-junk", "--check", "-n"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() > Duration::from_millis(1500),
        "the transcript save waited"
    );
    sync.wait().unwrap();

    let mut sync = env.std_cmd().arg("sync").spawn().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    let started = Instant::now();
    let output = env
        .cmd()
        .args(["memory", "classify", "--format", "ids"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() > Duration::from_millis(1500),
        "the memory saves waited"
    );
    sync.wait().unwrap();
}

/// Bump a chat's update time in the fake ChatGPT, optionally with new text.
fn touch(env: &Env, id: &str, time: &str, text: Option<&str>) {
    let mut state = env.fake().state();
    let chat = state.chats.iter_mut().find(|c| c.id == id).unwrap();
    chat.update_time = time.into();
    if let Some(text) = text {
        chat.text = text.into();
    }
}

#[test]
fn a_late_judgment_lands_current_when_a_sync_moved_the_unchanged_chat() {
    let typesafe = FakeTypeSafe::start();
    let env = with_models(&typesafe);
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    typesafe.state().delay_ms = 3000;
    let classify = env
        .std_cmd()
        .args(["classify", "a-junk", "-y"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(800));
    // Its update time moves; its content doesn't. The sync's reconcile
    // moves the caches forward while Jev is answering.
    touch(&env, "a-junk", "2024-03-02T10:00:00.000000Z", None);
    let synced = env.cmd().arg("sync").output().unwrap();
    assert!(
        String::from_utf8_lossy(&synced.stderr).contains("Preserved 1 unchanged cache(s)"),
        "{}",
        String::from_utf8_lossy(&synced.stderr)
    );
    let output = classify.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows = list_json(&env);
    assert_eq!(
        row(&rows, "a-junk")["jev"]["suggestion"],
        "delete",
        "the verdict is current at the new time"
    );
}

#[test]
fn a_late_judgment_of_changed_content_is_not_saved() {
    let typesafe = FakeTypeSafe::start();
    let env = with_models(&typesafe);
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    typesafe.state().delay_ms = 3000;
    let classify = env
        .std_cmd()
        .args(["classify", "a-junk", "-y"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(800));
    touch(
        &env,
        "a-junk",
        "2024-03-02T10:00:00.000000Z",
        Some("Rewritten"),
    );
    env.cmd().arg("sync").assert().success();
    let output = classify.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("failed: a-junk Junk ping: the chat changed while it was being classified; run classify again\n"),
        "{stderr}"
    );
    let judgments: i64 = env
        .index_db()
        .query_row(
            "select count(*) from judgments where id = 'a-junk'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(judgments, 0, "no verdict from the old content");
}

#[test]
fn a_late_title_lands_current_when_a_sync_moved_the_unchanged_chat() {
    let typesafe = FakeTypeSafe::start();
    let env = with_models(&typesafe);
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    env.set_tools("slowok", &["codex"]);
    let titles = env
        .std_cmd()
        .args(["titles", "b-maybe"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(800));
    touch(&env, "b-maybe", "2024-03-02T09:00:00.000000Z", None);
    env.cmd().arg("sync").assert().success();
    let output = titles.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows = list_json(&env);
    assert_eq!(
        row(&rows, "b-maybe")["display_title"],
        "Luna: Maybe receipt"
    );
}

#[test]
fn a_broken_fallback_key_never_blocks_a_working_summariser() {
    let typesafe = FakeTypeSafe::start();
    let openai = FakeModelApi::openai();
    let mut env = with_models(&typesafe);
    env.extra_env.extend([
        ("OPENAI_API_KEY".to_owned(), OPENAI_KEY.to_owned()),
        ("CHATGPT_TEST_OPENAI_URL".to_owned(), openai.url.clone()),
        // A key no HTTP header can carry.
        (
            "ANTHROPIC_API_KEY".to_owned(),
            "ant-SENTINEL\nbroken".to_owned(),
        ),
    ]);
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    let (code, _, stderr) = run(&env, &["classify", "e-long", "-y"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stderr.contains("  summaries (gpt-6-luna (API))"),
        "{stderr}"
    );
    assert!(!stderr.contains("SENTINEL"), "{stderr}");

    // The broken one fails as a summariser would, unseen, and the chain
    // goes on.
    env.index_db().execute("delete from summaries", []).unwrap();
    openai.state().fail = Some(500);
    let (_, _, stderr) = run(&env, &["classify", "e-long", "--redo", "-y"]);
    assert!(
        stderr.contains("No summariser succeeded. gpt-6-luna (API): 127.0.0.1 returned 500 | claude-haiku (API): the Anthropic API key has characters an HTTP header can't carry (a line break?)"),
        "{stderr}"
    );
    assert!(!stderr.contains("SENTINEL"), "{stderr}");
}
