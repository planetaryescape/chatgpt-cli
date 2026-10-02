//! Jev's questions, as `JSON.stringify` sends them:
//!
//! - [`chats`]: `QUESTIONS` from `src/classify/questions.ts`;
//! - [`deep`]: `DEEP_QUESTIONS` from `deep-questions.ts`, the follow-up
//!   for chats the first pass left unsure;
//! - [`memories`]: `MEMORY_QUESTIONS` from `memories.ts`;
//!
//! all from the TS CLI @ 1b8c950, written out with
//! `bun -e 'import {QUESTIONS} from "./src/classify/questions.ts"; console.log(JSON.stringify(QUESTIONS, null, 2))'`
//! (and likewise for the others). The TS parity harnesses check both CLIs
//! send the same request bodies.
//!
//! Changing a question means a new version in `Profile::builtin`
//! (`questions_version`, `deep_questions_version`, `memory_version`).

use std::sync::LazyLock;

use serde_json::Value;

fn parse(text: &str) -> Value {
    #[allow(clippy::unwrap_used, reason = "fixed files that a test parses")]
    serde_json::from_str(text).unwrap()
}

static CHATS: LazyLock<Value> = LazyLock::new(|| parse(include_str!("questions.json")));
static DEEP: LazyLock<Value> = LazyLock::new(|| parse(include_str!("deep_questions.json")));
static MEMORIES: LazyLock<Value> = LazyLock::new(|| parse(include_str!("memory_questions.json")));

pub fn chats() -> &'static Value {
    &CHATS
}

pub fn deep() -> &'static Value {
    &DEEP
}

pub fn memories() -> &'static Value {
    &MEMORIES
}

/// Room for a float's last bit at a range's ends.
const SLACK: f64 = 1e-9;

fn number_in(value: Option<&Value>, low: f64, high: f64) -> bool {
    value
        .and_then(Value::as_f64)
        .is_some_and(|n| n >= low - SLACK && n <= high + SLACK)
}

