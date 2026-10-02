//! The Jev guard behind `archive`/`delete --check` and `--suggest <action>`:
//! the TS CLI's `checkWithJev` (`src/commands/check.ts` @ 1b8c950) over
//! `Classifier.classify` (`crate::classify::pipeline`). Chats without a
//! current judgment are judged now, with the same steps, notes, summaries
//! (and the question before a large batch of them) and cost lines, and
//! only the ones Jev confidently backs the action for are approved.
//! Anything it can't judge is held back, never approved, and so is a chat
//! that changed while it was being judged (a sync pass ran meanwhile).
//!
//! Judgments use this repository's questions and policy (D9). Live calls
//! happen only here, while the command runs, never in the background.

use std::collections::HashMap;
use std::sync::Arc;

use chatgpt_core::ErrorKind;
use chatgpt_protocol::{ChatAction, ModelAccess, Progress, Secret, SessionChoice};
use chatgpt_store::IndexedConversation;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::classify::Access;
use crate::classify::pipeline::{Classifier, Options, profile};
use crate::handlers::Failure;
use crate::policy::Judged;
use crate::progress::{Asker, Reporter};
use crate::state::State;

/// `checkWithJev`: the ids of `ids` Jev backs `action` for, in order.
#[allow(clippy::too_many_arguments, reason = "one request's fields")]
pub async fn check(
    state: &Arc<State>,
    action: ChatAction,
    ids: Vec<String>,
    api_key: Option<Secret>,
    mut access: ModelAccess,
    yes: bool,
    session: SessionChoice,
    progress: Option<UnboundedSender<Progress>>,
    answers: Option<UnboundedReceiver<bool>>,
) -> Result<Vec<String>, Failure> {
    let action = match action {
        ChatAction::Archive | ChatAction::Delete => action.as_str(),
        _ => {
            return Err(Failure::new(
                ErrorKind::InvalidInput,
                "--check applies to archive and delete only.",
            ));
        }
    };
    // A client from before model access passed only its Jev key.
    if access.typesafe.is_none() {
        access.typesafe = api_key;
    }
    let state = Arc::clone(state);
    // Its own task: a judgment paid for is saved even if the client leaves.
    crate::progress::spawn(async move {
        let reporter = Reporter::for_client(progress);
        let asker = answers.map(|answers| Asker::new(reporter.clone(), answers));
        let access = Access::for_client(access);
        let _claim = state.flight.claim(&ids).await;
        let targets = crate::classify::chats(&state, ids.clone()).await?;
        let chats: HashMap<String, IndexedConversation> = targets
            .iter()
            .map(|chat| (chat.id.clone(), chat.clone()))
            .collect();
        let classifier = Classifier {
            state: &state,
            reporter: &reporter,
            session,
            access: &access,
            asker: asker.as_ref(),
        };
        let options = Options {
            force: false,
            yes,
            summarise: true,
        };
        let classified = classifier.classify(&targets, options).await?;
        // A pass that changed a chat after it was judged leaves the
        // verdict resting on old content.
        let now = crate::classify::chats(&state, ids.clone()).await?;
        let changed = |id: &str, judged_at: &str| {
            now.iter()
                .find(|chat| chat.id == id)
                .is_none_or(|chat| chat.update_time != judged_at)
        };
        let mut approved = Vec::new();
        let mut held = Vec::new();
        for id in &ids {
            let Some(chat) = chats.get(id) else {
                held.push(format!("  not judged  {id}"));
                continue;
            };
            let Some(row) = classified
                .judgments
                .get(id)
                .filter(|row| !changed(id, &row.update_time))
            else {
                held.push(format!("  not judged  {}", chat.display_title()));
                continue;
            };
            let verdict = Judged::new(row, profile())
                .and_then(|judged| judged.verdict())
                .map_err(Failure::policy)?;
            if verdict.backs(action == "delete") {
                approved.push(id.clone());
            } else {
                let suggestion = format!(
                    "{}{}",
                    verdict.suggestion,
                    if verdict.unsure { "?" } else { "" }
                );
                held.push(format!(
                    "  {suggestion:<10}  {}  ({})",
                    chat.display_title(),
                    verdict.reason
                ));
            }
        }
        for failure in &classified.failures {
            reporter.note(format!("failed: {failure}"));
        }
        reporter.note(format!(
            "Classification backs {action} for {} of {}.",
            approved.len(),
            ids.len()
        ));
        if !held.is_empty() {
            reporter.note(format!(
                "Held back (suggestion, then the title):\n{}",
                held.join("\n")
            ));
        }
        tracing::info!(
            action,
            chats = ids.len(),
            approved = approved.len(),
            failed = classified.failures.len(),
            "Jev guard"
        );
        Ok(approved)
    })
    .await
    .map_err(Failure::join)?
}
