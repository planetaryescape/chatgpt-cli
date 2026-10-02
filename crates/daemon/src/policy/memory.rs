//! Saved-memory counts for `stats`, from the TS CLI's cached
//! classifications. Ported from `memoryCounts`, `relatedMemories`,
//! `inputHash`, `quickDecision` and `finalDecision` in the TS CLI's
//! `src/classify/memories.ts` @ 1b8c950. A cached row counts only for the
//! exact input it was made for: the memory's text and update time, its
//! related memories and the month.

use std::collections::HashSet;
use std::sync::LazyLock;

use chatgpt_protocol::MemoryCounts;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A saved memory as `GET /backend-api/memories` returns it, with the
/// fields the counts need.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SavedMemory {
    pub id: String,
    pub content: String,
    #[serde(default)]
    pub updated_at: Option<String>,
}

const STOP_WORDS: &[&str] = &[
    "that", "this", "with", "from", "their", "they", "them", "user", "wants", "would", "have",
    "been", "about", "some", "into", "after", "before",
];

#[allow(clippy::unwrap_used, reason = "a constant pattern")]
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\p{L}\p{N}]{4,}").unwrap());

fn words(text: &str) -> HashSet<String> {
    WORD.find_iter(&text.to_lowercase())
        .map(|word| word.as_str().to_owned())
        .filter(|word| !STOP_WORDS.contains(&word.as_str()))
        .collect()
}

/// Up to three other memories sharing at least a quarter of their words
/// with `target`, closest first.
#[cfg(test)]
fn related_memories<'a>(target: &SavedMemory, all: &'a [SavedMemory]) -> Vec<&'a SavedMemory> {
    let sets: Vec<HashSet<String>> = all.iter().map(|memory| words(&memory.content)).collect();
    related_with(&words(&target.content), target, all, &sets)
}

/// [`related_memories`] with every memory's words already split, so a pass
/// over all memories splits each once rather than once per pair.
fn related_with<'a>(
    left: &HashSet<String>,
    target: &SavedMemory,
    all: &'a [SavedMemory],
    sets: &[HashSet<String>],
) -> Vec<&'a SavedMemory> {
    let mut scored: Vec<(&SavedMemory, f64)> = all
        .iter()
        .zip(sets)
        .filter(|(memory, _)| memory.id != target.id)
        .map(|(memory, right)| {
            let shared = left.iter().filter(|word| right.contains(*word)).count();
            let size = left.len().max(right.len()).max(1);
            (memory, shared as f64 / size as f64)
        })
        .filter(|(_, score)| *score >= 0.25)
        .collect();
    // Stable, as Array.prototype.sort is: equal scores keep list order.
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored
        .into_iter()
        .take(3)
        .map(|(memory, _)| memory)
        .collect()
}

/// `inputHash`: sha256 of the same JSON the TS CLI hashes, byte for byte
/// (JSON.stringify and serde_json escape strings the same way).
pub fn input_hash(memory: &SavedMemory, related: &[&SavedMemory], as_of: &str) -> String {
    #[derive(Serialize)]
    struct Input<'a> {
        content: &'a str,
        // JSON.stringify leaves an undefined property out.
        #[serde(skip_serializing_if = "Option::is_none")]
        updated: Option<&'a str>,
        related: Vec<(&'a str, &'a str)>,
        month: &'a str,
    }
    let input = Input {
        content: &memory.content,
        updated: memory.updated_at.as_deref(),
        related: related
            .iter()
            .map(|memory| (memory.id.as_str(), memory.content.as_str()))
            .collect(),
        month: as_of.get(..7).unwrap_or(as_of),
    };
    let json = serde_json::to_string(&input).unwrap_or_default();
    Sha256::digest(json.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Deserialize, Default)]
struct Scored {
    score: Option<f64>,
    confidence: Option<f64>,
}

#[derive(Deserialize, Default)]
struct Noul {
    noul: Option<f64>,
}

#[derive(Deserialize)]
struct MemoryAnswers {
    #[serde(default)]
    lasting_value: Scored,
    #[serde(default)]
    expired: Noul,
    #[serde(default)]
    superseded: Noul,
    #[serde(default)]
    redundant: Noul,
}

#[derive(Deserialize)]
struct DeepAnswer {
    suggestion: String,
    #[serde(default)]
    reason: String,
}

/// `undefined` comparisons are false in JS; NaN compares false here too.
fn value(value: Option<f64>) -> f64 {
    value.unwrap_or(f64::NAN)
}

