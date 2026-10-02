//! Luna's review of the chats Jev can't settle: the TS CLI's
//! `classifyRemainingWithLuna` (`src/classify/luna-pipeline.ts` @
//! 1b8c950). It reviews chats still unsure after the follow-up, every
//! product-brainstorm label and borderline product idea, and conflicting
//! time-expired cases. Only `classify` runs it; never the background Jev.

use std::collections::HashMap;
use std::sync::Arc;

use chatgpt_store::{IndexedConversation, JudgmentRow, NewLunaJudgment};
use futures_util::StreamExt;
use serde_json::{Value, json};

use super::access::Access;
use super::pipeline::{CONCURRENCY, failure_line, is_long, profile, utc_today};
use super::summarise::SUMMARY_PROMPT_VERSION;
use crate::handlers::Failure;
use crate::policy::{Judged, Verdict};
use crate::progress::Reporter;
use crate::state::State;

const PROMPT: &str = include_str!("prompts/luna_review.txt");

const SUGGESTIONS: [&str; 3] = ["keep", "archive", "delete"];
const BRAINSTORMS: [&str; 4] = ["writing", "sermon", "product", "other"];
const PRODUCT_STAGES: [&str; 3] = [
    "new_concept_or_requirements",
    "execution_or_explanation",
    "not_applicable",
];

/// `SCHEMA` in luna-pipeline.ts, in its key order.
fn schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false,
        "required": ["suggestion", "brainstorm", "product_stage", "reason"],
        "properties": {
            "suggestion": { "type": "string", "enum": ["keep", "archive", "delete"] },
            "brainstorm": { "type": ["string", "null"], "enum": ["writing", "sermon", "product", "other", null] },
            "product_stage": { "type": "string", "enum": PRODUCT_STAGES },
            "reason": { "type": "string" },
        },
    })
}

/// `jevVerdictOf`'s object as `JSON.stringify` writes it: `deep` only
/// when the follow-up settled or left it.
fn verdict_json(verdict: &Verdict) -> Value {
    let mut value = json!({
        "suggestion": verdict.suggestion,
        "unsure": verdict.unsure,
        "reason": verdict.reason,
        "brainstorm": verdict.brainstorm,
    });
    if verdict.deep
        && let Some(object) = value.as_object_mut()
    {
        object.insert("deep".to_owned(), Value::Bool(true));
    }
    value
}

