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
use chatgpt_store::{Candidate, ChunkVersions, Unindexed};
use tokio::sync::Notify;

use super::chunks::{bun_sqlite_text, transcript_chunks};
use crate::api::{ApiError, BATCH_MAX, BatchItem};
use crate::render::cached_transcript;
use crate::state::{State, now_unix};
use crate::sync::{backoff_for, reconcile, remaining};

const BATCH_GAP: Duration = Duration::from_millis(500);
/// Cached transcripts chunked per write, so the writer is never held long.
const LOCAL_GROUP: usize = 20;
/// How long a chat ChatGPT didn't return waits before it's asked for again.
const RETRY_UNAVAILABLE: Duration = Duration::from_secs(60 * 60);
/// How often a paused run checks whether the request it yields to is done.
const YIELD_POLL: Duration = Duration::from_millis(100);

#[derive(Default)]
pub struct Indexer {
    wake: Notify,
    inner: Mutex<Inner>,
    /// Exports in flight (a running sync shows in `Syncer::is_running`).
    foreground: AtomicUsize,
}

#[derive(Default)]
struct Inner {
    running: bool,
    /// Runs asked for (each wake), and the last request a finished run
    /// covered: until they meet, more indexing is coming even if no run
    /// has started yet, and `daemon status` says so.
    requested: u64,
    done: u64,
    fetched: u64,
    /// Every chat in the index, and how many have current chunks, as of
    /// the last count: `daemon status` (asked before every command) reads
    /// these instead of counting.
    chats: u64,
    indexed: u64,
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

impl Indexer {
    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Start a run (or another one after the current run).
    pub fn wake(&self) {
        self.inner().requested += 1;
        self.wake.notify_one();
    }

    /// A sync pass succeeded: the session is open, so runs may fetch.
    pub fn pass_succeeded(&self) {
        self.inner().may_fetch = true;
        self.wake();
    }

    /// `search-index`: fetch what's missing now, as the TS CLI's does,
    /// without waiting for a pass.
    pub fn fetch_now(&self) {
        self.pass_succeeded();
    }

    /// Chats set aside because ChatGPT didn't return them.
    pub fn unavailable_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.inner().unavailable.keys().cloned().collect();
        ids.sort();
        ids
    }

    pub fn foreground(&self) -> Foreground<'_> {
        self.foreground.fetch_add(1, Ordering::SeqCst);
        Foreground(&self.foreground)
    }

    pub fn status(&self) -> SearchIndexStatus {
        let inner = self.inner();
        SearchIndexStatus {
            chats: inner.chats,
            indexed: inner.indexed,
            in_progress: inner.running || inner.requested > inner.done,
            fetched: inner.fetched,
            failed: u64::try_from(inner.unavailable.len()).unwrap_or(u64::MAX),
            last_finished_at: inner.last_finished_at,
            last_error: inner.last_error.clone(),
            waiting: inner.waiting.clone(),
            // The embedder's own status; `daemon status` fills it in.
            embeddings: Default::default(),
        }
    }

    fn backoff_left(&self) -> Option<Duration> {
        self.inner().backoff_until.and_then(remaining)
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
        let generation = {
            let mut inner = state.indexer.inner();
            inner.running = true;
            inner.requested
        };
        let outcome = index(&state).await;
        let mut inner = state.indexer.inner();
        inner.running = false;
        inner.done = generation;
        inner.last_finished_at = Some(now_unix());
        match outcome {
            Err(message) => {
                tracing::warn!("search indexing stopped: {message}");
                inner.last_error = Some(message);
            }
            // Nothing waiting or set aside: an earlier error no longer applies.
            Ok(()) if inner.waiting.is_none() && inner.unavailable.is_empty() => {
                inner.last_error = None;
            }
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
    count(state, versions).await?;
    let missing = retryable(state, missing);
    if pruned + cached.len() + missing.len() > 0 {
        tracing::info!(
            pruned,
            chunked = cached.len(),
            to_fetch = missing.len(),
            "search indexing"
        );
    }
    let why = if missing.is_empty() {
        None
    } else {
        cannot_fetch(state)
    };
    let stop = missing.is_empty() || why.is_some();
    state.indexer.inner().waiting = why;
    if stop {
        return Ok(());
    }
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
        count(state, versions).await?;
    }
    tracing::info!(fetched, "search indexing done");
    Ok(())
}

