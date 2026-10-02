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
fn a_new_judgment_replaces_its_reviews_and_the_import_never_touches_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let (path, legacy) = legacy_db(dir.path());
    store
        .write(|db| replace_all(db, &[chat("a", "t1")], "t"))
        .unwrap();
    let first = NewJudgment {
        id: "a".into(),
        update_time: "t1".into(),
        version: "v1".into(),
        content_kind: "full".into(),
        answers: "{\"old\":1}".into(),
        classified_at: "2026-09-01T00:00:00.000Z".into(),
    };
    store.write(|db| save_judgment(db, &first)).unwrap();
    let deep = NewDeepJudgment {
        id: "a".into(),
        update_time: "t1".into(),
        questions_version: "v1".into(),
        version: "d1".into(),
        answers: "{}".into(),
        classified_at: "2026-09-01T00:00:00.000Z".into(),
    };
    store.write(|db| save_deep_judgment(db, &deep)).unwrap();
    let luna = NewLunaJudgment {
        id: "a".into(),
        update_time: "t1".into(),
        questions_version: "v1".into(),
        deep_version: "d1".into(),
        version: 8,
        suggestion: "keep".into(),
        brainstorm: None,
        reason: "r".into(),
        classified_at: "2026-09-01T00:00:00.000Z".into(),
    };
    store.write(|db| save_luna_judgment(db, &luna)).unwrap();
    let before = store
        .read(|db| judgment(db, "a", "t1", "v1"))
        .unwrap()
        .unwrap();
    assert_eq!(before.luna_suggestion.as_deref(), Some("keep"));
    assert_eq!(before.deep_version.as_deref(), Some("d1"));
    assert!(
        store
            .read(|db| has_deep_judgment(db, "a", "t1", "v1", "d1"))
            .unwrap()
    );
    assert!(
        store
            .read(|db| has_luna_judgment(db, "a", "t1", "v1", "d1", 8))
            .unwrap()
    );

    // A second first pass for the same chat and version (the background
    // Jev racing `classify`) keeps the follow-up and review already paid
    // for.
    let again = NewJudgment {
        classified_at: "2026-09-02T00:00:00.000Z".into(),
        ..first.clone()
    };
    store.write(|db| save_judgment(db, &again)).unwrap();
    let kept = store
        .read(|db| judgment(db, "a", "t1", "v1"))
        .unwrap()
        .unwrap();
    assert_eq!(kept.luna_suggestion.as_deref(), Some("keep"));
    assert_eq!(kept.deep_version.as_deref(), Some("d1"));

    // A new follow-up drops the review that rested on the old one.
    store.write(|db| save_deep_judgment(db, &deep)).unwrap();
    assert!(
        !store
            .read(|db| has_luna_judgment(db, "a", "t1", "v1", "d1", 8))
            .unwrap()
    );

    // A judgment for a newer update of the chat (or newer questions) drops
    // the old follow-up and review.
    store.write(|db| save_luna_judgment(db, &luna)).unwrap();
    store
        .write(|db| replace_all(db, &[chat("a", "t2")], "t"))
        .unwrap();
    let fresh = NewJudgment {
        update_time: "t2".into(),
        answers: "{\"new\":1}".into(),
        classified_at: "2026-10-02T10:00:00.000Z".into(),
        ..first
    };
    store.write(|db| save_judgment(db, &fresh)).unwrap();
    let left: i64 = store
        .read(|db| {
            Ok(db.query_row(
                "select (select count(*) from deep_judgments) + (select count(*) from luna_judgments)",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(left, 0, "the old follow-up and review went");
    let saved = store
        .read(|db| judgment(db, "a", "t2", "v1"))
        .unwrap()
        .unwrap();
    assert_eq!(saved.answers, "{\"new\":1}");
    assert_eq!(
        saved.luna_suggestion, None,
        "the old review no longer applies"
    );
    assert_eq!(saved.deep_answers, None);

    // The TS CLI's judgment for the chat, however new, never comes over.
    legacy
        .execute(
            "insert into judgments (id, update_time, version, content_kind, answers, classified_at)
             values ('a', 't2', 'v1', 'full', '{\"ts\":2}', '2030-01-01T00:00:00.000Z')",
            [],
        )
        .unwrap();
    store.write(|db| import_legacy(db, &path)).unwrap();
    let kept = store
        .read(|db| judgment(db, "a", "t2", "v1"))
        .unwrap()
        .unwrap();
    assert_eq!(kept, saved);
}

#[test]
fn a_luna_title_is_kept_by_the_import_and_never_replaces_a_manual_one() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let (path, _legacy) = legacy_db(dir.path());
    store
        .write(|db| replace_all(db, &[chat("a", "t1"), chat("b", "t1")], "t"))
        .unwrap();
    store
        .write(|db| {
            set_luna_title(
                db,
                &manual("a", "From Luna", "2026-10-02T10:00:00.000Z"),
                "theme",
            )
        })
        .unwrap();
    assert_eq!(
        store
            .read(|db| local_title_source(db, "a", "t1", 2))
            .unwrap()
            .as_deref(),
        Some("luna")
    );
    assert_eq!(
        store
            .read(|db| local_title_source(db, "a", "t2", 2))
            .unwrap(),
        None
    );
    store.write(|db| import_legacy(db, &path)).unwrap();
    assert_eq!(shown_title(&store, "a").as_deref(), Some("From Luna"));
    store
        .write(|db| set_local_title(db, &manual("b", "Mine", "2026-10-02T10:00:00.000Z")))
        .unwrap();
    assert_eq!(
        store
            .read(|db| local_title_source(db, "b", "other-time", 1))
            .unwrap()
            .as_deref(),
        Some("manual"),
        "a manual title counts whatever the chat's time"
    );
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
