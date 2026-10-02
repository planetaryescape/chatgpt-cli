//! The background search indexer: the TS CLI's `search-index` without
//! embeddings (`SearchIndexer` in `src/search/indexer.ts` @ 1b8c950), run
//! by the daemon so `search` never needs a manual step.
//!
//! Each run drops chunks of chats gone from the index, chunks every chat
//! whose cached transcript is current but whose chunks aren't, then fetches
//! the missing transcripts through the batch endpoint, 10 at a time with
//! 500 ms between batches, as `search-index` does. Every chat is written in
//! its own short transaction, so `list` and `search` (on the reader
//! connection) never wait, a sync's writes wait at most one chat, and a run
//! that stops halfway resumes where it stopped: the next run only sees what
//! is still missing.
//!
//! Runs start when the daemon starts (chunking only), after every sync pass
//! and after an import. Fetching starts only after this daemon's first
//! successful pass, so a cold start reads no cookies and sends nothing. It
//! steps aside while a `sync` or `export` is running, honours rate limits
//! with its own backoff, and logs counts and ids, never transcript text.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use chatgpt_core::ErrorKind;
use chatgpt_protocol::SearchIndexStatus;
use chatgpt_store::{ChunkVersions, Unindexed};
use tokio::sync::Notify;

use super::chunks::{bun_sqlite_text, transcript_chunks};
use crate::api::{ApiError, BATCH_MAX, BatchItem};
use crate::render::cached_transcript;
use crate::state::{State, now_unix};

const BATCH_GAP: Duration = Duration::from_millis(500);
/// Cached transcripts chunked per write, so the writer is never held long.
const LOCAL_GROUP: usize = 20;
/// How long a chat ChatGPT didn't return waits before it's asked for again.
const RETRY_UNAVAILABLE: Duration = Duration::from_secs(60 * 60);
/// A rate limit is waited out for at least a minute and at most an hour,
/// as the sync's backoff is.
const MIN_BACKOFF: Duration = Duration::from_secs(60);
const MAX_BACKOFF: Duration = Duration::from_secs(60 * 60);
/// How often a paused run checks whether the request it yields to is done.
const YIELD_POLL: Duration = Duration::from_millis(100);

pub struct Indexer {
    wake: Notify,
    inner: Mutex<Inner>,
    /// User requests that read ChatGPT (`sync`, `export`) in flight.
    foreground: AtomicUsize,
}

#[derive(Default)]
struct Inner {
    in_progress: bool,
    fetched: u64,
    failed: u64,
    last_finished_at: Option<i64>,
    last_error: Option<String>,
    waiting: Option<String>,
    may_fetch: bool,
    backoff_until: Option<Instant>,
    /// Chats a fetch didn't return: their `update_time` then, and when.
    unavailable: HashMap<String, (String, Instant)>,
}

/// Marks a user request that reads ChatGPT while it runs; the indexer
/// waits for it between batches.
pub struct Foreground<'a>(&'a AtomicUsize);

impl Drop for Foreground<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Default for Indexer {
    fn default() -> Self {
        Self {
            wake: Notify::new(),
            inner: Mutex::new(Inner::default()),
            foreground: AtomicUsize::new(0),
        }
    }
}

impl Indexer {
    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Start a run (or another one after the current run).
    pub fn wake(&self) {
        self.wake.notify_one();
    }

    /// A sync pass succeeded: the session is open, so runs may fetch.
    pub fn pass_succeeded(&self) {
        self.inner().may_fetch = true;
        self.wake();
    }

    pub fn foreground(&self) -> Foreground<'_> {
        self.foreground.fetch_add(1, Ordering::SeqCst);
        Foreground(&self.foreground)
    }

    /// `chats` and `indexed` come from the index; the rest is this run's.
    pub fn status(&self, chats: u64, indexed: u64) -> SearchIndexStatus {
        let inner = self.inner();
        SearchIndexStatus {
            chats,
            indexed,
            in_progress: inner.in_progress,
            fetched: inner.fetched,
            failed: inner.failed,
            last_finished_at: inner.last_finished_at,
            last_error: inner.last_error.clone(),
            waiting: inner.waiting.clone(),
        }
    }

    fn backoff_left(&self) -> Option<Duration> {
        self.inner()
            .backoff_until
            .map(|until| until.saturating_duration_since(Instant::now()))
            .filter(|left| !left.is_zero())
    }
}

