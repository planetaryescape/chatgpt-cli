//! `export`/`show`: one chat, fetched live and rendered as markdown.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportedChat {
    /// The transcript, as the TS CLI's `renderTranscript` writes it.
    pub markdown: String,
    /// ChatGPT's title, for `-o`'s file name and `-c`'s note.
    pub title: String,
    /// When the index last synced, set when the chat was found by an id
    /// prefix (the TS CLI's `requireSynced` note).
    #[serde(default)]
    pub synced_at: Option<String>,
}
