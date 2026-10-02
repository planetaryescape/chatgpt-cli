//! The chat index. Ported from the TS CLI's `src/index/store.ts` @ 1b8c950.

use rusqlite::{Connection, OptionalExtension, Transaction, params, params_from_iter};

use crate::Result;
use crate::meta::{get_meta, set_meta};

/// A chat as the index holds it, with its current local title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedConversation {
    pub id: String,
    pub title: String,
    pub create_time: String,
    pub update_time: String,
    pub is_archived: bool,
    pub pinned: bool,
    pub project_id: Option<String>,
    /// A manual title, or a Luna one made for this `update_time` and the
    /// current title version.
    pub local_title: Option<String>,
}

impl IndexedConversation {
    /// What the CLI shows: the local title when there is one.
    pub fn display_title(&self) -> &str {
        match self.local_title.as_deref() {
            Some(title) if !title.is_empty() => title,
            _ => &self.title,
        }
    }
}

/// A chat from the conversation list, mapped as `ConversationIndex.row` maps
/// it: a missing title is "(untitled)", any pinned time means pinned, and
/// only a project's gizmo id (`g-p-…`) is a project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewConversation {
    pub id: String,
    pub title: String,
    pub create_time: String,
    pub update_time: String,
    pub is_archived: bool,
    pub pinned: bool,
    pub project_id: Option<String>,
}

/// `Filter` in store.ts, without the title regex, which the daemon applies.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IndexFilter {
    /// `None`: active and archived.
    pub archived: Option<bool>,
    pub include_pinned: bool,
    /// ISO 8601, compared as text with `update_time`, as the TS CLI does.
    pub updated_before: Option<String>,
    pub updated_after: Option<String>,
}

fn upsert(transaction: &Transaction<'_>, chat: &NewConversation) -> Result<()> {
    transaction
        .prepare_cached("insert or replace into conversations values (?, ?, ?, ?, ?, ?, ?)")?
        .execute(params![
            chat.id,
            chat.title,
            chat.create_time,
            chat.update_time,
            chat.is_archived,
            chat.pinned,
            chat.project_id,
        ])?;
    Ok(())
}

/// Full replace, so chats deleted elsewhere drop out of the index.
pub fn replace_all(
    connection: &mut Connection,
    chats: &[NewConversation],
    synced_at: &str,
) -> Result<()> {
    let transaction = connection.transaction()?;
    transaction.execute("delete from conversations", [])?;
    for chat in chats {
        upsert(&transaction, chat)?;
    }
    set_meta(&transaction, "synced_at", synced_at)?;
    transaction.commit()?;
    Ok(())
}

/// Archiving doesn't bump `update_time`, so archive state comes from the
/// complete archived list rather than the delta. `all_archived` is empty
/// when the pass didn't read that list.
pub fn apply_delta(
    connection: &mut Connection,
    changed_active: &[NewConversation],
    all_archived: &[NewConversation],
    synced_at: &str,
) -> Result<()> {
    let transaction = connection.transaction()?;
    for chat in changed_active.iter().chain(all_archived) {
        upsert(&transaction, chat)?;
    }
    set_meta(&transaction, "synced_at", synced_at)?;
    transaction.commit()?;
    Ok(())
}

/// The newest `update_time` among active chats: a delta sync reads the
/// active list from the top until it reaches this.
pub fn active_watermark(connection: &Connection) -> Result<Option<String>> {
    Ok(connection.query_row(
        "select max(update_time) from conversations where is_archived = 0",
        [],
        |row| row.get(0),
    )?)
}

pub fn archived_ids(connection: &Connection) -> Result<Vec<String>> {
    let mut statement = connection.prepare("select id from conversations where is_archived = 1")?;
    let ids = statement
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}

/// Every indexed chat's id, newest first.
pub fn all_ids(connection: &Connection) -> Result<Vec<String>> {
    let mut statement =
        connection.prepare("select id from conversations order by update_time desc")?;
    let ids = statement
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}

pub fn count_all(connection: &Connection) -> Result<u64> {
    let count: i64 =
        connection.query_row("select count(*) from conversations", [], |row| row.get(0))?;
    Ok(u64::try_from(count).unwrap_or(0))
}

pub fn synced_at(connection: &Connection) -> Result<Option<String>> {
    get_meta(connection, "synced_at")
}

