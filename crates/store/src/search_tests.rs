#![allow(clippy::unwrap_used)]

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
    let bodies = |text: &str| vec![text.to_owned()];
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
    assert_eq!(found[0].snippet, "Meet at the café on Tuesday about rust");
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
fn chunks_that_may_not_be_utf8_are_dropped_by_the_text_migration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chatgpt.db");
    {
        // An index at migration 6, before the text migration.
        let mut db = rusqlite::Connection::open(&path).unwrap();
        let transaction = db.transaction().unwrap();
        for sql in [
            include_str!("../migrations/0001_index.sql"),
            include_str!("../migrations/0002_search.sql"),
            include_str!("../migrations/0003_search_follows_transcripts.sql"),
            include_str!("../migrations/0004_search_vectors.sql"),
            include_str!("../migrations/0005_native_rows.sql"),
            include_str!("../migrations/0006_drop_native_rows.sql"),
        ] {
            transaction.execute_batch(sql).unwrap();
        }
        transaction.pragma_update(None, "user_version", 6).unwrap();
        transaction.commit().unwrap();
        let chats = [chat("a", "T", "t", false), chat("b", "T", "t", false)];
        replace_all(&mut db, &chats, "t").unwrap();
        let current = ChunkVersions {
            render: 2,
            chunk: 2,
        };
        replace_chunks(
            &mut db,
            &target("b", "T", "t"),
            current,
            &["kept".to_owned()],
        )
        .unwrap();
        // What a chunk from before chunk version 2 can hold: the bytes Bun
        // wrote for half of a surrogate pair.
        db.execute_batch(
            "insert into search_chunks
             (conversation_id, update_time, render_version, chunk_version, chunk_index, title, body)
             values ('a', 't', 2, 1, 0, 'T', cast(x'68656c6c6f20eda0bd' as text));
             insert into search_indexed values ('a', 't', 2, 1);",
        )
        .unwrap();
    }
    let store = Store::open(&path).unwrap();
    let left: Vec<(String, String)> = store
        .read(|db| {
            let mut statement = db.prepare("select conversation_id, body from search_chunks")?;
            let rows = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
        .unwrap();
    assert_eq!(left, [("b".to_owned(), "kept".to_owned())]);
    let indexed: i64 = store
        .read(|db| Ok(db.query_row("select count(*) from search_indexed", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(indexed, 1);
    let fts: i64 = store
        .read(|db| {
            Ok(db.query_row(
                "select count(*) from search_fts where search_fts match 'hello'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(fts, 0, "the FTS rows went with them");
}

// docs/issues/export-search-followups.md: `coverage` and `lexical` agree
// with `unindexed` that chunks under an old title aren't current.
#[test]
fn chunks_under_an_old_title_are_not_current_anywhere() {
    let (_dir, store) = store_with(&[chat("a", "New name", "t", false)]);
    store
        .write(|db| {
            replace_chunks(
                db,
                &target("a", "Old name", "t"),
                V,
                &["body text".to_owned()],
            )
        })
        .unwrap();
    assert_eq!(store.read(|db| unindexed(db, V)).unwrap().len(), 1);
    assert_eq!(store.read(|db| coverage(db, None, V)).unwrap(), (1, 0));
    assert!(
        store
            .read(|db| lexical(db, "\"old\"", None, V, 200))
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .read(|db| lexical(db, "\"body\"", None, V, 200))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.read(|db| vector_coverage(db, None, V, "m")).unwrap(),
        (0, 0)
    );
    assert!(
        store
            .read(|db| pending_vectors(db, V, "m", 0, 10, 0))
            .unwrap()
            .is_empty()
    );
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
                &["## Me\n\nhi".to_owned()],
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
            replace_chunks(db, &target("a", "Idea", old), V, &["an idea".to_owned()])
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

// The indexer backs off while a chat moves from t1 to t2; its cached
// transcript and chunks say "oldword" at t1. Another write then replaces
// the transcript with "newword", still at t1, and the reconcile verifies
// that replacement against a fresh render and moves the caches to t2. The
// old chunks must not ride along as current.
#[test]
fn chunks_never_outlive_the_transcript_they_were_built_from() {
    let (_dir, store) = store_with(&[chat("a", "Idea", "t1", false)]);
    store
        .write(|db| {
            db.execute(
                "insert into transcripts values ('a', 't1', 2, '# Idea\n\n---\n\noldword', 1, 1)",
                [],
            )?;
            replace_chunks(db, &target("a", "Idea", "t1"), V, &["oldword".to_owned()])?;
            // The chat moves on; the indexer is backing off.
            apply_delta(db, &[chat("a", "Idea", "t2", false)], &[], "t2")
        })
        .unwrap();
    store
        .write(|db| {
            db.execute(
                "insert or replace into transcripts values ('a', 't1', 2, '# Idea\n\n---\n\nnewword', 1, 1)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
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
            replace_chunks(db, &target("a", "Idea", "t1"), V, &["an idea".to_owned()])?;
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
    // …while another write replaces it with B under the same time.
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
fn reconcile_skips_a_transcript_whose_turns_changed_after_it_was_verified() {
    let (_dir, store) = store_with(&[chat("a", "Idea", "t2", false)]);
    store
        .write(|db| {
            db.execute(
                "insert into transcripts values ('a', 't1', 2, '# Idea', 1, 1)",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let found = store
        .read(|db| candidates(db, &["a".to_owned()], 2))
        .unwrap();
    store
        .write(|db| {
            db.execute("update transcripts set turns = 2 where id = 'a'", [])?;
            Ok(())
        })
        .unwrap();
    assert!(!store.write(|db| preserve(db, &found[0])).unwrap());
}
