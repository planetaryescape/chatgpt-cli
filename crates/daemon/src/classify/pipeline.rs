//! Jev's first pass over chats: the TS CLI's `Classifier.classify`
//! (`src/classify/pipeline.ts` @ 1b8c950), with its steps, notes, pacing
//! and cost lines. Current judgments are reused (unless a time-bound one is
//! due for review, or `--redo`); the rest are judged in three steps:
//! download the transcripts not cached, judge the short chats, then
//! summarise (when needed) and judge the long ones. Every finished chat is
//! saved at once, so a run that stops partway keeps what it paid for.
//!
//! Used by `classify`, the Jev guard (`--check`), and the background Jev,
//! which never summarises: a long chat without a cached summary waits for
//! a `classify` someone runs.
//!
//! Every save takes the sync pass lock briefly, so no pass writes the
//! chat's row (or moves its caches forward) halfway through one.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chatgpt_core::ErrorKind;
use chatgpt_protocol::SessionChoice;
use chatgpt_store::{IndexedConversation, JudgmentRow, NewJudgment, Transcript, Unindexed};
use futures_util::StreamExt;
use serde_json::json;

use super::access::Access;
use super::costs::{CostMeter, GPT_6_LUNA, PaidBy, format_usd};
use super::summarise::{self, SUMMARY_PROMPT_VERSION};
use crate::api::BATCH_MAX;
use crate::handlers::Failure;
use crate::policy::{Judged, Profile};
use crate::progress::{Asker, Reporter};
use crate::state::State;

/// Jev takes 32k tokens of state and gets less accurate as it grows, so
/// longer chats are judged from a summary.
pub const FULL_TRANSCRIPT_MAX_TOKENS: i64 = 12_000;
/// Between batch reads: single-chat reads hit ChatGPT's 429s within
/// minutes, the batch endpoint doesn't at this pace. A courtesy, not a
/// measured limit.
pub const BATCH_GAP: Duration = Duration::from_millis(500);
pub const CONCURRENCY: usize = 4;
/// Summaries use the user's subscription; a big batch of them eats into
/// its usage limits, so it's confirmed first.
const CONFIRM_ABOVE_TOKENS: i64 = 500_000;
/// Codex adds ~19k tokens of its own instructions per call (measured
/// 2026-09-27).
const CODEX_OVERHEAD_TOKENS: i64 = 19_000;

/// What a run may do.
#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    /// `--redo`: judge again even what's current.
    pub force: bool,
    /// `-y`: summarise a large batch without asking.
    pub yes: bool,
    /// Summarise long chats that have no summary. The background Jev
    /// never does.
    pub summarise: bool,
}

pub struct Classified {
    pub judgments: HashMap<String, JudgmentRow>,
    /// `<id> <title>: <why>`.
    pub failures: Vec<String>,
    /// Long chats left out for want of a summary.
    pub held_back: Vec<String>,
    /// What this run's calls cost, USD.
    pub cost: f64,
}

pub struct Classifier<'a> {
    pub state: &'a Arc<State>,
    /// The client's lines; none for the background Jev.
    pub reporter: &'a Reporter,
    pub session: SessionChoice,
    pub access: &'a Access,
    /// Asks the client before a large batch of summaries; without one, a
    /// large batch is never summarised unless `yes`.
    pub asker: Option<&'a Asker>,
}

/// The policy judgments are made and read with: this repository's
/// versions (D9).
pub fn profile() -> &'static Profile {
    Profile::current()
}

pub fn utc_today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

pub fn failure_line(chat: &IndexedConversation, why: &str) -> String {
    format!("{} {}: {why}", chat.id, chat.title)
}

/// `t.approx_tokens > FULL_TRANSCRIPT_MAX_TOKENS`.
pub fn is_long(transcript: &Transcript) -> bool {
    transcript.approx_tokens > FULL_TRANSCRIPT_MAX_TOKENS
}

