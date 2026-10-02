//! `memory delete` after the confirmation: each saved memory in turn, a
//! quarter second apart, as the TS CLI's `deleteSelectedMemories`
//! (`src/commands/memories.ts` @ 1b8c950) does. The client resolved the ids
//! against the live list and showed the preview.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chatgpt_protocol::{Outcome, SessionChoice};

use crate::api::Api;
use crate::handlers::Failure;
use crate::mutate::{DELAY, StopOnDrop};
use crate::state::State;

pub async fn delete(
    state: &Arc<State>,
    ids: Vec<String>,
    session: SessionChoice,
) -> Result<Outcome, Failure> {
    let stop = Arc::new(AtomicBool::new(false));
    let _stop_on_drop = StopOnDrop(Arc::clone(&stop));
    let api = Api::new(Arc::clone(&state.sessions), session);
    let task = tokio::spawn(async move {
        let mut outcome = Outcome::default();
        for (number, id) in ids.iter().enumerate() {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            match api.delete_memory(id).await {
                Ok(()) => outcome.done += 1,
                Err(error) => outcome.failures.push(format!("{id}: {}", error.message)),
            }
            if number + 1 < ids.len() {
                tokio::time::sleep(DELAY).await;
            }
        }
        outcome
    });
    task.await.map_err(Failure::join)
}
