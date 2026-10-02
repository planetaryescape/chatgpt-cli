//! The fake's write endpoints: archive and project moves (`PATCH
//! /backend-api/conversation/{id}`), deletes, renames, the project list,
//! new projects and project deletes, the memory summary and memory deletes, with the
//! quirks chatgpt.com has (observed 2026-09-27/28): a legacy chat's rename
//! answers 500 yet applies, a project move can answer 500 after applying,
//! and a memory delete is done only when it answers `success: true`.

use serde_json::{Value, json};
use wiremock::{Request, ResponseTemplate};

use crate::State;

/// A project the sidebar lists.
#[derive(Clone, Debug)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub can_write: bool,
    pub archived: bool,
}

impl Project {
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            can_write: true,
            archived: false,
        }
    }
}

/// How the next write is answered instead of carried out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteFailure {
    /// This status, and the write not applied.
    Status(u16),
    /// The write applied, then the answer broke off mid-body: to the
    /// client, a dropped connection.
    Truncated,
}

#[derive(Clone, Copy, Debug)]
pub enum Write {
    /// `PATCH /backend-api/conversation/{id}`: `is_archived` or `gizmo_id`.
    Patch,
    Delete,
    Rename,
    ProjectList,
    CreateProject,
    /// `DELETE /backend-api/gizmos/{id}`.
    DeleteProject,
    MemorySummary,
    DeleteMemory,
}

/// The project list's page size, as the CLI asks for it.
const PROJECT_PAGE: usize = 20;

fn body(request: &Request) -> Value {
    serde_json::from_slice(&request.body).unwrap_or(Value::Null)
}

fn segment(request: &Request, from_end: usize) -> String {
    request
        .url
        .path_segments()
        .and_then(|segments| segments.rev().nth(from_end).map(str::to_owned))
        .unwrap_or_default()
}

fn ok() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "success": true }))
}

fn not_found() -> ResponseTemplate {
    ResponseTemplate::new(404).set_body_json(json!({ "detail": "secret body: not found" }))
}

/// An answer that claims more body than it sends.
fn truncated() -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-length", "1000")
        .set_body_raw("{\"success\":", "application/json")
}

pub fn respond(state: &mut State, write: Write, request: &Request) -> ResponseTemplate {
    let failure = match write {
        Write::ProjectList | Write::MemorySummary => None,
        _ => state.fail_writes.pop_front(),
    };
    if let Some(WriteFailure::Status(status)) = failure {
        return ResponseTemplate::new(status).set_body_string("{\"detail\":\"secret body\"}");
    }
    let answer = apply(state, write, request);
    match failure {
        Some(WriteFailure::Truncated) => truncated(),
        _ => answer,
    }
}

fn apply(state: &mut State, write: Write, request: &Request) -> ResponseTemplate {
    match write {
        Write::Patch => {
            let id = segment(request, 0);
            let body = body(request);
            let Some(chat) = state.chats.iter_mut().find(|chat| chat.id == id) else {
                return not_found();
            };
            if let Some(archived) = body.get("is_archived").and_then(Value::as_bool) {
                chat.archived = archived;
                return ok();
            }
            if let Some(project) = body.get("gizmo_id").and_then(Value::as_str) {
                chat.gizmo_id = (!project.is_empty()).then(|| project.to_owned());
                if state.project_500.contains(&id) {
                    return ResponseTemplate::new(500)
                        .set_body_string("{\"detail\":\"secret body\"}");
                }
                if state.unconfirmed_moves.contains(&id) {
                    return ResponseTemplate::new(200).set_body_json(json!({ "success": false }));
                }
                return ok();
            }
            ResponseTemplate::new(400)
        }
        Write::Delete => {
            let id = segment(request, 0);
            let before = state.chats.len();
            state.chats.retain(|chat| chat.id != id);
            if state.chats.len() == before {
                return not_found();
            }
            ok()
        }
        Write::Rename => {
            let id = segment(request, 1);
            let title = body(request)["title"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let legacy = state.legacy_rename.contains(&id);
            let Some(chat) = state.chats.iter_mut().find(|chat| chat.id == id) else {
                return not_found();
            };
            chat.title = title;
            if legacy {
                return ResponseTemplate::new(500).set_body_string("{\"detail\":\"secret body\"}");
            }
            ResponseTemplate::new(200).set_body_json(json!(true))
        }
        Write::ProjectList => {
            let start: usize = request
                .url
                .query_pairs()
                .find(|(key, _)| key == "cursor")
                .and_then(|(_, cursor)| cursor.strip_prefix("page ").and_then(|n| n.parse().ok()))
                .unwrap_or(0);
            let items: Vec<Value> = state
                .projects
                .iter()
                .skip(start)
                .take(PROJECT_PAGE)
                .map(|project| {
                    json!({ "gizmo": { "gizmo": {
                        "id": project.id, "display": { "name": project.name },
                        "current_user_permission": { "can_write": project.can_write },
                        "is_archived": project.archived,
                    } } })
                })
                .collect();
            let next = start + PROJECT_PAGE;
            // A cursor with a space and a plus, which must arrive encoded.
            let cursor = (next < state.projects.len()).then(|| format!("page {next}"));
            ResponseTemplate::new(200).set_body_json(json!({ "items": items, "cursor": cursor }))
        }
        Write::CreateProject => {
            let name = body(request)["name"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            state.created_projects += 1;
            let id = format!("g-p-new{}", state.created_projects);
            state.projects.push(Project::new(&id, &name));
            ResponseTemplate::new(200).set_body_json(json!({ "resource": { "gizmo": {
                "id": id, "display": { "name": name },
                "current_user_permission": { "can_write": true },
            } } }))
        }
        Write::DeleteProject => {
            let id = segment(request, 0);
            let before = state.projects.len();
            state.projects.retain(|project| project.id != id);
            if state.projects.len() == before {
                return not_found();
            }
            state.deleted_projects.push(id);
            ResponseTemplate::new(200).set_body_json(json!({ "deleted": true }))
        }
        Write::MemorySummary => {
            ResponseTemplate::new(200).set_body_json(state.memory_summary.clone())
        }
        Write::DeleteMemory => {
            let id = segment(request, 0);
            let Some(memories) = state.memories.as_mut() else {
                return ResponseTemplate::new(500);
            };
            let before = memories.len();
            memories.retain(|memory| memory["id"] != id.as_str());
            if memories.len() == before {
                return not_found();
            }
            if state.unconfirmed_memories.contains(&id) {
                return ResponseTemplate::new(200).set_body_json(json!({ "success": false }));
            }
            ok()
        }
    }
}
