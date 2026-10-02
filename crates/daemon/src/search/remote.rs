//! `search --remote`: ChatGPT's own search, mapped as the remote branch of
//! the TS CLI's `search` action (`src/cli.ts` @ 1b8c950) maps it. The one
//! search that reads the network, because the user asked for it. It needs
//! no synced index; chats the index knows show their display title.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chatgpt_core::ErrorKind;
use chatgpt_core::js::collapse_spaces;
use chatgpt_protocol::{REMOTE_SEARCH_MAX, SearchHit, SearchResults, SessionChoice};

use super::{slice_units, with_display_titles};
use crate::api::{Api, GlobalSearchHit};
use crate::handlers::Failure;
use crate::js::iso_from_seconds;
use crate::state::State;

const SNIPPET_UNITS: usize = 160;

/// How many results to ask ChatGPT for: more than `limit` when some will
/// be filtered out by archive state.
fn requested(limit: u64, all: bool) -> u64 {
    if all {
        limit
    } else {
        REMOTE_SEARCH_MAX.min(limit.saturating_mul(5))
    }
}

/// The TS CLI's mapping: in scope, each chat once, at most `limit`;
/// `score` null; the snippet's whitespace collapsed (not trimmed) and cut
/// to 160 UTF-16 units. `archived`: `None` for `--all`.
fn hits(
    found: Vec<GlobalSearchHit>,
    limit: u64,
    archived: Option<bool>,
) -> Result<Vec<SearchHit>, Failure> {
    let mut seen = HashSet::new();
    let mut hits = Vec::new();
    for hit in found {
        let is_archived = hit.payload.is_archived.unwrap_or(false);
        if archived.is_some_and(|archived| is_archived != archived) {
            continue;
        }
        if !seen.insert(hit.payload.conversation_id.clone()) {
            continue;
        }
        let updated = iso_from_seconds(hit.update_time)
            .ok_or_else(|| Failure::new(ErrorKind::Decode, "Invalid time value"))?;
        let snippet = slice_units(&collapse_spaces(&hit.snippet), SNIPPET_UNITS);
        hits.push(SearchHit {
            id: hit.payload.conversation_id,
            title: hit.title,
            updated,
            archived: is_archived,
            score: None,
            snippet,
        });
        if hits.len() as u64 == limit {
            break;
        }
    }
    Ok(hits)
}

/// A hit without `payload.is_archived` (ChatGPT has always sent it) takes
/// the index's archive state for its chat; `known` holds those.
fn fill_archive_state(found: &mut [GlobalSearchHit], known: &HashMap<String, bool>) {
    for hit in found {
        if hit.payload.is_archived.is_none() {
            hit.payload.is_archived = known.get(&hit.payload.conversation_id).copied();
        }
    }
}

pub async fn search(
    state: &Arc<State>,
    query: String,
    limit: u64,
    archived: Option<bool>,
    session: SessionChoice,
) -> Result<SearchResults, Failure> {
    let api = Api::new(Arc::clone(&state.sessions), session);
    let mut found = api
        .global_search(&query, requested(limit, archived.is_none()))
        .await?;
    let profile = state.profile();
    let unknown: Vec<String> = found
        .iter()
        .filter(|hit| hit.payload.is_archived.is_none())
        .map(|hit| hit.payload.conversation_id.clone())
        .collect();
    if !unknown.is_empty() {
        let known = state
            .db(move |db| {
                let mut known = HashMap::new();
                for id in unknown {
                    for chat in chatgpt_store::get(db, &id, profile.local_title_version)? {
                        if chat.id == id {
                            known.insert(id.clone(), chat.is_archived);
                        }
                    }
                }
                Ok(known)
            })
            .await?;
        fill_archive_state(&mut found, &known);
    }
    let hits = hits(found, limit, archived)?;
    let hits = state
        .db(move |db| Ok(with_display_titles(db, hits, profile)))
        .await??;
    Ok(SearchResults {
        hits,
        synced_at: String::new(),
        chats: 0,
        indexed: 0,
        chunks: 0,
        embedded: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::GlobalSearchPayload;

    fn found(id: &str, archived: bool, snippet: &str) -> GlobalSearchHit {
        GlobalSearchHit {
            title: format!("T {id}"),
            snippet: snippet.into(),
            update_time: 1_758_000_000.123_456_7,
            payload: GlobalSearchPayload {
                conversation_id: id.into(),
                is_archived: Some(archived),
            },
        }
    }

    #[test]
    fn requests_more_when_archive_state_filters() {
        assert_eq!(requested(5, false), 25);
        assert_eq!(requested(20, false), 40);
        assert_eq!(requested(30, true), 30);
    }

    #[test]
    fn results_are_scoped_deduplicated_and_cut() {
        let long = format!("{}  x\n\ny", "a".repeat(150));
        let results = hits(
            vec![
                found("a", false, " lead\tspace "),
                found("b", true, "archived"),
                found("a", false, "again"),
                found("c", false, &long),
                found("d", false, "over the limit"),
            ],
            2,
            Some(false),
        )
        .expect("hits");
        let ids: Vec<&str> = results.iter().map(|hit| hit.id.as_str()).collect();
        assert_eq!(ids, ["a", "c"]);
        // Collapsed but not trimmed.
        assert_eq!(results[0].snippet, " lead space ");
        assert_eq!(results[0].score, None);
        // `new Date(seconds * 1000).toISOString()`.
        assert_eq!(results[0].updated, "2025-09-16T05:20:00.123Z");
        assert_eq!(results[1].snippet, format!("{} x y", "a".repeat(150)));
        let everything = hits(vec![found("b", true, "s")], 5, None).expect("hits");
        assert!(everything[0].archived);
        let cut = hits(
            vec![found("e", false, &format!("{}👍", "a".repeat(159)))],
            5,
            None,
        )
        .expect("hits");
        assert_eq!(cut[0].snippet, "a".repeat(159), "no half emoji");
        let bad = GlobalSearchHit {
            update_time: f64::NAN,
            ..found("f", false, "")
        };
        assert_eq!(
            hits(vec![bad], 5, None).expect_err("bad time").message,
            "Invalid time value"
        );
    }

    #[test]
    fn a_hit_without_archive_state_takes_the_indexs() {
        let missing = |id: &str| GlobalSearchHit {
            payload: GlobalSearchPayload {
                conversation_id: id.into(),
                is_archived: None,
            },
            ..found(id, false, "s")
        };
        let mut found = vec![
            missing("archived"),
            missing("unknown"),
            found("sent", true, "s"),
        ];
        let known = HashMap::from([("archived".to_owned(), true), ("sent".to_owned(), false)]);
        fill_archive_state(&mut found, &known);
        let states: Vec<Option<bool>> = found.iter().map(|hit| hit.payload.is_archived).collect();
        assert_eq!(
            states,
            [Some(true), None, Some(true)],
            "ChatGPT's own answer wins"
        );
        let active = hits(found.clone(), 5, Some(false)).expect("hits");
        assert_eq!(active.len(), 1, "an unknown chat counts as active");
        assert_eq!(active[0].id, "unknown");
        let archived = hits(found, 5, Some(true)).expect("hits");
        let ids: Vec<&str> = archived.iter().map(|hit| hit.id.as_str()).collect();
        assert_eq!(ids, ["archived", "sent"]);
    }
}
