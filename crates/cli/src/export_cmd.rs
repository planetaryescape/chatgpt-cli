//! `export`/`show`: the daemon fetches and renders the chat; the markdown
//! goes where the TS CLI's `exportConversation` (`src/commands/export.ts`
//! @ 1b8c950) sends it: a file (`-o`), the clipboard (`-c`), else stdout.

use std::io::Write;
use std::process::{ExitCode, Stdio};

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
    let request = Request::Export {
        reference: args.link,
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
    if let Some(output) = &args.output {
        let path = if output.is_empty() {
            format!("{}.md", slugify(&chat.title))
        } else {
            output.clone()
        };
        std::fs::write(&path, &chat.markdown)
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
    if args.output.is_none() && !args.copy {
        data(&chat.markdown);
    }
    Ok(ExitCode::SUCCESS)
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

/// `copyToClipboard`: the markdown into `pbcopy`.
fn copy_to_clipboard(text: &str) -> Result<(), ClientError> {
    if !cfg!(target_os = "macos") {
        return Err(ClientError::new(
            ErrorKind::Unsupported,
            "--copy uses pbcopy and only works on macOS.",
        ));
    }
    let failed = || ClientError::new(ErrorKind::Internal, "pbcopy failed.");
    let mut child = std::process::Command::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|_| failed())?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes()).map_err(|_| failed())?;
    }
    match child.wait() {
        Ok(status) if status.success() => Ok(()),
        _ => Err(failed()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
