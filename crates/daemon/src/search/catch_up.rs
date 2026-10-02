//! `search-index`: the daemon indexes and embeds in the background anyway,
//! so this asks both to run now (fetching missing transcripts at once, as
//! the TS CLI's `search-index` does, and retrying a failed model download),
//! shows their progress, and reports on the scope once neither has work
//! left. `--archived` and `--all` scope the report as the TS CLI scopes its
//! work.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chatgpt_embed::MODEL_VERSION;
use chatgpt_protocol::{Progress, SearchIndexReport};
use tokio::sync::mpsc::UnboundedSender;

use crate::handlers::Failure;
use crate::progress::{Reporter, Step};
use crate::reads::require_synced;
use crate::state::State;

const POLL: Duration = Duration::from_millis(500);

/// Chats, indexed chats, chunks and embedded chunks in scope.
async fn counts(state: &State, archived: Option<bool>) -> Result<[u64; 4], Failure> {
    let versions = super::versions(&state.profile());
    state
        .db(move |db| {
            let (chats, indexed) = chatgpt_store::coverage(db, archived, versions)?;
            let (chunks, embedded) =
                chatgpt_store::vector_coverage(db, archived, versions, MODEL_VERSION)?;
            Ok([chats, indexed, chunks, embedded])
        })
        .await
}

pub async fn search_index(
    state: &Arc<State>,
    archived: bool,
    all: bool,
    progress: Option<UnboundedSender<Progress>>,
) -> Result<SearchIndexReport, Failure> {
    let started = Instant::now();
    state.db(|db| Ok(require_synced(db))).await??;
    let scope = (!all).then_some(archived);
    let reporter = Reporter::default();
    let _attached = progress.map(|sender| reporter.attach(sender));
    state.indexer.fetch_now();
    state.embedder.retry_now();

    let [chats, ..] = counts(state, scope).await?;
    let mut indexing: Option<Step> =
        Some(reporter.step("Indexing search transcripts", Some(chats as usize)));
    let mut embedding: Option<Step> = None;
    let [chats, indexed, chunks, embedded] = loop {
        let indexer_busy = state.indexer.status().in_progress;
        let embedder_busy = state.embedder.status().in_progress;
        let now = counts(state, scope).await?;
        let [_, indexed, chunks, embedded] = now;
        if let Some(step) = &indexing {
            step.update(indexed as usize);
        }
        if !indexer_busy && let Some(step) = indexing.take() {
            step.finish(&format!("Indexed {indexed} chat(s)"));
            embedding = Some(reporter.step("Embedding search chunks", Some(chunks as usize)));
        }
        if let Some(step) = &embedding {
            step.update(embedded as usize);
        }
        if !indexer_busy && !embedder_busy {
            break now;
        }
        tokio::time::sleep(POLL).await;
    };
    if let Some(step) = embedding {
        step.finish(&format!("Embedded {embedded} chunk(s)"));
    }

    let unavailable = state.indexer.unavailable_ids();
    let profile = state.profile();
    let failures = state
        .db(move |db| {
            let mut failures = Vec::new();
            for id in unavailable {
                for chat in chatgpt_store::get(db, &id, profile.local_title_version)? {
                    if scope.is_none_or(|archived| chat.is_archived == archived) {
                        failures.push(format!(
                            "{} {}: not returned by ChatGPT; run sync.",
                            chat.id, chat.title
                        ));
                    }
                }
            }
            Ok(failures)
        })
        .await?;
    let waiting = [
        state.indexer.status().waiting,
        state.embedder.status().waiting,
    ]
    .into_iter()
    .flatten()
    .collect();
    Ok(SearchIndexReport {
        chats,
        indexed,
        chunks,
        embedded,
        failures,
        waiting,
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}
