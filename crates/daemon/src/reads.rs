//! `list` and the chat part of `stats`, from the index. Ported from the
//! `list` and `stats` actions in the TS CLI's `src/cli.ts` and from
//! `src/commands/stats.ts` @ 1b8c950. Never touches the network.

use std::collections::HashMap;

use chatgpt_core::ErrorKind;
use chatgpt_protocol::{Filter, Jev, ListRows, Row, StatsReport, TopicCounts};
use chatgpt_store::{IndexedConversation, JudgmentRow};
use rusqlite::Connection;

use crate::filters::{jev_filter, selection, title_matches};
use crate::handlers::Failure;
use crate::policy::{Judged, Profile};

pub const NOT_SYNCED: &str = "No local index yet. Run `chatgpt sync` first.";
const LABELS: [&str; 6] = ["delete", "delete?", "archive", "archive?", "keep", "keep?"];
const BRAINSTORM_KINDS: [&str; 4] = ["writing", "sermon", "product", "other"];

/// `requireSynced`: when the index last synced, or the TS CLI's error.
pub fn require_synced(db: &Connection) -> Result<String, Failure> {
    chatgpt_store::synced_at(db)
        .map_err(Failure::store)?
        .ok_or_else(|| Failure::new(ErrorKind::NotSynced, NOT_SYNCED))
}

/// The chats `list` and `stats` report on, with their current judgments.
struct Selected {
    synced_at: String,
    chats: Vec<IndexedConversation>,
    judgments: HashMap<String, JudgmentRow>,
}

/// The rows `list` and `stats` share: `requireSynced`, then `toFilter`, the
/// index query and the title regex, then `applyJevFiltersAndLimit`.
fn selected(
    db: &Connection,
    filter: &Filter,
    profile: &Profile,
    now_ms: i64,
) -> Result<Selected, Failure> {
    let synced_at = require_synced(db)?;
    let chosen = selection(filter, now_ms).map_err(Failure::invalid)?;
    let rows: Vec<IndexedConversation> =
        chatgpt_store::query(db, &chosen.index, profile.local_title_version)
            .map_err(Failure::store)?
            .into_iter()
            .filter(|chat| title_matches(chosen.title.as_ref(), chat))
            .collect();
    let jev = jev_filter(filter, profile).map_err(Failure::invalid)?;
    let judgments = crate::policy::current_judgments(db, profile).map_err(Failure::store)?;
    let chats = jev
        .apply(rows, |chat| judgments.get(&chat.id), profile)
        .map_err(Failure::policy)?;
    Ok(Selected {
        synced_at,
        chats,
        judgments,
    })
}

pub fn list(
    db: &Connection,
    filter: &Filter,
    profile: &Profile,
    now_ms: i64,
) -> Result<ListRows, Failure> {
    let Selected {
        synced_at,
        chats,
        judgments,
    } = selected(db, filter, profile, now_ms)?;
    let mut rows = Vec::with_capacity(chats.len());
    for chat in chats {
        let judgment = judgments.get(&chat.id);
        let (row_topic, jev) = match judgment {
            Some(row) => {
                let judged = Judged::new(row, profile).map_err(Failure::policy)?;
                let verdict = judged.verdict().map_err(Failure::policy)?;
                let answers = serde_json::from_str(&row.answers).map_err(|error| {
                    Failure::new(
                        ErrorKind::Internal,
                        format!(
                            "the stored Jev answers for {} are unreadable: {error}",
                            row.id
                        ),
                    )
                })?;
                let jev = Jev {
                    suggestion: verdict.suggestion,
                    unsure: verdict.unsure,
                    reason: verdict.reason,
                    brainstorm: verdict.brainstorm,
                    deep: verdict.deep,
                    luna: verdict.luna,
                    answers,
                };
                (judged.topic(), Some(jev))
            }
            None => (None, None),
        };
        rows.push(Row {
            display_title: chat.display_title().to_owned(),
            topic: judgment.and_then(|row| row.topic.clone()),
            row_topic,
            jev,
            id: chat.id,
            title: chat.title,
            create_time: chat.create_time,
            update_time: chat.update_time,
            is_archived: u8::from(chat.is_archived),
            pinned: u8::from(chat.pinned),
            project_id: chat.project_id,
            local_title: chat.local_title,
        });
    }
    Ok(ListRows { rows, synced_at })
}

/// `printStats`'s numbers. The saved-memory part is added by the caller.
pub fn chat_stats(
    db: &Connection,
    filter: &Filter,
    profile: &Profile,
    now_ms: i64,
) -> Result<StatsReport, Failure> {
    let Selected {
        synced_at,
        chats,
        judgments,
    } = selected(db, filter, profile, now_ms)?;
    let label_index = |label: &str| LABELS.iter().position(|known| *known == label);
    let mut suggestions = [0u64; 6];
    let mut brainstorms = [0u64; 4];
    let mut by_topic: HashMap<String, [u64; 6]> = HashMap::new();
    let mut unjudged = 0;
    for chat in &chats {
        let Some(row) = judgments.get(&chat.id) else {
            unjudged += 1;
            continue;
        };
        let judged = Judged::new(row, profile).map_err(Failure::policy)?;
        let verdict = judged.verdict().map_err(Failure::policy)?;
        let label = if verdict.unsure {
            format!("{}?", verdict.suggestion)
        } else {
            verdict.suggestion.clone()
        };
        // A label outside the six (an unknown Luna suggestion) is counted
        // nowhere visible, as in the TS CLI.
        let index = label_index(&label);
        if let Some(index) = index {
            suggestions[index] += 1;
        }
        if let Some(kind) = verdict.brainstorm.as_deref()
            && let Some(at) = BRAINSTORM_KINDS.iter().position(|known| *known == kind)
        {
            brainstorms[at] += 1;
        }
        // A judgment without a topic counts in no topic row.
        if let (Some(topic), Some(index)) = (judged.topic(), index) {
            by_topic.entry(topic).or_default()[index] += 1;
        }
    }
    let mut topics: Vec<TopicCounts> = profile
        .topics
        .iter()
        .map(|topic| TopicCounts {
            topic: topic.clone(),
            counts: by_topic.get(topic).copied().unwrap_or_default().to_vec(),
        })
        .collect();
    // Stable, as Array.prototype.sort is: ties keep the TOPICS order.
    topics.sort_by_key(|topic| std::cmp::Reverse(topic.counts.iter().sum::<u64>()));
    Ok(StatsReport {
        synced_at,
        chats: chats.len() as u64,
        unjudged,
        labels: LABELS.map(str::to_owned).to_vec(),
        suggestions: suggestions.to_vec(),
        brainstorms: BRAINSTORM_KINDS
            .iter()
            .zip(brainstorms)
            .map(|(kind, count)| ((*kind).to_owned(), count))
            .collect(),
        topics,
        memory: None,
        memory_error: None,
    })
}
