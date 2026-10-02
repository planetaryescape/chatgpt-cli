//! `review`: triage chats one at a time, as the TS CLI's `review`
//! (`src/commands/review.ts` @ 1b8c950) does. Each chat's preview shows its
//! dates, Jev's (or Luna's) verdict, the summary a long chat was judged
//! from, and its first and last turns, fetched through the daemon; one key
//! decides it. Nothing changes until the end, when the user types `apply`;
//! then the archives and deletes go through the same `Mutate` path as
//! `archive` and `delete -y`.
//!
//! Keys are read from `/dev/tty` in raw mode, so review works when the ids
//! came in on stdin.

use std::collections::HashMap;
use std::io::{IsTerminal, Read, Write};
use std::os::fd::AsFd;
use std::process::{ExitCode, Stdio};

use chatgpt_core::js::{clip, utf16_prefix};
use chatgpt_core::{ErrorKind, Paths};
use chatgpt_launcher::ClientError;
use chatgpt_protocol::{
    ChatAction, ChatTranscript, Jev, Request, ResponseData, Row, Selection, SessionChoice,
    TranscriptSource,
};
use nix::sys::termios::{self, SetArg, Termios};

use crate::args::ReviewArgs;
use crate::change_cmd::{apply, given_ids, select};
use crate::output::{note, unexpected};
use crate::prompt;
use crate::reads::filter;

const HELP: &str = "[k]eep  [a]rchive  [d]elete  [v]iew  [o]pen in browser  [u]ndo  [q]uit";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Decision {
    Keep,
    Archive,
    Delete,
}

impl Decision {
    fn from_suggestion(suggestion: &str) -> Option<Self> {
        match suggestion {
            "keep" => Some(Self::Keep),
            "archive" => Some(Self::Archive),
            "delete" => Some(Self::Delete),
            _ => None,
        }
    }
}

pub async fn review(
    paths: &Paths,
    args: ReviewArgs,
    session: SessionChoice,
) -> Result<ExitCode, ClientError> {
    let selection = Selection {
        ids: given_ids(args.ids)?,
        filter: filter(args.filters),
        pinned: args.pinned,
        exclude_unsure: false,
        allow_unfiltered: true,
    };
    let mut targets = select(paths, selection).await?;
    if args.oldest_first {
        targets.reverse();
    }
    if targets.is_empty() {
        note("Nothing matched.");
        return Ok(ExitCode::SUCCESS);
    }
    let verdicts = verdicts(paths).await?;
    let decisions = {
        let mut tty = RawTty::open()?;
        triage(paths, &session, &targets, &verdicts, &mut tty).await?
    };

    clear_screen();
    let pick = |wanted: Decision| -> Vec<Row> {
        targets
            .iter()
            .filter(|row| decisions.get(&row.id) == Some(&wanted))
            .cloned()
            .collect()
    };
    let (keep, to_archive, to_delete) = (
        pick(Decision::Keep),
        pick(Decision::Archive),
        pick(Decision::Delete),
    );
    note(&format!(
        "Reviewed {}: keep {}, archive {}, delete {}.",
        decisions.len(),
        keep.len(),
        to_archive.len(),
        to_delete.len()
    ));
    if to_archive.is_empty() && to_delete.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }
    for row in &to_delete {
        note(&format!("delete   {}", row.display_title));
    }
    for row in &to_archive {
        note(&format!("archive  {}", row.display_title));
    }
    if prompt::ask("Type apply to carry these out: ")? != "apply" {
        note("Nothing changed.");
        return Ok(ExitCode::SUCCESS);
    }
    let archived = apply(
        paths,
        ChatAction::Archive,
        &to_archive,
        false,
        true,
        session.clone(),
    )
    .await?;
    let deleted = apply(paths, ChatAction::Delete, &to_delete, false, true, session).await?;
    Ok(if archived == ExitCode::SUCCESS {
        deleted
    } else {
        archived
    })
}

/// Every chat's current verdict and topic, by id (`store.currentJudgments`
/// and `topicOf`).
async fn verdicts(paths: &Paths) -> Result<HashMap<String, (Jev, String)>, ClientError> {
    let request = Request::list_every_chat();
    let ResponseData::Rows(answer) = chatgpt_launcher::ask(paths, request, |_| {}).await? else {
        return Err(unexpected());
    };
    Ok(answer
        .rows
        .into_iter()
        .filter_map(|row| {
            let jev = row.jev?;
            Some((row.id, (jev, row.row_topic.unwrap_or_default())))
        })
        .collect())
}

