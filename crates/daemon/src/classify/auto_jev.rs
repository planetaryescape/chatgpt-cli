//! Jev in the background (D4): after each successful sync pass, the daemon
//! judges chats that are new or changed since the background Jev was first
//! enabled and have no current judgment, as a bare `chatgpt classify`
//! would pick them (active, unpinned), newest first and at most
//! [`PER_PASS`] a pass, with `classify`'s pacing.
//!
//! It runs only Jev's first pass: never its follow-up, Luna or a summary.
//! A long chat without a cached summary waits for a `classify` someone
//! runs. It needs a Jev key in the user config (a key a client passes on
//! doesn't count: no client asked for this), and `"auto_jev": false` there
//! switches it off. What it judged and spent today shows in `chatgpt
//! daemon status`; each save takes the sync pass lock, as `classify`'s do.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use chatgpt_core::user_config::Provider;
use chatgpt_protocol::AutoJevStatus;

use super::access::Access;
use super::pipeline::{Classifier, FULL_TRANSCRIPT_MAX_TOKENS, Options, profile, utc_today};
use crate::progress::Reporter;
use crate::state::{State, now_unix};

/// At most this many chats a pass.
const PER_PASS: usize = 50;
/// A chat Jev failed on waits this long before it's tried again.
const RETRY_AFTER: Duration = Duration::from_secs(6 * 60 * 60);
/// In debug builds: another per-pass bound, and the time after which chats
/// count as new (an ISO date), for tests and demos.
const LIMIT_ENV: &str = "CHATGPT_TEST_AUTO_JEV_LIMIT";
const SINCE_ENV: &str = "CHATGPT_TEST_AUTO_JEV_SINCE";
/// The `meta` keys: when it was first enabled, and today's tally.
const SINCE_KEY: &str = "auto_jev_since";
const DAY_KEY: &str = "auto_jev_day";
const JUDGED_KEY: &str = "auto_jev_judged";
const USD_KEY: &str = "auto_jev_usd";

#[derive(Default)]
pub struct AutoJev {
    running: AtomicBool,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    status: AutoJevStatus,
    /// Chats (id, update_time) Jev failed on, and when.
    failed: HashMap<(String, String), Instant>,
    /// Today's tally was read from the index.
    loaded: bool,
}

fn per_pass() -> usize {
    chatgpt_core::debug_env(LIMIT_ENV)
        .and_then(|limit| limit.parse().ok())
        .unwrap_or(PER_PASS)
}

impl AutoJev {
    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// For `daemon status`: whether it's on (the config is read now), and
    /// today's tally.
    pub fn status(&self) -> AutoJevStatus {
        let mut status = self.inner().status.clone();
        match switch() {
            Ok(()) => {
                status.enabled = true;
                status.off_reason = None;
            }
            Err(why) => {
                status.enabled = false;
                status.off_reason = Some(why);
            }
        }
        status.in_progress = self.running.load(Ordering::SeqCst);
        roll_day(&mut status);
        status
    }
}

/// A new UTC day starts a new tally.
fn roll_day(status: &mut AutoJevStatus) {
    let today = utc_today();
    if status.day.as_deref() != Some(today.as_str()) {
        status.day = Some(today);
        status.judged_today = 0;
        status.cost_today_usd = 0.0;
    }
}

/// Whether the background Jev may run, or why not.
fn switch() -> Result<(), String> {
    let config = Access::config_only().config()?;
    if config.auto_jev == Some(false) {
        return Err("auto_jev is false in the user config".to_owned());
    }
    if config.key(Provider::Jev).is_none() {
        return Err("no Jev key in the user config (`chatgpt configure jev`)".to_owned());
    }
    Ok(())
}

/// After a successful pass: start a run in the background unless one is
/// running or it's off.
pub fn after_pass(state: &Arc<State>) {
    if switch().is_err() {
        return;
    }
    if state.auto_jev.running.swap(true, Ordering::SeqCst) {
        return;
    }
    let state = Arc::clone(state);
    tokio::spawn(async move {
        let outcome = run(&state).await;
        let mut inner = state.auto_jev.inner();
        inner.status.last_run_at = Some(now_unix());
        match outcome {
            Ok(Some(summary)) => {
                tracing::info!("background Jev: {summary}");
                inner.status.last_summary = Some(summary);
                inner.status.last_error = None;
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!("background Jev failed: {error}");
                inner.status.last_error = Some(error);
            }
        }
        drop(inner);
        state.auto_jev.running.store(false, Ordering::SeqCst);
    });
}

