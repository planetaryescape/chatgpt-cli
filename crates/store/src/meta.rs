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

/// The ChatGPT account the index holds chats for, from its first sync on.
pub fn account(connection: &Connection) -> Result<Option<String>> {
    get_meta(connection, "account_id")
}

/// Bind the index to `account` unless it's bound already, in one
/// statement, and return the account it's bound to: two requests that both
/// found it unbound can't both win.
pub fn bind_account(connection: &Connection, account: &str) -> Result<String> {
    connection.execute(
        "insert into meta values ('account_id', ?) on conflict (key) do nothing",
        [account],
    )?;
    Ok(connection.query_row(
        "select value from meta where key = 'account_id'",
        [],
        |row| row.get(0),
    )?)
}
