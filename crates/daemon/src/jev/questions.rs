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
}
