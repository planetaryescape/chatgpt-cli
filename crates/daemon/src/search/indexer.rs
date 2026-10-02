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
//! Runs start when the daemon starts (chunking only) and after every sync
//! pass. Fetching starts only after this daemon's first
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

use super::chunks::transcript_chunks;
use crate::api::{ApiError, BATCH_MAX, BatchItem};
use crate::render::cached_transcript;
use crate::state::{State, now_unix};
use crate::sync::reconcile::{self, Checked};
use crate::sync::{backoff_for, remaining};

const BATCH_GAP: Duration = Duration::from_millis(500);
/// Cached transcripts chunked per write, so the writer is never held long.
const LOCAL_GROUP: usize = 20;
/// How long a chat ChatGPT didn't return waits before it's asked for again.
const RETRY_UNAVAILABLE: Duration = Duration::from_secs(60 * 60);
/// Why a chat a batch read left out is set aside.
const NOT_RETURNED: &str = "not returned by ChatGPT; run sync.";
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
    /// Chats a fetch didn't return: their `update_time` then, when, and
    /// why (an error kind and path, never a body).
    unavailable: HashMap<String, (String, Instant, String)>,
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

    /// Chats set aside because ChatGPT didn't return them, by id, with
    /// why.
    pub fn set_aside(&self) -> Vec<(String, String)> {
        let mut chats: Vec<(String, String)> = self
            .inner()
            .unavailable
            .iter()
            .map(|(id, (_, _, why))| (id.clone(), why.clone()))
            .collect();
        chats.sort();
        chats
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
    let versions = super::versions(state.profile());
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
        // Read and chunked off the writer, so a sync's writes never wait
        // on the chunking.
        let group = group.to_vec();
        let read: Vec<(Unindexed, String)> = state
            .db(move |db| {
                let mut read = Vec::new();
                for chat in group {
                    if let Some(transcript) =
                        chatgpt_store::transcript(db, &chat.id, &chat.update_time, versions.render)?
                    {
                        read.push((chat, transcript.markdown));
                    }
                }
                Ok(read)
            })
            .await
            .map_err(|failure| failure.message)?;
        let chunked: Vec<(Unindexed, String, Vec<String>)> = read
            .into_iter()
            .map(|(chat, markdown)| {
                let bodies = transcript_chunks(&markdown);
                (chat, markdown, bodies)
            })
            .collect();
        state
            .db_write(move |db| {
                for (chat, markdown, bodies) in &chunked {
                    // Only while the transcript is still the one chunked: a
                    // write in between left these chunks stale, and the
                    // next run chunks the new one.
                    let current = chatgpt_store::transcript(
                        db,
                        &chat.id,
                        &chat.update_time,
                        versions.render,
                    )?;
                    if current.is_some_and(|transcript| transcript.markdown == *markdown)
                        && still_at(db, chat)?
                    {
                        chatgpt_store::replace_chunks(db, chat, versions, bodies)?;
                    }
                }
                Ok(())
            })
            .await
            .map_err(|failure| failure.message)?;
    }
    count(state, versions).await?;
    if pruned + cached.len() > 0 {
        // Chunks to embed, or vectors gone with their chunks.
        state.embedder.wake();
    }
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
        // Holds off sync passes (and changes) for this batch's read and
        // save, so a pass can't start between the check and the request,
        // nor move the chats on between the read and the save.
        let admitted = admit_fetch(state).await;
        if let Some(why) = cannot_fetch(state) {
            state.indexer.inner().waiting = Some(why);
            break;
        }
        let ids: Vec<String> = batch.iter().map(|chat| chat.id.clone()).collect();
        let items = match api.batch(&ids).await {
            Ok(items) => items,
            Err(error) if error.is_rate_limit() => return Err(rate_limited(state, error)),
            // A timeout or a dropped connection says nothing about these
            // chats, and a session ChatGPT refuses would refuse every batch:
            // stop, and the run after the next pass tries again.
            Err(error) if matches!(error.kind, ErrorKind::Network | ErrorKind::AuthRequired) => {
                return Err(error.message);
            }
            // ChatGPT answered with an error for this batch: set its chats
            // aside for a while, so they can't hold up the rest.
            Err(error) => {
                tracing::warn!(
                    chats = ids.len(),
                    "search transcripts not fetched: {}",
                    error.message
                );
                unavailable(
                    state,
                    batch.iter().map(|chat| (chat, error.message.clone())),
                );
                state.indexer.inner().last_error = Some(error.message);
                continue;
            }
        };
        let saved = save_batch(state, batch.to_vec(), items, versions).await?;
        drop(admitted);
        fetched += saved;
        state.indexer.inner().fetched += saved;
        count(state, versions).await?;
        if saved > 0 {
            state.embedder.wake();
        }
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
            failed.push((chat, NOT_RETURNED.to_owned()));
            continue;
        };
        if let Some(candidate) = stale.get(&chat.id) {
            match reconcile::check(state, candidate, &item).await {
                // Unchanged: every cache moved to `update_time`, so the
                // cached transcript is current; only chunks are needed.
                Ok(Checked::Unchanged) => {
                    preserved.push((chat, candidate.markdown.clone()));
                    continue;
                }
                // Changed, or the cache was replaced meanwhile: replace it
                // with this fetch below and leave the rest stale.
                Ok(Checked::Changed | Checked::Superseded) => {}
                Err(why) => {
                    tracing::warn!(id = %chat.id, "search transcript not checked: {why}");
                    failed.push((chat, why));
                    continue;
                }
            }
        }
        match cached_transcript(&item, &chat.update_time, versions.render) {
            Ok(transcript) => ready.push((chat, transcript)),
            Err(why) => {
                tracing::warn!(id = %chat.id, "search transcript not rendered: {why}");
                failed.push((chat, why));
            }
        }
    }
    unavailable(state, failed.iter().map(|(chat, why)| (chat, why.clone())));
    let preserved: Vec<_> = preserved
        .into_iter()
        .map(|(chat, markdown)| {
            let bodies = transcript_chunks(&markdown);
            (chat, markdown, bodies)
        })
        .collect();
    // Chunked before taking the writer.
    let ready: Vec<_> = ready
        .into_iter()
        .map(|(chat, transcript)| {
            let bodies = transcript_chunks(&transcript.markdown);
            (chat, transcript, bodies)
        })
        .collect();
    state
        .db_write(move |db| save_fetched(db, &ready, &preserved, versions))
        .await
        .map_err(|failure| failure.message)
}

