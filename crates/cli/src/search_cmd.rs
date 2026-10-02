//! `search` and `search-index`: the daemon answers from its search index
//! (or, with `--remote`, from ChatGPT's search); this prints the hits as the
//! TS CLI's `search` action and `formatSearchResults` (`src/cli.ts`,
//! `src/search/output.ts` @ 1b8c950) do.

use std::process::ExitCode;

use chatgpt_core::js::{collapse_spaces, number, trim};
use chatgpt_core::{format_duration, js_number_string};
use chatgpt_launcher::ClientError;
use chatgpt_protocol::{Request, ResponseData, SearchHit, SearchMode, SessionChoice};
use unicode_width::UnicodeWidthStr;

use crate::args::{ScopeArgs, SearchArgs};
use crate::output::{ProgressLines, data, note, unexpected};
use crate::reads::{invalid, stale_note};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Text,
    Json,
    Csv,
    Table,
    Ids,
}

/// `Number(limit)`, a safe integer of at least 1.
fn limit(raw: &str) -> Result<u64, ClientError> {
    let limit = number(raw);
    const MAX_SAFE: f64 = 9_007_199_254_740_991.0;
    if limit.fract() != 0.0 || !(1.0..=MAX_SAFE).contains(&limit) {
        return Err(invalid("--limit must be a positive integer."));
    }
    Ok(limit as u64)
}

fn format(raw: Option<&str>) -> Result<Format, ClientError> {
    match raw {
        None => Ok(Format::Text),
        Some("json") => Ok(Format::Json),
        Some("csv") => Ok(Format::Csv),
        Some("table") => Ok(Format::Table),
        Some("ids") => Ok(Format::Ids),
        Some(_) => Err(invalid("--format must be json, csv, table, or ids.")),
    }
}

/// ChatGPT's search API returns at most this many.
const REMOTE_MAX: u64 = 40;

pub async fn search(
    paths: &chatgpt_core::Paths,
    args: SearchArgs,
    session: SessionChoice,
) -> Result<ExitCode, ClientError> {
    let limit = limit(&args.limit)?;
    if u8::from(args.semantic) + u8::from(args.hybrid) + u8::from(args.remote) > 1 {
        return Err(invalid(
            "Choose only one of --semantic, --hybrid, or --remote.",
        ));
    }
    if args.remote && limit > REMOTE_MAX {
        return Err(invalid(
            "--remote supports --limit up to 40 (ChatGPT's search API limit).",
        ));
    }
    let format = format(args.format.as_deref())?;
    let mode = if args.hybrid {
        SearchMode::Hybrid
    } else if args.semantic {
        SearchMode::Semantic
    } else {
        SearchMode::Lexical
    };
    let request = if args.remote {
        Request::RemoteSearch {
            query: args.query,
            limit,
            archived: args.archived,
            all: args.all,
            session,
        }
    } else {
        Request::Search {
            query: args.query,
            limit,
            archived: args.archived,
            all: args.all,
            mode,
        }
    };
    let ResponseData::SearchHits(results) = chatgpt_launcher::ask(paths, request, |_| {}).await?
    else {
        return Err(unexpected());
    };
    // The remote search reads no index, and says nothing more.
    let local = !args.remote;
    if local {
        stale_note(&results.synced_at);
        if results.indexed < results.chats {
            note(&format!(
                "{} of {} chats indexed; the daemon is indexing the rest in the background.",
                results.indexed, results.chats
            ));
        }
        if mode != SearchMode::Lexical && results.embedded < results.chunks {
            note(&format!(
                "{} of {} chunks embedded; the daemon is embedding the rest in the background.",
                results.embedded, results.chunks
            ));
        }
    }
    // `if (output) console.log(output)`.
    let output = render(&results.hits, format);
    if !output.is_empty() {
        data(&format!("{output}\n"));
    }
    if local {
        note(&format!(
            "{} conversation(s) found locally.",
            results.hits.len()
        ));
    }
    Ok(ExitCode::SUCCESS)
}

