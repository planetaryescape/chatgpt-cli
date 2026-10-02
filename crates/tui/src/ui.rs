//! Drawing the TUI, laid out as the TS CLI's `src/tui/app.tsx`,
//! `list-pane.tsx` and `preview-pane.tsx` @ 1b8c950 lay it out: a header
//! with counts, filters and marks; the list; the preview; a footer; and
//! the help, apply, title and applying boxes over them.

use std::borrow::Cow;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Padding, Paragraph, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, HELP, HINTS, Mode, Transcript};
use crate::input::LineInput;
use crate::model::Mark;

/// Tokyo Night-ish, as `theme.ts`: readable on dark terminals, distinct
/// per suggestion.
mod color {
    use ratatui::style::Color;

    pub const FG: Color = Color::Rgb(0xc0, 0xca, 0xf5);
    pub const DIM: Color = Color::Rgb(0x56, 0x5f, 0x89);
    pub const ACCENT: Color = Color::Rgb(0x7a, 0xa2, 0xf7);
    pub const BORDER: Color = Color::Rgb(0x3b, 0x42, 0x61);
    pub const SELECTED_BG: Color = Color::Rgb(0x28, 0x34, 0x57);
    pub const DELETE: Color = Color::Rgb(0xf7, 0x76, 0x8e);
    pub const ARCHIVE: Color = Color::Rgb(0xe0, 0xaf, 0x68);
    pub const KEEP: Color = Color::Rgb(0x9e, 0xce, 0x6a);
    pub const DIALOG_BG: Color = Color::Rgb(0x1a, 0x1b, 0x26);
}

fn suggestion_color(suggestion: &str) -> Color {
    match suggestion {
        "delete" => color::DELETE,
        "archive" => color::ARCHIVE,
        _ => color::KEEP,
    }
}

fn dim(text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::new().fg(color::DIM))
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_header(frame, app, header);
    let list_width = (area.width / 2).max(60).min(area.width);
    let [list, preview] =
        Layout::horizontal([Constraint::Length(list_width), Constraint::Fill(1)]).areas(body);
    draw_list(frame, app, list);
    draw_preview(frame, app, preview);
    draw_footer(frame, app, footer);
    match &app.mode {
        Mode::Help => draw_help(frame, area),
        Mode::Confirm(input) => draw_confirm(frame, app, input, area),
        Mode::Title { input, .. } => draw_title(frame, input, area),
        Mode::Applying { done, total } => {
            let lines = vec![Line::styled(
                format!("{done}/{total} done…"),
                Style::new().fg(color::FG),
            )];
            draw_dialog(frame, area, DIALOG_AT, 50, "Applying", color::ACCENT, lines);
        }
        Mode::Browse | Mode::Filter(_) => {}
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let view = &app.view;
    let mut parts = vec![
        format!("{} of {}", app.visible.len(), app.rows.len()),
        if view.archived { "archived" } else { "active" }.to_owned(),
        view.suggestion.label().to_owned(),
        if view.topic == "all" {
            "all topics".to_owned()
        } else {
            view.topic.clone()
        },
    ];
    if view.older_than != "any" {
        parts.push(format!("older than {}", view.older_than));
    }
    if view.brainstorm != "off" {
        parts.push(format!("brainstorms: {}", view.brainstorm));
    }
    if !view.query.is_empty() {
        parts.push(format!("\"{}\"", view.query));
    }
    let accent = Style::new().fg(color::ACCENT);
    frame.render_widget(
        Paragraph::new(format!("chatgpt · {}", parts.join(" · "))).style(accent),
        area,
    );
    if !app.marks.is_empty() {
        let (delete, archive) = app.mark_counts();
        let marks = format!("marked: {delete} delete, {archive} archive (x to apply)");
        frame.render_widget(
            Paragraph::new(marks)
                .style(accent)
                .alignment(ratatui::layout::Alignment::Right),
            area,
        );
    }
}