/// Write what a batch brought, for each chat only while the index still
/// has it at the `update_time` it was fetched for (and, for chunks of a
/// kept transcript, while that transcript is unchanged): a snapshot a
/// write overtook is never saved over the newer caches, and the next run
/// fetches the chat again. How many were saved.
fn save_fetched(
    db: &mut rusqlite::Connection,
    ready: &[(Unindexed, chatgpt_store::Transcript, Vec<String>)],
    preserved: &[(Unindexed, String, Vec<String>)],
    versions: ChunkVersions,
) -> chatgpt_store::Result<u64> {
    let mut saved = 0;
    for (chat, transcript, bodies) in ready {
        if still_at(db, chat)? {
            chatgpt_store::save_indexed(db, transcript, chat, versions, bodies)?;
            saved += 1;
        }
    }
    for (chat, markdown, bodies) in preserved {
        let kept = chatgpt_store::transcript(db, &chat.id, &chat.update_time, versions.render)?
            .is_some_and(|transcript| transcript.markdown == *markdown);
        if kept && still_at(db, chat)? {
            chatgpt_store::replace_chunks(db, chat, versions, bodies)?;
            saved += 1;
        }
    }
    Ok(saved)
}

/// Whether the index still has `chat` at the `update_time` it was read at.
fn still_at(db: &rusqlite::Connection, chat: &Unindexed) -> chatgpt_store::Result<bool> {
    let current: Option<String> = rusqlite::OptionalExtension::optional(db.query_row(
        "select update_time from conversations where id = ?",
        [&chat.id],
        |row| row.get(0),
    ))?;
    Ok(current.as_deref() == Some(chat.update_time.as_str()))
}

/// Refresh the counts `daemon status` shows.
async fn count(state: &State, versions: ChunkVersions) -> Result<(), String> {
    let (chats, indexed) = state
        .db_write(move |db| chatgpt_store::coverage(db, None, versions))
        .await
        .map_err(|failure| failure.message)?;
    let mut inner = state.indexer.inner();
    inner.chats = chats;
    inner.indexed = indexed;
    Ok(())
}

/// `missing` without chats ChatGPT recently didn't return (unless they
/// changed since).
fn retryable(state: &State, missing: Vec<Unindexed>) -> Vec<Unindexed> {
    without_set_aside(&mut state.indexer.inner().unavailable, missing)
}

/// Drops set-aside entries that no longer hold (expired, or for a chat that
/// changed, got indexed or left the index), so `daemon status` counts only
/// chats still waiting, then `missing` without the ones that do.
fn without_set_aside(
    unavailable: &mut HashMap<String, (String, Instant, String)>,
    missing: Vec<Unindexed>,
) -> Vec<Unindexed> {
    let current: HashMap<&str, &str> = missing
        .iter()
        .map(|chat| (chat.id.as_str(), chat.update_time.as_str()))
        .collect();
    unavailable.retain(|id, (update_time, at, _)| {
        at.elapsed() < RETRY_UNAVAILABLE && current.get(id.as_str()) == Some(&update_time.as_str())
    });
    missing
        .into_iter()
        .filter(|chat| !unavailable.contains_key(&chat.id))
        .collect()
}

