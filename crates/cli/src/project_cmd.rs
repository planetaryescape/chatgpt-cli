//! `project create|list|add|remove`, as the TS CLI's (`src/cli.ts` and
//! `src/commands/projects.ts` @ 1b8c950) run them, and `project delete`,
//! which the TS CLI didn't have.

use std::process::ExitCode;

use chatgpt_core::Paths;
use chatgpt_launcher::ClientError;
use chatgpt_protocol::{Filter, Project, Request, ResponseData, Row, Selection, SessionChoice};

use crate::args::{ProjectCommand, ProjectMoveArgs};
use crate::change_cmd::{ask_showing_progress, finish, given_ids, preview, select, targets};
use crate::output::{data, json, note, unexpected};
use crate::prompt;
use crate::reads::invalid;

pub async fn run(
    paths: &Paths,
    command: ProjectCommand,
    session: SessionChoice,
) -> Result<ExitCode, ClientError> {
    match command {
        ProjectCommand::Create {
            name,
            json: as_json,
        } => {
            let request = Request::CreateProject { name, session };
            let ResponseData::ProjectCreated(project) =
                chatgpt_launcher::ask(paths, request, |_| {}).await?
            else {
                return Err(unexpected());
            };
            if as_json {
                json(&project)?;
            } else {
                data(&format!("{}  {}\n", project.id, project.name));
            }
        }
        ProjectCommand::List {
            json: as_json,
            limit,
        } => {
            let mut projects = projects(paths, session).await?;
            if let Some(limit) = positive_limit(limit.as_deref())? {
                projects.truncate(limit);
            }
            if as_json {
                json(&projects)?;
            } else {
                let text: String = projects
                    .iter()
                    .map(|project| {
                        format!(
                            "{}  {}{}\n",
                            project.id,
                            project.name,
                            if project.can_write {
                                ""
                            } else {
                                " (read-only)"
                            }
                        )
                    })
                    .collect();
                data(&text);
            }
        }
        ProjectCommand::Add(args) => return move_chats(paths, args, false, session).await,
        ProjectCommand::Remove(args) => return move_chats(paths, args, true, session).await,
        ProjectCommand::Delete {
            project,
            dry_run,
            yes,
        } => return delete(paths, &project, dry_run, yes, session).await,
    }
    Ok(ExitCode::SUCCESS)
}

async fn projects(paths: &Paths, session: SessionChoice) -> Result<Vec<Project>, ClientError> {
    let ResponseData::Projects { projects } =
        chatgpt_launcher::ask(paths, Request::Projects { session }, |_| {}).await?
    else {
        return Err(unexpected());
    };
    Ok(projects)
}

/// `positiveLimit`: `--limit` as a positive safe integer; an empty one is
/// no limit.
pub fn positive_limit(raw: Option<&str>) -> Result<Option<usize>, ClientError> {
    let Some(raw) = raw.filter(|raw| !raw.is_empty()) else {
        return Ok(None);
    };
    let limit = crate::search_cmd::limit(raw)?;
    Ok(Some(usize::try_from(limit).unwrap_or(usize::MAX)))
}

/// `resolveProject`: an exact id or name (ignoring case), else a unique id
/// prefix.
fn resolve_project(projects: Vec<Project>, reference: &str) -> Result<Project, ClientError> {
    let input = chatgpt_core::js::trim(reference).to_lowercase();
    let (exact, rest): (Vec<Project>, Vec<Project>) = projects.into_iter().partition(|project| {
        project.id.to_lowercase() == input || project.name.to_lowercase() == input
    });
    let mut matches = if exact.is_empty() {
        rest.into_iter()
            .filter(|project| project.id.to_lowercase().starts_with(&input))
            .collect()
    } else {
        exact
    };
    match matches.len() {
        0 => Err(invalid(format!(
            "No project matching \"{reference}\". Run `chatgpt project list` to see names and ids."
        ))),
        1 => Ok(matches.remove(0)),
        many => Err(invalid(format!(
            "\"{reference}\" matches {many} projects; use a project id."
        ))),
    }
}