/// The indexer's loop, for the daemon's life.
pub async fn run(state: Arc<State>) {
    loop {
        match state.indexer.backoff_left() {
            // Resume once a rate limit has passed, even without a pass.
            Some(left) => {
                tokio::select! {
                    () = tokio::time::sleep(left) => {}
                    () = state.indexer.wake.notified() => {}
                }
            }
            None => state.indexer.wake.notified().await,
        }
        state.indexer.inner().in_progress = true;
        let outcome = index(&state).await;
        let mut inner = state.indexer.inner();
        inner.in_progress = false;
        inner.last_finished_at = Some(now_unix());
        match outcome {
            Err(message) => {
                tracing::warn!("search indexing stopped: {message}");
                inner.last_error = Some(message);
            }
            // Nothing waiting or set aside: an earlier error no longer applies.
            Ok(()) if inner.waiting.is_none() && inner.failed == 0 => inner.last_error = None,
            Ok(()) => {}
        }
    }
}

async fn index(state: &State) -> Result<(), String> {
    let versions = super::versions(&state.profile());
    let pruned = state
        .db_write(chatgpt_store::prune_search)
        .await
        .map_err(|failure| failure.message)?;
    // Read on the writer connection, so `list` and `search` keep the reader.
    let targets = state
        .db_write(move |db| chatgpt_store::unindexed(db, versions))
        .await
        .map_err(|failure| failure.message)?;
    let (cached, missing): (Vec<Unindexed>, Vec<Unindexed>) =
        targets.into_iter().partition(|chat| chat.cached);
    for group in cached.chunks(LOCAL_GROUP) {
        let group = group.to_vec();
        state
            .db_write(move |db| {
                for chat in &group {
                    let Some(transcript) = chatgpt_store::transcript(
                        db,
                        &chat.id,
                        &chat.update_time,
                        versions.render,
                    )?
                    else {
                        continue;
                    };
                    let bodies = chunk_bytes(&transcript.markdown);
                    chatgpt_store::replace_chunks(db, chat, versions, &bodies)?;
                }
                Ok(())
            })
            .await
            .map_err(|failure| failure.message)?;
    }
    let missing = retryable(state, missing);
    tracing::info!(
        pruned,
        chunked = cached.len(),
        to_fetch = missing.len(),
        "search indexing"
    );
    if missing.is_empty() {
        state.indexer.inner().waiting = None;
        return Ok(());
    }
    if let Some(why) = cannot_fetch(state) {
        state.indexer.inner().waiting = Some(why);
        return Ok(());
    }
    state.indexer.inner().waiting = None;
    let api = crate::sync::pinned_api(state, state.syncer.choice())
        .await
        .map_err(|error| rate_limited(state, error))?;
    let mut fetched = 0;
    for (number, batch) in missing.chunks(BATCH_MAX).enumerate() {
        if number > 0 {
            tokio::time::sleep(BATCH_GAP).await;
        }
        yield_to_requests(state).await;
        if let Some(why) = cannot_fetch(state) {
            state.indexer.inner().waiting = Some(why);
            break;
        }
        let ids: Vec<String> = batch.iter().map(|chat| chat.id.clone()).collect();
        let items = match api.batch(&ids).await {
            Ok(items) => items,
            Err(error) if error.is_rate_limit() => return Err(rate_limited(state, error)),
            // A timeout or a dropped connection says nothing about these
            // chats: stop, and the run after the next pass tries again.
            Err(error) if error.kind == ErrorKind::Network => return Err(error.message),
            // ChatGPT answered with an error for this batch: set its chats
            // aside for a while, so they can't hold up the rest.
            Err(error) => {
                tracing::warn!(
                    chats = ids.len(),
                    "search transcripts not fetched: {}",
                    error.message
                );
                unavailable(state, batch);
                state.indexer.inner().last_error = Some(error.message);
                continue;
            }
        };
        let saved = save_batch(state, batch.to_vec(), items, versions).await?;
        fetched += saved;
        state.indexer.inner().fetched += saved;
    }
    tracing::info!(fetched, "search indexing done");
    Ok(())
}