fn unavailable<'a>(state: &State, chats: impl IntoIterator<Item = (&'a Unindexed, String)>) {
    let mut inner = state.indexer.inner();
    for (chat, why) in chats {
        inner.unavailable.insert(
            chat.id.clone(),
            (chat.update_time.clone(), Instant::now(), why),
        );
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

/// Wait until no `export` reads ChatGPT and no sync pass (or change) runs,
/// then hold the pass lock: until the guard drops, no pass can start, so
/// the indexer's read and a pass's never overlap.
async fn admit_fetch(state: &State) -> tokio::sync::MutexGuard<'_, ()> {
    loop {
        if state.indexer.foreground.load(Ordering::SeqCst) == 0
            && let Some(guard) = state.syncer.try_exclusive()
        {
            return guard;
        }
        tokio::time::sleep(YIELD_POLL).await;
    }
}

/// Wait while a `sync` or `export` reads ChatGPT, so neither queues behind
/// a long indexing or embedding run.
pub(super) async fn yield_to_requests(state: &State) {
    while state.indexer.foreground.load(Ordering::SeqCst) > 0 || state.syncer.is_running() {
        tokio::time::sleep(YIELD_POLL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(id: &str, update_time: &str) -> Unindexed {
        Unindexed {
            id: id.into(),
            title: id.into(),
            update_time: update_time.into(),
            cached: false,
        }
    }

    #[test]
    fn a_fetch_a_sync_overtook_never_replaces_the_newer_transcript() {
        let dir = tempfile::tempdir().expect("dir");
        let store = chatgpt_store::Store::open(&dir.path().join("chatgpt.db")).expect("store");
        let versions = ChunkVersions {
            render: 2,
            chunk: 2,
        };
        let transcript = |update_time: &str, markdown: &str| chatgpt_store::Transcript {
            id: "a".into(),
            update_time: update_time.into(),
            render_version: 2,
            markdown: markdown.into(),
            turns: 1,
            approx_tokens: 1,
        };
        // A sync moved the chat to t2 and cached its transcript there
        // while the indexer held a t1 snapshot.
        let at_t2 = chat("a", "t2");
        store
            .write(|db| {
                chatgpt_store::replace_all(
                    db,
                    &[chatgpt_store::NewConversation {
                        id: "a".into(),
                        title: "a".into(),
                        create_time: "t0".into(),
                        update_time: "t2".into(),
                        is_archived: false,
                        pinned: false,
                        project_id: None,
                    }],
                    "now",
                )?;
                chatgpt_store::save_indexed(db, &transcript("t2", "newer"), &at_t2, versions, &[])
            })
            .expect("t2");
        let stale = vec![(chat("a", "t1"), transcript("t1", "older"), Vec::new())];
        let saved = store
            .write(|db| save_fetched(db, &stale, &[], versions))
            .expect("saved");
        assert_eq!(saved, 0);
        let kept = store
            .read(|db| chatgpt_store::transcript(db, "a", "t2", 2))
            .expect("read")
            .expect("the t2 transcript");
        assert_eq!(kept.markdown, "newer");

        // A current one is saved.
        let current = vec![(at_t2, transcript("t2", "fetched"), Vec::new())];
        let saved = store
            .write(|db| save_fetched(db, &current, &[], versions))
            .expect("saved");
        assert_eq!(saved, 1);
    }

    #[test]
    fn a_set_aside_chat_that_changed_or_left_stops_counting_as_failed() {
        let mut unavailable: HashMap<String, (String, Instant, String)> =
            [("same", "t1"), ("changed", "t1"), ("indexed-since", "t1")]
                .into_iter()
                .map(|(id, time)| {
                    let entry = (time.to_owned(), Instant::now(), NOT_RETURNED.to_owned());
                    (id.to_owned(), entry)
                })
                .collect();
        let ready = without_set_aside(
            &mut unavailable,
            vec![chat("same", "t1"), chat("changed", "t2"), chat("new", "t1")],
        );
        let ready: Vec<&str> = ready.iter().map(|chat| chat.id.as_str()).collect();
        assert_eq!(ready, ["changed", "new"]);
        let mut left: Vec<&str> = unavailable.keys().map(String::as_str).collect();
        left.sort_unstable();
        assert_eq!(left, ["same"], "only the unchanged chat stays set aside");
    }
}
