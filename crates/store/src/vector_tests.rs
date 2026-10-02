#![allow(clippy::unwrap_used)]

use rusqlite::Connection;

use super::*;

const V: ChunkVersions = ChunkVersions {
    render: 2,
    chunk: 1,
};
const MODEL: &str = "model@1";

fn chat(id: &str, update_time: &str, archived: bool) -> NewConversation {
    NewConversation {
        id: id.into(),
        title: format!("Title {id}"),
        create_time: "2024-01-01T00:00:00Z".into(),
        update_time: update_time.into(),
        is_archived: archived,
        pinned: false,
        project_id: None,
    }
}

fn target(id: &str, update_time: &str) -> Unindexed {
    Unindexed {
        id: id.into(),
        title: format!("Title {id}"),
        update_time: update_time.into(),
        cached: false,
    }
}

fn store_with(chats: &[NewConversation]) -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("chatgpt.db")).unwrap();
    store.write(|db| replace_all(db, chats, "t")).unwrap();
    (dir, store)
}

fn chunk(store: &Store, id: &str, update_time: &str, bodies: &[&str]) {
    let bodies: Vec<String> = bodies.iter().map(|&body| body.to_owned()).collect();
    store
        .write(|db| replace_chunks(db, &target(id, update_time), V, &bodies))
        .unwrap();
}

/// Embed every pending chunk with a vector of its id.
fn embed_all(store: &Store) -> usize {
    embed_all_at(store, V)
}

fn embed_all_at(store: &Store, versions: ChunkVersions) -> usize {
    let pending = store
        .read(|db| pending_vectors(db, versions, MODEL, 0, 1000, 0))
        .unwrap();
    let vectors: Vec<NewVector> = pending
        .into_iter()
        .map(|chunk| NewVector {
            chunk_id: chunk.id,
            embedding: (chunk.id as f32).to_le_bytes().repeat(384),
            text: chunk.text,
        })
        .collect();
    store.write(|db| save_vectors(db, &vectors, MODEL)).unwrap()
}

fn vector_count(store: &Store) -> i64 {
    store
        .read(|db| Ok(db.query_row("select count(*) from search_vectors", [], |r| r.get(0))?))
        .unwrap()
}

#[test]
fn pending_chunks_are_current_ones_without_a_vector_from_this_model() {
    let (_dir, store) = store_with(&[chat("a", "t1", false), chat("b", "t1", true)]);
    chunk(&store, "a", "t1", &["alpha one", "alpha two"]);
    chunk(&store, "b", "t1", &["beta"]);
    let pending = store
        .read(|db| pending_vectors(db, V, MODEL, 0, 10, 0))
        .unwrap();
    assert_eq!(pending.len(), 3);
    // The TS CLI embeds `title || '\n' || body`.
    assert_eq!(pending[0].text, "Title a\nalpha one");
    // Paging resumes after an id.
    let rest = store
        .read(|db| pending_vectors(db, V, MODEL, pending[0].id, 1, 0))
        .unwrap();
    assert_eq!(rest, vec![pending[1].clone()]);

    assert_eq!(embed_all(&store), 3);
    assert!(
        store
            .read(|db| pending_vectors(db, V, MODEL, 0, 10, 0))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .read(|db| vector_coverage(db, Some(false), V, MODEL))
            .unwrap(),
        (2, 2)
    );
    assert_eq!(
        store
            .read(|db| vector_coverage(db, None, V, MODEL))
            .unwrap(),
        (3, 3)
    );
    // Another model's vectors don't count.
    assert_eq!(
        store
            .read(|db| pending_vectors(db, V, "model@2", 0, 10, 0))
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        store
            .read(|db| vector_coverage(db, None, V, "model@2"))
            .unwrap(),
        (3, 0)
    );

    // A chat that changed hides its chunks, and their vectors with them.
    store
        .write(|db| apply_delta(db, &[chat("a", "t2", false)], &[], "t2"))
        .unwrap();
    assert_eq!(
        store
            .read(|db| vector_coverage(db, None, V, MODEL))
            .unwrap(),
        (1, 1)
    );
    let mut seen = Vec::new();
    store
        .read(|db| {
            each_vector(db, None, V, MODEL, |row| {
                seen.push((
                    row.conversation_id.to_owned(),
                    row.archived,
                    row.embedding.len(),
                ));
                Ok(())
            })
        })
        .unwrap();
    assert_eq!(seen, [("b".to_owned(), true, 384 * 4)]);
}

