//! What a client asks the daemon.

use serde::{Deserialize, Serialize};

use crate::{
    ChatAction, Filter, ModelAccess, Project, SearchMode, Secret, Selection, Target,
    TranscriptSource,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum Request {
    /// How the daemon stands. Answers within the client's quick timeout,
    /// whatever else is running.
    Status,
    /// Stop after answering.
    Shutdown,
    /// Sync now and answer when done, with progress events meanwhile. Runs
    /// the archived sweep and the cache reconcile whatever their schedule
    /// says.
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
    /// The TUI's preview and `review`: chat `id`'s transcript as the cache
    /// holds it (the batch endpoint's rendering), and its summary.
    Transcript {
        id: String,
        #[serde(default)]
        source: TranscriptSource,
        #[serde(default)]
        session: SessionChoice,
    },
    /// Local `search` over the indexed transcripts. Never touches the
    /// network; answers from what the background indexer and embedder have
    /// done so far.
    Search {
        query: String,
        limit: u64,
        #[serde(default)]
        archived: bool,
        #[serde(default)]
        all: bool,
        #[serde(default)]
        mode: SearchMode,
    },
    /// `search --remote`: ChatGPT's own search, asked now.
    RemoteSearch {
        query: String,
        limit: u64,
        #[serde(default)]
        archived: bool,
        #[serde(default)]
        all: bool,
        #[serde(default)]
        session: SessionChoice,
    },
    /// `search-index`: wait for the indexer and the embedder to catch up,
    /// with progress events, and report on the scope.
    SearchIndex {
        #[serde(default)]
        archived: bool,
        #[serde(default)]
        all: bool,
        /// The browser to fetch missing transcripts with; it stays the
        /// daemon's choice, as a `sync`'s does.
        #[serde(default)]
        session: SessionChoice,
    },
    /// The chats `archive`, `unarchive`, `delete` or `project add/remove`
    /// would act on, for the client's preview. Never touches the network.
    Select { selection: Box<Selection> },
    /// The Jev guard (`--check`, `--suggest <action>`): judge the chats
    /// that lack a current judgment now, with progress events, and answer
    /// with the ids Jev backs `action` for.
    JevCheck {
        action: ChatAction,
        ids: Vec<String>,
        /// The client's `TYPESAFE_API_KEY`, if it has one; else the daemon
        /// reads the user config.
        #[serde(default)]
        api_key: Option<Secret>,
        #[serde(default)]
        session: SessionChoice,
        /// Summarisers for long chats, and the client's other keys.
        #[serde(default)]
        access: ModelAccess,
        /// `-y`: summarise a large batch without asking.
        #[serde(default)]
        yes: bool,
        /// The client answers `Ask` progress lines. Without it (a client
        /// from before 0.1.4), the daemon never asks: a large batch of
        /// summaries without `yes` fails before anything is spent.
        #[serde(default)]
        can_answer: bool,
    },
    /// `classify`: Jev (then its follow-up for unsure chats), Luna's
    /// review, and missing local titles, for exactly these chats, with
    /// progress events and the cost lines. May ask (`ProgressKind::Ask`)
    /// before summarising a large batch, unless `yes`.
    Classify {
        ids: Vec<String>,
        /// `--redo`: judge again even what's current.
        #[serde(default)]
        redo: bool,
        #[serde(default)]
        yes: bool,
        #[serde(default)]
        access: ModelAccess,
        #[serde(default)]
        session: SessionChoice,
        /// As for `JevCheck`.
        #[serde(default)]
        can_answer: bool,
    },
    /// `titles`: Luna's local titles and themes for these chats, from
    /// their cached transcripts or summaries.
    Titles {
        ids: Vec<String>,
        #[serde(default)]
        redo: bool,
        #[serde(default)]
        access: ModelAccess,
    },
    /// `memory classify`: Jev's quick pass over the saved memories, then
    /// Luna's review of the ones it can't keep. Never deletes.
    MemoryClassify {
        #[serde(default)]
        redo: bool,
        #[serde(default)]
        access: ModelAccess,
        #[serde(default)]
        session: SessionChoice,
    },
    /// The client's answer to the question its running request asked
    /// (`ProgressKind::Ask`), sent with that request's id.
    Answer { yes: bool },
    /// Archive, unarchive or delete exactly these chats, in ChatGPT and
    /// then the index, with progress events.
    Mutate {
        action: ChatAction,
        targets: Vec<Target>,
        #[serde(default)]
        session: SessionChoice,
    },
    /// `rename`: resolve one chat and rename it in ChatGPT and the index.
    Rename {
        reference: String,
        title: String,
        #[serde(default)]
        archived: bool,
        #[serde(default)]
        all: bool,
        #[serde(default)]
        session: SessionChoice,
    },
    /// `title`: a local display title. Never touches the network.
    SetTitle {
        reference: String,
        title: String,
        #[serde(default)]
        archived: bool,
        #[serde(default)]
        all: bool,
    },
    /// `project list`, and the projects `project add/remove` resolve against.
    Projects {
        #[serde(default)]
        session: SessionChoice,
    },
    /// `project create`.
    CreateProject {
        name: String,
        #[serde(default)]
        session: SessionChoice,
    },
    /// Move exactly these chats into `project`, or (`remove`) out of it,
    /// with progress events.
    MoveToProject {
        project: Project,
        targets: Vec<Target>,
        #[serde(default)]
        remove: bool,
        #[serde(default)]
        session: SessionChoice,
    },
    /// `memory list`, and the memories `memory delete` resolves against:
    /// ChatGPT's saved memories as it sends them.
    Memories {
        #[serde(default)]
        session: SessionChoice,
    },
    /// `memory summary`: ChatGPT's generated memory summary as it sends it.
    MemorySummary {
        #[serde(default)]
        session: SessionChoice,
    },
    /// Delete exactly these saved memories.
    DeleteMemories {
        ids: Vec<String>,
        #[serde(default)]
        session: SessionChoice,
    },
    #[serde(other)]
    Unknown,
}

impl Request {
    /// `List` of every chat, active and archived, pinned ones included: the
    /// TUI's rows and `review`'s verdicts (`index.query({ includePinned:
    /// true })`).
    pub fn list_every_chat() -> Self {
        Self::List {
            filter: Box::new(Filter {
                all: true,
                ..Filter::default()
            }),
        }
    }
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
