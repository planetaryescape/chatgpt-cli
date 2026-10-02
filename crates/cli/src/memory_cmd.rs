//! `memory list|summary|delete`, as the TS CLI's (`src/cli.ts` and
//! `src/commands/memories.ts` @ 1b8c950) run them: ChatGPT's saved memories
//! read live, printed in its formats. `memory classify` runs in the TS CLI.

use std::process::ExitCode;

use chatgpt_core::js::{collapse_spaces, trim};
use chatgpt_core::{ErrorKind, Paths};
use chatgpt_launcher::ClientError;
use chatgpt_protocol::{Request, ResponseData, SessionChoice};
use serde_json::Value;

use crate::args::MemoryCommand;
use crate::change_cmd::{finish, given_ids, preview};
use crate::output::{data, json, note, unexpected};
use crate::project_cmd::positive_limit;
use crate::prompt;

fn invalid(message: impl Into<String>) -> ClientError {
    ClientError::new(ErrorKind::InvalidInput, message)
}

pub async fn run(
    paths: &Paths,
    command: MemoryCommand,
    session: SessionChoice,
) -> Result<ExitCode, ClientError> {
    match command {
        MemoryCommand::List {
            search,
            limit,
            format,
        } => {
            if !["json", "csv", "table", "ids"].contains(&format.as_str()) {
                return Err(invalid("--format must be json, csv, table, or ids."));
            }
            let memories = memories(paths, session).await?;
            let search = search
                .filter(|search| !search.is_empty())
                .map(|search| search.to_lowercase());
            let mut selected: Vec<Value> = match &search {
                Some(search) => memories
                    .into_iter()
                    .filter(|memory| field(memory, "content").to_lowercase().contains(search))
                    .collect(),
                None => memories,
            };
            if let Some(limit) = positive_limit(limit.as_deref())? {
                selected.truncate(limit);
            }
            write_formatted(&selected, &format)?;
            Ok(ExitCode::SUCCESS)
        }
        MemoryCommand::Summary { format } => {
            if format != "json" && format != "table" {
                return Err(invalid("--format must be json or table."));
            }
            let ResponseData::MemorySummary { summary } =
                chatgpt_launcher::ask(paths, Request::MemorySummary { session }, |_| {}).await?
            else {
                return Err(unexpected());
            };
            if format == "json" {
                json(&summary)?;
            } else {
                write_data(&summary_table(&summary));
            }
            Ok(ExitCode::SUCCESS)
        }
        MemoryCommand::Delete { ids, dry_run, yes } => {
            delete(paths, ids, dry_run, yes, session).await
        }
        // Routed to the TS CLI before it gets here.
        MemoryCommand::Classify { .. } => Err(unexpected()),
    }
}

async fn memories(paths: &Paths, session: SessionChoice) -> Result<Vec<Value>, ClientError> {
    let ResponseData::Memories { memories } =
        chatgpt_launcher::ask(paths, Request::Memories { session }, |_| {}).await?
    else {
        return Err(unexpected());
    };
    Ok(memories)
}

/// A memory's field as text: strings as they are, numbers and booleans as
/// JS prints them, nothing for null or a missing field (as `Array.join`
/// writes them).
fn field(memory: &Value, name: &str) -> String {
    match memory.get(name) {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number
            .as_f64()
            .map_or_else(|| number.to_string(), chatgpt_core::js_number_string),
        Some(other) => other.to_string(),
    }
}

/// `writeData`: the text and a newline, or nothing for no text.
fn write_data(text: &str) {
    if !text.is_empty() {
        data(&format!("{text}\n"));
    }
}