/// The review loop: a decision per chat, until the last one or `q`.
async fn triage(
    paths: &Paths,
    session: &SessionChoice,
    targets: &[Row],
    verdicts: &HashMap<String, (Jev, String)>,
    tty: &mut RawTty,
) -> Result<HashMap<String, Decision>, ClientError> {
    // Each chat as fetched, once; a failed fetch is tried again when the
    // chat is shown again.
    let mut loaded: HashMap<String, ChatTranscript> = HashMap::new();
    let mut decisions = HashMap::new();
    let mut at = 0;
    // The chat on screen; `None` redraws it.
    let mut shown = None;
    while let Some(row) = targets.get(at) {
        let jev = verdicts.get(&row.id);
        if shown != Some(at) {
            let chat = match loaded.get(&row.id) {
                Some(chat) => chat.clone(),
                None => load(paths, session, &row.id).await,
            };
            show(row, &format!("[{}/{}]", at + 1, targets.len()), jev, &chat);
            if chat.markdown.is_some() {
                loaded.insert(row.id.clone(), chat);
            }
            shown = Some(at);
        }
        let key = tty.read_key()?;
        match key.as_str() {
            "q" | "\u{3}" => break,
            "u" => {
                at = at.saturating_sub(1);
                decisions.remove(&targets[at].id);
                shown = None;
            }
            "v" => {
                if let Some(markdown) = loaded.get(&row.id).and_then(|chat| chat.markdown.as_ref())
                {
                    tty.cooked()?;
                    page(markdown);
                    tty.raw()?;
                }
                shown = None;
            }
            // As `Bun.spawn(["open", …])`: failing to start it isn't the
            // review's problem.
            "o" => {
                let _ = chatgpt_core::desktop::open_chat(&row.id);
            }
            key => {
                let accepted = matches!(key, "\r" | "\n")
                    .then(|| jev.and_then(|(jev, _)| Decision::from_suggestion(&jev.suggestion)))
                    .flatten();
                let typed = match key {
                    "k" | " " => Some(Decision::Keep),
                    "a" => Some(Decision::Archive),
                    "d" => Some(Decision::Delete),
                    _ => None,
                };
                if let Some(decision) = accepted.or(typed) {
                    decisions.insert(row.id.clone(), decision);
                    at += 1;
                }
            }
        }
    }
    Ok(decisions)
}

/// The chat through the batch endpoint, as the TS CLI's `load`, with its
/// summary. Any failure is kept as the reason it couldn't be loaded.
async fn load(paths: &Paths, session: &SessionChoice, id: &str) -> ChatTranscript {
    let request = Request::Transcript {
        id: id.to_owned(),
        source: TranscriptSource::Fresh,
        session: session.clone(),
    };
    let failed = |message: String| ChatTranscript {
        fetch_error: Some(message),
        ..ChatTranscript::default()
    };
    match chatgpt_launcher::ask(paths, request, |_| {}).await {
        Ok(ResponseData::Transcript(chat)) => *chat,
        Ok(_) => failed(unexpected().message),
        Err(error) => failed(error.message),
    }
}

/// `preview`: the chat's header, verdict and summary, then its turns, or
/// why it couldn't be loaded.
fn show(row: &Row, position: &str, jev: Option<&(Jev, String)>, chat: &ChatTranscript) {
    clear_screen();
    let mut out = format!("{position}  {}\n", row.display_title);
    out.push_str(&format!(
        "updated {} · created {}{}{}\n\n",
        utf16_prefix(&row.update_time, 10),
        utf16_prefix(&row.create_time, 10),
        if row.project_id.is_some() {
            " · in a project"
        } else {
            ""
        },
        if row.is_archived != 0 {
            " · archived"
        } else {
            ""
        },
    ));
    if let Some((verdict, topic)) = jev {
        out.push_str(&format!(
            "{} suggests: {}{} · {topic}\n  {}\n\n",
            if verdict.luna { "Luna" } else { "Jev" },
            verdict.suggestion.to_uppercase(),
            if verdict.unsure { " (unsure)" } else { "" },
            verdict.reason
        ));
        if let Some(summary) = &chat.summary {
            out.push_str(&format!("Summary: {}\n\n", clip(summary, 900)));
        }
    }
    match (&chat.excerpt, &chat.fetch_error) {
        (Some(excerpt), _) => {
            out.push_str(&format!("{} turns\n", excerpt.turns));
            if let Some(text) = &excerpt.first_user {
                out.push_str(&format!("\nYou: {}\n", clip(text, 500)));
            }
            if let Some(text) = &excerpt.last_assistant {
                out.push_str(&format!("\nChatGPT (last): {}\n", clip(text, 400)));
            }
        }
        (None, why) => out.push_str(&format!(
            "(could not load: {})\n",
            why.as_deref().unwrap_or("no transcript")
        )),
    }
    if jev.is_some() {
        out.push_str(&format!("\n[enter] accept suggestion  {HELP}\n"));
    } else {
        out.push_str(&format!("\n{HELP}\n"));
    }
    print(&out);
}

