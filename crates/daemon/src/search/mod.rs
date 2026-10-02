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

/// At most the first `units` UTF-16 code units of `text`, never cutting
/// through a character (where JS's `text.slice(0, units)` would split an
/// emoji's surrogate pair).
pub fn slice_units(text: &str, units: usize) -> String {
    chatgpt_core::js::utf16_prefix(text, units).to_owned()
}

/// The longest local snippet, in UTF-16 code units (`excerpt`).
const SNIPPET_UNITS: usize = 200;

/// `excerpt`: whitespace collapsed, trimmed, cut to 200 UTF-16 units.
pub fn excerpt(text: &str) -> String {
    slice_units(trim(&collapse_spaces(text)), SNIPPET_UNITS)
}

/// Run `read` in one read transaction, so every query in it sees the same
/// snapshot (WAL keeps it while the writer goes on): a hit's chat, score,
/// scope and snippet can't come from different moments, as they could if
/// the indexer replaced a chunk, and SQLite reused its id, in between.
pub fn in_snapshot<T>(
    db: &Connection,
    read: impl FnOnce(&Connection) -> Result<T, Failure>,
) -> Result<T, Failure> {
    let snapshot = db
        .unchecked_transaction()
        .map_err(|error| Failure::store(error.into()))?;
    let answer = read(&snapshot)?;
    snapshot
        .commit()
        .map_err(|error| Failure::store(error.into()))?;
    Ok(answer)
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
    fn reads_in_a_snapshot_never_see_a_chunk_replaced_meanwhile() {
        use chatgpt_store::{NewConversation, Store, Unindexed};
        let dir = tempfile::tempdir().expect("dir");
        let store = Store::open(&dir.path().join("chatgpt.db")).expect("store");
        let versions = ChunkVersions {
            render: 2,
            chunk: 1,
        };
        let chat = Unindexed {
            id: "a".into(),
            title: "A".into(),
            update_time: "t".into(),
            cached: false,
        };
        store
            .write(|db| {
                let row = NewConversation {
                    id: "a".into(),
                    title: "A".into(),
                    create_time: "t".into(),
                    update_time: "t".into(),
                    is_archived: false,
                    pinned: false,
                    project_id: None,
                };
                chatgpt_store::replace_all(db, &[row], "t")?;
                chatgpt_store::replace_chunks(db, &chat, versions, &[b"first".to_vec()])
            })
            .expect("index");
        let body = |db: &Connection| {
            chatgpt_store::chunk_body(db, 1)
                .map_err(Failure::store)
                .map(Option::unwrap_or_default)
        };
        let seen = store
            .read(|db| {
                Ok(in_snapshot(db, |db| {
                    let before = body(db)?;
                    // The indexer replaces the chunk; SQLite reuses its id.
                    store
                        .write(|db| {
                            chatgpt_store::replace_chunks(
                                db,
                                &chat,
                                versions,
                                &[b"second".to_vec()],
                            )
                        })
                        .map_err(Failure::store)?;
                    Ok((before, body(db)?))
                }))
            })
            .expect("read")
            .map_err(|failure| failure.message)
            .expect("snapshot");
        assert_eq!(seen, (b"first".to_vec(), b"first".to_vec()));
        let now = store
            .read(|db| Ok(body(db).map_err(|failure| failure.message)))
            .expect("read")
            .expect("body");
        assert_eq!(now, b"second", "the id was reused");
    }

    #[test]
    fn excerpts_collapse_space_and_cut_at_200_utf16_units() {
        assert_eq!(excerpt("  a \n\n b\t"), "a b");
        // A cut through an emoji leaves it out.
        let long = format!("{}👍tail", "x".repeat(199));
        assert_eq!(excerpt(&long), "x".repeat(199));
        assert_eq!(excerpt(&"é".repeat(300)).chars().count(), 200);
        // A whole emoji at the end is kept.
        let whole = format!("{}👍tail", "x".repeat(198));
        assert_eq!(excerpt(&whole), format!("{}👍", "x".repeat(198)));
    }
}