fn quick_decision(a: &MemoryAnswers) -> &'static str {
    let keep = value(a.lasting_value.score) >= 2.5
        && value(a.lasting_value.confidence) >= 0.65
        && value(a.expired.noul) < 0.25
        && value(a.superseded.noul) < 0.25
        && value(a.redundant.noul) < 0.25;
    if keep { "keep" } else { "review" }
}

fn final_decision(a: &MemoryAnswers, deep: &DeepAnswer) -> String {
    let conflicting = deep.suggestion == "delete"
        && value(a.lasting_value.score) >= 2.3
        && value(a.expired.noul) < 0.5
        && value(a.redundant.noul) < 0.6
        && value(a.superseded.noul) < 0.6;
    if conflicting {
        "review".into()
    } else {
        deep.suggestion.clone()
    }
}

/// `MemoryDecision`: what `memory classify` shows for a memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    pub suggestion: String,
    pub reason: String,
    /// `quick` (Jev alone) or `deep` (Luna reviewed it).
    pub stage: &'static str,
}

/// `quickDecision`, or `finalDecision` once Luna reviewed it.
pub fn decide(system_one: &str, system_two: Option<&str>) -> Result<Decision, String> {
    let answers: MemoryAnswers =
        serde_json::from_str(system_one).map_err(|error| error.to_string())?;
    let Some(deep) = system_two else {
        return Ok(match quick_decision(&answers) {
            "keep" => Decision {
                suggestion: "keep".into(),
                reason: "Enduring context with no clear expiry, replacement, or full duplicate."
                    .into(),
                stage: "quick",
            },
            _ => Decision {
                suggestion: "review".into(),
                reason: "Needs a closer look at currency, usefulness, or overlap.".into(),
                stage: "quick",
            },
        });
    };
    let deep: DeepAnswer = serde_json::from_str(deep).map_err(|error| error.to_string())?;
    let suggestion = final_decision(&answers, &deep);
    let reason = if suggestion == deep.suggestion {
        deep.reason
    } else {
        "Quick and deep reviews disagree about lasting value; check this memory manually.".into()
    };
    Ok(Decision {
        suggestion,
        reason,
        stage: "deep",
    })
}

/// For each memory, the indexes of its related ones (`relatedMemories`),
/// closest first.
pub fn related_indexes(memories: &[SavedMemory]) -> Vec<Vec<usize>> {
    let sets: Vec<HashSet<String>> = memories
        .iter()
        .map(|memory| words(&memory.content))
        .collect();
    memories
        .iter()
        .zip(&sets)
        .map(|(memory, left)| {
            related_with(left, memory, memories, &sets)
                .into_iter()
                .filter_map(|related| {
                    memories
                        .iter()
                        .position(|candidate| std::ptr::eq(candidate, related))
                })
                .collect()
        })
        .collect()
}

/// A cached classification: Jev's quick answers, and Luna's decision if it
/// reviewed the memory.
#[derive(Clone)]
pub struct Cached {
    pub system_one: String,
    pub system_two: Option<String>,
}

