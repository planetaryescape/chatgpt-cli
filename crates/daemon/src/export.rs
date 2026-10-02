//! `export`/`show`: resolve a link, id or id prefix, fetch the chat now and
//! render it. Ported from the TS CLI's `src/commands/export.ts` and the
//! `selectOne` path of `src/commands/select.ts` @ 1b8c950, with its error
//! messages.
//!
//! Always the single-chat endpoint, as the TS CLI does, never the cached
//! transcript: the cache is filled from the batch endpoint, which names no
//! model, so its header would differ from the export's.

use chatgpt_core::ErrorKind;
use chatgpt_protocol::{ExportedChat, SessionChoice};
use serde_json::Value;

use crate::api::Api;
use crate::handlers::Failure;
use crate::js;
use crate::reads::NOT_SYNCED;
use crate::render::{Conversation, render_transcript};
use crate::state::State;

const UUID: &str = "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}";

fn invalid(message: String) -> Failure {
    Failure::new(ErrorKind::InvalidInput, message)
}

fn wrong_scope(archived: bool) -> &'static str {
    if archived {
        "archived; pass --archived or --all to include it"
    } else {
        "active; omit --archived or pass --all to include it"
    }
}

pub async fn export(
    state: &State,
    reference: String,
    archived: bool,
    all: bool,
    session: SessionChoice,
) -> Result<ExportedChat, Failure> {
    let _foreground = state.indexer.foreground();
    let (id, synced_at) = resolve(state, reference, archived, all).await?;
    let api = Api::new(std::sync::Arc::clone(&state.sessions), session);
    let path = format!("/backend-api/conversation/{id}");
    let value = api
        .conversation(&id)
        .await?
        .ok_or_else(|| Failure::new(ErrorKind::Api, format!("404 from {path}")))?;
    let is_archived = value.get("is_archived").is_some_and(truthy);
    if !all && is_archived != archived {
        return Err(invalid(format!(
            "Conversation is {}.",
            wrong_scope(is_archived)
        )));
    }
    let (markdown, title) = render(&value)?;
    Ok(ExportedChat {
        markdown,
        title,
        synced_at,
    })
}

/// `resolveId`: a link or full id needs no index; anything else is an id
/// prefix looked up in it. Also the index's `synced_at` when it was used.
async fn resolve(
    state: &State,
    reference: String,
    archived: bool,
    all: bool,
) -> Result<(String, Option<String>), Failure> {
    if reference.contains("/share/") {
        return Err(invalid(
            "Shared links (/share/…) aren't supported; use the chat's own /c/… link.".to_owned(),
        ));
    }
    let uuid = js::regex(UUID, true)
        .map_err(|error| Failure::new(ErrorKind::Internal, error.to_string()))?;
    if let Ok(Some(found)) = uuid.find(&reference) {
        return Ok((found.as_str().to_lowercase(), None));
    }
    let local_title_version = state.profile().local_title_version;
    state
        .db(move |db| {
            Ok(select_one(
                db,
                &reference,
                archived,
                all,
                local_title_version,
            ))
        })
        .await?
}

/// `selectTargets(index, [id], opts)[0]`.
fn select_one(
    db: &rusqlite::Connection,
    prefix: &str,
    archived: bool,
    all: bool,
    local_title_version: u32,
) -> Result<(String, Option<String>), Failure> {
    let synced_at = chatgpt_store::synced_at(db)
        .map_err(Failure::store)?
        .ok_or_else(|| Failure::new(ErrorKind::NotSynced, NOT_SYNCED))?;
    let matches = chatgpt_store::get(db, prefix, local_title_version).map_err(Failure::store)?;
    let target = match matches.as_slice() {
        [] => {
            return Err(invalid(format!(
                "No conversation matching \"{prefix}\" in the index. Run `chatgpt sync`?"
            )));
        }
        [one] => one,
        many => {
            return Err(invalid(format!(
                "\"{prefix}\" matches {} conversations; use a longer prefix.",
                many.len()
            )));
        }
    };
    if !all && target.is_archived != archived {
        return Err(invalid(format!(
            "Conversation \"{prefix}\" is {}.",
            wrong_scope(target.is_archived)
        )));
    }
    Ok((target.id.clone(), Some(synced_at)))
}

/// `Boolean(value)`.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// How a template literal prints a JSON value (`${value}`), for the header.
fn template(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_owned(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) => "null".to_owned(),
        Some(Value::Number(number)) => number
            .as_f64()
            .map_or_else(|| number.to_string(), chatgpt_core::js_number_string),
        Some(other) => other.to_string(),
    }
}

/// `renderTranscript` for the single-chat endpoint's answer, and the title
/// (for `-o`'s file name and `-c`'s note).
fn render(value: &Value) -> Result<(String, String), Failure> {
    let failed = |why: String| Failure::new(ErrorKind::Decode, why);
    let mut convo: Conversation = serde_json::from_value(value.clone()).map_err(|error| {
        failed(format!(
            "ChatGPT's conversation wasn't what the CLI expects ({:?} error)",
            error.classify()
        ))
    })?;
    // `new Date(create_time * 1000).toISOString()`: a missing or bad time
    // throws in the TS CLI too.
    let created = value
        .get("create_time")
        .and_then(Value::as_f64)
        .and_then(js::iso_from_seconds)
        .ok_or_else(|| failed("Invalid time value".to_owned()))?;
    let title = template(value.get("title"));
    convo.title = Some(title.clone());
    let id = template(value.get("conversation_id"));
    let markdown = render_transcript(&id, &convo, &created[..10]).map_err(failed)?;
    Ok((markdown, title))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_header_prints_values_as_a_template_literal_does() {
        let value = json!({
            "conversation_id": "c-1", "title": null, "create_time": 1_735_689_600.5,
            "default_model_slug": "gpt-5", "current_node": "m",
            "mapping": { "m": { "parent": null, "message": {
                "author": { "role": "user" }, "content": { "content_type": "text", "parts": ["hi"] } } } }
        });
        let (markdown, title) = render(&value).expect("render");
        assert_eq!(title, "null");
        assert_eq!(
            markdown,
            "# null\n\nhttps://chatgpt.com/c/c-1 · 2025-01-01 · gpt-5\n\n---\n\n## Me\n\nhi\n"
        );
        let bad = json!({ "conversation_id": "c", "mapping": {}, "current_node": null });
        assert_eq!(
            render(&bad).expect_err("no time").message,
            "Invalid time value"
        );
    }

    #[test]
    fn archive_state_is_read_as_js_truthiness() {
        assert!(!truthy(&Value::Null));
        assert!(!truthy(&json!(0)));
        assert!(truthy(&json!(true)));
        assert!(!truthy(&json!("")));
    }
}
