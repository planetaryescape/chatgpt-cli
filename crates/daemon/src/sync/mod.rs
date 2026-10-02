//! Keeping the index fresh: delta and full sync passes, ported from the TS
//! CLI's `sync` (`src/cli.ts` @ 1b8c950), and when they run.
//!
//! - In the background: a delta every 2 minutes while a client has been
//!   active in the last 10 minutes, otherwise every 15. The archived sweep
//!   and the cache reconcile run at most hourly there.
//! - `chatgpt sync`: a pass now, with the sweep and the reconcile; `--full`
//!   for a full pass. The first pass of an empty index lists everything.
//! - After a pass, while the bridge exists: the TS CLI's own sync (at most
//!   every 15 minutes, always for `chatgpt sync`), then an import of its
//!   judgments and titles.
//! - A rate limit backs off for as long as ChatGPT asked (clamped), and
//!   `daemon status` shows it.

mod delta;
mod full;
pub(crate) mod reconcile;

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use chatgpt_core::ErrorKind;
use chatgpt_protocol::{Backoff, Progress, SessionChoice, SyncMode, SyncReport, SyncStatus};
use tokio::sync::Notify;
use tokio::sync::mpsc::UnboundedSender;

use crate::api::{Api, ApiError};
use crate::handlers::Failure;
use crate::state::{State, now_unix};

const ACTIVE_INTERVAL: Duration = Duration::from_secs(2 * 60);
const IDLE_INTERVAL: Duration = Duration::from_secs(15 * 60);
const ACTIVE_WINDOW: Duration = Duration::from_secs(10 * 60);
const SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);
pub const TS_SYNC_INTERVAL: Duration = Duration::from_secs(15 * 60);
/// A rate limit is waited out for at least this long, even when ChatGPT
/// gave no `retry-after` (its 429s clear after about a minute)…
const MIN_BACKOFF: Duration = Duration::from_secs(60);
/// …and at most this long, whatever it asked.
const MAX_BACKOFF: Duration = Duration::from_secs(60 * 60);
/// An explicit `chatgpt sync` waits out a backoff this short rather than
/// failing: the skill's "the CLI retries by itself for up to ~75s".
const EXPLICIT_WAIT: Duration = Duration::from_secs(75);
/// How often the scheduler checks whether a pass is due.
const TICK: Duration = Duration::from_secs(15);

/// When passes run, and how the last one went.
pub struct Syncer {
    /// One pass at a time; an explicit sync waits for a background one.
    running: tokio::sync::Mutex<()>,
    inner: Mutex<Schedule>,
    wake: Notify,
}

struct Schedule {
    status: SyncStatus,
    /// The browser choice passes read with: the last one `chatgpt sync`
    /// sent. It only changes under `running`, between passes.
    choice: SessionChoice,
    last_client: Instant,
    last_pass: Option<Instant>,
    last_sweep: Option<Instant>,
    last_ts_sync: Option<Instant>,
    backoff: Option<(Instant, Backoff)>,
}

pub struct PassOptions {
    /// `chatgpt sync`: sweep, reconcile and run the TS sync now.
    pub explicit: bool,
    pub full: bool,
    /// The browser and profile `chatgpt sync` asked for; it becomes the one
    /// background passes use too. `None`: keep the current one.
    pub choice: Option<SessionChoice>,
    /// Where the waiting client wants progress lines; a background pass
    /// has no client.
    pub progress: Option<UnboundedSender<Progress>>,
}

impl Syncer {
    /// `synced_age`: how long ago the index last synced, if it ever did. A
    /// daemon that starts on an index synced within the interval waits out
    /// the rest of it, so a cold `list` sends no request. An older index, or
    /// an empty one, gets a background pass at once (`list` still answers
    /// from the index without waiting for it).
    pub fn new(synced_age: Option<Duration>) -> Self {
        Self {
            running: tokio::sync::Mutex::new(()),
            inner: Mutex::new(Schedule {
                status: SyncStatus::default(),
                choice: SessionChoice::default(),
                last_client: Instant::now(),
                // A restart behaves as if the last pass, with its sweep and
                // TS sync, ran when the index last synced.
                last_pass: synced_age.and_then(|age| Instant::now().checked_sub(age)),
                last_sweep: synced_age.and_then(|age| Instant::now().checked_sub(age)),
                last_ts_sync: synced_age.and_then(|age| Instant::now().checked_sub(age)),
                backoff: None,
            }),
            wake: Notify::new(),
        }
    }

    fn schedule(&self) -> std::sync::MutexGuard<'_, Schedule> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A client asked something: background passes speed up for a while.
    pub fn client_active(&self) {
        let was_idle = {
            let mut schedule = self.schedule();
            let was_idle = schedule.last_client.elapsed() > ACTIVE_WINDOW;
            schedule.last_client = Instant::now();
            was_idle
        };
        if was_idle {
            // A pass that waited on the idle interval may be due now.
            self.wake.notify_one();
        }
    }

