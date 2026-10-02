//! The native commands' arguments, matching the TS CLI's flags
//! (`src/cli.ts` @ 1b8c950) for `sync`, `list`, `stats`, `export`,
//! `search`, `search-index`, `archive`, `unarchive`, `delete`, `rename`,
//! `title`, `project` and `memory`.

use clap::{Args, Parser, Subcommand};

/// The commands that still run in the TS CLI, with its descriptions, for
/// the top-level help. Their own `--help` comes from the TS CLI.
pub const BRIDGED: &[(&str, &str)] = &[
    (
        "configure",
        "Store Jev, OpenAI, or Anthropic API keys in the user config; omit provider to show status",
    ),
    (
        "memory classify",
        "Classify saved memories for keep, delete, or review with Jev and Luna; never deletes",
    ),
    (
        "titles",
        "Generate local display titles and topic themes with gpt-6-luna",
    ),
    (
        "classify",
        "Classify with Jev and Luna, then generate missing local titles and topic themes",
    ),
    (
        "review",
        "Triage conversations one by one; changes apply after a final confirmation",
    ),
    (
        "tui",
        "Browse, filter and triage conversations in a terminal UI",
    ),
];

pub const NATIVE: &[&str] = &[
    "sync",
    "list",
    "stats",
    "export",
    "show",
    "search",
    "search-index",
    "daemon",
    "import-legacy",
    "archive",
    "unarchive",
    "delete",
    "rename",
    "title",
    "project",
    "memory",
];

fn bridged_help() -> String {
    let width = BRIDGED
        .iter()
        .map(|(name, _)| name.len())
        .max()
        .unwrap_or(0);
    let mut help = String::from(
        "Commands that run in the TS chatgpt CLI (`chatgpt <command> --help` shows theirs):\n",
    );
    for (name, about) in BRIDGED {
        help.push_str(&format!("  {name:width$}  {about}\n"));
    }
    help
}

#[derive(Parser)]
#[command(
    name = "chatgpt",
    version,
    about = "Manage your ChatGPT conversations from the terminal (uses your browser login).",
    after_help = bridged_help(),
    arg_required_else_help = true
)]
pub struct Cli {
    /// Use a specific browser: dia, chrome, safari, firefox, arc, brave, or edge
    #[arg(long, global = true, value_name = "name")]
    pub browser: Option<String>,
    /// Use a browser profile directory (with --browser)
    #[arg(long, global = true, value_name = "name")]
    pub profile: Option<String>,
    /// Which daemon and index to use (default: the installed one)
    #[arg(long, global = true, hide = true)]
    pub instance: Option<String>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Update the local index: new and changed chats, plus archive state
    Sync {
        /// Rebuild from scratch (also drops chats deleted in the browser)
        #[arg(long)]
        full: bool,
    },
    /// List conversations from the local index
    List(ListArgs),
    /// Chat suggestions, brainstorms and topics, plus saved-memory suggestions
    Stats(FilterArgs),
    /// Export a conversation as markdown: chat link, id, or id prefix
    #[command(visible_alias = "show")]
    Export(ExportArgs),
    /// Search the local transcript index; use --semantic or --hybrid for meaning-based matches
    Search(SearchArgs),
    /// Build or refresh the local text and semantic search index
    SearchIndex(ScopeArgs),
    /// Start, stop and inspect the background daemon
    #[command(subcommand)]
    Daemon(DaemonCommand),
    /// Import Jev judgments, Luna reviews and local titles from the TS CLI's index now
    ImportLegacy,
    /// Archive conversations by id/prefix, `-` for ids on stdin, or by filter
    Archive(ChangeArgs),
    /// Unarchive conversations by id/prefix, `-` for ids on stdin, or by filter
    Unarchive(ChangeArgs),
    /// Delete conversations by id/prefix, `-` for ids on stdin, or by filter
    Delete(ChangeArgs),
    /// Rename a conversation in ChatGPT (changes its order in the app)
    Rename(RenameArgs),
    /// Set a local display title without changing ChatGPT
    Title(RenameArgs),
    /// Create, list and manage chat projects
    #[command(subcommand)]
    Project(ProjectCommand),
    /// List and delete saved memories, or read the memory summary
    #[command(subcommand)]
    Memory(MemoryCommand),
}

