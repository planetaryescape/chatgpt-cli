//! `reconcileMetadataChanges` from the TS CLI's
//! `src/index/reconcile-metadata.ts` @ 1b8c950. ChatGPT changes
//! `update_time` for many project moves without changing the chat, which
//! would make every cached transcript, judgment, follow-up, Luna review,
//! summary and local title look stale. For each chat whose cached transcript
//! is for an older `update_time`, the batch endpoint fetches the chat and a
//! fresh render is compared with the cached one; only an unchanged title and
//! body move the caches forward.

use std::collections::HashMap;
use std::time::Duration;

use chatgpt_protocol::ReconcileReport;
use chatgpt_store::Candidate;

use crate::api::{Api, ApiError, BATCH_MAX, BatchItem};
use crate::render::{SEPARATOR, render_batch_item, visible_turns};
use crate::state::State;

/// A chat that can't be checked is a failure in the report and stays
/// stale. A rate limit ends the whole pass instead, so the backoff is
/// recorded and nothing more is sent.
pub(super) async fn run(
    state: &State,
    api: &Api,
    ids: &[String],
) -> Result<ReconcileReport, ApiError> {
    let mut report = ReconcileReport::default();
    let render_version = state.profile().render_version;
    let ids = ids.to_vec();
    let candidates = match state
        .db(move |db| chatgpt_store::candidates(db, &ids, render_version))
        .await
    {
        Ok(candidates) => candidates,
        Err(failure) => {
            report
                .failures
                .push(format!("reading the cache: {}", failure.message));
            return Ok(report);
        }
    };
    if candidates.is_empty() {
        return Ok(report);
    }
    let step = state
        .reporter
        .step("Checking changed chat content", Some(candidates.len()));
    let batches: Vec<&[Candidate]> = candidates.chunks(BATCH_MAX).collect();
    for (index, batch) in batches.iter().enumerate() {
        let ids: Vec<String> = batch.iter().map(|candidate| candidate.id.clone()).collect();
        let done = (index * BATCH_MAX + batch.len()).min(candidates.len());
        let fetched: HashMap<String, BatchItem> = match api.batch(&ids).await {
            Ok(items) => items
                .into_iter()
                .map(|item| (item.id.clone(), item))
                .collect(),
            Err(error) if error.is_rate_limit() => return Err(error),
            Err(error) => {
                report
                    .failures
                    .extend(ids.iter().map(|id| format!("{id}: {}", error.message)));
                step.update(done);
                continue;
            }
        };
        for candidate in *batch {
            let Some(item) = fetched.get(&candidate.id) else {
                report.failures.push(format!(
                    "{}: ChatGPT did not return the chat.",
                    candidate.id
                ));
                continue;
            };
            match same_content(candidate, item) {
                Err(why) => report.failures.push(format!("{}: {why}", candidate.id)),
                Ok(false) => report.changed += 1,
                Ok(true) => {
                    let id = candidate.id.clone();
                    let candidate = candidate.clone();
                    match state
                        .db_write(move |db| chatgpt_store::preserve(db, &candidate))
                        .await
                    {
                        Ok(true) => report.preserved += 1,
                        Ok(false) => report.changed += 1,
                        Err(failure) => report.failures.push(format!("{id}: {}", failure.message)),
                    }
                }
            }
        }
        step.update(done);
        if index + 1 < batches.len() {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    let failed = if report.failures.is_empty() {
        String::new()
    } else {
        format!(", {} failed", report.failures.len())
    };
    step.finish(&format!(
        "Preserved {} unchanged cache(s); {} content or metadata change(s) left stale{failed}",
        report.preserved, report.changed
    ));
    for failure in &report.failures {
        tracing::warn!("reconcile failed: {failure}");
    }
    Ok(report)
}

/// `sameContent`. The batch endpoint returns old or rounded update times
/// for some legacy chats, so only the title and the rendered body count.
fn same_content(candidate: &Candidate, item: &BatchItem) -> Result<bool, String> {
    if item.conversation.title != candidate.title {
        return Ok(false);
    }
    let date_prefix: String = candidate.create_time.chars().take(10).collect();
    let header = format!(
        "# {}\n\nhttps://chatgpt.com/c/{} · {date_prefix} · ",
        candidate.title, candidate.id
    );
    if !candidate.markdown.starts_with(&header) {
        return Ok(false);
    }
    let fresh = render_batch_item(item)?;
    let fresh_turns = visible_turns(&item.conversation)?.len();
    if i64::try_from(fresh_turns).ok() != Some(candidate.turns) {
        return Ok(false);
    }
    Ok(
        match (candidate.markdown.find(SEPARATOR), fresh.find(SEPARATOR)) {
            (Some(old), Some(new)) => candidate.markdown[old..] == fresh[new..],
            (old, new) => old.is_none() && new.is_none(),
        },
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const ID: &str = "chat-one";

    fn item(text: &str, title: &str) -> BatchItem {
        serde_json::from_value(json!({
            "id": ID, "title": title, "create_time": "2026-09-01T00:00:00.000Z",
            "update_time": "2026-09-28T00:00:00.000Z",
            "mapping": { "message": { "id": "message", "parent": null, "children": [],
                "message": { "author": { "role": "user" }, "create_time": 1,
                    "content": { "content_type": "text", "parts": [text] } } } },
            "current_node": "message"
        }))
        .expect("batch item")
    }

    fn cached(text: &str) -> Candidate {
        let original = item(text, "Idea");
        Candidate {
            id: ID.into(),
            title: "Idea".into(),
            create_time: "2026-09-01T00:00:00.000Z".into(),
            update_time: "2026-09-28T00:00:00.000Z".into(),
            cached_update_time: "2026-09-01T00:00:00.000Z".into(),
            // The TS CLI cached it from the single-chat endpoint, which
            // names the model; the batch doesn't. Only the body counts.
            markdown: crate::render::render_transcript(ID, &original.conversation, "2026-09-01")
                .expect("render")
                .replace("unknown model", "gpt-4"),
            turns: 1,
        }
    }

    // Ported from the TS CLI's src/index/reconcile-metadata.test.ts.
    #[test]
    fn a_metadata_only_change_keeps_the_caches_and_an_edit_does_not() {
        let candidate = cached("A product idea worth developing.");
        assert_eq!(
            same_content(
                &candidate,
                &item("A product idea worth developing.", "Idea")
            ),
            Ok(true)
        );
        assert_eq!(
            same_content(&candidate, &item("A changed idea.", "Idea")),
            Ok(false)
        );
        assert_eq!(
            same_content(
                &candidate,
                &item("A product idea worth developing.", "Renamed")
            ),
            Ok(false)
        );
    }
}
