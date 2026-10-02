//! The store half of `reconcileMetadataChanges` (the TS CLI's
//! `src/index/reconcile-metadata.ts` @ 1b8c950): chats whose cached
//! transcript is for an older `update_time`, and moving every cache forward
//! once the daemon has confirmed the content didn't change.

use rusqlite::{Connection, OptionalExtension, params};

use crate::Result;

/// A chat whose `update_time` moved past its cached transcript's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub id: String,
    pub title: String,
    pub create_time: String,
    pub update_time: String,
    pub cached_update_time: String,
    pub markdown: String,
    pub turns: i64,
}

/// The candidates among `ids`, in `ids` order (duplicates dropped).
pub fn candidates(
    connection: &Connection,
    ids: &[String],
    render_version: u32,
) -> Result<Vec<Candidate>> {
    let mut statement = connection.prepare_cached(
        "select c.id, c.title, c.create_time, c.update_time, t.update_time, t.markdown, t.turns
         from conversations c join transcripts t on t.id = c.id
         where c.id = ? and t.update_time != c.update_time and t.render_version = ?",
    )?;
    let mut seen = std::collections::HashSet::new();
    let mut found = Vec::new();
    for id in ids {
        if !seen.insert(id.as_str()) {
            continue;
        }
        let candidate = statement
            .query_row(params![id, render_version], |row| {
                Ok(Candidate {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    create_time: row.get(2)?,
                    update_time: row.get(3)?,
                    cached_update_time: row.get(4)?,
                    markdown: row.get(5)?,
                    turns: row.get(6)?,
                })
            })
            .optional()?;
        found.extend(candidate);
    }
    Ok(found)
}

/// Move every cache of `candidate` from its cached `update_time` to the
/// chat's current one, unless the chat changed again since it was read.
/// Returns whether it did. Search chunks move too when they were indexed
/// from the cached version under the same title.
pub fn preserve(connection: &mut Connection, candidate: &Candidate) -> Result<bool> {
    let transaction = connection.transaction()?;
    let current: Option<(String, String)> = transaction
        .query_row(
            "select update_time, title from conversations where id = ?",
            [&candidate.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if current.as_ref() != Some(&(candidate.update_time.clone(), candidate.title.clone())) {
        return Ok(false);
    }
    for table in [
        "transcripts",
        "summaries",
        "judgments",
        "deep_judgments",
        "luna_judgments",
        "local_titles",
    ] {
        transaction.execute(
            &format!("update {table} set update_time = ? where id = ? and update_time = ?"),
            params![
                candidate.update_time,
                candidate.id,
                candidate.cached_update_time
            ],
        )?;
    }
    let indexed = transaction
        .query_row(
            "select 1 from search_chunks where conversation_id = ? and update_time = ? and title = ? limit 1",
            params![candidate.id, candidate.cached_update_time, candidate.title],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if indexed {
        for table in ["search_indexed", "search_chunks"] {
            transaction.execute(
                &format!(
                    "update {table} set update_time = ? where conversation_id = ? and update_time = ?"
                ),
                params![
                    candidate.update_time,
                    candidate.id,
                    candidate.cached_update_time
                ],
            )?;
        }
    }
    transaction.commit()?;
    Ok(true)
}
