//! Jev's follow-up for chats its first pass left unsure: the TS CLI's
//! `DeepClassifier.classify` (`src/classify/deep-pipeline.ts` @ 1b8c950).
//! It asks what deleting the chat would lose. Only `classify` runs it;
//! the guard and the background Jev never do.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use chatgpt_protocol::SessionChoice;
use chatgpt_store::{IndexedConversation, JudgmentRow, NewDeepJudgment, Transcript};
use futures_util::StreamExt;

use super::access::Access;
use super::costs::{CostMeter, format_usd};
use super::pipeline::{
    Approved, CONCURRENCY, CONFIRM_ABOVE_TOKENS, SummaryGate, cached_summaries, download,
    ensure_summary, failure_line, is_long, jev_state, profile, summary_kind,
};
use super::summarise;
use crate::handlers::Failure;
use crate::policy::Judged;
use crate::progress::{Asker, Reporter};
use crate::state::State;

pub struct DeepClassified {
    pub judgments: HashMap<String, JudgmentRow>,
    pub failures: Vec<String>,
    /// Chats whose transcript couldn't be had.
    pub held_back: usize,
}

pub struct DeepClassifier<'a> {
    pub state: &'a Arc<State>,
    pub reporter: &'a Reporter,
    pub session: SessionChoice,
    pub access: &'a Access,
    /// The question before a large batch of summaries: `-y`, and the
    /// client if it can answer.
    pub yes: bool,
    pub asker: Option<&'a Asker>,
}

