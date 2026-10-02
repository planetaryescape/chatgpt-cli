//! The lexical search index and the transcript cache it's built from.
//! Ported from the TS CLI's `src/search/store.ts` and the transcript half of
//! `src/index/classification-store.ts` @ 1b8c950.
//!
//! Chunk bodies are bytes, not `&str`: the TS CLI cuts transcripts into
//! chunks at UTF-16 offsets, which can split an emoji's surrogate pair, and
//! Bun hands SQLite such a string in a form that isn't always valid UTF-8.
//! The daemon reproduces those bytes so the FTS index, its ranking and its
//! snippets come out as the TS CLI's do.

use rusqlite::types::{ToSqlOutput, ValueRef};
use rusqlite::{Connection, OptionalExtension, ToSql, params};

use crate::Result;

/// A transcript as the cache holds it (`CachedTranscript`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transcript {
    pub id: String,
    /// The chat's `update_time` in the index when it was fetched.
    pub update_time: String,
    pub render_version: u32,
    pub markdown: String,
    pub turns: i64,
    pub approx_tokens: i64,
}

/// A chat whose search chunks aren't current, newest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unindexed {
    pub id: String,
    pub title: String,
    pub update_time: String,
    /// Whether the cache holds a current transcript to chunk.
    pub cached: bool,
}

/// The versions a chunk must carry to count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkVersions {
    pub render: u32,
    pub chunk: u32,
}

/// One chunk-level FTS match, best first (`lexical`'s SQL rows).
#[derive(Clone, Debug, PartialEq)]
pub struct LexicalRow {
    pub id: String,
    /// The chat title the chunk was indexed with.
    pub title: String,
    /// The chat's current `update_time`.
    pub updated: String,
    pub archived: bool,
    /// `bm25(search_fts, 5.0, 1.0)`: lower is better.
    pub bm25: f64,
    /// `snippet(…)` as SQLite returns it, possibly not UTF-8.
    pub snippet: Vec<u8>,
}

/// Chunk text, bound as SQLite TEXT without a UTF-8 check.
struct RawText<'a>(&'a [u8]);

impl ToSql for RawText<'_> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Borrowed(ValueRef::Text(self.0)))
    }
}

/// `ClassificationStore.transcript`: the cached transcript for exactly this
/// `update_time` and render version.
pub fn transcript(
    connection: &Connection,
    id: &str,
    update_time: &str,
    render_version: u32,
) -> Result<Option<Transcript>> {
    Ok(connection
        .prepare_cached(
            "select id, update_time, render_version, markdown, turns, approx_tokens
             from transcripts where id = ? and update_time = ? and render_version = ?",
        )?
        .query_row(params![id, update_time, render_version], |row| {
            Ok(Transcript {
                id: row.get(0)?,
                update_time: row.get(1)?,
                render_version: row.get(2)?,
                markdown: row.get(3)?,
                turns: row.get(4)?,
                approx_tokens: row.get(5)?,
            })
        })
        .optional()?)
}

/// `ClassificationStore.saveTranscript`.
fn save_transcript(connection: &Connection, transcript: &Transcript) -> Result<()> {
    connection
        .prepare_cached("insert or replace into transcripts values (?, ?, ?, ?, ?, ?)")?
        .execute(params![
            transcript.id,
            transcript.update_time,
            transcript.render_version,
            transcript.markdown,
            transcript.turns,
            transcript.approx_tokens,
        ])?;
    Ok(())
}

