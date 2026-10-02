#![allow(clippy::unwrap_used)]

use std::path::Path;

use rusqlite::{Connection, params};

use super::*;

pub(crate) fn chat(id: &str, update_time: &str) -> NewConversation {
    NewConversation {
        id: id.into(),
        title: "Original title".into(),
        create_time: "2024-01-01T00:00:00Z".into(),
        update_time: update_time.into(),
        is_archived: false,
        pinned: false,
        project_id: None,
    }
}

pub(crate) fn open(dir: &Path) -> Store {
    Store::open(&dir.join("chatgpt.db")).unwrap()
}

pub(crate) fn all(store: &Store, version: u32) -> Vec<IndexedConversation> {
    store
        .read(|db| {
            query(
                db,
                &IndexFilter {
                    include_pinned: true,
                    ..IndexFilter::default()
                },
                version,
            )
        })
        .unwrap()
}

fn set_title(store: &Store, id: &str, update_time: &str, source: &str, title: &str) {
    store
        .write(|db| {
            db.execute(
                "insert or replace into local_titles values (?, ?, 2, ?, ?, '', 'now')",
                params![id, update_time, source, title],
            )?;
            Ok(())
        })
        .unwrap();
}

// Ported from the TS CLI's src/index/local-titles.test.ts.
#[test]
fn generated_titles_go_stale_when_the_chat_changes_and_manual_ones_stay() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    store
        .write(|db| replace_all(db, &[chat("chat-1", "2024-01-01T00:00:00Z")], "t"))
        .unwrap();
    set_title(
        &store,
        "chat-1",
        "2024-01-01T00:00:00Z",
        "luna",
        "Accurate local name",
    );
    assert_eq!(all(&store, 2)[0].display_title(), "Accurate local name");
    // A different title version doesn't count.
    assert_eq!(all(&store, 1)[0].display_title(), "Original title");

    store
        .write(|db| replace_all(db, &[chat("chat-1", "2024-01-02T00:00:00Z")], "t"))
        .unwrap();
    assert_eq!(all(&store, 2)[0].display_title(), "Original title");

    set_title(
        &store,
        "chat-1",
        "2024-01-01T00:00:00Z",
        "manual",
        "My chosen title",
    );
    assert_eq!(all(&store, 2)[0].display_title(), "My chosen title");
}

#[test]
fn filters_compare_update_times_as_text_and_skip_pinned_unless_asked() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let mut pinned = chat("pinned", "2024-03-01T00:00:00Z");
    pinned.pinned = true;
    let mut archived = chat("archived", "2024-02-01T00:00:00Z");
    archived.is_archived = true;
    store
        .write(|db| {
            replace_all(
                db,
                &[chat("old", "2024-01-01T00:00:00Z"), pinned, archived],
                "t",
            )
        })
        .unwrap();
    let ids = |filter: IndexFilter| -> Vec<String> {
        store
            .read(|db| query(db, &filter, 2))
            .unwrap()
            .into_iter()
            .map(|row| row.id)
            .collect()
    };
    assert_eq!(ids(IndexFilter::default()), ["archived", "old"]);
    assert_eq!(
        ids(IndexFilter {
            archived: Some(false),
            include_pinned: true,
            ..IndexFilter::default()
        }),
        ["pinned", "old"]
    );
    assert_eq!(
        ids(IndexFilter {
            include_pinned: true,
            updated_before: Some("2024-02-02T00:00:00.000Z".into()),
            updated_after: Some("2024-01-01T00:00:00Z".into()),
            ..IndexFilter::default()
        }),
        ["archived", "old"]
    );
    assert_eq!(
        store.read(active_watermark).unwrap().as_deref(),
        Some("2024-03-01T00:00:00Z")
    );
}

fn judge(store: &Store, id: &str, update_time: &str, version: &str, answers: &str) {
    store
        .write(|db| {
            db.execute(
                "insert or replace into judgments (id, update_time, version, content_kind, answers, classified_at)
                 values (?, ?, ?, 'full', ?, 'now')",
                params![id, update_time, version, answers],
            )?;
            Ok(())
        })
        .unwrap();
}

