//! Reading the TS CLI's judgments. Ported from `ClassificationStore` and
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
/// earlier one, and its follow-up and Luna review, which no longer apply.
/// All three are marked as the daemon's, so the TS import neither drops the
/// judgment nor brings the old reviews back.
pub fn save_judgment(connection: &mut Connection, judgment: &NewJudgment) -> Result<()> {
    let transaction = connection.transaction()?;
    transaction.execute("delete from deep_judgments where id = ?", [&judgment.id])?;
    transaction.execute("delete from luna_judgments where id = ?", [&judgment.id])?;
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
    for table in ["judgments", "deep_judgments", "luna_judgments"] {
        crate::native::mark(&transaction, table, &judgment.id, &judgment.classified_at)?;
    }
    transaction.commit()?;
    Ok(())
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
