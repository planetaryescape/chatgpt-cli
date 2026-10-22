//! The whole write path, both CLIs, side by side: every change the
//! TypeScript CLI could make — archive, unarchive, delete, rename, title,
//! project moves, memory deletes — runs in the TS CLI and in the Rust CLI,
//! each against its own fake chatgpt.com, and their stdout, stderr, exit
//! status, the requests they sent and the state they leave behind are
//! compared.
//!
//! Every scenario also states what it expects to have happened: the
//! requests each CLI must send, the chats that must be archived, deleted,
//! renamed or moved, and the memories that must be gone. Agreeing is not
//! enough: two CLIs that both did nothing would agree.
//!
//! The scenarios are the v0.1.5 harness's (`git show v0.1.5:crates/cli/tests/parity_mutations.rs`),
//! ported to today's CLI and shaped like the one-write harness of
//! `parity_write.rs`. The Jev guard (`--check`) is not here: it asks Jev
//! about each chat and summarises the long ones, which needs the model
//! CLIs as well as the fake TypeSafe.
//!
//! Needs bun and the TypeScript CLI's checkout (`CHATGPT_TS_REPO`); skips
//! without them.

#![allow(clippy::unwrap_used)]

mod support;

use std::path::PathBuf;

use fake_chatgpt::typesafe::{API_KEY, FakeTypeSafe};
use fake_chatgpt::{Chat, FakeChatGpt, Project};
use serde_json::{Value, json};
use support::{
    Env, PARITY_PLUGIN, QUESTIONS_VERSION, Ts, bun_for, copy_chats, copy_judgments, delete_answers,
    normalize, ts_index, ts_repo, writes,
};

const OLD_DAY: &str = "2024-02";

/// The sidebar request both CLIs make to list the projects.
const SIDEBAR: &str =
    "GET /backend-api/gizmos/snorlax/sidebar?conversations_per_gizmo=0&limit=20&owned_only=false";

/// The request both CLIs make to create a project.
const CREATE_PROJECT: &str = "POST /backend-api/projects";

/// The request both CLIs make to list the saved memories.
const MEMORIES: &str = "GET /backend-api/memories?include_memory_entries=true";

/// The request both CLIs make to read the memory summary (a POST).
const MEMORY_SUMMARY: &str =
    "POST /backend-api/memories/about_you/summary?source=personalization-setting";

/// The v0.1.5 harness's chat set: enough old chats for a preview's
/// "… and N more", the four the suggestions select, a long chat and the
/// awkward ones (a legacy chat, an archived pair, a fresh chat).
fn chats() -> Vec<Chat> {
    let mut chats = Vec::new();
    // Enough old chats for a preview's "… and N more".
    for n in 0..32 {
        let mut chat = Chat::new(
            &format!("n-{n:02}"),
            &format!("Old note {n}"),
            &format!("{OLD_DAY}-{:02}T10:{n:02}:00.000000Z", 1 + n % 28),
        );
        chat.pinned = n % 7 == 3;
        if n % 5 == 0 {
            chat.gizmo_id = Some("g-p-writing".into());
        }
        chats.push(chat);
    }
    for (id, title, minute) in [
        ("j-junk", "Junk ping", 1),
        ("j-maybe", "Maybe receipt", 2),
        ("j-idea", "Book idea", 3),
        ("j-stale", "Stale trip", 4),
    ] {
        chats.push(Chat::new(
            id,
            title,
            &format!("2023-05-01T10:0{minute}:00.000000Z"),
        ));
    }
    let mut long = Chat::new("j-long", "Long junk essay", "2023-05-01T09:00:00.000000Z");
    long.text = "word ".repeat(12_000);
    chats.push(long);
    chats.push(Chat::new(
        "r-legacy",
        "Legacy chat",
        "2022-01-01T10:00:00.000000Z",
    ));
    for n in 0..2 {
        let mut archived = Chat::new(
            &format!("z-arch{n}"),
            &format!("Archived {n}"),
            &format!("2025-03-0{}T10:00:00.000000Z", n + 1),
        );
        archived.archived = true;
        chats.push(archived);
    }
    chats.push(Chat::new(
        "f-fresh",
        "Fresh chat",
        "2026-09-30T10:00:00.000000Z",
    ));
    chats
}

/// The state both fakes start in: the chats, the projects, the memories.
fn set_up(fake: &FakeChatGpt) {
    let mut state = fake.state();
    state.chats = chats();
    let mut shared = Project::new("g-p-shared", "Shared");
    shared.can_write = false;
    state.projects = vec![
        Project::new("g-p-writing", "Writing"),
        Project::new("g-p-ideas", "Ideas"),
        shared,
    ];
    state.legacy_rename.insert("r-legacy".into());
    state.project_500.insert("n-01".into());
    state.unconfirmed_memories.insert("mem-ccc".into());
    state.memories = Some(vec![
        json!({ "id": "mem-aaa", "content": "Prefers tea,\n not \"coffee\"", "updated_at": "2026-09-01T00:00:00Z",
                "status": "active", "conversation_id": null, "gizmo_id": null, "created_timestamp": 1.5,
                "last_updated": null, "labels": null }),
        json!({ "id": "mem-bbb", "content": "y ".repeat(80), "updated_at": "2026-09-02T00:00:00Z",
                "status": "active", "conversation_id": "n-01" }),
        json!({ "id": "mem-ccc", "content": "Unconfirmable", "updated_at": "2026-09-03T00:00:00Z",
                "status": "active", "conversation_id": null }),
    ]);
    state.memory_summary = json!({
        "sections": [{ "id": "1", "title": "Work", "description": "Builds CLIs" },
                     { "id": "2", "title": "Home", "description": "Two kids" }],
        "generatedAtIso": "2026-09-28T00:00:00.000Z", "emptyStateMessage": null, "sourceChecksum": "c",
    });
}

