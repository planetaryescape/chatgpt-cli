//! `project create|list|add|remove` and `memory list|summary|delete`
//! through the real binary and the fake chatgpt.com, with its quirks: a
//! project move that answers 500 after applying, a move or memory delete
//! ChatGPT doesn't confirm.

#![allow(clippy::unwrap_used)]

mod support;

use fake_chatgpt::{Chat, Project, WriteFailure};
use serde_json::{Value, json};
use support::{Env, in_terminal};

fn synced() -> Env {
    let mut moved = Chat::new("b-moved", "In a project", "2026-09-26T10:00:00.000000Z");
    moved.gizmo_id = Some("g-p-other".into());
    let mut writing = Chat::new("c-writing", "Draft", "2026-09-25T10:00:00.000000Z");
    writing.gizmo_id = Some("g-p-writing".into());
    let env = Env::with_fake(vec![
        Chat::new("a-loose", "Loose chat", "2026-09-27T10:00:00.000000Z"),
        moved,
        writing,
    ]);
    {
        let mut state = env.fake().state();
        let mut projects: Vec<Project> = (0..23)
            .map(|n| Project::new(&format!("g-p-filler{n:02}"), &format!("Filler {n}")))
            .collect();
        projects.insert(0, Project::new("g-p-writing", "Writing"));
        let mut read_only = Project::new("g-p-shared", "Shared");
        read_only.can_write = false;
        projects.push(read_only);
        let mut gone = Project::new("g-p-old", "Old");
        gone.archived = true;
        projects.push(gone);
        state.projects = projects;
        state.memories = Some(vec![
            json!({ "id": "mem-aaa", "content": "Prefers  tea,\n not \"coffee\"", "updated_at": "2026-09-01T00:00:00Z",
                    "status": "active", "conversation_id": null, "gizmo_id": null, "created_timestamp": 1.5,
                    "last_updated": null, "labels": null }),
            json!({ "id": "mem-bbb", "content": "x".repeat(130), "updated_at": "2026-09-02T00:00:00Z",
                    "status": "active", "conversation_id": "a-loose" }),
        ]);
    }
    env.cmd().arg("sync").assert().success();
    env
}

