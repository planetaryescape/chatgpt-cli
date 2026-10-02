//! Local display titles the daemon writes: `chatgpt title` (manual) and
//! `chatgpt titles` (Luna's). Ported from `ConversationIndex.localTitle` and
//! `setLocalTitle` in the TS CLI's `src/index/store.ts` @ 1b8c950.

use rusqlite::{Connection, OptionalExtension, params};

use crate::Result;

/// A manual title for chat `id`, as of its `update_time` (a manual title
/// counts whatever the chat's `update_time` later is).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManualTitle<'a> {
    pub id: &'a str,
    pub update_time: &'a str,
    pub version: u32,
    /// Already cleaned: trimmed, whitespace runs collapsed.
    pub title: &'a str,
    /// ISO 8601, `toISOString`'s form.
    pub updated_at: &'a str,
}

/// Save a manual local title, replacing any Luna or manual one, and mark it
/// as the daemon's so the TS import keeps it.
pub fn set_local_title(connection: &mut Connection, title: &ManualTitle<'_>) -> Result<()> {
    let transaction = connection.transaction()?;
    transaction.execute(
        "insert or replace into local_titles values (?, ?, ?, 'manual', ?, '', ?)",
        params![
            title.id,
            title.update_time,
            title.version,
            title.title,
            title.updated_at
        ],
    )?;
    crate::native::mark(&transaction, "local_titles", title.id, title.updated_at)?;
    transaction.commit()?;
    Ok(())
}

/// `ConversationIndex.localTitle`: chat `id`'s manual title, or its Luna
/// title for this `update_time` and title `version`. The source, `luna` or
/// `manual`.
pub fn local_title_source(
    connection: &Connection,
    id: &str,
    update_time: &str,
    version: u32,
) -> Result<Option<String>> {
    Ok(connection
        .prepare_cached(
            "select source from local_titles
             where id = ? and (source = 'manual' or (update_time = ? and version = ?))",
        )?
        .query_row(params![id, update_time, version], |row| row.get(0))
        .optional()?)
}

/// A Luna title and theme for chat `id` (`setLocalTitle(c, title, theme,
/// "luna")`), already cleaned. Marked as the daemon's, so the TS import
/// keeps it until the TS CLI writes a newer manual one.
pub fn set_luna_title(
    connection: &mut Connection,
    title: &ManualTitle<'_>,
    theme: &str,
) -> Result<()> {
    let transaction = connection.transaction()?;
    transaction.execute(
        "insert or replace into local_titles values (?, ?, ?, 'luna', ?, ?, ?)",
        params![
            title.id,
            title.update_time,
            title.version,
            title.title,
            theme,
            title.updated_at
        ],
    )?;
    crate::native::mark(&transaction, "local_titles", title.id, title.updated_at)?;
    transaction.commit()?;
    Ok(())
}
