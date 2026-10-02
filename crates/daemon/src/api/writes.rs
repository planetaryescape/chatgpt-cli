//! The chatgpt.com endpoints that change chats, projects and saved
//! memories, and the project and memory reads that go with them. Ported
//! from the TS CLI's `src/api/conversations.ts`, `projects.ts` and
//! `memories.ts` @ 1b8c950, with their messages and quirks:
//!
//! - a rename of a pre-2025 chat answers 500 yet applies (the caller says so);
//! - a project move that answers 500 may have applied: it's checked with a
//!   GET of the chat;
//! - a delete answering 404 has reached its goal (the caller decides);
//! - a memory delete counts only when ChatGPT answers `success: true`, and
//!   a project delete only when it answers `deleted: true`.
//!
//! Archiving is idempotent and retried like a read. A delete, a rename, a
//! project move, a new or deleted project, and a memory delete must not happen
//! twice: they're retried only after an answer that shows ChatGPT turned
//! them away (a Cloudflare challenge, a 429), and a dropped connection or a
//! gateway error comes back as "may have applied".

use chatgpt::http::{HttpError, HttpMethod, Resend};
use chatgpt_core::ErrorKind;
use chatgpt_protocol::Project;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Api, ApiError, SendError, decode_error};

const RENAME_500: &str = "ChatGPT returned a server error. On older chats the rename usually applies anyway; run `chatgpt sync` and `chatgpt list --title` to check.";

/// `/backend-api/conversation/{id}`.
fn conversation_path(id: &str) -> String {
    format!("/backend-api/conversation/{id}")
}

/// A write that mustn't happen twice failed in a way that leaves it unknown
/// whether ChatGPT applied it.
fn unknown_outcome(path: &str, why: &str) -> ApiError {
    ApiError::new(
        ErrorKind::Network,
        format!(
            "{why} {path}, so it may have applied; run `chatgpt sync` and check before trying again"
        ),
    )
}

/// The error of a write sent with `resend`, worded for its outcome.
fn write_error(error: SendError, path: &str, resend: Resend) -> ApiError {
    if resend == Resend::OnlyIfRefused {
        match &error {
            SendError::Http(HttpError::Transport { .. }) => {
                return unknown_outcome(path, "the connection failed before ChatGPT answered");
            }
            SendError::Http(HttpError::Status { status, .. })
                if matches!(status.as_u16(), 502..=504) =>
            {
                let mut failure =
                    unknown_outcome(path, &format!("ChatGPT's gateway answered {status} for"));
                failure.status = Some(status.as_u16());
                return failure;
            }
            _ => {}
        }
    }
    error.into()
}

impl Api {
    /// Send a write, with `body` as JSON. The answer's text, or the error
    /// worded for the write's outcome.
    async fn write(
        &self,
        method: HttpMethod,
        path: &str,
        body: Option<&Value>,
        resend: Resend,
    ) -> Result<String, ApiError> {
        let body = body.map(Value::to_string);
        self.send_raw(method, path, body.as_deref(), resend)
            .await
            .map_err(|error| write_error(error, path, resend))
    }

    /// `setArchived`. Idempotent.
    pub async fn set_archived(&self, id: &str, archived: bool) -> Result<(), ApiError> {
        let body = json!({ "is_archived": archived });
        self.write(
            HttpMethod::Patch,
            &conversation_path(id),
            Some(&body),
            Resend::Allowed,
        )
        .await
        .map(drop)
    }

    /// `deleteConversation`.
    pub async fn delete_conversation(&self, id: &str) -> Result<(), ApiError> {
        let path = format!("/backend-api/conversation/id/{id}");
        self.write(HttpMethod::Delete, &path, None, Resend::OnlyIfRefused)
            .await
            .map(drop)
    }

    /// `renameConversation`.
    pub async fn rename(&self, id: &str, title: &str) -> Result<(), ApiError> {
        let path = format!("/backend-api/conversation/id/{id}/rename");
        let body = json!({ "title": title });
        match self
            .write(HttpMethod::Post, &path, Some(&body), Resend::OnlyIfRefused)
            .await
        {
            Ok(_) => Ok(()),
            // Observed 2026-09-27: legacy (pre-2025) chats answer 500 yet
            // the sidebar title changes, and only the full list reads it back.
            Err(error) if error.status == Some(500) => Err(ApiError {
                status: Some(500),
                ..ApiError::new(ErrorKind::Api, RENAME_500)
            }),
            Err(error) => Err(error),
        }
    }

