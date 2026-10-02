//! `export`/`show`: the daemon fetches and renders the chat (an export too
//! large for one frame arrives in parts, which the launcher joins); the
//! markdown goes where the TS CLI's `exportConversation` (`src/commands/export.ts`
//! @ 1b8c950) sends it: a file (`-o`), the clipboard (`-c`), else stdout.

use std::process::ExitCode;

use chatgpt_core::js::{is_space, trim};
use chatgpt_core::{ErrorKind, Paths};
use chatgpt_launcher::ClientError;
use chatgpt_protocol::{Request, ResponseData, SessionChoice};

use crate::args::ExportArgs;
use crate::output::{data, io_error, note, unexpected};
use crate::reads::stale_note;

pub async fn export(
    paths: &Paths,
    args: ExportArgs,
    session: SessionChoice,
) -> Result<ExitCode, ClientError> {
    let stdin_ids = if args.link == "-" {
        stdin_ids(&read_stdin()?)
    } else {
        Vec::new()
    };
    let request = Request::Export {
        reference: args.link,
        stdin_ids,
        archived: args.archived,
        all: args.all,
        session,
    };
    let ResponseData::Exported(chat) = chatgpt_launcher::ask(paths, request, |_| {}).await? else {
        return Err(unexpected());
    };
    if let Some(synced_at) = &chat.synced_at {
        stale_note(synced_at);
    }
    let kb = format!(
        "{} KB",
        (chat.markdown.len() as f64 / 1024.0).round() as u64
    );
    // `opts.output` is truthy: `-o` alone, or a non-empty name.
    let output = match args.output {
        Some(None) => Some(format!("{}.md", slugify(&chat.title))),
        Some(Some(name)) if !name.is_empty() => Some(name),
        _ => None,
    };
    if let Some(path) = &output {
        std::fs::write(path, &chat.markdown)
            .map_err(|error| io_error(std::path::Path::new(&path), &error))?;
        note(&format!("wrote {path} ({kb})"));
    }
    if args.copy {
        copy_to_clipboard(&chat.markdown)?;
        note(&format!(
            "copied \"{}\" to the clipboard ({kb})",
            chat.title
        ));
    }
    if output.is_none() && !args.copy {
        data(&chat.markdown);
    }
    Ok(ExitCode::SUCCESS)
}

pub fn read_stdin() -> Result<String, ClientError> {
    let mut text = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).map_err(|error| {
        ClientError::new(ErrorKind::Internal, format!("reading stdin: {error}"))
    })?;
    Ok(text)
}

/// `readStdinIds`: the first word of each line, so `list` output can be
/// piped in.
pub fn stdin_ids(text: &str) -> Vec<String> {
    text.split('\n')
        .filter_map(|line| trim(line).split(is_space).next())
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `slugify`: lowercase ASCII letters and digits, other runs as `-`, at
/// most 80 characters.
fn slugify(title: &str) -> String {
    let mut slug = String::new();
    for c in title.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.strip_prefix('-').unwrap_or(&slug);
    let slug = slug.strip_suffix('-').unwrap_or(slug);
    let slug: String = slug.chars().take(80).collect();
    if slug.is_empty() {
        "conversation".to_owned()
    } else {
        slug
    }
}

/// `copyToClipboard`.
fn copy_to_clipboard(text: &str) -> Result<(), ClientError> {
    chatgpt_core::desktop::copy_to_clipboard(text)
        .map_err(|(kind, message)| ClientError::new(kind, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdin_ids_are_the_first_word_of_each_line() {
        assert_eq!(
            stdin_ids("abc  2026-01-01  title\n\n  def\tx\r\n\u{a0}ghi\n"),
            ["abc", "def", "ghi"]
        );
        assert!(stdin_ids("").is_empty());
    }

    #[test]
    fn titles_slugify_as_the_ts_cli_does() {
        assert_eq!(slugify("Rust: async & await!"), "rust-async-await");
        assert_eq!(slugify("--Café crème--"), "caf-cr-me");
        assert_eq!(slugify("日本語"), "conversation");
        assert_eq!(slugify("İstanbul"), "i-stanbul");
        let long = slugify(&"abc ".repeat(40));
        assert_eq!(long.len(), 80);
        assert!(
            long.ends_with('-'),
            "cut after the dash, as slice(0, 80) does"
        );
    }
}
