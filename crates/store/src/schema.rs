//! Embedded migrations, tracked in SQLite's `user_version`.

use rusqlite::Connection;

use crate::{Result, StoreError};

/// Each migration's SQL, in order; the version is its position plus one.
const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_index.sql"),
    include_str!("../migrations/0002_search.sql"),
    include_str!("../migrations/0003_search_follows_transcripts.sql"),
];

pub(crate) fn migrate(connection: &mut Connection) -> Result<()> {
    let known = i64::try_from(MIGRATIONS.len()).unwrap_or(i64::MAX);
    let found: i64 = connection.query_row("pragma user_version", [], |row| row.get(0))?;
    if found > known {
        return Err(StoreError::NewerDatabase { found, known });
    }
    for (version, sql) in (1..)
        .zip(MIGRATIONS)
        .skip(usize::try_from(found).unwrap_or(0))
    {
        let transaction = connection.transaction()?;
        transaction.execute_batch(sql)?;
        transaction.pragma_update(None, "user_version", version)?;
        transaction.commit()?;
    }
    Ok(())
}
