//! An index made by v0.1.5 upgrades in place. `fixtures/v0.1.5.db` was made
//! by the v0.1.5 debug build against the fake chatgpt.com (18 rich chats,
//! synced, indexed and embedded with the stand-in embedder, a manual title,
//! judgments of every kind, a summary and a memory classification), and
//! `fixtures/v0.1.5-list.json` is what that build's `list --json --all`
//! printed for it. Nothing in it is anyone's real data.
//!
//! The new daemon must keep every row, drop `native_rows` (migration 6)
//! and the chunks that may not be UTF-8 (migration 7). Its transcripts
//! were rendered at version 2, which render version 3 (canvas edits by
//! UTF-16 unit) makes stale, so the indexer fetches each chat once more
//! through the batch endpoint, from the same fake chatgpt.com the fixture
//! came from (`rich_chats(18, 0x5eed)`), then chunks and embeds them.

#![allow(clippy::unwrap_used)]

mod support;

use fake_chatgpt::fixtures::rich_chats;
use rusqlite::Connection;
use serde_json::Value;
use support::Env;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v0.1.5.db");
const LIST: &str = include_str!("fixtures/v0.1.5-list.json");

fn count(db: &Connection, sql: &str) -> i64 {
    db.query_row(sql, [], |row| row.get(0)).unwrap()
}

/// Every row of the tables a user's work lives in, as text, in a stable
/// order. Transcripts are a cache the upgrade fetches again, and
/// `synced_at` moves with the sync the new daemon runs.
fn kept_rows(db: &Connection) -> Vec<String> {
    let mut rows = Vec::new();
    for table in [
        "conversations",
        "meta",
        "local_titles",
        "summaries",
        "judgments",
        "deep_judgments",
        "luna_judgments",
        "memory_judgments",
    ] {
        let mut statement = db
            .prepare(&format!("select * from {table} order by 1"))
            .unwrap();
        let columns = statement.column_count();
        let mut query = statement.query([]).unwrap();
        while let Some(row) = query.next().unwrap() {
            let values: Vec<String> = (0..columns)
                .map(|i| format!("{:?}", row.get::<_, rusqlite::types::Value>(i).unwrap()))
                .collect();
            rows.push(format!("{table}: {}", values.join(" | ")));
        }
    }
    rows.retain(|row| !row.starts_with("meta: Text(\"synced_at\")"));
    rows
}

fn non_utf8_chunks(db: &Connection) -> i64 {
    let mut statement = db.prepare("select body from search_chunks").unwrap();
    let bodies: Vec<Vec<u8>> = statement
        .query_map([], |row| {
            Ok(match row.get_ref(0)? {
                rusqlite::types::ValueRef::Text(bytes) | rusqlite::types::ValueRef::Blob(bytes) => {
                    bytes.to_vec()
                }
                _ => Vec::new(),
            })
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    bodies
        .iter()
        .filter(|body| std::str::from_utf8(body).is_err())
        .count() as i64
}

#[test]
fn an_index_from_0_1_5_keeps_its_data_and_fetches_its_transcripts_again() {
    let env = Env::with_fake(rich_chats(18, 0x5eed));
    std::fs::create_dir_all(env.data_dir()).unwrap();
    let database = env.data_dir().join("chatgpt.db");
    std::fs::copy(FIXTURE, &database).unwrap();
    let before = {
        let db = Connection::open(&database).unwrap();
        assert_eq!(count(&db, "pragma user_version"), 5);
        assert_eq!(count(&db, "select count(*) from native_rows"), 1);
        assert_eq!(
            count(
                &db,
                "select count(*) from search_chunks where chunk_version = 1"
            ),
            count(&db, "select count(*) from search_chunks")
        );
        assert!(
            non_utf8_chunks(&db) > 0,
            "the fixture has Bun's bytes for half an emoji"
        );
        kept_rows(&db)
    };

    // The first command starts the new daemon, which migrates the index;
    // `list` answers from it as v0.1.5 did.
    let listed: Value = serde_json::from_str(&env.stdout(&["list", "--json", "--all"])).unwrap();
    let expected: Value = serde_json::from_str(LIST).unwrap();
    assert_eq!(listed, expected, "list --json --all as v0.1.5 printed it");

    // The indexer fetches once the daemon's first sync pass has opened the
    // session; until then it has nothing it may do.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while env.status()["search_index"]["indexed"] != 18 {
        assert!(std::time::Instant::now() < deadline, "{}", env.status());
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let embeddings = env.wait_for_embedder();
    let status = env.status();
    let index = &status["search_index"];
    assert_eq!(index["indexed"], 18, "{index}");
    assert_eq!(index["chats"], 18, "{index}");
    assert_eq!(
        index["fetched"], 18,
        "render 2 transcripts are stale: {index}"
    );
    assert!(embeddings["chunks"].as_u64().unwrap() > 0, "{embeddings}");
    assert_eq!(embeddings["embedded"], embeddings["chunks"], "{embeddings}");

    let db = Connection::open(&database).unwrap();
    assert_eq!(count(&db, "pragma user_version"), 8);
    assert_eq!(
        count(
            &db,
            "select count(*) from sqlite_schema where name = 'native_rows'"
        ),
        0
    );
    assert_eq!(kept_rows(&db), before, "every row kept as it was");
    assert_eq!(
        count(
            &db,
            "select count(*) from transcripts where render_version = 3"
        ),
        18
    );
    assert_eq!(count(&db, "select count(*) from transcripts"), 18);
    let chunks = count(&db, "select count(*) from search_chunks");
    assert!(chunks > 0);
    assert_eq!(
        count(
            &db,
            "select count(*) from search_chunks where chunk_version = 2"
        ),
        chunks
    );
    assert_eq!(non_utf8_chunks(&db), 0, "no half emoji any more");
    assert_eq!(count(&db, "select count(*) from search_vectors"), chunks);
    drop(db);

    let found = env.stdout(&["search", "garden", "--all", "--format", "ids"]);
    assert!(!found.is_empty(), "lexical search answers");
}