/// Every chat whose chunks aren't current (`SearchStore.isCurrent` is
/// false): no chunks, chunks for another `update_time`, version or title.
/// Pinned and archived chats included, newest first, as `refreshCached`
/// walks them for `--all`.
pub fn unindexed(connection: &Connection, versions: ChunkVersions) -> Result<Vec<Unindexed>> {
    let mut statement = connection.prepare(
        "select c.id, c.title, c.update_time,
            exists (select 1 from transcripts t
                where t.id = c.id and t.update_time = c.update_time and t.render_version = ?1)
         from conversations c
         where not exists (select 1 from search_indexed si
            join search_chunks sc on sc.conversation_id = si.conversation_id and sc.chunk_index = 0
            where si.conversation_id = c.id and si.update_time = c.update_time
            and si.render_version = ?1 and si.chunk_version = ?2 and sc.title = c.title)
         order by c.update_time desc",
    )?;
    let rows = statement
        .query_map(params![versions.render, versions.chunk], |row| {
            Ok(Unindexed {
                id: row.get(0)?,
                title: row.get(1)?,
                update_time: row.get(2)?,
                cached: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// `SearchStore.replace`: the chat's chunks, all of them new, in one
/// transaction with its `search_indexed` row.
pub fn replace_chunks(
    connection: &mut Connection,
    chat: &Unindexed,
    versions: ChunkVersions,
    bodies: &[Vec<u8>],
) -> Result<()> {
    let transaction = connection.transaction()?;
    write_chunks(&transaction, chat, versions, bodies)?;
    transaction.commit()?;
    Ok(())
}

/// A freshly fetched transcript and its chunks, together: a chat is never
/// left with one and not the other.
pub fn save_indexed(
    connection: &mut Connection,
    transcript: &Transcript,
    chat: &Unindexed,
    versions: ChunkVersions,
    bodies: &[Vec<u8>],
) -> Result<()> {
    let transaction = connection.transaction()?;
    save_transcript(&transaction, transcript)?;
    write_chunks(&transaction, chat, versions, bodies)?;
    transaction.commit()?;
    Ok(())
}

fn write_chunks(
    connection: &Connection,
    chat: &Unindexed,
    versions: ChunkVersions,
    bodies: &[Vec<u8>],
) -> Result<()> {
    connection
        .prepare_cached("delete from search_chunks where conversation_id = ?")?
        .execute([&chat.id])?;
    let mut insert = connection.prepare_cached(
        "insert into search_chunks
         (conversation_id, update_time, render_version, chunk_version, chunk_index, title, body)
         values (?, ?, ?, ?, ?, ?, ?)",
    )?;
    for (index, body) in bodies.iter().enumerate() {
        insert.execute(params![
            chat.id,
            chat.update_time,
            versions.render,
            versions.chunk,
            i64::try_from(index).unwrap_or(i64::MAX),
            chat.title,
            RawText(body),
        ])?;
    }
    connection
        .prepare_cached("insert or replace into search_indexed values (?, ?, ?, ?)")?
        .execute(params![
            chat.id,
            chat.update_time,
            versions.render,
            versions.chunk
        ])?;
    Ok(())
}

/// `SearchStore.prune`: drop the chunks of chats no longer indexed.
pub fn prune_search(connection: &mut Connection) -> Result<usize> {
    let transaction = connection.transaction()?;
    transaction.execute(
        "delete from search_chunks where conversation_id not in (select id from conversations)",
        [],
    )?;
    let pruned = transaction.execute(
        "delete from search_indexed where conversation_id not in (select id from conversations)",
        [],
    )?;
    transaction.commit()?;
    Ok(pruned)
}

/// Chats in scope, and how many of them have current chunks
/// (`SearchStore.coverage`'s `chats` and `indexed`). `archived`: `None` for
/// both.
pub fn coverage(
    connection: &Connection,
    archived: Option<bool>,
    versions: ChunkVersions,
) -> Result<(u64, u64)> {
    let scope = archived.map(i64::from);
    let (chats, indexed): (i64, i64) = connection.query_row(
        "select
            (select count(*) from conversations where (?1 is null or is_archived = ?1)),
            (select count(*) from search_indexed si join conversations c on c.id = si.conversation_id
                where si.update_time = c.update_time and si.render_version = ?2 and si.chunk_version = ?3
                and (?1 is null or c.is_archived = ?1))",
        params![scope, versions.render, versions.chunk],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok((
        u64::try_from(chats).unwrap_or(0),
        u64::try_from(indexed).unwrap_or(0),
    ))
}

/// `SearchStore.lexical`'s query: chunk matches for `fts_query`, best
/// first, only from chunks current for their chat, at most `max_rows`.
pub fn lexical(
    connection: &Connection,
    fts_query: &str,
    archived: Option<bool>,
    versions: ChunkVersions,
    max_rows: u64,
) -> Result<Vec<LexicalRow>> {
    let scope = archived.map(i64::from);
    let mut statement = connection.prepare_cached(
        "select sc.conversation_id as id, sc.title, c.update_time as updated,
            c.is_archived as archived, bm25(search_fts, 5.0, 1.0) as score,
            snippet(search_fts, 1, '', '', '…', 24) as snippet
            from search_fts join search_chunks sc on sc.id = search_fts.rowid
            join conversations c on c.id = sc.conversation_id and c.update_time = sc.update_time
            where search_fts match ? and sc.render_version = ? and sc.chunk_version = ?
            and (?4 is null or c.is_archived = ?4)
            order by score limit ?5",
    )?;
    let rows = statement
        .query_map(
            params![
                fts_query,
                versions.render,
                versions.chunk,
                scope,
                i64::try_from(max_rows).unwrap_or(i64::MAX)
            ],
            |row| {
                Ok(LexicalRow {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    updated: row.get(2)?,
                    archived: row.get::<_, i64>(3)? != 0,
                    bm25: row.get(4)?,
                    snippet: match row.get_ref(5)? {
                        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => bytes.to_vec(),
                        _ => Vec::new(),
                    },
                })
            },
        )?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}