fn csv_cell(value: &str) -> String {
    if value.contains(['"', ',', '\r', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

/// `preview`: whitespace collapsed, trimmed, at most 120 UTF-16 units.
fn preview_content(content: &str) -> String {
    let single = collapse_spaces(content);
    let single = trim(&single);
    let units: Vec<u16> = single.encode_utf16().collect();
    if units.len() > 120 {
        format!("{}…", String::from_utf16_lossy(&units[..119]))
    } else {
        single.to_owned()
    }
}

/// `formatMemories`.
fn write_formatted(memories: &[Value], format: &str) -> Result<(), ClientError> {
    if format == "json" {
        return json(&memories);
    }
    let lines: Vec<String> = match format {
        "ids" => memories.iter().map(|memory| field(memory, "id")).collect(),
        "csv" => std::iter::once("id,content,updated_at,status,conversation_id".to_owned())
            .chain(memories.iter().map(|memory| {
                ["id", "content", "updated_at", "status", "conversation_id"]
                    .map(|name| csv_cell(&field(memory, name)))
                    .join(",")
            }))
            .collect(),
        _ => {
            std::iter::once("ID                                    UPDATED      CONTENT".to_owned())
                .chain(memories.iter().map(|memory| {
                    let updated: String = field(memory, "updated_at").chars().take(10).collect();
                    format!(
                        "{}  {updated}  {}",
                        field(memory, "id"),
                        preview_content(&field(memory, "content"))
                    )
                }))
                .collect()
        }
    };
    write_data(&lines.join("\n"));
    Ok(())
}

/// `formatMemorySummary` as a table: each section's title and description.
fn summary_table(summary: &Value) -> String {
    let sections = summary
        .get("sections")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if sections.is_empty() {
        return match summary.get("emptyStateMessage") {
            None | Some(Value::Null) => "No memory summary available.".to_owned(),
            Some(_) => field(summary, "emptyStateMessage"),
        };
    }
    sections
        .iter()
        .map(|section| {
            let text = |name: &str| match section.get(name) {
                None => "undefined".to_owned(),
                Some(Value::Null) => "null".to_owned(),
                Some(_) => field(section, name),
            };
            format!("{}\n{}", text("title"), text("description"))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// `resolveMemories`: each id a unique prefix (ignoring case), each memory
/// once, in the order first named.
fn resolve<'a>(memories: &'a [Value], ids: &[String]) -> Result<Vec<&'a Value>, ClientError> {
    let mut selected: Vec<&Value> = Vec::new();
    for raw in ids {
        let id = trim(raw).to_lowercase();
        if id.is_empty() {
            return Err(invalid("Saved memory id cannot be empty."));
        }
        let matches: Vec<&Value> = memories
            .iter()
            .filter(|memory| field(memory, "id").to_lowercase().starts_with(&id))
            .collect();
        match matches.as_slice() {
            [] => {
                return Err(invalid(format!(
                    "No saved memory matching \"{raw}\". Run `chatgpt memory list` to see current ids."
                )));
            }
            [memory] => {
                if !selected
                    .iter()
                    .any(|kept| field(kept, "id") == field(memory, "id"))
                {
                    selected.push(memory);
                }
            }
            many => {
                return Err(invalid(format!(
                    "\"{raw}\" matches {} saved memories; use a longer id prefix.",
                    many.len()
                )));
            }
        }
    }
    Ok(selected)
}

fn memories_word(count: usize) -> &'static str {
    if count == 1 { "memory" } else { "memories" }
}

/// `deleteSelectedMemories`.
async fn delete(
    paths: &Paths,
    ids: Vec<String>,
    dry_run: bool,
    yes: bool,
    session: SessionChoice,
) -> Result<ExitCode, ClientError> {
    let raw = given_ids(ids)?.unwrap_or_default();
    if raw.is_empty() {
        return Err(invalid(
            "Pass saved memory ids, or `-` to read ids from stdin.",
        ));
    }
    let memories = memories(paths, session.clone()).await?;
    let targets = resolve(&memories, &raw)?;
    let count = targets.len();
    preview(
        targets.iter().map(|memory| {
            format!(
                "{}  {}",
                field(memory, "id"),
                preview_content(&field(memory, "content"))
            )
        }),
        count,
    );
    if dry_run {
        note(&format!(
            "dry run: would delete {count} saved {}.",
            memories_word(count)
        ));
        return Ok(ExitCode::SUCCESS);
    }
    if !yes
        && !prompt::confirm_count(
            &format!(
                "Permanently delete {count} saved {}? Type {count} to confirm: ",
                memories_word(count)
            ),
            count,
        )?
    {
        note("Cancelled.");
        return Ok(ExitCode::SUCCESS);
    }
    let request = Request::DeleteMemories {
        ids: targets.iter().map(|memory| field(memory, "id")).collect(),
        session,
    };
    let ResponseData::Outcome(outcome) = chatgpt_launcher::ask(paths, request, |_| {}).await?
    else {
        return Err(unexpected());
    };
    let deleted = usize::try_from(outcome.done).unwrap_or(usize::MAX);
    let failed = if outcome.failures.is_empty() {
        String::new()
    } else {
        format!(", {} failed", outcome.failures.len())
    };
    note(&format!(
        "Deleted {deleted} saved {}{failed}.",
        memories_word(deleted)
    ));
    Ok(finish(&outcome))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn previews_collapse_whitespace_and_cut_at_120_units() {
        assert_eq!(preview_content("  a \n\t b  "), "a b");
        let long = "x".repeat(130);
        assert_eq!(preview_content(&long), format!("{}…", "x".repeat(119)));
        assert_eq!(preview_content(&"x".repeat(120)), "x".repeat(120));
    }

    #[test]
    fn csv_cells_quote_as_the_ts_cli_does() {
        assert_eq!(csv_cell("plain"), "plain");
        assert_eq!(csv_cell("a, \"b\""), "\"a, \"\"b\"\"\"");
        assert_eq!(field(&json!({ "a": null }), "a"), "");
        assert_eq!(field(&json!({ "a": 1.5 }), "a"), "1.5");
        assert_eq!(field(&json!({}), "a"), "");
    }

    #[test]
    fn ids_resolve_as_unique_prefixes_ignoring_case() {
        let all = vec![
            json!({ "id": "ABC-1", "content": "x" }),
            json!({ "id": "abd-2", "content": "y" }),
        ];
        let picked = resolve(&all, &["abc".into(), "ABC-1".into()]).unwrap();
        assert_eq!(picked.len(), 1);
        assert_eq!(
            resolve(&all, &["ab".into()]).unwrap_err().message,
            "\"ab\" matches 2 saved memories; use a longer id prefix."
        );
        assert_eq!(
            resolve(&all, &[" ".into()]).unwrap_err().message,
            "Saved memory id cannot be empty."
        );
    }

    #[test]
    fn the_summary_table_lists_sections_or_the_empty_message() {
        let summary = json!({ "sections": [
            { "title": "A", "description": "one" }, { "title": "B", "description": "two" }
        ] });
        assert_eq!(summary_table(&summary), "A\none\n\nB\ntwo");
        assert_eq!(
            summary_table(&json!({ "sections": [], "emptyStateMessage": null })),
            "No memory summary available."
        );
        assert_eq!(
            summary_table(&json!({ "sections": [], "emptyStateMessage": "Nothing yet" })),
            "Nothing yet"
        );
    }
}
