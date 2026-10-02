//! Classifying chats and saved memories, natively: `classify`, `titles`,
//! `memory classify`, the Jev guard's judging (`crate::jev`) and the
//! background Jev. Ported from the TS CLI's `src/classify/` @ 1b8c950 with
//! its models, prompts, flags, fallback order, pacing, notes and cost
//! lines, at this repository's question and policy versions (D9).
//!
//! - [`pipeline`]: Jev's first pass (download, judge short chats, then
//!   summarise and judge long ones);
//! - [`deep`]: Jev's follow-up for unsure chats;
//! - [`review`]: Luna's review of what Jev can't settle;
//! - [`titles`]: Luna's local titles and themes;
//! - [`memories`]: saved-memory classification;
//! - [`auto_jev`]: Jev on newly synced chats, in the background;
//! - [`summarise`], [`luna`], [`tools`]: the model calls, through the APIs
//!   with a key or the `codex` and `claude` CLIs without;
//! - [`access`]: where keys and tools come from; [`costs`]: the meter.
//!
//! Every run that a client asked for runs in its own task, so what it paid
//! for is saved even if the client goes away; a client that went away
//! stops it from starting further calls.

mod access;
pub mod auto_jev;
mod costs;
mod deep;
pub mod flight;
mod luna;
mod memories;
pub mod pipeline;
mod questions;
mod review;
mod summarise;
mod titles;
mod tools;

pub use summarise::SUMMARY_PROMPT_VERSION;

use std::sync::Arc;
use std::time::Instant;

use chatgpt_protocol::{ClassifiedMemories, ClassifyOutcome, ModelAccess, Progress, SessionChoice};
use chatgpt_store::IndexedConversation;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

pub use access::Access;
use pipeline::{Classifier, Options, profile};

use crate::handlers::Failure;
use crate::policy::Judged;
use crate::progress::{Asker, Reporter};
use crate::state::State;

/// The chats `ids` name, in order; ids no longer in the index are left
/// out (the client resolved them a moment ago).
pub async fn chats(state: &State, ids: Vec<String>) -> Result<Vec<IndexedConversation>, Failure> {
    let display = profile().local_title_version;
    state
        .db(move |db| chatgpt_store::by_ids(db, &ids, display))
        .await
}

