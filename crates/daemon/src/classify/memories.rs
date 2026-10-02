//! `memory classify`: the TS CLI's `classifyMemories`
//! (`src/classify/memories.ts` @ 1b8c950). Jev's quick pass over each saved
//! memory that changed (its text, its related memories or the month), then
//! Luna's review of the ones Jev couldn't keep, eight at a time, retried
//! one by one when a batch fails. A result counts only for the exact input
//! it was made for. Never deletes anything.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use chatgpt_protocol::{ClassifiedMemories, SessionChoice};
use chatgpt_store::NewMemoryJudgment;
use futures_util::StreamExt;
use serde_json::{Value, json};

use super::access::Access;
use super::pipeline::{CONCURRENCY, profile, utc_today};
use crate::handlers::Failure;
use crate::policy::memory::{SavedMemory, decide, input_hash, related_indexes};
use crate::progress::Reporter;
use crate::state::State;

const DEEP_PROMPT: &str = include_str!("prompts/memory_review.txt");
const DEEP_BATCH: usize = 8;

/// `DEEP_SCHEMA` in memories.ts, in its key order.
fn deep_schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false, "required": ["decisions"], "properties": {
            "decisions": { "type": "array", "items": { "type": "object", "additionalProperties": false,
                "required": ["id", "suggestion", "reason"], "properties": {
                    "id": { "type": "string" }, "suggestion": { "type": "string", "enum": ["keep", "delete", "review"] }, "reason": { "type": "string" },
                } } },
        },
    })
}

/// A cached or new classification.
#[derive(Clone)]
struct Row {
    hash: String,
    system_one: String,
    system_two: Option<String>,
    classified_at: String,
}

/// `validateDeepAnswers`: one decision per id, in order.
fn validate_deep(ids: &[&str], result: &Value) -> Result<Vec<Value>, String> {
    let invalid = || "Luna returned incomplete or invalid memory decisions.".to_owned();
    let decisions = result
        .get("decisions")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    let fits = decisions.len() == ids.len()
        && decisions.iter().zip(ids).all(|(decision, id)| {
            decision.get("id").and_then(Value::as_str) == Some(*id)
                && decision
                    .get("suggestion")
                    .and_then(Value::as_str)
                    .is_some_and(|s| ["keep", "delete", "review"].contains(&s))
                && decision
                    .get("reason")
                    .and_then(Value::as_str)
                    .is_some_and(|reason| !chatgpt_core::js::trim(reason).is_empty())
        });
    if fits {
        Ok(decisions.clone())
    } else {
        Err(invalid())
    }
}

