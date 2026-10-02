#![allow(clippy::unwrap_used)]

//! Rows the daemon writes (a `title`, a Jev guard judgment) and the TS
//! import that must leave them alone.

use rusqlite::params;

use super::*;
use crate::tests::{all, chat, counts, legacy_db, open};

fn manual<'a>(id: &'a str, title: &'a str, at: &'a str) -> ManualTitle<'a> {
    ManualTitle {
        id,
        update_time: "t1",
        version: 2,
        title,
        updated_at: at,
    }
}

fn shown_title(store: &Store, id: &str) -> Option<String> {
    all(store, 2)
        .into_iter()
        .find(|chat| chat.id == id)
        .and_then(|chat| chat.local_title)
}

#[test]
fn a_manual_title_survives_the_import_until_the_ts_cli_writes_a_newer_manual_one() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let (path, legacy) = legacy_db(dir.path());
    store
        .write(|db| replace_all(db, &[chat("a", "t1"), chat("b", "t1")], "t"))
        .unwrap();
    store
        .write(|db| set_local_title(db, &manual("a", "Mine", "2026-10-02T10:00:00.000Z")))
        .unwrap();

    // The TS index doesn't have it: kept, not deleted.
    let report = store.write(|db| import_legacy(db, &path)).unwrap();
    assert_eq!(counts(&report, "local_titles"), (0, 0, 0));
    assert_eq!(shown_title(&store, "a").as_deref(), Some("Mine"));

    // A later Luna title from the TS CLI never replaces the user's own.
    legacy
        .execute(
            "insert into local_titles values ('a', 't1', 2, 'luna', 'Luna', '', '2026-10-03T00:00:00.000Z')",
            [],
        )
        .unwrap();
    let report = store.write(|db| import_legacy(db, &path)).unwrap();
    assert_eq!(counts(&report, "local_titles"), (0, 0, 0));
    assert_eq!(shown_title(&store, "a").as_deref(), Some("Mine"));

    // An older manual one doesn't either; a newer manual one does, and the
    // TS CLI owns the row again.
    legacy
        .execute(
            "update local_titles set source = 'manual', title = 'Old', updated_at = '2026-10-01T00:00:00.000Z'",
            [],
        )
        .unwrap();
    store.write(|db| import_legacy(db, &path)).unwrap();
    assert_eq!(shown_title(&store, "a").as_deref(), Some("Mine"));
    legacy
        .execute(
            "update local_titles set title = 'Newer', updated_at = '2026-10-04T00:00:00.000Z'",
            [],
        )
        .unwrap();
    let report = store.write(|db| import_legacy(db, &path)).unwrap();
    assert_eq!(counts(&report, "local_titles"), (0, 1, 0));
    assert_eq!(shown_title(&store, "a").as_deref(), Some("Newer"));
    let markers: i64 = store
        .read(|db| Ok(db.query_row("select count(*) from native_rows", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(markers, 0);
    // Now an ordinary TS row: it goes when the TS index drops it.
    legacy.execute("delete from local_titles", []).unwrap();
    let report = store.write(|db| import_legacy(db, &path)).unwrap();
    assert_eq!(counts(&report, "local_titles"), (0, 0, 1));
}

#[test]
fn a_guard_judgment_replaces_the_reviews_and_the_import_keeps_it_that_way() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let (path, legacy) = legacy_db(dir.path());
    store
        .write(|db| replace_all(db, &[chat("a", "t1")], "t"))
        .unwrap();
    legacy
        .execute_batch(
            "insert into judgments (id, update_time, version, content_kind, answers, classified_at)
                values ('a', 't1', 'v1', 'full', '{\"old\":1}', '2026-09-01T00:00:00.000Z');
             insert into deep_judgments values ('a', 't1', 'v1', 'd1', '{}', '2026-09-01T00:00:00.000Z');
             insert into luna_judgments values ('a', 't1', 'v1', 'd1', 8, 'keep', null, 'r', '2026-09-01T00:00:00.000Z');",
        )
        .unwrap();
    store.write(|db| import_legacy(db, &path)).unwrap();
    let before = store
        .read(|db| judgment(db, "a", "t1", "v1"))
        .unwrap()
        .unwrap();
    assert_eq!(before.luna_suggestion.as_deref(), Some("keep"));
    assert_eq!(before.deep_version.as_deref(), Some("d1"));

    let fresh = NewJudgment {
        id: "a".into(),
        update_time: "t1".into(),
        version: "v1".into(),
        content_kind: "full".into(),
        answers: "{\"new\":1}".into(),
        classified_at: "2026-10-02T10:00:00.000Z".into(),
    };
    store.write(|db| save_judgment(db, &fresh)).unwrap();
    let saved = store
        .read(|db| judgment(db, "a", "t1", "v1"))
        .unwrap()
        .unwrap();
    assert_eq!(saved.answers, "{\"new\":1}");
    assert_eq!(
        saved.luna_suggestion, None,
        "the old review no longer applies"
    );
    assert_eq!(saved.deep_answers, None);

    let report = store.write(|db| import_legacy(db, &path)).unwrap();
    for table in ["judgments", "deep_judgments", "luna_judgments"] {
        assert_eq!(counts(&report, table), (0, 0, 0), "{table}");
    }
    let kept = store
        .read(|db| judgment(db, "a", "t1", "v1"))
        .unwrap()
        .unwrap();
    assert_eq!(kept, saved);

    // The TS CLI classifies it again later: its judgment wins.
    legacy
        .execute(
            "update judgments set answers = '{\"ts\":2}', classified_at = '2026-10-03T00:00:00.000Z'",
            [],
        )
        .unwrap();
    store.write(|db| import_legacy(db, &path)).unwrap();
    let replaced = store
        .read(|db| judgment(db, "a", "t1", "v1"))
        .unwrap()
        .unwrap();
    assert_eq!(replaced.answers, "{\"ts\":2}");
}

#[test]
fn renames_project_moves_and_exact_lookups_write_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    store
        .write(|db| {
            replace_all(
                db,
                &[chat("abc", "t2"), chat("abd", "t1"), chat("x", "t3")],
                "t",
            )
        })
        .unwrap();
    store
        .write(|db| {
            rename(db, "abc", "Renamed")?;
            set_project(db, "abd", Some("g-p-1"))?;
            set_project(db, "x", Some("g-p-2"))?;
            set_project(db, "x", None)
        })
        .unwrap();
    let found = store
        .read(|db| {
            by_ids(
                db,
                &["x".into(), "gone".into(), "ab".into(), "abc".into()],
                2,
            )
        })
        .unwrap();
    let rows: Vec<(&str, &str, Option<&str>)> = found
        .iter()
        .map(|c| (c.id.as_str(), c.title.as_str(), c.project_id.as_deref()))
        .collect();
    assert_eq!(
        rows,
        [("x", "Original title", None), ("abc", "Renamed", None)],
        "exact ids only, in the order asked"
    );
    assert_eq!(
        store.read(|db| get(db, "abd", 2)).unwrap()[0]
            .project_id
            .as_deref(),
        Some("g-p-1")
    );
    store
        .write(|db| {
            db.execute(
                "insert into summaries values ('abc', 't2', 9, 'short', 'm')",
                params![],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        store
            .read(|db| summary(db, "abc", "t2", 9))
            .unwrap()
            .as_deref(),
        Some("short")
    );
    assert_eq!(store.read(|db| summary(db, "abc", "t1", 9)).unwrap(), None);
}

#[test]
fn the_first_account_to_bind_the_index_keeps_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    assert_eq!(
        store.write(|db| bind_account(db, "user-a")).unwrap(),
        "user-a"
    );
    assert_eq!(
        store.write(|db| bind_account(db, "user-b")).unwrap(),
        "user-a",
        "a later binder sees the winner"
    );
    assert_eq!(store.read(account).unwrap().as_deref(), Some("user-a"));
}
