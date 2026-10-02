//! Keeping the index fresh: delta and full sync passes, ported from the TS
//! CLI's `sync` (`src/cli.ts` @ 1b8c950), and when they run.
//!
//! - In the background: a delta every 2 minutes while a client has been
//!   active in the last 10 minutes, otherwise every 15. The archived sweep
//!   and the cache reconcile run at most hourly there.
//! - `chatgpt sync`: a pass now, with the sweep and the reconcile; `--full`
//!   for a full pass. The first pass of an empty index lists everything.
//! - A full pass about once a day, in the background, once nobody has used
//!   the CLI for a while and the last full pass is a day old: the only way
//!   a chat deleted while active leaves the index. It checks at most
//!   [`full::BACKGROUND_OMISSION_CHECKS`] chats one by one, and after a
//!   rate limit it waits another day.
//! - After a successful pass: Jev on new and changed chats, in the
//!   background (`crate::classify::auto_jev`).
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

const SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// The index's meta key for when a full pass last succeeded (Unix seconds).
pub(crate) const FULL_SYNCED_KEY: &str = "full_synced_at";
/// In debug builds: every cadence below in milliseconds instead, so tests
/// can see background passes, a full one included, without waiting.
const SCHEDULE_ENV: &str = "CHATGPT_TEST_SCHEDULE_MS";

/// How often background passes run.
#[derive(Clone, Copy, Debug)]
struct Cadence {
    /// Between passes while a client has been active in `active_window`…
    active_interval: Duration,
    /// …and otherwise.
    idle_interval: Duration,
    active_window: Duration,
    /// Between full passes, and after one hit a rate limit (a night off).
    full_interval: Duration,
    /// After a full pass failed some other way.
    full_retry: Duration,
}

impl Cadence {
    fn new() -> Self {
        match chatgpt_core::debug_env(SCHEDULE_ENV).and_then(|ms| ms.parse().ok()) {
            Some(ms) => {
                let every = Duration::from_millis(ms);
                Self {
                    active_interval: every,
                    idle_interval: every,
                    active_window: every,
                    full_interval: every,
                    full_retry: every,
                }
            }
            None => Self {
                active_interval: Duration::from_secs(2 * 60),
                idle_interval: Duration::from_secs(15 * 60),
                active_window: Duration::from_secs(10 * 60),
                full_interval: Duration::from_secs(24 * 60 * 60),
                full_retry: Duration::from_secs(60 * 60),
            },
        }
    }
}
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
    cadence: Cadence,
    status: SyncStatus,
    /// The browser choice passes read with: the last one `chatgpt sync`
    /// sent. It only changes under `running`, between passes.
    choice: SessionChoice,
    last_client: Instant,
    last_pass: Option<Instant>,
    last_sweep: Option<Instant>,
    backoff: Option<(Instant, Backoff)>,
    /// When a full pass last succeeded, Unix seconds (from the index, so a
    /// restart keeps it).
    last_full: Option<i64>,
    /// The last background full pass that failed, and whether a rate limit
    /// stopped it.
    full_failed: Option<(Instant, bool)>,
}

