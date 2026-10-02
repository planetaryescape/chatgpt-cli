//! Changing chats, projects and saved memories: `archive`, `unarchive`,
//! `delete`, `rename`, `title`, `project …` and `memory …`.
//!
//! The client resolves what a bulk command will act on first (`Select`),
//! shows the preview and asks for confirmation itself, then hands back
//! exactly the chats it showed (`Mutate`, `MoveToProject`): the daemon never
//! resolves the selection again.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::Filter;

/// The change `archive`, `unarchive` and `delete` make.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatAction {
    Archive,
    Unarchive,
    Delete,
    #[serde(other)]
    Unknown,
}

impl ChatAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Archive => "archive",
            Self::Unarchive => "unarchive",
            Self::Delete => "delete",
            Self::Unknown => "unknown",
        }
    }
}

/// The TS CLI's `selectTargets`: explicit ids or id prefixes (or the ones
/// read from stdin for `-`), else the filters.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    /// `None`: no ids were given, so the filters choose.
    #[serde(default)]
    pub ids: Option<Vec<String>>,
    #[serde(default)]
    pub filter: Filter,
    /// `--pinned`: filters include pinned chats.
    #[serde(default)]
    pub pinned: bool,
    /// Applying Jev's own suggestion (`delete --suggest delete`): unsure
    /// chats are left out before `--limit`.
    #[serde(default)]
    pub exclude_unsure: bool,
    /// `classify`, `titles` and `review`: with neither ids nor a filter,
    /// every chat in scope (`selectTargets`' `allowUnfiltered`).
    #[serde(default)]
    pub allow_unfiltered: bool,
    /// `review`: each row with its current verdict and topic, as `List`
    /// gives them (0.2.1 and newer; an older daemon leaves them out).
    #[serde(default)]
    pub verdicts: bool,
}

/// A chat the client showed in its preview and the user confirmed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub id: String,
    /// The original ChatGPT title, for failure lines.
    pub title: String,
}

/// A ChatGPT project (`project list`'s row; `--json` prints `canWrite`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    #[serde(rename = "canWrite")]
    pub can_write: bool,
}

/// What a bulk change did. The step's summary line went out as progress.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub done: u64,
    /// `<id> <title>: <why>`, without any response body.
    pub failures: Vec<String>,
}

/// An API key a client passes on (its `TYPESAFE_API_KEY`). `Debug` never
/// shows it; nothing logs a request.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_never_shows_in_debug_output() {
        let secret = Secret::new("ts-live-key".into());
        assert_eq!(format!("{secret:?}"), "Secret(<redacted>)");
        assert_eq!(
            serde_json::to_string(&secret).expect("encode"),
            "\"ts-live-key\""
        );
    }

    #[test]
    fn projects_keep_the_ts_clis_json_names() {
        let project = Project {
            id: "g-p-1".into(),
            name: "P".into(),
            can_write: true,
        };
        assert_eq!(
            serde_json::to_string(&project).expect("encode"),
            r#"{"id":"g-p-1","name":"P","canWrite":true}"#
        );
    }
}
