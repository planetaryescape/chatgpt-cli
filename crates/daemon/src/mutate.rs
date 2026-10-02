//! `archive`, `unarchive`, `delete`, `rename` and `title`: ChatGPT first,
//! then the index, as the TS CLI's `performAction` and `applyAction`
//! (`src/commands/mutate.ts`) and its `rename` and `title` commands
//! (`src/cli.ts`) @ 1b8c950 do. The client has already shown the preview
//! and had the user confirm; these act on exactly the chats it sends.
//!
//! A bulk change runs in its own task, so a client that goes away (Ctrl-C)
//! never leaves a chat changed in ChatGPT but not in the index: the chat in
//! hand is finished, and no further one is started.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chatgpt_core::ErrorKind;
use chatgpt_protocol::{ChatAction, Outcome, Progress, SessionChoice, Target};
use tokio::sync::mpsc::UnboundedSender;

use crate::api::{Api, ApiError};
use crate::handlers::Failure;
use crate::progress::{Reporter, Step};
use crate::select;
use crate::state::State;

/// Gentle pacing: a private API on a personal account.
pub const DELAY: Duration = Duration::from_millis(250);
const DELETE_WORKERS: usize = 3;

const RENAME_500: &str = "ChatGPT returned a server error. On older chats the rename usually applies anyway; run `chatgpt sync` and `chatgpt list --title` to check.";

