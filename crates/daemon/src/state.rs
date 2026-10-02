//! What every part of the daemon shares.

use std::sync::Arc;

use chatgpt_core::Paths;
use chatgpt_store::Store;
use rusqlite::Connection;

use crate::classify::auto_jev::AutoJev;
use crate::handlers::Failure;
use crate::policy::Profile;
use crate::progress::Reporter;
use crate::search::embedder::Embedder;
use crate::search::indexer::Indexer;
use crate::session::Sessions;
use crate::sync::Syncer;

/// Reports another version in `Status`, in debug builds only, so tests can
/// check how clients treat older and newer daemons.
const VERSION_OVERRIDE_ENV: &str = "CHATGPT_DAEMON_VERSION";

pub struct State {
    pub paths: Paths,
    pub store: Arc<Store>,
    pub sessions: Arc<Sessions>,
    pub reporter: Reporter,
    pub syncer: Syncer,
    pub indexer: Indexer,
    pub embedder: Embedder,
    pub auto_jev: AutoJev,
    /// The chats a classification is judging now.
    pub flight: crate::classify::flight::InFlight,
    pub started_at: i64,
    pub version: String,
}

pub fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

impl State {
    pub fn new(paths: Paths, store: Store) -> Self {
        let reporter = Reporter::default();
        let sessions = Arc::new(Sessions::default());
        let version = std::env::var(VERSION_OVERRIDE_ENV)
            .ok()
            .filter(|version| cfg!(debug_assertions) && !version.is_empty())
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned());
        let synced_age = store
            .read(chatgpt_store::synced_at)
            .ok()
            .flatten()
            .and_then(|at| crate::js::parse_date(&at))
            .map(|at| {
                let age_ms = (chrono::Utc::now().timestamp_millis() - at).max(0);
                std::time::Duration::from_millis(u64::try_from(age_ms).unwrap_or(0))
            });
        let last_full = store
            .read(|db| chatgpt_store::get_meta(db, crate::sync::FULL_SYNCED_KEY))
            .ok()
            .flatten()
            .and_then(|at| at.parse().ok());
        Self {
            paths,
            store: Arc::new(store),
            sessions,
            reporter,
            syncer: Syncer::new(synced_age, last_full),
            indexer: Indexer::default(),
            embedder: Embedder::default(),
            auto_jev: AutoJev::default(),
            flight: Default::default(),
            started_at: now_unix(),
            version,
        }
    }

    /// Run `work` on the store's reader connection, off the async threads.
    pub async fn db<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Connection) -> chatgpt_store::Result<T> + Send + 'static,
    ) -> Result<T, Failure> {
        let store = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || store.read(work))
            .await
            .map_err(Failure::join)?
            .map_err(Failure::store)
    }

    /// Run `work` on the store's writer connection, off the async threads.
    pub async fn db_write<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Connection) -> chatgpt_store::Result<T> + Send + 'static,
    ) -> Result<T, Failure> {
        let store = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || store.write(work))
            .await
            .map_err(Failure::join)?
            .map_err(Failure::store)
    }

    /// The classification versions verdicts are read with: this build's.
    pub fn profile(&self) -> &'static Profile {
        Profile::current()
    }
}
