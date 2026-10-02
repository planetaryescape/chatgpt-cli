//! Rows the daemon wrote itself, which the import from the TS index must
//! leave alone (migrations/0005_native_rows.sql). Bridge-only: the import
//! goes when the TS CLI does.

use rusqlite::{Connection, params};

use crate::Result;

/// Record that the daemon wrote `table`'s row for `id`, stamped `written_at`.
pub(crate) fn mark(connection: &Connection, table: &str, id: &str, written_at: &str) -> Result<()> {
    connection
        .prepare_cached("insert or replace into native_rows values (?, ?, ?)")?
        .execute(params![table, id, written_at])?;
    Ok(())
}

pub(crate) fn forget(connection: &Connection, table: &str, id: &str) -> Result<()> {
    connection
        .prepare_cached("delete from native_rows where tbl = ? and id = ?")?
        .execute(params![table, id])?;
    Ok(())
}

/// The column a table's rows are stamped with, for the tables the daemon
/// writes rows of.
pub(crate) fn stamp(table: &str) -> Option<&'static str> {
    match table {
        "local_titles" => Some("updated_at"),
        "judgments" | "deep_judgments" | "luna_judgments" => Some("classified_at"),
        _ => None,
    }
}
