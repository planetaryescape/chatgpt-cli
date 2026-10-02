//! Search: transcript chunks in an FTS5 index the daemon keeps current in
//! the background ([`indexer`]), their vectors ([`embedder`]), and the
//! reads over them: lexical ([`query`]), semantic and hybrid
//! ([`semantic`]), ChatGPT's own search ([`remote`]), and `search-index`
//! ([`catch_up`]). Ported from the TS CLI's `src/search/` @ 1b8c950.

pub mod catch_up;
pub mod chunks;
pub mod embedder;
pub mod indexer;
pub mod model;
pub mod query;
pub mod remote;
pub mod semantic;
pub mod worker;

use std::borrow::Cow;

use chatgpt_core::js::{collapse_spaces, trim};
use chatgpt_protocol::SearchHit;
use chatgpt_store::ChunkVersions;
use rusqlite::Connection;

use crate::handlers::Failure;
use crate::policy::Profile;

/// The versions current chunks carry: the transcript render and the chunker.
pub fn versions(profile: &Profile) -> ChunkVersions {
    ChunkVersions {
        render: profile.render_version,
        chunk: chunks::CHUNK_VERSION,
    }
}

/// A scope's search coverage. `archived`: `None` for both.
pub struct Coverage {
    pub chats: u64,
    /// Chats with current chunks.
    pub indexed: u64,
    /// Current chunks, and how many have vectors from this model.
    pub chunks: u64,
    pub embedded: u64,
}

pub fn coverage(
    db: &Connection,
    archived: Option<bool>,
    versions: ChunkVersions,
) -> chatgpt_store::Result<Coverage> {
    let (chats, indexed) = chatgpt_store::coverage(db, archived, versions)?;
    let (chunks, embedded) =
        chatgpt_store::vector_coverage(db, archived, versions, chatgpt_embed::MODEL_VERSION)?;
    Ok(Coverage {
        chats,
        indexed,
        chunks,
        embedded,
    })
}

/// Bun reads text that isn't UTF-8 (a chunk holding half an emoji; see
/// `chunks.rs`) as `""` under this many bytes, and with each invalid
/// sequence as U+FFFD from this many on. Observed with Bun 1.3.14.
const BUN_LOSSY_FROM_BYTES: usize = 64;

/// What Bun reads back for stored chunk text.
pub fn bun_text(bytes: &[u8]) -> Cow<'_, str> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Cow::Borrowed(text),
        Err(_) if bytes.len() >= BUN_LOSSY_FROM_BYTES => String::from_utf8_lossy(bytes),
        Err(_) => Cow::Borrowed(""),
    }
}

/// `text.slice(0, units)`: the first `units` UTF-16 code units. A cut
/// through an emoji leaves its high surrogate, which comes back as U+FFFD
/// (as Bun prints it) along with the surrogate (as `JSON.stringify` escapes
/// it).
pub fn slice_units(text: &str, units: usize) -> (String, Option<u16>) {
    let kept: Vec<u16> = text.encode_utf16().take(units).collect();
    let cut = kept
        .last()
        .copied()
        .filter(|unit| (0xD800..0xDC00).contains(unit));
    (String::from_utf16_lossy(&kept), cut)
}

/// The longest local snippet, in UTF-16 code units (`excerpt`).
const SNIPPET_UNITS: usize = 200;

/// `excerpt`: whitespace collapsed, trimmed, cut to 200 UTF-16 units.
pub fn excerpt(text: &str) -> (String, Option<u16>) {
    slice_units(trim(&collapse_spaces(text)), SNIPPET_UNITS)
}

/// The cli's last step for every search: each hit takes its chat's display
/// title, as `index.get(id)[0]` gives it, when the chat is indexed.
pub fn with_display_titles(
    db: &Connection,
    hits: Vec<SearchHit>,
    profile: &Profile,
) -> Result<Vec<SearchHit>, Failure> {
    hits.into_iter()
        .map(|mut hit| {
            if let Some(chat) = chatgpt_store::get(db, &hit.id, profile.local_title_version)
                .map_err(Failure::store)?
                .first()
            {
                chat.display_title().clone_into(&mut hit.title);
            }
            Ok(hit)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_that_is_not_utf8_reads_back_as_bun_reads_it() {
        assert_eq!(bun_text(b"hello rust \xED\xA0\xBD"), "");
        let mut long = b"a".repeat(61);
        long.extend(b"\xED\xA0\xBD");
        assert_eq!(
            bun_text(&long),
            format!("{}\u{FFFD}\u{FFFD}\u{FFFD}", "a".repeat(61))
        );
        assert_eq!(bun_text(b"fine"), "fine");
    }

    #[test]
    fn excerpts_collapse_space_and_cut_at_200_utf16_units() {
        assert_eq!(excerpt("  a \n\n b\t"), ("a b".to_owned(), None));
        let long = format!("{}👍tail", "x".repeat(199));
        assert_eq!(
            excerpt(&long),
            (format!("{}\u{FFFD}", "x".repeat(199)), Some(0xD83D))
        );
        assert_eq!(excerpt(&"é".repeat(300)).0.chars().count(), 200);
        // A whole emoji at the end is no cut.
        let whole = format!("{}👍tail", "x".repeat(198));
        assert_eq!(excerpt(&whole), (format!("{}👍", "x".repeat(198)), None));
    }
}
