//! Lexical `search`, ported from the TS CLI's `SearchStore.lexical`
//! (`src/search/store.ts`) and the local branch of its `search` action
//! (`src/cli.ts`) @ 1b8c950. Reads the index only; it never fetches, so
//! while the indexer catches up it answers from what's indexed and says how
//! much that is.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::LazyLock;

use chatgpt_core::ErrorKind;
use chatgpt_core::js::{collapse_spaces, trim};
use chatgpt_protocol::{SearchHit, SearchResults};
use chatgpt_store::LexicalRow;
use regex::Regex;
use rusqlite::Connection;

use crate::handlers::Failure;
use crate::policy::Profile;
use crate::reads::NOT_SYNCED;

/// The longest snippet, in UTF-16 code units (`excerpt`).
const SNIPPET_UNITS: usize = 200;

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

/// `excerpt`: whitespace collapsed, trimmed, cut to 200 UTF-16 units. A cut
/// through an emoji leaves half of it, which the TS CLI prints as U+FFFD.
fn excerpt(text: &str) -> String {
    let collapsed = collapse_spaces(text);
    let units: Vec<u16> = trim(&collapsed)
        .encode_utf16()
        .take(SNIPPET_UNITS)
        .collect();
    String::from_utf16_lossy(&units)
}

/// Bun reads text that isn't UTF-8 (a snippet of a chunk holding half an
/// emoji; see `chunks.rs`) as `""` under this many bytes, and with each
/// invalid sequence as U+FFFD from this many on. Observed with Bun 1.3.14.
const BUN_LOSSY_FROM_BYTES: usize = 64;

/// What Bun reads back for a snippet.
fn snippet_text(row: &LexicalRow) -> Cow<'_, str> {
    match std::str::from_utf8(&row.snippet) {
        Ok(text) => Cow::Borrowed(text),
        Err(_) if row.snippet.len() >= BUN_LOSSY_FROM_BYTES => {
            String::from_utf8_lossy(&row.snippet)
        }
        Err(_) => Cow::Borrowed(""),
    }
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
    let synced_at = chatgpt_store::synced_at(db)
        .map_err(Failure::store)?
        .ok_or_else(|| Failure::new(ErrorKind::NotSynced, NOT_SYNCED))?;
    let versions = super::versions(profile);
    let (chats, indexed) =
        chatgpt_store::coverage(db, archived, versions).map_err(Failure::store)?;
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
    for row in &rows {
        if hits.len() as u64 == limit {
            break;
        }
        if !seen.insert(row.id.as_str()) {
            continue;
        }
        let snippet = match snippet_text(row).as_ref() {
            "" => excerpt(&row.title),
            text => excerpt(text),
        };
        // The display title, as `index.get(id)[0]` gives it.
        let title = chatgpt_store::get(db, &row.id, profile.local_title_version)
            .map_err(Failure::store)?
            .first()
            .map_or_else(|| row.title.clone(), |chat| chat.display_title().to_owned());
        hits.push(SearchHit {
            id: row.id.clone(),
            title,
            updated: row.updated.clone(),
            archived: row.archived,
            score: -row.bm25,
            snippet,
        });
    }
    Ok(SearchResults {
        hits,
        synced_at,
        chats,
        indexed,
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

    #[test]
    fn snippets_that_are_not_utf8_read_back_as_bun_reads_them() {
        let row = |snippet: Vec<u8>| LexicalRow {
            id: "a".into(),
            title: "Title".into(),
            updated: String::new(),
            archived: false,
            bm25: -1.0,
            snippet,
        };
        let short = row(b"hello rust \xED\xA0\xBD".to_vec());
        assert_eq!(snippet_text(&short), "");
        let mut long = b"a".repeat(61);
        long.extend(b"\xED\xA0\xBD");
        assert_eq!(
            snippet_text(&row(long)),
            format!("{}\u{FFFD}\u{FFFD}\u{FFFD}", "a".repeat(61))
        );
        assert_eq!(snippet_text(&row(b"fine".to_vec())), "fine");
    }

    #[test]
    fn excerpts_collapse_space_and_cut_at_200_utf16_units() {
        assert_eq!(excerpt("  a \n\n b\t"), "a b");
        let long = format!("{}👍tail", "x".repeat(199));
        assert_eq!(excerpt(&long), format!("{}\u{FFFD}", "x".repeat(199)));
        assert_eq!(excerpt(&"é".repeat(300)).chars().count(), 200);
    }
}
