//! Conversation rendering, as far as the cache reconcile needs it: the
//! markdown the TS CLI caches for a chat and its turn count. Ported from the
//! TS CLI's `src/render/transcript.ts` @ 1b8c950 (RENDER_VERSION 2).
//!
//! The reconcile compares a fresh render with the cached one, so a
//! difference here only leaves a cache stale (the conservative side); it
//! never keeps one current wrongly unless both renders agree.

use std::collections::HashSet;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::js;

pub const SEPARATOR: &str = "\n\n---\n\n";

/// A full conversation from the batch endpoint, with what rendering reads.
#[derive(Clone, Debug, Deserialize)]
pub struct Conversation {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub mapping: Map<String, Value>,
    #[serde(default)]
    pub current_node: Option<String>,
    #[serde(default)]
    pub default_model_slug: Option<String>,
}

/// One message, as far as rendering reads it.
struct Message<'a>(&'a Value);

impl<'a> Message<'a> {
    fn role(&self) -> Option<&'a str> {
        self.0.get("author")?.get("role")?.as_str()
    }
    fn recipient(&self) -> Option<&'a str> {
        self.0.get("recipient")?.as_str()
    }
    fn content(&self) -> Option<&'a Value> {
        self.0.get("content")
    }
    fn content_type(&self) -> Option<&'a str> {
        self.content()?.get("content_type")?.as_str()
    }
    fn parts(&self) -> &'a [Value] {
        self.content()
            .and_then(|content| content.get("parts"))
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice)
    }
    fn metadata(&self) -> Option<&'a Value> {
        self.0.get("metadata")
    }
}

/// The visible thread: the path from `current_node` up to the root, in
/// order. Edits and regenerations branch the tree; this is the branch shown.
fn visible_thread(convo: &Conversation) -> Vec<Message<'_>> {
    let mut messages = Vec::new();
    let mut seen = HashSet::new();
    let mut node_id = convo.current_node.as_deref().filter(|id| !id.is_empty());
    while let Some(id) = node_id {
        // The TS CLI would loop forever on a cycle; stop instead.
        if !seen.insert(id) {
            break;
        }
        let Some(node) = convo.mapping.get(id) else {
            break;
        };
        if let Some(message) = node.get("message").filter(|message| !message.is_null()) {
            messages.push(Message(message));
        }
        node_id = node
            .get("parent")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty());
    }
    messages.reverse();
    messages
}

/// Plain strings, voice transcriptions, and `[image]` for image pointers.
fn part_text(part: &Value) -> Option<String> {
    match part {
        Value::String(text) => Some(text.clone()),
        Value::Object(typed) => match typed.get("content_type").and_then(Value::as_str) {
            Some("audio_transcription") => {
                typed.get("text").and_then(Value::as_str).map(Into::into)
            }
            Some("image_asset_pointer") => Some("[image]".into()),
            _ => None,
        },
        _ => None,
    }
}

fn is_image_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp", ".heic"]
        .iter()
        .any(|extension| lower.ends_with(extension))
}

/// Replace each content reference's next occurrence with its `alt` text,
/// from a moving cursor, skipping whitespace-only references; then drop
/// the leftover private-use markers.
fn resolve_references(text: &str, message: &Message<'_>) -> Result<String, String> {
    let mut out = text.to_owned();
    let mut cursor = 0;
    let references = message
        .metadata()
        .and_then(|metadata| metadata.get("content_references"))
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    for reference in references {
        let matched = reference
            .get("matched_text")
            .and_then(Value::as_str)
            .ok_or("a content reference has no matched_text")?;
        if js::trim(matched).is_empty() {
            continue;
        }
        let Some(found) = out.get(cursor..).and_then(|rest| rest.find(matched)) else {
            continue;
        };
        let at = cursor + found;
        let alt = reference.get("alt").and_then(Value::as_str).unwrap_or("");
        out.replace_range(at..at + matched.len(), alt);
        cursor = at + alt.len();
    }
    Ok(strip_markers(&out))
}

