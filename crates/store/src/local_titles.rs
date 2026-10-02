//! Local display titles the daemon writes: `chatgpt title`. Ported from
//! `ConversationIndex.setLocalTitle` in the TS CLI's `src/index/store.ts`
//! @ 1b8c950.

use rusqlite::{Connection, params};

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
