//! What every part of the daemon shares.

use std::sync::{Arc, Mutex, PoisonError, RwLock};

use chatgpt_core::Paths;
use chatgpt_core::ts_cli::{self, TsCli};
use chatgpt_protocol::{ImportReport, ImportStatus, TsSyncOutcome, TsSyncStatus};
use chatgpt_store::Store;
use rusqlite::Connection;

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
    pub started_at: i64,
    pub version: String,
    profile: RwLock<Arc<Profile>>,
    ts_sync: Mutex<TsSyncStatus>,
    import: Mutex<ImportStatus>,
}

pub fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl State {
    pub fn new(paths: Paths, store: Store) -> Self {
        let reporter = Reporter::default();
        let sessions = Arc::new(Sessions::new(reporter.clone()));
        let version = std::env::var(VERSION_OVERRIDE_ENV)
            .ok()
            .filter(|version| cfg!(debug_assertions) && !version.is_empty())
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned());
        let located = ts_cli::locate();
        let profile = Profile::for_ts_sources(located.as_ref().ok().and_then(TsCli::source_dir));
        if let Some(problem) = &profile.problem {
            tracing::warn!("{problem}");
        }
        let ts_sync = match &located {
            Ok(ts) => TsSyncStatus {
                cli: Some(ts.entry.display().to_string()),
                ..TsSyncStatus::default()
            },
            Err(error) => TsSyncStatus {
                unavailable: Some(error.to_string()),
                ..TsSyncStatus::default()
            },
        };
        let synced_age = store
            .read(chatgpt_store::synced_at)
            .ok()
            .flatten()
            .and_then(|at| crate::js::parse_date(&at))
            .map(|at| {
                let age_ms = (chrono::Utc::now().timestamp_millis() - at).max(0);
                std::time::Duration::from_millis(u64::try_from(age_ms).unwrap_or(0))
            });
        Self {
            paths,
            store: Arc::new(store),
            sessions,
            reporter,
            syncer: Syncer::new(synced_age),
            indexer: Indexer::default(),
            embedder: Embedder::default(),
            started_at: now_unix(),
            version,
            profile: RwLock::new(Arc::new(profile)),
            ts_sync: Mutex::new(ts_sync),
            import: Mutex::new(ImportStatus::default()),
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

    pub fn profile(&self) -> Arc<Profile> {
        Arc::clone(&self.profile.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// Read the classification versions from the TS CLI again: it may have
    /// been updated since the daemon started.
    pub fn reload_profile(&self) {
        let located = ts_cli::locate();
        let profile = Profile::for_ts_sources(located.as_ref().ok().and_then(TsCli::source_dir));
        let mut current = self.profile.write().unwrap_or_else(PoisonError::into_inner);
        if **current != profile {
            tracing::info!(source = %profile.source, questions = %profile.questions_version, "classification versions changed");
            *current = Arc::new(profile);
        }
    }

    pub fn record_ts_sync(&self, ts: Option<&TsCli>, outcome: &TsSyncOutcome) {
        let mut status = lock(&self.ts_sync);
        status.cli = ts.map(|ts| ts.entry.display().to_string());
        status.unavailable = None;
        status.last_run_at = Some(now_unix());
        status.last_ok = Some(outcome.ok);
        status.last_message = Some(outcome.message.clone());
        if outcome.ok {
            tracing::info!("{}", outcome.message);
        } else {
            tracing::warn!("{}", outcome.message);
        }
    }

    pub fn record_ts_unavailable(&self, why: String) {
        let mut status = lock(&self.ts_sync);
        status.cli = None;
        status.unavailable = Some(why);
    }

    pub fn record_ts_skipped(&self, reason: &str) {
        let message = format!("TS sync skipped: {reason}");
        tracing::info!("{message}");
        let mut status = lock(&self.ts_sync);
        status.last_run_at = Some(now_unix());
        status.last_ok = None;
        status.last_message = Some(message);
    }

    pub fn ts_sync_status(&self) -> TsSyncStatus {
        lock(&self.ts_sync).clone()
    }

    pub fn record_import(&self, result: Result<ImportReport, String>) {
        let mut status = lock(&self.import);
        status.last_at = Some(now_unix());
        match result {
            Ok(report) => {
                status.last = Some(report);
                status.last_error = None;
            }
            Err(error) => status.last_error = Some(error),
        }
    }

    pub fn import_status(&self) -> ImportStatus {
        lock(&self.import).clone()
    }
}