impl DeepClassifier<'_> {
    /// `classify(unsureTargets, { force })`.
    pub async fn classify(
        &self,
        targets: &[IndexedConversation],
        force: bool,
    ) -> Result<DeepClassified, Failure> {
        let render = self.state.profile().render_version;
        let questions = profile().questions_version.clone();
        let deep_version = profile().deep_questions_version.clone();
        let lookups: Vec<(String, String)> = targets
            .iter()
            .map(|chat| (chat.id.clone(), chat.update_time.clone()))
            .collect();
        let (bases, deep_done, cached_transcripts) = self
            .state
            .db(move |db| {
                let mut bases = HashMap::new();
                let mut deep_done = HashMap::new();
                let mut transcripts = HashMap::new();
                for (id, update_time) in &lookups {
                    if let Some(row) = chatgpt_store::judgment(db, id, update_time, &questions)? {
                        bases.insert(id.clone(), row);
                    }
                    deep_done.insert(
                        id.clone(),
                        chatgpt_store::has_deep_judgment(
                            db,
                            id,
                            update_time,
                            &questions,
                            &deep_version,
                        )?,
                    );
                    if let Some(transcript) =
                        chatgpt_store::transcript(db, id, update_time, render)?
                    {
                        transcripts.insert(id.clone(), transcript);
                    }
                }
                Ok((bases, deep_done, transcripts))
            })
            .await?;
        let mut judgments = HashMap::new();
        let mut todo: Vec<&IndexedConversation> = Vec::new();
        for chat in targets {
            let Some(base) = bases.get(&chat.id) else {
                continue;
            };
            let unsure = Judged::new(base, profile())
                .and_then(|judged| judged.base_verdict())
                .map_err(Failure::policy)?
                .unsure;
            if !unsure {
                continue;
            }
            if !force && deep_done.get(&chat.id).copied().unwrap_or(false) {
                judgments.insert(chat.id.clone(), base.clone());
            } else {
                todo.push(chat);
            }
        }
        let cached = judgments.len();
        self.reporter.note(format!(
            "{} unsure chat(s): {cached} already deep-classified, {} to judge.",
            targets.len(),
            todo.len()
        ));
        let mut failures = Vec::new();
        if todo.is_empty() {
            return Ok(DeepClassified {
                judgments,
                failures,
                held_back: 0,
            });
        }
        let mut transcripts: HashMap<String, Transcript> = todo
            .iter()
            .filter_map(|chat| {
                cached_transcripts
                    .get(&chat.id)
                    .map(|transcript| (chat.id.clone(), transcript.clone()))
            })
            .collect();
        let missing: Vec<&IndexedConversation> = todo
            .iter()
            .copied()
            .filter(|chat| !transcripts.contains_key(&chat.id))
            .collect();
        if !missing.is_empty() {
            let found_before = transcripts.len();
            download(
                self.state,
                self.reporter,
                &self.session,
                "Downloading missing transcripts",
                &missing,
                &mut transcripts,
                &mut failures,
                |chat| failure_line(chat, "not returned by ChatGPT; run sync."),
                |downloaded, _| {
                    format!(
                        "Found transcripts for {} chat(s)",
                        found_before + downloaded
                    )
                },
            )
            .await?;
        }
        let ready: Vec<&IndexedConversation> = todo
            .iter()
            .copied()
            .filter(|chat| transcripts.contains_key(&chat.id))
            .collect();
        let long: Vec<&IndexedConversation> = ready
            .iter()
            .copied()
            .filter(|chat| transcripts.get(&chat.id).is_some_and(is_long))
            .collect();
        let summaries = cached_summaries(self.state, &long).await?;
        // New summaries pass the same question as the first pass's: a large
        // batch is asked about (or needs -y), and a no holds those back.
        let need: Vec<&IndexedConversation> = long
            .iter()
            .copied()
            .filter(|chat| !summaries.contains_key(&chat.id))
            .collect();
        let tokens: i64 = need
            .iter()
            .filter_map(|chat| transcripts.get(&chat.id))
            .map(|transcript| transcript.approx_tokens)
            .sum();
        let names = if need.is_empty() {
            Vec::new()
        } else {
            summarise::names(self.access)
                .map_err(|why| Failure::new(chatgpt_core::ErrorKind::InvalidInput, why))?
        };
        let gate = SummaryGate {
            yes: self.yes,
            asker: self.asker,
        };
        let mut approval = None;
        let mut ready = ready;
        if !need.is_empty() && !names.is_empty() {
            if tokens > CONFIRM_ABOVE_TOKENS && !self.yes {
                self.reporter.note(format!(
                    "{} unsure long chat(s) need a new summary for the follow-up ({}), ~{}k tokens.",
                    need.len(),
                    names.join(", then "),
                    (tokens as f64 / 1000.0).round()
                ));
            }
            approval = gate.confirm(tokens).await?;
            if approval.is_none() {
                ready.retain(|chat| !need.iter().any(|held| held.id == chat.id));
                self.reporter
                    .note("Skipping those; the other follow-ups go ahead.".to_owned());
            }
        }
        let held_back = todo.len() - ready.len();
        let meter = Mutex::new(CostMeter::default());
        let client = self.access.jev();
        let step = self.reporter.step("Deep-classifying", Some(ready.len()));
        let ready: Vec<IndexedConversation> = ready.into_iter().cloned().collect();
        let results: Vec<(IndexedConversation, Result<JudgmentRow, String>)> =
            futures_util::stream::iter(ready)
                .map(|chat| {
                    let (step, meter, client) = (&step, &meter, &client);
                    let transcript = transcripts.get(&chat.id);
                    let summary = summaries.get(&chat.id).map(String::as_str);
                    async move {
                        let judged = match transcript {
                            Some(transcript) => {
                                self.judge(&chat, transcript, summary, client, meter, approval)
                                    .await
                            }
                            None => Err("no transcript".to_owned()),
                        };
                        let count = step.advance(1);
                        let total = meter.lock().unwrap_or_else(PoisonError::into_inner).total();
                        step.update_with(count, &format_usd(total));
                        (chat, judged)
                    }
                })
                .buffer_unordered(CONCURRENCY)
                .collect()
                .await;
        for (chat, judged) in results {
            match judged {
                Ok(row) => {
                    judgments.insert(chat.id.clone(), row);
                }
                Err(why) => failures.push(failure_line(&chat, &why)),
            }
        }
        step.finish(&format!(
            "Deep-classified {} chat(s){}",
            judgments.len() - cached,
            if failures.is_empty() {
                String::new()
            } else {
                format!(", {} failed", failures.len())
            }
        ));
        let meter = meter.into_inner().unwrap_or_else(PoisonError::into_inner);
        for line in meter.report() {
            self.reporter.note(line);
        }
        Ok(DeepClassified {
            judgments,
            failures,
            held_back,
        })
    }

    async fn judge(
        &self,
        chat: &IndexedConversation,
        transcript: &Transcript,
        summary: Option<&str>,
        client: &Result<typesafe_client::Client, String>,
        meter: &Mutex<CostMeter>,
        approval: Option<Approved>,
    ) -> Result<JudgmentRow, String> {
        if self.reporter.client_gone() {
            return Err("not judged: the command was interrupted".to_owned());
        }
        let (content, content_kind) = if is_long(transcript) {
            if summary.is_none() && summarise::names(self.access)?.is_empty() {
                return Err(
                    "Long chat needs a summary, but neither codex nor claude is on PATH.".into(),
                );
            }
            let summary = ensure_summary(
                self.state,
                self.access,
                self.reporter,
                meter,
                chat,
                transcript,
                summary,
                approval,
            )
            .await?;
            (summary, summary_kind(transcript))
        } else {
            (transcript.markdown.clone(), "full transcript".to_owned())
        };
        let client = client.as_ref().map_err(String::clone)?;
        if self.reporter.client_gone() {
            return Err("not judged: the command was interrupted".to_owned());
        }
        // No `as_of` here, as in the TS CLI's follow-up.
        let state = jev_state(chat, transcript, None, &content_kind, &content);
        let result = client
            .system_one(&state, super::questions::deep())
            .await
            .map_err(|error| error.to_string())?;
        super::questions::validate(super::questions::deep(), &result.answers)?;
        meter
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .add_jev(result.input_tokens);
        let row = NewDeepJudgment {
            id: chat.id.clone(),
            update_time: chat.update_time.clone(),
            questions_version: profile().questions_version.clone(),
            version: profile().deep_questions_version.clone(),
            answers: crate::js::stringify(&result.answers),
            classified_at: crate::js::now_iso(),
        };
        let _no_pass = self.state.syncer.exclusive().await;
        self.state
            .db_write(move |db| {
                chatgpt_store::save_deep_judgment(db, &row)?;
                chatgpt_store::judgment(db, &row.id, &row.update_time, &row.questions_version)
            })
            .await
            .map_err(|failure| failure.message)?
            .ok_or_else(|| "the judgment it follows up is gone".to_owned())
    }
}
