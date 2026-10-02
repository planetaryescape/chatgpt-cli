//! `export`/`show`: one chat, fetched live and rendered as markdown; and a
//! chat's cached transcript, for the TUI's preview and `review`. Either can
//! be larger than a frame: the daemon then sends it in parts
//! ([`crate::Response::Parted`]), which the launcher joins.

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

/// `Transcript`'s answer: chat `id`'s transcript as the cache holds it
/// (the batch endpoint's rendering), with what the TUI's preview and
/// `review` show beside it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatTranscript {
    pub id: String,
    /// `None`: the cache doesn't have it, and it wasn't fetched (or the
    /// fetch failed: `fetch_error`).
    #[serde(default)]
    pub markdown: Option<String>,
    /// Why the fetch failed, worded for people.
    #[serde(default)]
    pub fetch_error: Option<String>,
    /// The summary a long chat's current judgment was made from.
    #[serde(default)]
    pub summary: Option<String>,
    /// With `TranscriptSource::Fresh`: the turns `review` shows.
    #[serde(default)]
    pub excerpt: Option<Excerpt>,
}

/// What `review` shows of a chat: how many visible turns it has, the first
/// thing the user said and ChatGPT's last answer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Excerpt {
    pub turns: u64,
    #[serde(default)]
    pub first_user: Option<String>,
    #[serde(default)]
    pub last_assistant: Option<String>,
}

/// Where `Transcript` gets the transcript from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptSource {
    /// The cache only: no network.
    #[default]
    Cache,
    /// The cache, else the batch endpoint (then cached): the TUI's preview.
    CacheOrFetch,
    /// The batch endpoint (then cached), with the [`Excerpt`]: `review`,
    /// which loads each chat fresh, as the TS CLI's does.
    Fresh,
    #[serde(other)]
    Unknown,
}

/// A slice of an answer too large for one frame: part of its JSON.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Part {
    /// 0, 1, 2…: a missing, repeated or reordered part is refused.
    pub index: u64,
    pub text: String,
}
