//! The background embedder: `search-index`'s embedding half
//! (`SearchIndexer.build` in the TS CLI's `src/search/indexer.ts` @
//! 1b8c950), run by the daemon after the indexer chunks, so semantic search
//! never needs a manual step.
//!
//! A run embeds every chunk that's current for its chat and has no vector
//! from this model (`pendingVectors`), one text at a time in the worker
//! process, and saves vectors 16 at a time, so a run that stops resumes
//! where it stopped. It steps aside while a `sync` or `export` runs, and a
//! semantic search's query goes to the worker before the next chunk.
//!
//! The model files are downloaded on the first run that needs them. A
//! failed download (offline, say) is retried after a while and never
//! affects lexical search; `daemon status` and semantic search say why
//! they're waiting. The worker, and the model with it, is stopped after
//! ten idle minutes. Logs carry counts and chunk ids, never text.

use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use chatgpt_embed::MODEL_VERSION;
use chatgpt_embed::worker::vector_bytes;
use chatgpt_protocol::EmbeddingStatus;
use chatgpt_store::{ChunkVersions, NewVector};
use tokio::sync::Notify;

use super::bun_text;
use super::worker::{Worker, WorkerError, WorkerModel};
use crate::state::{State, now_unix};

/// `EMBEDDING_BATCH_SIZE`: vectors saved per write.
const SAVE_EVERY: usize = 16;
/// Pending chunks read per query.
const PAGE: usize = 64;
/// How long a failed download waits before the next try (a `search-index`
/// tries at once).
const DOWNLOAD_RETRY: Duration = Duration::from_secs(15 * 60);
/// The worker stops after this long unused.
const UNLOAD_AFTER: Duration = Duration::from_secs(10 * 60);
const UNLOAD_CHECK: Duration = Duration::from_secs(60);

/// Picks the fake embedder in debug builds, for tests that need vectors
/// but not the model (and no download).
const TEST_EMBEDDER_ENV: &str = "CHATGPT_TEST_EMBEDDER";

#[derive(Default)]
pub struct Embedder {
    wake: Notify,
    inner: Mutex<Inner>,
    /// One worker, shared by the background run and search queries; the
    /// lock is fair, so a query waits for one chunk at most.
    worker: tokio::sync::Mutex<Option<Worker>>,
}

#[derive(Default)]
struct Inner {
    running: bool,
    /// Runs asked for, and the last request a finished run covered.
    requested: u64,
    done: u64,
    chunks: u64,
    embedded: u64,
    /// The request generation `chunks` was counted for: a later wake means
    /// the indexer wrote chunks since.
    counted_for: u64,
    /// Chunks the model failed on, skipped until the daemon restarts.
    failed: HashSet<i64>,
    waiting: Option<String>,
    last_error: Option<String>,
    last_finished_at: Option<i64>,
    /// The files were checked (or downloaded) and are in place.
    model_ready: bool,
    download_failed_at: Option<Instant>,
    last_used: Option<Instant>,
}

/// The worker's embedder: the model in the cache, or the fake in tests.
fn worker_model() -> Result<WorkerModel, String> {
    let fake = std::env::var(TEST_EMBEDDER_ENV).is_ok_and(|value| value == "fake");
    if cfg!(debug_assertions) && fake {
        return Ok(WorkerModel::Fake);
    }
    super::model::dir().map(WorkerModel::Files)
}

impl Embedder {
    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Start a run (or another one after the current run).
    pub fn wake(&self) {
        self.inner().requested += 1;
        self.wake.notify_one();
    }

    /// `search-index`: try a failed download again now, and run.
    pub fn retry_now(&self) {
        self.inner().download_failed_at = None;
        self.wake();
    }

    pub fn status(&self) -> EmbeddingStatus {
        let inner = self.inner();
        EmbeddingStatus {
            chunks: inner.chunks,
            embedded: inner.embedded,
            in_progress: inner.running || inner.requested > inner.done,
            failed: u64::try_from(inner.failed.len()).unwrap_or(u64::MAX),
            waiting: inner.waiting.clone(),
            last_error: inner.last_error.clone(),
            last_finished_at: inner.last_finished_at,
        }
    }

    /// Embed a search query, ahead of the next background chunk.
    pub async fn embed_query(&self, text: &str) -> Result<Vec<f32>, String> {
        let model = worker_model()?;
        if let WorkerModel::Files(dir) = &model
            && !chatgpt_embed::model::present(dir)
        {
            return Err(self
                .inner()
                .waiting
                .clone()
                .unwrap_or_else(|| "the embedding model isn't downloaded yet".to_owned()));
        }
        self.embed(&model, text)
            .await
            .map_err(|error| error.to_string())
    }

    /// One text through the worker, starting it if need be. A worker that
    /// broke is dropped, so the next text starts a fresh one.
    async fn embed(&self, model: &WorkerModel, text: &str) -> Result<Vec<f32>, WorkerError> {
        let mut worker = self.worker.lock().await;
        let running = match worker.as_mut() {
            Some(running) => running,
            None => worker.insert(Worker::spawn(model)?),
        };
        let answer = running.embed(text).await;
        if matches!(
            answer,
            Err(WorkerError::Broken(_) | WorkerError::Unusable(_))
        ) {
            *worker = None;
        }
        self.inner().last_used = Some(Instant::now());
        answer
    }

    /// Stop the worker if it's been idle long enough.
    async fn unload_if_idle(&self) {
        let idle = {
            let inner = self.inner();
            !inner.running
                && inner
                    .last_used
                    .is_some_and(|used| used.elapsed() >= UNLOAD_AFTER)
        };
        if !idle {
            return;
        }
        let mut worker = self.worker.lock().await;
        if worker.take().is_some() {
            tracing::info!("stopped the idle embedding worker");
        }
        self.inner().last_used = None;
    }
}