/// Whether Jev's `answers` to `questions` have the shape every question
/// asks for: a probability for a yes/no question, a score in the rubric's
/// range with a confidence, one of the named choices. An answer that
/// doesn't is never saved. The error names the question and what it
/// should be, never the value, which could be anything Jev echoed.
pub fn validate(questions: &Value, answers: &Value) -> Result<(), String> {
    let Some(questions) = questions.as_object() else {
        return Ok(());
    };
    for (name, question) in questions {
        let answer = answers.get(name).filter(|answer| answer.is_object());
        let top = question["criteria"]
            .as_array()
            .map_or(0, Vec::len)
            .saturating_sub(1);
        let fits = match question["type"].as_str() {
            Some("noul") => number_in(answer.and_then(|a| a.get("noul")), 0.0, 1.0),
            Some("score") => {
                number_in(answer.and_then(|a| a.get("score")), 0.0, top as f64)
                    && number_in(answer.and_then(|a| a.get("confidence")), 0.0, 1.0)
            }
            Some("choice") => answer
                .and_then(|a| a.get("choice"))
                .and_then(Value::as_str)
                .is_some_and(|choice| question["criteria"].get(choice).is_some()),
            _ => true,
        };
        if !fits {
            let expected = match question["type"].as_str() {
                Some("noul") => "a probability from 0 to 1".to_owned(),
                Some("score") => format!("a score from 0 to {top} with a confidence"),
                _ => "one of its choices".to_owned(),
            };
            return Err(format!(
                "Jev's answer to {name} isn't {expected}; nothing was saved"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(questions: &Value) -> Vec<&str> {
        questions
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect()
    }

    #[test]
    fn the_questions_are_the_ts_clis_sets() {
        assert_eq!(
            names(chats()),
            [
                "worth_keeping",
                "nothing_there",
                "unfinished",
                "personal_record",
                "re_askable",
                "time_bound",
                "overtaken_by_time",
                "brainstorming",
                "product_idea",
                "brainstorm_for",
                "topic"
            ]
        );
        let topics: Vec<&str> = chats()["topic"]["criteria"]
            .as_object()
            .expect("topics")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(topics, crate::policy::Profile::builtin().topics);
        assert_eq!(
            names(deep()),
            [
                "personal_record_lost",
                "reusable_artifact_lost",
                "original_thinking_lost",
                "work_to_resume",
                "creative_idea_lost",
                "reaskable_without_loss",
                "worth_finding_again"
            ]
        );
        assert_eq!(
            names(memories()),
            ["lasting_value", "expired", "superseded", "redundant"]
        );
    }

    /// The files are this repository's TS questions: a change to either
    /// side fails here (bridge-era: the TS sources go at stage 6).
    #[test]
    fn the_files_match_this_repositorys_ts_sources() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/classify");
        for (file, constant) in [
            ("questions.ts", "QUESTIONS_VERSION = \"2026-09-28.9\""),
            (
                "deep-questions.ts",
                "DEEP_QUESTIONS_VERSION = \"2026-09-27.2\"",
            ),
            (
                "memories.ts",
                "MEMORY_CLASSIFICATION_VERSION = \"2026-09-28.5\"",
            ),
        ] {
            let Ok(source) = std::fs::read_to_string(src.join(file)) else {
                return;
            };
            assert!(source.contains(constant), "{file} changed its version");
        }
        let profile = crate::policy::Profile::builtin();
        assert_eq!(profile.questions_version, "2026-09-28.9");
        assert_eq!(profile.deep_questions_version, "2026-09-27.2");
        assert_eq!(profile.memory_version, "2026-09-28.5");
    }

    #[test]
    fn answers_of_the_wrong_shape_are_named_without_their_value() {
        let good = fake_answers();
        assert_eq!(validate(chats(), &good), Ok(()));
        for (path, bad) in [
            (
                "/worth_keeping/score",
                serde_json::json!("SENTINEL fragment"),
            ),
            ("/worth_keeping/score", serde_json::json!(3.5)),
            ("/nothing_there/noul", serde_json::json!(1.2)),
            ("/topic/choice", serde_json::json!("SENTINEL topic")),
            ("/re_askable", serde_json::json!(null)),
        ] {
            let mut answers = good.clone();
            *answers.pointer_mut(path).expect("path") = bad;
            let error = validate(chats(), &answers).expect_err("refused");
            assert!(!error.contains("SENTINEL"), "{error}");
            assert!(error.starts_with("Jev's answer to "), "{error}");
        }
        let mut missing = good;
        missing
            .as_object_mut()
            .expect("object")
            .remove("brainstorm_for");
        assert_eq!(
            validate(chats(), &missing),
            Err(
                "Jev's answer to brainstorm_for isn't one of its choices; nothing was saved"
                    .to_owned()
            )
        );
        let memory = serde_json::json!({
            "lasting_value": { "score": 3.0, "confidence": 0.8 },
            "expired": { "noul": 0.1 }, "superseded": { "noul": 0.1 }, "redundant": { "noul": 2 },
        });
        assert_eq!(
            validate(memories(), &memory),
            Err(
                "Jev's answer to redundant isn't a probability from 0 to 1; nothing was saved"
                    .to_owned()
            )
        );
    }

    fn fake_answers() -> Value {
        let noul = |p: f64| serde_json::json!({ "type": "noul", "noul": p });
        serde_json::json!({
            "worth_keeping": { "score": 3.0, "confidence": 0.9 },
            "nothing_there": noul(0.0), "unfinished": noul(1.0), "personal_record": noul(0.2),
            "re_askable": noul(0.5), "time_bound": noul(0.1), "overtaken_by_time": noul(0.1),
            "brainstorming": noul(0.9), "product_idea": noul(1e-7),
            "brainstorm_for": { "choice": "none", "confidence": 0.8 },
            "topic": { "choice": "other" },
        })
    }
}