/// Cache and chunk what a batch returned; chats it left out wait an hour.
async fn save_batch(
    state: &State,
    batch: Vec<Unindexed>,
    items: Vec<BatchItem>,
    versions: ChunkVersions,
) -> Result<u64, String> {
    let mut items: HashMap<String, BatchItem> = items
        .into_iter()
        .map(|item| (item.id.clone(), item))
        .collect();
    let mut ready = Vec::new();
    let mut failed = Vec::new();
    for chat in batch {
        match items
            .remove(&chat.id)
            .map(|item| cached_transcript(&item, &chat.update_time, versions.render))
        {
            Some(Ok(transcript)) => ready.push((chat, transcript)),
            Some(Err(why)) => {
                tracing::warn!(id = %chat.id, "search transcript not rendered: {why}");
                failed.push(chat);
            }
            None => {
                tracing::info!(id = %chat.id, "ChatGPT did not return the chat for the search index");
                failed.push(chat);
            }
        }
    }
    unavailable(state, &failed);
    let saved = u64::try_from(ready.len()).unwrap_or(u64::MAX);
    state
        .db_write(move |db| {
            for (chat, transcript) in &ready {
                let bodies = chunk_bytes(&transcript.markdown);
                chatgpt_store::save_indexed(db, transcript, chat, versions, &bodies)?;
            }
            Ok(())
        })
        .await
        .map_err(|failure| failure.message)?;
    Ok(saved)
}

fn chunk_bytes(markdown: &str) -> Vec<Vec<u8>> {
    transcript_chunks(markdown)
        .iter()
        .map(|chunk| bun_sqlite_text(chunk))
        .collect()
}

/// `missing` without chats ChatGPT recently didn't return (unless they
/// changed since).
fn retryable(state: &State, missing: Vec<Unindexed>) -> Vec<Unindexed> {
    let mut inner = state.indexer.inner();
    inner
        .unavailable
        .retain(|_, (_, at)| at.elapsed() < RETRY_UNAVAILABLE);
    let ready: Vec<Unindexed> = missing
        .into_iter()
        .filter(|chat| {
            inner
                .unavailable
                .get(&chat.id)
                .is_none_or(|(update_time, _)| *update_time != chat.update_time)
        })
        .collect();
    inner.failed = u64::try_from(inner.unavailable.len()).unwrap_or(u64::MAX);
    ready
}

fn unavailable(state: &State, chats: &[Unindexed]) {
    let mut inner = state.indexer.inner();
    for chat in chats {
        inner
            .unavailable
            .insert(chat.id.clone(), (chat.update_time.clone(), Instant::now()));
    }
    inner.failed = u64::try_from(inner.unavailable.len()).unwrap_or(u64::MAX);
}

/// Why this run can't fetch now, if it can't.
fn cannot_fetch(state: &State) -> Option<String> {
    if !state.indexer.inner().may_fetch {
        return Some("transcripts are fetched after the next sync".to_owned());
    }
    if let Some(left) = state.indexer.backoff_left() {
        return Some(format!(
            "rate limited; fetching again in {}s",
            left.as_secs()
        ));
    }
    if let Some(left) = state.syncer.backoff_left() {
        return Some(format!(
            "sync is rate limited; fetching again after {}s",
            left.as_secs()
        ));
    }
    None
}

/// Back off from fetching for as long as ChatGPT asked (clamped).
fn rate_limited(state: &State, error: ApiError) -> String {
    if error.is_rate_limit() {
        let wait = error
            .retry_after
            .unwrap_or(MIN_BACKOFF)
            .clamp(MIN_BACKOFF, MAX_BACKOFF);
        let mut inner = state.indexer.inner();
        inner.backoff_until = Some(Instant::now() + wait);
        inner.waiting = Some(format!(
            "rate limited; fetching again in {}s",
            wait.as_secs()
        ));
        inner.last_error = Some(error.message.clone());
    }
    error.message
}

/// Wait while a `sync` or `export` reads ChatGPT, so neither queues behind
/// a long indexing run.
async fn yield_to_requests(state: &State) {
    while state.indexer.foreground.load(Ordering::SeqCst) > 0 || state.syncer.is_running() {
        tokio::time::sleep(YIELD_POLL).await;
    }
}