/// `search-index`: wait for the daemon's indexer and embedder to catch up,
/// showing their progress, then report as the TS CLI's `search-index` does.
pub async fn search_index(
    paths: &chatgpt_core::Paths,
    scope: ScopeArgs,
) -> Result<ExitCode, ClientError> {
    let request = Request::SearchIndex {
        archived: scope.archived,
        all: scope.all,
    };
    let mut progress = ProgressLines::new();
    let answer = chatgpt_launcher::ask(paths, request, |event| progress.show(event)).await;
    progress.close();
    let ResponseData::SearchIndexed(report) = answer? else {
        return Err(unexpected());
    };
    for failure in &report.failures {
        eprintln!("failed: {failure}");
    }
    for why in &report.waiting {
        note(&format!("waiting: {why}"));
    }
    note(&format!(
        "Search index in {}: {}/{} chats, {} text chunks, {} embeddings.",
        format_duration(report.elapsed_ms as f64),
        report.indexed,
        report.chats,
        report.chunks,
        report.embedded
    ));
    let complete = report.indexed == report.chats && report.embedded == report.chunks;
    if report.failures.is_empty() && complete {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}

/// `text` as a JSON string, escaped as `JSON.stringify` escapes it.
fn quoted(text: &str) -> String {
    serde_json::Value::from(text).to_string()
}

/// A hit's snippet as JSON: `JSON.stringify` escapes a lone surrogate left
/// by a cut, where text output shows U+FFFD.
fn snippet_json(hit: &SearchHit) -> String {
    let Some(unit) = hit.snippet_cut else {
        return quoted(&hit.snippet);
    };
    let kept = quoted(hit.snippet.strip_suffix('\u{FFFD}').unwrap_or(&hit.snippet));
    let open = kept.strip_suffix('"').unwrap_or(&kept);
    format!("{open}\\u{unit:04x}\"")
}

/// `JSON.stringify(results, null, 2)`, numbers formatted as JS does.
fn json_text(hits: &[SearchHit]) -> String {
    if hits.is_empty() {
        return "[]".to_owned();
    }
    let objects: Vec<String> = hits
        .iter()
        .map(|hit| {
            let score = hit.score.map_or_else(|| "null".to_owned(), js_number_string);
            format!(
                "  {{\n    \"id\": {},\n    \"title\": {},\n    \"updated\": {},\n    \"archived\": {},\n    \"score\": {score},\n    \"snippet\": {}\n  }}",
                quoted(&hit.id),
                quoted(&hit.title),
                quoted(&hit.updated),
                hit.archived,
                snippet_json(hit),
            )
        })
        .collect();
    format!("[\n{}\n]", objects.join(",\n"))
}

/// `formatSearchResults`.
fn render(hits: &[SearchHit], format: Format) -> String {
    let date = |hit: &SearchHit| hit.updated.chars().take(10).collect::<String>();
    match format {
        Format::Ids => hits
            .iter()
            .map(|hit| hit.id.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        Format::Csv => std::iter::once("id,title,updated,archived,score,snippet".to_owned())
            .chain(hits.iter().map(|hit| {
                [
                    hit.id.clone(),
                    hit.title.clone(),
                    hit.updated.clone(),
                    hit.archived.to_string(),
                    // `String(value ?? "")`.
                    hit.score.map(js_number_string).unwrap_or_default(),
                    hit.snippet.clone(),
                ]
                .iter()
                .map(|cell| format!("\"{}\"", cell.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(",")
            }))
            .collect::<Vec<_>>()
            .join("\n"),
        Format::Table => table(hits),
        Format::Json => json_text(hits),
        Format::Text => hits
            .iter()
            .map(|hit| {
                format!(
                    "{}  {}  {}  {}\n    {}",
                    hit.id,
                    date(hit),
                    if hit.archived { "A" } else { " " },
                    hit.title,
                    hit.snippet
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// `searchTable`.
fn table(hits: &[SearchHit]) -> String {
    const WIDTHS: [usize; 5] = [12, 10, 1, 36, 50];
    let row = |cells: [String; 5]| {
        cells
            .iter()
            .zip(WIDTHS)
            .map(|(cell, width)| cell_text(cell, width))
            .collect::<Vec<_>>()
            .join("  ")
    };
    let mut lines = vec![
        row(["id", "updated", "A", "title", "snippet"].map(str::to_owned)),
        row(WIDTHS.map(|width| "─".repeat(width))),
    ];
    for hit in hits {
        lines.push(row([
            hit.id.chars().take(12).collect(),
            hit.updated.chars().take(10).collect(),
            if hit.archived { "A" } else { "" }.to_owned(),
            hit.title.clone(),
            hit.snippet.clone(),
        ]));
    }
    lines.join("\n")
}

/// `tableCell`: whitespace collapsed, then padded or clipped with `…` to
/// `width` terminal columns (`Bun.stringWidth`).
fn cell_text(value: &str, width: usize) -> String {
    let collapsed = collapse_spaces(value);
    let text = trim(&collapsed);
    let columns = text.width();
    if columns <= width {
        return format!("{text}{}", " ".repeat(width - columns));
    }
    let mut clipped = String::new();
    for c in text.chars() {
        let next = format!("{clipped}{c}");
        if next.width() > width - 1 {
            break;
        }
        clipped = next;
    }
    let result = format!("{clipped}…");
    let pad = width.saturating_sub(result.width());
    format!("{result}{}", " ".repeat(pad))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(id: &str, title: &str, archived: bool, score: f64, snippet: &str) -> SearchHit {
        SearchHit {
            id: id.into(),
            title: title.into(),
            updated: "2026-09-27T11:05:37.123456Z".into(),
            archived,
            score: Some(score),
            snippet: snippet.into(),
            snippet_cut: None,
        }
    }

    #[test]
    fn json_is_written_as_json_stringify_writes_it() {
        let mut remote = hit("r", "Line\nbreak \"q\"", true, 0.0, "cut \u{FFFD}");
        remote.score = None;
        remote.snippet_cut = Some(0xD83D);
        let hits = [hit("a", "T", false, 1.5e-7, "é"), remote];
        // `bun -e 'console.log(JSON.stringify([...], null, 2))'` for the
        // same values, with "cut \ud83d" as the second snippet.
        assert_eq!(
            json_text(&hits),
            "[\n  {\n    \"id\": \"a\",\n    \"title\": \"T\",\n    \"updated\": \"2026-09-27T11:05:37.123456Z\",\n    \"archived\": false,\n    \"score\": 1.5e-7,\n    \"snippet\": \"é\"\n  },\n  {\n    \"id\": \"r\",\n    \"title\": \"Line\\nbreak \\\"q\\\"\",\n    \"updated\": \"2026-09-27T11:05:37.123456Z\",\n    \"archived\": true,\n    \"score\": null,\n    \"snippet\": \"cut \\ud83d\"\n  }\n]"
        );
        assert_eq!(json_text(&[]), "[]");
        // Text formats show the cut as U+FFFD; CSV leaves a null score empty.
        assert!(render(&hits[1..], Format::Text).ends_with("cut \u{FFFD}"));
        assert!(render(&hits[1..], Format::Csv).contains("\"true\",\"\",\"cut"));
    }

    #[test]
    fn limits_and_formats_are_checked_as_the_ts_cli_checks_them() {
        assert_eq!(limit("20").ok(), Some(20));
        assert_eq!(limit(" 5 ").ok(), Some(5));
        assert_eq!(limit("1e1").ok(), Some(10));
        for bad in ["0", "-1", "2.5", "abc", "", "9007199254740992"] {
            assert_eq!(
                limit(bad).err().map(|error| error.message),
                Some("--limit must be a positive integer.".to_owned()),
                "{bad:?}"
            );
        }
        assert!(format(Some("yaml")).is_err());
        assert_eq!(format(None).ok(), Some(Format::Text));
    }

    #[test]
    fn hits_print_in_each_format() {
        let hits = [
            hit("abc", "Rust \"async\"", false, 1.5e-7, "a, b"),
            hit("def", "Old", true, 3.0, "x"),
        ];
        assert_eq!(
            render(&hits, Format::Text),
            "abc  2026-09-27     Rust \"async\"\n    a, b\ndef  2026-09-27  A  Old\n    x"
        );
        assert_eq!(render(&hits, Format::Ids), "abc\ndef");
        assert_eq!(
            render(&hits, Format::Csv),
            "id,title,updated,archived,score,snippet\n\
             \"abc\",\"Rust \"\"async\"\"\",\"2026-09-27T11:05:37.123456Z\",\"false\",\"1.5e-7\",\"a, b\"\n\
             \"def\",\"Old\",\"2026-09-27T11:05:37.123456Z\",\"true\",\"3\",\"x\""
        );
        assert_eq!(render(&[], Format::Text), "");
        assert_eq!(
            render(&[], Format::Csv),
            "id,title,updated,archived,score,snippet"
        );
    }

    #[test]
    fn table_cells_pad_and_clip_by_terminal_width() {
        assert_eq!(cell_text(" a\n b ", 5), "a b  ");
        assert_eq!(cell_text("abcdefgh", 5), "abcd…");
        // Wide characters take two columns, as Bun.stringWidth counts them.
        assert_eq!(cell_text("中文字符", 5), "中文…");
        assert_eq!(cell_text("👍👍👍", 4), "👍… ");
        let table = table(&[hit("0123456789abcdef", "T", true, 1.0, "s")]);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(
            lines[0].trim_end(),
            "id            updated     A  title                                 snippet"
        );
        assert!(lines[2].starts_with("0123456789ab  2026-09-27  A  T "));
    }

    #[test]
    fn widths_match_bun_string_width() {
        // Bun 1.3.14's Bun.stringWidth for each.
        for (text, bun) in [
            ("👍🏽", 2),
            ("👨‍👩‍👧", 2),
            ("é", 1),
            ("中", 2),
            ("e\u{301}", 1),
            ("\u{200b}", 0),
            ("❤️", 2),
            ("☺", 1),
            ("…", 1),
            ("─", 1),
        ] {
            assert_eq!(text.width(), bun, "{text:?}");
        }
    }
}
