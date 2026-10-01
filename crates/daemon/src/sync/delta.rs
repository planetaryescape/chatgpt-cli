//! `deltaSync` from the TS CLI's `src/cli.ts` @ 1b8c950: new and changed
//! chats from the top of the active list down to the watermark, then (when
//! `sweep`) the whole archived list, a batch check of every chat that left
//! it, and the cache reconcile.

use std::collections::HashSet;

use chatgpt_protocol::{SyncMode, SyncReport};
use chatgpt_store::NewConversation;

use super::{Listing, reconcile};
use crate::api::{ApiError, BATCH_MAX, PAGE_SIZE};
use crate::js;
use crate::state::State;

pub(super) async fn run(
    state: &State,
    watermark: &str,
    sweep: bool,
) -> Result<SyncReport, ApiError> {
    let known: HashSet<String> = state
        .db(chatgpt_store::all_ids)
        .await?
        .into_iter()
        .collect();

    let step = state
        .reporter
        .step("Checking for new and updated chats", None);
    let mut changed = Listing::new();
    'pages: for offset in (0..).step_by(PAGE_SIZE) {
        let page = state.api.list_page(false, offset).await?;
        let short = page.len() < PAGE_SIZE;
        for chat in page {
            if chat.update_time.as_str() <= watermark {
                break 'pages;
            }
            changed.insert(chat.id.clone(), chat);
            step.update(changed.len());
        }
        if short {
            break;
        }
    }
    let added = changed.keys().filter(|id| !known.contains(*id)).count();
    step.finish(&format!("{added} new, {} updated", changed.len() - added));
    let changed_rows: Vec<NewConversation> = changed.values().map(|chat| chat.to_index()).collect();
    let mut report = SyncReport {
        mode: SyncMode::Delta,
        added: added as u64,
        updated: (changed.len() - added) as u64,
        swept: sweep,
        ..SyncReport::default()
    };

    if !sweep {
        let synced_at = js::now_iso();
        state
            .db_write(move |db| chatgpt_store::apply_delta(db, &changed_rows, &[], &synced_at))
            .await?;
        return Ok(report);
    }

    let archived_step = state.reporter.step("Reading archived chats", None);
    let mut archived = Listing::new();
    for offset in (0..).step_by(PAGE_SIZE) {
        let page = state.api.list_page(true, offset).await?;
        let short = page.len() < PAGE_SIZE;
        for chat in page {
            archived.insert(chat.id.clone(), chat);
            archived_step.update(archived.len());
        }
        if short {
            break;
        }
    }
    archived_step.finish(&format!("{} archived", archived.len()));

    // Chats that left the archived list were unarchived or deleted. The list
    // has once come back short right after archive changes, so confirm each.
    let was_archived: Vec<String> = state.db(chatgpt_store::archived_ids).await?;
    let was_set: HashSet<&str> = was_archived.iter().map(String::as_str).collect();
    let dropped: Vec<String> = was_archived
        .iter()
        .filter(|id| !archived.contains_key(id.as_str()))
        .cloned()
        .collect();
    report.newly_archived = archived
        .keys()
        .filter(|id| !was_set.contains(id.as_str()))
        .count() as u64;
    let archived_rows: Vec<NewConversation> =
        archived.values().map(|chat| chat.to_index()).collect();
    let synced_at = js::now_iso();
    state
        .db_write(move |db| {
            chatgpt_store::apply_delta(db, &changed_rows, &archived_rows, &synced_at)
        })
        .await?;

    for ids in dropped.chunks(BATCH_MAX) {
        let found = state.api.batch(ids).await?;
        for id in ids {
            match found.iter().find(|item| &item.id == id) {
                None => {
                    let id = id.clone();
                    state
                        .db_write(move |db| chatgpt_store::remove(db, &id))
                        .await?;
                    report.deleted += 1;
                }
                Some(item) if !item.is_archived.unwrap_or(false) => {
                    let id = id.clone();
                    state
                        .db_write(move |db| chatgpt_store::set_archived(db, &id, false))
                        .await?;
                    report.unarchived += 1;
                }
                Some(_) => {}
            }
        }
    }

    let all_ids = state.db(chatgpt_store::all_ids).await?;
    report.reconcile = Some(reconcile::run(state, &all_ids).await);
    Ok(report)
}