/// `memoryCounts`. `cached` looks a memory's classification up by id and
/// input hash. A memory only Jev looked at and didn't keep counts as not
/// classified: it still waits for Luna.
pub fn memory_counts(
    memories: &[SavedMemory],
    as_of: &str,
    mut cached: impl FnMut(&str, &str) -> Result<Option<Cached>, String>,
) -> Result<MemoryCounts, String> {
    let mut counts = MemoryCounts {
        total: memories.len() as u64,
        ..MemoryCounts::default()
    };
    let sets: Vec<HashSet<String>> = memories
        .iter()
        .map(|memory| words(&memory.content))
        .collect();
    for (memory, left) in memories.iter().zip(&sets) {
        let related = related_with(left, memory, memories, &sets);
        let Some(row) = cached(&memory.id, &input_hash(memory, &related, as_of))? else {
            counts.unclassified += 1;
            continue;
        };
        let answers: MemoryAnswers = serde_json::from_str(&row.system_one)
            .map_err(|error| format!("saved memory {}: {error}", memory.id))?;
        let deep = row
            .system_two
            .as_deref()
            .map(serde_json::from_str::<DeepAnswer>)
            .transpose()
            .map_err(|error| format!("saved memory {}: {error}", memory.id))?;
        let decision = match &deep {
            Some(deep) => final_decision(&answers, deep),
            None => quick_decision(&answers).to_owned(),
        };
        match decision.as_str() {
            "review" if deep.is_none() => counts.unclassified += 1,
            "keep" => counts.keep += 1,
            "delete" => counts.delete += 1,
            "review" => counts.review += 1,
            _ => {}
        }
    }
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn memory(id: &str, content: &str) -> SavedMemory {
        SavedMemory {
            id: id.into(),
            content: content.into(),
            updated_at: Some("2026-09-28T14:00:00Z".into()),
        }
    }

    fn answers(score: f64, expired: f64, redundant: f64) -> String {
        format!(
            r#"{{"lasting_value":{{"type":"score","score":{score},"confidence":0.9}},"expired":{{"type":"noul","noul":{expired}}},"superseded":{{"type":"noul","noul":0}},"redundant":{{"type":"noul","noul":{redundant}}}}}"#
        )
    }

    fn parse(json: &str) -> MemoryAnswers {
        serde_json::from_str(json).expect("answers")
    }

    // Ported from the TS CLI's src/classify/memories.test.ts.
    #[test]
    fn quick_stage_only_keeps_strong_enduring_context() {
        assert_eq!(quick_decision(&parse(&answers(3.0, 0.0, 0.0))), "keep");
        assert_eq!(quick_decision(&parse(&answers(0.0, 0.99, 0.0))), "review");
        assert_eq!(quick_decision(&parse(&answers(3.0, 0.0, 0.7))), "review");
    }

    #[test]
    fn conflicting_lasting_value_evidence_prevents_a_deep_delete() {
        let delete = DeepAnswer {
            suggestion: "delete".into(),
            reason: "passed".into(),
        };
        assert_eq!(
            final_decision(&parse(&answers(2.5, 0.0, 0.0)), &delete),
            "review"
        );
        assert_eq!(
            final_decision(&parse(&answers(2.5, 0.0, 0.8)), &delete),
            "delete"
        );
    }

    #[test]
    fn related_memories_expose_strong_overlap_but_not_a_shared_topic() {
        let a = memory("a", "A user writes a blog about things they learn");
        let b = memory(
            "b",
            "A user likes writing blog posts about things they learn",
        );
        let c = memory("c", "A user is growing tomatoes in a garden");
        let all = [a.clone(), b, c.clone()];
        let ids: Vec<&str> = related_memories(&a, &all)
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(ids, ["b"]);
        let long = memory(
            "long",
            &format!(
                "garden planting  {}",
                "unrelated fantasy details ".repeat(80)
            ),
        );
        assert!(related_memories(&c, &[c.clone(), long]).is_empty());
    }

    /// The hash the TS CLI computes for the same input (bun, 2026-10-01):
    /// `createHash("sha256").update(JSON.stringify({content, updated,
    /// related, month})).digest("hex")`.
    #[test]
    fn the_input_hash_matches_the_ts_clis() {
        let target = memory("m1", "The user prefers \"dark\" mode\n\tand ünïcode");
        let other = memory("m2", "Another fact");
        assert_eq!(
            input_hash(&target, &[&other], "2026-09-28"),
            "469cdcc5d5f828d96c2dd261753884c3b7092532b31a57f7052cf6b6640db5d3"
        );
    }

    #[test]
    fn counts_follow_the_cached_decisions() {
        let rows = [
            memory("lasting", "The user prefers dark mode"),
            memory("temporary", "The user is preparing tomorrow's event"),
        ];
        let as_of = "2026-09-28";
        let mut cache: HashMap<(String, String), Cached> = HashMap::new();
        let hash =
            |m: &SavedMemory, all: &[SavedMemory]| input_hash(m, &related_memories(m, all), as_of);
        cache.insert(
            ("lasting".into(), hash(&rows[0], &rows)),
            Cached {
                system_one: answers(3.0, 0.0, 0.0),
                system_two: None,
            },
        );
        cache.insert(
            ("temporary".into(), hash(&rows[1], &rows)),
            Cached {
                system_one: answers(0.0, 0.95, 0.0),
                system_two: Some(
                    r#"{"id":"temporary","suggestion":"delete","reason":"passed"}"#.into(),
                ),
            },
        );
        fn lookup(
            cache: &HashMap<(String, String), Cached>,
        ) -> impl FnMut(&str, &str) -> Result<Option<Cached>, String> + '_ {
            move |id, hash| Ok(cache.get(&(id.to_owned(), hash.to_owned())).cloned())
        }
        assert_eq!(
            memory_counts(&rows, as_of, lookup(&cache)).expect("counts"),
            MemoryCounts {
                total: 2,
                keep: 1,
                delete: 1,
                review: 0,
                unclassified: 0
            }
        );
        let changed = [rows[0].clone(), memory("new", "A new fact")];
        assert_eq!(
            memory_counts(&changed, as_of, lookup(&cache))
                .expect("counts")
                .unclassified,
            1
        );
    }
}
