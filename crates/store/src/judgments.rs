//! Judgments, their follow-ups and Luna reviews, summaries and saved-memory
//! classifications. Ported from `ClassificationStore` and
//! `MemoryClassificationStore` in the TS CLI's `src/index/` @ 1b8c950.

use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension, params};

use crate::Result;

/// A judgment with its follow-up and Luna review joined, as `Judgment` in
/// classification-store.ts. Verdicts are computed from it on read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JudgmentRow {
    pub id: String,
    pub update_time: String,
    pub version: String,
    pub content_kind: String,
    /// Jev's raw answers as JSON.
    pub answers: String,
    pub classified_at: String,
    /// `answers.topic.choice`, from the generated column.
    pub topic: Option<String>,
    pub deep_answers: Option<String>,
    pub deep_version: Option<String>,
    pub luna_suggestion: Option<String>,
    pub luna_brainstorm: Option<String>,
    pub luna_reason: Option<String>,
    pub luna_version: Option<i64>,
}

/// Current judgments only: a judgment for an older `update_time` (the chat
/// changed) or other questions doesn't count. `currentJudgments` in
/// classification-store.ts.
pub fn current_judgments(
    connection: &Connection,
    questions_version: &str,
) -> Result<HashMap<String, JudgmentRow>> {
    let mut statement = connection.prepare_cached(&format!(
        "{JUDGMENT_SELECT} join conversations c on c.id = j.id
         where j.update_time = c.update_time and j.version = ?"
    ))?;
    let rows = statement
        .query_map([questions_version], judgment_row)?
        .map(|row| row.map(|judgment| (judgment.id.clone(), judgment)))
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// A judgment joined with its follow-up and Luna review, as
/// `ClassificationStore` selects it.
const JUDGMENT_SELECT: &str = "select j.id, j.update_time, j.version, j.content_kind, j.answers,
    j.classified_at, cast(j.topic as text), d.answers, d.version,
    l.suggestion, l.brainstorm, l.reason, l.version
 from judgments j
 left join deep_judgments d
    on d.id = j.id and d.update_time = j.update_time and d.questions_version = j.version
 left join luna_judgments l
    on l.id = j.id and l.update_time = j.update_time and l.questions_version = j.version
    and l.deep_version = coalesce(d.version, '')";

fn judgment_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<JudgmentRow> {
    Ok(JudgmentRow {
        id: row.get(0)?,
        update_time: row.get(1)?,
        version: row.get(2)?,
        content_kind: row.get(3)?,
        answers: row.get(4)?,
        classified_at: row.get(5)?,
        topic: row.get(6)?,
        deep_answers: row.get(7)?,
        deep_version: row.get(8)?,
        luna_suggestion: row.get(9)?,
        luna_brainstorm: row.get(10)?,
        luna_reason: row.get(11)?,
        luna_version: row.get(12)?,
    })
}

/// `ClassificationStore.judgment`: chat `id`'s judgment for exactly this
/// `update_time` and questions version, with its follow-up and Luna review.
pub fn judgment(
    connection: &Connection,
    id: &str,
    update_time: &str,
    version: &str,
) -> Result<Option<JudgmentRow>> {
    Ok(connection
        .prepare_cached(&format!(
            "{JUDGMENT_SELECT} where j.id = ? and j.update_time = ? and j.version = ?"
        ))?
        .query_row(params![id, update_time, version], judgment_row)
        .optional()?)
}

/// A judgment the daemon made (the Jev guard), as `saveJudgment` writes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewJudgment {
    pub id: String,
    pub update_time: String,
    pub version: String,
    /// `full` or `summary`.
    pub content_kind: String,
    /// Jev's answers, as `JSON.stringify` writes them.
    pub answers: String,
    /// ISO 8601, `toISOString`'s form.
    pub classified_at: String,
}

