//! `search --semantic` and `--hybrid`, ported from `SearchStore.semantic`
//! (`src/search/store.ts`), `searchLocal` (`src/search/query.ts`) and the
//! local branch of the `search` action (`src/cli.ts`) @ 1b8c950.
//!
//! The query is embedded by the background embedder's worker; everything
//! else reads the index. Neither touches the network: until the embedder
//! has vectors, the search says how far it has got instead.

use chatgpt_core::ErrorKind;
use chatgpt_embed::{DIM, MODEL_VERSION};
use chatgpt_protocol::{SearchHit, SearchMode, SearchResults};
use chatgpt_store::ChunkVersions;
use std::collections::HashMap;

use indexmap::IndexMap;
use rusqlite::Connection;

use super::{Coverage, coverage, excerpt, in_snapshot, query::lexical_hits, with_display_titles};
use crate::handlers::Failure;
use crate::reads::require_synced;
use crate::state::State;

/// The dot product as the TS CLI sums it: each `f32` pair multiplied and
/// added in f64, in order.
pub fn dot(query: &[f32], embedding: &[u8]) -> f64 {
    query
        .iter()
        .zip(embedding.as_chunks::<4>().0)
        .fold(0.0, |score, (q, four)| {
            score + f64::from(*q) * f64::from(f32::from_le_bytes(*four))
        })
}

/// `(a, b) => b.score - a.score`: equal (and NaN) scores keep their order.
fn js_descending(a: f64, b: f64) -> std::cmp::Ordering {
    b.partial_cmp(&a).unwrap_or(std::cmp::Ordering::Equal)
}

/// A chat's best chunk so far.
struct Best {
    chunk_id: i64,
    chunk_index: i64,
    title: String,
    updated: String,
    archived: bool,
    score: f64,
}

/// `SearchStore.semantic`: each chat's best-scoring chunk, best first, at
/// most `limit`, with an excerpt of that chunk's body. Exact ties go to the
/// earlier chunk in a chat and the smaller chat id between chats, which
/// every index agrees on (SQLite's row order and chunk ids don't).
pub fn semantic_hits(
    db: &Connection,
    query: &[f32],
    limit: u64,
    archived: Option<bool>,
    versions: ChunkVersions,
) -> Result<Vec<SearchHit>, Failure> {
    // Fully ordered below, so insertion order doesn't matter.
    let mut best: HashMap<String, Best> = HashMap::new();
    let mut bad = None;
    chatgpt_store::each_vector(db, archived, versions, MODEL_VERSION, |row| {
        if row.embedding.len() != DIM * 4 {
            bad.get_or_insert(row.chunk_id);
            return Ok(());
        }
        let score = dot(query, row.embedding);
        if best.get(row.conversation_id).is_some_and(|previous| {
            score < previous.score
                || (score == previous.score && row.chunk_index > previous.chunk_index)
        }) {
            return Ok(());
        }
        best.insert(
            row.conversation_id.to_owned(),
            Best {
                chunk_id: row.chunk_id,
                chunk_index: row.chunk_index,
                title: row.title.to_owned(),
                updated: row.updated.to_owned(),
                archived: row.archived,
                score,
            },
        );
        Ok(())
    })
    .map_err(Failure::store)?;
    if let Some(chunk) = bad {
        return Err(Failure::new(
            ErrorKind::Internal,
            format!("Bad embedding for chunk {chunk} in the index."),
        ));
    }
    let mut ranked: Vec<(String, Best)> = best.into_iter().collect();
    ranked.sort_by(|(a_id, a), (b_id, b)| {
        js_descending(a.score, b.score).then_with(|| a_id.cmp(b_id))
    });
    ranked
        .into_iter()
        .take(usize::try_from(limit).unwrap_or(usize::MAX))
        .map(|(id, best)| {
            let body = chatgpt_store::chunk_body(db, best.chunk_id)
                .map_err(Failure::store)?
                .unwrap_or_default();
            let snippet = excerpt(&body);
            Ok(SearchHit {
                id,
                title: best.title,
                updated: best.updated,
                archived: best.archived,
                score: Some(best.score),
                snippet,
            })
        })
        .collect()
}

