//! `list` and `stats`: ask the daemon, print as the TS CLI prints
//! (`src/cli.ts`, `src/commands/select.ts` `formatRow` and
//! `src/commands/stats.ts` @ 1b8c950).

use std::process::ExitCode;

use chatgpt_core::{ErrorKind, Paths};
use chatgpt_launcher::ClientError;
use chatgpt_protocol::{Filter, Request, ResponseData, Row, SessionChoice, StatsReport};
use serde_json::{Map, Value, json};

use crate::args::{FilterArgs, ListArgs};
use crate::output::{data, json, note, unexpected};

fn filter(args: FilterArgs) -> Filter {
    Filter {
        older_than: args.older_than,
        newer_than: args.newer_than,
        before: args.before,
        after: args.after,
        title: args.title,
        archived: args.archived,
        all: args.all,
        limit: args.limit,
        suggest: args.suggest,
        topic: args.topic,
        brainstorm: args.brainstorm,
    }
}

fn invalid(message: &str) -> ClientError {
    ClientError::new(ErrorKind::InvalidInput, message)
}

/// `requireSynced`'s note when the index is more than a day old.
pub fn stale_note(synced_at: &str) {
    let Ok(synced) = chrono::DateTime::parse_from_rfc3339(synced_at) else {
        return;
    };
    let hours =
        (chrono::Utc::now().timestamp_millis() - synced.timestamp_millis()) as f64 / 3_600_000.0;
    if hours > 24.0 {
        note(&format!(
            "note: index last synced {}d ago; run `chatgpt sync` to refresh.",
            (hours / 24.0).round()
        ));
    }
}

pub async fn list(paths: &Paths, args: ListArgs) -> Result<ExitCode, ClientError> {
    if args.format.as_deref().is_some_and(|format| format != "ids") {
        return Err(invalid("--format must be ids."));
    }
    if args.json && args.format.is_some() {
        return Err(invalid("Choose only one of --json or --format."));
    }
    if args.count && args.format.is_some() {
        return Err(invalid("Choose only one of --count or --format."));
    }
    let request = Request::List {
        filter: Box::new(filter(args.filters)),
    };
    let ResponseData::Rows(answer) = chatgpt_launcher::ask(paths, request, |_| {}).await? else {
        return Err(unexpected());
    };
    stale_note(&answer.synced_at);
    let rows = answer.rows;
    if args.count {
        data(&format!("{}\n", rows.len()));
    } else if args.format.is_some() {
        let ids: String = rows.iter().map(|row| format!("{}\n", row.id)).collect();
        data(&ids);
    } else if args.json {
        json(&rows.iter().map(json_row).collect::<Vec<Value>>())?;
    } else {
        let text: String = rows
            .iter()
            .map(|row| format!("{}\n", text_row(row)))
            .collect();
        data(&text);
    }
    Ok(ExitCode::SUCCESS)
}

/// A row as `list --json` prints it: the index columns, then
/// `display_title`, `topic` and, with a current judgment, `jev`.
fn json_row(row: &Row) -> Value {
    let mut object = Map::new();
    object.insert("id".into(), json!(row.id));
    object.insert("title".into(), json!(row.title));
    object.insert("create_time".into(), json!(row.create_time));
    object.insert("update_time".into(), json!(row.update_time));
    object.insert("is_archived".into(), json!(row.is_archived));
    object.insert("pinned".into(), json!(row.pinned));
    object.insert("project_id".into(), json!(row.project_id));
    object.insert("local_title".into(), json!(row.local_title));
    object.insert("display_title".into(), json!(row.display_title));
    object.insert("topic".into(), json!(row.topic));
    if let Some(jev) = &row.jev {
        let mut verdict = Map::new();
        verdict.insert("suggestion".into(), json!(jev.suggestion));
        verdict.insert("unsure".into(), json!(jev.unsure));
        verdict.insert("reason".into(), json!(jev.reason));
        verdict.insert("brainstorm".into(), json!(jev.brainstorm));
        // `deep` and `luna` are only ever `true` in the TS CLI's verdicts;
        // otherwise JSON.stringify leaves them out.
        if jev.deep {
            verdict.insert("deep".into(), json!(true));
        }
        if jev.luna {
            verdict.insert("luna".into(), json!(true));
        }
        verdict.insert("answers".into(), jev.answers.clone());
        object.insert("jev".into(), Value::Object(verdict));
    }
    Value::Object(object)
}

