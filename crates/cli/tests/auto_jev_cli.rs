//! The daemon's background Jev (D4): after a successful sync pass it judges
//! chats that are new or changed since it was first enabled, with the Jev
//! key in the user config only, at most so many a pass, and never runs the
//! follow-up, Luna or a summary. `daemon status` shows it.

#![allow(clippy::unwrap_used)]

mod support;

use std::time::{Duration, Instant};

use fake_chatgpt::Chat;
use fake_chatgpt::typesafe::{API_KEY, FakeTypeSafe};
use serde_json::Value;
use support::Env;

fn chats() -> Vec<Chat> {
    vec![
        Chat::new("a-junk", "Junk ping", "2026-09-01T10:00:00.000000Z"),
        Chat::new("b-idea", "Book idea", "2026-09-01T09:00:00.000000Z"),
    ]
}

fn env_with(typesafe: &FakeTypeSafe, config: Option<&str>) -> Env {
    let mut env = Env::with_fake(chats());
    env.extra_env
        .push(("TYPESAFE_BASE_URL".into(), typesafe.url.clone()));
    if let Some(config) = config {
        let dir = env.home.path().join("xdg-config/chatgpt-cli");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), config).unwrap();
    }
    env
}

fn key_config() -> String {
    format!("{{\"jev\":\"{API_KEY}\"}}")
}

/// `daemon status`'s background Jev once no run is going on.
fn settled(env: &Env) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = env.status()["auto_jev"].clone();
        if status["in_progress"] == false {
            return status;
        }
        assert!(Instant::now() < deadline, "never settled: {status}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn add_chat(env: &Env, chat: Chat) {
    env.fake().state().chats.push(chat);
}

fn judged(env: &Env) -> Vec<String> {
    let rows: Vec<Value> = serde_json::from_str(&env.stdout(&["list", "--json"])).unwrap();
    let mut ids: Vec<String> = rows
        .iter()
        .filter(|row| row.get("jev").is_some())
        .map(|row| row["id"].as_str().unwrap().to_owned())
        .collect();
    ids.sort();
    ids
}

#[test]
fn new_and_changed_chats_get_jev_in_the_background_after_a_pass() {
    let typesafe = FakeTypeSafe::start();
    let env = env_with(&typesafe, Some(&key_config()));
    env.cmd().arg("sync").assert().success();
    let status = settled(&env);
    assert_eq!(status["enabled"], true, "{status}");
    // The history it found when first enabled isn't judged.
    assert_eq!(typesafe.calls(), 0);
    assert!(judged(&env).is_empty());

    add_chat(
        &env,
        Chat::new("c-new", "Junk new", "2026-10-01T10:00:00.000000Z"),
    );
    {
        let mut state = env.fake().state();
        let idea = state.chats.iter_mut().find(|c| c.id == "b-idea").unwrap();
        idea.update_time = "2026-10-01T11:00:00.000000Z".into();
        idea.text = "Changed".into();
    }
    env.cmd().arg("sync").assert().success();
    let status = settled(&env);
    assert_eq!(judged(&env), ["b-idea", "c-new"]);
    assert_eq!(typesafe.calls(), 2);
    assert_eq!(status["judged_today"], 2, "{status}");
    assert!(status["cost_today_usd"].as_f64().unwrap() > 0.0, "{status}");
    assert!(
        status["last_summary"]
            .as_str()
            .unwrap()
            .starts_with("judged 2 of 2 new or changed chat(s), $0.0001"),
        "{status}"
    );
    let text = env.stdout(&["daemon", "status"]);
    assert!(
        text.contains("background Jev: on, 2 judged today ($0.0001)"),
        "{text}"
    );
    // Only Jev's first pass: no follow-up questions, no Luna, no summaries.
    assert!(
        typesafe
            .state()
            .bodies
            .iter()
            .all(|body| body.contains("\"worth_keeping\"")),
    );
    assert!(env.model_calls().is_empty());

    // Judged chats aren't judged again by the next pass.
    env.cmd().arg("sync").assert().success();
    settled(&env);
    assert_eq!(typesafe.calls(), 2);
}

