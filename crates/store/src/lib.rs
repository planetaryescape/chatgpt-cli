//! The daemon's index: SQLite in WAL mode, written by one connection and
//! read by another, so a long write (an import) never holds up `list`.
//!
//! rusqlite rather than sqlx (docs/issues/sqlite-crate-choice.md): the cookie
//! readers already use rusqlite 0.40, only one `libsqlite3-sys` can be in the
//! build, and the daemon's queries are few and synchronous. The daemon calls
//! the store from `spawn_blocking`.

mod conversations;
mod judgments;
mod legacy;
mod local_titles;
mod meta;
mod native;
mod reconcile;
mod schema;
mod search;
mod vectors;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use rusqlite::Connection;

pub use conversations::{IndexFilter, IndexedConversation, NewConversation};
pub use judgments::{JudgmentRow, MemoryJudgmentRow, NewJudgment};
pub use legacy::{ImportCounts, LEGACY_TABLES, TableCounts};
pub use local_titles::{ManualTitle, set_local_title};
pub use reconcile::Candidate;
pub use search::{ChunkVersions, LexicalRow, Transcript, Unindexed};
pub use vectors::{NewVector, PendingChunk, VectorRow};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("cannot set the permissions of {path}: {source}")]
    Permissions {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The index has a migration this build doesn't know.
    #[error("the index is at schema version {found}, newer than this build's {known}")]
    NewerDatabase { found: i64, known: i64 },
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// How long a connection waits on another's lock before failing.
const BUSY_TIMEOUT: Duration = Duration::from_secs(10);

pub struct Store {
    path: PathBuf,
    writer: Mutex<Connection>,
    reader: Mutex<Connection>,
}

impl Store {
    /// Open (creating and migrating) the index at `path`. The file is 0600:
    /// it holds chat titles and Jev's notes on them.
    pub fn open(path: &Path) -> Result<Self> {
        let mut writer = Connection::open(path)?;
        writer.busy_timeout(BUSY_TIMEOUT)?;
        writer.pragma_update(None, "journal_mode", "wal")?;
        writer.pragma_update(None, "synchronous", "normal")?;
        schema::migrate(&mut writer)?;
        restrict(path)?;
        let reader = Connection::open(path)?;
        reader.busy_timeout(BUSY_TIMEOUT)?;
        Ok(Self {
            path: path.to_path_buf(),
            writer: Mutex::new(writer),
            reader: Mutex::new(reader),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Run `work` on the writer connection. Blocking: call it from
    /// `spawn_blocking`.
    pub fn write<T>(&self, work: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut connection = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        work(&mut connection)
    }

    /// Run `work` on the reader connection, which never waits for a writer
    /// (WAL). Blocking: call it from `spawn_blocking`.
    pub fn read<T>(&self, work: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let connection = self.reader.lock().unwrap_or_else(PoisonError::into_inner);
        work(&connection)
    }
}

fn restrict(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|source| {
        StoreError::Permissions {
            path: path.to_path_buf(),
            source,
        }
    })
}

pub use conversations::{
    active_watermark, all_ids, apply_delta, archived_ids, by_ids, count_all, get, query, remove,
    rename, replace_all, set_archived, set_project, synced_at,
};
pub use judgments::{current_judgments, judgment, memory_judgment, save_judgment, summary};
pub use legacy::import_legacy;
pub use meta::{account, get_meta, set_account, set_meta};
pub use reconcile::{candidates, preserve};
pub use search::{
    coverage, lexical, prune_search, replace_chunks, save_indexed, transcript, unindexed,
};
pub use vectors::{chunk_body, each_vector, pending_vectors, save_vectors, vector_coverage};

#[cfg(test)]
mod native_tests;
#[cfg(test)]
mod search_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod vector_tests;
