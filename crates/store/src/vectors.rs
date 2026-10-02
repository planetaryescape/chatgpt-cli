//! Semantic search's vectors: which chunks still need one, saving them, and
//! reading them back to rank. Ported from `pendingVectors`, `saveVectors`,
//! `coverage` and `semantic` in the TS CLI's `src/search/store.ts` @
//! 1b8c950.
//!
//! A chunk's text is `title || '\n' || body`.

use rusqlite::types::{Type, ValueRef};
use rusqlite::{Connection, OptionalExtension, params};

use crate::Result;
use crate::search::ChunkVersions;

/// A chunk that has no vector from `model_version` yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingChunk {
    pub id: i64,
    /// `title || '\n' || body`.
    pub text: String,
}

/// A vector to save, for the chunk text it was made from.
#[derive(Clone, Debug, PartialEq)]
pub struct NewVector {
    pub chunk_id: i64,
    /// The chunk's `title || '\n' || body` when it was read.
    pub text: String,
    /// Little-endian `f32`s.
    pub embedding: Vec<u8>,
}

/// One stored vector, with what ranking needs from its chunk and chat.
pub struct VectorRow<'a> {
    pub chunk_id: i64,
    /// The chunk's place in its chat, the same in every index.
    pub chunk_index: i64,
    pub conversation_id: &'a str,
    /// The chat title the chunk was indexed with.
    pub title: &'a str,
    /// The chat's current `update_time`.
    pub updated: &'a str,
    pub archived: bool,
    pub embedding: &'a [u8],
}

fn text<'a>(row: &'a rusqlite::Row<'_>, index: usize) -> rusqlite::Result<&'a str> {
    row.get_ref(index)?.as_str().map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
    })
}

