//! Conversation trees shaped like chatgpt.com's (observed 2026-09-27):
//! regenerated and edited branches, hidden and system messages, tool calls,
//! canvases built by create and update edits, web citations, file and image
//! attachments, voice transcriptions, generated images, and text in many
//! scripts, with emoji placed where a 800-unit chunk cut splits them. Tests
//! render, export and search them, and compare with the TS CLI.

use serde_json::{Map, Value, json};

use crate::Chat;

/// Builds a `mapping` tree. Messages hang off the last one added unless a
/// branch point is given.
pub struct Tree {
    nodes: Map<String, Value>,
    last: String,
    count: usize,
}

impl Default for Tree {
    fn default() -> Self {
        Self::new()
    }
}

impl Tree {
    /// A tree with its root node, which has no message, as real ones do.
    pub fn new() -> Self {
        let mut nodes = Map::new();
        nodes.insert(
            "root".into(),
            json!({ "id": "root", "parent": null, "children": [], "message": null }),
        );
        Self {
            nodes,
            last: "root".into(),
            count: 0,
        }
    }

    /// Add `message` under the last node; returns its id.
    pub fn push(&mut self, message: Value) -> String {
        let parent = self.last.clone();
        self.push_under(&parent, message)
    }

    /// Add `message` under `parent` (a regeneration or an edit when
    /// `parent` already has a child); it becomes the last node. A parent
    /// the tree doesn't have is a mistake in the fixture, so it panics
    /// rather than leave an orphan.
    pub fn push_under(&mut self, parent: &str, message: Value) -> String {
        self.count += 1;
        let id = format!("n{:03}", self.count);
        self.nodes
            .get_mut(parent)
            .and_then(|node| node.get_mut("children"))
            .and_then(Value::as_array_mut)
            .expect("push_under: no such parent in the tree")
            .push(json!(id));
        self.nodes.insert(
            id.clone(),
            json!({ "id": id, "parent": parent, "children": [], "message": message }),
        );
        self.last = id.clone();
        id
    }

    pub fn last(&self) -> &str {
        &self.last
    }

    /// The `mapping` and `current_node` (the last node added).
    pub fn finish(self) -> (Value, String) {
        (Value::Object(self.nodes), self.last)
    }
}

fn message(role: &str, content: Value, extra: Value) -> Value {
    let mut message = json!({
        "author": { "role": role, "name": null },
        "create_time": 1_700_000_000.0,
        "content": content,
        "metadata": {},
        "recipient": "all",
    });
    if let (Some(target), Value::Object(fields)) = (message.as_object_mut(), extra) {
        target.extend(fields);
    }
    message
}

pub fn text(role: &str, text: &str) -> Value {
    message(
        role,
        json!({ "content_type": "text", "parts": [text] }),
        json!({}),
    )
}

/// A deterministic word source, so fixtures and failures reproduce.
pub struct Words(u64);

const WORDS: &[&str] = &[
    "rust",
    "async",
    "tokio",
    "garden",
    "sermon",
    "pricing",
    "newsletter",
    "café",
    "naïve",
    "résumé",
    "Zürich",
    "embedding",
    "outline",
    "budget",
    "kids",
    "running",
    "runner",
    "ran",
    "C++",
    "e-mail",
    "don't",
    "v1.2",
    "2026",
    "plan",
    "draft",
    "chapter",
    "recipe",
    "taxes",
    "trip",
    "Lagos",
    "Durban",
    "jollof",
    "élan",
    "coöperate",
    "señor",
    "straße",
    "Ærø",
    "日本語",
    "中文",
    "한국어",
    "Привет",
    "مرحبا",
    "λόγος",
    "👍",
    "🎉",
    "🚀",
    "❤️",
    "👨‍👩‍👧",
    "100%",
    "#hashtag",
    "@mention",
    "snake_case",
    "kebab-case",
    "(parens)",
    "\"quoted\"",
    "semi;colon",
    "page",
    "launch",
    "ideas",
    "notes",
    "meeting",
    "the",
    "and",
    "of",
    "for",
];