pub async fn classify(
    state: &Arc<State>,
    reporter: &Reporter,
    access: &Access,
    session: SessionChoice,
    redo: bool,
) -> Result<ClassifiedMemories, Failure> {
    let api = crate::sync::pinned_api(state, session).await?;
    let objects = api.memory_objects().await?;
    let memories: Vec<SavedMemory> = objects
        .iter()
        .map(|object| serde_json::from_value(object.clone()))
        .collect::<Result<_, _>>()
        .map_err(|_| {
            Failure::new(
                chatgpt_core::ErrorKind::Decode,
                "ChatGPT returned an unexpected saved-memory list.",
            )
        })?;
    let as_of = utc_today();
    let related = related_indexes(&memories);
    let hashes: Vec<String> = memories
        .iter()
        .zip(&related)
        .map(|(memory, related)| {
            let others: Vec<&SavedMemory> = related.iter().map(|&at| &memories[at]).collect();
            input_hash(memory, &others, &as_of)
        })
        .collect();
    let version = profile().memory_version.clone();
    let lookups: Vec<(String, String)> = memories
        .iter()
        .zip(&hashes)
        .map(|(memory, hash)| (memory.id.clone(), hash.clone()))
        .collect();
    let cached: HashMap<String, Row> = if redo {
        HashMap::new()
    } else {
        state
            .db(move |db| {
                let mut found = HashMap::new();
                for (id, hash) in &lookups {
                    if let Some(row) = chatgpt_store::memory_judgment(db, id, hash, &version)? {
                        found.insert(
                            id.clone(),
                            Row {
                                hash: hash.clone(),
                                system_one: row.system_one,
                                system_two: row.system_two,
                                classified_at: String::new(),
                            },
                        );
                    }
                }
                Ok(found)
            })
            .await?
    };
    let results = Mutex::new(cached);
    let todo: Vec<usize> = (0..memories.len())
        .filter(|&at| {
            !results
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains_key(&memories[at].id)
        })
        .collect();
    reporter.note(format!(
        "{} saved memories: {} cached, {} new or changed.",
        memories.len(),
        memories.len() - todo.len(),
        todo.len()
    ));
    let failures = Mutex::new(Vec::new());
    let related_json = |at: usize| -> Value {
        Value::Array(
            related[at]
                .iter()
                .map(|&other| json!({ "id": memories[other].id, "content": memories[other].content }))
                .collect(),
        )
    };
    let client = access.jev();
    let step = reporter.step("Quick memory classification", Some(todo.len()));
    let done = Mutex::new(0usize);
    futures_util::stream::iter(todo.iter().copied())
        .for_each_concurrent(CONCURRENCY, |at| {
            let (step, done, client, results, failures, hashes, as_of) =
                (&step, &done, &client, &results, &failures, &hashes, &as_of);
            let memory = &memories[at];
            let related = related_json(at);
            async move {
                let quick = async {
                    if reporter.client_gone() {
                        return Err("not classified: the command was interrupted".to_owned());
                    }
                    let client = client.as_ref().map_err(String::clone)?;
                    let mut entry = json!({ "id": memory.id, "content": memory.content });
                    if let (Some(updated), Some(object)) =
                        (&memory.updated_at, entry.as_object_mut())
                    {
                        object.insert("updated_at".to_owned(), Value::String(updated.clone()));
                    }
                    let quick_state = json!({
                        "as_of": as_of,
                        "memory": entry,
                        "related_memories": related,
                    });
                    let answer = client
                        .system_one(&quick_state, super::questions::memories())
                        .await
                        .map_err(|error| error.to_string())?;
                    super::questions::validate(super::questions::memories(), &answer.answers)?;
                    let row = Row {
                        hash: hashes[at].clone(),
                        system_one: crate::js::stringify(&answer.answers),
                        system_two: None,
                        classified_at: crate::js::now_iso(),
                    };
                    save(state, &memory.id, &row).await?;
                    Ok(row)
                };
                match quick.await {
                    Ok(row) => {
                        results
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .insert(memory.id.clone(), row);
                    }
                    Err(why) => failures
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(format!("{}: {why}", memory.id)),
                }
                let count = {
                    let mut done = done.lock().unwrap_or_else(PoisonError::into_inner);
                    *done += 1;
                    *done
                };
                step.update(count);
            }
        })
        .await;
    let quick_failed = failures
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .len();
    step.finish(&format!(
        "Quick-classified {} saved memories",
        todo.len() - quick_failed
    ));
    let mut results = results.into_inner().unwrap_or_else(PoisonError::into_inner);
    let mut failures = failures
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner);

    let review: Vec<usize> = (0..memories.len())
        .filter(|&at| {
            results.get(&memories[at].id).is_some_and(|row| {
                row.system_two.is_none()
                    && decide(&row.system_one, None).is_ok_and(|d| d.suggestion == "review")
            })
        })
        .collect();
    reporter.note(format!(
        "{} saved memories need Luna's deeper review.",
        review.len()
    ));
    let deep_input = |batch: &[usize], results: &HashMap<String, Row>| -> Value {
        json!({
            "as_of": as_of,
            "memories": batch.iter().map(|&at| {
                let memory = &memories[at];
                let quick = results
                    .get(&memory.id)
                    .and_then(|row| serde_json::from_str::<Value>(&row.system_one).ok())
                    .unwrap_or(Value::Null);
                let mut entry = json!({ "id": memory.id, "content": memory.content });
                if let (Some(updated), Some(object)) = (&memory.updated_at, entry.as_object_mut()) {
                    object.insert("updated_at".to_owned(), Value::String(updated.clone()));
                }
                if let Some(object) = entry.as_object_mut() {
                    object.insert("related_memories".to_owned(), related_json(at));
                    object.insert("quick_answers".to_owned(), quick);
                }
                entry
            }).collect::<Vec<_>>(),
        })
    };
    for (number, batch) in review.chunks(DEEP_BATCH).enumerate() {
        let ids: Vec<&str> = batch.iter().map(|&at| memories[at].id.as_str()).collect();
        let answered = async {
            if reporter.client_gone() {
                return Err("not reviewed: the command was interrupted".to_owned());
            }
            let result = super::luna::ask(
                access,
                DEEP_PROMPT,
                &deep_input(batch, &results),
                &deep_schema(),
            )
            .await?;
            validate_deep(&ids, &result)
        }
        .await;
        match answered {
            Ok(decisions) => {
                save_deep(state, &mut results, &decisions).await?;
                reporter.note(format!(
                    "Luna reviewed {}/{} memories.",
                    (number * DEEP_BATCH + batch.len()).min(review.len()),
                    review.len()
                ));
            }
            Err(why) => {
                if batch.len() > 1 {
                    reporter.note(format!(
                        "Luna batch failed ({why}); retrying {} memories individually.",
                        batch.len()
                    ));
                }
                for &at in batch {
                    let id = memories[at].id.as_str();
                    let retried = async {
                        if reporter.client_gone() {
                            return Err("not reviewed: the command was interrupted".to_owned());
                        }
                        let result = super::luna::ask(
                            access,
                            DEEP_PROMPT,
                            &deep_input(&[at], &results),
                            &deep_schema(),
                        )
                        .await?;
                        validate_deep(&[id], &result)
                    }
                    .await;
                    match retried {
                        Ok(decisions) => save_deep(state, &mut results, &decisions).await?,
                        Err(why) => failures.push(format!("{id}: {why}")),
                    }
                }
            }
        }
    }
    let mut rows = Vec::new();
    for (object, (memory, related)) in objects.into_iter().zip(memories.iter().zip(&related)) {
        let Some(row) = results.get(&memory.id) else {
            continue;
        };
        let decision = decide(&row.system_one, row.system_two.as_deref()).map_err(|why| {
            Failure::new(
                chatgpt_core::ErrorKind::Internal,
                format!("saved memory {}: {why}", memory.id),
            )
        })?;
        let Value::Object(mut fields) = object else {
            continue;
        };
        fields.insert("suggestion".into(), Value::String(decision.suggestion));
        fields.insert("reason".into(), Value::String(decision.reason));
        fields.insert("stage".into(), Value::String(decision.stage.to_owned()));
        fields.insert(
            "related_ids".into(),
            Value::Array(
                related
                    .iter()
                    .map(|&at| Value::String(memories[at].id.clone()))
                    .collect(),
            ),
        );
        rows.push(Value::Object(fields));
    }
    Ok(ClassifiedMemories { rows, failures })
}