/// Cache and chunk what a batch returned; chats it left out wait an hour.
/// A chat whose cached transcript is only older goes through the cache
/// reconcile's check first, as `sync` would have done.
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
    // Chats whose cached transcript is for an older `update_time`: the
    // reconcile's check decides whether that was only a metadata change.
    let ids: Vec<String> = batch.iter().map(|chat| chat.id.clone()).collect();
    let stale: HashMap<String, Candidate> = state
        .db(move |db| chatgpt_store::candidates(db, &ids, versions.render))
        .await
        .map_err(|failure| failure.message)?
        .into_iter()
        .map(|candidate| (candidate.id.clone(), candidate))
        .collect();
    let mut ready = Vec::new();
    let mut preserved = Vec::new();
    let mut failed = Vec::new();
    for chat in batch {
        let Some(item) = items.remove(&chat.id) else {
            tracing::info!(id = %chat.id, "ChatGPT did not return the chat for the search index");
            failed.push(chat);
            continue;
        };
        if let Some(candidate) = stale.get(&chat.id) {
            match reconcile::check(state, candidate, &item).await {
                // Unchanged: every cache moved to `update_time`, so the
                // cached transcript is current; only chunks are needed.
                Ok(true) => {
                    preserved.push((chat, candidate.markdown.clone()));
                    continue;
                }
                // Changed: replace it below and leave the rest stale.
                Ok(false) => {}
                Err(why) => {
                    tracing::warn!(id = %chat.id, "search transcript not checked: {why}");
                    failed.push(chat);
                    continue;
                }
            }
        }
        match cached_transcript(&item, &chat.update_time, versions.render) {
            Ok(transcript) => ready.push((chat, transcript)),
            Err(why) => {
                tracing::warn!(id = %chat.id, "search transcript not rendered: {why}");
                failed.push(chat);
            }
        }
    }
    unavailable(state, &failed);
    let saved = u64::try_from(ready.len() + preserved.len()).unwrap_or(u64::MAX);
    let preserved: Vec<_> = preserved
        .into_iter()
        .map(|(chat, markdown)| (chat, chunk_bytes(&markdown)))
        .collect();
    // Chunked before taking the writer.
    let ready: Vec<_> = ready
        .into_iter()
        .map(|(chat, transcript)| {
            let bodies = chunk_bytes(&transcript.markdown);
            (chat, transcript, bodies)
        })
        .collect();
    state
        .db_write(move |db| {
            for (chat, transcript, bodies) in &ready {
                chatgpt_store::save_indexed(db, transcript, chat, versions, bodies)?;
            }
            for (chat, bodies) in &preserved {
                chatgpt_store::replace_chunks(db, chat, versions, bodies)?;
            }
            Ok(())
        })
        .await
        .map_err(|failure| failure.message)?;
    Ok(saved)
}

/// Refresh the counts `daemon status` shows.
async fn count(state: &State, versions: ChunkVersions) -> Result<(), String> {
    let (chats, indexed) = state
        .db_write(move |db| chatgpt_store::coverage(db, None, versions))
        .await
        .map_err(|failure| failure.message)?;
    {
        let mut inner = state.indexer.inner();
        inner.chats = chats;
        inner.indexed = indexed;
    }
    // New chunks to embed.
    state.embedder.wake();
    Ok(())
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
    ready
}

fn unavailable(state: &State, chats: &[Unindexed]) {
    let mut inner = state.indexer.inner();
    for chat in chats {
        inner
            .unavailable
            .insert(chat.id.clone(), (chat.update_time.clone(), Instant::now()));
    }
}

fn rate_limit_note(left: Duration) -> String {
    format!("rate limited; fetching again in {}s", left.as_secs())
}

/// Why this run can't fetch now, if it can't.
fn cannot_fetch(state: &State) -> Option<String> {
    if !state.indexer.inner().may_fetch {
        return Some("transcripts are fetched after the next sync".to_owned());
    }
    if let Some(left) = state.indexer.backoff_left() {
        return Some(rate_limit_note(left));
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
        // As long as the sync's backoff would be.
        let wait = backoff_for(&error);
        let mut inner = state.indexer.inner();
        inner.backoff_until = Some(Instant::now() + wait);
        inner.waiting = Some(rate_limit_note(wait));
        inner.last_error = Some(error.message.clone());
    }
    error.message
}

/// Wait while a `sync` or `export` reads ChatGPT, so neither queues behind
/// a long indexing or embedding run.
pub(super) async fn yield_to_requests(state: &State) {
    while state.indexer.foreground.load(Ordering::SeqCst) > 0 || state.syncer.is_running() {
        tokio::time::sleep(YIELD_POLL).await;
    }
}