impl Words {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    pub fn word(&mut self) -> &'static str {
        WORDS[(self.next() as usize) % WORDS.len()]
    }

    /// `count` words, with sentence breaks and the odd paragraph break.
    pub fn prose(&mut self, count: usize) -> String {
        let mut out = String::new();
        for n in 0..count {
            if n > 0 {
                out.push(match self.next() % 23 {
                    0 => '\n',
                    1 | 2 => '\t',
                    _ => ' ',
                });
                if self.next().is_multiple_of(61) {
                    out.push_str("\n\n");
                }
            }
            out.push_str(self.word());
            if self.next().is_multiple_of(11) {
                out.push('.');
            }
        }
        out
    }
}

/// The kinds of conversation [`rich_chats`] cycles through.
pub const KINDS: usize = 9;

/// `count` chats, each with a tree of one of [`KINDS`] shapes and text from
/// `seed`. Chat `n` is `chat-NNN`; every eleventh is archived.
pub fn rich_chats(count: usize, seed: u64) -> Vec<Chat> {
    let mut words = Words::new(seed);
    (0..count)
        .map(|n| {
            let update = format!(
                "2026-0{}-{:02}T{:02}:{:02}:{:02}.{:06}Z",
                1 + n % 9,
                1 + n % 28,
                n % 24,
                n % 60,
                (n * 7) % 60,
                n * 37
            );
            let title = format!("{} {} {n}", words.word(), words.word());
            let mut chat = Chat::new(&format!("chat-{n:03}"), &title, &update);
            chat.create_time = format!("2025-{:02}-{:02}T23:59:59.900000Z", 1 + n % 12, 1 + n % 28);
            chat.archived = n % 11 == 10;
            chat.model = match n % 4 {
                0 => Some("gpt-5".into()),
                1 => Some("gpt-4o".into()),
                2 => None,
                _ => Some("o3".into()),
            };
            chat.tree = Some(tree(n % KINDS, &mut words));
            chat
        })
        .collect()
}

fn tree(kind: usize, words: &mut Words) -> (Value, String) {
    let mut tree = Tree::new();
    tree.push(message(
        "system",
        json!({ "content_type": "text", "parts": [""] }),
        json!({ "metadata": { "is_visually_hidden_from_conversation": true } }),
    ));
    match kind {
        0 => long_turns(&mut tree, words),
        1 => canvas(&mut tree, words, false),
        2 => citations(&mut tree, words),
        3 => attachments(&mut tree, words),
        4 => voice(&mut tree, words),
        5 => images(&mut tree, words),
        6 => branches(&mut tree, words),
        7 => emoji_at_cuts(&mut tree, words),
        _ => canvas(&mut tree, words, true),
    }
    tree.finish()
}

/// Turns long enough to be cut into several chunks.
fn long_turns(tree: &mut Tree, words: &mut Words) {
    tree.push(text("user", &words.prose(40)));
    tree.push(text("assistant", &words.prose(400)));
    tree.push(text("user", &words.prose(12)));
    tree.push(text(
        "assistant",
        &format!("{}\n\n---\n\n{}", words.prose(90), words.prose(90)),
    ));
}

/// A canvas created, then edited by regex (one edit fails and stops that
/// message's edits); `replace_all` replaces it whole with `.*` too.
fn canvas(tree: &mut Tree, words: &mut Words, replace_all: bool) {
    tree.push(text(
        "user",
        &format!("Write a draft about {}", words.prose(6)),
    ));
    let doc = json!({
        "name": format!("{} draft", words.word()),
        "type": "document",
        "content": format!("# Draft\n\nThe draft says {}.\n\nAnother draft line: {}", words.prose(30), words.prose(20)),
    });
    tree.push(message(
        "assistant",
        json!({ "content_type": "code", "language": "json", "text": doc.to_string() }),
        json!({ "recipient": "canmore.create_textdoc" }),
    ));
    tree.push(message(
        "tool",
        json!({ "content_type": "text", "parts": ["Successfully created text document"] }),
        json!({ "author": { "role": "tool", "name": "canmore.create_textdoc" } }),
    ));
    tree.push(text("assistant", "I've drafted it in the canvas."));
    tree.push(text("user", "Sharpen it."));
    let mut updates = vec![
        json!({ "pattern": "\\bdraft\\b", "multiple": true, "replacement": "final cut" }),
        json!({ "pattern": "Another (\\w+) line", "replacement": "One more $1 line" }),
        json!({ "pattern": "(unclosed", "replacement": "never applied" }),
        json!({ "pattern": "Draft", "replacement": "also never applied" }),
    ];
    if replace_all {
        updates.insert(0, json!({ "pattern": ".*", "replacement": format!("# Rewritten\n\n{} draft draft", words.prose(25)) }));
    }
    tree.push(message(
        "assistant",
        json!({ "content_type": "code", "text": json!({ "updates": updates }).to_string() }),
        json!({ "recipient": "canmore.update_textdoc" }),
    ));
    tree.push(text("assistant", &format!("Updated: {}", words.prose(15))));
}