/// `archive`, `unarchive` and `delete`: `withFilters` with `--pinned`.
#[derive(Args)]
pub struct ChangeArgs {
    /// Conversation ids or id prefixes, or `-` to read them from stdin
    #[arg(value_name = "ids")]
    pub ids: Vec<String>,
    #[command(flatten)]
    pub filters: FilterArgs,
    /// Include pinned conversations (skipped by default)
    #[arg(long)]
    pub pinned: bool,
    /// Show what would change
    #[arg(short = 'n', long)]
    pub dry_run: bool,
    /// Skip the confirmation prompt
    #[arg(short, long)]
    pub yes: bool,
    /// Ask Jev to read each chat and only act on those it agrees with (archive/delete)
    #[arg(long)]
    pub check: bool,
}

/// `rename` and `title`.
#[derive(Args)]
pub struct RenameArgs {
    /// Conversation id or id prefix
    #[arg(value_name = "id")]
    pub id: String,
    /// The new title
    #[arg(value_name = "title")]
    pub title: String,
    /// Allow an archived conversation
    #[arg(long)]
    pub archived: bool,
    /// Allow either active or archived
    #[arg(long)]
    pub all: bool,
}

#[derive(Subcommand)]
pub enum ProjectCommand {
    /// Create a project with ChatGPT's default settings
    Create {
        #[arg(value_name = "name")]
        name: String,
        /// Output JSON
        #[arg(long)]
        json: bool,
    },
    /// List projects available to your account
    List {
        /// Output JSON
        #[arg(long)]
        json: bool,
        /// At most n projects
        #[arg(long, value_name = "n")]
        limit: Option<String>,
    },
    /// Move chats into an existing project by name or id; use `-` for ids on stdin
    Add(ProjectMoveArgs),
    /// Remove chats from an existing project; use `-` for ids on stdin
    Remove(ProjectMoveArgs),
}

#[derive(Args)]
pub struct ProjectMoveArgs {
    /// Project name, id or id prefix
    #[arg(value_name = "project")]
    pub project: String,
    /// Conversation ids or id prefixes, or `-` to read them from stdin
    #[arg(value_name = "ids", required = true)]
    pub ids: Vec<String>,
    /// Preview without changing anything
    #[arg(short = 'n', long)]
    pub dry_run: bool,
    /// Skip the confirmation prompt
    #[arg(short, long)]
    pub yes: bool,
    /// Select archived conversations instead of active ones
    #[arg(long)]
    pub archived: bool,
    /// Select both active and archived conversations
    #[arg(long)]
    pub all: bool,
}