/// What one review needs, read before Luna is asked.
struct Review {
    chat: IndexedConversation,
    row: JudgmentRow,
    jev: Verdict,
    product_review: bool,
    time_review: bool,
    content: Result<(String, &'static str), String>,
}

/// `classifyRemainingWithLuna(targets, judgments, store, { force })`: the
/// reviewed chats' judgments are replaced in `judgments`. Its failures.
pub async fn review(
    state: &Arc<State>,
    reporter: &Reporter,
    access: &Access,
    targets: &[IndexedConversation],
    judgments: &mut HashMap<String, JudgmentRow>,
    force: bool,
) -> Result<Vec<String>, Failure> {
    let deep_version = &profile().deep_questions_version;
    let mut candidates = Vec::new();
    for chat in targets {
        let Some(row) = judgments.get(&chat.id) else {
            continue;
        };
        let judged = Judged::new(row, profile()).map_err(Failure::policy)?;
        let jev = judged.jev_verdict().map_err(Failure::policy)?;
        let deep_ready = row.deep_version.as_deref() == Some(deep_version.as_str());
        let product_review = judged.needs_product_review().map_err(Failure::policy)?;
        let time_review = judged.needs_time_review().map_err(Failure::policy)?;
        if !(deep_ready && jev.unsure)
            && !((product_review || time_review) && (!jev.unsure || deep_ready))
        {
            continue;
        }
        candidates.push((chat.clone(), row.clone(), jev, product_review, time_review));
    }
    let render = state.profile().render_version;
    let luna_version = profile().luna_version;
    let questions = profile().questions_version.clone();
    let lookups: Vec<(IndexedConversation, Option<String>)> = candidates
        .iter()
        .map(|(chat, row, ..)| (chat.clone(), row.deep_version.clone()))
        .collect();
    // Whether each is already reviewed, and its content.
    let looked_up = state
        .db(move |db| {
            let mut found = Vec::new();
            for (chat, deep) in &lookups {
                let reviewed = chatgpt_store::has_luna_judgment(
                    db,
                    &chat.id,
                    &chat.update_time,
                    &questions,
                    deep.as_deref().unwrap_or(""),
                    luna_version,
                )?;
                if reviewed && !force {
                    found.push(None);
                    continue;
                }
                let content =
                    match chatgpt_store::transcript(db, &chat.id, &chat.update_time, render)? {
                        None => Err("No cached transcript; rerun classify.".to_owned()),
                        Some(transcript) if is_long(&transcript) => chatgpt_store::summary(
                            db,
                            &chat.id,
                            &chat.update_time,
                            SUMMARY_PROMPT_VERSION,
                        )?
                        .map(|summary| (summary, "summary"))
                        .ok_or_else(|| {
                            "No current summary for long chat; rerun classify.".to_owned()
                        }),
                        Some(transcript) => Ok((transcript.markdown, "transcript")),
                    };
                found.push(Some(content));
            }
            Ok(found)
        })
        .await?;
    let todo: Vec<Review> = candidates
        .into_iter()
        .zip(looked_up)
        .filter_map(|(candidate, content)| Some((candidate, content?)))
        .map(
            |((chat, row, jev, product_review, time_review), content)| Review {
                chat,
                row,
                jev,
                product_review,
                time_review,
                content,
            },
        )
        .collect();
    reporter.note(format!("{} chat(s) need Luna's deeper review.", todo.len()));
    let step = reporter.step("Deeper Luna review", Some(todo.len()));
    let total = todo.len();
    let today = utc_today();
    let results: Vec<(IndexedConversation, Result<JudgmentRow, String>)> =
        futures_util::stream::iter(todo)
            .map(|review| {
                let (step, today) = (&step, &today);
                async move {
                    let reviewed = review_one(state, reporter, access, &review, today).await;
                    let count = step.advance(1);
                    step.update(count);
                    (review.chat, reviewed)
                }
            })
            .buffer_unordered(CONCURRENCY)
            .collect()
            .await;
    let mut failures = Vec::new();
    for (chat, reviewed) in results {
        match reviewed {
            Ok(row) => {
                judgments.insert(chat.id.clone(), row);
            }
            Err(why) => failures.push(failure_line(&chat, &why)),
        }
    }
    step.finish(&format!(
        "Luna reviewed {} chat(s){}",
        total - failures.len(),
        if failures.is_empty() {
            String::new()
        } else {
            format!(", {} failed", failures.len())
        }
    ));
    Ok(failures)
}

async fn review_one(
    state: &Arc<State>,
    reporter: &Reporter,
    access: &Access,
    review: &Review,
    today: &str,
) -> Result<JudgmentRow, String> {
    if reporter.client_gone() {
        return Err("not reviewed: the command was interrupted".to_owned());
    }
    let (content, content_kind) = review.content.as_ref().map_err(String::clone)?;
    let mut input = json!({
        "title": review.chat.title,
        "as_of": today,
        "content_kind": content_kind,
        "content": content,
        "product_review": review.product_review,
        "time_review": review.time_review,
    });
    if !review.product_review
        && let Some(object) = input.as_object_mut()
    {
        let parse = |text: &str| serde_json::from_str::<Value>(text).unwrap_or(Value::Null);
        object.insert("jev_verdict".to_owned(), verdict_json(&review.jev));
        object.insert("jev_answers".to_owned(), parse(&review.row.answers));
        object.insert(
            "deep_answers".to_owned(),
            review
                .row
                .deep_answers
                .as_deref()
                .filter(|answers| !answers.is_empty())
                .map_or(Value::Null, parse),
        );
    }
    let result = super::luna::ask(access, PROMPT, &input, &schema()).await?;
    let text = |name: &str| result.get(name).and_then(Value::as_str);
    let suggestion = text("suggestion").filter(|s| SUGGESTIONS.contains(s));
    let brainstorm = match result.get("brainstorm") {
        Some(Value::Null) => Some(None),
        Some(Value::String(kind)) if BRAINSTORMS.contains(&kind.as_str()) => {
            Some(Some(kind.as_str()))
        }
        _ => None,
    };
    let stage = text("product_stage").filter(|stage| PRODUCT_STAGES.contains(stage));
    let reason = text("reason")
        .map(chatgpt_core::js::trim)
        .filter(|r| !r.is_empty());
    let (Some(suggestion), Some(brainstorm), Some(stage), Some(reason)) =
        (suggestion, brainstorm, stage, reason)
    else {
        return Err("Luna returned an invalid verdict.".to_owned());
    };
    // The existing product rule protects brainstorms even when the final
    // reviewer disagrees with an uncertain kind.
    let reviewed = if brainstorm == Some("product") && stage != "new_concept_or_requirements" {
        None
    } else {
        brainstorm
    };
    let brainstorm = if review.product_review {
        reviewed.map(str::to_owned)
    } else {
        reviewed
            .map(str::to_owned)
            .or_else(|| review.jev.brainstorm.clone())
    };
    let suggestion = if brainstorm.is_some() {
        "keep"
    } else {
        suggestion
    };
    let row = NewLunaJudgment {
        id: review.chat.id.clone(),
        update_time: review.chat.update_time.clone(),
        questions_version: profile().questions_version.clone(),
        deep_version: review.row.deep_version.clone().unwrap_or_default(),
        version: profile().luna_version,
        suggestion: suggestion.to_owned(),
        brainstorm,
        reason: chatgpt_core::js::utf16_prefix(reason, 250).to_owned(),
        classified_at: crate::js::now_iso(),
    };
    let _no_pass = state.syncer.exclusive().await;
    state
        .db_write(move |db| {
            chatgpt_store::save_luna_judgment(db, &row)?;
            chatgpt_store::judgment(db, &row.id, &row.update_time, &row.questions_version)
        })
        .await
        .map_err(|failure| failure.message)?
        .ok_or_else(|| "the judgment it reviews is gone".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verdict_keeps_the_ts_key_order_and_drops_unset_flags() {
        let verdict = Verdict {
            suggestion: "keep".into(),
            unsure: true,
            reason: "r".into(),
            brainstorm: None,
            deep: false,
            luna: false,
        };
        assert_eq!(
            crate::js::stringify(&verdict_json(&verdict)),
            r#"{"suggestion":"keep","unsure":true,"reason":"r","brainstorm":null}"#
        );
        let deep = Verdict {
            deep: true,
            brainstorm: Some("other".into()),
            ..verdict
        };
        assert_eq!(
            crate::js::stringify(&verdict_json(&deep)),
            r#"{"suggestion":"keep","unsure":true,"reason":"r","brainstorm":"other","deep":true}"#
        );
    }
}
