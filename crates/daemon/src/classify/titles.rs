//! Local display titles and topic themes from Luna: the TS CLI's
//! `generateLocalTitles` (`src/classify/titles.ts` @ 1b8c950). It reads the
//! cached transcript (or a long chat's summary), never ChatGPT, asks Luna
//! about up to twelve chats at once, and splits a batch that fails in half
//! until each chat succeeds or fails alone. Manual titles are never
//! replaced.

use std::sync::{Arc, Mutex, PoisonError};

use chatgpt_store::{IndexedConversation, ManualTitle};
use futures_util::StreamExt;
use serde_json::{Value, json};

use super::access::Access;
use super::pipeline::{failure_line, is_long};
use super::summarise::SUMMARY_PROMPT_VERSION;
use crate::handlers::Failure;
use crate::progress::Reporter;
use crate::state::State;

const PROMPT: &str = include_str!("prompts/titles.txt");
/// Batches run three at a time.
const CONCURRENCY: usize = 3;
const BATCH_CHATS: usize = 12;
const BATCH_TOKENS: i64 = 22_000;

/// `SCHEMA` in titles.ts, in its key order.
fn schema() -> Value {
    json!({
        "type": "object", "additionalProperties": false, "required": ["items"],
        "properties": { "items": { "type": "array", "items": { "type": "object", "additionalProperties": false,
            "required": ["id", "title", "theme"], "properties": {
                "id": { "type": "string" }, "title": { "type": "string" }, "theme": { "type": "string" },
            } } } },
    })
}

#[derive(Clone)]
struct Item {
    chat: IndexedConversation,
    input: Value,
    tokens: i64,
}

/// `batches`: at most twelve chats and 22k tokens each, a chat too big
/// for one on its own.
fn batches(items: Vec<Item>) -> Vec<Vec<Item>> {
    let mut result = Vec::new();
    let mut batch: Vec<Item> = Vec::new();
    let mut tokens = 0;
    for item in items {
        if !batch.is_empty() && (batch.len() >= BATCH_CHATS || tokens + item.tokens > BATCH_TOKENS)
        {
            result.push(std::mem::take(&mut batch));
            tokens = 0;
        }
        tokens += item.tokens;
        batch.push(item);
    }
    if !batch.is_empty() {
        result.push(batch);
    }
    result
}

struct Run<'a> {
    state: &'a Arc<State>,
    reporter: &'a Reporter,
    access: &'a Access,
    generated: Mutex<usize>,
    failures: Mutex<Vec<String>>,
}

/// `generateLocalTitles(targets, index, store, { redo })`: its failures.
pub async fn generate(
    state: &Arc<State>,
    reporter: &Reporter,
    access: &Access,
    targets: &[IndexedConversation],
    redo: bool,
) -> Result<Vec<String>, Failure> {
    let version = super::pipeline::profile().local_title_version;
    let render = state.profile().render_version;
    let chats = targets.to_vec();
    // Whether each target keeps its title (a manual one, or without
    // `redo` a current Luna one); else its transcript and (long) summary.
    let looked_up = state
        .db(move |db| {
            let mut found = Vec::new();
            for chat in chats {
                let source =
                    chatgpt_store::local_title_source(db, &chat.id, &chat.update_time, version)?;
                if source.as_deref() == Some("manual") || (!redo && source.is_some()) {
                    found.push((chat, true, None, None));
                    continue;
                }
                let transcript =
                    chatgpt_store::transcript(db, &chat.id, &chat.update_time, render)?;
                let summary = match &transcript {
                    Some(transcript) if is_long(transcript) => chatgpt_store::summary(
                        db,
                        &chat.id,
                        &chat.update_time,
                        SUMMARY_PROMPT_VERSION,
                    )?,
                    _ => None,
                };
                found.push((chat, false, transcript, summary));
            }
            Ok(found)
        })
        .await?;
    let mut failures = Vec::new();
    let mut ready = Vec::new();
    let mut pending = 0;
    for (chat, kept, transcript, summary) in looked_up {
        if kept {
            continue;
        }
        pending += 1;
        let Some(transcript) = transcript else {
            failures.push(failure_line(
                &chat,
                "no cached transcript; run classify first.",
            ));
            continue;
        };
        let long = is_long(&transcript);
        let content = if long {
            summary
        } else {
            Some(transcript.markdown)
        };
        let Some(content) = content else {
            failures.push(failure_line(
                &chat,
                "no current summary; run classify first.",
            ));
            continue;
        };
        let tokens = if long {
            i64::try_from(chatgpt_core::js::utf16_len(&content).div_ceil(3)).unwrap_or(i64::MAX)
        } else {
            transcript.approx_tokens
        };
        let input = json!({
            "id": chat.id,
            "original_title": chat.title,
            "content_kind": if long { "summary" } else { "transcript" },
            "content": content,
        });
        ready.push(Item {
            chat,
            input,
            tokens,
        });
    }
    let ready_count = ready.len();
    let groups = batches(ready);
    reporter.note(format!(
        "{} chat(s): {} local titles cached or manual, {ready_count} to generate in {} batch(es).",
        targets.len(),
        targets.len() - pending,
        groups.len()
    ));
    let step = reporter.step("Generating local titles", Some(ready_count));
    let run = Run {
        state,
        reporter,
        access,
        generated: Mutex::new(0),
        failures: Mutex::new(failures),
    };
    futures_util::stream::iter(groups)
        .for_each_concurrent(CONCURRENCY, |group| {
            let (run, step) = (&run, &step);
            async move {
                let size = group.len();
                run.process(group).await;
                let count = step.advance(size);
                step.update(count);
            }
        })
        .await;
    let generated = *run.generated.lock().unwrap_or_else(PoisonError::into_inner);
    let failures = run
        .failures
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner);
    step.finish(&format!(
        "Generated {generated} local title(s){}",
        if failures.is_empty() {
            String::new()
        } else {
            format!(", {} failed or missing content", failures.len())
        }
    ));
    Ok(failures)
}