    pub fn status(&self, synced_at: Option<String>) -> (SyncStatus, Option<Backoff>) {
        let schedule = self.schedule();
        let mut status = schedule.status.clone();
        status.synced_at = synced_at;
        status.next_at = Some(now_unix() + secs(schedule.until_due()));
        let backoff = schedule
            .backoff
            .as_ref()
            .filter(|(until, _)| *until > Instant::now())
            .map(|(_, backoff)| backoff.clone());
        (status, backoff)
    }

    /// The browser choice passes read with.
    pub fn choice(&self) -> SessionChoice {
        self.schedule().choice.clone()
    }

    pub fn ts_sync_due(&self, explicit: bool) -> bool {
        explicit
            || self
                .schedule()
                .last_ts_sync
                .is_none_or(|at| at.elapsed() >= TS_SYNC_INTERVAL)
    }

    pub fn ts_sync_ran(&self) {
        self.schedule().last_ts_sync = Some(Instant::now());
    }

    /// Hold off sync passes while a change writes ChatGPT and the index,
    /// after waiting out a running one: a pass whose listing predates the
    /// change would otherwise write the chat's old state back (a deleted
    /// chat re-added, an archive undone).
    pub async fn exclusive(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.running.lock().await
    }

    /// Whether a pass is running now.
    pub fn is_running(&self) -> bool {
        self.running.try_lock().is_err()
    }

    /// How much of a rate-limit backoff is left, if one holds.
    pub fn backoff_left(&self) -> Option<Duration> {
        self.schedule()
            .backoff
            .as_ref()
            .and_then(|(until, _)| remaining(*until))
    }
}

impl Schedule {
    fn interval(&self) -> Duration {
        if self.last_client.elapsed() <= ACTIVE_WINDOW {
            ACTIVE_INTERVAL
        } else {
            IDLE_INTERVAL
        }
    }

    /// How long until the next background pass may run.
    fn until_due(&self) -> Duration {
        let after_pass = self.last_pass.map_or(Duration::ZERO, |at| {
            self.interval().saturating_sub(at.elapsed())
        });
        let after_backoff = self.backoff.as_ref().map_or(Duration::ZERO, |(until, _)| {
            until.saturating_duration_since(Instant::now())
        });
        after_pass.max(after_backoff)
    }
}

/// How long to back off after `error`'s rate limit: what ChatGPT asked,
/// clamped to [`MIN_BACKOFF`, `MAX_BACKOFF`].
pub(crate) fn backoff_for(error: &ApiError) -> Duration {
    error
        .retry_after
        .unwrap_or(MIN_BACKOFF)
        .clamp(MIN_BACKOFF, MAX_BACKOFF)
}

/// Time left until `until`, `None` once it has passed.
pub(crate) fn remaining(until: Instant) -> Option<Duration> {
    Some(until.saturating_duration_since(Instant::now())).filter(|left| !left.is_zero())
}

fn secs(duration: Duration) -> i64 {
    i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
}

/// The background loop: a pass whenever one is due. Runs for the daemon's
/// life.
pub async fn run_scheduled(state: std::sync::Arc<State>) {
    loop {
        let due = state.syncer.schedule().until_due();
        if due.is_zero() {
            if let Err(failure) = run_pass(
                &state,
                PassOptions {
                    explicit: false,
                    full: false,
                    choice: None,
                    progress: None,
                },
            )
            .await
            {
                tracing::warn!(
                    kind = failure.kind.as_str(),
                    "background sync failed: {}",
                    failure.message
                );
            }
            continue;
        }
        tokio::select! {
            () = tokio::time::sleep(due.min(TICK)) => {}
            () = state.syncer.wake.notified() => {}
        }
    }
}

