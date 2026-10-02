//! Import from the TS CLI's index (D2). While the bridge exists the TS CLI
//! writes judgments, titles, summaries and transcripts; the daemon copies
//! them so `list` and `stats` show them.
//!
//! The TS index is attached read-only (`mode=ro`): the import never writes
//! to it. Each table is mirrored by its TS key (`id`): rows the TS index no
//! longer has are deleted (a re-judgment drops its old follow-ups, a deleted
//! chat its title), new rows are inserted, and changed rows replaced, so
//! running it twice changes nothing. One exception keeps it from undoing the
//! daemon's own reconcile: a row that differs only in `update_time` is left
//! alone while its `update_time` is the chat's current one here. (Comparing
//! the times themselves doesn't work: the TS index holds some as
//! `toISOString` milliseconds and others as the list's microseconds.)
//!
//! Transcripts are a cache both sides fill: the daemon's search indexer
//! fetches them too. So the TS index never deletes one here, and its copy
//! replaces the daemon's only while the daemon's isn't current for the
//! chat's `update_time` (two current renders can differ only in the model
//! named in the header, which the single-chat endpoint gives and the batch
//! doesn't).

use std::path::Path;

use rusqlite::Connection;

use crate::Result;

/// The tables the import copies, their columns (the key first), and whether
/// they carry the chat's `update_time`.
pub const LEGACY_TABLES: &[(&str, &[&str], bool)] = &[
    (
        "local_titles",
        &[
            "id",
            "update_time",
            "version",
            "source",
            "title",
            "theme",
            "updated_at",
        ],
        true,
    ),
    (
        "judgments",
        &[
            "id",
            "update_time",
            "version",
            "content_kind",
            "answers",
            "classified_at",
        ],
        true,
    ),
    (
        "deep_judgments",
        &[
            "id",
            "update_time",
            "questions_version",
            "version",
            "answers",
            "classified_at",
        ],
        true,
    ),
    (
        "luna_judgments",
        &[
            "id",
            "update_time",
            "questions_version",
            "deep_version",
            "version",
            "suggestion",
            "brainstorm",
            "reason",
            "classified_at",
        ],
        true,
    ),
    (
        "memory_judgments",
        &[
            "id",
            "input_hash",
            "version",
            "system_one",
            "system_two",
            "classified_at",
        ],
        false,
    ),
    (
        "summaries",
        &["id", "update_time", "prompt_version", "summary", "model"],
        true,
    ),
    (
        "transcripts",
        &[
            "id",
            "update_time",
            "render_version",
            "markdown",
            "turns",
            "approx_tokens",
        ],
        true,
    ),
];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportCounts {
    pub tables: Vec<TableCounts>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableCounts {
    pub table: String,
    /// Rows in the TS index.
    pub rows: u64,
    pub inserted: u64,
    pub updated: u64,
    pub deleted: u64,
    /// Why the table was left alone: the TS index lacks it or a column.
    pub skipped: Option<String>,
}

/// Mirror the TS index at `legacy` into this one, in one transaction.
pub fn import_legacy(connection: &mut Connection, legacy: &Path) -> Result<ImportCounts> {
    connection.execute("attach database ? as legacy", [file_uri(legacy)])?;
    let imported = import_attached(connection);
    // Detach whatever happened, so the next import can attach again.
    let detached = connection.execute("detach database legacy", []);
    let counts = imported?;
    detached?;
    Ok(counts)
}

fn import_attached(connection: &mut Connection) -> Result<ImportCounts> {
    let transaction = connection.transaction()?;
    let mut counts = ImportCounts::default();
    for (table, columns, has_update_time) in LEGACY_TABLES {
        let mut table_counts = TableCounts {
            table: (*table).to_owned(),
            ..TableCounts::default()
        };
        if let Some(missing) = missing_columns(&transaction, table, columns)? {
            table_counts.skipped = Some(missing);
            counts.tables.push(table_counts);
            continue;
        }
        table_counts.rows = count(
            &transaction,
            &format!("select count(*) from legacy.{table}"),
        )?;
        let cache = *table == "transcripts";
        if !cache {
            table_counts.deleted = transaction.execute(
                &format!(
                    "delete from main.{table} where id not in (select id from legacy.{table})"
                ),
                [],
            )? as u64;
        }
        table_counts.inserted = count(
            &transaction,
            &format!(
                "select count(*) from legacy.{table} where id not in (select id from main.{table})"
            ),
        )?;
        let list = columns.join(", ");
        let content: Vec<&&str> = columns
            .iter()
            .skip(1)
            .filter(|column| !(*has_update_time && **column == "update_time"))
            .collect();
        let assignments = columns
            .iter()
            .skip(1)
            .map(|column| format!("{column} = excluded.{column}"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut changed = content
            .iter()
            .map(|column| format!("{table}.{column} is not excluded.{column}"))
            .collect::<Vec<_>>();
        if *has_update_time {
            changed.push(format!(
                "({table}.update_time is not excluded.update_time and not exists \
                 (select 1 from main.conversations c \
                 where c.id = {table}.id and c.update_time = {table}.update_time))"
            ));
        }
        let mut condition = format!("({})", changed.join(" or "));
        if cache {
            condition.push_str(&format!(
                " and not exists (select 1 from main.conversations c \
                 where c.id = {table}.id and c.update_time = {table}.update_time \
                 and {table}.render_version >= excluded.render_version)"
            ));
        }
        // `where true` lets SQLite tell the upsert's ON from a join's.
        let upserted = transaction.execute(
            &format!(
                "insert into main.{table} ({list}) select {list} from legacy.{table} where true
                 on conflict (id) do update set {assignments} where {condition}"
            ),
            [],
        )? as u64;
        table_counts.updated = upserted.saturating_sub(table_counts.inserted);
        counts.tables.push(table_counts);
    }
    transaction.commit()?;
    Ok(counts)
}

fn count(connection: &Connection, sql: &str) -> Result<u64> {
    let count: i64 = connection.query_row(sql, [], |row| row.get(0))?;
    Ok(u64::try_from(count).unwrap_or(0))
}

/// `None` when the TS index has `table` with every one of `columns`.
fn missing_columns(
    connection: &Connection,
    table: &str,
    columns: &[&str],
) -> Result<Option<String>> {
    let mut statement = connection.prepare(&format!("pragma legacy.table_info({table})"))?;
    let present: Vec<String> = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<_>>()?;
    if present.is_empty() {
        return Ok(Some("not in the TS index".to_owned()));
    }
    let missing: Vec<&str> = columns
        .iter()
        .filter(|column| !present.iter().any(|name| name == *column))
        .copied()
        .collect();
    Ok((!missing.is_empty()).then(|| format!("the TS index lacks {}", missing.join(", "))))
}

/// A read-only SQLite URI for `path`.
fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file:");
    for byte in path.to_string_lossy().bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri.push_str("?mode=ro");
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_uri_escapes_what_sqlite_would_misread() {
        assert_eq!(
            file_uri(Path::new("/Users/a b/x?y#z%.db")),
            "file:/Users/a%20b/x%3Fy%23z%25.db?mode=ro"
        );
    }
}