fn run(env: &Env, args: &[&str]) -> (Option<i32>, String, String) {
    let output = env.cmd().args(args).output().unwrap();
    (
        output.status.code(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

fn steady(stderr: &str) -> String {
    stderr
        .lines()
        // A running count draws a bar; a summary line doesn't.
        .filter(|line| !line.contains(['█', '░']))
        .map(|line| match line.rfind(" (") {
            Some(at) if line.ends_with("s)") => &line[..at],
            _ => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn project_of(env: &Env, id: &str) -> Option<String> {
    let rows: Vec<Value> = serde_json::from_str(&env.stdout(&["list", "--json"])).unwrap();
    rows.into_iter()
        .find(|row| row["id"] == id)
        .and_then(|row| row["project_id"].as_str().map(str::to_owned))
}

#[test]
fn projects_list_across_pages_and_new_names_must_be_new() {
    let env = synced();
    let listed = env.stdout(&["project", "list"]);
    assert_eq!(listed.lines().count(), 25, "every page, archived left out");
    assert!(listed.starts_with("g-p-writing  Writing\n"));
    assert!(listed.ends_with("g-p-shared  Shared (read-only)\n"));
    let calls: Vec<String> = env
        .fake()
        .calls()
        .into_iter()
        .filter(|call| call.contains("sidebar"))
        .collect();
    assert_eq!(
        calls,
        [
            "GET /backend-api/gizmos/snorlax/sidebar?conversations_per_gizmo=0&limit=20&owned_only=false",
            "GET /backend-api/gizmos/snorlax/sidebar?conversations_per_gizmo=0&limit=20&owned_only=false&cursor=page+20",
        ]
    );
    let limited: Value =
        serde_json::from_str(&env.stdout(&["project", "list", "--json", "--limit", "1"])).unwrap();
    assert_eq!(
        limited,
        json!([{ "id": "g-p-writing", "name": "Writing", "canWrite": true }])
    );
    let (_, _, stderr) = run(&env, &["project", "list", "--limit", "0"]);
    assert_eq!(stderr, "error: --limit must be a positive integer.\n");

    assert_eq!(
        env.stdout(&["project", "create", "  chatgpt-cli test  "]),
        "g-p-new1  chatgpt-cli test\n"
    );
    let (code, _, stderr) = run(&env, &["project", "create", "WRITING"]);
    assert_ne!(code, Some(0));
    assert_eq!(
        stderr,
        "error: Project \"Writing\" already exists (g-p-writing).\n"
    );
    let (_, _, stderr) = run(&env, &["project", "create", " "]);
    assert_eq!(stderr, "error: Project name cannot be empty.\n");
    let created: Value =
        serde_json::from_str(&env.stdout(&["project", "create", "Another", "--json"])).unwrap();
    assert_eq!(
        created,
        json!({ "id": "g-p-new2", "name": "Another", "canWrite": true })
    );
    assert_eq!(
        env.fake()
            .calls()
            .iter()
            .filter(|call| call.starts_with("POST /backend-api/projects"))
            .count(),
        2
    );
}

#[test]
fn chats_move_into_a_project_and_out_again() {
    let env = synced();
    let (code, _, stderr) = run(
        &env,
        &[
            "project",
            "add",
            "writ",
            "a-loose",
            "b-moved",
            "c-writing",
            "a-l",
            "-n",
        ],
    );
    assert_eq!(code, Some(2), "a name isn't a prefix: {stderr}");
    assert!(
        stderr.starts_with("error: No project matching \"writ\"."),
        "{stderr}"
    );
    let (code, _, stderr) = run(
        &env,
        &[
            "project",
            "add",
            "Writing",
            "a-loose",
            "b-moved",
            "c-writing",
            "a-l",
            "-n",
        ],
    );
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        stderr,
        "a-loose  Loose chat\nb-moved  In a project  (from g-p-other)\ndry run: would move 2 chat(s) to \"Writing\".\n"
    );
    let (code, _, stderr) = run(
        &env,
        &["project", "add", "g-p-writing", "a-loose", "b-moved", "-y"],
    );
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        steady(&stderr).ends_with("Moving chats to project…\nMoved 2 chat(s) to \"Writing\""),
        "{stderr}"
    );
    assert_eq!(project_of(&env, "a-loose").as_deref(), Some("g-p-writing"));
    let (_, _, stderr) = run(&env, &["project", "add", "Writing", "a-loose", "-y"]);
    assert_eq!(
        stderr,
        "No chats to move; all 1 are already in \"Writing\".\n"
    );

    let (code, _, stderr) = run(&env, &["project", "remove", "Writing", "a-loose", "-y"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        stderr.contains("Removed 1 chat(s) from \"Writing\""),
        "{stderr}"
    );
    assert_eq!(project_of(&env, "a-loose"), None);
    let (_, _, stderr) = run(&env, &["project", "remove", "Writing", "a-loose", "-y"]);
    assert_eq!(stderr, "No selected chats are in \"Writing\".\n");
    let (_, _, stderr) = run(&env, &["project", "add", "Shared", "a-loose", "-y"]);
    assert_eq!(
        stderr,
        "error: You do not have write access to project \"Shared\".\n"
    );
}

#[test]
fn a_move_answering_500_is_checked_and_an_unconfirmed_one_fails() {
    let env = synced();
    env.fake().state().project_500.insert("a-loose".into());
    let (code, _, stderr) = run(&env, &["project", "add", "Writing", "a-loose", "-y"]);
    assert_eq!(code, Some(0), "applied despite the 500: {stderr}");
    assert!(
        env.fake()
            .calls()
            .contains(&"GET /backend-api/conversation/a-loose".to_owned())
    );
    assert_eq!(project_of(&env, "a-loose").as_deref(), Some("g-p-writing"));

    env.fake()
        .state()
        .unconfirmed_moves
        .insert("b-moved".into());
    let (code, _, stderr) = run(&env, &["project", "add", "Writing", "b-moved", "-y"]);
    assert_eq!(code, Some(1));
    assert!(
        stderr.contains("failed: b-moved In a project: ChatGPT did not confirm changing the project of b-moved.\n"),
        "{stderr}"
    );
    assert_eq!(project_of(&env, "b-moved").as_deref(), Some("g-p-other"));

    // A move is sent once even when the gateway fails.
    env.fake()
        .state()
        .fail_writes
        .push_back(WriteFailure::Status(504));
    let (code, _, stderr) = run(&env, &["project", "remove", "Writing", "a-loose", "-y"]);
    assert_eq!(code, Some(1));
    assert!(stderr.contains("so it may have applied"), "{stderr}");
}

#[test]
fn deleting_a_project_previews_its_chats_wants_its_name_and_clears_the_index() {
    let env = synced();
    let (code, _, stderr) = run(&env, &["project", "delete", "writing", "-n"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        stderr,
        "Project \"Writing\" (g-p-writing)\n1 indexed chat(s) are in it:\nc-writing  Draft\n\
         ChatGPT may delete these chats along with the project, or only take them out of it. Treat them as deleted: export any you want to keep first.\n\
         dry run: would delete project \"Writing\".\n"
    );
    let deletes = |env: &Env| {
        env.fake()
            .calls()
            .into_iter()
            .filter(|call| call.starts_with("DELETE /backend-api/gizmos/"))
            .collect::<Vec<_>>()
    };
    assert!(deletes(&env).is_empty());

    let question = "This can't be undone. Type the project name (Writing) to delete it: ";
    let (code, shown) = in_terminal(
        &env,
        &["project", "delete", "g-p-writing"],
        None,
        question,
        "writing",
    );
    assert_eq!(code, Some(0), "{shown}");
    assert!(
        shown.ends_with("Cancelled: the name didn't match.\n"),
        "{shown}"
    );
    assert!(deletes(&env).is_empty());

    let (code, shown) = in_terminal(
        &env,
        &["project", "delete", "g-p-writing"],
        None,
        question,
        "Writing",
    );
    assert_eq!(code, Some(0), "{shown}");
    assert!(
        shown.ends_with(
            "Deleted project \"Writing\" (g-p-writing).\nTook 1 indexed chat(s) out of it; `chatgpt sync` shows whether ChatGPT kept them.\n"
        ),
        "{shown}"
    );
    assert_eq!(deletes(&env), ["DELETE /backend-api/gizmos/g-p-writing"]);
    assert_eq!(env.fake().state().deleted_projects, ["g-p-writing"]);
    assert_eq!(project_of(&env, "c-writing"), None);
    assert_eq!(project_of(&env, "b-moved").as_deref(), Some("g-p-other"));
    assert!(!env.stdout(&["project", "list"]).contains("Writing"));

    let (code, _, stderr) = run(&env, &["project", "delete", "Filler 1", "-y"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        stderr,
        "Project \"Filler 1\" (g-p-filler01)\nNo indexed chats are in it.\nDeleted project \"Filler 1\" (g-p-filler01).\n"
    );

    let (code, _, stderr) = run(&env, &["project", "delete", "Shared", "-y"]);
    assert_ne!(code, Some(0));
    assert_eq!(
        stderr,
        "error: You do not have write access to project \"Shared\".\n"
    );
}

#[test]
fn a_project_delete_is_sent_once_and_a_missing_project_says_so() {
    let env = synced();
    env.fake()
        .state()
        .fail_writes
        .push_back(WriteFailure::Status(504));
    let (code, _, stderr) = run(&env, &["project", "delete", "Writing", "-y"]);
    assert_ne!(code, Some(0));
    assert!(stderr.contains("so it may have applied"), "{stderr}");
    assert!(!stderr.contains("secret body"), "{stderr}");
    let deletes = env
        .fake()
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("DELETE /backend-api/gizmos/"))
        .count();
    assert_eq!(deletes, 1);
    assert_eq!(
        project_of(&env, "c-writing").as_deref(),
        Some("g-p-writing"),
        "the index is left as it was"
    );

    // Gone from ChatGPT since the list was read: the 404 says so.
    env.fake()
        .state()
        .fail_writes
        .push_back(WriteFailure::Status(404));
    let (code, _, stderr) = run(&env, &["project", "delete", "Writing", "-y"]);
    assert_ne!(code, Some(0));
    assert!(
        stderr
            .ends_with("error: ChatGPT has no project g-p-writing; run `chatgpt project list`.\n"),
        "{stderr}"
    );
}

/// `""` prefixes every id, so a blank project or chat reference would pick
/// the only one there is: each is refused before anything is sent.
#[test]
fn a_blank_project_or_chat_reference_is_refused() {
    let env = synced();
    env.fake().state().projects = vec![Project::new("g-p-only", "Only")];
    let (code, _, stderr) = run(&env, &["project", "delete", " ", "-y"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert_eq!(stderr, "error: Pass a project name or id.\n");
    let (code, _, stderr) = run(&env, &["project", "add", "", "a-loose", "-y"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert_eq!(stderr, "error: Pass a project name or id.\n");
    let (code, _, stderr) = run(&env, &["project", "remove", "  ", "c-writing", "-y"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(env.fake().state().deleted_projects.is_empty());
    assert_eq!(project_of(&env, "a-loose"), None);

    // With one chat indexed, `""` would be a unique prefix of it.
    let env = Env::with_fake(vec![Chat::new(
        "a-loose",
        "Loose chat",
        "2026-09-27T10:00:00.000000Z",
    )]);
    env.cmd().arg("sync").assert().success();
    env.fake().state().projects = vec![Project::new("g-p-only", "Only")];
    for id in ["", " "] {
        let (code, _, stderr) = run(&env, &["project", "add", "Only", id, "-y"]);
        assert_eq!(code, Some(2), "{id:?}: {stderr}");
        assert_eq!(
            stderr,
            "error: A conversation id can't be empty; pass an id or a unique id prefix.\n"
        );
        let (code, _, stderr) = run(&env, &["delete", id, "-y"]);
        assert_eq!(code, Some(2), "{id:?}: {stderr}");
    }
    let calls = env.fake().calls();
    assert!(
        !calls
            .iter()
            .any(|call| call.starts_with("PATCH") || call.starts_with("DELETE")),
        "{calls:?}"
    );
}

#[test]
fn the_project_prompt_reads_the_terminal_after_ids_on_stdin() {
    let env = synced();
    let (code, shown) = in_terminal(
        &env,
        &["project", "add", "Writing", "-"],
        Some("a-loose"),
        "Move 1 chat(s) to \"Writing\"? [y/N] ",
        "y",
    );
    assert_eq!(code, Some(0), "{shown}");
    assert!(shown.contains("Moved 1 chat(s) to \"Writing\""), "{shown}");
}

#[test]
fn memories_list_in_every_format() {
    let env = synced();
    assert_eq!(
        env.stdout(&["memory", "list"]),
        format!(
            "ID                                    UPDATED      CONTENT\nmem-aaa  2026-09-01  Prefers tea, not \"coffee\"\nmem-bbb  2026-09-02  {}…\n",
            "x".repeat(119)
        )
    );
    assert_eq!(
        env.stdout(&["memory", "list", "--format", "ids"]),
        "mem-aaa\nmem-bbb\n"
    );
    assert_eq!(
        env.stdout(&["memory", "list", "--format", "csv", "--search", "TEA"]),
        "id,content,updated_at,status,conversation_id\nmem-aaa,\"Prefers  tea,\n not \"\"coffee\"\"\",2026-09-01T00:00:00Z,active,\n"
    );
    let raw: Value =
        serde_json::from_str(&env.stdout(&["memory", "list", "--format", "json", "--limit", "1"]))
            .unwrap();
    assert_eq!(raw[0]["created_timestamp"], 1.5, "as ChatGPT sent it");
    assert_eq!(
        env.stdout(&["memory", "list", "--format", "ids", "--search", "zzz"]),
        ""
    );
    let (_, _, stderr) = run(&env, &["memory", "list", "--format", "xml"]);
    assert_eq!(
        stderr,
        "error: --format must be json, csv, table, or ids.\n"
    );
}

#[test]
fn the_memory_summary_prints_its_sections() {
    let env = synced();
    assert_eq!(
        env.stdout(&["memory", "summary"]),
        "No memory summary available.\n"
    );
    env.fake().state().memory_summary = json!({
        "sections": [{ "id": "1", "title": "Work", "description": "Builds CLIs" },
                     { "id": "2", "title": "Home", "description": "Two kids" }],
        "generatedAtIso": "x", "emptyStateMessage": null, "sourceChecksum": "c",
    });
    assert_eq!(
        env.stdout(&["memory", "summary"]),
        "Work\nBuilds CLIs\n\nHome\nTwo kids\n"
    );
    let json: Value =
        serde_json::from_str(&env.stdout(&["memory", "summary", "--format", "json"])).unwrap();
    assert_eq!(json["sourceChecksum"], "c");
}

#[test]
fn memories_delete_after_a_preview_and_must_be_confirmed() {
    let env = synced();
    let (code, _, stderr) = run(&env, &["memory", "delete", "mem-a", "MEM-AAA", "-n"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(
        stderr,
        "mem-aaa  Prefers tea, not \"coffee\"\ndry run: would delete 1 saved memory.\n"
    );
    let (_, _, stderr) = run(&env, &["memory", "delete", "mem", "-n"]);
    assert_eq!(
        stderr,
        "error: \"mem\" matches 2 saved memories; use a longer id prefix.\n"
    );
    let (code, shown) = in_terminal(
        &env,
        &["memory", "delete", "mem-aaa"],
        None,
        "Permanently delete 1 saved memory? Type 1 to confirm: ",
        "1",
    );
    assert_eq!(code, Some(0), "{shown}");
    assert!(shown.ends_with("Deleted 1 saved memory.\n"), "{shown}");
    assert_eq!(
        env.stdout(&["memory", "list", "--format", "ids"]),
        "mem-bbb\n"
    );

    env.fake()
        .state()
        .unconfirmed_memories
        .insert("mem-bbb".into());
    let (code, _, stderr) = run(&env, &["memory", "delete", "mem-bbb", "-y"]);
    assert_eq!(code, Some(1));
    assert_eq!(
        stderr,
        "mem-bbb  ".to_owned()
            + &"x".repeat(119)
            + "…\nDeleted 0 saved memories, 1 failed.\nfailed: mem-bbb: ChatGPT did not confirm deleting memory mem-bbb.\n"
    );
    let (_, _, stderr) = run(&env, &["memory", "delete", "-", "-y"]);
    assert_eq!(
        stderr,
        "error: Pass saved memory ids, or `-` to read ids from stdin.\n"
    );
}

#[test]
fn projects_and_memories_are_refused_for_another_accounts_session() {
    // The daemon reads test cookies from its own environment.
    let mut env = Env::with_fake(vec![Chat::new(
        "a-loose",
        "Loose chat",
        "2026-09-27T10:00:00.000000Z",
    )]);
    env.extra_env.push((
        "CHATGPT_TEST_COOKIE_CHROME".into(),
        fake_chatgpt::OTHER_COOKIE.into(),
    ));
    env.fake().state().memories = Some(vec![
        json!({ "id": "mem-aaa", "content": "x", "updated_at": "2026-09-01" }),
    ]);
    env.cmd().arg("sync").assert().success();
    for args in [
        &["--browser", "chrome", "project", "create", "New"][..],
        &["--browser", "chrome", "project", "list"][..],
        &["--browser", "chrome", "memory", "delete", "mem-aaa", "-y"][..],
        &["--browser", "chrome", "memory", "summary"][..],
    ] {
        let (code, _, stderr) = run(&env, args);
        assert_ne!(code, Some(0), "{args:?}");
        assert!(
            stderr.contains("this index holds another ChatGPT account's chats"),
            "{args:?}: {stderr}"
        );
    }
    let calls = env.fake().calls();
    assert!(
        calls.iter().all(|call| !call.contains("/projects")
            && !call.contains("/memories")
            && !call.contains("sidebar")),
        "{calls:?}"
    );
}

#[test]
fn a_move_whose_500_cant_be_checked_may_have_applied() {
    let env = synced();
    {
        let mut state = env.fake().state();
        state.project_500.insert("a-loose".into());
        state.fail_detail = 1;
    }
    let (code, _, stderr) = run(&env, &["project", "add", "Writing", "a-loose", "-y"]);
    assert_eq!(code, Some(1));
    assert!(
        stderr.contains("failed: a-loose Loose chat: ChatGPT answered 500 and the check failed for /backend-api/conversation/a-loose, so it may have applied; run `chatgpt sync` and check before trying again\n"),
        "{stderr}"
    );
}
