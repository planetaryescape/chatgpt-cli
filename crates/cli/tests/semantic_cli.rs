//! Semantic, hybrid and remote `search`, `search-index`, and the daemon's
//! background embedder, through the real binary against the fake
//! chatgpt.com. The embedder is the fake one (`CHATGPT_TEST_EMBEDDER`), so
//! nothing here needs the model or the internet.

#![allow(clippy::unwrap_used)]

mod support;

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use fake_chatgpt::Chat;
use rusqlite::Connection;
use serde_json::{Value, json};
use support::Env;

fn chats() -> Vec<Chat> {
    let mut rust = Chat::new("a-rust", "Rust runtimes", "2026-09-27T10:00:00.000000Z");
    rust.text = "async rust with tokio and smol runtimes".into();
    let mut garden = Chat::new("b-garden", "Garden plan", "2026-09-26T10:00:00.000000Z");
    garden.text = "compost beds and tomatoes in the garden".into();
    let mut archived = Chat::new(
        "c-archived",
        "Old rust notes",
        "2026-09-20T10:00:00.000000Z",
    );
    archived.archived = true;
    archived.text = "rust borrow checker notes".into();
    vec![rust, garden, archived]
}

/// Many chats with several chunks each, for tests that watch the
/// embedder at work.
fn many_chats() -> Vec<Chat> {
    (0..12)
        .map(|n| {
            let mut chat = Chat::new(
                &format!("chat-{n:02}"),
                &format!("Chat {n}"),
                &format!("2026-09-{:02}T10:00:00.000000Z", n + 1),
            );
            chat.text = format!("topic{n} ").repeat(400);
            chat
        })
        .collect()
}

fn synced(env: &Env) {
    env.cmd().arg("sync").assert().success();
}