/// Judgments in the TS index both CLIs read: a confident delete, an unsure
/// one, and none at all (the v0.1.5 harness's mix).
fn fill_ts_index(db: &rusqlite::Connection) {
    let unsure = {
        let mut answers: Value = serde_json::from_str(&delete_answers("other")).unwrap();
        answers["personal_record"]["noul"] = json!(0.45);
        answers["nothing_there"]["noul"] = json!(0.5);
        answers["re_askable"]["noul"] = json!(0.75);
        answers.to_string()
    };
    for chat in chats().iter().filter(|chat| chat.id.starts_with("n-")) {
        let n: usize = chat.id[2..].parse().unwrap();
        let answers = match n % 3 {
            0 => delete_answers("other"),
            1 => unsure.clone(),
            _ => continue,
        };
        db.execute(
            "insert into judgments (id, update_time, version, content_kind, answers, classified_at)
             values (?, ?, ?, 'full', ?, '2026-09-30T00:00:00.000Z')",
            rusqlite::params![chat.id, chat.update_time, QUESTIONS_VERSION, answers],
        )
        .unwrap();
    }
}

/// The exit code, stdout and stderr of one Rust CLI run.
fn rust(env: &Env, args: &[&str]) -> (Option<i32>, String, String) {
    let output = env.cmd().args(args).output().unwrap();
    (
        output.status.code(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

/// One run of one CLI: what it printed and what it asked for.
struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
    writes: Vec<String>,
}

/// What differs between the two sides, if anything. Exit codes are compared
/// as success or failure, because the Rust CLI's invalid-input errors exit
/// 2 where the TS CLI exits 1 (documented in
/// `docs/issues/mutations-followups.md`). The requests are sorted, because
/// the Rust CLI runs a bulk change with three workers at once.
fn difference(ts: &Run, rust: &Run) -> Option<String> {
    let mut out = Vec::new();
    if (ts.code == Some(0)) != (rust.code == Some(0)) {
        out.push(format!("exit: TS {:?}, Rust {:?}", ts.code, rust.code));
    }
    if ts.stdout != rust.stdout {
        out.push(format!(
            "stdout\nTS: {:?}\nRust: {:?}",
            ts.stdout, rust.stdout
        ));
    }
    let ts_stderr = normalize(&ts.stderr);
    let rust_stderr = normalize(&rust.stderr);
    if ts_stderr != rust_stderr {
        out.push(format!("stderr\nTS:\n{ts_stderr}\nRust:\n{rust_stderr}"));
    }
    let mut ts_writes: Vec<&str> = ts.writes.iter().map(String::as_str).collect();
    let mut rust_writes: Vec<&str> = rust.writes.iter().map(String::as_str).collect();
    ts_writes.sort_unstable();
    rust_writes.sort_unstable();
    if ts_writes != rust_writes {
        out.push(format!(
            "requests\nTS: {ts_writes:?}\nRust: {rust_writes:?}"
        ));
    }
    if out.is_empty() {
        None
    } else {
        Some(out.join("\n"))
    }
}

/// One chat's state in a fake or an index.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ChatState {
    id: String,
    title: String,
    archived: bool,
    project: Option<String>,
}

/// Every chat a fake has: its id, title, archived flag and project.
fn chat_states(fake: &FakeChatGpt) -> Vec<ChatState> {
    let mut chats: Vec<_> = fake
        .state()
        .chats
        .iter()
        .map(|chat| ChatState {
            id: chat.id.clone(),
            title: chat.title.clone(),
            archived: chat.archived,
            project: chat.gizmo_id.clone(),
        })
        .collect();
    chats.sort_by(|a, b| a.id.cmp(&b.id));
    chats
}

/// Every chat an index has: its id, title, archived flag and project.
fn index_states(db: &rusqlite::Connection) -> Vec<ChatState> {
    let mut rows = db
        .prepare("select id, title, is_archived, project_id from conversations order by id")
        .unwrap();
    let mut chats: Vec<ChatState> = rows
        .query_map([], |row| {
            Ok(ChatState {
                id: row.get(0)?,
                title: row.get(1)?,
                archived: row.get::<_, i64>(2)? != 0,
                project: row.get(3)?,
            })
        })
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    chats.sort_by(|a, b| a.id.cmp(&b.id));
    chats
}

/// The manual local titles an index holds, by chat id.
fn local_titles(db: &rusqlite::Connection) -> Vec<(String, String)> {
    let mut rows = db
        .prepare("select id, title from local_titles where source = 'manual' order by id")
        .unwrap();
    rows.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(|row| row.unwrap())
        .collect()
}

/// One scenario: the command, and what it must do.
struct Scenario {
    args: &'static [&'static str],
    /// The exit status the TS CLI must give.
    ts_code: i32,
    /// The exit status the Rust CLI must give: 2 where the TS CLI gives 1
    /// (invalid input; `docs/issues/mutations-followups.md`).
    rust_code: i32,
    /// The stdout both must print, byte for byte.
    stdout: &'static str,
    /// The requests both must send, in any order.
    writes: &'static [&'static str],
    /// Chats that must be archived (true) or unarchived (false) afterwards,
    /// in both fakes and both indexes.
    archived: &'static [(&'static str, bool)],
    /// Chats that must be gone from both fakes and both indexes.
    gone: &'static [&'static str],
    /// Chats whose title must have changed in ChatGPT, to what.
    renamed: &'static [(&'static str, &'static str)],
    /// The titles the indexes must hold: a rename that failed (a legacy
    /// chat's 500) changes ChatGPT but leaves the index as it was.
    index_titles: &'static [(&'static str, &'static str)],
    /// Chats whose project must have changed, to what.
    moved: &'static [(&'static str, Option<&'static str>)],
    /// Memories that must be gone from both fakes.
    memories_gone: &'static [&'static str],
    /// The manual local titles both indexes must hold.
    local_titles: &'static [(&'static str, &'static str)],
    /// The names of the projects each CLI must create, in order: each one
    /// a `POST /backend-api/projects` with ChatGPT's defaults, and a project
    /// in both fakes. Empty means neither CLI may create one.
    created: &'static [&'static str],
}