impl Run<'_> {
    /// `processGroup`: the whole batch, else each half, down to one chat.
    async fn process(&self, group: Vec<Item>) {
        let mut stack = vec![group];
        while let Some(group) = stack.pop() {
            match self.attempt(&group).await {
                Ok(()) => {}
                Err(_) if group.len() > 1 => {
                    let mid = group.len().div_ceil(2);
                    let (first, second) = group.split_at(mid);
                    // The first half goes first, as the TS CLI awaits it.
                    stack.push(second.to_vec());
                    stack.push(first.to_vec());
                }
                Err(why) => {
                    if let Some(item) = group.first() {
                        self.failures
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .push(failure_line(&item.chat, &why));
                    }
                }
            }
        }
    }

    async fn attempt(&self, group: &[Item]) -> Result<(), String> {
        if self.reporter.client_gone() {
            return Err("no title: the command was interrupted".to_owned());
        }
        let input = Value::Array(group.iter().map(|item| item.input.clone()).collect());
        let mut response = super::luna::ask(self.access, PROMPT, &input, &schema()).await?;
        let items = response
            .get_mut("items")
            .and_then(Value::as_array_mut)
            .ok_or("Luna returned a missing or duplicate id.")?;
        // A lone chat's id is taken on trust: Luna sometimes mangles it.
        if let ([only], [item]) = (group, items.as_mut_slice())
            && let Some(object) = item.as_object_mut()
        {
            object.insert("id".to_owned(), Value::String(only.chat.id.clone()));
        }
        let ids: Vec<Option<&str>> = items
            .iter()
            .map(|item| item.get("id").and_then(Value::as_str))
            .collect();
        let returned: std::collections::HashSet<Option<&str>> = ids.iter().copied().collect();
        let expected: std::collections::HashSet<&str> =
            group.iter().map(|item| item.chat.id.as_str()).collect();
        if items.len() != group.len()
            || returned.len() != expected.len()
            || expected.iter().any(|id| !returned.contains(&Some(*id)))
        {
            return Err("Luna returned a missing or duplicate id.".to_owned());
        }
        let field = |item: &Value, name: &str| {
            item.get(name)
                .and_then(Value::as_str)
                .map(chatgpt_core::js::trim)
                .unwrap_or_default()
                .to_owned()
        };
        if items.iter().any(|item| {
            let title = field(item, "title");
            title.is_empty()
                || field(item, "theme").is_empty()
                || chatgpt_core::js::utf16_len(&title) > 100
        }) {
            return Err("Luna returned an invalid title or theme.".to_owned());
        }
        let version = super::pipeline::profile().local_title_version;
        for item in items.iter() {
            let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
            let chat = group
                .iter()
                .find(|candidate| candidate.chat.id == id)
                .map(|candidate| candidate.chat.clone())
                .ok_or("Luna returned an unknown id.")?;
            // `setLocalTitle`'s cleaning and check, then the theme's.
            let title = crate::mutate::clean_local_title(&field(item, "title"))?;
            let theme = chatgpt_core::js::collapse_spaces(&field(item, "theme"));
            let theme = chatgpt_core::js::utf16_prefix(&theme, 80).to_owned();
            let updated_at = crate::js::now_iso();
            let _no_pass = self.state.syncer.exclusive().await;
            self.state
                .db_write(move |db| {
                    let row = ManualTitle {
                        id: &chat.id,
                        update_time: &chat.update_time,
                        version,
                        title: &title,
                        updated_at: &updated_at,
                    };
                    chatgpt_store::set_luna_title(db, &row, &theme)
                })
                .await
                .map_err(|failure| failure.message)?;
        }
        *self
            .generated
            .lock()
            .unwrap_or_else(PoisonError::into_inner) += items.len();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, tokens: i64) -> Item {
        Item {
            chat: IndexedConversation {
                id: id.into(),
                title: id.into(),
                create_time: String::new(),
                update_time: String::new(),
                is_archived: false,
                pinned: false,
                project_id: None,
                local_title: None,
            },
            input: Value::Null,
            tokens,
        }
    }

    #[test]
    fn batches_hold_twelve_chats_or_22k_tokens() {
        let sizes = |items: Vec<Item>| batches(items).iter().map(Vec::len).collect::<Vec<_>>();
        assert_eq!(
            sizes((0..25).map(|n| item(&n.to_string(), 10)).collect()),
            [12, 12, 1]
        );
        assert_eq!(
            sizes(vec![
                item("a", 15_000),
                item("b", 7_000),
                item("c", 1),
                item("d", 30_000)
            ]),
            [2, 1, 1]
        );
        assert!(batches(Vec::new()).is_empty());
    }
}
