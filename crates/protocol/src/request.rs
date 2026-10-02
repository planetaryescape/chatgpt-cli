//! What a client asks the daemon.

use serde::{Deserialize, Serialize};

use crate::Filter;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum Request {
    /// How the daemon stands. Answers within the client's quick timeout,
    /// whatever else is running.
    Status,
    /// Stop after answering.
    Shutdown,
    /// Sync now and answer when done, with progress events meanwhile. Runs
    /// the archived sweep, the cache reconcile and the TS sync whatever their
    /// schedule says.
    Sync {
        #[serde(default)]
        full: bool,
        #[serde(default)]
        session: SessionChoice,
    },
    /// Chats from the index, newest first, with their current Jev verdicts.
    /// Never touches the network.
    List { filter: Box<Filter> },
    /// `stats`: the chat counts from the index plus live saved-memory counts.
    Stats {
        filter: Box<Filter>,
        #[serde(default)]
        session: SessionChoice,
    },
    /// Import the TS CLI's judgments, titles, summaries and transcripts now.
    ImportLegacy,
    /// `export`/`show`: resolve a link, id or id prefix, fetch the chat now
    /// and render it.
    Export {
        reference: String,
        /// For a `reference` of `-`: the ids the client read from stdin.
        #[serde(default)]
        stdin_ids: Vec<String>,
        #[serde(default)]
        archived: bool,
        #[serde(default)]
        all: bool,
        #[serde(default)]
        session: SessionChoice,
    },
    /// Lexical `search` over the indexed transcripts. Never touches the
    /// network; answers from what the background indexer has done so far.
    Search {
        query: String,
        limit: u64,
        #[serde(default)]
        archived: bool,
        #[serde(default)]
        all: bool,
    },
    #[serde(other)]
    Unknown,
}

/// `--browser` and `--profile` (or `CHATGPT_BROWSER` and
/// `CHATGPT_BROWSER_PROFILE`), as the client resolved them. `None` means the
/// macOS default browser. A daemon holding another session reads cookies
/// again for this one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionChoice {
    #[serde(default)]
    pub browser: Option<String>,
    #[serde(default)]
    pub profile: Option<String>,
}
