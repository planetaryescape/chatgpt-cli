//! `selectTargets` from the TS CLI's `src/commands/select.ts` @ 1b8c950:
//! the chats a command acts on, from explicit ids or id prefixes (or the
//! ones read from stdin), else from the filters, with its messages and its
//! order of checks. Never touches the network.

use chatgpt_core::ErrorKind;
use chatgpt_protocol::{Filter, ListRows, Row, Selection};
use chatgpt_store::IndexedConversation;
use rusqlite::Connection;

use crate::filters::{jev_filter, selection, title_matches};
use crate::handlers::Failure;
use crate::policy::Profile;
use crate::reads::require_synced;

fn invalid(message: String) -> Failure {
    Failure::new(ErrorKind::InvalidInput, message)
}

pub(crate) fn wrong_scope(archived: bool) -> &'static str {
    if archived {
        "archived; pass --archived or --all to include it"
    } else {
        "active; omit --archived or pass --all to include it"
    }
}

/// `hasFilter`: a filter that narrows (the scope flags don't).
fn has_filter(filter: &Filter) -> bool {
    let set = |value: &Option<String>| value.as_deref().is_some_and(|value| !value.is_empty());
    set(&filter.older_than)
        || set(&filter.newer_than)
        || set(&filter.before)
        || set(&filter.after)
        || set(&filter.title)
        || set(&filter.limit)
        || set(&filter.suggest)
        || set(&filter.topic)
        || filter.brainstorm.is_some()
}

/// `selectTargets`, and when the index last synced.
pub fn targets(
    db: &Connection,
    chosen: &Selection,
    profile: &Profile,
    now_ms: i64,
) -> Result<(Vec<IndexedConversation>, String), Failure> {
    let synced_at = require_synced(db)?;
    let filter = &chosen.filter;
    let Some(ids) = &chosen.ids else {
        if !chosen.allow_unfiltered && !has_filter(filter) {
            return Err(invalid(
                "Pass conversation ids, `-` to read ids from stdin, or a filter such as --older-than 1y or --title.".to_owned(),
            ));
        }
        let mut narrowed = selection(filter, now_ms).map_err(Failure::invalid)?;
        narrowed.index.include_pinned = chosen.pinned;
        let rows: Vec<IndexedConversation> =
            chatgpt_store::query(db, &narrowed.index, profile.local_title_version)
                .map_err(Failure::store)?
                .into_iter()
                .filter(|chat| title_matches(narrowed.title.as_ref(), chat))
                .collect();
        let jev = jev_filter(filter, profile)
            .map_err(Failure::invalid)?
            .excluding_unsure(chosen.exclude_unsure);
        let judgments = if jev.narrows() {
            crate::policy::current_judgments(db, profile).map_err(Failure::store)?
        } else {
            std::collections::HashMap::new()
        };
        let chats = jev
            .apply(rows, |chat| judgments.get(&chat.id), profile)
            .map_err(Failure::policy)?;
        return Ok((chats, synced_at));
    };
    // Explicit ids bypass the filters (and `--pinned`): each must be found
    // once and be in scope. A chat named twice (`delete a a`, or a prefix
    // and its full id) is acted on once, where it first appears; the TS
    // CLI would act on it twice.
    let mut chats: Vec<IndexedConversation> = Vec::with_capacity(ids.len());
    for id in ids {
        let chat = target(
            db,
            id,
            filter.archived,
            filter.all,
            profile.local_title_version,
        )?;
        if !chats.iter().any(|seen| seen.id == chat.id) {
            chats.push(chat);
        }
    }
    Ok((chats, synced_at))
}

/// One id or prefix of `selectTargets`: found exactly once, in scope.
pub fn target(
    db: &Connection,
    prefix: &str,
    archived: bool,
    all: bool,
    local_title_version: u32,
) -> Result<IndexedConversation, Failure> {
    let mut matches =
        chatgpt_store::get(db, prefix, local_title_version).map_err(Failure::store)?;
    if matches.len() > 1 {
        return Err(invalid(format!(
            "\"{prefix}\" matches {} conversations; use a longer prefix.",
            matches.len()
        )));
    }
    let target = matches.pop().ok_or_else(|| {
        invalid(format!(
            "No conversation matching \"{prefix}\" in the index. Run `chatgpt sync`?"
        ))
    })?;
    if !all && target.is_archived != archived {
        return Err(invalid(format!(
            "Conversation \"{prefix}\" is {}.",
            wrong_scope(target.is_archived)
        )));
    }
    Ok(target)
}

/// `selectOne`: the one chat `reference` names, after `requireSynced`.
pub fn one(
    db: &Connection,
    reference: &str,
    archived: bool,
    all: bool,
    profile: &Profile,
) -> Result<(IndexedConversation, String), Failure> {
    let synced_at = require_synced(db)?;
    let chat = target(db, reference, archived, all, profile.local_title_version)?;
    Ok((chat, synced_at))
}

/// A selected chat as the client's preview needs it: no verdict, since
/// `formatRow(c)` is called without one.
pub fn plain_row(chat: &IndexedConversation) -> Row {
    Row {
        id: chat.id.clone(),
        title: chat.title.clone(),
        create_time: chat.create_time.clone(),
        update_time: chat.update_time.clone(),
        is_archived: u8::from(chat.is_archived),
        pinned: u8::from(chat.pinned),
        project_id: chat.project_id.clone(),
        local_title: chat.local_title.clone(),
        display_title: chat.display_title().to_owned(),
        topic: None,
        row_topic: None,
        jev: None,
    }
}

/// `Select`: the chats, as rows for the preview.
pub fn rows(
    db: &Connection,
    chosen: &Selection,
    profile: &Profile,
    now_ms: i64,
) -> Result<ListRows, Failure> {
    let (chats, synced_at) = targets(db, chosen, profile, now_ms)?;
    Ok(ListRows {
        rows: chats.iter().map(plain_row).collect(),
        synced_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_narrowing_options_count_as_filters() {
        assert!(!has_filter(&Filter {
            archived: true,
            all: true,
            ..Filter::default()
        }));
        assert!(!has_filter(&Filter {
            title: Some(String::new()),
            ..Filter::default()
        }));
        assert!(has_filter(&Filter {
            brainstorm: Some(String::new()),
            ..Filter::default()
        }));
        assert!(has_filter(&Filter {
            limit: Some("3".into()),
            ..Filter::default()
        }));
    }
}