/// `searchLocal`'s hybrid: reciprocal-rank fusion, `1 / (60 + rank + 1)`
/// from each list, a chat keeping the hit of the list that found it first;
/// best first (stable), at most `limit`.
pub fn fuse(lexical: Vec<SearchHit>, semantic: Vec<SearchHit>, limit: u64) -> Vec<SearchHit> {
    let mut fused: IndexMap<String, (SearchHit, f64)> = IndexMap::new();
    for list in [lexical, semantic] {
        for (rank, hit) in list.into_iter().enumerate() {
            let add = 1.0 / (60.0 + rank as f64 + 1.0);
            match fused.get_mut(&hit.id) {
                Some((_, score)) => *score += add,
                None => {
                    fused.insert(hit.id.clone(), (hit, add));
                }
            }
        }
    }
    let mut ranked: Vec<(SearchHit, f64)> = fused.into_values().collect();
    ranked.sort_by(|(_, a), (_, b)| js_descending(*a, *b));
    ranked
        .into_iter()
        .take(usize::try_from(limit).unwrap_or(usize::MAX))
        .map(|(hit, score)| SearchHit {
            score: Some(score),
            ..hit
        })
        .collect()
}

/// Why there's nothing to rank by meaning yet.
fn not_ready(state: &State, coverage: &Coverage) -> Failure {
    let message = match state.embedder.status().waiting {
        Some(why) => format!("No local embeddings yet: {why}."),
        None => format!(
            "No local embeddings yet: the daemon is embedding in the background ({} of {} chunks so far).",
            coverage.embedded, coverage.chunks
        ),
    };
    Failure::new(ErrorKind::NotSynced, message)
}

/// `searchLocal`'s hybrid: `limit * 4` from each ranking, fused.
fn hybrid_hits(
    db: &Connection,
    query: &str,
    vector: &[f32],
    limit: u64,
    archived: Option<bool>,
    versions: ChunkVersions,
) -> Result<Vec<SearchHit>, Failure> {
    let wider = limit.saturating_mul(4);
    let lexical = lexical_hits(db, query, wider, archived, versions)?;
    let semantic = semantic_hits(db, vector, wider, archived, versions)?;
    Ok(fuse(lexical, semantic, limit))
}

