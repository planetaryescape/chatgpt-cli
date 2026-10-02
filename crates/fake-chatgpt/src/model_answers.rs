//! What the fake models answer, decided from the input alone so the TS CLI
//! and the Rust CLI get the same answers for the same requests. Shared by
//! the fake OpenAI and Anthropic APIs (`models.rs`) and by the fake `codex`
//! and `claude` the debug `chatgpt` binary plays (`crates/cli/src/fake_model.rs`
//! includes this file by path; it uses nothing but `serde_json`).
//!
//! - Luna's review: a title containing `junk` is a delete, `idea` a writing
//!   brainstorm (keep), anything else an archive.
//! - Luna's titles: `Luna: <original title>`, themed `fake theme`; an
//!   original title containing `notitle` gets an empty one (refused).
//! - Luna's memory review: a memory mentioning `tea` is kept, `move` sent
//!   for review, anything else deleted.
//! - A summary: `Summary of <the input's first line>`.

use serde_json::{Value, json};

/// Token usage every fake model call reports, so cost lines are the same
/// whatever order calls finish in.
pub const INPUT_TOKENS: u64 = 20_000;
pub const CACHED_INPUT_TOKENS: u64 = 1_000;
pub const OUTPUT_TOKENS: u64 = 300;
pub const REASONING_TOKENS: u64 = 100;
/// What the fake `claude -p` says a call cost.
pub const CLAUDE_COST_USD: f64 = 0.0123;

/// The summary of `input` (`Title: …` then the transcript).
pub fn summary(input: &str) -> String {
    let first = input.lines().next().unwrap_or_default();
    format!("Summary of {first}")
}

/// Luna's JSON for `input` under `schema`, by the schema's shape.
pub fn luna(schema: &Value, input: &Value) -> Value {
    let properties = &schema["properties"];
    if properties.get("items").is_some() {
        let items: Vec<Value> = input
            .as_array()
            .into_iter()
            .flatten()
            .map(|chat| {
                let original = chat["original_title"].as_str().unwrap_or_default();
                let title = if original.contains("notitle") {
                    String::new()
                } else {
                    format!("Luna: {original}")
                };
                json!({ "id": chat["id"], "title": title, "theme": "fake theme" })
            })
            .collect();
        return json!({ "items": items });
    }
    if properties.get("decisions").is_some() {
        let decisions: Vec<Value> = input["memories"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|memory| {
                let content = memory["content"].as_str().unwrap_or_default().to_lowercase();
                let suggestion = if content.contains("tea") {
                    "keep"
                } else if content.contains("move") {
                    "review"
                } else {
                    "delete"
                };
                json!({ "id": memory["id"], "suggestion": suggestion, "reason": "Fake memory reason." })
            })
            .collect();
        return json!({ "decisions": decisions });
    }
    let title = input["title"].as_str().unwrap_or_default().to_lowercase();
    let (suggestion, brainstorm, stage) = if title.contains("junk") {
        ("delete", Value::Null, "not_applicable")
    } else if title.contains("idea") {
        ("keep", json!("writing"), "not_applicable")
    } else {
        ("archive", Value::Null, "execution_or_explanation")
    };
    json!({
        "suggestion": suggestion,
        "brainstorm": brainstorm,
        "product_stage": stage,
        "reason": "Fake Luna reason.",
    })
}