fn run(env: &Env, args: &[&str]) -> (Option<i32>, String, String) {
    let output = env.cmd().args(args).output().unwrap();
    (
        output.status.code(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

fn ids(json: &str) -> Vec<String> {
    serde_json::from_str::<Vec<Value>>(json)
        .unwrap()
        .iter()
        .map(|hit| hit["id"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn semantic_and_hybrid_search_need_no_search_index_step() {
    let env = Env::with_fake(chats());
    synced(&env);
    let embeddings = env.wait_for_embedder();
    assert!(embeddings["chunks"].as_u64().unwrap() > 0, "{embeddings}");
    assert_eq!(embeddings["embedded"], embeddings["chunks"], "{embeddings}");

    let (code, stdout, stderr) = run(
        &env,
        &["search", "async rust", "--semantic", "--format", "json"],
    );
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(ids(&stdout), ["a-rust", "b-garden"], "active chats only");
    assert_eq!(stderr, "2 conversation(s) found locally.\n");
    let hits: Vec<Value> = serde_json::from_str(&stdout).unwrap();
    assert!(hits[0]["score"].as_f64().unwrap() > hits[1]["score"].as_f64().unwrap());
    assert_eq!(hits[0]["title"], "Rust runtimes");

    let (_, stdout, _) = run(
        &env,
        &["search", "rust", "--semantic", "--all", "--format", "ids"],
    );
    assert_eq!(stdout.lines().next(), Some("c-archived"), "{stdout}");
    let (_, stdout, _) = run(
        &env,
        &[
            "search",
            "rust",
            "--semantic",
            "--archived",
            "--format",
            "ids",
        ],
    );
    assert_eq!(stdout, "c-archived\n");

    // Hybrid: lexical "garden" matches only b; fused with the semantic list.
    let (code, stdout, stderr) = run(&env, &["search", "garden", "--hybrid", "--format", "json"]);
    assert_eq!(code, Some(0), "{stderr}");
    let hits: Vec<Value> = serde_json::from_str(&stdout).unwrap();
    assert_eq!(hits[0]["id"], "b-garden");
    // In both lists at rank 0: 2/61.
    assert_eq!(hits[0]["score"].as_f64().unwrap(), 1.0 / 61.0 + 1.0 / 61.0);

    let (code, _, stderr) = run(&env, &["search", "x", "--semantic", "--hybrid"]);
    assert_eq!(code, Some(2));
    assert_eq!(
        stderr,
        "error: Choose only one of --semantic, --hybrid, or --remote.\n"
    );

    let status = env.stdout(&["daemon", "status"]);
    let chunks = embeddings["chunks"].as_u64().unwrap();
    assert!(
        status.contains(&format!("embeddings: {chunks} of {chunks} chunks embedded")),
        "{status}"
    );
}

#[test]
fn search_index_waits_for_the_daemon_and_reports_the_scope() {
    let env = Env::with_fake(chats());
    synced(&env);
    let (code, stdout, stderr) = run(&env, &["search-index"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.is_empty());
    let last = stderr.lines().last().unwrap();
    assert!(last.starts_with("Search index in "), "{stderr}");
    assert!(last.contains(": 2/2 chats, "), "{last}");
    let embeddings = env.status()["search_index"]["embeddings"].clone();
    assert_eq!(embeddings["embedded"], embeddings["chunks"]);
    let (_, _, stderr) = run(&env, &["search-index", "--all"]);
    assert!(
        stderr.lines().last().unwrap().contains(": 3/3 chats, "),
        "{stderr}"
    );
    let (_, _, stderr) = run(&env, &["search-index", "--archived"]);
    assert!(
        stderr.lines().last().unwrap().contains(": 1/1 chats, "),
        "{stderr}"
    );
}

#[test]
fn semantic_search_says_how_far_the_embedder_has_got() {
    let mut env = Env::with_fake(many_chats());
    env.extra_env
        .push(("CHATGPT_TEST_EMBED_DELAY_MS".into(), "400".into()));
    synced(&env);
    env.wait_for_indexer();
    let (code, _, stderr) = run(&env, &["search", "topic3", "--semantic"]);
    if code == Some(0) {
        assert!(
            stderr.contains("chunks embedded; the daemon is embedding the rest in the background."),
            "{stderr}"
        );
    } else {
        assert!(
            stderr.starts_with(
                "error: No local embeddings yet: the daemon is embedding in the background (0 of "
            ),
            "{stderr}"
        );
    }
    assert!(!stderr.contains("search-index"), "{stderr}");
}

#[test]
fn embedding_steps_aside_for_a_sync_and_a_query() {
    let mut env = Env::with_fake(many_chats());
    env.extra_env
        .push(("CHATGPT_TEST_EMBED_DELAY_MS".into(), "100".into()));
    synced(&env);
    env.wait_for_indexer();
    let embedded = |env: &Env| {
        env.status()["search_index"]["embeddings"]["embedded"]
            .as_u64()
            .unwrap()
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    while embedded(&env) == 0 {
        assert!(Instant::now() < deadline, "nothing embedded");
        std::thread::sleep(Duration::from_millis(50));
    }

    // A query waits for one chunk at most, not the whole run.
    let started = Instant::now();
    let (code, stdout, stderr) = run(&env, &["search", "topic3", "--semantic", "--format", "ids"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(!stdout.is_empty() || stderr.contains("chunks embedded"));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );

    // While a sync runs, no more chunks are embedded (one may finish).
    env.fake().state().list_delay_ms = 1500;
    let mut sync = env.std_cmd().arg("sync").spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while env.status()["sync"]["in_progress"] != true {
        assert!(Instant::now() < deadline, "the sync never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(200));
    let before = embedded(&env);
    std::thread::sleep(Duration::from_millis(800));
    let during = embedded(&env);
    assert!(
        during <= before + 1,
        "embedded {before} → {during} during a sync"
    );
    assert!(sync.wait().unwrap().success());
    let embeddings = env.wait_for_embedder();
    assert_eq!(embeddings["embedded"], embeddings["chunks"]);
}

#[test]
fn embedding_resumes_where_it_stopped() {
    let env = Env::with_fake(many_chats());
    synced(&env);
    let embeddings = env.wait_for_embedder();
    let chunks = embeddings["chunks"].as_i64().unwrap();
    env.cmd().args(["daemon", "stop"]).assert().success();

    // As if the run had stopped halfway: half the vectors are missing, and
    // the rest are marked so a redo would show.
    let db = Connection::open(env.data_dir().join("chatgpt.db")).unwrap();
    let marker = vec![0x3Fu8; 384 * 4];
    db.execute("update search_vectors set embedding = ?", [&marker])
        .unwrap();
    let deleted = db
        .execute("delete from search_vectors where chunk_id % 2 = 0", [])
        .unwrap() as i64;
    assert!(deleted > 0 && deleted < chunks);
    drop(db);

    let embeddings = env.wait_for_embedder();
    assert_eq!(embeddings["embedded"].as_i64(), Some(chunks));
    let db = Connection::open(env.data_dir().join("chatgpt.db")).unwrap();
    let kept: i64 = db
        .query_row(
            "select count(*) from search_vectors where embedding = ?",
            [&marker],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kept, chunks - deleted, "vectors already made are kept");
}

#[test]
fn an_index_from_0_1_1_upgrades_in_place_and_embeds_in_the_background() {
    let env = Env::with_fake(chats());
    synced(&env);
    env.wait_for_indexer();
    env.cmd().args(["daemon", "stop"]).assert().success();
    // Back to 0.1.1's schema: chunks, no vectors.
    let db = Connection::open(env.data_dir().join("chatgpt.db")).unwrap();
    db.execute_batch(
        "drop trigger search_chunks_delete_vectors; drop table search_vectors; pragma user_version = 3;",
    )
    .unwrap();
    drop(db);

    let (code, stdout, stderr) = run(&env, &["search", "garden"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.starts_with("b-garden"), "{stdout}");
    let embeddings = env.wait_for_embedder();
    assert!(embeddings["chunks"].as_u64().unwrap() > 0);
    assert_eq!(embeddings["embedded"], embeddings["chunks"]);
    let (code, stdout, _) = run(
        &env,
        &["search", "tomatoes", "--semantic", "--format", "ids"],
    );
    assert_eq!(code, Some(0));
    assert_eq!(stdout.lines().next(), Some("b-garden"));
}

/// A server that answers every request with 200 and `body`.
fn serve_forever(body: &'static [u8]) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(body);
        }
    });
    url
}

#[test]
fn without_the_model_lexical_search_works_and_semantic_search_says_why() {
    for (base, why) in [
        (
            "http://127.0.0.1:9".to_owned(),
            "couldn't download onnx/model_quantized.onnx",
        ),
        (
            serve_forever(b"not a model"),
            "the downloaded onnx/model_quantized.onnx doesn't match its pinned checksum",
        ),
    ] {
        let mut env = Env::with_fake(chats());
        // The real embedder, from a model server that fails.
        env.extra_env
            .push(("CHATGPT_TEST_EMBEDDER".into(), String::new()));
        env.extra_env.push(("CHATGPT_MODEL_BASE_URL".into(), base));
        synced(&env);
        let embeddings = env.wait_for_embedder();
        let waiting = embeddings["waiting"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        assert!(
            waiting.starts_with("the embedding model isn't available (") && waiting.contains(why),
            "{embeddings}"
        );
        assert_eq!(embeddings["embedded"], 0);

        let (code, stdout, stderr) = run(&env, &["search", "garden"]);
        assert_eq!(code, Some(0), "{stderr}");
        assert!(stdout.starts_with("b-garden"));

        let (code, stdout, stderr) = run(&env, &["search", "garden", "--semantic"]);
        assert_ne!(code, Some(0));
        assert!(stdout.is_empty());
        assert_eq!(
            stderr,
            format!("error: No local embeddings yet: {waiting}.\n")
        );

        // search-index tries the download again, then says why it stopped.
        let (code, _, stderr) = run(&env, &["search-index"]);
        assert_eq!(code, Some(1), "{stderr}");
        assert!(
            stderr.contains("waiting: the embedding model isn't available ("),
            "{stderr}"
        );
        let status = env.stdout(&["daemon", "status"]);
        assert!(status.contains("isn't available"), "{status}");
        assert!(!std::fs::exists(env.home.path().join(
            "xdg-cache/chatgpt-cli/models/Xenova/all-MiniLM-L6-v2/751bff37182d3f1213fa05d7196b954e230abad9/onnx/model_quantized.onnx"
        ))
        .unwrap());
    }
}

fn search_item(id: &str, title: &str, archived: bool, snippet: &str, update_time: f64) -> Value {
    json!({
        "source_type": "conversation",
        "title": title,
        "snippet": snippet,
        "update_time": update_time,
        "payload": { "conversation_id": id, "is_archived": archived },
    })
}

#[test]
fn remote_search_asks_chatgpt_and_prints_its_results() {
    let env = Env::with_fake(chats());
    synced(&env);
    env.fake().state().search_items = vec![
        json!({ "source_type": "project", "title": "Not a chat" }),
        search_item(
            "a-rust",
            "Rust runtimes (remote)",
            false,
            "  tokio\n\nand  smol ",
            1_758_000_000.123_9,
        ),
        search_item(
            "c-archived",
            "Old rust notes",
            true,
            "borrow",
            1_757_000_000.0,
        ),
        search_item("a-rust", "dup", false, "again", 1_758_000_000.0),
        search_item(
            "z-unknown",
            "Not synced yet",
            false,
            "elsewhere",
            1_756_000_000.5,
        ),
    ];
    let (code, stdout, stderr) = run(&env, &["search", "rust", "--remote", "--limit", "3"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(stderr, "", "no notes for a remote search");
    assert_eq!(
        stdout,
        "a-rust  2025-09-16     Rust runtimes\n     tokio and smol \n\
         z-unknown  2025-08-24     Not synced yet\n    elsewhere\n"
    );
    let body = env.fake().state().search_bodies.last().cloned().unwrap();
    assert_eq!(
        body,
        json!({
            "entrypoint": "global_search", "limit": 15, "query": "rust",
            "source_requests": [{ "type": "conversation" }], "cursor": null,
        })
    );

    let (_, stdout, _) = run(
        &env,
        &["search", "rust", "--remote", "--all", "--format", "json"],
    );
    let hits: Vec<Value> = serde_json::from_str(&stdout).unwrap();
    assert_eq!(hits.len(), 3);
    assert_eq!(hits[0]["score"], Value::Null);
    assert_eq!(hits[0]["updated"], "2025-09-16T05:20:00.123Z");
    assert_eq!(hits[1]["archived"], true);
    assert_eq!(
        env.fake().state().search_bodies.last().unwrap()["limit"],
        20
    );

    let (code, _, stderr) = run(&env, &["search", "rust", "--remote", "--limit", "41"]);
    assert_eq!(code, Some(2));
    assert_eq!(
        stderr,
        "error: --remote supports --limit up to 40 (ChatGPT's search API limit).\n"
    );
}