/// `formatRow`.
fn text_row(row: &Row) -> String {
    let flags = format!(
        "{}{}{}",
        if row.is_archived != 0 { "A" } else { " " },
        if row.pinned != 0 { "P" } else { " " },
        if row.project_id.as_deref().is_some_and(|id| !id.is_empty()) {
            "J"
        } else {
            " "
        }
    );
    let jev = match &row.jev {
        Some(jev) => {
            let suggestion = format!("{}{}", jev.suggestion, if jev.unsure { "?" } else { "" });
            let idea = jev
                .brainstorm
                .as_deref()
                .map(|kind| format!("idea:{kind}"))
                .unwrap_or_default();
            format!(
                "{suggestion:<9}{:<21}{idea:<14}",
                row.row_topic.as_deref().unwrap_or("")
            )
        }
        None => String::new(),
    };
    let date: String = row.update_time.chars().take(10).collect();
    format!("{}  {date}  {flags}  {jev}{}", row.id, row.display_title)
}

pub async fn stats(
    paths: &Paths,
    args: FilterArgs,
    session: SessionChoice,
) -> Result<ExitCode, ClientError> {
    let request = Request::Stats {
        filter: Box::new(filter(args)),
        session,
    };
    let ResponseData::Stats(report) = chatgpt_launcher::ask(paths, request, |_| {}).await? else {
        return Err(unexpected());
    };
    stale_note(&report.synced_at);
    data(&stats_text(&report));
    match &report.memory_error {
        Some(error) => {
            // console.error in the TS CLI, and exit code 1.
            eprintln!("Saved-memory stats unavailable: {error}");
            Ok(ExitCode::FAILURE)
        }
        None => Ok(ExitCode::SUCCESS),
    }
}

/// `printStats` and `printMemoryStats`.
fn stats_text(report: &StatsReport) -> String {
    let judged = report.chats - report.unjudged;
    let mut out = format!(
        "{} chat(s), {judged} judged, {} not yet judged\n\n",
        report.chats, report.unjudged
    );
    let mut suggestions: Vec<Vec<String>> = report
        .labels
        .iter()
        .zip(&report.suggestions)
        .map(|(label, count)| vec![label.clone(), count.to_string()])
        .collect();
    suggestions.push(vec!["not judged".into(), report.unjudged.to_string()]);
    out.push_str(&table(&["suggestion", "chats"], &suggestions));
    out.push('\n');

    let mut brainstorms: Vec<Vec<String>> = report
        .brainstorms
        .iter()
        .map(|(kind, count)| vec![kind.clone(), count.to_string()])
        .collect();
    let total: u64 = report.brainstorms.iter().map(|(_, count)| count).sum();
    brainstorms.push(vec!["total".into(), total.to_string()]);
    out.push_str(&format!(
        "\n{}\n",
        table(&["brainstorm chats (kept)", "chats"], &brainstorms)
    ));
    out.push_str("Counts are conversations, not distinct products or writing pieces.\n");

    let mut headers = vec!["topic"];
    headers.extend(report.labels.iter().map(String::as_str));
    let topics: Vec<Vec<String>> = report
        .topics
        .iter()
        .map(|topic| {
            std::iter::once(topic.topic.clone())
                .chain(topic.counts.iter().map(u64::to_string))
                .collect()
        })
        .collect();
    out.push_str(&format!("\n{}\n", table(&headers, &topics)));

    if let Some(memory) = &report.memory {
        out.push_str(&format!(
            "\n{} saved memor{}\n",
            memory.total,
            if memory.total == 1 { "y" } else { "ies" }
        ));
        let rows = [
            ("keep", memory.keep),
            ("delete", memory.delete),
            ("review", memory.review),
            ("not classified", memory.unclassified),
        ]
        .map(|(label, count)| vec![label.to_owned(), count.to_string()]);
        out.push_str(&table(&["memory suggestion", "memories"], &rows));
        out.push('\n');
    }
    out
}