pub struct PassOptions {
    /// `chatgpt sync`: sweep and reconcile now.
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
    /// `last_full`: when a full pass last succeeded (Unix seconds), if one
    /// ever did.
    pub fn new(synced_age: Option<Duration>, last_full: Option<i64>) -> Self {
        Self {
            running: tokio::sync::Mutex::new(()),
            inner: Mutex::new(Schedule {
                cadence: Cadence::new(),
                status: SyncStatus::default(),
                choice: SessionChoice::default(),
                last_client: Instant::now(),
                // A restart behaves as if the last pass, with its sweep, ran
                // when the index last synced.
                last_pass: synced_age.and_then(|age| Instant::now().checked_sub(age)),
                last_sweep: synced_age.and_then(|age| Instant::now().checked_sub(age)),
                backoff: None,
                last_full,
                full_failed: None,
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
            let was_idle = schedule.last_client.elapsed() > schedule.cadence.active_window;
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
        status.last_full_at = schedule.last_full;
        status.next_full_at = Some(schedule.next_full_at(now_unix()));
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

    /// Hold off sync passes while a change writes ChatGPT and the index,
    /// after waiting out a running one: a pass whose listing predates the
    /// change would otherwise write the chat's old state back (a deleted
    /// chat re-added, an archive undone).
    pub async fn exclusive(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.running.lock().await
    }

    /// [`Syncer::exclusive`] if no pass runs now, without waiting.
    pub fn try_exclusive(&self) -> Option<tokio::sync::MutexGuard<'_, ()>> {
        self.running.try_lock().ok()
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
    fn idle(&self) -> bool {
        self.last_client.elapsed() > self.cadence.active_window
    }

    fn interval(&self) -> Duration {
        if self.idle() {
            self.cadence.idle_interval
        } else {
            self.cadence.active_interval
        }
    }

    /// The earliest a background full pass may run (Unix seconds): a day
    /// after the last one, a day after one a rate limit stopped, an hour
    /// after one that failed otherwise. It waits for an idle moment too.
    fn next_full_at(&self, now: i64) -> i64 {
        let after_last = self.last_full.map_or(now, |at| {
            at.saturating_add(secs(self.cadence.full_interval))
        });
        let after_failure = self.full_failed.map_or(now, |(at, rate_limited)| {
            let wait = if rate_limited {
                self.cadence.full_interval
            } else {
                self.cadence.full_retry
            };
            now.saturating_add(secs(wait.saturating_sub(at.elapsed())))
        });
        after_last.max(after_failure)
    }

    /// Whether the background pass due now should be a full one.
    fn full_due(&self, now: i64) -> bool {
        self.idle() && self.backoff.is_none() && self.next_full_at(now) <= now
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
        state.sessions.prune(&state.syncer.choice());
        let (due, full) = {
            let schedule = state.syncer.schedule();
            (schedule.until_due(), schedule.full_due(now_unix()))
        };
        if due.is_zero() {
            if full {
                tracing::info!("starting the daily full sync");
            }
            if let Err(failure) = run_pass(
                &state,
                PassOptions {
                    explicit: false,
                    full,
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

/// One pass. A background pass that fails is retried at the next interval;
/// one that hit a rate limit waits out the backoff first.
pub async fn run_pass(
    state: &std::sync::Arc<State>,
    options: PassOptions,
) -> Result<SyncReport, Failure> {
    let pass_lock = state.syncer.running.lock().await;
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
    let outcome = async { pass(state, &pinned_api(state, choice).await?, &options).await }.await;
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
                if report.mode == SyncMode::Full {
                    schedule.last_full = Some(finished_at);
                    schedule.full_failed = None;
                }
            }
            Err(error) => {
                schedule.status.last_error = Some(error.message.clone());
                if options.full && !options.explicit {
                    schedule.full_failed = Some((Instant::now(), error.is_rate_limit()));
                }
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

    // New and changed chats get indexed for search.
    state.indexer.pass_succeeded();
    // And new and changed chats get Jev's verdict, once this pass lets go
    // of the pass lock.
    drop(pass_lock);
    crate::classify::auto_jev::after_pass(state);
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
    let bound = match stored {
        Some(stored) => stored,
        // The first request to get here binds the index; one that raced it
        // and lost sees the winner's account.
        None => {
            let ours = account.clone();
            state
                .db_write(move |db| chatgpt_store::bind_account(db, &ours))
                .await?
        }
    };
    if bound == account {
        Ok(api)
    } else {
        Err(ApiError::new(
            ErrorKind::InvalidInput,
            format!(
                "this index holds another ChatGPT account's chats than the session in {}. \
                 Keep each account in its own instance: CHATGPT_INSTANCE=<name> chatgpt sync",
                source()
            ),
        ))
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
        // A background full pass is bounded; one someone asked for isn't.
        _ => full::run(state, api, !options.explicit).await,
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

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 60 * 60;

    fn idle_schedule(last_full: Option<i64>) -> Schedule {
        let syncer = Syncer::new(None, last_full);
        let mut schedule = syncer.inner.into_inner().expect("schedule");
        schedule.last_client = Instant::now()
            .checked_sub(schedule.cadence.active_window + Duration::from_secs(1))
            .expect("an instant that long ago");
        schedule
    }

    #[test]
    fn the_daily_full_sync_waits_for_a_day_and_for_nobody_using_the_cli() {
        let now = now_unix();
        assert!(
            idle_schedule(None).full_due(now),
            "never ran: due once idle"
        );
        assert!(!idle_schedule(Some(now - HOUR)).full_due(now));
        assert!(idle_schedule(Some(now - 25 * HOUR)).full_due(now));

        let mut busy = idle_schedule(None);
        busy.last_client = Instant::now();
        assert!(!busy.full_due(now), "a client is active");

        let mut backing_off = idle_schedule(None);
        backing_off.backoff = Some((
            Instant::now() + Duration::from_secs(60),
            Backoff {
                until: now + 60,
                reason: "rate limited".into(),
            },
        ));
        assert!(!backing_off.full_due(now));
    }

    #[test]
    fn a_rate_limited_full_sync_skips_a_day_and_another_failure_an_hour() {
        let now = now_unix();
        let mut limited = idle_schedule(None);
        limited.full_failed = Some((Instant::now(), true));
        assert!(!limited.full_due(now));
        assert!((now + 24 * HOUR - 5..=now + 24 * HOUR).contains(&limited.next_full_at(now)));

        let mut failed = idle_schedule(None);
        failed.full_failed = Some((Instant::now(), false));
        assert!(!failed.full_due(now));
        assert!((now + HOUR - 5..=now + HOUR).contains(&failed.next_full_at(now)));
    }
}
