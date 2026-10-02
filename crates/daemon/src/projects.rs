//! `project add` and `project remove` after the confirmation: each chat's
//! project in ChatGPT, then in the index, one at a time, as the TS CLI's
//! `applyProjectAdd` and `applyProjectRemove` (`src/commands/projects.ts`
//! @ 1b8c950) do. The client resolved the project and the chats, skipped
//! the ones already where they're going, and showed the preview.
//!
//! `project delete` (new in the Rust CLI): the project in ChatGPT, then its
//! chats taken out of it in the index.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chatgpt_protocol::{Outcome, Progress, Project, SessionChoice, Target};
use tokio::sync::mpsc::UnboundedSender;

use crate::api::{Api, ApiError};
use crate::handlers::Failure;
use crate::mutate::{DELAY, StopOnDrop, Tally};
use crate::progress::Reporter;
use crate::state::State;

pub async fn move_chats(
    state: &Arc<State>,
    project: Project,
    targets: Vec<Target>,
    remove: bool,
    session: SessionChoice,
    progress: Option<UnboundedSender<Progress>>,
) -> Result<Outcome, Failure> {
    let stop = Arc::new(AtomicBool::new(false));
    let _stop_on_drop = StopOnDrop(Arc::clone(&stop));
    let api = crate::sync::pinned_api(state, session).await?;
    let state = Arc::clone(state);
    let reporter = Reporter::for_client(progress);
    // Its own task, so a client that goes away never leaves a chat moved in
    // ChatGPT but not in the index.
    let task = crate::progress::spawn(async move {
        let _no_pass = state.syncer.exclusive().await;
        let _foreground = state.indexer.foreground();
        let label = if remove {
            "Removing chats from project"
        } else {
            "Moving chats to project"
        };
        let step = reporter.step(label, Some(targets.len()));
        let tally = Tally::default();
        for (number, target) in targets.iter().enumerate() {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let result = move_one(&state, &api, &project, remove, &target.id).await;
            tally.record(&step, target, result);
            if number + 1 < targets.len() {
                tokio::time::sleep(DELAY).await;
            }
        }
        let summary = if remove {
            format!("Removed {} chat(s) from \"{}\"", tally.done(), project.name)
        } else {
            format!("Moved {} chat(s) to \"{}\"", tally.done(), project.name)
        };
        step.finish(&format!("{summary}{}", tally.failed_suffix()));
        tally.into_outcome()
    });
    task.await.map_err(Failure::join)
}

async fn move_one(
    state: &State,
    api: &Api,
    project: &Project,
    remove: bool,
    id: &str,
) -> Result<(), ApiError> {
    let to = if remove { "" } else { project.id.as_str() };
    api.set_project(id, to).await?;
    let id = id.to_owned();
    let project_id = (!remove).then(|| project.id.clone());
    state
        .db_write(move |db| chatgpt_store::set_project(db, &id, project_id.as_deref()))
        .await
        .map_err(ApiError::from)
}

/// `project delete` after the confirmation: the project in ChatGPT, then
/// `project_id` cleared for the index's chats in it. How many those were.
pub async fn delete(
    state: &Arc<State>,
    project: Project,
    session: SessionChoice,
) -> Result<usize, Failure> {
    let api = crate::sync::pinned_api(state, session).await?;
    let state = Arc::clone(state);
    // Its own task, as a move's: a client that goes away never leaves the
    // project deleted in ChatGPT but still named in the index. Under the
    // pass lock, so a pass that listed the chats before the delete can't
    // put the old project back.
    let task = crate::progress::spawn(async move {
        let _no_pass = state.syncer.exclusive().await;
        api.delete_project(&project.id).await?;
        let id = project.id;
        state
            .db_write(move |db| chatgpt_store::clear_project(db, &id))
            .await
            .map_err(ApiError::from)
    });
    Ok(task.await.map_err(Failure::join)??)
}