/// `project delete`: the project, what it holds in the index, then the
/// user types its name (or passes `-y`) before it's deleted.
async fn delete(
    paths: &Paths,
    reference: &str,
    dry_run: bool,
    yes: bool,
    session: SessionChoice,
) -> Result<ExitCode, ClientError> {
    let (projects, every) = tokio::join!(
        projects(paths, session.clone()),
        chatgpt_launcher::ask(paths, Request::list_every_chat(), |_| {})
    );
    let project = resolve_project(projects?, reference)?;
    if !project.can_write {
        return Err(invalid(format!(
            "You do not have write access to project \"{}\".",
            project.name
        )));
    }
    let ResponseData::Rows(every) = every? else {
        return Err(unexpected());
    };
    let chats: Vec<&Row> = every
        .rows
        .iter()
        .filter(|row| in_project(row, &project))
        .collect();
    let name = &project.name;
    note(&format!("Project \"{name}\" ({})", project.id));
    if chats.is_empty() {
        note("No indexed chats are in it.");
    } else {
        let archived = chats.iter().filter(|row| row.is_archived != 0).count();
        note(&format!(
            "{} indexed chat(s) are in it{}:",
            chats.len(),
            if archived > 0 {
                format!(" ({archived} archived)")
            } else {
                String::new()
            }
        ));
        preview(chats.iter().map(|row| preview_line(row)), chats.len());
        note(
            "ChatGPT may delete these chats along with the project, or only take them out of it. Treat them as deleted: export any you want to keep first.",
        );
    }
    if dry_run {
        note(&format!("dry run: would delete project \"{name}\"."));
        return Ok(ExitCode::SUCCESS);
    }
    if !yes {
        let typed = prompt::ask(&format!(
            "This can't be undone. Type the project name ({name}) to delete it: "
        ))?;
        if typed != *name {
            note("Cancelled: the name didn't match.");
            return Ok(ExitCode::SUCCESS);
        }
    }
    let request = Request::DeleteProject {
        project: project.clone(),
        session,
    };
    let ResponseData::ProjectDeleted { unassigned } =
        chatgpt_launcher::ask(paths, request, |_| {}).await?
    else {
        return Err(unexpected());
    };
    note(&format!("Deleted project \"{name}\" ({}).", project.id));
    if unassigned > 0 {
        note(&format!(
            "Took {unassigned} indexed chat(s) out of it; `chatgpt sync` shows whether ChatGPT kept them."
        ));
    }
    Ok(ExitCode::SUCCESS)
}

/// Whether the index has `row` in `project`.
fn in_project(row: &Row, project: &Project) -> bool {
    row.project_id.as_deref() == Some(project.id.as_str())
}

/// A chat's line in a preview.
fn preview_line(row: &Row) -> String {
    format!("{}  {}", row.id, row.display_title)
}