async fn save(state: &State, id: &str, row: &Row) -> Result<(), String> {
    let new = NewMemoryJudgment {
        id: id.to_owned(),
        input_hash: row.hash.clone(),
        version: profile().memory_version.clone(),
        system_one: row.system_one.clone(),
        system_two: row.system_two.clone(),
        classified_at: if row.classified_at.is_empty() {
            crate::js::now_iso()
        } else {
            row.classified_at.clone()
        },
    };
    state
        .db_write(move |db| chatgpt_store::save_memory_judgment(db, &new))
        .await
        .map_err(|failure| failure.message)
}

/// `saveDeep`: Luna's decision joins each memory's row.
async fn save_deep(
    state: &State,
    results: &mut HashMap<String, Row>,
    decisions: &[Value],
) -> Result<(), Failure> {
    for decision in decisions {
        let Some(id) = decision.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(row) = results.get_mut(id) else {
            continue;
        };
        row.system_two = Some(crate::js::stringify(decision));
        save(state, id, row)
            .await
            .map_err(|message| Failure::new(chatgpt_core::ErrorKind::Internal, message))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_decisions_must_answer_every_id_in_order() {
        let good = json!({ "decisions": [
            { "id": "a", "suggestion": "keep", "reason": "x" },
            { "id": "b", "suggestion": "delete", "reason": "y" },
        ] });
        assert!(validate_deep(&["a", "b"], &good).is_ok());
        assert!(validate_deep(&["b", "a"], &good).is_err());
        assert!(validate_deep(&["a"], &good).is_err());
        let blank = json!({ "decisions": [{ "id": "a", "suggestion": "keep", "reason": "  " }] });
        assert!(validate_deep(&["a"], &blank).is_err());
        let odd = json!({ "decisions": [{ "id": "a", "suggestion": "burn", "reason": "z" }] });
        assert!(validate_deep(&["a"], &odd).is_err());
    }
}
