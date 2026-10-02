#![allow(clippy::unwrap_used)]

use rusqlite::Connection;

use super::*;

const V: ChunkVersions = ChunkVersions {
    render: 2,
    chunk: 1,
};

fn chat(id: &str, title: &str, update_time: &str, archived: bool) -> NewConversation {
    NewConversation {
        id: id.into(),
        title: title.into(),
        create_time: "2024-01-01T00:00:00Z".into(),
        update_time: update_time.into(),
        is_archived: archived,
        pinned: false,
        project_id: None,
    }
}

fn target(id: &str, title: &str, update_time: &str) -> Unindexed {
    Unindexed {
        id: id.into(),
        title: title.into(),
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

fn ids(rows: &[LexicalRow]) -> Vec<&str> {
    rows.iter().map(|row| row.id.as_str()).collect()
}

#[test]
fn chunks_are_searchable_only_while_current_for_their_chat() {
    let (_dir, store) = store_with(&[
        chat("a", "Cafe plans", "2026-01-02T00:00:00Z", false),
        chat("b", "Other", "2026-01-01T00:00:00Z", true),
    ]);
    let bodies = |text: &str| vec![text.as_bytes().to_vec()];
    store
        .write(|db| {
            replace_chunks(
                db,
                &target("a", "Cafe plans", "2026-01-02T00:00:00Z"),
                V,
                &bodies("Meet at the café on Tuesday about rust"),
            )?;
            replace_chunks(
                db,
                &target("b", "Other", "2026-01-01T00:00:00Z"),
                V,
                &bodies("rust everywhere and more rust"),
            )
        })
        .unwrap();

    // remove_diacritics: "cafe" finds "café"; the porter stemmer and scope.
    let found = store
        .read(|db| lexical(db, "\"cafe\"", Some(false), V, 200))
        .unwrap();
    assert_eq!(ids(&found), ["a"]);
    assert_eq!(
        String::from_utf8(found[0].snippet.clone()).unwrap(),
        "Meet at the café on Tuesday about rust"
    );
    let both = store
        .read(|db| lexical(db, "\"rust\"", None, V, 200))
        .unwrap();
    assert_eq!(ids(&both), ["b", "a"], "more matches rank first");
    assert!(both[0].bm25 < both[1].bm25);
    assert_eq!(store.read(|db| coverage(db, None, V)).unwrap(), (2, 2));
    assert!(store.read(|db| unindexed(db, V)).unwrap().is_empty());

    // A changed chat hides its old chunks and needs indexing again.
    store
        .write(|db| {
            apply_delta(
                db,
                &[chat("a", "Cafe plans", "2026-02-01T00:00:00Z", false)],
                &[],
                "t2",
            )
        })
        .unwrap();
    let found = store
        .read(|db| lexical(db, "\"cafe\"", None, V, 200))
        .unwrap();
    assert!(found.is_empty());
    assert_eq!(
        store.read(|db| coverage(db, Some(false), V)).unwrap(),
        (1, 0)
    );
    let stale = store.read(|db| unindexed(db, V)).unwrap();
    assert_eq!(stale.len(), 1);
    assert!(!stale[0].cached);

    // Chats gone from the index lose their chunks.
    store
        .write(|db| {
            replace_all(
                db,
                &[chat("a", "Cafe plans", "2026-02-01T00:00:00Z", false)],
                "t3",
            )
        })
        .unwrap();
    assert_eq!(store.write(prune_search).unwrap(), 1);
    let left: i64 = store
        .read(|db| {
            Ok(db.query_row(
                "select count(*) from search_fts where search_fts match 'everywhere'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(left, 0, "the FTS index follows its content table");
}

#[test]
fn chunk_bytes_that_are_not_utf8_are_stored_and_read_as_they_are() {
    let (_dir, store) = store_with(&[chat("a", "T", "2026-01-01T00:00:00Z", false)]);
    // What Bun writes for a chunk ending in half of a surrogate pair.
    let body = b"hello rust \xED\xA0\xBD".to_vec();
    store
        .write(|db| {
            replace_chunks(
                db,
                &target("a", "T", "2026-01-01T00:00:00Z"),
                V,
                std::slice::from_ref(&body),
            )
        })
        .unwrap();
    let found = store
        .read(|db| lexical(db, "\"rust\"", None, V, 200))
        .unwrap();
    assert_eq!(found[0].snippet, body);
}

#[test]
fn a_cached_transcript_is_found_for_its_update_time_only() {
    let (_dir, store) = store_with(&[chat("a", "T", "2026-01-01T00:00:00Z", false)]);
    let saved = Transcript {
        id: "a".into(),
        update_time: "2026-01-01T00:00:00Z".into(),
        render_version: 2,
        markdown: "# T\n\n---\n\n## Me\n\nhi\n".into(),
        turns: 1,
        approx_tokens: 5,
    };
    store
        .write(|db| {
            save_indexed(
                db,
                &saved,
                &target("a", "T", &saved.update_time),
                V,
                &[b"## Me\n\nhi".to_vec()],
            )
        })
        .unwrap();
    assert_eq!(
        store
            .read(|db| transcript(db, "a", "2026-01-01T00:00:00Z", 2))
            .unwrap(),
        Some(saved)
    );
    assert_eq!(
        store
            .read(|db| transcript(db, "a", "2026-01-01T00:00:00Z", 3))
            .unwrap(),
        None
    );
    assert_eq!(store.read(|db| coverage(db, None, V)).unwrap(), (1, 1));
}

#[test]
fn reconcile_moves_search_chunks_with_the_other_caches() {
    let old = "2026-09-01T00:00:00.000Z";
    let new = "2026-09-28T00:00:00.000Z";
    let (_dir, store) = store_with(&[chat("a", "Idea", new, false)]);
    store
        .write(|db| {
            db.execute(
                "insert into transcripts values ('a', ?, 2, '# Idea', 1, 1)",
                [old],
            )?;
            replace_chunks(db, &target("a", "Idea", old), V, &[b"an idea".to_vec()])
        })
        .unwrap();
    assert!(
        store
            .read(|db| lexical(db, "\"idea\"", None, V, 200))
            .unwrap()
            .is_empty()
    );
    let found = store
        .read(|db| candidates(db, &["a".to_owned()], 2))
        .unwrap();
    assert!(store.write(|db| preserve(db, &found[0])).unwrap());
    assert_eq!(
        ids(&store
            .read(|db| lexical(db, "\"idea\"", None, V, 200))
            .unwrap()),
        ["a"]
    );
    assert!(store.read(|db| unindexed(db, V)).unwrap().is_empty());
}

#[test]
fn the_import_keeps_transcripts_the_daemon_fetched() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("chatgpt.db")).unwrap();
    let path = dir.path().join("ts.db");
    let legacy = Connection::open(&path).unwrap();
    legacy
        .execute_batch(include_str!("../migrations/0001_index.sql"))
        .unwrap();
    store
        .write(|db| {
            replace_all(
                db,
                &[
                    chat("current", "T", "t2", false),
                    chat("stale", "T", "t2", false),
                    chat("mine", "T", "t2", false),
                ],
                "t",
            )?;
            db.execute_batch(
                "insert into transcripts values ('current', 't2', 2, 'daemon', 1, 1);
                 insert into transcripts values ('stale', 't1', 2, 'daemon old', 1, 1);
                 insert into transcripts values ('mine', 't2', 2, 'daemon only', 1, 1);",
            )?;
            Ok(())
        })
        .unwrap();
    legacy
        .execute_batch(
            "insert into transcripts values ('current', 't2', 2, 'ts', 1, 1);
             insert into transcripts values ('stale', 't2', 2, 'ts new', 1, 1);
             insert into transcripts values ('new', 't2', 2, 'ts only', 1, 1);",
        )
        .unwrap();
    store.write(|db| import_legacy(db, &path)).unwrap();
    let rows: Vec<(String, String)> = store
        .read(|db| {
            let mut statement = db.prepare("select id, markdown from transcripts order by id")?;
            Ok(statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?)
        })
        .unwrap();
    let rows: Vec<(&str, &str)> = rows
        .iter()
        .map(|(id, markdown)| (id.as_str(), markdown.as_str()))
        .collect();
    assert_eq!(
        rows,
        [
            ("current", "daemon"),
            ("mine", "daemon only"),
            ("new", "ts only"),
            ("stale", "ts new"),
        ]
    );
}

// The review's scenario: the indexer backs off while a chat moves from t1
// to t2; its cached transcript and chunks say "oldword" at t1. An import
// then replaces the transcript with "newword", still at t1, and the
// reconcile verifies that replacement against a fresh render and moves the
// caches to t2. The old chunks must not ride along as current.
#[test]
fn chunks_never_outlive_the_transcript_they_were_built_from() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("chatgpt.db")).unwrap();
    let path = dir.path().join("ts.db");
    let legacy = Connection::open(&path).unwrap();
    legacy
        .execute_batch(include_str!("../migrations/0001_index.sql"))
        .unwrap();
    store
        .write(|db| {
            replace_all(db, &[chat("a", "Idea", "t1", false)], "t")?;
            db.execute(
                "insert into transcripts values ('a', 't1', 2, '# Idea\n\n---\n\noldword', 1, 1)",
                [],
            )?;
            replace_chunks(db, &target("a", "Idea", "t1"), V, &[b"oldword".to_vec()])?;
            // The chat moves on; the indexer is backing off.
            apply_delta(db, &[chat("a", "Idea", "t2", false)], &[], "t2")
        })
        .unwrap();
    legacy
        .execute(
            "insert into transcripts values ('a', 't1', 2, '# Idea\n\n---\n\nnewword', 1, 1)",
            [],
        )
        .unwrap();
    store.write(|db| import_legacy(db, &path)).unwrap();
    let found = store
        .read(|db| candidates(db, &["a".to_owned()], 2))
        .unwrap();
    assert_eq!(found[0].markdown, "# Idea\n\n---\n\nnewword");
    assert!(store.write(|db| preserve(db, &found[0])).unwrap());

    assert!(
        store
            .read(|db| lexical(db, "\"oldword\"", None, V, 200))
            .unwrap()
            .is_empty(),
        "chunks of the replaced transcript are gone"
    );
    let todo = store.read(|db| unindexed(db, V)).unwrap();
    assert_eq!(todo.len(), 1, "the indexer sees the chat to redo");
    assert!(
        todo[0].cached,
        "from the verified transcript, without fetching"
    );
}

#[test]
fn moving_a_transcript_forward_keeps_its_chunks() {
    let (_dir, store) = store_with(&[chat("a", "Idea", "t2", false)]);
    store
        .write(|db| {
            db.execute(
                "insert into transcripts values ('a', 't1', 2, '# Idea', 1, 1)",
                [],
            )?;
            replace_chunks(db, &target("a", "Idea", "t1"), V, &[b"an idea".to_vec()])?;
            // Same content, only the time: the reconcile's kind of write.
            db.execute(
                "update transcripts set update_time = 't2' where id = 'a'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let left: i64 = store
        .read(|db| Ok(db.query_row("select count(*) from search_chunks", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(left, 1);
}

#[test]
fn reconcile_skips_a_transcript_replaced_after_it_was_verified() {
    let (_dir, store) = store_with(&[chat("a", "Idea", "t2", false)]);
    store
        .write(|db| {
            db.execute(
                "insert into transcripts values ('a', 't1', 2, '# Idea A', 1, 1)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    // The reconcile snapshots A and checks it against ChatGPT…
    let found = store
        .read(|db| candidates(db, &["a".to_owned()], 2))
        .unwrap();
    // …while an import replaces it with B under the same time.
    store
        .write(|db| {
            db.execute(
                "update transcripts set markdown = '# Idea B' where id = 'a'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(!store.write(|db| preserve(db, &found[0])).unwrap());
    let time: String = store
        .read(|db| Ok(db.query_row("select update_time from transcripts", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(time, "t1", "B stays stale");
}

#[test]
fn the_import_never_replaces_a_current_transcript_with_an_older_one() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("chatgpt.db")).unwrap();
    let path = dir.path().join("ts.db");
    let legacy = Connection::open(&path).unwrap();
    legacy
        .execute_batch(include_str!("../migrations/0001_index.sql"))
        .unwrap();
    store
        .write(|db| {
            replace_all(
                db,
                &[
                    chat("newer-render-older-time", "T", "t2", false),
                    chat("newer-render-current", "T", "t2", false),
                    chat("both-stale-older", "T", "t3", false),
                    chat("both-stale-newer", "T", "t3", false),
                ],
                "t",
            )?;
            db.execute_batch(
                "insert into transcripts values ('newer-render-older-time', 't2', 2, 'daemon', 1, 1);
                 insert into transcripts values ('newer-render-current', 't2', 2, 'daemon', 1, 1);
                 insert into transcripts values ('both-stale-older', 't2', 2, 'daemon', 1, 1);
                 insert into transcripts values ('both-stale-newer', 't1', 2, 'daemon', 1, 1);",
            )?;
            Ok(())
        })
        .unwrap();
    legacy
        .execute_batch(
            "insert into transcripts values ('newer-render-older-time', 't1', 3, 'ts', 1, 1);
             insert into transcripts values ('newer-render-current', 't2', 3, 'ts', 1, 1);
             insert into transcripts values ('both-stale-older', 't1', 2, 'ts', 1, 1);
             insert into transcripts values ('both-stale-newer', 't2', 2, 'ts', 1, 1);",
        )
        .unwrap();
    store.write(|db| import_legacy(db, &path)).unwrap();
    let rows: Vec<(String, String)> = store
        .read(|db| {
            let mut statement = db.prepare("select id, markdown from transcripts order by id")?;
            Ok(statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?)
        })
        .unwrap();
    let rows: Vec<(&str, &str)> = rows
        .iter()
        .map(|(id, markdown)| (id.as_str(), markdown.as_str()))
        .collect();
    assert_eq!(
        rows,
        [
            ("both-stale-newer", "ts"),
            ("both-stale-older", "daemon"),
            ("newer-render-current", "ts"),
            ("newer-render-older-time", "daemon"),
        ]
    );
}
