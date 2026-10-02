//! `search --remote`: ChatGPT's own search, mapped as the remote branch of
//! the TS CLI's `search` action (`src/cli.ts` @ 1b8c950) maps it. The one
//! search that reads the network, because the user asked for it. It needs
//! no synced index; chats the index knows show their display title.

use std::collections::HashSet;
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
        if archived.is_some_and(|archived| hit.payload.is_archived != archived) {
            continue;
        }
        if !seen.insert(hit.payload.conversation_id.clone()) {
            continue;
        }
        let updated = iso_from_seconds(hit.update_time)
            .ok_or_else(|| Failure::new(ErrorKind::Decode, "Invalid time value"))?;
        let (snippet, snippet_cut) = slice_units(&collapse_spaces(&hit.snippet), SNIPPET_UNITS);
        hits.push(SearchHit {
            id: hit.payload.conversation_id,
            title: hit.title,
            updated,
            archived: hit.payload.is_archived,
            score: None,
            snippet,
            snippet_cut,
        });
        if hits.len() as u64 == limit {
            break;
        }
    }
    Ok(hits)
}

pub async fn search(
    state: &Arc<State>,
    query: String,
    limit: u64,
    archived: Option<bool>,
    session: SessionChoice,
) -> Result<SearchResults, Failure> {
    let api = Api::new(Arc::clone(&state.sessions), session);
    let found = api
        .global_search(&query, requested(limit, archived.is_none()))
        .await?;
    let hits = hits(found, limit, archived)?;
    let profile = state.profile();
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
                is_archived: archived,
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
        assert_eq!(cut[0].snippet_cut, Some(0xD83D));
        let bad = GlobalSearchHit {
            update_time: f64::NAN,
            ..found("f", false, "")
        };
        assert_eq!(
            hits(vec![bad], 5, None).expect_err("bad time").message,
            "Invalid time value"
        );
    }
}
