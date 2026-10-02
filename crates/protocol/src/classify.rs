//! Classifying chats and saved memories: `classify`, `titles`, `memory
//! classify`, the Jev guard's model access, and the daemon's background
//! Jev (`daemon status`).
//!
//! The client resolves the chats first (`Select`), as the TS CLI's
//! `selectTargets` does, then hands their ids over. A step that needs the
//! user's say mid-run (the TS CLI's "Go ahead? [y/N]" before a large batch
//! of summaries) arrives as a progress line of kind `Ask`; the client
//! answers with [`crate::Request::Answer`] on the same request id.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Secret;

/// What the requesting client can lend the daemon for model calls: its
/// environment's API keys (the daemon's own environment is whichever
/// client started it, so it never reads keys from it), and its `PATH`,
/// where `codex` and `claude` are found (a daemon started at login has a
/// bare one). `Debug` never shows a key.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelAccess {
    /// `TYPESAFE_API_KEY`.
    #[serde(default)]
    pub typesafe: Option<Secret>,
    /// `OPENAI_API_KEY`.
    #[serde(default)]
    pub openai: Option<Secret>,
    /// `ANTHROPIC_API_KEY`.
    #[serde(default)]
    pub anthropic: Option<Secret>,
    #[serde(default)]
    pub path: Option<String>,
}

/// How a `classify` or `titles` run went. Its notes, failures and cost
/// lines went out as progress.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassifyOutcome {
    /// Something failed: the command exits 1.
    pub failed: bool,
}

/// `memory classify`'s rows: each saved memory as ChatGPT sent it, with
/// `suggestion`, `reason`, `stage` and `related_ids` added, as the TS CLI's
/// `ClassifiedMemory`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ClassifiedMemories {
    pub rows: Vec<Value>,
    /// `<id>: <why>`.
    pub failures: Vec<String>,
}

/// The daemon's background Jev: it judges chats that are new or changed
/// since it was first enabled, after each successful sync pass, with the
/// key in the user config only.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AutoJevStatus {
    /// A Jev key is configured and `auto_jev` isn't `false`.
    #[serde(default)]
    pub enabled: bool,
    /// Why it's off.
    #[serde(default)]
    pub off_reason: Option<String>,
    #[serde(default)]
    pub in_progress: bool,
    /// Unix seconds.
    #[serde(default)]
    pub last_run_at: Option<i64>,
    #[serde(default)]
    pub last_summary: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    /// The UTC day the counts below are for, `YYYY-MM-DD`.
    #[serde(default)]
    pub day: Option<String>,
    #[serde(default)]
    pub judged_today: u64,
    /// What Jev billed for them, USD.
    #[serde(default)]
    pub cost_today_usd: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_never_show_in_debug_output() {
        let access = ModelAccess {
            typesafe: Some(Secret::new("ts-live".into())),
            openai: Some(Secret::new("sk-live".into())),
            anthropic: Some(Secret::new("ant-live".into())),
            path: Some("/usr/bin".into()),
        };
        let shown = format!("{access:?}");
        for key in ["ts-live", "sk-live", "ant-live"] {
            assert!(!shown.contains(key), "{shown}");
        }
    }

    #[test]
    fn an_older_status_has_auto_jev_off() {
        let status: AutoJevStatus = serde_json::from_str("{}").expect("decode");
        assert!(!status.enabled);
        assert_eq!(status.cost_today_usd, 0.0);
    }
}
