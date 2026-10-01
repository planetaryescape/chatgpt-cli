//! Small daemon state in the `meta` table. `synced_at` is the TS CLI's key.

use rusqlite::{Connection, OptionalExtension, params};

use crate::Result;

pub fn get_meta(connection: &Connection, key: &str) -> Result<Option<String>> {
    Ok(connection
        .query_row("select value from meta where key = ?", [key], |row| {
            row.get(0)
        })
        .optional()?)
}

pub fn set_meta(connection: &Connection, key: &str, value: &str) -> Result<()> {
    connection.execute(
        "insert or replace into meta values (?, ?)",
        params![key, value],
    )?;
    Ok(())
}
