//! `fullSync` from the TS CLI's `src/cli.ts` and `reconcileFullSyncOmissions`
//! from `src/index/reconcile-full-sync.ts` @ 1b8c950: every chat, active and
//! archived, replacing the index, so chats deleted in the browser drop out.
//! Only a 404 from the single-chat endpoint deletes a chat the lists left
//! out; any other failure stops the pass with the index untouched.

use chatgpt_core::ErrorKind;
use chatgpt_protocol::{SyncMode, SyncReport};
use chatgpt_store::{IndexFilter, IndexedConversation, NewConversation};
use serde_json::Value;

use super::{Listing, reconcile};
use crate::api::{Api, ApiError, ConversationSummary, PAGE_SIZE};
use crate::js;
use crate::state::State;

pub(super) async fn run(state: &State, api: &Api) -> Result<SyncReport, ApiError> {
    let started_at = js::now_iso();
    let mut listed = Listing::new();
    for archived in [false, true] {
        let kind = if archived { "archived" } else { "active" };
        let step = state.reporter.step(&format!("Listing {kind} chats"), None);
        let mut seen = 0;
        for offset in (0..).step_by(PAGE_SIZE) {
            let page = api.list_page(archived, offset).await?;
            let short = page.len() < PAGE_SIZE;
            for chat in page {
                if !listed.contains_key(&chat.id) {
                    listed.insert(chat.id.clone(), chat);
                }
                seen += 1;
                step.update(seen);
            }
            if short {
                break;
            }
        }
        step.finish(&format!("Listed {seen} {kind} chat(s)"));
    }
    // A chat updated during the sync jumps to the top, possibly past the
    // pages already read. Re-read the top until reaching chats older than
    // the start.
    for archived in [false, true] {
        'pages: for offset in (0..).step_by(PAGE_SIZE) {
            let page = api.list_page(archived, offset).await?;
            let short = page.len() < PAGE_SIZE;
            for chat in page {
                if chat.update_time < started_at {
                    break 'pages;
                }
                listed.insert(chat.id.clone(), chat);
            }
            if short {
                break;
            }
        }
    }

    let previous: Vec<IndexedConversation> = state
        .db(|db| {
            chatgpt_store::query(
                db,
                &IndexFilter {
                    include_pinned: true,
                    ..IndexFilter::default()
                },
                0,
            )
        })
        .await?;
    let recovered = recover_omissions(api, &mut listed, &previous).await?;
    if recovered > 0 {
        state.reporter.note(format!(
            "Recovered {recovered} chat(s) omitted from the conversation lists after individual checks."
        ));
    }

    let all: Vec<NewConversation> = listed.values().map(ConversationSummary::to_index).collect();
    let ids: Vec<String> = all.iter().map(|chat| chat.id.clone()).collect();
    let archived = all.iter().filter(|chat| chat.is_archived).count() as u64;
    let total = all.len() as u64;
    let before = state.db(chatgpt_store::count_all).await?;
    let synced_at = js::now_iso();
    state
        .db_write(move |db| chatgpt_store::replace_all(db, &all, &synced_at))
        .await?;
    let reconciled = reconcile::run(state, api, &ids).await?;
    Ok(SyncReport {
        mode: SyncMode::Full,
        swept: true,
        total,
        active: total - archived,
        archived,
        before,
        reconcile: Some(reconciled),
        ..SyncReport::default()
    })
}

/// Check every previously indexed chat the lists left out through the
/// single-chat endpoint: a 404 is a deletion, anything found goes back in.
async fn recover_omissions(
    api: &Api,
    listed: &mut Listing,
    previous: &[IndexedConversation],
) -> Result<u64, ApiError> {
    let mut recovered = 0;
    for old in previous {
        if listed.contains_key(&old.id) {
            continue;
        }
        let Some(chat) = api.conversation(&old.id).await? else {
            continue;
        };
        listed.insert(old.id.clone(), recovered_summary(old, &chat)?);
        recovered += 1;
    }
    Ok(recovered)
}

/// The list entry for a chat read individually. Fields the single-chat
/// answer leaves out keep their indexed values.
fn recovered_summary(
    old: &IndexedConversation,
    chat: &Value,
) -> Result<ConversationSummary, ApiError> {
    // `new Date(seconds * 1000).toISOString()`: null is 0, anything else
    // that isn't a number throws.
    let time = |name: &str| {
        let seconds = match chat.get(name) {
            Some(Value::Number(number)) => number.as_f64(),
            Some(Value::Null) => Some(0.0),
            _ => None,
        };
        seconds
            .and_then(js::iso_from_seconds)
            .ok_or_else(|| ApiError::new(ErrorKind::Decode, "Invalid time value"))
    };
    let optional = |name: &str, fallback: Option<String>| match chat.get(name) {
        None => fallback,
        Some(value) => value.as_str().map(str::to_owned),
    };
    Ok(ConversationSummary {
        id: old.id.clone(),
        title: chat.get("title").and_then(Value::as_str).map(str::to_owned),
        create_time: time("create_time")?,
        update_time: time("update_time")?,
        is_archived: chat
            .get("is_archived")
            .and_then(Value::as_bool)
            .unwrap_or(old.is_archived),
        pinned_time: optional("pinned_time", old.pinned.then(|| old.create_time.clone())),
        gizmo_id: optional("gizmo_id", old.project_id.clone()),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    // Ported from the TS CLI's src/index/reconcile-full-sync.test.ts.
    #[test]
    fn a_chat_read_individually_keeps_its_indexed_fields_where_the_answer_has_none() {
        let old = IndexedConversation {
            id: "omitted".into(),
            title: "omitted".into(),
            create_time: "2025-01-01T00:00:00.000Z".into(),
            update_time: "2025-01-02T00:00:00.000Z".into(),
            is_archived: true,
            pinned: true,
            project_id: Some("g-p-old".into()),
            local_title: None,
        };
        let detail = json!({
            "conversation_id": "omitted", "title": "omitted current", "create_time": 1735689600,
            "update_time": 1735776000, "is_archived": true, "pinned_time": null, "gizmo_id": "g-p-new",
            "mapping": {}, "current_node": ""
        });
        let recovered = recovered_summary(&old, &detail).expect("recovered");
        assert_eq!(recovered.title.as_deref(), Some("omitted current"));
        assert!(recovered.is_archived);
        assert_eq!(recovered.pinned_time, None);
        assert_eq!(recovered.gizmo_id.as_deref(), Some("g-p-new"));
        assert_eq!(recovered.update_time, "2025-01-02T00:00:00.000Z");

        let sparse = json!({ "title": "t", "create_time": 1735689600, "update_time": 1735776000 });
        let kept = recovered_summary(&old, &sparse).expect("recovered");
        assert_eq!(
            kept.pinned_time.as_deref(),
            Some("2025-01-01T00:00:00.000Z")
        );
        assert_eq!(kept.gizmo_id.as_deref(), Some("g-p-old"));
        assert!(kept.is_archived);
        assert!(recovered_summary(&old, &json!({ "title": "t" })).is_err());
    }
}