/// `/[^]*/g` → "".
fn strip_markers(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('\u{E200}') {
        let after = &rest[start + '\u{E200}'.len_utf8()..];
        match after.find('\u{E201}') {
            Some(end) => {
                out.push_str(&rest[..start]);
                rest = &after[end + '\u{E201}'.len_utf8()..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

fn message_text(message: &Message<'_>) -> Result<String, String> {
    let mut pieces: Vec<String> = message
        .parts()
        .iter()
        .filter_map(part_text)
        .filter(|text| !text.is_empty())
        .collect();
    let attachments = message
        .metadata()
        .and_then(|metadata| metadata.get("attachments"))
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    for attachment in attachments {
        let name = attachment
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("undefined");
        let image_mime = attachment
            .get("mime_type")
            .and_then(Value::as_str)
            .is_some_and(|mime| mime.starts_with("image/"));
        if !image_mime && !is_image_file(name) {
            pieces.push(format!("[attached file: {name}]"));
        }
    }
    Ok(js::trim(&resolve_references(&pieces.join("\n"), message)?).to_owned())
}

/// Image generation replies arrive as a tool message holding only the image.
fn is_generated_image(message: &Message<'_>) -> bool {
    message.role() == Some("tool")
        && message.content_type() == Some("multimodal_text")
        && message.parts().iter().any(|part| {
            part.get("content_type").and_then(Value::as_str) == Some("image_asset_pointer")
        })
}

/// `(role, text)` for each turn shown in the UI.
pub fn visible_turns(convo: &Conversation) -> Result<Vec<(&'static str, String)>, String> {
    let mut turns = Vec::new();
    for message in visible_thread(convo) {
        if is_generated_image(&message) {
            turns.push(("assistant", "[generated image]".to_owned()));
            continue;
        }
        let role = match message.role() {
            Some("user") => "user",
            Some("assistant") => "assistant",
            _ => continue,
        };
        let hidden = message
            .metadata()
            .and_then(|metadata| metadata.get("is_visually_hidden_from_conversation"))
            .is_some_and(|hidden| hidden.as_bool() == Some(true));
        if hidden {
            continue;
        }
        // Assistant messages addressed to a tool are tool calls, not replies.
        if role == "assistant"
            && message
                .recipient()
                .is_some_and(|to| !to.is_empty() && to != "all")
        {
            continue;
        }
        if !matches!(message.content_type(), Some("text" | "multimodal_text")) {
            continue;
        }
        let text = message_text(&message)?;
        if !text.is_empty() {
            turns.push((role, text));
        }
    }
    Ok(turns)
}

/// Canvas documents in their final state: `create_textdoc` carries a whole
/// document, `update_textdoc` regex edits to the last one.
fn final_canvases(convo: &Conversation) -> Vec<(String, String)> {
    let mut docs: Vec<(String, String)> = Vec::new();
    for message in visible_thread(convo) {
        let raw = message
            .content()
            .and_then(|content| content.get("text"))
            .and_then(Value::as_str)
            .or_else(|| message.parts().iter().find_map(Value::as_str));
        let Some(raw) = raw else {
            continue;
        };
        match message.recipient() {
            Some("canmore.create_textdoc") => {
                if let Ok(doc) = serde_json::from_str::<Value>(raw) {
                    let field = |name: &str| match doc.get(name) {
                        Some(Value::String(text)) => text.clone(),
                        Some(other) => other.to_string(),
                        None => "undefined".to_owned(),
                    };
                    docs.push((field("name"), field("content")));
                }
            }
            Some("canmore.update_textdoc") => {
                let Some(current) = docs.last_mut() else {
                    continue;
                };
                let Ok(edit) = serde_json::from_str::<Value>(raw) else {
                    continue;
                };
                let updates = edit.get("updates").and_then(Value::as_array);
                for update in updates.into_iter().flatten() {
                    // A failing edit stops this message's edits, keeping
                    // the ones already applied, as the TS CLI's catch does.
                    if apply_update(&mut current.1, update).is_none() {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    docs
}

/// One canvas edit: `.*` replaces the document, anything else is a regex
/// whose matches become the replacement text literally.
fn apply_update(content: &mut String, update: &Value) -> Option<()> {
    let pattern = update.get("pattern")?.as_str()?;
    let replacement = update.get("replacement")?.as_str()?;
    if pattern == ".*" {
        *content = replacement.to_owned();
        return Some(());
    }
    let regex = js::regex(pattern, false).ok()?;
    let literal = fancy_regex::NoExpand(replacement);
    let multiple = update.get("multiple").and_then(Value::as_bool) == Some(true);
    *content = if multiple {
        regex.try_replacen(content, 0, literal).ok()?.into_owned()
    } else {
        regex.try_replacen(content, 1, literal).ok()?.into_owned()
    };
    Some(())
}

/// `renderTranscript` with `create_time` already formatted as
/// `YYYY-MM-DD`.
pub fn render_transcript(id: &str, convo: &Conversation, date: &str) -> Result<String, String> {
    let header = format!(
        "# {}\n\nhttps://chatgpt.com/c/{id} · {date} · {}",
        convo.title.as_deref().unwrap_or("undefined"),
        convo
            .default_model_slug
            .as_deref()
            .unwrap_or("unknown model")
    );
    let mut sections = vec![header];
    for (role, text) in visible_turns(convo)? {
        let who = if role == "user" { "Me" } else { "ChatGPT" };
        sections.push(format!("## {who}\n\n{text}"));
    }
    for (name, content) in final_canvases(convo) {
        sections.push(format!("## Canvas (final): {name}\n\n{content}"));
    }
    Ok(format!("{}\n", sections.join(SEPARATOR)))
}

/// `toCachedTranscript` for a batch item: the cache row for chat `id` as
/// fetched, under the index's `update_time` (what cache lookups compare
/// against). The batch names no model, so the header says "unknown model".
pub fn cached_transcript(
    item: &crate::api::BatchItem,
    update_time: &str,
    render_version: u32,
) -> Result<chatgpt_store::Transcript, String> {
    // `Date.parse(create_time) / 1000`, then `toISOString().slice(0, 10)`.
    let created = item
        .create_time
        .as_deref()
        .and_then(js::parse_date)
        .and_then(js::iso_from_millis)
        .ok_or("Invalid time value")?;
    let markdown = render_transcript(&item.id, &item.conversation, &created[..10])?;
    let turns = visible_turns(&item.conversation)?.len();
    // `Math.ceil(markdown.length / 4)`, in UTF-16 units as JS counts.
    let approx_tokens = markdown.encode_utf16().count().div_ceil(4);
    Ok(chatgpt_store::Transcript {
        id: item.id.clone(),
        update_time: update_time.to_owned(),
        render_version,
        markdown,
        turns: i64::try_from(turns).unwrap_or(i64::MAX),
        approx_tokens: i64::try_from(approx_tokens).unwrap_or(i64::MAX),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn convo(mapping: Value, current: &str) -> Conversation {
        serde_json::from_value(json!({
            "title": "Idea", "mapping": mapping, "current_node": current, "default_model_slug": null
        }))
        .expect("conversation")
    }

    fn node(parent: Option<&str>, message: Value) -> Value {
        json!({ "parent": parent, "children": [], "message": message })
    }

    #[test]
    fn renders_the_visible_branch_with_references_files_and_canvases() {
        let mapping = json!({
            "root": node(None, Value::Null),
            "u1": node(Some("root"), json!({
                "author": {"role": "user"}, "content": {"content_type": "text", "parts": ["Hello", ""]},
                "metadata": {"attachments": [{"name": "notes.pdf", "mime_type": null}, {"name": "pic.PNG"}]}
            })),
            "a-old": node(Some("u1"), json!({
                "author": {"role": "assistant"}, "content": {"content_type": "text", "parts": ["old branch"]}
            })),
            "a1": node(Some("u1"), json!({
                "author": {"role": "assistant"},
                "content": {"content_type": "text", "parts": ["See \u{E200}cite\u{E202}x\u{E201} and here and here"]},
                "metadata": {"content_references": [
                    {"matched_text": " ", "alt": ""},
                    {"matched_text": "here", "alt": "[link](u)"}
                ]}
            })),
            "tool": node(Some("a1"), json!({
                "author": {"role": "assistant"}, "recipient": "canmore.create_textdoc",
                "content": {"content_type": "code", "text": "{\"name\":\"Doc\",\"content\":\"one two two\"}"}
            })),
            "edit": node(Some("tool"), json!({
                "author": {"role": "assistant"}, "recipient": "canmore.update_textdoc",
                "content": {"content_type": "code", "text": "{\"updates\":[{\"pattern\":\"two\",\"replacement\":\"$1\",\"multiple\":true}]}"}
            })),
            "img": node(Some("edit"), json!({
                "author": {"role": "tool"},
                "content": {"content_type": "multimodal_text", "parts": [{"content_type": "image_asset_pointer"}]}
            })),
        });
        let rendered =
            render_transcript("id-1", &convo(mapping, "img"), "2026-09-01").expect("render");
        assert_eq!(
            rendered,
            "# Idea\n\nhttps://chatgpt.com/c/id-1 · 2026-09-01 · unknown model\n\n---\n\n\
             ## Me\n\nHello\n[attached file: notes.pdf]\n\n---\n\n\
             ## ChatGPT\n\nSee  and [link](u) and here\n\n---\n\n\
             ## ChatGPT\n\n[generated image]\n\n---\n\n\
             ## Canvas (final): Doc\n\none $1 $1\n"
        );
    }

    #[test]
    fn canvas_edits_use_js_regex_semantics() {
        let mut content = "café menu".to_owned();
        let edit = json!({ "pattern": r"\w+", "replacement": "X", "multiple": true });
        apply_update(&mut content, &edit).expect("applied");
        // JS's \w stops at é.
        assert_eq!(content, "Xé X");
    }

    #[test]
    fn a_cycle_in_the_tree_ends_the_thread() {
        let mapping = json!({
            "a": node(Some("b"), json!({"author": {"role": "user"}, "content": {"content_type": "text", "parts": ["x"]}})),
            "b": node(Some("a"), Value::Null),
        });
        assert_eq!(visible_turns(&convo(mapping, "a")).expect("turns").len(), 1);
    }
}
