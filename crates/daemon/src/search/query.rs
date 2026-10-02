//! Lexical `search`, ported from the TS CLI's `SearchStore.lexical`
//! (`src/search/store.ts`) and the local branch of its `search` action
//! (`src/cli.ts`) @ 1b8c950. Reads the index only; it never fetches, so
//! while the indexer catches up it answers from what's indexed and says how
//! much that is.

use std::collections::HashSet;
use std::sync::LazyLock;

use chatgpt_core::ErrorKind;
use chatgpt_protocol::{SearchHit, SearchResults};
use chatgpt_store::ChunkVersions;
use regex::Regex;
use rusqlite::Connection;

use super::{bun_text, excerpt, with_display_titles};
use crate::handlers::Failure;
use crate::policy::Profile;
use crate::reads::require_synced;

/// `/[\p{L}\p{N}]+/gu`. With the `u` flag JS matches by code point and
/// these classes mean what they mean in Rust's `regex`, so this one needs
/// none of `js::regex`'s translation.
static TERM: LazyLock<Regex> = LazyLock::new(|| {
    #[allow(clippy::unwrap_used, reason = "a fixed pattern that compiles")]
    Regex::new(r"[\p{L}\p{N}]+").unwrap()
});

/// `ftsQuery`: every run of letters and digits, quoted, all required.
fn fts_query(query: &str) -> Result<String, Failure> {
    let terms: Vec<String> = TERM
        .find_iter(query)
        .map(|term| format!("\"{}\"", term.as_str()))
        .collect();
    if terms.is_empty() {
        return Err(Failure::new(
            ErrorKind::InvalidInput,
            "Search query needs at least one letter or number.",
        ));
    }
    Ok(terms.join(" AND "))
}

/// `SearchStore.lexical`: each chat's best chunk, best first, at most
/// `limit`, with the title its chunks were indexed with. `archived`: `None`
/// for both.
pub fn lexical_hits(
    db: &Connection,
    query: &str,
    limit: u64,
    archived: Option<bool>,
    versions: ChunkVersions,
) -> Result<Vec<SearchHit>, Failure> {
    let fts = fts_query(query)?;
    let rows = chatgpt_store::lexical(
        db,
        &fts,
        archived,
        versions,
        limit.saturating_mul(20).max(200),
    )
    .map_err(Failure::store)?;
    let mut seen = HashSet::new();
    let mut hits = Vec::new();
    for row in rows {
        if hits.len() as u64 == limit {
            break;
        }
        if !seen.insert(row.id.clone()) {
            continue;
        }
        let (snippet, snippet_cut) = match bun_text(&row.snippet).as_ref() {
            "" => excerpt(&row.title),
            text => excerpt(text),
        };
        hits.push(SearchHit {
            id: row.id,
            title: row.title,
            updated: row.updated,
            archived: row.archived,
            score: Some(-row.bm25),
            snippet,
            snippet_cut,
        });
    }
    Ok(hits)
}

/// `search <query>` without `--semantic`, `--hybrid` or `--remote`.
/// `archived`: `None` for `--all`.
pub fn search(
    db: &Connection,
    query: &str,
    limit: u64,
    archived: Option<bool>,
    profile: &Profile,
) -> Result<SearchResults, Failure> {
    let synced_at = require_synced(db)?;
    let versions = super::versions(profile);
    let (chats, indexed) =
        chatgpt_store::coverage(db, archived, versions).map_err(Failure::store)?;
    let hits = lexical_hits(db, query, limit, archived, versions)?;
    Ok(SearchResults {
        hits: with_display_titles(db, hits, profile)?,
        synced_at,
        chats,
        indexed,
        chunks: 0,
        embedded: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_quote_every_letter_and_digit_run() {
        assert_eq!(
            fts_query("rust's \"async\" café-2026 日本").expect("query"),
            "\"rust\" AND \"s\" AND \"async\" AND \"café\" AND \"2026\" AND \"日本\""
        );
        let empty = fts_query(" -- !? ").expect_err("no terms");
        assert_eq!(
            empty.message,
            "Search query needs at least one letter or number."
        );
    }
}
