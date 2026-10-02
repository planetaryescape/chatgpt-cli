//! Jev's questions for a chat: `QUESTIONS` from the TS CLI's
//! `src/classify/questions.ts` @ 1b8c950, as `JSON.stringify` sends them
//! (`questions.json`, written from it with
//! `bun -e 'import {QUESTIONS} from "./src/classify/questions.ts"; …'`).
//! The TS parity harness checks both CLIs send the same request body.
//!
//! Changing a question means a new `QUESTIONS_VERSION` in `Profile::builtin`.

use std::sync::LazyLock;

use serde_json::Value;

static QUESTIONS: LazyLock<Value> = LazyLock::new(|| {
    #[allow(clippy::unwrap_used, reason = "a fixed file that a test parses")]
    serde_json::from_str(include_str!("questions.json")).unwrap()
});

pub fn questions() -> &'static Value {
    &QUESTIONS
}

/// Room for a float's last bit at a range's ends.
const SLACK: f64 = 1e-9;

fn number_in(value: Option<&Value>, low: f64, high: f64) -> bool {
    value
        .and_then(Value::as_f64)
        .is_some_and(|n| n >= low - SLACK && n <= high + SLACK)
}

/// Whether Jev's `answers` have the shape every question asks for: a
/// probability for a yes/no question, a score in the rubric's range with a
/// confidence, one of the named choices. An answer that doesn't is never
/// saved. The error names the question and what it should be, never the
/// value, which could be anything Jev echoed.
pub fn validate(answers: &Value) -> Result<(), String> {
    let Some(questions) = QUESTIONS.as_object() else {
        return Ok(());
    };
    for (name, question) in questions {
        let answer = answers.get(name).filter(|answer| answer.is_object());
        let fits = match question["type"].as_str() {
            Some("noul") => number_in(answer.and_then(|a| a.get("noul")), 0.0, 1.0),
            Some("score") => {
                let top = question["criteria"]
                    .as_array()
                    .map_or(0, Vec::len)
                    .saturating_sub(1) as f64;
                number_in(answer.and_then(|a| a.get("score")), 0.0, top)
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
                Some("score") => format!(
                    "a score from 0 to {} with a confidence",
                    question["criteria"]
                        .as_array()
                        .map_or(0, Vec::len)
                        .saturating_sub(1)
                ),
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

    #[test]
    fn the_questions_are_the_ts_clis_set() {
        let questions = questions().as_object().expect("an object");
        let names: Vec<&str> = questions.keys().map(String::as_str).collect();
        assert_eq!(
            names,
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
        let topics: Vec<&str> = questions["topic"]["criteria"]
            .as_object()
            .expect("topics")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(topics, crate::policy::Profile::builtin().topics);
    }

    #[test]
    fn answers_of_the_wrong_shape_are_named_without_their_value() {
        let good = fake_chatgpt_like_answers();
        assert_eq!(validate(&good), Ok(()));
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
            let error = validate(&answers).expect_err("refused");
            assert!(!error.contains("SENTINEL"), "{error}");
            assert!(error.starts_with("Jev's answer to "), "{error}");
        }
        let mut missing = good;
        missing
            .as_object_mut()
            .expect("object")
            .remove("brainstorm_for");
        assert_eq!(
            validate(&missing),
            Err(
                "Jev's answer to brainstorm_for isn't one of its choices; nothing was saved"
                    .to_owned()
            )
        );
    }

    fn fake_chatgpt_like_answers() -> Value {
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
