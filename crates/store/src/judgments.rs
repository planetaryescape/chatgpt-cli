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
    let mut statement = connection.prepare_cached(
        "select j.id, j.update_time, j.version, j.content_kind, j.answers, j.classified_at,
            cast(j.topic as text),
            d.answers, d.version,
            l.suggestion, l.brainstorm, l.reason, l.version
         from judgments j join conversations c on c.id = j.id
         left join deep_judgments d
            on d.id = j.id and d.update_time = j.update_time and d.questions_version = j.version
         left join luna_judgments l
            on l.id = j.id and l.update_time = j.update_time and l.questions_version = j.version
            and l.deep_version = coalesce(d.version, '')
         where j.update_time = c.update_time and j.version = ?",
    )?;
    let rows = statement
        .query_map([questions_version], |row| {
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
        })?
        .map(|row| row.map(|judgment| (judgment.id.clone(), judgment)))
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
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
