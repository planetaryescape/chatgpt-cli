//! `Transcript`: a chat's transcript for the TUI's preview and `review`,
//! as the TS CLI's `useTranscript` (`src/tui/use-transcript.ts`) and
//! `review`'s `load` (`src/commands/review.ts`) @ 1b8c950 get it: the
//! cache when it's current, else (or, for `review`, always) the batch
//! endpoint, whose rendering is cached (with its search chunks) for next
//! time. With it, the summary a long chat's current judgment was made
//! from, which both show, also when the fetch fails.

use chatgpt_core::ErrorKind;
use chatgpt_protocol::{ChatTranscript, Excerpt, SessionChoice, TranscriptSource};
use chatgpt_store::IndexedConversation;

use crate::classify::SUMMARY_PROMPT_VERSION;
use crate::classify::pipeline::save_transcript_if_current;
use crate::handlers::Failure;
use crate::render::{cached_transcript, visible_turns};
use crate::state::State;

/// The TS CLI's words when the batch leaves the chat out.
const NOT_RETURNED: &str = "ChatGPT didn't return this conversation (deleted?)";

pub async fn transcript(
    state: &State,
    id: String,
    source: TranscriptSource,
    session: SessionChoice,
) -> Result<ChatTranscript, Failure> {
    if source == TranscriptSource::Unknown {
        return Err(Failure::new(
            ErrorKind::Unsupported,
            "this daemon doesn't know that transcript source; run `chatgpt daemon stop` and try again",
        ));
    }
    let profile = state.profile();
    let lookup = id.clone();
    let fresh = source == TranscriptSource::Fresh;
    let (chat, cached, summary) = state
        .db(move |db| {
            let chat = chatgpt_store::get(db, &lookup, profile.local_title_version)?
                .into_iter()
                .find(|chat| chat.id == lookup);
            let Some(chat) = chat else {
                return Ok((None, None, None));
            };
            let cached = if fresh {
                None
            } else {
                chatgpt_store::transcript(db, &chat.id, &chat.update_time, profile.render_version)?
                    .map(|transcript| transcript.markdown)
            };
            // Long chats were judged from a summary; it's the best overview.
            let judged_from_summary = chatgpt_store::judgment(
                db,
                &chat.id,
                &chat.update_time,
                &profile.questions_version,
            )?
            .is_some_and(|judgment| judgment.content_kind == "summary");
            let summary = if judged_from_summary {
                chatgpt_store::summary(db, &chat.id, &chat.update_time, SUMMARY_PROMPT_VERSION)?
            } else {
                None
            };
            Ok((Some(chat), cached, summary))
        })
        .await?;
    let chat = chat.ok_or_else(|| {
        Failure::new(
            ErrorKind::InvalidInput,
            format!("No conversation matching \"{id}\"."),
        )
    })?;
    let mut answer = ChatTranscript {
        markdown: cached,
        summary,
        ..ChatTranscript::default()
    };
    if answer.markdown.is_none() && source != TranscriptSource::Cache {
        // The summary still goes back when the fetch fails.
        match download(state, &chat, session).await {
            Ok((markdown, excerpt)) => {
                answer.markdown = Some(markdown);
                answer.excerpt = fresh.then_some(excerpt);
            }
            Err(failure) => answer.fetch_error = Some(failure.message),
        }
    }
    answer.id = chat.id;
    Ok(answer)
}

/// The chat through the batch endpoint (as the index's account), cached
/// under the `update_time` the index has for it.
async fn download(
    state: &State,
    chat: &IndexedConversation,
    session: SessionChoice,
) -> Result<(String, Excerpt), Failure> {
    let api = crate::sync::pinned_api(state, session).await?;
    let _foreground = state.indexer.foreground();
    let item = api
        .batch(std::slice::from_ref(&chat.id))
        .await?
        .into_iter()
        .find(|item| item.id == chat.id)
        .ok_or_else(|| Failure::new(ErrorKind::Api, NOT_RETURNED))?;
    let failed = |why: String| Failure::new(ErrorKind::Decode, why);
    let transcript = cached_transcript(&item, &chat.update_time, state.profile().render_version)
        .map_err(failed)?;
    let turns = visible_turns(&item.conversation).map_err(failed)?;
    let excerpt = Excerpt {
        turns: u64::try_from(turns.len()).unwrap_or(u64::MAX),
        first_user: turns
            .iter()
            .find(|(role, _)| *role == "user")
            .map(|(_, text)| text.clone()),
        last_assistant: turns
            .iter()
            .rev()
            .find(|(role, _)| *role == "assistant")
            .map(|(_, text)| text.clone()),
    };
    let markdown = transcript.markdown.clone();
    // Shown either way; cached only if no sync moved the chat on meanwhile.
    if save_transcript_if_current(state, chat, transcript).await? {
        state.embedder.wake();
    }
    Ok((markdown, excerpt))
}
