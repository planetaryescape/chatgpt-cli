//! `search-index`: the daemon indexes and embeds in the background anyway,
//! so this asks both to run now (fetching missing transcripts at once, as
//! the TS CLI's `search-index` does, and retrying a failed model download),
//! shows their progress, and reports on the scope once neither has work
//! left. `--archived` and `--all` scope the report as the TS CLI scopes its
//! work.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chatgpt_protocol::{Progress, SearchIndexReport};
use tokio::sync::mpsc::UnboundedSender;

use super::Coverage;
use crate::handlers::Failure;
use crate::progress::{Reporter, Step};
use crate::reads::require_synced;
use crate::state::State;

/// Each poll counts the scope's chunks and vectors (tens of ms on BK's
/// 47,000 chunks), so not too often.
const POLL: Duration = Duration::from_secs(1);

async fn counts(state: &State, archived: Option<bool>) -> Result<Coverage, Failure> {
    let versions = super::versions(&state.profile());
    state
        .db(move |db| super::coverage(db, archived, versions))
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

    // Each step counts what was left when it began, as the TS CLI's
    // counts the chats it downloads and the chunks it embeds.
    let start = counts(state, scope).await?;
    let indexed_before = start.indexed;
    let mut indexing = Some(reporter.step(
        "Indexing search transcripts",
        Some(start.chats.saturating_sub(indexed_before) as usize),
    ));
    let mut embedding: Option<(Step, u64)> = None;
    let Coverage {
        chats,
        indexed,
        chunks,
        embedded,
    } = loop {
        let indexer_busy = state.indexer.status().in_progress;
        let embedder_busy = state.embedder.status().in_progress;
        let now = counts(state, scope).await?;
        let Coverage {
            indexed,
            chunks,
            embedded,
            ..
        } = now;
        let indexed_now = indexed.saturating_sub(indexed_before);
        if let Some(step) = &indexing {
            step.update(indexed_now as usize);
        }
        if !indexer_busy && let Some(step) = indexing.take() {
            step.finish(&format!("Indexed {indexed_now} chat(s)"));
            let step = reporter.step(
                "Embedding search chunks",
                Some(chunks.saturating_sub(embedded) as usize),
            );
            embedding = Some((step, embedded));
        }
        if let Some((step, before)) = &embedding {
            step.update(embedded.saturating_sub(*before) as usize);
        }
        if !indexer_busy && !embedder_busy {
            break now;
        }
        tokio::time::sleep(POLL).await;
    };
    if let Some((step, before)) = embedding {
        step.finish(&format!(
            "Embedded {} chunk(s)",
            embedded.saturating_sub(before)
        ));
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