// Ported from classification-store.test.ts and local-titles.test.ts.
#[test]
fn follow_ups_join_only_their_current_judgment() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let time = "2026-09-27T00:00:00Z";
    store
        .write(|db| replace_all(db, &[chat("chat-1", time), chat("product", time)], "t"))
        .unwrap();
    judge(
        &store,
        "chat-1",
        time,
        "v1",
        r#"{"topic":{"choice":"faith"}}"#,
    );
    judge(
        &store,
        "product",
        time,
        "v1",
        r#"{"topic":{"choice":"side_projects"}}"#,
    );
    store
        .write(|db| {
            db.execute(
                "insert into deep_judgments values ('chat-1', ?, 'v1', 'deep-v', '{\"followup\":true}', 'now')",
                [time],
            )?;
            // A Luna review without a follow-up joins on an empty deep version.
            db.execute(
                "insert into luna_judgments values ('product', ?, 'v1', '', 8, 'keep', 'product', 'own idea', 'now')",
                [time],
            )?;
            // A stale Luna review (for the follow-up version that isn't there).
            db.execute(
                "insert into luna_judgments values ('chat-1', ?, 'v1', 'other', 8, 'delete', null, 'x', 'now')",
                [time],
            )?;
            Ok(())
        })
        .unwrap();
    let current = store.read(|db| current_judgments(db, "v1")).unwrap();
    let first = &current["chat-1"];
    assert_eq!(first.topic.as_deref(), Some("faith"));
    assert_eq!(first.deep_answers.as_deref(), Some("{\"followup\":true}"));
    assert_eq!(first.luna_suggestion, None);
    assert_eq!(
        current["product"].luna_brainstorm.as_deref(),
        Some("product")
    );
    assert_eq!(current["product"].luna_version, Some(8));
    assert!(
        store
            .read(|db| current_judgments(db, "v2"))
            .unwrap()
            .is_empty()
    );

    store
        .write(|db| replace_all(db, &[chat("chat-1", "2026-09-28T00:00:00Z")], "t"))
        .unwrap();
    assert!(
        store
            .read(|db| current_judgments(db, "v1"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn reconcile_moves_every_cache_forward_unless_the_chat_changed_again() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let old = "2026-09-01T00:00:00.000Z";
    let new = "2026-09-28T00:00:00.000Z";
    store
        .write(|db| {
            replace_all(db, &[chat("chat-1", new)], "t")?;
            db.execute(
                "insert into transcripts values ('chat-1', ?, 2, '# md', 1, 1)",
                [old],
            )?;
            db.execute(
                "insert into summaries values ('chat-1', ?, 1, 'sum', 'm')",
                [old],
            )?;
            Ok(())
        })
        .unwrap();
    judge(&store, "chat-1", old, "v1", "{}");
    let ids = vec!["chat-1".to_owned(), "chat-1".to_owned(), "other".to_owned()];
    let found = store.read(|db| candidates(db, &ids, 2)).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].cached_update_time, old);
    assert!(store.read(|db| candidates(db, &ids, 3)).unwrap().is_empty());

    let mut moved = found[0].clone();
    moved.title = "Renamed since".into();
    assert!(!store.write(|db| preserve(db, &moved)).unwrap());
    assert!(store.write(|db| preserve(db, &found[0])).unwrap());
    let times: Vec<String> = store
        .read(|db| {
            let mut statement = db.prepare(
                "select update_time from transcripts union all select update_time from summaries
                 union all select update_time from judgments",
            )?;
            Ok(statement
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?)
        })
        .unwrap();
    assert_eq!(times, [new, new, new]);
}

#[test]
fn a_newer_schema_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chatgpt.db");
    drop(Store::open(&path).unwrap());
    Connection::open(&path)
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();
    assert!(matches!(
        Store::open(&path),
        Err(StoreError::NewerDatabase { found: 99, .. })
    ));
}

#[test]
fn the_index_file_and_its_wal_and_shm_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let database = store.path().to_path_buf();
    let sidecar = |suffix: &str| {
        let mut path = database.as_os_str().to_owned();
        path.push(suffix);
        std::path::PathBuf::from(path)
    };
    assert_eq!(mode(&database), 0o600);
    // Open while the store is: WAL mode keeps them until the last close.
    assert_eq!(mode(&sidecar("-wal")), 0o600, "-wal");
    assert_eq!(mode(&sidecar("-shm")), 0o600, "-shm");

    // An older build's, made from the umask, are made private on open.
    drop(store);
    for suffix in ["-wal", "-shm"] {
        std::fs::write(sidecar(suffix), b"").unwrap();
        std::fs::set_permissions(sidecar(suffix), std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let _store = open(dir.path());
    assert_eq!(mode(&sidecar("-wal")), 0o600, "older -wal");
    assert_eq!(mode(&sidecar("-shm")), 0o600, "older -shm");
}

#[test]
fn chats_new_since_the_background_jev_baseline_compare_by_time_not_text() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let chats = [
        // 100 µs after the baseline, though it sorts before it as text.
        chat("a-later", "2026-10-01T14:05:59.2061Z"),
        // Before it, though it sorts after it as text.
        chat("b-earlier", "2026-10-01T14:05:59Z"),
        // The same moment, written another way.
        chat("c-same", "2026-10-01T14:05:59.206000Z"),
        chat("d-next-day", "2026-10-02T00:00:00.000Z"),
    ];
    store
        .write(|db| replace_all(db, &chats, "2026-10-02T00:00:00.000Z"))
        .unwrap();
    let new_since = |after: &str| {
        store
            .read(|db| {
                unjudged(
                    db,
                    Unjudged {
                        after,
                        questions_version: "v",
                        render_version: 1,
                        max_tokens: 12_000,
                        summary_version: 1,
                        limit: 10,
                    },
                )
            })
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        new_since("2026-10-01T14:05:59.206Z"),
        ["d-next-day", "a-later"]
    );
    assert_eq!(new_since("").len(), 4, "no baseline: every chat");
}

#[test]
fn the_background_jev_baseline_is_the_newest_chat_by_time_not_text() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let chats = [
        chat("a-earlier", "2026-10-01T14:05:59.206Z"),
        // Newer by 100 µs, though it sorts first as text.
        chat("b-newest", "2026-10-01T14:05:59.2061Z"),
    ];
    store
        .write(|db| replace_all(db, &chats, "2026-10-02T00:00:00.000Z"))
        .unwrap();
    assert_eq!(
        store.read(newest_active_update_time).unwrap().as_deref(),
        Some("2026-10-01T14:05:59.2061Z")
    );
}