/// Set when the request's handler is dropped (its client went away), so a
/// bulk task stops before its next item.
pub struct StopOnDrop(pub Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// A bulk change's running tally, shared by its workers.
#[derive(Default)]
pub struct Tally {
    attempted: AtomicUsize,
    done: AtomicUsize,
    failures: Mutex<Vec<String>>,
}

impl Tally {
    /// Count one item and show it on `step`: the TS CLI's
    /// `step.update(n, failures ? "N failed" : "")`.
    pub fn record(&self, step: &Step, target: &Target, result: Result<(), ApiError>) {
        let failed = {
            let mut failures = self.failures();
            match result {
                Ok(()) => {
                    self.done.fetch_add(1, Ordering::SeqCst);
                }
                Err(error) => {
                    failures.push(format!("{} {}: {}", target.id, target.title, error.message));
                }
            }
            failures.len()
        };
        let attempted = self.attempted.fetch_add(1, Ordering::SeqCst) + 1;
        let detail = if failed > 0 {
            format!("{failed} failed")
        } else {
            String::new()
        };
        step.update_with(attempted, &detail);
    }

    fn failures(&self) -> std::sync::MutexGuard<'_, Vec<String>> {
        self.failures.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `, N failed` for a summary line, or nothing.
    pub fn failed_suffix(&self) -> String {
        match self.failures().len() {
            0 => String::new(),
            failed => format!(", {failed} failed"),
        }
    }

    pub fn done(&self) -> usize {
        self.done.load(Ordering::SeqCst)
    }

    pub fn into_outcome(self) -> Outcome {
        Outcome {
            done: u64::try_from(self.done.into_inner()).unwrap_or(u64::MAX),
            failures: self
                .failures
                .into_inner()
                .unwrap_or_else(PoisonError::into_inner),
        }
    }
}

/// `applyAction` after the confirmation: each chat in ChatGPT, then in the
/// index. Archiving goes one chat at a time; deleting, three at once.
pub async fn apply(
    state: &Arc<State>,
    action: ChatAction,
    targets: Vec<Target>,
    session: SessionChoice,
    progress: Option<UnboundedSender<Progress>>,
) -> Result<Outcome, Failure> {
    let (doing, did) = match action {
        ChatAction::Archive => ("Archiving", "Archived"),
        ChatAction::Unarchive => ("Unarchiving", "Unarchived"),
        ChatAction::Delete => ("Deleting", "Deleted"),
        ChatAction::Unknown => {
            return Err(Failure::new(
                ErrorKind::Unsupported,
                "this daemon doesn't know that change; run `chatgpt daemon stop` and try again",
            ));
        }
    };
    let stop = Arc::new(AtomicBool::new(false));
    let _stop_on_drop = StopOnDrop(Arc::clone(&stop));
    let state = Arc::clone(state);
    let reporter = Reporter::for_client(progress);
    let api = Api::new(Arc::clone(&state.sessions), session);
    let task = tokio::spawn(async move {
        let _foreground = state.indexer.foreground();
        let step = reporter.step(doing, Some(targets.len()));
        let tally = Tally::default();
        let next = AtomicUsize::new(0);
        let workers = if action == ChatAction::Delete {
            DELETE_WORKERS.min(targets.len())
        } else {
            1
        };
        let worker = || async {
            while !stop.load(Ordering::SeqCst) {
                let Some(target) = targets.get(next.fetch_add(1, Ordering::SeqCst)) else {
                    break;
                };
                let result = perform(&state, &api, action, &target.id).await;
                tally.record(&step, target, result);
                tokio::time::sleep(DELAY).await;
            }
        };
        futures_util::future::join_all((0..workers).map(|_| worker())).await;
        step.finish(&format!(
            "{did} {} conversation(s){}",
            tally.done(),
            tally.failed_suffix()
        ));
        // Deleted chats' search chunks go with the next indexer run.
        state.indexer.wake();
        tally.into_outcome()
    });
    task.await.map_err(Failure::join)
}

/// `performAction`: one chat, ChatGPT first, then the index.
async fn perform(state: &State, api: &Api, action: ChatAction, id: &str) -> Result<(), ApiError> {
    let sent = match action {
        ChatAction::Delete => api.delete_conversation(id).await,
        _ => api.set_archived(id, action == ChatAction::Archive).await,
    };
    let delete = action == ChatAction::Delete;
    match sent {
        // The chat is gone: a repeated delete has reached its goal; an
        // archive change fails, and the index drops it either way.
        Err(error) if error.status == Some(404) => {
            remove(state, id).await?;
            if delete { Ok(()) } else { Err(error) }
        }
        Err(error) => Err(error),
        Ok(()) if delete => remove(state, id).await,
        Ok(()) => {
            let id = id.to_owned();
            let archived = action == ChatAction::Archive;
            state
                .db_write(move |db| chatgpt_store::set_archived(db, &id, archived))
                .await
                .map_err(ApiError::from)
        }
    }
}

async fn remove(state: &State, id: &str) -> Result<(), ApiError> {
    let id = id.to_owned();
    state
        .db_write(move |db| chatgpt_store::remove(db, &id))
        .await
        .map_err(ApiError::from)
}

/// `rename`: the chat in ChatGPT, then in the index (whose search chunks,
/// under the old title, the indexer rebuilds). The chat's id and its title
/// before, and when the index last synced.
pub async fn rename(
    state: &Arc<State>,
    reference: String,
    title: String,
    archived: bool,
    all: bool,
    session: SessionChoice,
) -> Result<(String, String, String), Failure> {
    let state = Arc::clone(state);
    // Its own task: once ChatGPT has the new title, the index gets it too.
    tokio::spawn(async move {
        let profile = state.profile();
        let (chat, synced_at) = state
            .db(move |db| Ok(select::one(db, &reference, archived, all, &profile)))
            .await??;
        let api = Api::new(Arc::clone(&state.sessions), session);
        if let Err(error) = api.rename(&chat.id, &title).await {
            // Observed 2026-09-27: legacy (pre-2025) chats answer 500 yet
            // the sidebar title changes, and only the full list reads it back.
            if error.status == Some(500) {
                return Err(Failure::new(ErrorKind::Api, RENAME_500));
            }
            return Err(error.into());
        }
        let id = chat.id.clone();
        state
            .db_write(move |db| chatgpt_store::rename(db, &id, &title))
            .await?;
        state.indexer.wake();
        Ok((chat.id, chat.title, synced_at))
    })
    .await
    .map_err(Failure::join)?
}

/// `title`: a manual local title, in the index only. The chat's id, and
/// when the index last synced.
pub async fn set_title(
    state: &State,
    reference: String,
    title: String,
    archived: bool,
    all: bool,
) -> Result<(String, String), Failure> {
    let profile = state.profile();
    // As `setLocalTitle` writes it: the public CLI's title version (a manual
    // title counts whatever its version).
    let version = crate::policy::Profile::builtin().local_title_version;
    let updated_at = crate::js::now_iso();
    state
        .db_write(move |db| {
            let chosen = select::one(db, &reference, archived, all, &profile);
            Ok(chosen.and_then(|(chat, synced_at)| {
                let clean = chatgpt_core::js::collapse_spaces(crate::js::trim(&title));
                if clean.is_empty() || clean.encode_utf16().count() > 100 {
                    return Err(Failure::new(
                        ErrorKind::InvalidInput,
                        "Local title must be 1–100 characters.",
                    ));
                }
                let manual = chatgpt_store::ManualTitle {
                    id: &chat.id,
                    update_time: &chat.update_time,
                    version,
                    title: &clean,
                    updated_at: &updated_at,
                };
                chatgpt_store::set_local_title(db, &manual).map_err(Failure::store)?;
                Ok((chat.id, synced_at))
            }))
        })
        .await?
}
