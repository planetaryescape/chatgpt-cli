//! Lexical `search`: the daemon answers from its search index; this prints
//! the hits as the TS CLI's `search` action and `formatSearchResults`
//! (`src/cli.ts`, `src/search/output.ts` @ 1b8c950) do.

use std::process::ExitCode;

use chatgpt_core::js::{collapse_spaces, number, trim};
use chatgpt_core::js_number_string;
use chatgpt_launcher::ClientError;
use chatgpt_protocol::{Request, ResponseData, SearchHit};
use unicode_width::UnicodeWidthStr;

use crate::args::SearchArgs;
use crate::output::{data, json, note, unexpected};
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

pub async fn search(
    paths: &chatgpt_core::Paths,
    args: SearchArgs,
) -> Result<ExitCode, ClientError> {
    let limit = limit(&args.limit)?;
    let format = format(args.format.as_deref())?;
    let request = Request::Search {
        query: args.query,
        limit,
        archived: args.archived,
        all: args.all,
    };
    let ResponseData::SearchHits(results) = chatgpt_launcher::ask(paths, request, |_| {}).await?
    else {
        return Err(unexpected());
    };
    stale_note(&results.synced_at);
    if results.indexed < results.chats {
        note(&format!(
            "{} of {} chats indexed; the daemon is indexing the rest in the background.",
            results.indexed, results.chats
        ));
    }
    if format == Format::Json {
        json(&results.hits)?;
    } else {
        let output = render(&results.hits, format);
        // `if (output) console.log(output)`.
        if !output.is_empty() {
            data(&format!("{output}\n"));
        }
    }
    note(&format!(
        "{} conversation(s) found locally.",
        results.hits.len()
    ));
    Ok(ExitCode::SUCCESS)
}

/// `formatSearchResults` for every format but JSON.
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
                    js_number_string(hit.score),
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
        Format::Text | Format::Json => hits
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
            score,
            snippet: snippet.into(),
        }
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