/// The scenarios, in the order they run. Each one's expectations are the
/// state it must leave behind, not only what the two CLIs agree on.
fn scenarios() -> Vec<Scenario> {
    vec![
        // Previews and dry runs: nothing is written.
        Scenario {
            args: &["archive", "--older-than", "1y", "-n"],
            ts_code: 0,
            rust_code: 0,
            stdout: "",
            writes: &[],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["archive", "--older-than", "1y", "--pinned", "--dry-run"],
            ts_code: 0,
            rust_code: 0,
            stdout: "",
            writes: &[],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["delete", "--title", "^old note 1", "--limit", "3", "-n"],
            ts_code: 0,
            rust_code: 0,
            stdout: "",
            writes: &[],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["unarchive", "--older-than", "1y", "-n"],
            ts_code: 0,
            rust_code: 0,
            stdout: "",
            writes: &[],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["unarchive", "--all", "--title", "archived|fresh", "-n"],
            ts_code: 0,
            rust_code: 0,
            stdout: "",
            writes: &[],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["archive", "--brainstorm", "-n"],
            ts_code: 0,
            rust_code: 0,
            stdout: "",
            writes: &[],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        // Selection errors: the TS CLI exits 1, the Rust CLI 2.
        Scenario {
            args: &["delete", "-y"],
            ts_code: 1,
            rust_code: 2,
            stdout: "",
            writes: &[],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["archive", "nope", "-y"],
            ts_code: 1,
            rust_code: 2,
            stdout: "",
            writes: &[],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["archive", "z-arch0", "-y"],
            ts_code: 1,
            rust_code: 2,
            stdout: "",
            writes: &[],
            archived: &[("z-arch0", true)],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["unarchive", "f-fresh", "-y"],
            ts_code: 1,
            rust_code: 2,
            stdout: "",
            writes: &[],
            archived: &[("f-fresh", false)],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["archive", "n-0", "-y"],
            ts_code: 1,
            rust_code: 2,
            stdout: "",
            writes: &[],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["archive", "--older-than", "3x", "-n"],
            ts_code: 1,
            rust_code: 2,
            stdout: "",
            writes: &[],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["archive", "--suggest", "burn", "-n"],
            ts_code: 1,
            rust_code: 2,
            stdout: "",
            writes: &[],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        // The cached verdicts, previewed.
        Scenario {
            args: &["delete", "--suggest", "delete", "-n"],
            ts_code: 0,
            rust_code: 0,
            stdout: "",
            writes: &[],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &[
                "archive",
                "--suggest",
                "archive",
                "--older-than",
                "1y",
                "-n",
            ],
            ts_code: 0,
            rust_code: 0,
            stdout: "",
            writes: &[],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        // The changes.
        Scenario {
            args: &["archive", "n-02", "n-04", "-y"],
            ts_code: 0,
            rust_code: 0,
            stdout: "",
            writes: &[
                "PATCH /backend-api/conversation/n-02",
                "PATCH /backend-api/conversation/n-04",
            ],
            archived: &[("n-02", true), ("n-04", true)],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["unarchive", "n-02", "-y"],
            ts_code: 0,
            rust_code: 0,
            stdout: "",
            writes: &["PATCH /backend-api/conversation/n-02"],
            archived: &[("n-02", false)],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["delete", "n-05", "n-08", "n-11", "-y"],
            ts_code: 0,
            rust_code: 0,
            stdout: "",
            writes: &[
                "DELETE /backend-api/conversation/id/n-05",
                "DELETE /backend-api/conversation/id/n-08",
                "DELETE /backend-api/conversation/id/n-11",
            ],
            archived: &[],
            gone: &["n-05", "n-08", "n-11"],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &[
                "delete",
                "--suggest",
                "delete",
                "--older-than",
                "1y",
                "--limit",
                "2",
                "-y",
            ],
            ts_code: 0,
            rust_code: 0,
            stdout: "",
            writes: &[
                "DELETE /backend-api/conversation/id/n-21",
                "DELETE /backend-api/conversation/id/n-27",
            ],
            archived: &[],
            gone: &["n-21", "n-27"],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["rename", "n-10", "Renamed note"],
            ts_code: 0,
            rust_code: 0,
            stdout: "",
            writes: &["POST /backend-api/conversation/id/n-10/rename"],
            archived: &[],
            gone: &[],
            renamed: &[("n-10", "Renamed note")],
            index_titles: &[("n-10", "Renamed note")],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        // A legacy chat's rename answers 500 and applies: both CLIs report
        // the failure and exit 1, and the title changed.
        Scenario {
            args: &["rename", "r-legacy", "Still legacy"],
            ts_code: 1,
            rust_code: 1,
            stdout: "",
            writes: &["POST /backend-api/conversation/id/r-legacy/rename"],
            archived: &[],
            gone: &[],
            renamed: &[("r-legacy", "Still legacy")],
            // The rename failed (a 500), so neither index learned of it.
            index_titles: &[("r-legacy", "Legacy chat")],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["title", "n-13", "  A   local title  "],
            ts_code: 0,
            rust_code: 0,
            stdout: "",
            writes: &[],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[("n-13", "A local title")],
            created: &[],
        },
        Scenario {
            args: &["title", "n-13", " "],
            ts_code: 1,
            rust_code: 2,
            stdout: "",
            writes: &[],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[("n-13", "A local title")],
            created: &[],
        },
        // The projects.
        Scenario {
            args: &["project", "list"],
            ts_code: 0,
            rust_code: 0,
            stdout: "g-p-writing  Writing\ng-p-ideas  Ideas\ng-p-shared  Shared (read-only)\n",
            writes: &[SIDEBAR],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &[
                "project", "add", "Ideas", "n-14", "n-15", "n-15", "n-01", "-n",
            ],
            ts_code: 0,
            rust_code: 0,
            stdout: "",
            writes: &[SIDEBAR],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        // n-01's move answers 500 and applies: both CLIs read the chat back
        // to see it landed, and both moved it.
        Scenario {
            args: &["project", "add", "ideas", "n-14", "n-01", "-y"],
            ts_code: 0,
            rust_code: 0,
            stdout: "",
            writes: &[
                "GET /backend-api/conversation/n-01",
                SIDEBAR,
                "PATCH /backend-api/conversation/n-01",
                "PATCH /backend-api/conversation/n-14",
            ],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[("n-01", Some("g-p-ideas")), ("n-14", Some("g-p-ideas"))],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["project", "remove", "g-p-ideas", "n-14", "n-16", "-y"],
            ts_code: 0,
            rust_code: 0,
            stdout: "",
            writes: &[SIDEBAR, "PATCH /backend-api/conversation/n-14"],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[("n-14", None)],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["project", "remove", "Ideas", "n-16", "-y"],
            ts_code: 0,
            rust_code: 0,
            stdout: "",
            writes: &[SIDEBAR],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        // A read-only project, and an unknown one: the TS CLI exits 1, the
        // Rust CLI 2.
        Scenario {
            args: &["project", "add", "Shared", "n-16", "-y"],
            ts_code: 1,
            rust_code: 2,
            stdout: "",
            writes: &[SIDEBAR],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["project", "add", "g-p", "n-16", "-y"],
            ts_code: 1,
            rust_code: 2,
            stdout: "",
            writes: &[SIDEBAR],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        // New projects: after every scenario that lists the projects, so
        // the ones made here change no other scenario's output.
        Scenario {
            args: &["project", "create", "  chatgpt-cli test  "],
            ts_code: 0,
            rust_code: 0,
            stdout: "g-p-new1  chatgpt-cli test\n",
            writes: &[SIDEBAR, CREATE_PROJECT],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &["chatgpt-cli test"],
        },
        // A name that is a project's already, ignoring case, and a blank
        // one: the TS CLI exits 1, the Rust CLI 2, and neither creates one.
        Scenario {
            args: &["project", "create", "WRITING"],
            ts_code: 1,
            rust_code: 2,
            stdout: "",
            writes: &[SIDEBAR],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["project", "create", " "],
            ts_code: 1,
            rust_code: 2,
            stdout: "",
            writes: &[],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["project", "create", "Another", "--json"],
            ts_code: 0,
            rust_code: 0,
            stdout: "{\n  \"id\": \"g-p-new2\",\n  \"name\": \"Another\",\n  \"canWrite\": true\n}\n",
            writes: &[SIDEBAR, CREATE_PROJECT],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &["Another"],
        },
        // The memories: read before any is deleted, so all three are there.
        Scenario {
            args: &["memory", "list"],
            ts_code: 0,
            rust_code: 0,
            stdout: concat!(
                "ID                                    UPDATED      CONTENT\n",
                "mem-aaa  2026-09-01  Prefers tea, not \"coffee\"\n",
                "mem-bbb  2026-09-02  y y y y y y y y y y y y y y y y y y y y y y y y y y y y y y ",
                "y y y y y y y y y y y y y y y y y y y y y y y y y y y y y y…\n",
                "mem-ccc  2026-09-03  Unconfirmable\n",
            ),
            writes: &[MEMORIES],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["memory", "list", "--format", "ids"],
            ts_code: 0,
            rust_code: 0,
            stdout: "mem-aaa\nmem-bbb\nmem-ccc\n",
            writes: &[MEMORIES],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["memory", "list", "--format", "csv", "--search", "TEA"],
            ts_code: 0,
            rust_code: 0,
            stdout: "id,content,updated_at,status,conversation_id\nmem-aaa,\"Prefers tea,\n not \"\"coffee\"\"\",2026-09-01T00:00:00Z,active,\n",
            writes: &[MEMORIES],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        // As ChatGPT sent it: every field, in its order, the float as is.
        Scenario {
            args: &["memory", "list", "--format", "json", "--limit", "1"],
            ts_code: 0,
            rust_code: 0,
            stdout: concat!(
                "[\n  {\n",
                "    \"id\": \"mem-aaa\",\n",
                "    \"content\": \"Prefers tea,\\n not \\\"coffee\\\"\",\n",
                "    \"updated_at\": \"2026-09-01T00:00:00Z\",\n",
                "    \"status\": \"active\",\n",
                "    \"conversation_id\": null,\n",
                "    \"gizmo_id\": null,\n",
                "    \"created_timestamp\": 1.5,\n",
                "    \"last_updated\": null,\n",
                "    \"labels\": null\n",
                "  }\n]\n",
            ),
            writes: &[MEMORIES],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["memory", "list", "--format", "ids", "--search", "zzz"],
            ts_code: 0,
            rust_code: 0,
            stdout: "",
            writes: &[MEMORIES],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["memory", "list", "--format", "xml"],
            ts_code: 1,
            rust_code: 2,
            stdout: "",
            writes: &[],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["memory", "summary"],
            ts_code: 0,
            rust_code: 0,
            stdout: "Work\nBuilds CLIs\n\nHome\nTwo kids\n",
            writes: &[MEMORY_SUMMARY],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["memory", "summary", "--format", "json"],
            ts_code: 0,
            rust_code: 0,
            stdout: concat!(
                "{\n  \"sections\": [\n",
                "    {\n      \"id\": \"1\",\n      \"title\": \"Work\",\n      \"description\": \"Builds CLIs\"\n    },\n",
                "    {\n      \"id\": \"2\",\n      \"title\": \"Home\",\n      \"description\": \"Two kids\"\n    }\n",
                "  ],\n",
                "  \"generatedAtIso\": \"2026-09-28T00:00:00.000Z\",\n",
                "  \"emptyStateMessage\": null,\n",
                "  \"sourceChecksum\": \"c\"\n}\n",
            ),
            writes: &[MEMORY_SUMMARY],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["memory", "summary", "--format", "xml"],
            ts_code: 1,
            rust_code: 2,
            stdout: "",
            writes: &[],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["memory", "delete", "mem-a", "MEM-AAA", "-n"],
            ts_code: 0,
            rust_code: 0,
            stdout: "",
            writes: &[MEMORIES],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        Scenario {
            args: &["memory", "delete", "mem", "-n"],
            ts_code: 1,
            rust_code: 2,
            stdout: "",
            writes: &[MEMORIES],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &[],
            local_titles: &[],
            created: &[],
        },
        // mem-ccc's delete is not confirmed: both CLIs report it as failed
        // and exit 1, and both deleted what ChatGPT answered.
        Scenario {
            args: &["memory", "delete", "mem-aaa", "mem-ccc", "-y"],
            ts_code: 1,
            rust_code: 1,
            stdout: "",
            writes: &[
                "DELETE /backend-api/memories/mem-aaa",
                "DELETE /backend-api/memories/mem-ccc",
                MEMORIES,
            ],
            archived: &[],
            gone: &[],
            renamed: &[],
            index_titles: &[],
            moved: &[],
            memories_gone: &["mem-aaa", "mem-ccc"],
            local_titles: &[],
            created: &[],
        },
    ]
}

/// One chat's state in a fake or an index, or a failure that says what is
/// there instead.
fn state_of(states: &[ChatState], id: &str, context: &str) -> ChatState {
    let found = states.iter().find(|state| state.id == id);
    assert!(found.is_some(), "{context} has no chat {id}: {states:?}");
    found.unwrap().clone()
}

/// The two CLIs, their two fake chatgpt.coms and their two fake TypeSafes,
/// set up alike: the same chats, the same projects, the same memories, the
/// same verdicts, and an index each. `None` when the TypeScript CLI's
/// checkout or a `bun` that runs is missing.
struct Parity {
    env: Env,
    ts: Ts,
    ts_fake: FakeChatGpt,
    rust_typesafe: FakeTypeSafe,
    ts_typesafe: FakeTypeSafe,
    ts_db_path: PathBuf,
    rust_index: PathBuf,
}

fn parity() -> Option<Parity> {
    let Some(repo) = ts_repo() else {
        eprintln!(
            "skipping the TS parity harness: set CHATGPT_TS_REPO to the TypeScript CLI's checkout"
        );
        return None;
    };
    let mut env = Env::with_fake(Vec::new());
    set_up(env.fake());
    let ts_fake = FakeChatGpt::start(Vec::new());
    set_up(&ts_fake);
    // `CHATGPT_PARITY_SEED_DIFFERENCE=1` breaks the TS side before the
    // scenarios, to show that the harness fails with both sides printed
    // when the two CLIs differ.
    if std::env::var_os("CHATGPT_PARITY_SEED_DIFFERENCE").is_some() {
        ts_fake.state().projects[0].name = "Writing (seeded)".into();
    }
    let rust_typesafe = FakeTypeSafe::start();
    let ts_typesafe = FakeTypeSafe::start();
    env.extra_env.extend([
        ("TYPESAFE_API_KEY".to_owned(), API_KEY.to_owned()),
        ("TYPESAFE_BASE_URL".to_owned(), rust_typesafe.url.clone()),
    ]);
    // The Rust index, synced from its own fake.
    env.cmd().arg("sync").assert().success();
    env.wait_for_indexer();
    let rust_index = env.data_dir().join("chatgpt.db");
    // The TS CLI's own index: the same chats, the same verdicts.
    let work = env.home.path().join("parity");
    let ts_home = work.join("home");
    std::fs::create_dir_all(&ts_home).unwrap();
    std::fs::create_dir_all(work.join("config")).unwrap();
    std::fs::create_dir_all(work.join("cache")).unwrap();
    let ts_data_home = work.join("data");
    let ts_db_path = ts_index(&ts_data_home);
    {
        let db = rusqlite::Connection::open(&ts_db_path).unwrap();
        fill_ts_index(&db);
    }
    assert_eq!(copy_judgments(&ts_db_path, &rust_index), 22);
    assert_eq!(copy_chats(&rust_index, &ts_db_path), chats().len());

    let plugin = work.join("plugin.ts");
    std::fs::write(&plugin, PARITY_PLUGIN).unwrap();
    let client = std::fs::read_to_string(repo.join("src/api/client.ts")).unwrap();
    assert!(
        client.contains("const BASE = \"https://chatgpt.com\";"),
        "src/api/client.ts changed: update the parity plugin"
    );
    let Some(bun) = bun_for(&ts_home) else {
        eprintln!("skipping the TS parity harness: no bun on PATH runs with a synthetic HOME");
        return None;
    };
    let ts = Ts {
        bun,
        repo,
        work,
        data_home: ts_data_home,
        home: ts_home,
        plugin,
        tools: env.tools.clone(),
        fake_url: ts_fake.url.clone(),
        typesafe_url: Some(ts_typesafe.url.clone()),
        openai_url: None,
        anthropic_url: None,
        openai_key: false,
        anthropic_key: false,
    };
    Some(Parity {
        env,
        ts,
        ts_fake,
        rust_typesafe,
        ts_typesafe,
        ts_db_path,
        rust_index,
    })
}

#[test]
fn the_whole_write_path_prints_and_writes_as_the_ts_cli_does() {
    let Some(parity) = parity() else {
        return;
    };
    let Parity {
        env,
        ts,
        ts_fake,
        rust_typesafe,
        ts_typesafe,
        ts_db_path,
        rust_index,
    } = parity;
    for scenario in scenarios() {
        let args = scenario.args;
        let rust_seen = writes(env.fake()).len();
        let ts_seen = writes(&ts_fake).len();
        let rust_bodies_seen = env.fake().state().project_bodies.len();
        let ts_bodies_seen = ts_fake.state().project_bodies.len();
        let (ts_code, ts_out, ts_err) = ts.run(args);
        let (rust_code, rust_out, rust_err) = rust(&env, args);
        env.wait_for_indexer();
        let ts_writes: Vec<String> = writes(&ts_fake)[ts_seen..].to_vec();
        let rust_writes: Vec<String> = writes(env.fake())[rust_seen..].to_vec();
        let ts_run = Run {
            code: ts_code,
            stdout: ts_out.clone(),
            stderr: ts_err.clone(),
            writes: ts_writes.clone(),
        };
        let rust_run = Run {
            code: rust_code,
            stdout: rust_out.clone(),
            stderr: rust_err.clone(),
            writes: rust_writes.clone(),
        };

        // The proof: the two sides, side by side.
        eprintln!("======== {args:?}");
        eprintln!("--- TS exit: {ts_code:?}  stdout: {ts_out:?}");
        eprintln!("--- TS stderr:\n{ts_err}");
        eprintln!("--- Rust exit: {rust_code:?}  stdout: {rust_out:?}");
        eprintln!("--- Rust stderr:\n{rust_err}");
        eprintln!("--- TS requests: {ts_writes:?}");
        eprintln!("--- Rust requests: {rust_writes:?}");

        // The two CLIs must agree, and a difference must be printed with
        // both sides.
        let found = difference(&ts_run, &rust_run);
        assert!(
            found.is_none(),
            "{args:?} differs:\n{}",
            found.unwrap_or_default()
        );
        // The exit statuses, each one the documented one.
        assert_eq!(
            ts_code,
            Some(scenario.ts_code),
            "{args:?} TS exit: {ts_code:?} (stderr: {ts_err})"
        );
        assert_eq!(
            rust_code,
            Some(scenario.rust_code),
            "{args:?} Rust exit: {rust_code:?} (stderr: {rust_err})"
        );
        // The stdout, byte for byte, and the requests, in any order.
        assert_eq!(ts_out, scenario.stdout, "{args:?} TS stdout");
        assert_eq!(rust_out, scenario.stdout, "{args:?} Rust stdout");
        let mut expected: Vec<&str> = scenario.writes.to_vec();
        expected.sort_unstable();
        let mut ts_writes_sorted: Vec<&str> = ts_writes.iter().map(String::as_str).collect();
        ts_writes_sorted.sort_unstable();
        let mut rust_writes_sorted: Vec<&str> = rust_writes.iter().map(String::as_str).collect();
        rust_writes_sorted.sort_unstable();
        assert_eq!(ts_writes_sorted, expected, "{args:?} TS requests");
        assert_eq!(rust_writes_sorted, expected, "{args:?} Rust requests");

        // The end state, not only that the two CLIs agree: agreeing is not
        // enough, because both could have done nothing at all.
        let rust_fake_states = chat_states(env.fake());
        let ts_fake_states = chat_states(&ts_fake);
        let ts_db = rusqlite::Connection::open(&ts_db_path).unwrap();
        let rust_db = rusqlite::Connection::open(&rust_index).unwrap();
        let ts_index_states = index_states(&ts_db);
        let rust_index_states = index_states(&rust_db);
        assert_eq!(
            rust_fake_states, ts_fake_states,
            "{args:?} the two fakes hold different chats"
        );
        assert_eq!(
            rust_index_states, ts_index_states,
            "{args:?} the two indexes hold different chats"
        );
        for (id, archived) in scenario.archived {
            for (side, states) in [
                ("Rust fake", &rust_fake_states),
                ("TS fake", &ts_fake_states),
                ("Rust index", &rust_index_states),
                ("TS index", &ts_index_states),
            ] {
                let state = state_of(states, id, &format!("{args:?} {side}"));
                assert_eq!(
                    state.archived, *archived,
                    "{args:?} {side} has {id} archived = {}",
                    state.archived
                );
            }
        }
        for id in scenario.gone {
            for (side, states) in [
                ("Rust fake", &rust_fake_states),
                ("TS fake", &ts_fake_states),
                ("Rust index", &rust_index_states),
                ("TS index", &ts_index_states),
            ] {
                assert!(
                    !states.iter().any(|state| state.id == *id),
                    "{args:?} {side} still has {id}"
                );
            }
        }
        for (id, title) in scenario.renamed {
            for (side, states) in [
                ("Rust fake", &rust_fake_states),
                ("TS fake", &ts_fake_states),
            ] {
                let state = state_of(states, id, &format!("{args:?} {side}"));
                assert_eq!(state.title, *title, "{args:?} {side} has {id} titled");
            }
        }
        for (id, title) in scenario.index_titles {
            for (side, states) in [
                ("Rust index", &rust_index_states),
                ("TS index", &ts_index_states),
            ] {
                let state = state_of(states, id, &format!("{args:?} {side}"));
                assert_eq!(state.title, *title, "{args:?} {side} has {id} titled");
            }
        }
        for (id, project) in scenario.moved {
            for (side, states) in [
                ("Rust fake", &rust_fake_states),
                ("TS fake", &ts_fake_states),
                ("Rust index", &rust_index_states),
                ("TS index", &ts_index_states),
            ] {
                let state = state_of(states, id, &format!("{args:?} {side}"));
                assert_eq!(
                    state.project.as_deref(),
                    *project,
                    "{args:?} {side} has {id} in another project"
                );
            }
        }
        for id in scenario.memories_gone {
            for (side, fake) in [("Rust fake", env.fake()), ("TS fake", &ts_fake)] {
                let memories = fake.state().memories.clone().unwrap_or_default();
                assert!(
                    !memories.iter().any(|memory| memory["id"] == *id),
                    "{args:?} {side} still has the memory {id}"
                );
            }
        }
        for (id, title) in scenario.local_titles {
            for (side, db) in [("Rust index", &rust_db), ("TS index", &ts_db)] {
                assert!(
                    local_titles(db).contains(&(id.to_string(), (*title).to_owned())),
                    "{args:?} {side} has no local title for {id}: {:?}",
                    local_titles(db)
                );
            }
        }
        // The projects created: each one asked for with ChatGPT's defaults
        // and there in both fakes, and none at all when none is expected.
        let expected_bodies: Vec<Value> = scenario
            .created
            .iter()
            .map(|name| {
                json!({ "emoji": null, "instructions": "", "memory_scope": "unset",
                        "name": name, "theme": null })
            })
            .collect();
        for (side, fake, seen) in [
            ("Rust fake", env.fake(), rust_bodies_seen),
            ("TS fake", &ts_fake, ts_bodies_seen),
        ] {
            assert_eq!(
                fake.state().project_bodies[seen..],
                expected_bodies,
                "{args:?} {side} was not asked to create the expected projects"
            );
        }
        for name in scenario.created {
            for (side, fake) in [("Rust fake", env.fake()), ("TS fake", &ts_fake)] {
                let names: Vec<String> = fake
                    .state()
                    .projects
                    .iter()
                    .map(|project| project.name.clone())
                    .collect();
                assert!(
                    names.iter().any(|project| project == name),
                    "{args:?} {side} has no project {name:?}: {names:?}"
                );
            }
        }
    }

    // The Jev requests: the same ones on both sides, byte for byte. The
    // scenarios above ask no Jev question (the verdicts are cached), so
    // both sides must have asked none.
    let mut rust_bodies = rust_typesafe.state().bodies.clone();
    let mut ts_bodies = ts_typesafe.state().bodies.clone();
    rust_bodies.sort();
    ts_bodies.sort();
    assert_eq!(
        rust_bodies, ts_bodies,
        "the two CLIs asked Jev different questions"
    );
    assert!(
        rust_bodies.is_empty(),
        "a scenario asked Jev a question: {rust_bodies:?}"
    );

    // The harness must not hide a difference: seed one in the TS side's
    // projects and check that the comparison sees it.
    ts_fake.state().projects[0].name = "Writing (seeded)".into();
    let args = ["project", "list"];
    let (ts_code, ts_out, ts_err) = ts.run(&args);
    let (rust_code, rust_out, rust_err) = rust(&env, &args);
    let ts_run = Run {
        code: ts_code,
        stdout: ts_out,
        stderr: ts_err,
        writes: Vec::new(),
    };
    let rust_run = Run {
        code: rust_code,
        stdout: rust_out,
        stderr: rust_err,
        writes: Vec::new(),
    };
    let found = difference(&ts_run, &rust_run).expect("the seeded difference was not seen");
    eprintln!("--- the seeded difference:\n{found}");
    assert!(found.contains("Writing (seeded)"), "{found}");
}

/// The TypeScript CLI must not be able to reach a real service: no real
/// TypeSafe, no real OpenAI or Anthropic key, no real `codex` or `claude`.
/// The environment it runs with is checked as it is built, and a
/// Jev-asking command is shown to reach the fake, not a real service.
#[test]
fn the_ts_cli_cannot_reach_a_real_service() {
    let Some(parity) = parity() else {
        return;
    };
    let Parity {
        env,
        ts,
        ts_fake,
        ts_typesafe,
        ..
    } = parity;

    // The environment a command would run with, as built.
    let environment = |ts: &Ts| -> Vec<(String, Option<String>)> {
        ts.command(&["--help"])
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect()
    };
    let value = |env: &[(String, Option<String>)], name: &str| {
        env.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };

    // The sealed environment: only the stand-ins on the PATH, the fake's
    // TypeSafe, the fake's key, and no key that could reach a provider.
    let sealed = environment(&ts);
    assert_eq!(
        value(&sealed, "PATH"),
        Some(Some(support::tools_path(&ts.tools))),
        "the TS CLI's PATH is not the stand-ins alone"
    );
    assert_eq!(
        value(&sealed, "TYPESAFE_BASE_URL"),
        Some(Some(ts_typesafe.url.clone())),
        "the TS CLI's TypeSafe base URL is not the fake's"
    );
    assert_eq!(
        value(&sealed, "TYPESAFE_API_KEY"),
        Some(Some(API_KEY.to_owned())),
        "the TS CLI's TypeSafe key is not the fake's"
    );
    for name in [
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
    ] {
        assert_eq!(
            value(&sealed, name),
            Some(None),
            "the TS CLI's environment still carries {name}"
        );
    }
    // Without a fake TypeSafe the base URL is still set: an address
    // nothing answers, never the SDK's default (api.typesafe.ai).
    let without = Ts {
        typesafe_url: None,
        ..ts.clone()
    };
    assert_eq!(
        value(&environment(&without), "TYPESAFE_BASE_URL"),
        Some(Some(support::UNREACHABLE.to_owned())),
        "the TS CLI's TypeSafe base URL could fall back to the real service"
    );
    assert_eq!(
        value(&environment(&without), "OPENAI_API_KEY"),
        Some(None),
        "the TS CLI's environment still carries an OpenAI key"
    );

    // And the end to end: a command that asks Jev reaches the fake, which
    // records the request. Had the base URL fallen back to the real
    // service, the fake would have seen nothing and the request would
    // have carried the chat to api.typesafe.ai.
    let before = ts_typesafe.state().bodies.len();
    let (code, _out, err) = ts.run(&["delete", "j-junk", "--check", "-n"]);
    let after = ts_typesafe.state().bodies.len();
    eprintln!("--- the TS CLI asked the fake TypeSafe {after} time(s)");
    assert!(
        after > before,
        "the TS CLI's Jev request did not reach the fake (exit {code:?}): {err}"
    );
    // The request carried the chat, not a key: the fake answers only the
    // fake's key, so a request it answered had that key.
    assert!(
        ts_typesafe.state().bodies[before..]
            .iter()
            .all(|body| body.contains("Junk ping")),
        "the TS CLI's Jev request did not carry the chat it asked about"
    );
    // Nothing reached the developer's own account: the TS CLI's browser
    // selection is off, so it read no cookie but the fake's.
    assert!(
        ts_fake
            .state()
            .calls
            .iter()
            .any(|call| call.starts_with("GET /api/auth/session")),
        "the TS CLI did not read a session from the fake"
    );
    // The Rust side is sealed the same way.
    let rust_env: Vec<(String, Option<String>)> = env
        .std_cmd()
        .get_envs()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.map(|value| value.to_string_lossy().into_owned()),
            )
        })
        .collect();
    assert_eq!(
        value(&rust_env, "PATH"),
        Some(Some(support::tools_path(&env.tools))),
        "the Rust CLI's PATH is not the stand-ins alone"
    );
    for name in ["OPENAI_API_KEY", "ANTHROPIC_API_KEY"] {
        assert_eq!(
            value(&rust_env, name),
            Some(None),
            "the Rust CLI's environment still carries {name}"
        );
    }
    assert!(
        matches!(value(&rust_env, "TYPESAFE_BASE_URL"), Some(Some(_))),
        "the Rust CLI's TypeSafe base URL is not set"
    );
}

/// A write that answers 404: the chat is gone from ChatGPT, so both CLIs
/// fail, both say so without the body (the TS CLI quotes it; the Rust one
/// does not, by rule), and both drop the chat from their index.
#[test]
fn a_404_drops_the_chat_from_both_indexes() {
    let Some(parity) = parity() else {
        return;
    };
    let Parity {
        env,
        ts,
        ts_fake,
        ts_db_path,
        rust_index,
        ..
    } = parity;
    let args = ["archive", "n-06", "-y"];
    // Both fakes answer the write with a 404, without applying it.
    env.fake()
        .state()
        .fail_writes
        .push_back(fake_chatgpt::WriteFailure::Status(404));
    ts_fake
        .state()
        .fail_writes
        .push_back(fake_chatgpt::WriteFailure::Status(404));
    let rust_seen = writes(env.fake()).len();
    let ts_seen = writes(&ts_fake).len();
    let (ts_code, ts_out, ts_err) = ts.run(&args);
    let (rust_code, rust_out, rust_err) = rust(&env, &args);
    let ts_writes: Vec<String> = writes(&ts_fake)[ts_seen..].to_vec();
    let rust_writes: Vec<String> = writes(env.fake())[rust_seen..].to_vec();

    eprintln!("======== {args:?} (a 404)");
    eprintln!("--- TS exit: {ts_code:?}  stdout: {ts_out:?}");
    eprintln!("--- TS stderr:\n{ts_err}");
    eprintln!("--- Rust exit: {rust_code:?}  stdout: {rust_out:?}");
    eprintln!("--- Rust stderr:\n{rust_err}");
    eprintln!("--- TS requests: {ts_writes:?}");
    eprintln!("--- Rust requests: {rust_writes:?}");

    // Both failed, and both asked for the same one write.
    assert_ne!(ts_code, Some(0), "the TS CLI did not fail: {ts_err}");
    assert_ne!(rust_code, Some(0), "the Rust CLI did not fail: {rust_err}");
    assert_eq!(
        ts_writes,
        ["PATCH /backend-api/conversation/n-06"],
        "the TS CLI's requests"
    );
    assert_eq!(
        rust_writes,
        ["PATCH /backend-api/conversation/n-06"],
        "the Rust CLI's requests"
    );
    // The failure line: the TS CLI quotes the body, the Rust one does not
    // (`docs/issues/mutations-followups.md`: no bodies, by rule).
    assert!(
        ts_err.contains("404 from /backend-api/conversation/n-06"),
        "the TS CLI's failure line: {ts_err}"
    );
    assert!(
        ts_err.contains("secret body"),
        "the TS CLI did not quote the body: {ts_err}"
    );
    assert!(
        rust_err.contains("404 Not Found from /backend-api/conversation/n-06"),
        "the Rust CLI's failure line: {rust_err}"
    );
    assert!(
        !rust_err.contains("secret body"),
        "the Rust CLI quoted the body: {rust_err}"
    );
    // The expected end state: the chat is gone from both indexes, and
    // still in both fakes (the 404 was forced, so nothing was archived).
    let ts_db = rusqlite::Connection::open(&ts_db_path).unwrap();
    let rust_db = rusqlite::Connection::open(&rust_index).unwrap();
    let ts_index_states = index_states(&ts_db);
    let rust_index_states = index_states(&rust_db);
    assert!(
        !ts_index_states.iter().any(|state| state.id == "n-06"),
        "the TS index still has n-06: {ts_index_states:?}"
    );
    assert!(
        !rust_index_states.iter().any(|state| state.id == "n-06"),
        "the Rust index still has n-06: {rust_index_states:?}"
    );
    assert!(
        chat_states(&ts_fake).iter().any(|state| state.id == "n-06"),
        "the TS fake lost n-06"
    );
    assert!(
        chat_states(env.fake())
            .iter()
            .any(|state| state.id == "n-06"),
        "the Rust fake lost n-06"
    );
    // The two indexes agree: the same rows, the same titles.
    assert_eq!(rust_index_states, ts_index_states);
}
