//! `export`/`show`: resolve a link, id or id prefix, fetch the chat now and
//! render it. Ported from the TS CLI's `src/commands/export.ts` and the
//! `selectOne` path of `src/commands/select.ts` @ 1b8c950, with its error
//! messages.
//!
//! Always the single-chat endpoint, as the TS CLI does, never the cached
//! transcript: the cache is filled from the batch endpoint, which names no
//! model, so its header would differ from the export's.

use std::sync::LazyLock;

use chatgpt_core::ErrorKind;
use chatgpt_protocol::{ExportedChat, SessionChoice};
use serde::Deserialize;
use serde_json::Value;

use crate::api::Api;
use crate::handlers::Failure;
use crate::js;
use crate::reads::require_synced;
use crate::render::{Conversation, render_transcript};
use crate::state::State;

/// `/[0-9a-f]{8}-…-[0-9a-f]{12}/i`, through `js::regex` like every JS regex.
static UUID: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    #[allow(clippy::unwrap_used, reason = "a fixed pattern that compiles")]
    js::regex(
        "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
        true,
    )
    .unwrap()
});

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
    let is_archived = value.get("is_archived").is_some_and(js::truthy);
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
    if let Ok(Some(found)) = UUID.find(&reference) {
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
    let synced_at = require_synced(db)?;
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

/// `renderTranscript` for the single-chat endpoint's answer, and the title
/// (for `-o`'s file name and `-c`'s note).
fn render(value: &Value) -> Result<(String, String), Failure> {
    let failed = |why: String| Failure::new(ErrorKind::Decode, why);
    let convo = Conversation::deserialize(value).map_err(|error| {
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
    let id = js::template(value.get("conversation_id"));
    let markdown = render_transcript(&id, &convo, &created[..10]).map_err(failed)?;
    Ok((markdown, convo.title))
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
        assert!(!js::truthy(&Value::Null));
        assert!(!js::truthy(&json!(0)));
        assert!(js::truthy(&json!(true)));
        assert!(!js::truthy(&json!("")));
    }
}