/// A judgment that's stale for a still-current time-bound chat although
/// the chat didn't change.
fn due_for_time_review(row: &JudgmentRow, today: &str) -> Result<bool, Failure> {
    Ok(Judged::new(row, profile())
        .map_err(Failure::policy)?
        .needs_time_refresh(today))
}

/// The chat's transcript and, for a long one, its summary if cached.
type Input = (IndexedConversation, Transcript, Option<String>);

impl Classifier<'_> {
    /// `classify(targets, { force, yes })`.
    pub async fn classify(
        &self,
        targets: &[IndexedConversation],
        options: Options,
    ) -> Result<Classified, Failure> {
        let today = utc_today();
        let render = self.state.profile().render_version;
        let lookups: Vec<(String, String)> = targets
            .iter()
            .map(|chat| (chat.id.clone(), chat.update_time.clone()))
            .collect();
        let questions_version = profile().questions_version.clone();
        let cached = self
            .state
            .db(move |db| {
                let mut judgments = HashMap::new();
                for (id, update_time) in &lookups {
                    if let Some(row) =
                        chatgpt_store::judgment(db, id, update_time, &questions_version)?
                    {
                        judgments.insert(id.clone(), row);
                    }
                }
                Ok(judgments)
            })
            .await?;
        let mut judgments = HashMap::new();
        let mut todo = Vec::new();
        for chat in targets {
            match cached.get(&chat.id) {
                // An unreadable one is judged again.
                Some(row)
                    if !options.force
                        && crate::policy::readable(row, profile())
                        && !due_for_time_review(row, &today)? =>
                {
                    judgments.insert(chat.id.clone(), row.clone());
                }
                _ => todo.push(chat),
            }
        }
        let mut failures = Vec::new();
        let mut held_back = Vec::new();
        self.reporter.note(if options.force {
            format!("Re-judging all {} matching chat(s).", targets.len())
        } else {
            format!(
                "{} chat(s): {} already judged, {} new or changed to judge.",
                targets.len(),
                judgments.len(),
                todo.len()
            )
        });
        if todo.is_empty() {
            return Ok(Classified {
                judgments,
                failures,
                held_back,
                cost: 0.0,
            });
        }
        self.reporter.note(
            "Steps: [1/3] download transcripts → [2/3] judge short chats → [3/3] summarise and judge long chats"
                .to_owned(),
        );
        // Only the chats to judge: a re-run over everything reads no
        // transcript it doesn't need.
        let wanted: Vec<(String, String)> = todo
            .iter()
            .map(|chat| (chat.id.clone(), chat.update_time.clone()))
            .collect();
        let mut transcripts: HashMap<String, Transcript> = self
            .state
            .db(move |db| {
                let mut found = HashMap::new();
                for (id, update_time) in wanted {
                    if let Some(transcript) =
                        chatgpt_store::transcript(db, &id, &update_time, render)?
                    {
                        found.insert(id, transcript);
                    }
                }
                Ok(found)
            })
            .await?;
        let to_fetch: Vec<&IndexedConversation> = todo
            .iter()
            .copied()
            .filter(|chat| !transcripts.contains_key(&chat.id))
            .collect();
        if to_fetch.is_empty() {
            self.reporter.note(format!(
                "[1/3] Download transcripts: all {} cached, nothing to download.",
                transcripts.len()
            ));
        } else {
            download(
                self.state,
                self.reporter,
                &self.session,
                "[1/3] Downloading transcripts",
                &to_fetch,
                &mut transcripts,
                &mut failures,
                |chat| {
                    failure_line(
                        chat,
                        "not returned by ChatGPT (deleted? run `chatgpt sync`)",
                    )
                },
                |downloaded, total| {
                    format!(
                        "Downloaded {downloaded} transcript(s){}",
                        if downloaded < total {
                            format!(", {} failed", total - downloaded)
                        } else {
                            String::new()
                        }
                    )
                },
            )
            .await?;
        }

        // Short chats first: they need no summary, so results land in
        // seconds. Long chats follow, each summarised and judged in one go.
        let ready: Vec<&IndexedConversation> = todo
            .iter()
            .copied()
            .filter(|chat| transcripts.contains_key(&chat.id))
            .collect();
        let long_of = |chat: &&IndexedConversation| transcripts.get(&chat.id).is_some_and(is_long);
        let short: Vec<&IndexedConversation> = ready
            .iter()
            .copied()
            .filter(|chat| !long_of(chat))
            .collect();
        let mut long: Vec<&IndexedConversation> = ready.iter().copied().filter(long_of).collect();
        let summaries = cached_summaries(self.state, &long).await?;
        let need_summary: Vec<&IndexedConversation> = long
            .iter()
            .copied()
            .filter(|chat| !summaries.contains_key(&chat.id))
            .collect();
        let hold = |long: &mut Vec<&IndexedConversation>, held_back: &mut Vec<String>| {
            held_back.extend(need_summary.iter().map(|chat| chat.id.clone()));
            long.retain(|chat| summaries.contains_key(&chat.id));
        };
        let names = if need_summary.is_empty() || !options.summarise {
            Vec::new()
        } else {
            summarise::names(self.access).map_err(invalid)?
        };
        if !need_summary.is_empty() {
            if !options.summarise {
                hold(&mut long, &mut held_back);
            } else if names.is_empty() {
                hold(&mut long, &mut held_back);
                self.reporter.note(format!(
                    "{} long chat(s) need a summary but neither codex nor claude is on PATH; skipping them in step 3.",
                    need_summary.len()
                ));
            } else {
                let tokens: i64 = need_summary
                    .iter()
                    .filter_map(|chat| transcripts.get(&chat.id))
                    .map(|transcript| transcript.approx_tokens)
                    .sum();
                let calls = i64::try_from(need_summary.len()).unwrap_or(i64::MAX);
                let estimate =
                    (tokens + calls * CODEX_OVERHEAD_TOKENS) as f64 * GPT_6_LUNA.input / 1e6;
                self.reporter.note(format!(
                    "{} long chat(s) for step 3; {} need a new summary ({}), ~{}k tokens, about {} API-equivalent on your subscription.",
                    long.len(),
                    need_summary.len(),
                    names.join(", then "),
                    (tokens as f64 / 1000.0).round(),
                    format_usd(estimate)
                ));
                if tokens > CONFIRM_ABOVE_TOKENS && !options.yes {
                    let go = match self.asker {
                        Some(asker) => asker.ask("Go ahead? [y/N] ").await.ok_or_else(|| {
                            Failure::new(
                                ErrorKind::Internal,
                                "the command went away before answering",
                            )
                        })?,
                        None => false,
                    };
                    if !go {
                        hold(&mut long, &mut held_back);
                        self.reporter.note(
                            "Skipping those in step 3; everything else will still be judged."
                                .to_owned(),
                        );
                    }
                }
            }
        }

        let meter = Mutex::new(CostMeter::default());
        let judge = Judge {
            state: self.state,
            reporter: self.reporter,
            client: self.access.jev(),
            access: self.access,
            meter: &meter,
            today: &today,
        };
        let inputs = |chats: Vec<&IndexedConversation>| -> Vec<Input> {
            chats
                .into_iter()
                .filter_map(|chat| {
                    let transcript = transcripts.get(&chat.id)?.clone();
                    Some((chat.clone(), transcript, summaries.get(&chat.id).cloned()))
                })
                .collect()
        };
        for (label, items) in [
            ("[2/3] Judging short chats", inputs(short)),
            ("[3/3] Summarising and judging long chats", inputs(long)),
        ] {
            judge.all(label, items, &mut judgments, &mut failures).await;
        }
        let meter = meter.into_inner().unwrap_or_else(PoisonError::into_inner);
        for line in meter.report() {
            self.reporter.note(line);
        }
        Ok(Classified {
            judgments,
            failures,
            held_back,
            cost: meter.total(),
        })
    }
}

