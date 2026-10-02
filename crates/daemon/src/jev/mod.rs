//! The Jev guard behind `archive`/`delete --check` and `--suggest <action>`:
//! the TS CLI's `checkWithJev` (`src/commands/check.ts`) over the part of
//! `Classifier.classify` (`src/classify/pipeline.ts`) it runs, @ 1b8c950.
//! Chats without a current judgment are judged now, with the same steps,
//! notes and cost lines, and only the ones Jev confidently backs the action
//! for are approved. Anything it can't judge is held back, never approved.
//!
//! Judgments use this repository's questions and policy (D9), whatever
//! versions the bridged TS CLI reads with. Live calls happen only here,
//! while the command runs, never in the background.
//!
//! One difference: a long chat needs a summary for Jev, and summaries are
//! still the TS CLI's (`classify` writes them, the import brings them
//! over). A long chat without a cached one is held back instead of
//! summarised.

mod costs;
mod key;
mod questions;

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;

use chatgpt_core::ErrorKind;
use chatgpt_protocol::{ChatAction, Progress, Secret, SessionChoice};
use chatgpt_store::{IndexedConversation, JudgmentRow, NewJudgment, Transcript, Unindexed};
use futures_util::StreamExt;
use serde_json::json;
use tokio::sync::mpsc::UnboundedSender;

use crate::api::BATCH_MAX;
use crate::handlers::Failure;
use crate::policy::{Judged, Profile};
use crate::progress::Reporter;
use crate::state::State;
use costs::{CostMeter, format_usd};

/// Jev takes 32k tokens of state and gets less accurate as it grows, so
/// longer chats are judged from a summary.
const FULL_TRANSCRIPT_MAX_TOKENS: i64 = 12_000;
/// Between batch reads: a courtesy, not a measured limit.
const BATCH_GAP: Duration = Duration::from_millis(500);
const CONCURRENCY: usize = 4;
/// The summary prompt version whose summaries count (`SUMMARY_PROMPT_VERSION`).
const SUMMARY_PROMPT_VERSION: u32 = 9;
/// In debug builds, the TypeSafe API to use instead of the real one.
const BASE_URL_ENV: &str = "TYPESAFE_BASE_URL";

/// The policy the guard judges with: this repository's versions (D9).
static GUARD_PROFILE: LazyLock<Profile> = LazyLock::new(Profile::builtin);