/// `ConversationIndex.query`: newest first, with local titles.
pub fn query(
    connection: &Connection,
    filter: &IndexFilter,
    local_title_version: u32,
) -> Result<Vec<IndexedConversation>> {
    let mut clauses = Vec::new();
    let mut values: Vec<rusqlite::types::Value> = vec![i64::from(local_title_version).into()];
    if let Some(archived) = filter.archived {
        clauses.push("c.is_archived = ?");
        values.push(i64::from(archived).into());
    }
    if !filter.include_pinned {
        clauses.push("c.pinned = 0");
    }
    if let Some(before) = &filter.updated_before {
        clauses.push("c.update_time < ?");
        values.push(before.clone().into());
    }
    if let Some(after) = &filter.updated_after {
        clauses.push("c.update_time >= ?");
        values.push(after.clone().into());
    }
    let condition = if clauses.is_empty() {
        String::new()
    } else {
        format!("where {}", clauses.join(" and "))
    };
    let sql = format!(
        "select c.id, c.title, c.create_time, c.update_time, c.is_archived, c.pinned, c.project_id,
            case when l.source = 'manual' or (l.update_time = c.update_time and l.version = ?1)
                then l.title end as local_title
         from conversations c left join local_titles l on l.id = c.id
         {condition} order by c.update_time desc"
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement
        .query_map(params_from_iter(values), indexed)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// `ConversationIndex.get`: chats whose id is like `prefix%`, with local
/// titles. SQL `like`, as in the TS CLI: ASCII case doesn't matter, and `%`
/// and `_` in the prefix are wildcards. A full UUID (what `Transcript` and
/// search ask for) is looked up by the primary key instead, which `like`
/// can't use; lower-cased, so it matches what `like` would.
pub fn get(
    connection: &Connection,
    prefix: &str,
    local_title_version: u32,
) -> Result<Vec<IndexedConversation>> {
    const SELECT: &str =
        "select c.id, c.title, c.create_time, c.update_time, c.is_archived, c.pinned, c.project_id,
            case when l.source = 'manual' or (l.update_time = c.update_time and l.version = ?)
                then l.title end as local_title
         from conversations c left join local_titles l on l.id = c.id";
    // A blank prefix names no chat, though `like '%'` would match them all.
    if prefix.trim().is_empty() {
        return Ok(Vec::new());
    }
    let (sql, value) = if is_uuid(prefix) {
        (
            format!("{SELECT} where c.id = ?"),
            prefix.to_ascii_lowercase(),
        )
    } else {
        (format!("{SELECT} where c.id like ?"), format!("{prefix}%"))
    };
    let mut statement = connection.prepare_cached(&sql)?;
    let rows = statement
        .query_map(params![local_title_version, value], indexed)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// `8-4-4-4-12` hex digits, as every ChatGPT conversation id is.
fn is_uuid(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(at, byte)| match at {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

fn indexed(row: &rusqlite::Row<'_>) -> rusqlite::Result<IndexedConversation> {
    Ok(IndexedConversation {
        id: row.get(0)?,
        title: row.get(1)?,
        create_time: row.get(2)?,
        update_time: row.get(3)?,
        is_archived: row.get::<_, i64>(4)? != 0,
        pinned: row.get::<_, i64>(5)? != 0,
        project_id: row.get(6)?,
        local_title: row.get(7)?,
    })
}

pub fn set_archived(connection: &Connection, id: &str, archived: bool) -> Result<()> {
    connection.execute(
        "update conversations set is_archived = ? where id = ?",
        params![archived, id],
    )?;
    Ok(())
}

/// Drop a deleted chat and its local title.
pub fn remove(connection: &mut Connection, id: &str) -> Result<()> {
    let transaction = connection.transaction()?;
    transaction.execute("delete from conversations where id = ?", [id])?;
    transaction.execute("delete from local_titles where id = ?", [id])?;
    transaction.commit()?;
    Ok(())
}

/// `ConversationIndex.rename`: the new ChatGPT title. Its search chunks,
/// built under the old title, stop counting as current.
pub fn rename(connection: &Connection, id: &str, title: &str) -> Result<()> {
    connection.execute(
        "update conversations set title = ? where id = ?",
        params![title, id],
    )?;
    Ok(())
}

/// `ConversationIndex.setProject`: `None` takes the chat out of any project.
pub fn set_project(connection: &Connection, id: &str, project_id: Option<&str>) -> Result<()> {
    connection.execute(
        "update conversations set project_id = ? where id = ?",
        params![project_id, id],
    )?;
    Ok(())
}

/// Take every chat out of project `project_id` (it was deleted); how many
/// were in it.
pub fn clear_project(connection: &Connection, project_id: &str) -> Result<usize> {
    Ok(connection.execute(
        "update conversations set project_id = null where project_id = ?",
        params![project_id],
    )?)
}

/// The chats with exactly these ids, in the order asked (an id no longer
/// indexed is left out), with local titles.
pub fn by_ids(
    connection: &Connection,
    ids: &[String],
    local_title_version: u32,
) -> Result<Vec<IndexedConversation>> {
    let mut statement = connection.prepare_cached(
        "select c.id, c.title, c.create_time, c.update_time, c.is_archived, c.pinned, c.project_id,
            case when l.source = 'manual' or (l.update_time = c.update_time and l.version = ?)
                then l.title end as local_title
         from conversations c left join local_titles l on l.id = c.id where c.id = ?",
    )?;
    let mut chats = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(chat) = statement
            .query_row(params![local_title_version, id], indexed)
            .optional()?
        {
            chats.push(chat);
        }
    }
    Ok(chats)
}