/// One pass, then the TS sync and import when due. A background pass that
/// fails is retried at the next interval; one that hit a rate limit waits
/// out the backoff first.
pub async fn run_pass(state: &State, options: PassOptions) -> Result<SyncReport, Failure> {
    let _running = state.syncer.running.lock().await;
    let _attached = options
        .progress
        .clone()
        .map(|sender| state.reporter.attach(sender));
    if let Some(left) = state.syncer.backoff_left() {
        if !options.explicit || left > EXPLICIT_WAIT {
            return Err(Failure::new(
                ErrorKind::RateLimited,
                format!(
                    "rate limited by ChatGPT; waiting {}s before syncing again",
                    left.as_secs()
                ),
            ));
        }
        state.reporter.note(format!(
            "rate limited by ChatGPT; waiting {}s",
            left.as_secs_f64().round()
        ));
        tokio::time::sleep(left).await;
    }

    // One browser session, and one account, for the whole pass: a client
    // choosing another browser meanwhile can't change what this pass reads.
    let choice = {
        let mut schedule = state.syncer.schedule();
        if let Some(choice) = &options.choice {
            schedule.choice = choice.clone();
        }
        schedule.choice.clone()
    };
    let started = Instant::now();
    state.syncer.schedule().status.in_progress = true;
    let outcome =
        async { pass(state, &pinned_api(state, choice.clone()).await?, &options).await }.await;
    let finished_at = now_unix();
    {
        let mut schedule = state.syncer.schedule();
        schedule.status.in_progress = false;
        schedule.status.last_finished_at = Some(finished_at);
        schedule.last_pass = Some(Instant::now());
        match &outcome {
            Ok(report) => {
                schedule.status.last_error = None;
                schedule.status.last_summary = Some(summary(report));
                schedule.backoff = None;
                if report.swept {
                    schedule.last_sweep = Some(Instant::now());
                    schedule.status.last_sweep_at = Some(finished_at);
                }
            }
            Err(error) => {
                schedule.status.last_error = Some(error.message.clone());
                if error.is_rate_limit() {
                    let wait = backoff_for(error);
                    schedule.backoff = Some((
                        Instant::now() + wait,
                        Backoff {
                            until: finished_at + secs(wait),
                            reason: error.message.clone(),
                        },
                    ));
                }
            }
        }
    }
    let mut report = outcome?;
    report.elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    tracing::info!("sync pass done: {}", summary(&report));

    if state.syncer.ts_sync_due(options.explicit) {
        crate::ts_sync::after_pass(state, &choice, options.full, &mut report).await;
    }
    // New and changed chats, and transcripts the import brought, get
    // indexed for search.
    state.indexer.pass_succeeded();
    Ok(report)
}

/// An API client for `choice` pinned to the session's account, refusing an
/// account other than the one the index was built from: reconciling one
/// account's chats against another's would delete them.
pub(crate) async fn pinned_api(state: &State, choice: SessionChoice) -> Result<Api, ApiError> {
    let api = Api::new(std::sync::Arc::clone(&state.sessions), choice)
        .pinned()
        .await?;
    let stored = state.db(chatgpt_store::account).await?;
    let source = || {
        state
            .sessions
            .source(api.choice())
            .unwrap_or_else(|| "this browser".into())
    };
    let Some(account) = api.account().map(str::to_owned) else {
        // A session that doesn't say whose it is can't be checked against
        // an index that already belongs to an account.
        return match stored {
            Some(_) => Err(ApiError::new(
                ErrorKind::InvalidInput,
                format!(
                    "the session in {} doesn't say which ChatGPT account it is, and this index \
                     holds one account's chats. Keep each account in its own instance: \
                     CHATGPT_INSTANCE=<name> chatgpt sync",
                    source()
                ),
            )),
            None => Ok(api),
        };
    };
    match stored {
        Some(stored) if stored != account => Err(ApiError::new(
            ErrorKind::InvalidInput,
            format!(
                "this index holds another ChatGPT account's chats than the session in {}. \
                 Keep each account in its own instance: CHATGPT_INSTANCE=<name> chatgpt sync",
                source()
            ),
        )),
        Some(_) => Ok(api),
        None => {
            state
                .db_write(move |db| chatgpt_store::set_account(db, &account))
                .await?;
            Ok(api)
        }
    }
}

async fn pass(state: &State, api: &Api, options: &PassOptions) -> Result<SyncReport, ApiError> {
    let watermark = state.db(chatgpt_store::active_watermark).await?;
    match watermark {
        Some(watermark) if !options.full => {
            let sweep = options.explicit
                || state
                    .syncer
                    .schedule()
                    .last_sweep
                    .is_none_or(|at| at.elapsed() >= SWEEP_INTERVAL);
            delta::run(state, api, &watermark, sweep).await
        }
        _ => full::run(state, api).await,
    }
}

/// One line for `daemon status` and the log.
fn summary(report: &SyncReport) -> String {
    match report.mode {
        SyncMode::Full => format!(
            "full: {} chats ({} active, {} archived)",
            report.total, report.active, report.archived
        ),
        _ => format!(
            "delta: {} new, {} updated{}",
            report.added,
            report.updated,
            if report.swept {
                format!(
                    ", {} newly archived, {} unarchived, {} deleted",
                    report.newly_archived, report.unarchived, report.deleted
                )
            } else {
                String::new()
            }
        ),
    }
}

/// Chats from the conversation lists by id, in first-seen order: a JS
/// `Map`, as the TS CLI dedupes them (`insert` overwrites in place).
pub(crate) type Listing = indexmap::IndexMap<String, crate::api::ConversationSummary>;