/// `search <query> --semantic` or `--hybrid`. `archived`: `None` for `--all`.
pub async fn search(
    state: &State,
    query: String,
    limit: u64,
    archived: Option<bool>,
    mode: SearchMode,
) -> Result<SearchResults, Failure> {
    let profile = state.profile();
    let versions = super::versions(profile);
    // The query's vector doesn't depend on the counts: make both at once.
    let read = state.db(move |db| {
        let synced_at = require_synced(db);
        let coverage = coverage(db, archived, versions)?;
        Ok(synced_at.map(|synced_at| (synced_at, coverage)))
    });
    let (read, vector) = tokio::join!(read, state.embedder.embed_query(&query));
    let (synced_at, coverage) = read??;
    if coverage.embedded == 0 {
        return Err(not_ready(state, &coverage));
    }
    let vector = vector.map_err(|why| {
        Failure::new(
            ErrorKind::Internal,
            format!("Couldn't embed the query: {why}"),
        )
    })?;
    let hits = state
        .db(move |db| {
            Ok(in_snapshot(db, |db| {
                match mode {
                    SearchMode::Hybrid => {
                        hybrid_hits(db, &query, &vector, limit, archived, versions)
                    }
                    SearchMode::Semantic => semantic_hits(db, &vector, limit, archived, versions),
                    SearchMode::Lexical | SearchMode::Unknown => Err(Failure::new(
                        ErrorKind::Internal,
                        "not a semantic search mode",
                    )),
                }
                .and_then(|hits| with_display_titles(db, hits, profile))
            }))
        })
        .await??;
    Ok(SearchResults {
        hits,
        synced_at,
        chats: coverage.chats,
        indexed: coverage.indexed,
        chunks: coverage.chunks,
        embedded: coverage.embedded,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(id: &str) -> SearchHit {
        SearchHit {
            id: id.into(),
            title: format!("T{id}"),
            updated: "2026-01-01".into(),
            archived: false,
            score: Some(0.0),
            snippet: format!("from {id}"),
        }
    }

    fn bytes(values: &[f32]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }

    #[test]
    fn dot_products_sum_in_f64_as_js_does() {
        let query = [0.1f32, 0.2, 0.3];
        let embedding = bytes(&[0.4, 0.5, 0.6]);
        // Each f32 widened, multiplied and added in f64, in order.
        let expected = f64::from(0.1f32) * f64::from(0.4f32)
            + f64::from(0.2f32) * f64::from(0.5f32)
            + f64::from(0.3f32) * f64::from(0.6f32);
        assert_eq!(dot(&query, &embedding), expected);
        assert_ne!(expected, f64::from(0.1f32 * 0.4 + 0.2 * 0.5 + 0.3 * 0.6));
    }

    #[test]
    fn rrf_fuses_by_rank_and_keeps_the_first_lists_hit() {
        let lexical = vec![hit("a"), hit("b"), hit("c")];
        let mut semantic_b = hit("b");
        semantic_b.snippet = "semantic snippet".into();
        let semantic = vec![semantic_b, hit("d"), hit("a")];
        let fused = fuse(lexical, semantic, 10);
        let ids: Vec<&str> = fused.iter().map(|hit| hit.id.as_str()).collect();
        // a: 1/61 + 1/63; b: 1/62 + 1/61; c: 1/63; d: 1/62.
        assert_eq!(ids, ["b", "a", "d", "c"]);
        assert_eq!(fused[0].score, Some(1.0 / 62.0 + 1.0 / 61.0));
        assert_eq!(fused[1].score, Some(1.0 / 61.0 + 1.0 / 63.0));
        assert_eq!(fused[0].snippet, "from b", "the lexical hit is kept");
        // Equal scores keep insertion order: c (lexical rank 2) and a chat
        // first found by the semantic list at the same rank.
        let tied = fuse(vec![hit("x"), hit("y")], vec![hit("z"), hit("w")], 4);
        let ids: Vec<&str> = tied.iter().map(|hit| hit.id.as_str()).collect();
        assert_eq!(ids, ["x", "z", "y", "w"]);
        assert_eq!(fuse(vec![hit("a")], vec![hit("b")], 1).len(), 1);
    }

    #[test]
    fn semantic_ranking_takes_each_chats_best_chunk() {
        use chatgpt_store::{NewConversation, NewVector, Store, Unindexed};
        let dir = tempfile::tempdir().expect("dir");
        let store = Store::open(&dir.path().join("chatgpt.db")).expect("store");
        let versions = ChunkVersions {
            render: 2,
            chunk: 1,
        };
        let chats: Vec<NewConversation> = ["a", "b", "c"]
            .iter()
            .map(|id| NewConversation {
                id: (*id).into(),
                title: format!("Title {id}"),
                create_time: "2024-01-01T00:00:00Z".into(),
                update_time: "2024-02-01T00:00:00Z".into(),
                is_archived: *id == "c",
                pinned: false,
                project_id: None,
            })
            .collect();
        // Unit vectors on the first axes; the query points along axis 0.
        let axis = |weights: &[f32]| {
            let mut vector = vec![0f32; DIM];
            vector[..weights.len()].copy_from_slice(weights);
            vector
        };
        // (chat, chunk bodies, a vector per chunk)
        type Chunks<'a> = (&'a str, &'a [&'a str], Vec<Vec<f32>>);
        let chunks: [Chunks<'_>; 3] = [
            (
                "a",
                &["a one", "a two"],
                vec![axis(&[0.6, 0.8]), axis(&[0.8, 0.6])],
            ),
            ("b", &["b one"], vec![axis(&[0.9, 0.1])]),
            ("c", &["c one"], vec![axis(&[1.0])]),
        ];
        store
            .write(|db| {
                chatgpt_store::replace_all(db, &chats, "t")?;
                for (id, bodies, _) in &chunks {
                    let bodies: Vec<String> = bodies.iter().map(|&body| body.to_owned()).collect();
                    let target = Unindexed {
                        id: (*id).into(),
                        title: format!("Title {id}"),
                        update_time: "2024-02-01T00:00:00Z".into(),
                        cached: false,
                    };
                    chatgpt_store::replace_chunks(db, &target, versions, &bodies)?;
                }
                let pending =
                    chatgpt_store::pending_vectors(db, versions, MODEL_VERSION, 0, 100, 0)?;
                let vectors: Vec<NewVector> = pending
                    .into_iter()
                    .zip(chunks.iter().flat_map(|(_, _, vectors)| vectors))
                    .map(|(chunk, vector)| NewVector {
                        chunk_id: chunk.id,
                        text: chunk.text,
                        embedding: bytes(vector),
                    })
                    .collect();
                chatgpt_store::save_vectors(db, &vectors, MODEL_VERSION)?;
                Ok(())
            })
            .expect("index");
        let query = axis(&[1.0]);
        let hits = store
            .read(|db| {
                Ok(semantic_hits(db, &query, 10, Some(false), versions).map_err(|f| f.message))
            })
            .expect("read")
            .expect("hits");
        let found: Vec<(&str, &str)> = hits
            .iter()
            .map(|hit| (hit.id.as_str(), hit.snippet.as_str()))
            .collect();
        // b (0.9) beats a's best chunk (0.8, its second); c is archived.
        assert_eq!(found, [("b", "b one"), ("a", "a two")]);
        assert_eq!(hits[0].score, Some(f64::from(0.9f32)));
        let all = store
            .read(|db| Ok(semantic_hits(db, &query, 2, None, versions).map_err(|f| f.message)))
            .expect("read")
            .expect("hits");
        let ids: Vec<&str> = all.iter().map(|hit| hit.id.as_str()).collect();
        assert_eq!(ids, ["c", "b"]);
    }

    // docs/issues/semantic-search-followups.md: exact ties don't depend on
    // chunk ids or SQLite's row order.
    #[test]
    fn equal_scores_rank_by_chat_id_and_chunk_order() {
        use chatgpt_store::{NewConversation, NewVector, Store, Unindexed};
        let dir = tempfile::tempdir().expect("dir");
        let store = Store::open(&dir.path().join("chatgpt.db")).expect("store");
        let versions = ChunkVersions {
            render: 2,
            chunk: 1,
        };
        let same = bytes(&[1.0f32; DIM]);
        store
            .write(|db| {
                let chat = |id: &str| NewConversation {
                    id: id.into(),
                    title: id.into(),
                    create_time: "t".into(),
                    update_time: "t".into(),
                    is_archived: false,
                    pinned: false,
                    project_id: None,
                };
                chatgpt_store::replace_all(db, &[chat("z"), chat("y")], "t")?;
                // z first, so its chunks get the smaller ids; its second
                // chunk is written before its first.
                for (id, bodies) in [("z", ["z one", "z two"]), ("y", ["y one", "y two"])] {
                    let target = Unindexed {
                        id: id.into(),
                        title: id.into(),
                        update_time: "t".into(),
                        cached: false,
                    };
                    chatgpt_store::replace_chunks(db, &target, versions, &bodies.map(str::to_owned))?;
                }
                db.execute_batch(
                    "update search_chunks set chunk_index = chunk_index + 10 where conversation_id = 'z';
                     update search_chunks set chunk_index = 11 - chunk_index where conversation_id = 'z';",
                )?;
                let vectors: Vec<NewVector> =
                    chatgpt_store::pending_vectors(db, versions, MODEL_VERSION, 0, 100, 0)?
                        .into_iter()
                        .map(|chunk| NewVector {
                            chunk_id: chunk.id,
                            text: chunk.text,
                            embedding: same.clone(),
                        })
                        .collect();
                chatgpt_store::save_vectors(db, &vectors, MODEL_VERSION)?;
                Ok(())
            })
            .expect("index");
        let query = vec![1.0f32; DIM];
        let hits = store
            .read(|db| Ok(semantic_hits(db, &query, 10, None, versions).map_err(|f| f.message)))
            .expect("read")
            .expect("hits");
        let found: Vec<(&str, &str)> = hits
            .iter()
            .map(|hit| (hit.id.as_str(), hit.snippet.as_str()))
            .collect();
        assert_eq!(found, [("y", "y one"), ("z", "z two")]);
    }
}