/// `text` padded or cut to `width` terminal columns.
fn pad(text: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let c = if c.is_control() { ' ' } else { c };
        let w = c.width().unwrap_or(0);
        if used + w > width {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push_str(&" ".repeat(width - used));
    out
}

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered()
        .border_style(Style::new().fg(color::ACCENT))
        .title("Conversations");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let height = usize::from(inner.height);
    app.list_height = height.max(1);
    app.start = crate::model::window_start(app.selected, app.start, height, app.visible.len());
    if app.visible.is_empty() {
        frame.render_widget(Paragraph::new(dim("No conversations match.")), inner);
        return;
    }
    // mark(2) + date(11) + suggestion(9) + topic(15), then the title.
    let title_width = usize::from(inner.width)
        .saturating_sub(2 + 11 + 9 + 15)
        .max(10);
    let selected = app.selected.min(app.visible.len() - 1);
    let lines: Vec<Line> = app
        .visible
        .iter()
        .enumerate()
        .skip(app.start)
        .take(height)
        .map(|(position, &index)| {
            let row = &app.rows[index];
            let (mark, mark_color) = match app.marks.get(&row.id) {
                Some(Mark::Delete) => ("D ", color::DELETE),
                Some(Mark::Archive) => ("A ", color::ARCHIVE),
                None => ("  ", color::ARCHIVE),
            };
            let (suggestion, suggestion_style) = match &row.jev {
                Some(jev) => (
                    format!("{}{}", jev.suggestion, if jev.unsure { "?" } else { "" }),
                    Style::new().fg(suggestion_color(&jev.suggestion)),
                ),
                None => ("·".to_owned(), Style::new().fg(color::DIM)),
            };
            let brainstorm = row.jev.as_ref().and_then(|jev| jev.brainstorm.as_deref());
            let (topic, topic_color) = match brainstorm {
                Some(kind) => (format!("idea:{kind}"), color::ACCENT),
                None => (row.row_topic.clone().unwrap_or_default(), color::DIM),
            };
            let line = Line::from(vec![
                Span::styled(mark, Style::new().fg(mark_color)),
                dim(pad(day(&row.update_time), 11)),
                Span::styled(pad(&suggestion, 9), suggestion_style),
                Span::styled(pad(&topic, 15), Style::new().fg(topic_color)),
                Span::styled(
                    pad(&row.display_title, title_width),
                    Style::new().fg(color::FG),
                ),
            ]);
            if position == selected {
                line.style(Style::new().bg(color::SELECTED_BG))
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// `YYYY-MM-DD` of an ISO timestamp.
fn day(time: &str) -> &str {
    time.get(..10).unwrap_or(time)
}

/// Tabs as spaces and no other control characters, which would move the
/// terminal's cursor. Borrowed when there's nothing to change, as there
/// usually isn't.
fn printable(text: &str) -> Cow<'_, str> {
    if !text.chars().any(char::is_control) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// `text` wrapped at `width` columns, one line per wrapped line.
fn wrap(text: &str, width: usize, style: Style, into: &mut Vec<Line<'static>>) {
    for line in text.split('\n') {
        let line = printable(line);
        for piece in textwrap::wrap(&line, width.max(1)) {
            into.push(Line::styled(piece.into_owned(), style));
        }
    }
}

/// The preview's scrolling text: the summary, then the transcript or its
/// state. Wrapped once per text and width.
fn preview_text(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(summary) = &app.preview.summary {
        let text = format!("Summary\n{summary}\n\n{}\n", "─".repeat(40));
        wrap(&text, width, Style::new().fg(color::ACCENT), &mut lines);
    }
    match &app.preview.transcript {
        Transcript::Idle => {}
        Transcript::Loading => lines.push(Line::from(dim("Loading transcript…"))),
        Transcript::Failed(why) => wrap(
            &format!("Couldn't load: {why}"),
            width,
            Style::new().fg(color::DELETE),
            &mut lines,
        ),
        Transcript::Ready(markdown) => {
            wrap(markdown, width, Style::new().fg(color::FG), &mut lines);
        }
    }
    lines
}

fn draw_preview(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.current().is_none() {
        let block = Block::bordered().border_style(Style::new().fg(color::BORDER));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(Paragraph::new(dim("Nothing selected.")), inner);
        app.preview_lines = 0;
        return;
    }
    let Some(row) = app.current() else {
        return;
    };
    let block = Block::bordered()
        .border_style(Style::new().fg(color::BORDER))
        .title(printable(&row.display_title));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = usize::from(inner.width);

    let mut meta = vec![
        format!("updated {}", day(&row.update_time)),
        format!("created {}", day(&row.create_time)),
    ];
    if row.project_id.is_some() {
        meta.push("in a project".to_owned());
    }
    if row.pinned != 0 {
        meta.push("pinned".to_owned());
    }
    if row.is_archived != 0 {
        meta.push("archived".to_owned());
    }
    let verdict = match &row.jev {
        Some(jev) => {
            let rest = format!(
                "{} · {} · {}",
                if jev.unsure { " (unsure)" } else { "" },
                row.row_topic.as_deref().unwrap_or(""),
                printable(&jev.reason)
            );
            let height =
                textwrap::wrap(&format!("Jev: {}{rest}", jev.suggestion), width.max(1)).len();
            let line = Line::from(vec![
                dim(if jev.luna { "Luna: " } else { "Jev: " }),
                Span::styled(
                    jev.suggestion.to_uppercase(),
                    Style::new()
                        .fg(suggestion_color(&jev.suggestion))
                        .add_modifier(Modifier::BOLD),
                ),
                dim(rest),
            ]);
            (line, height)
        }
        None => (
            Line::from(dim("Jev: not classified yet (run `chatgpt classify`)")),
            1,
        ),
    };
    let [meta_area, verdict_area, _, text_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(u16::try_from(verdict.1).unwrap_or(u16::MAX)),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);
    frame.render_widget(Paragraph::new(dim(meta.join(" · "))), meta_area);
    frame.render_widget(
        Paragraph::new(verdict.0).wrap(Wrap { trim: false }),
        verdict_area,
    );

    let version = app.preview.version;
    if !matches!(&app.wrapped, Some((v, w, _)) if *v == version && *w == width) {
        app.wrapped = Some((version, width, preview_text(app, width)));
    }
    let total = app.wrapped.as_ref().map_or(0, |(_, _, lines)| lines.len());
    let height = usize::from(text_area.height);
    app.preview_height = height.max(1);
    app.preview_lines = total;
    app.preview.scroll = app.preview.scroll.min(total.saturating_sub(height));
    let shown: Vec<Line> = app
        .wrapped
        .iter()
        .flat_map(|(_, _, lines)| lines.iter())
        .skip(app.preview.scroll)
        .take(height)
        .cloned()
        .collect();
    frame.render_widget(Paragraph::new(shown), text_area);
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let line = match &app.mode {
        Mode::Filter(input) => {
            let mut spans = vec![Span::styled("/", Style::new().fg(color::ACCENT))];
            if input.text().is_empty() {
                spans.push(Span::styled(
                    " ",
                    Style::new().add_modifier(Modifier::REVERSED),
                ));
                spans.push(dim("filter titles, enter to keep, esc to clear"));
            } else {
                spans.extend(input_spans(input));
            }
            Line::from(spans)
        }
        _ if !app.status.is_empty() => Line::from(dim(printable(&app.status))),
        _ => Line::from(dim(HINTS)),
    };
    frame.render_widget(Paragraph::new(line), area);
}

/// The text with the cursor shown as a reversed cell.
fn input_spans(input: &LineInput) -> Vec<Span<'static>> {
    let (before, under, after) = input.split();
    let fg = Style::new().fg(color::FG);
    vec![
        Span::styled(printable(before).into_owned(), fg),
        Span::styled(
            under.map_or_else(
                || " ".to_owned(),
                |c| printable(&c.to_string()).into_owned(),
            ),
            fg.add_modifier(Modifier::REVERSED),
        ),
        Span::styled(printable(after).into_owned(), fg),
    ]
}

/// The TS TUI's dialogs sit 6 columns in and 3 rows down; its help, 4 and 2.
const DIALOG_AT: (u16, u16) = (6, 3);
const HELP_AT: (u16, u16) = (4, 2);

/// A box over the screen at `(left, top)`, `width` wide (narrower on a
/// small terminal), as tall as its lines.
fn draw_dialog(
    frame: &mut Frame,
    area: Rect,
    (left, top): (u16, u16),
    width: u16,
    title: &str,
    border: Color,
    lines: Vec<Line<'static>>,
) {
    let width = width.min(area.width.saturating_sub(left));
    let inner_width = usize::from(width.saturating_sub(4)).max(1);
    let wrapped: usize = lines
        .iter()
        .map(|line| {
            let text: String = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            text.width().div_ceil(inner_width).max(1)
        })
        .sum();
    let height = u16::try_from(wrapped + 4)
        .unwrap_or(u16::MAX)
        .min(area.height.saturating_sub(top));
    let rect = Rect::new(area.x + left, area.y + top, width, height).intersection(area);
    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .border_style(Style::new().fg(border))
        .style(Style::new().bg(color::DIALOG_BG))
        .padding(Padding::uniform(1))
        .title(title.to_owned());
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block),
        rect,
    );
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let lines = HELP
        .lines()
        .map(|line| Line::styled(line, Style::new().fg(color::FG)))
        .collect();
    draw_dialog(frame, area, HELP_AT, 84, "Keys", color::ACCENT, lines);
}

fn draw_confirm(frame: &mut Frame, app: &App, input: &LineInput, area: Rect) {
    let (delete, archive) = app.mark_counts();
    let lines = vec![
        Line::styled(
            format!(
                "Archive {archive} and permanently delete {delete} conversation(s). Deletes cannot be undone."
            ),
            Style::new().fg(color::FG),
        ),
        Line::from(dim("Type apply and press enter. Esc cancels.")),
        Line::from(input_spans(input)),
    ];
    draw_dialog(
        frame,
        area,
        DIALOG_AT,
        70,
        "Apply marks",
        color::DELETE,
        lines,
    );
}

fn draw_title(frame: &mut Frame, input: &LineInput, area: Rect) {
    let lines = vec![
        Line::from(dim(
            "Shown only in this CLI and TUI. Enter saves; Esc cancels.",
        )),
        Line::from(input_spans(input)),
    ];
    draw_dialog(
        frame,
        area,
        DIALOG_AT,
        70,
        "Edit local title",
        color::ACCENT,
        lines,
    );
}