fn invalid(message: String) -> Failure {
    Failure::new(ErrorKind::InvalidInput, message)
}

/// The cached summaries of `chats`, by id.
pub async fn cached_summaries(
    state: &State,
    chats: &[&IndexedConversation],
) -> Result<HashMap<String, String>, Failure> {
    let lookups: Vec<(String, String)> = chats
        .iter()
        .map(|chat| (chat.id.clone(), chat.update_time.clone()))
        .collect();
    state
        .db(move |db| {
            let mut found = HashMap::new();
            for (id, update_time) in &lookups {
                if let Some(summary) =
                    chatgpt_store::summary(db, id, update_time, SUMMARY_PROMPT_VERSION)?
                {
                    found.insert(id.clone(), summary);
                }
            }
            Ok(found)
        })
        .await
}

/// The transcripts of `to_fetch` not cached for the chat's `update_time`,
/// through the batch endpoint ten at a time, saved (with their search
/// chunks) batch by batch, so an interrupted run resumes.
#[allow(
    clippy::too_many_arguments,
    reason = "the TS steps' wording differs per caller"
)]
pub async fn download(
    state: &State,
    reporter: &Reporter,
    session: &SessionChoice,
    label: &str,
    to_fetch: &[&IndexedConversation],
    transcripts: &mut HashMap<String, Transcript>,
    failures: &mut Vec<String>,
    missing: impl Fn(&IndexedConversation) -> String,
    summary: impl Fn(usize, usize) -> String,
) -> Result<(), Failure> {
    let render = state.profile().render_version;
    // Pinned to the index's account, as every read that feeds it is.
    let api = crate::sync::pinned_api(state, session.clone()).await?;
    let _foreground = state.indexer.foreground();
    let versions = crate::search::versions(state.profile());
    let step = reporter.step(label, Some(to_fetch.len()));
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
                        failures.push(missing(chat));
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
                state
                    .db_write(move |db| {
                        for (target, transcript, bodies) in &rows {
                            chatgpt_store::save_indexed(db, transcript, target, versions, bodies)?;
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
        state.embedder.wake();
    }
    step.finish(&summary(downloaded, to_fetch.len()));
    Ok(())
}

/// A long chat's summary: the cached one, else a new one, metered and
/// saved (`ensureSummary`).
pub async fn ensure_summary(
    state: &State,
    access: &Access,
    meter: &Mutex<CostMeter>,
    chat: &IndexedConversation,
    transcript: &Transcript,
    cached: Option<&str>,
) -> Result<String, String> {
    if let Some(summary) = cached {
        return Ok(summary.to_owned());
    }
    let made = summarise::summarise(access, &chat.title, &transcript.markdown).await?;
    meter.lock().unwrap_or_else(PoisonError::into_inner).add(
        &format!("summaries ({})", made.model),
        PaidBy::Subscription,
        made.usd,
        made.tokens,
    );
    let (id, update_time, summary, model) = (
        chat.id.clone(),
        chat.update_time.clone(),
        made.summary.clone(),
        made.model,
    );
    let _no_pass = state.syncer.exclusive().await;
    state
        .db_write(move |db| {
            chatgpt_store::save_summary(
                db,
                &id,
                &update_time,
                SUMMARY_PROMPT_VERSION,
                &summary,
                &model,
            )
        })
        .await
        .map_err(|failure| failure.message)?;
    Ok(made.summary)
}

/// How Jev is told a long chat's content is a summary.
pub fn summary_kind(transcript: &Transcript) -> String {
    format!(
        "summary of a long conversation ({} turns), written by another model",
        transcript.turns
    )
}

/// The state Jev reads about a chat: its facts (`as_of` only for the first
/// pass, as the TS CLI sends it), what its content is, and the content.
pub fn jev_state(
    chat: &IndexedConversation,
    transcript: &Transcript,
    as_of: Option<&str>,
    content_kind: &str,
    content: &str,
) -> serde_json::Value {
    let day = |time: &str| time.chars().take(10).collect::<String>();
    let mut conversation = serde_json::Map::new();
    conversation.insert("title".into(), json!(chat.title));
    if let Some(as_of) = as_of {
        conversation.insert("as_of".into(), json!(as_of));
    }
    conversation.insert("created".into(), json!(day(&chat.create_time)));
    conversation.insert("last_updated".into(), json!(day(&chat.update_time)));
    conversation.insert("turns".into(), json!(transcript.turns));
    conversation.insert(
        "in_a_project".into(),
        json!(chat.project_id.as_deref().is_some_and(|id| !id.is_empty())),
    );
    conversation.insert("pinned".into(), json!(chat.pinned));
    json!({
        "conversation": conversation,
        "content_kind": content_kind,
        "content": content,
    })
}

struct Judge<'a> {
    state: &'a Arc<State>,
    reporter: &'a Reporter,
    client: Result<typesafe_client::Client, String>,
    access: &'a Access,
    meter: &'a Mutex<CostMeter>,
    today: &'a str,
}

impl Judge<'_> {
    /// `judgeAll`: up to four at once, the running cost on the step.
    async fn all(
        &self,
        label: &str,
        items: Vec<Input>,
        judgments: &mut HashMap<String, JudgmentRow>,
        failures: &mut Vec<String>,
    ) {
        if items.is_empty() {
            self.reporter.note(format!("{label}: nothing to do."));
            return;
        }
        let step = self.reporter.step(label, Some(items.len()));
        let results: Vec<(IndexedConversation, Result<JudgmentRow, String>)> =
            futures_util::stream::iter(items)
                .map(|(chat, transcript, summary)| {
                    let step = &step;
                    async move {
                        let judged = self.judge(&chat, &transcript, summary.as_deref()).await;
                        let count = step.advance(1);
                        let cost = format_usd(self.total());
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
        step.finish(&format!(
            "{label}: {} judged{}, {} so far",
            total - failed,
            if failed > 0 {
                format!(", {failed} failed")
            } else {
                String::new()
            },
            format_usd(self.total())
        ));
    }

    fn total(&self) -> f64 {
        self.meter
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .total()
    }

    /// `judge`: one chat, from its transcript or (long) its summary, saved
    /// as soon as Jev answers.
    async fn judge(
        &self,
        chat: &IndexedConversation,
        transcript: &Transcript,
        summary: Option<&str>,
    ) -> Result<JudgmentRow, String> {
        if self.reporter.client_gone() {
            return Err("not judged: the command was interrupted".to_owned());
        }
        let (content, content_kind, kind_label) = if is_long(transcript) {
            let summary = ensure_summary(
                self.state,
                self.access,
                self.meter,
                chat,
                transcript,
                summary,
            )
            .await?;
            (summary, "summary", summary_kind(transcript))
        } else {
            (
                transcript.markdown.clone(),
                "full",
                "full transcript".to_owned(),
            )
        };
        let client = self.client.as_ref().map_err(String::clone)?;
        let state = jev_state(chat, transcript, Some(self.today), &kind_label, &content);
        let result = client
            .system_one(&state, super::questions::chats())
            .await
            .map_err(|error| error.to_string())?;
        super::questions::validate(super::questions::chats(), &result.answers)?;
        self.meter
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .add_jev(result.input_tokens);
        let judgment = NewJudgment {
            id: chat.id.clone(),
            update_time: chat.update_time.clone(),
            version: profile().questions_version.clone(),
            content_kind: content_kind.to_owned(),
            answers: crate::js::stringify(&result.answers),
            classified_at: crate::js::now_iso(),
        };
        let _no_pass = self.state.syncer.exclusive().await;
        // Read back as every other judgment is read (the topic column
        // included).
        self.state
            .db_write(move |db| {
                chatgpt_store::save_judgment(db, &judgment)?;
                chatgpt_store::judgment(db, &judgment.id, &judgment.update_time, &judgment.version)
            })
            .await
            .map_err(|failure| failure.message)?
            .ok_or_else(|| "the judgment wasn't saved".to_owned())
    }
}