/// The embedder's loop, for the daemon's life.
pub async fn run(state: std::sync::Arc<State>) {
    loop {
        tokio::select! {
            () = state.embedder.wake.notified() => {}
            () = tokio::time::sleep(UNLOAD_CHECK) => {
                state.embedder.unload_if_idle().await;
                continue;
            }
        }
        let generation = {
            let mut inner = state.embedder.inner();
            inner.running = true;
            inner.requested
        };
        let outcome = embed_pending(&state).await;
        let mut inner = state.embedder.inner();
        inner.running = false;
        inner.done = generation;
        inner.last_finished_at = Some(now_unix());
        match outcome {
            Err(message) => {
                tracing::warn!("embedding stopped: {message}");
                inner.last_error = Some(message);
            }
            Ok(()) if inner.failed.is_empty() => inner.last_error = None,
            Ok(()) => {}
        }
    }
}

/// Count the chunks and their vectors, for `daemon status`.
async fn count(state: &State, versions: ChunkVersions) -> Result<(u64, u64), String> {
    let generation = state.embedder.inner().requested;
    let counted = state
        .db_write(move |db| chatgpt_store::vector_coverage(db, None, versions, MODEL_VERSION))
        .await
        .map_err(|failure| failure.message)?;
    let mut inner = state.embedder.inner();
    (inner.chunks, inner.embedded) = counted;
    inner.counted_for = generation;
    Ok(counted)
}

/// Have the model files ready, downloading them if need be. `false`: not
/// now, and `waiting` says why.
async fn model_ready(state: &State, model: &WorkerModel) -> bool {
    let WorkerModel::Files(dir) = model else {
        return true;
    };
    {
        let inner = state.embedder.inner();
        if inner.model_ready {
            return true;
        }
        if inner
            .download_failed_at
            .is_some_and(|at| at.elapsed() < DOWNLOAD_RETRY)
        {
            return false;
        }
    }
    state.embedder.inner().waiting = Some("downloading the embedding model".to_owned());
    let ready = super::model::ensure(dir).await;
    let mut inner = state.embedder.inner();
    match ready {
        Ok(()) => {
            inner.model_ready = true;
            inner.waiting = None;
            true
        }
        Err(why) => {
            tracing::warn!("embedding model unavailable: {why}");
            inner.download_failed_at = Some(Instant::now());
            inner.waiting = Some(format!(
                "the embedding model isn't available ({why}); trying again after the next sync"
            ));
            false
        }
    }
}

async fn embed_pending(state: &State) -> Result<(), String> {
    let versions = super::versions(&state.profile());
    let (chunks, embedded) = count(state, versions).await?;
    if embedded >= chunks {
        state.embedder.inner().waiting = None;
        return Ok(());
    }
    let model = worker_model()?;
    if !model_ready(state, &model).await {
        return Ok(());
    }
    tracing::info!(to_embed = chunks - embedded, "embedding search chunks");
    let mut after = 0;
    let mut batch = Vec::with_capacity(SAVE_EVERY);
    let mut made = 0usize;
    let result = loop {
        // The indexer adds chunks while a run goes on (and wakes the
        // embedder when it does): keep the total true.
        let stale = {
            let inner = state.embedder.inner();
            inner.requested != inner.counted_for
        };
        if stale {
            count(state, versions).await?;
        }
        let page = state
            .db_write(move |db| {
                chatgpt_store::pending_vectors(db, versions, MODEL_VERSION, after, PAGE)
            })
            .await
            .map_err(|failure| failure.message)?;
        let Some(last) = page.last() else {
            break Ok(());
        };
        after = last.id;
        let mut stopped = None;
        for chunk in page {
            if state.embedder.inner().failed.contains(&chunk.id) {
                continue;
            }
            super::indexer::yield_to_requests(state).await;
            let text = bun_text(&chunk.text).into_owned();
            match state.embedder.embed(&model, &text).await {
                Ok(vector) => batch.push(NewVector {
                    chunk_id: chunk.id,
                    text: chunk.text,
                    embedding: vector_bytes(&vector),
                }),
                Err(WorkerError::Text(why)) => {
                    tracing::warn!(chunk = chunk.id, "chunk not embedded: {why}");
                    let mut inner = state.embedder.inner();
                    inner.failed.insert(chunk.id);
                    inner.last_error = Some(why);
                }
                Err(WorkerError::Unusable(why)) => {
                    // A damaged file: checked (and fetched again) next run.
                    state.embedder.inner().model_ready = false;
                    stopped = Some(why);
                    break;
                }
                Err(broken @ WorkerError::Broken(_)) => {
                    stopped = Some(broken.to_string());
                    break;
                }
            }
            if batch.len() == SAVE_EVERY {
                made += save(state, &mut batch).await?;
            }
        }
        if let Some(why) = stopped {
            break Err(why);
        }
    };
    made += save(state, &mut batch).await?;
    count(state, versions).await?;
    tracing::info!(embedded = made, "embedding done");
    result
}

async fn save(state: &State, batch: &mut Vec<NewVector>) -> Result<usize, String> {
    if batch.is_empty() {
        return Ok(0);
    }
    let vectors = std::mem::take(batch);
    let saved = state
        .db_write(move |db| chatgpt_store::save_vectors(db, &vectors, MODEL_VERSION))
        .await
        .map_err(|failure| failure.message)?;
    let mut inner = state.embedder.inner();
    inner.embedded += saved as u64;
    // A page can hold chunks written after the last count; the next page
    // recounts.
    inner.chunks = inner.chunks.max(inner.embedded);
    Ok(saved)
}
