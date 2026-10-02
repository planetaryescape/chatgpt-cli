//! `classify`, `titles` and `memory classify`, as the TS CLI's (`src/cli.ts`
//! and `src/commands/memories.ts` @ 1b8c950) run them: the daemon resolves
//! the chats (as for `archive`), then classifies them, its notes, steps and
//! cost lines arriving as progress. A question it asks mid-run (before a
//! large batch of summaries) is asked here, at the terminal.
//!
//! The model keys in this command's environment, and its `PATH` (where
//! `codex` and `claude` are found), go to the daemon with the request:
//! whatever environment the daemon itself started with doesn't count.

use std::cell::RefCell;
use std::process::ExitCode;

use chatgpt_core::Paths;
use chatgpt_launcher::ClientError;
use chatgpt_protocol::{ModelAccess, Request, ResponseData, Secret, Selection, SessionChoice};
use serde_json::Value;

use crate::args::{ClassifyArgs, TitlesArgs};
use crate::change_cmd::{given_ids, select};
use crate::memory_cmd::{csv_cell, field, preview_content, write_data};
use crate::output::{ProgressLines, json, unexpected};
use crate::project_cmd::positive_limit;
use crate::prompt;
use crate::reads::{filter, invalid};

/// The keys and `PATH` this command lends the daemon.
pub fn model_access() -> ModelAccess {
    let env = |name: &str| std::env::var(name).ok().map(Secret::new);
    ModelAccess {
        typesafe: env("TYPESAFE_API_KEY"),
        openai: env("OPENAI_API_KEY"),
        anthropic: env("ANTHROPIC_API_KEY"),
        path: std::env::var("PATH").ok(),
    }
}

/// Send `request`, drawing progress, and ask the user what the daemon
/// asks.
pub async fn ask_showing_progress_and_asking(
    paths: &Paths,
    request: Request,
) -> Result<ResponseData, ClientError> {
    let progress = RefCell::new(ProgressLines::new());
    let answer = chatgpt_launcher::ask_asking(
        paths,
        request,
        |event| progress.borrow_mut().show(event),
        |question| {
            progress.borrow_mut().close();
            prompt::confirm(question)
        },
    )
    .await;
    progress.borrow_mut().close();
    answer
}

/// The chats a classify-like command acts on: ids, else the filters, else
/// every chat in scope.
async fn targets(
    paths: &Paths,
    ids: Vec<String>,
    filters: crate::args::FilterArgs,
    pinned: bool,
) -> Result<Vec<String>, ClientError> {
    let selection = Selection {
        ids: given_ids(ids)?,
        filter: filter(filters),
        pinned,
        exclude_unsure: false,
        allow_unfiltered: true,
    };
    Ok(select(paths, selection)
        .await?
        .into_iter()
        .map(|row| row.id)
        .collect())
}

fn exit(failed: bool) -> ExitCode {
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

pub async fn classify(
    paths: &Paths,
    args: ClassifyArgs,
    session: SessionChoice,
) -> Result<ExitCode, ClientError> {
    let ids = targets(paths, args.ids, args.filters, args.pinned).await?;
    let request = Request::Classify {
        ids,
        redo: args.redo,
        yes: args.yes,
        access: model_access(),
        session,
    };
    let ResponseData::Classified(outcome) = ask_showing_progress_and_asking(paths, request).await?
    else {
        return Err(unexpected());
    };
    Ok(exit(outcome.failed))
}

pub async fn titles(paths: &Paths, args: TitlesArgs) -> Result<ExitCode, ClientError> {
    let ids = targets(paths, args.ids, args.filters, args.pinned).await?;
    let request = Request::Titles {
        ids,
        redo: args.redo,
        access: model_access(),
    };
    let ResponseData::Classified(outcome) = ask_showing_progress_and_asking(paths, request).await?
    else {
        return Err(unexpected());
    };
    Ok(exit(outcome.failed))
}

pub async fn memory_classify(
    paths: &Paths,
    suggest: Option<String>,
    limit: Option<String>,
    format: String,
    redo: bool,
    session: SessionChoice,
) -> Result<ExitCode, ClientError> {
    if !["json", "csv", "table", "ids"].contains(&format.as_str()) {
        return Err(invalid("--format must be json, csv, table, or ids."));
    }
    let suggest = suggest.filter(|suggest| !suggest.is_empty());
    if suggest
        .as_deref()
        .is_some_and(|suggest| !["keep", "delete", "review"].contains(&suggest))
    {
        return Err(invalid("--suggest must be keep, delete, or review."));
    }
    // Checked before any paid call; the TS CLI checks it after.
    let limit = positive_limit(limit.as_deref())?;
    let request = Request::MemoryClassify {
        redo,
        access: model_access(),
        session,
    };
    let ResponseData::ClassifiedMemories(result) =
        ask_showing_progress_and_asking(paths, request).await?
    else {
        return Err(unexpected());
    };
    let mut rows: Vec<Value> = match &suggest {
        Some(suggest) => result
            .rows
            .into_iter()
            .filter(|row| field(row, "suggestion") == *suggest)
            .collect(),
        None => result.rows,
    };
    if let Some(limit) = limit {
        rows.truncate(limit);
    }
    write_classified(&rows, &format)?;
    for failure in &result.failures {
        eprintln!("failed: {failure}");
    }
    Ok(exit(!result.failures.is_empty()))
}

/// `formatClassifiedMemories`.
fn write_classified(rows: &[Value], format: &str) -> Result<(), ClientError> {
    if format == "json" {
        return json(&rows);
    }
    let related = |row: &Value| {
        row.get("related_ids")
            .and_then(Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default()
    };
    let lines: Vec<String> = match format {
        "ids" => rows.iter().map(|row| field(row, "id")).collect(),
        "csv" => {
            std::iter::once("id,suggestion,stage,reason,content,updated_at,related_ids".to_owned())
                .chain(rows.iter().map(|row| {
                    [
                        field(row, "id"),
                        field(row, "suggestion"),
                        field(row, "stage"),
                        field(row, "reason"),
                        field(row, "content"),
                        field(row, "updated_at"),
                        related(row),
                    ]
                    .iter()
                    .map(|cell| csv_cell(cell))
                    .collect::<Vec<_>>()
                    .join(",")
                }))
                .collect()
        }
        _ => std::iter::once(
            "SUGGEST  STAGE  ID                                    REASON".to_owned(),
        )
        .chain(rows.iter().map(|row| {
            format!(
                "{:<7}  {:<5}  {}  {}  [{}]",
                field(row, "suggestion"),
                field(row, "stage"),
                field(row, "id"),
                field(row, "reason"),
                preview_content(&field(row, "content"))
            )
        }))
        .collect(),
    };
    write_data(&lines.join("\n"));
    Ok(())
}