/// One run: what it did, or `None` when there was nothing to judge.
async fn run(state: &Arc<State>) -> Result<Option<String>, String> {
    load_tally(state).await?;
    let since = baseline(state).await?;
    let now = Instant::now();
    let failed_before = {
        let mut inner = state.auto_jev.inner();
        inner
            .failed
            .retain(|_, at| now.duration_since(*at) < RETRY_AFTER);
        inner.failed.clone()
    };
    let version = profile().questions_version.clone();
    let render = state.profile().render_version;
    // Enough to fill a pass after leaving out the ones that failed lately.
    let limit = per_pass() + failed_before.len();
    let candidates = state
        .db(move |db| {
            chatgpt_store::unjudged(
                db,
                chatgpt_store::Unjudged {
                    after: &since,
                    questions_version: &version,
                    render_version: render,
                    max_tokens: FULL_TRANSCRIPT_MAX_TOKENS,
                    summary_version: super::summarise::SUMMARY_PROMPT_VERSION,
                    limit,
                },
            )
        })
        .await
        .map_err(|failure| failure.message)?;
    let chosen: Vec<String> = candidates
        .into_iter()
        .filter(|key| !failed_before.contains_key(key))
        .take(per_pass())
        .map(|(id, _)| id)
        .collect();
    if chosen.is_empty() {
        return Ok(None);
    }
    // Chats a command is judging now are its.
    let (_claim, chosen) = state.flight.claim_free(chosen);
    if chosen.is_empty() {
        return Ok(None);
    }
    let chats = super::chats(state, chosen)
        .await
        .map_err(|failure| failure.message)?;
    let reporter = Reporter::for_client(None);
    let access = Access::config_only();
    let classifier = Classifier {
        state,
        reporter: &reporter,
        session: state.syncer.choice(),
        access: &access,
        asker: None,
    };
    let options = Options {
        force: false,
        yes: false,
        summarise: false,
    };
    let classified = classifier
        .classify(&chats, options)
        .await
        .map_err(|failure| failure.message)?;
    let judged = chats
        .iter()
        .filter(|chat| classified.judgments.contains_key(&chat.id))
        .count();
    let failed: Vec<(String, String)> = chats
        .iter()
        .filter(|chat| {
            !classified.judgments.contains_key(&chat.id) && !classified.held_back.contains(&chat.id)
        })
        .map(|chat| (chat.id.clone(), chat.update_time.clone()))
        .collect();
    {
        let mut inner = state.auto_jev.inner();
        inner
            .failed
            .extend(failed.iter().cloned().map(|key| (key, now)));
        roll_day(&mut inner.status);
        inner.status.judged_today += u64::try_from(judged).unwrap_or(0);
        inner.status.cost_today_usd += classified.cost;
    }
    save_tally(state).await?;
    let mut summary = format!(
        "judged {judged} of {} new or changed chat(s), {}",
        chats.len(),
        super::costs::format_usd(classified.cost)
    );
    if !failed.is_empty() {
        summary.push_str(&format!(", {} failed", failed.len()));
    }
    if !classified.held_back.is_empty() {
        summary.push_str(&format!(
            ", {} long chat(s) wait for a summary",
            classified.held_back.len()
        ));
    }
    Ok(Some(summary))
}

/// When chats count as new from: set to the index's newest chat the first
/// time it runs, so enabling it never judges the whole history.
async fn baseline(state: &State) -> Result<String, String> {
    if let Some(since) = chatgpt_core::debug_env(SINCE_ENV) {
        return Ok(since);
    }
    state
        .db_write(|db| {
            if let Some(since) = chatgpt_store::get_meta(db, SINCE_KEY)? {
                return Ok(since);
            }
            // Only chats updated after it count: the newest one already
            // existed when it was enabled.
            let since = chatgpt_store::active_watermark(db)?.unwrap_or_default();
            chatgpt_store::set_meta(db, SINCE_KEY, &since)?;
            Ok(since)
        })
        .await
        .map_err(|failure| failure.message)
}

/// Today's tally from the index, once, so a restart keeps it.
async fn load_tally(state: &State) -> Result<(), String> {
    if state.auto_jev.inner().loaded {
        return Ok(());
    }
    let stored = state
        .db(|db| {
            Ok((
                chatgpt_store::get_meta(db, DAY_KEY)?,
                chatgpt_store::get_meta(db, JUDGED_KEY)?,
                chatgpt_store::get_meta(db, USD_KEY)?,
            ))
        })
        .await
        .map_err(|failure| failure.message)?;
    let mut inner = state.auto_jev.inner();
    inner.loaded = true;
    if let (Some(day), Some(judged), Some(usd)) = stored
        && day == utc_today()
    {
        inner.status.day = Some(day);
        inner.status.judged_today = judged.parse().unwrap_or(0);
        inner.status.cost_today_usd = usd.parse().unwrap_or(0.0);
    }
    Ok(())
}

/// Save today's tally.
async fn save_tally(state: &State) -> Result<(), String> {
    let (day, judged, usd) = {
        let inner = state.auto_jev.inner();
        (
            inner.status.day.clone().unwrap_or_default(),
            inner.status.judged_today,
            inner.status.cost_today_usd,
        )
    };
    state
        .db_write(move |db| {
            chatgpt_store::set_meta(db, DAY_KEY, &day)?;
            chatgpt_store::set_meta(db, JUDGED_KEY, &judged.to_string())?;
            chatgpt_store::set_meta(db, USD_KEY, &usd.to_string())
        })
        .await
        .map_err(|failure| failure.message)
}