/// `table` in stats.ts: the first column left-aligned, the rest right,
/// three spaces apart, with a rule under the headers.
fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let width = |text: &str| text.chars().count();
    let widths: Vec<usize> = (0..headers.len())
        .map(|column| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|cell| width(cell))
                .chain(std::iter::once(width(headers[column])))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |cells: &[String]| {
        cells
            .iter()
            .enumerate()
            .map(|(column, cell)| {
                let pad = widths
                    .get(column)
                    .copied()
                    .unwrap_or(0)
                    .saturating_sub(width(cell));
                if column == 0 {
                    format!("{cell}{}", " ".repeat(pad))
                } else {
                    format!("{}{cell}", " ".repeat(pad))
                }
            })
            .collect::<Vec<_>>()
            .join("   ")
    };
    let header_cells: Vec<String> = headers.iter().map(|header| (*header).to_owned()).collect();
    let rule: Vec<String> = widths.iter().map(|width| "─".repeat(*width)).collect();
    std::iter::once(line(&header_cells))
        .chain(std::iter::once(line(&rule)))
        .chain(rows.iter().map(|row| line(row)))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use chatgpt_protocol::{Jev, MemoryCounts, TopicCounts};

    use super::*;

    fn row() -> Row {
        Row {
            id: "abc".into(),
            title: "Original".into(),
            create_time: "2026-01-01T00:00:00Z".into(),
            update_time: "2026-09-27T11:05:37.123Z".into(),
            is_archived: 0,
            pinned: 1,
            project_id: Some("g-p-1".into()),
            local_title: None,
            display_title: "Original".into(),
            topic: Some("faith".into()),
            row_topic: Some("faith".into()),
            jev: Some(Jev {
                suggestion: "keep".into(),
                unsure: true,
                reason: "r".into(),
                brainstorm: Some("writing".into()),
                deep: false,
                luna: true,
                answers: json!({"b": 1, "a": 0.5}),
            }),
        }
    }

    #[test]
    fn rows_print_as_format_row_does() {
        assert_eq!(
            text_row(&row()),
            "abc  2026-09-27   PJ  keep?    faith                idea:writing  Original"
        );
        let plain = Row {
            jev: None,
            pinned: 0,
            project_id: None,
            is_archived: 1,
            ..row()
        };
        assert_eq!(text_row(&plain), "abc  2026-09-27  A    Original");
    }

    #[test]
    fn json_rows_keep_the_ts_key_order_and_leave_out_false_flags() {
        let text = serde_json::to_string(&json_row(&row())).expect("json");
        assert_eq!(
            text,
            r#"{"id":"abc","title":"Original","create_time":"2026-01-01T00:00:00Z","update_time":"2026-09-27T11:05:37.123Z","is_archived":0,"pinned":1,"project_id":"g-p-1","local_title":null,"display_title":"Original","topic":"faith","jev":{"suggestion":"keep","unsure":true,"reason":"r","brainstorm":"writing","luna":true,"answers":{"b":1,"a":0.5}}}"#
        );
    }

    #[test]
    fn stats_tables_line_up_as_the_ts_clis() {
        let report = StatsReport {
            synced_at: String::new(),
            chats: 12,
            unjudged: 2,
            labels: ["delete", "delete?", "archive", "archive?", "keep", "keep?"]
                .map(str::to_owned)
                .to_vec(),
            suggestions: vec![3, 0, 1, 0, 5, 1],
            brainstorms: vec![
                ("writing".into(), 2),
                ("sermon".into(), 0),
                ("product".into(), 1),
                ("other".into(), 0),
            ],
            topics: vec![TopicCounts {
                topic: "faith".into(),
                counts: vec![0, 0, 0, 0, 10, 0],
            }],
            memory: Some(MemoryCounts {
                total: 1,
                keep: 1,
                ..MemoryCounts::default()
            }),
            memory_error: None,
        };
        let text = stats_text(&report);
        assert!(text.starts_with("12 chat(s), 10 judged, 2 not yet judged\n\nsuggestion   chats\n──────────   ─────\ndelete           3\n"), "{text}");
        assert!(text.contains("\nnot judged       2\n"));
        assert!(text.contains("total                         3\n"));
        assert!(text.contains("topic   delete   delete?   archive   archive?   keep   keep?\n"));
        assert!(text.contains("faith        0         0         0          0     10       0\n"));
        assert!(text.ends_with("\n1 saved memory\nmemory suggestion   memories\n─────────────────   ────────\nkeep                       1\ndelete                     0\nreview                     0\nnot classified             0\n"), "{text}");
    }
}
