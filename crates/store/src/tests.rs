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

pub(crate) const TS_SCHEMA: &str = "
create table conversations (id text primary key, title text not null, create_time text not null,
    update_time text not null, is_archived integer not null, pinned integer not null, project_id text);
create table meta (key text primary key, value text not null);
create table local_titles (id text primary key, update_time text not null, version integer not null,
    source text not null check(source in ('luna', 'manual')), title text not null,
    theme text not null default '', updated_at text not null);
create table transcripts (id text primary key, update_time text not null, render_version integer not null,
    markdown text not null, turns integer not null, approx_tokens integer not null);
create table summaries (id text primary key, update_time text not null, prompt_version integer not null,
    summary text not null, model text not null);
create table judgments (id text primary key, update_time text not null, version text not null,
    content_kind text not null, answers text not null, classified_at text not null,
    topic text generated always as (json_extract(answers, '$.topic.choice')) virtual);
create table deep_judgments (id text primary key, update_time text not null, questions_version text not null,
    version text not null, answers text not null, classified_at text not null);
create table luna_judgments (id text primary key, update_time text not null, questions_version text not null,
    deep_version text not null, version integer not null, suggestion text not null,
    brainstorm text, reason text not null, classified_at text not null);
";

pub(crate) fn legacy_db(dir: &Path) -> (std::path::PathBuf, Connection) {
    let path = dir.join("ts index.db");
    let db = Connection::open(&path).unwrap();
    db.execute_batch(TS_SCHEMA).unwrap();
    (path, db)
}

pub(crate) fn counts(report: &ImportCounts, table: &str) -> (u64, u64, u64) {
    let table = report.tables.iter().find(|t| t.table == table).unwrap();
    (table.inserted, table.updated, table.deleted)
}

#[test]
fn the_import_mirrors_the_ts_index_and_running_it_again_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let (path, legacy) = legacy_db(dir.path());
    legacy
        .execute_batch(
            "insert into judgments (id, update_time, version, content_kind, answers, classified_at)
                values ('a', 't1', 'v1', 'full', '{\"topic\":{\"choice\":\"faith\"}}', 'c1'),
                       ('b', 't1', 'v1', 'full', '{}', 'c1');
             insert into luna_judgments values ('a', 't1', 'v1', '', 8, 'keep', null, 'r', 'c1');
             insert into local_titles values ('a', 't1', 2, 'luna', 'Title', '', 'u');",
        )
        .unwrap();
    let before = std::fs::read(&path).unwrap();

    let first = store.write(|db| import_legacy(db, &path)).unwrap();
    assert_eq!(counts(&first, "judgments"), (2, 0, 0));
    assert_eq!(counts(&first, "luna_judgments"), (1, 0, 0));
    assert_eq!(
        first
            .tables
            .iter()
            .find(|t| t.table == "memory_judgments")
            .unwrap()
            .skipped
            .as_deref(),
        Some("not in the TS index")
    );
    let again = store.write(|db| import_legacy(db, &path)).unwrap();
    for table in &again.tables {
        assert_eq!(
            (table.inserted, table.updated, table.deleted),
            (0, 0, 0),
            "{}",
            table.table
        );
    }
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the TS index is never written"
    );

    // A re-judgment in the TS CLI drops the old Luna review and rewrites the judgment.
    legacy
        .execute_batch(
            "delete from luna_judgments where id = 'a';
             update judgments set answers = '{\"topic\":{\"choice\":\"health\"}}', classified_at = 'c2' where id = 'a';
             delete from judgments where id = 'b';",
        )
        .unwrap();
    let third = store.write(|db| import_legacy(db, &path)).unwrap();
    assert_eq!(counts(&third, "judgments"), (0, 1, 1));
    assert_eq!(counts(&third, "luna_judgments"), (0, 0, 1));
    let topic: String = store
        .read(|db| {
            Ok(
                db.query_row("select topic from judgments where id = 'a'", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(topic, "health");
}

#[test]
fn the_import_keeps_a_reconciled_update_time_while_it_is_current() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let (path, legacy) = legacy_db(dir.path());
    // The TS index recorded this chat's time from the single-chat endpoint,
    // in milliseconds; the list says the same instant in microseconds.
    let ts_time = "2026-09-28T00:04:08.278Z";
    let list_time = "2026-09-28T00:04:08.278467Z";
    legacy
        .execute(
            "insert into judgments (id, update_time, version, content_kind, answers, classified_at)
             values ('a', ?, 'v1', 'full', '{}', 'c1')",
            [ts_time],
        )
        .unwrap();
    store
        .write(|db| replace_all(db, &[chat("a", list_time)], "t"))
        .unwrap();
    store.write(|db| import_legacy(db, &path)).unwrap();
    assert!(
        store
            .read(|db| current_judgments(db, "v1"))
            .unwrap()
            .is_empty()
    );
    // The daemon's reconcile confirmed the content and moved it forward.
    store
        .write(|db| {
            db.execute(
                "update judgments set update_time = ? where id = 'a'",
                [list_time],
            )?;
            Ok(())
        })
        .unwrap();
    let report = store.write(|db| import_legacy(db, &path)).unwrap();
    assert_eq!(counts(&report, "judgments"), (0, 0, 0));
    assert_eq!(
        store.read(|db| current_judgments(db, "v1")).unwrap().len(),
        1
    );
    // A re-judgment in the TS CLI still replaces it.
    legacy
        .execute("update judgments set classified_at = 'c2'", [])
        .unwrap();
    let report = store.write(|db| import_legacy(db, &path)).unwrap();
    assert_eq!(counts(&report, "judgments"), (0, 1, 0));
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
fn the_index_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let mode = std::fs::metadata(store.path())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}
