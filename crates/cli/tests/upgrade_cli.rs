//! An index made by v0.1.5 upgrades in place. `fixtures/v0.1.5.db` was made
//! by the v0.1.5 debug build against the fake chatgpt.com (18 rich chats,
//! synced, indexed and embedded with the stand-in embedder, a manual title,
//! judgments of every kind, a summary and a memory classification), and
//! `fixtures/v0.1.5-list.json` is what that build's `list --json --all`
//! printed for it. Nothing in it is anyone's real data.
//!
//! The new daemon must keep every row, drop `native_rows` (migration 6),
//! and rebuild every search chunk at `CHUNK_VERSION` 2 from the cached
//! transcripts, then embed them, without the network: this environment has
//! no browser session and no reachable chatgpt.com.

#![allow(clippy::unwrap_used)]

mod support;

use rusqlite::Connection;
use serde_json::Value;
use support::Env;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v0.1.5.db");
const LIST: &str = include_str!("fixtures/v0.1.5-list.json");

fn count(db: &Connection, sql: &str) -> i64 {
    db.query_row(sql, [], |row| row.get(0)).unwrap()
}

/// Every row of the tables a user's work lives in, as text, in a stable
/// order.
fn kept_rows(db: &Connection) -> Vec<String> {
    let mut rows = Vec::new();
    for table in [
        "conversations",
        "meta",
        "local_titles",
        "transcripts",
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
fn an_index_from_0_1_5_keeps_its_data_and_rechunks_without_the_network() {
    let env = Env::new();
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

    let embeddings = env.wait_for_embedder();
    let status = env.status();
    let index = &status["search_index"];
    assert_eq!(index["indexed"], 18, "{index}");
    assert_eq!(index["chats"], 18, "{index}");
    assert_eq!(index["fetched"], 0, "re-chunked from the cache: {index}");
    assert!(embeddings["chunks"].as_u64().unwrap() > 0, "{embeddings}");
    assert_eq!(embeddings["embedded"], embeddings["chunks"], "{embeddings}");
    // No pass could succeed: there's no session here, and chatgpt.com
    // points at a closed port.
    assert!(status["sync"]["last_summary"].is_null(), "{status}");

    let db = Connection::open(&database).unwrap();
    assert_eq!(count(&db, "pragma user_version"), 6);
    assert_eq!(
        count(
            &db,
            "select count(*) from sqlite_schema where name = 'native_rows'"
        ),
        0
    );
    assert_eq!(kept_rows(&db), before, "every row kept as it was");
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