    /// `setConversationProject`: `project_id` is `""` to take the chat out
    /// of its project.
    pub async fn set_project(&self, id: &str, project_id: &str) -> Result<(), ApiError> {
        #[derive(Deserialize)]
        struct Moved {
            success: Option<Value>,
        }
        #[derive(Deserialize)]
        struct Chat {
            gizmo_id: Option<String>,
        }
        let path = conversation_path(id);
        let body = json!({ "gizmo_id": project_id });
        let text = match self
            .write(HttpMethod::Patch, &path, Some(&body), Resend::OnlyIfRefused)
            .await
        {
            Ok(text) => text,
            // Older chats can answer 500 after the move has applied.
            Err(error) if error.status == Some(500) => {
                // Unconfirmed either way if the check itself fails.
                let chat: Chat = self.json(HttpMethod::Get, &path, None).await.map_err(|_| {
                    unknown_outcome(&path, "ChatGPT answered 500 and the check failed for")
                })?;
                if chat.gizmo_id.unwrap_or_default() == project_id {
                    return Ok(());
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        let moved: Option<Moved> = parse_optional(&text, &path)?;
        if moved.and_then(|moved| moved.success) == Some(Value::Bool(true)) {
            Ok(())
        } else {
            Err(ApiError::new(
                ErrorKind::Api,
                format!("ChatGPT did not confirm changing the project of {id}."),
            ))
        }
    }

    /// `listProjects`: every project the account can see, archived ones
    /// left out, each once.
    pub async fn projects(&self) -> Result<Vec<Project>, ApiError> {
        let mut projects = Vec::new();
        let mut seen_cursors = std::collections::HashSet::new();
        let mut seen_projects = std::collections::HashSet::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut path = "/backend-api/gizmos/snorlax/sidebar?conversations_per_gizmo=0&limit=20&owned_only=false".to_owned();
            if let Some(cursor) = &cursor {
                path.push_str("&cursor=");
                path.push_str(&form_encode(cursor));
            }
            let page: Value = self.json(HttpMethod::Get, &path, None).await?;
            let invalid = |message: &str| ApiError::new(ErrorKind::Decode, message);
            let items = page
                .get("items")
                .and_then(Value::as_array)
                .ok_or_else(|| invalid("ChatGPT returned an invalid project list."))?;
            for item in items {
                let gizmo = item.pointer("/gizmo/gizmo");
                let id = gizmo
                    .and_then(|gizmo| gizmo.get("id"))
                    .and_then(Value::as_str)
                    .filter(|id| id.starts_with("g-p-"));
                let name = gizmo
                    .and_then(|gizmo| gizmo.pointer("/display/name"))
                    .and_then(Value::as_str);
                let (Some(id), Some(name), Some(gizmo)) = (id, name, gizmo) else {
                    return Err(invalid("ChatGPT returned a project with no id or name."));
                };
                let archived = gizmo.get("is_archived").is_some_and(crate::js::truthy);
                if !archived && !seen_projects.contains(id) {
                    projects.push(Project {
                        id: id.to_owned(),
                        name: name.to_owned(),
                        can_write: can_write(gizmo),
                    });
                }
                seen_projects.insert(id.to_owned());
            }
            let next = page
                .get("cursor")
                .filter(|cursor| crate::js::truthy(cursor))
                .map(|cursor| crate::js::template(Some(cursor)));
            let Some(next) = next else {
                return Ok(projects);
            };
            if !seen_cursors.insert(next.clone()) {
                return Err(invalid("ChatGPT repeated a project list cursor."));
            }
            cursor = Some(next);
        }
    }

    /// `createProject`, with ChatGPT's default settings. The name is
    /// trimmed, and refused when empty or already a project's (ignoring case).
    pub async fn create_project(&self, input: &str) -> Result<Project, ApiError> {
        let name = crate::js::trim(input);
        if name.is_empty() {
            return Err(ApiError::new(
                ErrorKind::InvalidInput,
                "Project name cannot be empty.",
            ));
        }
        let lower = name.to_lowercase();
        if let Some(existing) = self
            .projects()
            .await?
            .into_iter()
            .find(|project| project.name.to_lowercase() == lower)
        {
            return Err(ApiError::new(
                ErrorKind::InvalidInput,
                format!(
                    "Project \"{}\" already exists ({}).",
                    existing.name, existing.id
                ),
            ));
        }
        let path = "/backend-api/projects";
        let body = json!({
            "emoji": null, "instructions": "", "memory_scope": "unset", "name": name, "theme": null,
        });
        let text = self
            .write(HttpMethod::Post, path, Some(&body), Resend::OnlyIfRefused)
            .await?;
        let created: Option<Value> = parse_optional(&text, path)?;
        let gizmo = created
            .as_ref()
            .and_then(|value| value.pointer("/resource/gizmo"));
        let id = gizmo
            .and_then(|gizmo| gizmo.get("id"))
            .and_then(Value::as_str)
            .filter(|id| id.starts_with("g-p-"));
        let name = gizmo
            .and_then(|gizmo| gizmo.pointer("/display/name"))
            .and_then(Value::as_str);
        match (id, name, gizmo) {
            (Some(id), Some(name), Some(gizmo)) if can_write(gizmo) => Ok(Project {
                id: id.to_owned(),
                name: name.to_owned(),
                can_write: true,
            }),
            _ => Err(ApiError::new(
                ErrorKind::Decode,
                "ChatGPT returned an invalid created project.",
            )),
        }
    }

    /// Delete project `id` (`DELETE /backend-api/gizmos/{id}`, observed
    /// 2026-10-02). Done only when ChatGPT answers `deleted: true`; a 404
    /// means there's no such project to delete.
    pub async fn delete_project(&self, id: &str) -> Result<(), ApiError> {
        let path = format!("/backend-api/gizmos/{}", encode_uri_component(id));
        let text = match self
            .write(HttpMethod::Delete, &path, None, Resend::OnlyIfRefused)
            .await
        {
            Err(error) if error.status == Some(404) => {
                return Err(ApiError::new(
                    ErrorKind::InvalidInput,
                    format!("ChatGPT has no project {id}; run `chatgpt project list`."),
                ));
            }
            answer => answer?,
        };
        let answer: Option<Value> = parse_optional(&text, &path)?;
        if answer.as_ref().and_then(|value| value.get("deleted")) == Some(&Value::Bool(true)) {
            Ok(())
        } else {
            Err(ApiError::new(
                ErrorKind::Api,
                format!(
                    "ChatGPT didn't confirm deleting project {id}; run `chatgpt project list` to check."
                ),
            ))
        }
    }

    /// `listMemories`, each memory as ChatGPT sent it.
    pub async fn memory_objects(&self) -> Result<Vec<Value>, ApiError> {
        let path = "/backend-api/memories?include_memory_entries=true";
        let answer: Value = self.json(HttpMethod::Get, path, None).await?;
        match answer.get("memories") {
            Some(Value::Array(memories)) => Ok(memories.clone()),
            _ => Err(ApiError::new(
                ErrorKind::Decode,
                "ChatGPT returned an unexpected saved-memory list.",
            )),
        }
    }

    /// `getMemorySummary`: a read, although it's a POST.
    pub async fn memory_summary(&self) -> Result<Value, ApiError> {
        let path = "/backend-api/memories/about_you/summary?source=personalization-setting";
        let summary: Value = self.json(HttpMethod::Post, path, Some("{}")).await?;
        if summary.get("sections").is_some_and(Value::is_array) {
            Ok(summary)
        } else {
            Err(ApiError::new(
                ErrorKind::Decode,
                "ChatGPT returned an unexpected memory summary.",
            ))
        }
    }

    /// `deleteMemory`: done only when ChatGPT says `success: true`.
    pub async fn delete_memory(&self, id: &str) -> Result<(), ApiError> {
        let path = format!("/backend-api/memories/{}", encode_uri_component(id));
        let text = self
            .write(HttpMethod::Delete, &path, None, Resend::OnlyIfRefused)
            .await?;
        let answer: Option<Value> = parse_optional(&text, &path)?;
        if answer.as_ref().and_then(|value| value.get("success")) == Some(&Value::Bool(true)) {
            Ok(())
        } else {
            Err(ApiError::new(
                ErrorKind::Api,
                format!("ChatGPT did not confirm deleting memory {id}."),
            ))
        }
    }
}

/// `current_user_permission.can_write === true`.
fn can_write(gizmo: &Value) -> bool {
    gizmo.pointer("/current_user_permission/can_write") == Some(&Value::Bool(true))
}

/// `text ? JSON.parse(text) : null`, as the TS client reads an answer.
fn parse_optional<T: serde::de::DeserializeOwned>(
    text: &str,
    path: &str,
) -> Result<Option<T>, ApiError> {
    if text.is_empty() {
        return Ok(None);
    }
    serde_json::from_str::<Option<T>>(text).map_err(|error| decode_error(path, &error))
}

/// `URLSearchParams`'s value encoding (application/x-www-form-urlencoded).
fn form_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(char::from(byte));
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `encodeURIComponent`.
fn encode_uri_component(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(char::from(byte)),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_encoded_as_url_search_params_and_encode_uri_component_do() {
        assert_eq!(form_encode("a b+c/=~é*"), "a+b%2Bc%2F%3D%7E%C3%A9*");
        assert_eq!(encode_uri_component("a b/~'()é"), "a%20b%2F~'()%C3%A9");
    }
}
