//! `list` and `stats`: the filters the TS CLI's `withFilters` takes, and the
//! rows and counts the daemon answers with. The CLI only formats them.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The TS CLI's `withFilters` options, as typed. The daemon validates them,
/// in the TS CLI's order and with its messages.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    /// `--older-than <age>`: `30d`, `12w`, `6m`, `2y`.
    #[serde(default)]
    pub older_than: Option<String>,
    #[serde(default)]
    pub newer_than: Option<String>,
    /// `--before <date>`, `YYYY-MM-DD`.
    #[serde(default)]
    pub before: Option<String>,
    #[serde(default)]
    pub after: Option<String>,
    /// `--title <regex>`, case-insensitive, against the display and original
    /// titles.
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub all: bool,
    /// `--limit <n>`, as typed: it's validated after the index check.
    #[serde(default)]
    pub limit: Option<String>,
    #[serde(default)]
    pub suggest: Option<String>,
    #[serde(default)]
    pub topic: Option<String>,
    /// `--brainstorm [kind]`: `Some("")` for a bare `--brainstorm` (any kind).
    #[serde(default)]
    pub brainstorm: Option<String>,
}

/// A chat as `list --json` prints it, plus what the text row needs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub id: String,
    /// The original ChatGPT title.
    pub title: String,
    pub create_time: String,
    pub update_time: String,
    /// 0 or 1, as the TS CLI prints them.
    pub is_archived: u8,
    pub pinned: u8,
    pub project_id: Option<String>,
    pub local_title: Option<String>,
    pub display_title: String,
    /// The judgment's stored topic, `null` without a current judgment.
    pub topic: Option<String>,
    /// The topic the text row shows: the stored one, else Jev's answer.
    #[serde(default)]
    pub row_topic: Option<String>,
    /// Absent without a current judgment.
    #[serde(default)]
    pub jev: Option<Jev>,
}

/// A current judgment's verdict, as policy computes it on read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Jev {
    pub suggestion: String,
    pub unsure: bool,
    pub reason: String,
    pub brainstorm: Option<String>,
    /// Settled by the follow-up questions.
    #[serde(default)]
    pub deep: bool,
    /// Settled or adjusted by Luna's review.
    #[serde(default)]
    pub luna: bool,
    /// Jev's raw answers, in their stored key order.
    pub answers: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ListRows {
    pub rows: Vec<Row>,
    /// When the index last synced (ISO 8601), for the stale-index note.
    pub synced_at: String,
}

/// `stats`'s chat part, already ordered as the TS CLI prints it, and its
/// saved-memory part.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatsReport {
    pub synced_at: String,
    pub chats: u64,
    pub unjudged: u64,
    /// `delete`, `delete?`, `archive`, `archive?`, `keep`, `keep?`.
    pub labels: Vec<String>,
    /// One count per label.
    pub suggestions: Vec<u64>,
    /// Brainstorm kinds and their chat counts, in the TS CLI's order.
    pub brainstorms: Vec<(String, u64)>,
    /// Every topic, most chats first.
    pub topics: Vec<TopicCounts>,
    /// `None` when the saved memories couldn't be read; see `memory_error`.
    #[serde(default)]
    pub memory: Option<MemoryCounts>,
    #[serde(default)]
    pub memory_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicCounts {
    pub topic: String,
    /// One count per label.
    pub counts: Vec<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryCounts {
    pub total: u64,
    pub keep: u64,
    pub delete: u64,
    pub review: u64,
    pub unclassified: u64,
}