/// `ClassificationStore.saveJudgment`: the judgment replaces the chat's
/// earlier one. Follow-ups and Luna reviews of another judgment (an older
/// `update_time` or question version) go with it; ones for this very
/// judgment's key stay, so a second first pass for the same chat and
/// version (the background Jev racing `classify`, a time-bound refresh)
/// never discards a follow-up or review already paid for.
pub fn save_judgment(connection: &mut Connection, judgment: &NewJudgment) -> Result<()> {
    let transaction = connection.transaction()?;
    for table in ["deep_judgments", "luna_judgments"] {
        transaction.execute(
            &format!(
                "delete from {table} where id = ? and (update_time is not ? or questions_version is not ?)"
            ),
            params![judgment.id, judgment.update_time, judgment.version],
        )?;
    }
    transaction.execute(
        "insert or replace into judgments (id, update_time, version, content_kind, answers, classified_at)
         values (?, ?, ?, ?, ?, ?)",
        params![
            judgment.id,
            judgment.update_time,
            judgment.version,
            judgment.content_kind,
            judgment.answers,
            judgment.classified_at
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

/// The chat's current `update_time`, if its cached transcript there (at
/// `render_version`) is still `markdown`: a result computed from that
/// content is current at that time, even when a sync moved the chat's
/// caches forward meanwhile. `None` when the content changed (or isn't
/// cached any more): a result from the old content mustn't land.
pub fn fresh_update_time(
    connection: &Connection,
    id: &str,
    render_version: u32,
    markdown: &str,
) -> Result<Option<String>> {
    Ok(connection
        .prepare_cached(
            "select c.update_time from conversations c join transcripts t
               on t.id = c.id and t.update_time = c.update_time and t.render_version = ?
             where c.id = ? and t.markdown = ?",
        )?
        .query_row(params![render_version, id, markdown], |row| row.get(0))
        .optional()?)
}

/// `ClassificationStore.summary`: the cached summary text of a long chat,
/// for exactly this `update_time` and prompt version.
pub fn summary(
    connection: &Connection,
    id: &str,
    update_time: &str,
    prompt_version: u32,
) -> Result<Option<String>> {
    Ok(connection
        .prepare_cached(
            "select summary from summaries where id = ? and update_time = ? and prompt_version = ?",
        )?
        .query_row(params![id, update_time, prompt_version], |row| row.get(0))
        .optional()?)
}

/// `ClassificationStore.saveSummary`.
pub fn save_summary(
    connection: &Connection,
    id: &str,
    update_time: &str,
    prompt_version: u32,
    summary: &str,
    model: &str,
) -> Result<()> {
    connection
        .prepare_cached("insert or replace into summaries values (?, ?, ?, ?, ?)")?
        .execute(params![id, update_time, prompt_version, summary, model])?;
    Ok(())
}

/// `ClassificationStore.deepJudgment`, as a yes or no: whether chat `id`
/// has a follow-up for exactly these versions.
pub fn has_deep_judgment(
    connection: &Connection,
    id: &str,
    update_time: &str,
    questions_version: &str,
    version: &str,
) -> Result<bool> {
    Ok(connection
        .prepare_cached(
            "select 1 from deep_judgments
             where id = ? and update_time = ? and questions_version = ? and version = ?",
        )?
        .query_row(params![id, update_time, questions_version, version], |_| {
            Ok(())
        })
        .optional()?
        .is_some())
}

/// A follow-up (deep) judgment, as `saveDeepJudgment` writes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewDeepJudgment {
    pub id: String,
    pub update_time: String,
    pub questions_version: String,
    pub version: String,
    pub answers: String,
    pub classified_at: String,
}

/// `ClassificationStore.saveDeepJudgment`: the chat's Luna review, which
/// rested on the old follow-up, goes.
pub fn save_deep_judgment(connection: &mut Connection, judgment: &NewDeepJudgment) -> Result<()> {
    let transaction = connection.transaction()?;
    transaction.execute("delete from luna_judgments where id = ?", [&judgment.id])?;
    transaction.execute(
        "insert or replace into deep_judgments values (?, ?, ?, ?, ?, ?)",
        params![
            judgment.id,
            judgment.update_time,
            judgment.questions_version,
            judgment.version,
            judgment.answers,
            judgment.classified_at
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

/// `ClassificationStore.lunaJudgment`: whether chat `id` has a Luna review
/// for exactly these versions.
pub fn has_luna_judgment(
    connection: &Connection,
    id: &str,
    update_time: &str,
    questions_version: &str,
    deep_version: &str,
    version: i64,
) -> Result<bool> {
    Ok(connection
        .prepare_cached(
            "select 1 from luna_judgments where id = ? and update_time = ?
             and questions_version = ? and deep_version = ? and version = ?",
        )?
        .query_row(
            params![id, update_time, questions_version, deep_version, version],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// A Luna review, as `saveLunaJudgment` writes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewLunaJudgment {
    pub id: String,
    pub update_time: String,
    pub questions_version: String,
    pub deep_version: String,
    pub version: i64,
    pub suggestion: String,
    pub brainstorm: Option<String>,
    pub reason: String,
    pub classified_at: String,
}

pub fn save_luna_judgment(connection: &Connection, judgment: &NewLunaJudgment) -> Result<()> {
    connection
        .prepare_cached("insert or replace into luna_judgments values (?, ?, ?, ?, ?, ?, ?, ?, ?)")?
        .execute(params![
            judgment.id,
            judgment.update_time,
            judgment.questions_version,
            judgment.deep_version,
            judgment.version,
            judgment.suggestion,
            judgment.brainstorm,
            judgment.reason,
            judgment.classified_at
        ])?;
    Ok(())
}

/// A saved memory's classification, as `MemoryClassificationStore.save`
/// writes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewMemoryJudgment {
    pub id: String,
    pub input_hash: String,
    pub version: String,
    pub system_one: String,
    pub system_two: Option<String>,
    pub classified_at: String,
}

pub fn save_memory_judgment(connection: &Connection, row: &NewMemoryJudgment) -> Result<()> {
    connection
        .prepare_cached("insert or replace into memory_judgments values (?, ?, ?, ?, ?, ?)")?
        .execute(params![
            row.id,
            row.input_hash,
            row.version,
            row.system_one,
            row.system_two,
            row.classified_at
        ])?;
    Ok(())
}

/// What the background Jev may judge: active, unpinned chats (what a bare
/// `chatgpt classify` picks) updated after `after`, with no judgment for
/// their `update_time` at `questions_version`, and not a cached transcript
/// over `max_tokens` without a summary (those wait for `classify`).
#[derive(Clone, Copy, Debug)]
pub struct Unjudged<'a> {
    pub after: &'a str,
    pub questions_version: &'a str,
    pub render_version: u32,
    pub max_tokens: i64,
    pub summary_version: u32,
    pub limit: usize,
}

/// [`Unjudged`]'s chats, newest first: `(id, update_time)`.
pub fn unjudged(connection: &Connection, query: Unjudged<'_>) -> Result<Vec<(String, String)>> {
    let mut statement = connection.prepare_cached(
        "select c.id, c.update_time from conversations c
         where c.is_archived = 0 and c.pinned = 0 and c.update_time > ?
         and not exists (select 1 from judgments j
            where j.id = c.id and j.update_time = c.update_time and j.version = ?)
         and not exists (select 1 from transcripts t
            where t.id = c.id and t.update_time = c.update_time and t.render_version = ?
            and t.approx_tokens > ?
            and not exists (select 1 from summaries s
               where s.id = c.id and s.update_time = c.update_time and s.prompt_version = ?))
         order by c.update_time desc limit ?",
    )?;
    let rows = statement
        .query_map(
            params![
                query.after,
                query.questions_version,
                query.render_version,
                query.max_tokens,
                query.summary_version,
                i64::try_from(query.limit).unwrap_or(i64::MAX)
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// [`save_memory_judgment`], unless another run saved this memory's row
/// for other input since `since` (an ISO time): a late result for an older
/// input never displaces a newer one. Whether it was written.
pub fn save_memory_judgment_unless_newer(
    connection: &Connection,
    row: &NewMemoryJudgment,
    since: &str,
) -> Result<bool> {
    let written = connection
        .prepare_cached(
            "insert into memory_judgments values (?, ?, ?, ?, ?, ?)
             on conflict (id) do update set input_hash = excluded.input_hash,
                version = excluded.version, system_one = excluded.system_one,
                system_two = excluded.system_two, classified_at = excluded.classified_at
             where memory_judgments.input_hash = excluded.input_hash
                or memory_judgments.classified_at < ?",
        )?
        .execute(params![
            row.id,
            row.input_hash,
            row.version,
            row.system_one,
            row.system_two,
            row.classified_at,
            since
        ])?;
    Ok(written > 0)
}

/// A saved memory's cached classification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryJudgmentRow {
    pub system_one: String,
    pub system_two: Option<String>,
}

/// The cached classification of memory `id` for exactly this input and
/// version.
pub fn memory_judgment(
    connection: &Connection,
    id: &str,
    input_hash: &str,
    version: &str,
) -> Result<Option<MemoryJudgmentRow>> {
    Ok(connection
        .prepare_cached(
            "select system_one, system_two from memory_judgments
             where id = ? and input_hash = ? and version = ?",
        )?
        .query_row(params![id, input_hash, version], |row| {
            Ok(MemoryJudgmentRow {
                system_one: row.get(0)?,
                system_two: row.get(1)?,
            })
        })
        .optional()?)
}