/// Web citations: private-use markers the UI swaps for their `alt`, and a
/// whitespace-only reference that must be skipped.
fn citations(tree: &mut Tree, words: &mut Words) {
    tree.push(text(
        "user",
        &format!("Search the web for {}", words.prose(5)),
    ));
    tree.push(message(
        "assistant",
        json!({ "content_type": "code", "text": "search(\"query\")" }),
        json!({ "recipient": "web.run" }),
    ));
    let cite = "\u{E200}cite\u{E202}turn0search1\u{E202}turn0news4\u{E201}";
    let entity = "\u{E200}entity\u{E202}[\"city\",\"Zürich\"]\u{E201}";
    let body = format!(
        "{} {cite}. Also {entity} and {} {cite}.\n\nLeftover \u{E200}filenavlist\u{E202}x\u{E201} marker.",
        words.prose(30),
        words.prose(20)
    );
    tree.push(message(
        "assistant",
        json!({ "content_type": "text", "parts": [body] }),
        json!({ "metadata": { "content_references": [
            { "matched_text": " ", "alt": "", "type": "sources_footnote" },
            { "matched_text": cite, "alt": "([Example](https://example.com/a))", "type": "grouped_webpages" },
            { "matched_text": entity, "alt": "Zürich", "type": "entity" },
            { "matched_text": cite, "alt": null, "type": "grouped_webpages" },
            { "matched_text": "not in the text", "alt": "x" },
        ] } }),
    ));
}

/// Files and images attached to user messages.
fn attachments(tree: &mut Tree, words: &mut Words) {
    tree.push(message(
        "user",
        json!({ "content_type": "multimodal_text", "parts": [
            { "content_type": "image_asset_pointer", "asset_pointer": "file-service://file-1", "width": 10, "height": 10 },
            words.prose(20),
        ] }),
        json!({ "metadata": { "attachments": [
            { "id": "file-1", "name": "photo.PNG", "mime_type": null },
            { "id": "file-2", "name": "notes.pdf", "mime_type": "application/pdf" },
            { "id": "file-3", "name": "diagram", "mime_type": "image/webp" },
            { "id": "file-4", "name": "budget 2026.xlsx" },
        ] } }),
    ));
    tree.push(text("assistant", &words.prose(60)));
    tree.push(message(
        "user",
        json!({ "content_type": "text", "parts": [""] }),
        json!({ "metadata": { "attachments": [{ "name": "résumé.docx", "mime_type": "application/msword" }] } }),
    ));
    tree.push(text("assistant", &words.prose(25)));
}

/// Voice mode: transcriptions inside multimodal parts.
fn voice(tree: &mut Tree, words: &mut Words) {
    for _ in 0..3 {
        let said = words.prose(15);
        tree.push(message(
            "user",
            json!({ "content_type": "multimodal_text", "parts": [
                { "content_type": "audio_transcription", "text": said, "direction": "in" },
                { "content_type": "audio_asset_pointer", "asset_pointer": "sediment://a" },
            ] }),
            json!({}),
        ));
        let answer = words.prose(25);
        tree.push(message(
            "assistant",
            json!({ "content_type": "multimodal_text", "parts": [
                { "content_type": "audio_transcription", "text": answer, "direction": "out" },
                { "content_type": "audio_asset_pointer", "asset_pointer": "sediment://b" },
            ] }),
            json!({}),
        ));
    }
}

