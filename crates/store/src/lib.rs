//! The daemon's index: SQLite in WAL mode, written by one connection and
//! read by another, so a long write never holds up `list`.
//!
//! rusqlite rather than sqlx (docs/issues/sqlite-crate-choice.md): the cookie
//! readers already use rusqlite 0.40, only one `libsqlite3-sys` can be in the
//! build, and the daemon's queries are few and synchronous. The daemon calls
//! the store from `spawn_blocking`.

mod conversations;
mod judgments;
mod local_titles;
mod meta;
mod reconcile;
mod schema;
mod search;
mod vectors;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use rusqlite::Connection;

pub use conversations::{IndexFilter, IndexedConversation, NewConversation};
pub use judgments::{
    JudgmentRow, MemoryJudgmentRow, NewDeepJudgment, NewJudgment, NewLunaJudgment,
    NewMemoryJudgment, Unjudged,
};
pub use local_titles::{ManualTitle, local_title_source, set_local_title, set_luna_title};
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
    /// Open (creating and migrating) the index at `path`. The file and its
    /// `-wal` and `-shm` are 0600: they hold chat titles and Jev's notes on
    /// them.
    pub fn open(path: &Path) -> Result<Self> {
        // Created 0600 before SQLite opens it: SQLite gives the `-wal` and
        // `-shm` it creates the database's permissions, so they never exist
        // readable by others, even for the first moment.
        create_private(path)?;
        restrict(path)?;
        let mut writer = Connection::open(path)?;
        writer.busy_timeout(BUSY_TIMEOUT)?;
        writer.pragma_update(None, "journal_mode", "wal")?;
        writer.pragma_update(None, "synchronous", "normal")?;
        schema::migrate(&mut writer)?;
        // Ones an older build left behind were made from the umask.
        for suffix in ["-wal", "-shm"] {
            let mut sidecar = path.as_os_str().to_owned();
            sidecar.push(suffix);
            let sidecar = PathBuf::from(sidecar);
            if sidecar.exists() {
                restrict(&sidecar)?;
            }
        }
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

fn create_private(path: &Path) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .map(drop)
        .map_err(|source| StoreError::Permissions {
            path: path.to_path_buf(),
            source,
        })
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
    active_watermark, all_ids, apply_delta, archived_ids, by_ids, clear_project, count_all, get,
    query, remove, rename, replace_all, set_archived, set_project, synced_at,
};
pub use judgments::{
    current_judgments, fresh_update_time, has_deep_judgment, has_luna_judgment, judgment,
    memory_judgment, newest_active_update_time, save_deep_judgment, save_judgment,
    save_luna_judgment, save_memory_judgment, save_memory_judgment_unless_newer, save_summary,
    summary, unjudged,
};
pub use meta::{account, bind_account, get_meta, set_meta};
pub use reconcile::{candidates, preserve};
pub use search::{
    coverage, lexical, prune_search, replace_chunks, save_indexed, transcript, unindexed,
};
pub use vectors::{
    chunk_body, each_vector, pending_vectors, record_vector_failure, save_vectors, vector_coverage,
    vector_failures,
};

#[cfg(test)]
mod search_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod vector_tests;
