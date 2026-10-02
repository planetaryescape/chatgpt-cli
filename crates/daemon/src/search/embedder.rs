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
//! ten idle minutes. A chunk the model fails on is set aside in the index
//! for a day, across restarts. Logs carry counts and chunk ids, never text.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use chatgpt_embed::MODEL_VERSION;
use chatgpt_embed::worker::vector_bytes;
use chatgpt_protocol::EmbeddingStatus;
use chatgpt_store::{ChunkVersions, NewVector};
use tokio::sync::Notify;

use super::worker::{Worker, WorkerError, WorkerModel};
use crate::state::{State, now_unix};

/// `EMBEDDING_BATCH_SIZE`: vectors saved per write.
const SAVE_EVERY: usize = 16;
/// Pending chunks read per query.
const PAGE: usize = 64;
/// How long a failed download waits before the next try (a `search-index`
/// tries at once).
const DOWNLOAD_RETRY: Duration = Duration::from_secs(15 * 60);
/// How long a chunk the model failed on is set aside, in seconds.
const FAILED_CHUNK_RETRY: i64 = 24 * 60 * 60;
/// A shorter [`DOWNLOAD_RETRY`] for tests (debug builds only).
const TEST_RETRY_ENV: &str = "CHATGPT_TEST_MODEL_RETRY_MS";

fn download_retry() -> Duration {
    std::env::var(TEST_RETRY_ENV)
        .ok()
        .filter(|_| cfg!(debug_assertions))
        .and_then(|ms| ms.parse().ok())
        .map_or(DOWNLOAD_RETRY, Duration::from_millis)
}
/// The worker stops after this long unused.
const UNLOAD_AFTER: Duration = Duration::from_secs(10 * 60);
const UNLOAD_CHECK: Duration = Duration::from_secs(60);
/// A shorter [`UNLOAD_AFTER`] (and check) for tests (debug builds only).
const TEST_IDLE_ENV: &str = "CHATGPT_TEST_EMBED_IDLE_MS";

/// How long the worker may idle, and how often that's checked.
fn unload_timing() -> (Duration, Duration) {
    std::env::var(TEST_IDLE_ENV)
        .ok()
        .filter(|_| cfg!(debug_assertions))
        .and_then(|ms| ms.parse().ok())
        .map_or((UNLOAD_AFTER, UNLOAD_CHECK), |ms| {
            let idle = Duration::from_millis(ms);
            (idle, idle / 2)
        })
}

/// Picks the fake embedder in debug builds, for tests that need vectors
/// but not the model (and no download).
const TEST_EMBEDDER_ENV: &str = "CHATGPT_TEST_EMBEDDER";
/// The longest one text may take, model load included: a worker that
/// takes longer is stopped and replaced, so a hung one can't hold every
/// semantic search.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(60);
/// A shorter [`EXCHANGE_TIMEOUT`] for tests (debug builds only).
const TEST_TIMEOUT_ENV: &str = "CHATGPT_TEST_EMBED_TIMEOUT_MS";