fn print(text: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush());
}

/// `console.clear()`: only a terminal is cleared.
fn clear_screen() {
    if std::io::stdout().is_terminal() {
        print("\x1b[H\x1b[2J");
    }
}

/// `$PAGER` (else `less`) with `-R`, the transcript on its stdin. A
/// `PAGER` with arguments (`less -S`) keeps them.
fn page(text: &str) {
    let pager = std::env::var("PAGER")
        .ok()
        .filter(|pager| !pager.trim().is_empty())
        .unwrap_or_else(|| "less".to_owned());
    let mut words = pager.split_whitespace();
    let Some(program) = words.next() else {
        return;
    };
    let child = std::process::Command::new(program)
        .args(words)
        .arg("-R")
        .stdin(Stdio::piped())
        .spawn();
    let Ok(mut child) = child else {
        print(&format!("(couldn't start the pager {program})\n"));
        return;
    };
    if let Some(mut stdin) = child.stdin.take() {
        // A pager quit early stops reading; that's fine.
        let _ = stdin.write_all(text.as_bytes());
    }
    let _ = child.wait();
}

/// `/dev/tty` in the raw mode Node's `setRawMode(true)` sets
/// ([`crate::prompt::node_raw_mode`]). The terminal's settings come back when it's dropped, a panic
/// included.
struct RawTty {
    tty: std::fs::File,
    saved: Termios,
    /// Keys read but not handled yet.
    typed: std::collections::VecDeque<String>,
}

impl RawTty {
    fn open() -> Result<Self, ClientError> {
        let failed = |error: &dyn std::fmt::Display| {
            ClientError::new(
                ErrorKind::InvalidInput,
                format!("review needs a terminal to read keys from ({error})"),
            )
        };
        let tty = std::fs::File::open("/dev/tty").map_err(|error| failed(&error))?;
        let saved = termios::tcgetattr(tty.as_fd()).map_err(|error| failed(&error))?;
        let raw = Self {
            tty,
            saved,
            typed: std::collections::VecDeque::new(),
        };
        raw.raw()?;
        Ok(raw)
    }

    fn raw(&self) -> Result<(), ClientError> {
        self.set(&crate::prompt::node_raw_mode(&self.saved))
    }

    fn cooked(&self) -> Result<(), ClientError> {
        self.set(&self.saved)
    }

    fn set(&self, settings: &Termios) -> Result<(), ClientError> {
        termios::tcsetattr(self.tty.as_fd(), SetArg::TCSANOW, settings).map_err(|error| {
            ClientError::new(
                ErrorKind::Internal,
                format!("can't set the terminal's mode: {error}"),
            )
        })
    }

    /// The next key. Keys typed faster than they're handled arrive in one
    /// read, which Node hands over as one `data` chunk that matches no
    /// key; here each character is a key of its own, and an escape
    /// sequence (an arrow key) stays whole.
    fn read_key(&mut self) -> Result<String, ClientError> {
        while self.typed.is_empty() {
            let mut buffer = [0u8; 64];
            let read = self.tty.read(&mut buffer).map_err(|error| {
                ClientError::new(
                    ErrorKind::Internal,
                    format!("can't read a key from the terminal: {error}"),
                )
            })?;
            // End of input reads as `q`: nothing more will come.
            if read == 0 {
                return Ok("q".to_owned());
            }
            self.typed
                .extend(keys(&String::from_utf8_lossy(&buffer[..read])));
        }
        Ok(self.typed.pop_front().unwrap_or_default())
    }
}

/// A read's keys: an escape sequence whole, else one per character.
fn keys(chunk: &str) -> Vec<String> {
    if chunk.starts_with('\x1b') {
        return vec![chunk.to_owned()];
    }
    chunk.chars().map(String::from).collect()
}

impl Drop for RawTty {
    fn drop(&mut self) {
        let _ = self.cooked();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_splits_into_keys_but_an_escape_sequence_stays_whole() {
        assert_eq!(keys("ak\r"), ["a", "k", "\r"]);
        assert_eq!(keys("\x1b[A"), ["\x1b[A"]);
    }
}