/// `checkWithJev`: the ids of `ids` Jev backs `action` for, in order.
pub async fn check(
    state: &Arc<State>,
    action: ChatAction,
    ids: Vec<String>,
    api_key: Option<Secret>,
    session: SessionChoice,
    progress: Option<UnboundedSender<Progress>>,
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
    let state = Arc::clone(state);
    // Its own task: a judgment paid for is saved even if the client leaves.
    tokio::spawn(async move {
        let reporter = Reporter::for_client(progress);
        let display = state.profile().local_title_version;
        let wanted = ids.clone();
        let found = state
            .db(move |db| chatgpt_store::by_ids(db, &wanted, display))
            .await?;
        let chats: HashMap<String, IndexedConversation> = found
            .into_iter()
            .map(|chat| (chat.id.clone(), chat))
            .collect();
        let targets: Vec<IndexedConversation> =
            ids.iter().filter_map(|id| chats.get(id).cloned()).collect();
        let classifier = Classifier {
            state: &state,
            reporter: &reporter,
            session,
            api_key,
        };
        let classified = classifier.classify(&targets).await?;
        let mut approved = Vec::new();
        let mut held = Vec::new();
        for id in &ids {
            let Some(chat) = chats.get(id) else {
                held.push(format!("  not judged  {id}"));
                continue;
            };
            let Some(row) = classified.judgments.get(id) else {
                held.push(format!("  not judged  {}", chat.display_title()));
                continue;
            };
            let verdict = Judged::new(row, &GUARD_PROFILE)
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

struct Classified {
    judgments: HashMap<String, JudgmentRow>,
    failures: Vec<String>,
}

struct Classifier<'a> {
    state: &'a Arc<State>,
    reporter: &'a Reporter,
    session: SessionChoice,
    api_key: Option<Secret>,
}

fn utc_today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

fn failure_line(chat: &IndexedConversation, why: &str) -> String {
    format!("{} {}: {why}", chat.id, chat.title)
}

impl Classifier<'_> {
    /// `classify(targets)`: reuse current judgments (unless a time-bound
    /// one is due for review), then download, then judge short chats, then
    /// long ones.
    async fn classify(&self, targets: &[IndexedConversation]) -> Result<Classified, Failure> {
        let today = utc_today();
        let render = self.state.profile().render_version;
        let lookups: Vec<(String, String)> = targets
            .iter()
            .map(|chat| (chat.id.clone(), chat.update_time.clone()))
            .collect();
        let questions_version = GUARD_PROFILE.questions_version.clone();
        let (cached, transcripts) = self
            .state
            .db(move |db| {
                let mut judgments = HashMap::new();
                let mut transcripts = HashMap::new();
                for (id, update_time) in &lookups {
                    if let Some(row) =
                        chatgpt_store::judgment(db, id, update_time, &questions_version)?
                    {
                        judgments.insert(id.clone(), row);
                    }
                    if let Some(transcript) =
                        chatgpt_store::transcript(db, id, update_time, render)?
                    {
                        transcripts.insert(id.clone(), transcript);
                    }
                }
                Ok((judgments, transcripts))
            })
            .await?;
        let mut judgments = HashMap::new();
        let mut todo = Vec::new();
        for chat in targets {
            match cached.get(&chat.id) {
                Some(row) if !due_for_time_review(row, &today)? => {
                    judgments.insert(chat.id.clone(), row.clone());
                }
                _ => todo.push(chat),
            }
        }
        let mut failures = Vec::new();
        self.reporter.note(format!(
            "{} chat(s): {} already judged, {} new or changed to judge.",
            targets.len(),
            judgments.len(),
            todo.len()
        ));
        if todo.is_empty() {
            return Ok(Classified {
                judgments,
                failures,
            });
        }
        self.reporter.note(
            "Steps: [1/3] download transcripts → [2/3] judge short chats → [3/3] summarise and judge long chats"
                .to_owned(),
        );
        let mut transcripts: HashMap<String, Transcript> = todo
            .iter()
            .filter_map(|chat| {
                transcripts
                    .get(&chat.id)
                    .map(|transcript| (chat.id.clone(), transcript.clone()))
            })
            .collect();
        let to_fetch: Vec<&IndexedConversation> = todo
            .iter()
            .copied()
            .filter(|chat| !transcripts.contains_key(&chat.id))
            .collect();
        self.download(&to_fetch, render, &mut transcripts, &mut failures)
            .await?;

        let ready: Vec<&IndexedConversation> = todo
            .iter()
            .copied()
            .filter(|chat| transcripts.contains_key(&chat.id))
            .collect();
        let is_long = |chat: &&IndexedConversation| {
            transcripts
                .get(&chat.id)
                .is_some_and(|transcript| transcript.approx_tokens > FULL_TRANSCRIPT_MAX_TOKENS)
        };
        let short: Vec<&IndexedConversation> = ready
            .iter()
            .copied()
            .filter(|chat| !is_long(chat))
            .collect();
        let long_ids: Vec<(String, String)> = ready
            .iter()
            .filter(|chat| is_long(chat))
            .map(|chat| (chat.id.clone(), chat.update_time.clone()))
            .collect();
        let summaries: HashMap<String, String> = self
            .state
            .db(move |db| {
                let mut found = HashMap::new();
                for (id, update_time) in &long_ids {
                    if let Some(summary) =
                        chatgpt_store::summary(db, id, update_time, SUMMARY_PROMPT_VERSION)?
                    {
                        found.insert(id.clone(), summary);
                    }
                }
                Ok(found)
            })
            .await?;
        let long: Vec<&IndexedConversation> = ready
            .iter()
            .copied()
            .filter(|chat| is_long(chat) && summaries.contains_key(&chat.id))
            .collect();
        let need_summary = ready
            .iter()
            .filter(|chat| is_long(chat) && !summaries.contains_key(&chat.id))
            .count();
        if need_summary > 0 {
            self.reporter.note(format!(
                "{need_summary} long chat(s) need a summary, which only `chatgpt classify` writes for now; skipping them in step 3."
            ));
        }

        let meter = Mutex::new(CostMeter::default());
        let judge = Judge {
            state: self.state,
            client: self.client(),
            meter: &meter,
            today: &today,
        };
        let inputs = |chats: Vec<&IndexedConversation>,
                      long: bool|
         -> Vec<(IndexedConversation, Transcript, Option<String>)> {
            chats
                .into_iter()
                .filter_map(|chat| {
                    let transcript = transcripts.get(&chat.id)?.clone();
                    let summary = long.then(|| summaries.get(&chat.id).cloned()).flatten();
                    Some((chat.clone(), transcript, summary))
                })
                .collect()
        };
        for (label, items) in [
            ("[2/3] Judging short chats", inputs(short, false)),
            (
                "[3/3] Summarising and judging long chats",
                inputs(long, true),
            ),
        ] {
            judge
                .all(self.reporter, label, items, &mut judgments, &mut failures)
                .await;
        }
        let report = meter
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .report();
        for line in report {
            self.reporter.note(line);
        }
        Ok(Classified {
            judgments,
            failures,
        })
    }

    /// Step 1: the transcripts not cached for the chat's `update_time`,
    /// through the batch endpoint, saved (with their search chunks) batch
    /// by batch.
    async fn download(
        &self,
        to_fetch: &[&IndexedConversation],
        render: u32,
        transcripts: &mut HashMap<String, Transcript>,
        failures: &mut Vec<String>,
    ) -> Result<(), Failure> {
        if to_fetch.is_empty() {
            self.reporter.note(format!(
                "[1/3] Download transcripts: all {} cached, nothing to download.",
                transcripts.len()
            ));
            return Ok(());
        }
        // Pinned to the index's account, as every read that feeds it is.
        let api = crate::sync::pinned_api(self.state, self.session.clone()).await?;
        let _foreground = self.state.indexer.foreground();
        let versions = crate::search::versions(&self.state.profile());
        let step = self
            .reporter
            .step("[1/3] Downloading transcripts", Some(to_fetch.len()));
        let mut downloaded = 0;
        for (number, batch) in to_fetch.chunks(BATCH_MAX).enumerate() {
            let ids: Vec<String> = batch.iter().map(|chat| chat.id.clone()).collect();
            match api.batch(&ids).await {
                Ok(items) => {
                    let mut items: HashMap<String, _> = items
                        .into_iter()
                        .map(|item| (item.id.clone(), item))
                        .collect();
                    let mut saved = Vec::new();
                    for chat in batch {
                        let Some(item) = items.remove(&chat.id) else {
                            failures.push(failure_line(
                                chat,
                                "not returned by ChatGPT (deleted? run `chatgpt sync`)",
                            ));
                            continue;
                        };
                        match crate::render::cached_transcript(&item, &chat.update_time, render) {
                            Ok(transcript) => saved.push(((*chat).clone(), transcript)),
                            Err(why) => failures.push(failure_line(chat, &why)),
                        }
                    }
                    let rows: Vec<(Unindexed, Transcript, Vec<Vec<u8>>)> = saved
                        .iter()
                        .map(|(chat, transcript)| {
                            let target = Unindexed {
                                id: chat.id.clone(),
                                title: chat.title.clone(),
                                update_time: chat.update_time.clone(),
                                cached: true,
                            };
                            let bodies = crate::search::indexer::chunk_bytes(&transcript.markdown);
                            (target, transcript.clone(), bodies)
                        })
                        .collect();
                    self.state
                        .db_write(move |db| {
                            for (target, transcript, bodies) in &rows {
                                chatgpt_store::save_indexed(
                                    db, transcript, target, versions, bodies,
                                )?;
                            }
                            Ok(())
                        })
                        .await?;
                    downloaded += saved.len();
                    for (chat, transcript) in saved {
                        transcripts.insert(chat.id, transcript);
                    }
                }
                Err(error) => {
                    for chat in batch {
                        failures.push(failure_line(chat, &error.message));
                    }
                }
            }
            let end = ((number + 1) * BATCH_MAX).min(to_fetch.len());
            step.update(end);
            if end < to_fetch.len() {
                tokio::time::sleep(BATCH_GAP).await;
            }
        }
        if downloaded > 0 {
            self.state.embedder.wake();
        }
        let failed = to_fetch.len() - downloaded;
        step.finish(&format!(
            "Downloaded {downloaded} transcript(s){}",
            if failed > 0 {
                format!(", {failed} failed")
            } else {
                String::new()
            }
        ));
        Ok(())
    }

    /// The TypeSafe client, or why there's none (said for every chat, as
    /// the TS CLI does when the key is missing).
    fn client(&self) -> Result<typesafe_client::Client, String> {
        let key = key::api_key(self.api_key.as_ref())?;
        let base = std::env::var(BASE_URL_ENV)
            .ok()
            .filter(|base| cfg!(debug_assertions) && !base.is_empty())
            .unwrap_or_else(|| typesafe_client::DEFAULT_BASE_URL.to_owned());
        typesafe_client::Client::new(&base, key, typesafe_client::RetryPolicy::default())
            .map_err(|error| error.to_string())
    }
}

/// A cached judgment of a still-current time-bound chat goes stale after a
/// while even though the chat didn't change.
fn due_for_time_review(row: &JudgmentRow, today: &str) -> Result<bool, Failure> {
    Ok(Judged::new(row, &GUARD_PROFILE)
        .map_err(Failure::policy)?
        .needs_time_refresh(today))
}

struct Judge<'a> {
    state: &'a State,
    client: Result<typesafe_client::Client, String>,
    meter: &'a Mutex<CostMeter>,
    today: &'a str,
}

impl Judge<'_> {
    /// `judgeAll`: up to four at once, the running cost on the step.
    async fn all(
        &self,
        reporter: &Reporter,
        label: &str,
        items: Vec<(IndexedConversation, Transcript, Option<String>)>,
        judgments: &mut HashMap<String, JudgmentRow>,
        failures: &mut Vec<String>,
    ) {
        if items.is_empty() {
            reporter.note(format!("{label}: nothing to do."));
            return;
        }
        let step = reporter.step(label, Some(items.len()));
        let done = Mutex::new(0usize);
        let results: Vec<(IndexedConversation, Result<JudgmentRow, String>)> =
            futures_util::stream::iter(items)
                .map(|(chat, transcript, summary)| {
                    let step = &step;
                    let done = &done;
                    async move {
                        let judged = self.judge(&chat, &transcript, summary.as_deref()).await;
                        let count = {
                            let mut done = done.lock().unwrap_or_else(PoisonError::into_inner);
                            *done += 1;
                            *done
                        };
                        let cost = format_usd(
                            self.meter
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .total(),
                        );
                        step.update_with(count, &cost);
                        (chat, judged)
                    }
                })
                .buffer_unordered(CONCURRENCY)
                .collect()
                .await;
        let total = results.len();
        let mut failed = 0;
        for (chat, judged) in results {
            match judged {
                Ok(row) => {
                    judgments.insert(chat.id.clone(), row);
                }
                Err(why) => {
                    failed += 1;
                    failures.push(failure_line(&chat, &why));
                }
            }
        }
        let cost = format_usd(
            self.meter
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .total(),
        );
        step.finish(&format!(
            "{label}: {} judged{}, {cost} so far",
            total - failed,
            if failed > 0 {
                format!(", {failed} failed")
            } else {
                String::new()
            }
        ));
    }

    /// `judge`: one chat, from its transcript or (long) its summary, saved
    /// as soon as Jev answers.
    async fn judge(
        &self,
        chat: &IndexedConversation,
        transcript: &Transcript,
        summary: Option<&str>,
    ) -> Result<JudgmentRow, String> {
        let client = self.client.as_ref().map_err(String::clone)?;
        let (content, content_kind, kind_label) = match summary {
            Some(summary) => (
                summary,
                "summary",
                format!(
                    "summary of a long conversation ({} turns), written by another model",
                    transcript.turns
                ),
            ),
            None => (
                transcript.markdown.as_str(),
                "full",
                "full transcript".to_owned(),
            ),
        };
        let day = |time: &str| time.chars().take(10).collect::<String>();
        let state = json!({
            "conversation": {
                "title": chat.title,
                "as_of": self.today,
                "created": day(&chat.create_time),
                "last_updated": day(&chat.update_time),
                "turns": transcript.turns,
                "in_a_project": chat.project_id.as_deref().is_some_and(|id| !id.is_empty()),
                "pinned": chat.pinned,
            },
            "content_kind": kind_label,
            "content": content,
        });
        let result = client
            .system_one(&state, questions::questions())
            .await
            .map_err(|error| error.to_string())?;
        let judgment = NewJudgment {
            id: chat.id.clone(),
            update_time: chat.update_time.clone(),
            version: GUARD_PROFILE.questions_version.clone(),
            content_kind: content_kind.to_owned(),
            answers: crate::js::stringify(&result.answers),
            classified_at: crate::js::now_iso(),
        };
        // Read back as every other judgment is read (the topic column
        // included).
        let saved = self
            .state
            .db_write(move |db| {
                chatgpt_store::save_judgment(db, &judgment)?;
                chatgpt_store::judgment(db, &judgment.id, &judgment.update_time, &judgment.version)
            })
            .await;
        // Counted after the save, with no await before the step's update,
        // so the running cost there includes this call and the ones before
        // it, as the TS CLI's does.
        self.meter
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .add_jev(result.input_tokens);
        saved
            .map_err(|failure| failure.message)?
            .ok_or_else(|| "the judgment wasn't saved".to_owned())
    }
}