#[test]
fn vectors_go_with_their_chunks() {
    let (_dir, store) = store_with(&[chat("a", "t1", false), chat("b", "t1", false)]);
    chunk(&store, "a", "t1", &["one", "two"]);
    chunk(&store, "b", "t1", &["three"]);
    assert_eq!(embed_all(&store), 3);

    // Re-chunking a chat drops its vectors.
    chunk(&store, "a", "t1", &["one again"]);
    assert_eq!(vector_count(&store), 1);

    // So does a transcript with new content (migration 0003's trigger
    // deletes the chunks, and this one their vectors).
    assert_eq!(embed_all(&store), 1);
    store
        .write(|db| {
            db.execute(
                "insert into transcripts values ('b', 't1', 2, 'new content', 1, 1)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(vector_count(&store), 1);

    // And pruning a chat gone from the index.
    store
        .write(|db| replace_all(db, &[chat("b", "t1", false)], "t2"))
        .unwrap();
    store.write(prune_search).unwrap();
    assert_eq!(vector_count(&store), 0);
}

#[test]
fn a_vector_for_text_the_chunk_no_longer_holds_is_dropped() {
    let (_dir, store) = store_with(&[chat("a", "t1", false)]);
    chunk(&store, "a", "t1", &["old text"]);
    let pending = store
        .read(|db| pending_vectors(db, V, MODEL, 0, 10, 0))
        .unwrap();
    // The indexer replaces the chunk while its vector is being made; the
    // new chunk gets the same id.
    chunk(&store, "a", "t1", &["new text"]);
    let reused = store
        .read(|db| pending_vectors(db, V, MODEL, 0, 10, 0))
        .unwrap();
    assert_eq!(reused[0].id, pending[0].id, "SQLite reused the rowid");
    let stale = NewVector {
        chunk_id: pending[0].id,
        text: pending[0].text.clone(),
        embedding: vec![0; 384 * 4],
    };
    assert_eq!(
        store.write(|db| save_vectors(db, &[stale], MODEL)).unwrap(),
        0
    );
    assert_eq!(vector_count(&store), 0);
    // A deleted chunk gets no vector either.
    let gone = NewVector {
        chunk_id: 999,
        text: "x".to_owned(),
        embedding: vec![0; 384 * 4],
    };
    assert_eq!(
        store.write(|db| save_vectors(db, &[gone], MODEL)).unwrap(),
        0
    );
}

#[test]
fn an_index_from_before_vectors_upgrades_in_place() {
    const V2: ChunkVersions = ChunkVersions {
        render: 2,
        chunk: 2,
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chatgpt.db");
    {
        // 0.1.1's schema, with a chat already chunked (at chunk version 2,
        // which the text migration keeps).
        let mut db = Connection::open(&path).unwrap();
        let transaction = db.transaction().unwrap();
        for sql in [
            include_str!("../migrations/0001_index.sql"),
            include_str!("../migrations/0002_search.sql"),
            include_str!("../migrations/0003_search_follows_transcripts.sql"),
        ] {
            transaction.execute_batch(sql).unwrap();
        }
        transaction.pragma_update(None, "user_version", 3).unwrap();
        transaction.commit().unwrap();
        replace_all(&mut db, &[chat("a", "t1", false)], "t").unwrap();
        replace_chunks(&mut db, &target("a", "t1"), V2, &["kept".to_owned()]).unwrap();
    }
    let store = Store::open(&path).unwrap();
    let version: i64 = store
        .read(|db| Ok(db.query_row("pragma user_version", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(version, 8);
    assert_eq!(store.read(|db| coverage(db, None, V2)).unwrap(), (1, 1));
    assert_eq!(
        store
            .read(|db| lexical(db, "\"kept\"", None, V2, 10))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .read(|db| vector_coverage(db, None, V2, MODEL))
            .unwrap(),
        (1, 0),
        "the existing chunks wait for vectors"
    );
    assert_eq!(embed_all_at(&store, V2), 1);
    let body = store
        .read(|db| {
            let id: i64 = db.query_row("select id from search_chunks", [], |r| r.get(0))?;
            chunk_body(db, id)
        })
        .unwrap();
    assert_eq!(body.as_deref(), Some("kept"));
}

// docs/issues/semantic-search-followups.md: a chunk the model failed on
// stays set aside across restarts, until its retry time.
#[test]
fn a_failed_chunk_waits_until_its_retry_time() {
    let (_dir, store) = store_with(&[chat("a", "t1", false)]);
    chunk(&store, "a", "t1", &["one", "two"]);
    let pending = |now: i64| {
        store
            .read(|db| pending_vectors(db, V, MODEL, 0, 10, now))
            .unwrap()
            .into_iter()
            .map(|chunk| chunk.id)
            .collect::<Vec<_>>()
    };
    let ids = pending(100);
    store
        .write(|db| record_vector_failure(db, ids[0], MODEL, 200))
        .unwrap();
    assert_eq!(pending(100), [ids[1]], "set aside");
    assert_eq!(pending(200), ids, "due again");
    assert_eq!(store.read(|db| vector_failures(db, MODEL, 100)).unwrap(), 1);
    assert_eq!(store.read(|db| vector_failures(db, MODEL, 200)).unwrap(), 0);
    assert_eq!(
        store
            .read(|db| pending_vectors(db, V, "model@2", 0, 10, 100))
            .unwrap()
            .len(),
        2,
        "another model tries it"
    );
    // New chunks for the chat take the failure with the old ones.
    chunk(&store, "a", "t1", &["one", "two"]);
    assert_eq!(store.read(|db| vector_failures(db, MODEL, 100)).unwrap(), 0);
    // And a vector saved for a chunk clears its failure.
    let ids = pending(100);
    store
        .write(|db| record_vector_failure(db, ids[0], MODEL, 200))
        .unwrap();
    assert_eq!(embed_all_at(&store, V), 1);
    store
        .write(|db| {
            let text: String = db.query_row(
                "select title || char(10) || body from search_chunks where id = ?",
                [ids[0]],
                |r| r.get(0),
            )?;
            let vector = NewVector {
                chunk_id: ids[0],
                text,
                embedding: vec![0; 384 * 4],
            };
            save_vectors(db, &[vector], MODEL)
        })
        .unwrap();
    assert_eq!(store.read(|db| vector_failures(db, MODEL, 100)).unwrap(), 0);
}