fn exchange_timeout() -> Duration {
    std::env::var(TEST_TIMEOUT_ENV)
        .ok()
        .filter(|_| cfg!(debug_assertions))
        .and_then(|ms| ms.parse().ok())
        .map_or(EXCHANGE_TIMEOUT, Duration::from_millis)
}

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
    /// Chunks set aside after the model failed on them, as last counted.
    failed: u64,
    waiting: Option<String>,
    last_error: Option<String>,
    last_finished_at: Option<i64>,
    /// The files were checked (or downloaded) and are in place.
    model_ready: bool,
    download_failed_at: Option<Instant>,
    last_used: Option<Instant>,
    /// When a worker last found the model unusable and woke a run to
    /// repair it; cleared by the next text embedded. At most one such wake
    /// per [`DOWNLOAD_RETRY`], so files that check out but still won't load
    /// can't keep the embedder busy.
    repair_woken_at: Option<Instant>,
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
            failed: inner.failed,
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
            // Gone from the cache since it was checked: the background run
            // fetches it again (a search never downloads).
            let why = {
                let mut inner = self.inner();
                inner.model_ready = false;
                inner.waiting.clone().unwrap_or_else(|| {
                    "the embedding model is missing from its cache; the daemon is downloading it again"
                        .to_owned()
                })
            };
            self.wake();
            return Err(why);
        }
        self.embed(&model, text).await.map_err(|error| match error {
            WorkerError::Unusable(why) if self.repairing() => {
                format!("{why}; the daemon is fetching it again")
            }
            other => other.to_string(),
        })
    }

    /// The model download failed and its backoff has passed.
    fn download_due(&self) -> bool {
        let inner = self.inner();
        !inner.model_ready
            && inner
                .download_failed_at
                .is_some_and(|at| at.elapsed() >= download_retry())
    }

    /// Whether a repair run was woken in this retry window.
    fn repairing(&self) -> bool {
        self.inner()
            .repair_woken_at
            .is_some_and(|at| at.elapsed() < download_retry())
    }

    /// One text through the worker, starting it if need be. A worker that
    /// broke is dropped, so the next text starts a fresh one.
    async fn embed(&self, model: &WorkerModel, text: &str) -> Result<Vec<f32>, WorkerError> {
        let mut worker = self.worker.lock().await;
        // Its last caller went away mid-exchange: its answer is still due.
        if worker.as_ref().is_some_and(Worker::interrupted) {
            tracing::info!("replacing an interrupted embedding worker");
            *worker = None;
        }
        let running = match worker.as_mut() {
            Some(running) => running,
            None => worker.insert(Worker::spawn(model)?),
        };
        let timeout = exchange_timeout();
        let answer = tokio::time::timeout(timeout, running.embed(text))
            .await
            .unwrap_or_else(|_| {
                Err(WorkerError::Broken(format!(
                    "didn't answer within {}s",
                    timeout.as_secs_f64()
                )))
            });
        if matches!(
            answer,
            Err(WorkerError::Broken(_) | WorkerError::Unusable(_))
        ) {
            *worker = None;
        }
        let wake = {
            let mut inner = self.inner();
            inner.last_used = Some(Instant::now());
            match &answer {
                Ok(_) => {
                    inner.repair_woken_at = None;
                    false
                }
                // A damaged file (its size can be right): a run checks the
                // files and fetches them again, under the download backoff.
                Err(WorkerError::Unusable(_)) => {
                    inner.model_ready = false;
                    let due = inner
                        .repair_woken_at
                        .is_none_or(|at| at.elapsed() >= download_retry());
                    if due {
                        inner.repair_woken_at = Some(Instant::now());
                    }
                    due
                }
                Err(_) => false,
            }
        };
        if wake {
            tracing::warn!("the embedding model can't be used; checking it again");
            self.wake();
        }
        answer
    }

    /// Stop the worker if it's been idle long enough.
    async fn unload_if_idle(&self) {
        let idle = {
            let inner = self.inner();
            !inner.running
                && inner
                    .last_used
                    .is_some_and(|used| used.elapsed() >= unload_timing().0)
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
            () = tokio::time::sleep(unload_timing().1.min(download_retry())) => {
                state.embedder.unload_if_idle().await;
                // A failed download is tried again once its backoff is
                // over, without waiting for new chunks or a `search-index`.
                if !state.embedder.download_due() {
                    continue;
                }
                // Counted as asked for, so status shows it in progress.
                state.embedder.inner().requested += 1;
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
            Ok(()) if inner.failed == 0 => inner.last_error = None,
            Ok(()) => {}
        }
    }
}

/// Count the chunks, their vectors and the chunks set aside, for `daemon
/// status`.
async fn count(state: &State, versions: ChunkVersions) -> Result<(u64, u64), String> {
    let generation = state.embedder.inner().requested;
    let (counted, failed) = state
        .db_write(move |db| {
            let counted = chatgpt_store::vector_coverage(db, None, versions, MODEL_VERSION)?;
            let failed = chatgpt_store::vector_failures(db, MODEL_VERSION, now_unix())?;
            Ok((counted, failed))
        })
        .await
        .map_err(|failure| failure.message)?;
    let mut inner = state.embedder.inner();
    (inner.chunks, inner.embedded) = counted;
    inner.failed = failed;
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
            .is_some_and(|at| at.elapsed() < download_retry())
        {
            return false;
        }
    }
    let doing = if chatgpt_embed::model::present(dir) {
        "checking the embedding model"
    } else {
        "downloading the embedding model"
    };
    state.embedder.inner().waiting = Some(doing.to_owned());
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
                "the embedding model isn't available ({why}); trying again in {} minutes",
                download_retry().as_secs().div_ceil(60)
            ));
            false
        }
    }
}

async fn embed_pending(state: &State) -> Result<(), String> {
    let versions = super::versions(state.profile());
    let model = worker_model()?;
    // Queries need the model even when every chunk has its vector, so every
    // run checks it is still there (and fetches it again if not).
    if let WorkerModel::Files(dir) = &model
        && !chatgpt_embed::model::present(dir)
    {
        state.embedder.inner().model_ready = false;
    }
    let ready = model_ready(state, &model).await;
    let (chunks, embedded) = count(state, versions).await?;
    if !ready || embedded >= chunks {
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
                chatgpt_store::pending_vectors(db, versions, MODEL_VERSION, after, PAGE, now_unix())
            })
            .await
            .map_err(|failure| failure.message)?;
        let Some(last) = page.last() else {
            break Ok(());
        };
        after = last.id;
        let mut stopped = None;
        for chunk in page {
            super::indexer::yield_to_requests(state).await;
            match state.embedder.embed(&model, &chunk.text).await {
                Ok(vector) => batch.push(NewVector {
                    chunk_id: chunk.id,
                    text: chunk.text,
                    embedding: vector_bytes(&vector),
                }),
                Err(WorkerError::Text(why)) => {
                    tracing::warn!(chunk = chunk.id, "chunk not embedded: {why}");
                    let retry_at = now_unix() + FAILED_CHUNK_RETRY;
                    state
                        .db_write(move |db| {
                            chatgpt_store::record_vector_failure(
                                db,
                                chunk.id,
                                MODEL_VERSION,
                                retry_at,
                            )
                        })
                        .await
                        .map_err(|failure| failure.message)?;
                    // `failed` comes from the next count.
                    state.embedder.inner().last_error = Some(why);
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