/// `project add` and `project remove`: the chats, then the project, then
/// `applyProjectAdd` or `applyProjectRemove`.
async fn move_chats(
    paths: &Paths,
    args: ProjectMoveArgs,
    remove: bool,
    session: SessionChoice,
) -> Result<ExitCode, ClientError> {
    let selection = Selection {
        // `<ids...>` is required: never the filters.
        ids: Some(given_ids(args.ids)?.unwrap_or_default()),
        filter: Filter {
            archived: args.archived,
            all: args.all,
            ..Filter::default()
        },
        ..Selection::default()
    };
    let rows = select(paths, selection).await?;
    let project = resolve_project(projects(paths, session.clone()).await?, &args.project)?;
    if !project.can_write {
        return Err(invalid(format!(
            "You do not have write access to project \"{}\".",
            project.name
        )));
    }
    // One per chat, in the order first given.
    let mut unique: Vec<Row> = Vec::new();
    for row in rows {
        if !unique.iter().any(|kept| kept.id == row.id) {
            unique.push(row);
        }
    }
    let in_project = |row: &Row| in_project(row, &project);
    let name = &project.name;
    let pending: Vec<Row> = if remove {
        let pending: Vec<Row> = unique.into_iter().filter(in_project).collect();
        if pending.is_empty() {
            note(&format!("No selected chats are in \"{name}\"."));
            return Ok(ExitCode::SUCCESS);
        }
        preview(pending.iter().map(preview_line), pending.len());
        pending
    } else {
        if unique.is_empty() {
            note("No chats to move.");
            return Ok(ExitCode::SUCCESS);
        }
        let total = unique.len();
        let pending: Vec<Row> = unique.into_iter().filter(|row| !in_project(row)).collect();
        if pending.is_empty() {
            note(&format!(
                "No chats to move; all {total} are already in \"{name}\"."
            ));
            return Ok(ExitCode::SUCCESS);
        }
        preview(
            pending.iter().map(|row| {
                let from = row
                    .project_id
                    .as_deref()
                    .filter(|id| !id.is_empty())
                    .map(|id| format!("  (from {id})"))
                    .unwrap_or_default();
                format!("{}  {}{from}", row.id, row.display_title)
            }),
            pending.len(),
        );
        pending
    };
    let count = pending.len();
    let (verb, preposition) = if remove {
        ("remove", "from")
    } else {
        ("move", "to")
    };
    if args.dry_run {
        note(&format!(
            "dry run: would {verb} {count} chat(s) {preposition} \"{name}\"."
        ));
        return Ok(ExitCode::SUCCESS);
    }
    let question = format!(
        "{}{} {count} chat(s) {preposition} \"{name}\"? [y/N] ",
        verb[..1].to_uppercase(),
        &verb[1..]
    );
    if !args.yes && !prompt::confirm(&question)? {
        note("Cancelled.");
        return Ok(ExitCode::SUCCESS);
    }
    let request = Request::MoveToProject {
        project,
        targets: targets(&pending),
        remove,
        session,
    };
    let ResponseData::Outcome(outcome) = ask_showing_progress(paths, request).await? else {
        return Err(unexpected());
    };
    Ok(finish(&outcome))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn project(id: &str, name: &str) -> Project {
        Project {
            id: id.into(),
            name: name.into(),
            can_write: true,
        }
    }

    #[test]
    fn projects_resolve_by_exact_id_or_name_then_id_prefix() {
        let all = || {
            vec![
                project("g-p-abc", "Writing"),
                project("g-p-abd", "g-p-ab"),
                project("g-p-x", "Easydeck"),
            ]
        };
        assert_eq!(resolve_project(all(), " easydeck ").unwrap().id, "g-p-x");
        assert_eq!(
            resolve_project(all(), "G-P-AB").unwrap().id,
            "g-p-abd",
            "an exact name beats id prefixes"
        );
        assert_eq!(resolve_project(all(), "g-p-abc").unwrap().id, "g-p-abc");
        assert_eq!(
            resolve_project(all(), "g-p-a").unwrap_err().message,
            "\"g-p-a\" matches 2 projects; use a project id."
        );
        assert_eq!(
            resolve_project(all(), "nope").unwrap_err().message,
            "No project matching \"nope\". Run `chatgpt project list` to see names and ids."
        );
    }

    #[test]
    fn limits_must_be_positive_safe_integers() {
        assert_eq!(positive_limit(None).unwrap(), None);
        assert_eq!(positive_limit(Some("")).unwrap(), None);
        assert_eq!(positive_limit(Some("3")).unwrap(), Some(3));
        assert_eq!(positive_limit(Some("1e1")).unwrap(), Some(10));
        for bad in ["0", "-1", "2.5", "x", "9007199254740992"] {
            assert!(positive_limit(Some(bad)).is_err(), "{bad}");
        }
    }
}