#[derive(Subcommand)]
pub enum MemoryCommand {
    /// List saved memories from ChatGPT
    List {
        /// Only memories whose content contains text
        #[arg(long, value_name = "text")]
        search: Option<String>,
        /// At most n saved memories
        #[arg(long, value_name = "n")]
        limit: Option<String>,
        /// Output format: json, csv, table, or ids
        #[arg(long, value_name = "format", default_value = "table")]
        format: String,
    },
    /// Classify saved memories for keep, delete, or review with Jev and Luna; never deletes (runs in the TS CLI)
    #[command(disable_help_flag = true)]
    Classify {
        /// The TS CLI's options for it (`chatgpt memory classify --help`)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Read ChatGPT's generated memory summary
    Summary {
        /// Output format: json or table
        #[arg(long, value_name = "format", default_value = "table")]
        format: String,
    },
    /// Delete saved memories by id/prefix or `-` for ids on stdin; does not delete source chats
    Delete {
        /// Saved memory ids or id prefixes, or `-` to read them from stdin
        #[arg(value_name = "ids", required = true)]
        ids: Vec<String>,
        /// Preview without deleting memories
        #[arg(short = 'n', long)]
        dry_run: bool,
        /// Skip the confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },
}

/// `withFilters` without `--pinned`: `list` and `stats` include pinned chats.
#[derive(Args, Clone, Default)]
pub struct FilterArgs {
    /// Last updated more than <age> ago (30d, 12w, 6m, 2y)
    #[arg(long, value_name = "age")]
    pub older_than: Option<String>,
    /// Last updated within <age>
    #[arg(long, value_name = "age")]
    pub newer_than: Option<String>,
    /// Last updated before YYYY-MM-DD
    #[arg(long, value_name = "date")]
    pub before: Option<String>,
    /// Last updated on or after YYYY-MM-DD
    #[arg(long, value_name = "date")]
    pub after: Option<String>,
    /// Title matches regex (case-insensitive)
    #[arg(long, value_name = "regex")]
    pub title: Option<String>,
    /// Archived conversations instead of active ones
    #[arg(long)]
    pub archived: bool,
    /// Both active and archived
    #[arg(long)]
    pub all: bool,
    /// At most n conversations (newest first)
    #[arg(long, value_name = "n")]
    pub limit: Option<String>,
    /// Only chats classified as delete, archive, or keep (needs `classify`)
    #[arg(long, value_name = "action")]
    pub suggest: Option<String>,
    /// Only chats classified in this topic (`chatgpt stats` lists them)
    #[arg(long, value_name = "topic")]
    pub topic: Option<String>,
    /// Only chats where you were brainstorming; optionally writing, sermon, product, or other
    #[arg(long, value_name = "kind", num_args = 0..=1, default_missing_value = "")]
    pub brainstorm: Option<String>,
}

#[derive(Args)]
pub struct ListArgs {
    #[command(flatten)]
    pub filters: FilterArgs,
    /// Output JSON
    #[arg(long)]
    pub json: bool,
    /// Output format: ids (default: text)
    #[arg(long, value_name = "format")]
    pub format: Option<String>,
    /// Print only the number of matches
    #[arg(long)]
    pub count: bool,
}

#[derive(Args)]
pub struct ExportArgs {
    /// Chat link, id, or id prefix
    pub link: String,
    /// Write to a file (default name: from the title)
    // `Some(None)`: `-o` alone. `Some(Some(""))` (`--output=`) is unset, as
    // Commander's empty string is to the TS CLI.
    #[arg(short, long, value_name = "file", num_args = 0..=1)]
    pub output: Option<Option<String>>,
    /// Copy to the clipboard
    #[arg(short, long)]
    pub copy: bool,
    /// Allow an archived conversation
    #[arg(long)]
    pub archived: bool,
    /// Allow either active or archived
    #[arg(long)]
    pub all: bool,
}

#[derive(Args)]
pub struct SearchArgs {
    pub query: String,
    /// Rank by local embedding similarity
    #[arg(long)]
    pub semantic: bool,
    /// Combine full-text and semantic ranking
    #[arg(long)]
    pub hybrid: bool,
    /// Use ChatGPT's server-side search instead
    #[arg(long)]
    pub remote: bool,
    /// Search archived conversations instead of active ones
    #[arg(long)]
    pub archived: bool,
    /// Search both active and archived conversations
    #[arg(long)]
    pub all: bool,
    /// Output format: json, csv, table, or ids (default: two-line text)
    #[arg(long, value_name = "format")]
    pub format: Option<String>,
    /// Maximum conversations
    #[arg(long, value_name = "n", default_value = "20")]
    pub limit: String,
}

/// `search-index`'s scope.
#[derive(Args)]
pub struct ScopeArgs {
    /// Index archived conversations instead of active ones
    #[arg(long)]
    pub archived: bool,
    /// Index both active and archived conversations
    #[arg(long)]
    pub all: bool,
}

#[derive(Subcommand)]
pub enum DaemonCommand {
    /// Run the daemon in the foreground
    Run,
    /// Start the daemon detached (used by clients that find none running)
    #[command(hide = true)]
    Launch,
    /// Show the daemon's state, starting it if it isn't running
    Status {
        /// Output JSON
        #[arg(long)]
        json: bool,
    },
    /// Stop the daemon
    Stop,
    /// Print the daemon's log
    Logs {
        /// Keep printing new lines as they're written
        #[arg(long, short)]
        follow: bool,
        /// How many of the last lines to print first
        #[arg(long, short = 'n', default_value_t = 50)]
        lines: usize,
    },
    /// Start the daemon at login (writes a macOS LaunchAgent; doesn't load it)
    Install,
    /// Stop starting the daemon at login (removes the LaunchAgent)
    Uninstall,
    /// Run the search embedding model on stdin and stdout (started by the
    /// daemon)
    #[command(hide = true)]
    EmbedWorker {
        /// The model's files
        #[arg(long, value_name = "dir", required_unless_present = "fake")]
        model_dir: Option<std::path::PathBuf>,
        /// A stand-in embedder, for tests (debug builds only)
        #[arg(long)]
        fake: bool,
    },
}