/// An uploaded image with text, and a generated one.
fn images(tree: &mut Tree, words: &mut Words) {
    tree.push(message(
        "user",
        json!({ "content_type": "multimodal_text", "parts": [
            words.prose(10),
            { "content_type": "image_asset_pointer", "asset_pointer": "file-service://x" },
        ] }),
        json!({}),
    ));
    tree.push(message(
        "assistant",
        json!({ "content_type": "text", "parts": ["{\"prompt\": \"a garden\"}"] }),
        json!({ "recipient": "t2uay3k.sj1i4kz" }),
    ));
    tree.push(message(
        "tool",
        json!({ "content_type": "multimodal_text", "parts": [
            { "content_type": "image_asset_pointer", "asset_pointer": "sediment://img" },
        ] }),
        json!({ "author": { "role": "tool", "name": "t2uay3k.sj1i4kz" } }),
    ));
    tree.push(text("assistant", &words.prose(20)));
}

/// An edited question and a regenerated answer: only the current branch is
/// shown. Also a reasoning summary and code output, which aren't turns.
fn branches(tree: &mut Tree, words: &mut Words) {
    let system = tree.last().to_owned();
    tree.push(text("user", &format!("old question {}", words.prose(8))));
    tree.push(text("assistant", &format!("old answer {}", words.prose(8))));
    let question = tree.push_under(&system, text("user", &words.prose(12)));
    tree.push(text("assistant", &format!("first try {}", words.prose(10))));
    tree.push_under(
        &question,
        message(
            "assistant",
            json!({ "content_type": "reasoning_recap", "content": "Thought for 4s" }),
            json!({}),
        ),
    );
    tree.push(message(
        "assistant",
        json!({ "content_type": "thoughts", "thoughts": [{ "summary": "x", "content": "y" }] }),
        json!({}),
    ));
    tree.push(message(
        "assistant",
        json!({ "content_type": "code", "text": "print(1)" }),
        json!({ "recipient": "python" }),
    ));
    tree.push(message(
        "tool",
        json!({ "content_type": "execution_output", "text": "1" }),
        json!({}),
    ));
    tree.push(text("assistant", &words.prose(50)));
    tree.push(message(
        "user",
        json!({ "content_type": "text", "parts": ["hidden context"] }),
        json!({ "metadata": { "is_visually_hidden_from_conversation": true } }),
    ));
    tree.push(text("user", &words.prose(9)));
    tree.push(text("assistant", "   "));
    tree.push(text("assistant", &words.prose(30)));
}

/// Emoji right where the 800-unit cut and the 80-unit overlap fall, so a
/// chunk boundary splits a surrogate pair (see the daemon's chunks.rs).
fn emoji_at_cuts(tree: &mut Tree, words: &mut Words) {
    tree.push(text("user", &words.prose(10)));
    // Each section starts "## ChatGPT\n\n" (12 units) and has no space
    // before its cut, so the first chunk ends at exactly 800 units: here
    // between the halves of 👍…
    let cut_end = format!(
        "{}👍{} {}",
        "x".repeat(787),
        "y".repeat(200),
        words.prose(20)
    );
    tree.push(text("assistant", &cut_end));
    // …and here the next chunk starts 80 units earlier, at 🎉's second half.
    let cut_start = format!(
        "{}🎉{} {}",
        "z".repeat(707),
        "w".repeat(300),
        words.prose(20)
    );
    tree.push(text("assistant", &cut_start));
    tree.push(text("user", &format!("日本語 {} 中文", words.prose(40))));
    tree.push(text(
        "assistant",
        &format!(
            "{}\u{FEFF}\u{A0}\u{2028}{}",
            words.prose(30),
            words.prose(30)
        ),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "no such parent")]
    fn a_node_under_an_unknown_parent_is_refused() {
        Tree::new().push_under("nowhere", json!({}));
    }

    // Per-kind coverage is checked where it matters: the golden outputs
    // in crates/cli/tests/golden.rs render every kind.
    #[test]
    fn fixtures_are_deterministic() {
        let first = rich_chats(KINDS * 2, 7);
        let again = rich_chats(KINDS * 2, 7);
        assert_eq!(first.len(), KINDS * 2);
        for (a, b) in first.iter().zip(&again) {
            assert_eq!(a.tree, b.tree);
            assert_eq!(a.title, b.title);
        }
        let (mapping, current) = first[6].tree.clone().unwrap_or_default();
        assert!(mapping.get(&current).is_some());
    }
}