/// `pendingVectors` for every chat (`archived` null), in id order, from
/// after `after_id`, at most `limit`: only chunks current for their chat,
/// without a vector from `model_version`, and not set aside after a
/// failure until after `now` (unix seconds).
pub fn pending_vectors(
    connection: &Connection,
    versions: ChunkVersions,
    model_version: &str,
    after_id: i64,
    limit: usize,
    now: i64,
) -> Result<Vec<PendingChunk>> {
    let mut statement = connection.prepare_cached(
        "select sc.id, sc.title || char(10) || sc.body from search_chunks sc
         join conversations c on c.id = sc.conversation_id and c.update_time = sc.update_time
         left join search_vectors v on v.chunk_id = sc.id
         where sc.render_version = ?1 and sc.chunk_version = ?2 and sc.title = c.title
         and (v.chunk_id is null or v.model_version != ?3)
         and sc.id > ?4
         and not exists (select 1 from search_vector_failures f
            where f.chunk_id = sc.id and f.model_version = ?3 and f.retry_at > ?6)
         order by sc.id limit ?5",
    )?;
    let rows = statement
        .query_map(
            params![
                versions.render,
                versions.chunk,
                model_version,
                after_id,
                i64::try_from(limit).unwrap_or(i64::MAX),
                now
            ],
            |row| {
                Ok(PendingChunk {
                    id: row.get(0)?,
                    text: row.get(1)?,
                })
            },
        )?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// `saveVectors`, in one transaction. A vector is kept only if its chunk
/// still holds the text it was made from: the indexer may have replaced
/// the chunk meanwhile, and SQLite may have given its id to a new one.
/// Returns how many were saved.
pub fn save_vectors(
    connection: &mut Connection,
    vectors: &[NewVector],
    model_version: &str,
) -> Result<usize> {
    let transaction = connection.transaction()?;
    let mut saved = 0;
    {
        let mut insert = transaction.prepare_cached(
            "insert or replace into search_vectors (chunk_id, model_version, embedding)
             select ?1, ?2, ?3 where exists (
                select 1 from search_chunks
                where id = ?1 and title || char(10) || body = ?4)",
        )?;
        let mut clear =
            transaction.prepare_cached("delete from search_vector_failures where chunk_id = ?")?;
        for vector in vectors {
            saved += insert.execute(params![
                vector.chunk_id,
                model_version,
                vector.embedding,
                vector.text
            ])?;
            clear.execute([vector.chunk_id])?;
        }
    }
    transaction.commit()?;
    Ok(saved)
}

/// Set chunk `chunk_id` aside until `retry_at` (unix seconds): the model
/// failed on it.
pub fn record_vector_failure(
    connection: &Connection,
    chunk_id: i64,
    model_version: &str,
    retry_at: i64,
) -> Result<()> {
    connection
        .prepare_cached("insert or replace into search_vector_failures values (?, ?, ?)")?
        .execute(params![chunk_id, model_version, retry_at])?;
    Ok(())
}

/// How many chunks are set aside for `model_version` after `now`.
pub fn vector_failures(connection: &Connection, model_version: &str, now: i64) -> Result<u64> {
    let count: i64 = connection.query_row(
        "select count(*) from search_vector_failures where model_version = ? and retry_at > ?",
        params![model_version, now],
        |row| row.get(0),
    )?;
    Ok(u64::try_from(count).unwrap_or(0))
}

/// `coverage`'s `chunks` and `embedded`: chunks current for their chat in
/// scope, and how many of them have a vector from `model_version`.
/// `archived`: `None` for both.
pub fn vector_coverage(
    connection: &Connection,
    archived: Option<bool>,
    versions: ChunkVersions,
    model_version: &str,
) -> Result<(u64, u64)> {
    let scope = archived.map(i64::from);
    let (chunks, embedded): (i64, i64) = connection.query_row(
        "select
            (select count(*) from search_chunks sc join conversations c on c.id = sc.conversation_id
                where sc.update_time = c.update_time and sc.render_version = ?2 and sc.chunk_version = ?3
                and sc.title = c.title and (?1 is null or c.is_archived = ?1)),
            (select count(*) from search_vectors v join search_chunks sc on sc.id = v.chunk_id
                join conversations c on c.id = sc.conversation_id and c.update_time = sc.update_time
                where v.model_version = ?4 and sc.render_version = ?2 and sc.chunk_version = ?3
                and sc.title = c.title and (?1 is null or c.is_archived = ?1))",
        params![scope, versions.render, versions.chunk, model_version],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok((
        u64::try_from(chunks).unwrap_or(0),
        u64::try_from(embedded).unwrap_or(0),
    ))
}

/// `semantic`'s query: every vector from `model_version` on a chunk
/// current for its chat in scope, handed to `each` in no particular order.
pub fn each_vector(
    connection: &Connection,
    archived: Option<bool>,
    versions: ChunkVersions,
    model_version: &str,
    mut each: impl FnMut(VectorRow<'_>) -> Result<()>,
) -> Result<()> {
    let scope = archived.map(i64::from);
    let mut statement = connection.prepare_cached(
        "select sc.id as chunk_id, sc.conversation_id, sc.title,
            c.update_time, c.is_archived, v.embedding, sc.chunk_index from search_vectors v
            join search_chunks sc on sc.id = v.chunk_id
            join conversations c on c.id = sc.conversation_id and c.update_time = sc.update_time
            where v.model_version = ? and sc.render_version = ? and sc.chunk_version = ?
            and sc.title = c.title and (?4 is null or c.is_archived = ?4)",
    )?;
    let mut rows = statement.query(params![
        model_version,
        versions.render,
        versions.chunk,
        scope
    ])?;
    while let Some(row) = rows.next()? {
        let embedding = match row.get_ref(5)? {
            ValueRef::Blob(bytes) | ValueRef::Text(bytes) => bytes,
            _ => &[],
        };
        each(VectorRow {
            chunk_id: row.get(0)?,
            chunk_index: row.get(6)?,
            conversation_id: text(row, 1)?,
            title: text(row, 2)?,
            updated: text(row, 3)?,
            archived: row.get::<_, i64>(4)? != 0,
            embedding,
        })?;
    }
    Ok(())
}

/// A chunk's body as stored, for its excerpt.
pub fn chunk_body(connection: &Connection, chunk_id: i64) -> Result<Option<String>> {
    Ok(connection
        .prepare_cached("select body from search_chunks where id = ?")?
        .query_row([chunk_id], |row| row.get(0))
        .optional()?)
}