#[test]
fn each_pass_judges_at_most_its_bound_and_never_summarises() {
    let typesafe = FakeTypeSafe::start();
    let mut env = env_with(&typesafe, Some(&key_config()));
    env.extra_env.extend([
        ("CHATGPT_TEST_AUTO_JEV_LIMIT".to_owned(), "2".to_owned()),
        // Everything counts as new.
        ("CHATGPT_TEST_AUTO_JEV_SINCE".to_owned(), "2000".to_owned()),
    ]);
    for n in 0..3 {
        add_chat(
            &env,
            Chat::new(
                &format!("n-{n}"),
                "Junk",
                &format!("2026-09-0{}T10:00:00.000000Z", n + 2),
            ),
        );
    }
    let mut long = Chat::new("l-long", "Long junk", "2026-08-01T10:00:00.000000Z");
    long.text = "word ".repeat(12_000);
    add_chat(&env, long);
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    settled(&env);
    // Two a pass, newest first, then the rest the next pass.
    assert_eq!(judged(&env), ["n-1", "n-2"]);
    env.cmd().arg("sync").assert().success();
    settled(&env);
    assert_eq!(judged(&env), ["a-junk", "n-0", "n-1", "n-2"]);
    env.cmd().arg("sync").assert().success();
    let status = settled(&env);
    assert_eq!(judged(&env), ["a-junk", "b-idea", "n-0", "n-1", "n-2"]);
    // The long chat has no summary: it waits for `classify`, and never
    // takes a slot.
    env.cmd().arg("sync").assert().success();
    settled(&env);
    assert!(!judged(&env).contains(&"l-long".to_owned()));
    assert_eq!(typesafe.calls(), 5);
    assert!(env.model_calls().is_empty(), "nothing summarised");
    assert_eq!(status["judged_today"], 5, "{status}");
}

#[test]
fn it_is_off_without_a_configured_key_or_when_switched_off() {
    let typesafe = FakeTypeSafe::start();
    // A key in the requesting command's environment isn't enough.
    let mut env = env_with(&typesafe, None);
    env.extra_env.extend([
        ("TYPESAFE_API_KEY".to_owned(), API_KEY.to_owned()),
        ("CHATGPT_TEST_AUTO_JEV_SINCE".to_owned(), "2000".to_owned()),
    ]);
    env.cmd().arg("sync").assert().success();
    let status = settled(&env);
    assert_eq!(status["enabled"], false);
    assert_eq!(
        status["off_reason"],
        "no Jev key in the user config (`chatgpt configure jev`)"
    );
    assert!(
        env.stdout(&["daemon", "status"]).contains(
            "background Jev: off (no Jev key in the user config (`chatgpt configure jev`))"
        )
    );
    assert_eq!(typesafe.calls(), 0);

    // Configured, but switched off.
    let dir = env.home.path().join("xdg-config/chatgpt-cli");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.json"),
        format!("{{\"jev\":\"{API_KEY}\",\"auto_jev\":false}}"),
    )
    .unwrap();
    env.cmd().arg("sync").assert().success();
    let status = settled(&env);
    assert_eq!(status["enabled"], false);
    assert_eq!(status["off_reason"], "auto_jev is false in the user config");
    assert_eq!(typesafe.calls(), 0);

    // Switched on again: it runs at the next pass.
    std::fs::write(dir.join("config.json"), key_config()).unwrap();
    env.cmd().arg("sync").assert().success();
    settled(&env);
    assert_eq!(typesafe.calls(), 2);
}

#[test]
fn a_command_waits_for_the_background_instead_of_judging_the_same_chats_again() {
    let typesafe = FakeTypeSafe::start();
    typesafe.state().delay_ms = 1500;
    let mut env = env_with(&typesafe, Some(&key_config()));
    env.extra_env.extend([
        ("CHATGPT_TEST_AUTO_JEV_SINCE".to_owned(), "2000".to_owned()),
        ("TYPESAFE_API_KEY".to_owned(), API_KEY.to_owned()),
    ]);
    env.cmd().arg("sync").assert().success();
    // The background Jev is judging both chats now.
    assert_eq!(env.status()["auto_jev"]["in_progress"], true);
    let output = env
        .cmd()
        .args(["classify", "a-junk", "b-idea", "-y"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.starts_with("2 chat(s): 2 already judged, 0 new or changed to judge.\n"),
        "it waited for the background's verdicts: {stderr}"
    );
    settled(&env);
    assert_eq!(typesafe.calls(), 2, "each chat paid for once");
}