/// `classify`: Jev, its follow-up for the unsure, Luna's review, then the
/// missing local titles, and the summary of how they came out.
#[allow(clippy::too_many_arguments, reason = "one request's fields")]
pub async fn classify(
    state: &Arc<State>,
    ids: Vec<String>,
    redo: bool,
    yes: bool,
    access: ModelAccess,
    session: SessionChoice,
    progress: Option<UnboundedSender<Progress>>,
    answers: Option<UnboundedReceiver<bool>>,
) -> Result<ClassifyOutcome, Failure> {
    let state = Arc::clone(state);
    crate::progress::spawn(async move {
        let started = Instant::now();
        let reporter = Reporter::for_client(progress);
        let asker = answers.map(|answers| Asker::new(reporter.clone(), answers));
        let access = Access::for_client(access);
        // Waits out the background Jev on these chats, then keeps it off
        // them until this run ends.
        let _claim = state.flight.claim(&ids).await;
        let targets = chats(&state, ids.clone()).await?;
        let classifier = Classifier {
            state: &state,
            reporter: &reporter,
            session: session.clone(),
            access: &access,
            asker: asker.as_ref(),
        };
        let options = Options {
            force: redo,
            yes,
            summarise: true,
        };
        let classified = classifier.classify(&targets, options).await?;
        let mut judgments = classified.judgments;
        // Each stage reads the chats afresh: a sync may have moved them (and
        // their caches) forward while the one before was waiting on a model.
        let targets = chats(&state, ids.clone()).await?;
        let mut unsure = Vec::new();
        for chat in &targets {
            if let Some(row) = judgments.get(&chat.id) {
                let verdict = Judged::new(row, profile())
                    .and_then(|judged| judged.base_verdict())
                    .map_err(Failure::policy)?;
                if verdict.unsure {
                    unsure.push(chat.clone());
                }
            }
        }
        let deep = if unsure.is_empty() {
            None
        } else {
            let mut deep = deep::DeepClassifier {
                state: &state,
                reporter: &reporter,
                session,
                access: &access,
                yes,
                asker: asker.as_ref(),
            }
            .classify(&unsure, redo)
            .await?;
            judgments.extend(std::mem::take(&mut deep.judgments));
            Some(deep)
        };
        let targets = chats(&state, ids.clone()).await?;
        let luna_failures =
            review::review(&state, &reporter, &access, &targets, &mut judgments, redo).await?;
        let targets = chats(&state, ids).await?;
        let title_failures = titles::generate(&state, &reporter, &access, &targets, false).await?;
        let (mut delete, mut archive, mut keep, mut still_unsure) = (0, 0, 0, 0);
        for row in judgments.values() {
            let verdict = Judged::new(row, profile())
                .and_then(|judged| judged.verdict())
                .map_err(Failure::policy)?;
            match verdict.suggestion.as_str() {
                "delete" => delete += 1,
                "archive" => archive += 1,
                _ => keep += 1,
            }
            if verdict.unsure {
                still_unsure += 1;
            }
        }
        let (deep_failures, deep_held_back) = deep
            .map(|deep| (deep.failures, deep.held_back))
            .unwrap_or_default();
        let all_failures = [
            &classified.failures,
            &deep_failures,
            &luna_failures,
            &title_failures,
        ];
        for failure in all_failures.iter().flat_map(|list| list.iter()) {
            reporter.note(format!("failed: {failure}"));
        }
        let mut done = format!(
            "Done in {}. {} of {} judged: delete {delete}, archive {archive}, keep {keep} ({still_unsure} still unsure).",
            chatgpt_core::format_duration(started.elapsed().as_millis() as f64),
            judgments.len(),
            targets.len()
        );
        if !classified.held_back.is_empty() {
            done.push_str(&format!(
                " {} long chat(s) skipped for lack of a summary.",
                classified.held_back.len()
            ));
        }
        if deep_held_back > 0 {
            done.push_str(&format!(
                " {deep_held_back} deep classification(s) held back."
            ));
        }
        reporter.note(done);
        reporter.note(
            "Next: `chatgpt list --suggest delete`, then `chatgpt review --suggest delete`."
                .to_owned(),
        );
        let failed = all_failures.iter().any(|list| !list.is_empty());
        tracing::info!(
            chats = targets.len(),
            judged = judgments.len(),
            failed,
            "classify"
        );
        Ok(ClassifyOutcome { failed })
    })
    .await
    .map_err(Failure::join)?
}

/// `titles`: Luna's local titles for the chats that lack one (all but
/// manual ones with `redo`).
pub async fn titles(
    state: &Arc<State>,
    ids: Vec<String>,
    redo: bool,
    access: ModelAccess,
    progress: Option<UnboundedSender<Progress>>,
) -> Result<ClassifyOutcome, Failure> {
    let state = Arc::clone(state);
    crate::progress::spawn(async move {
        let reporter = Reporter::for_client(progress);
        let access = Access::for_client(access);
        let targets = chats(&state, ids).await?;
        let failures = titles::generate(&state, &reporter, &access, &targets, redo).await?;
        for failure in &failures {
            reporter.note(format!("failed: {failure}"));
        }
        Ok(ClassifyOutcome {
            failed: !failures.is_empty(),
        })
    })
    .await
    .map_err(Failure::join)?
}

/// `memory classify`: the rows to print, and what failed.
pub async fn memory_classify(
    state: &Arc<State>,
    redo: bool,
    access: ModelAccess,
    session: SessionChoice,
    progress: Option<UnboundedSender<Progress>>,
) -> Result<ClassifiedMemories, Failure> {
    let state = Arc::clone(state);
    crate::progress::spawn(async move {
        let reporter = Reporter::for_client(progress);
        let access = Access::for_client(access);
        memories::classify(&state, &reporter, &access, session, redo).await
    })
    .await
    .map_err(Failure::join)?
}
