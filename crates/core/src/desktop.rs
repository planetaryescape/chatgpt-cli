//! The macOS programs the TS CLI hands things to: `pbcopy` for `export
//! --copy` and the TUI's `c`, `open` for a chat's link (`src/commands/export.ts`,
//! `src/tui/app.tsx`, `src/commands/review.ts` @ 1b8c950). Both are found on
//! `PATH`, as `Bun.spawn` finds them.

use std::io::Write;
use std::process::{Command, Stdio};

use crate::ErrorKind;

/// A chat's page on chatgpt.com.
pub fn chat_url(id: &str) -> String {
    format!("https://chatgpt.com/c/{id}")
}

/// Why `pbcopy` or `open` couldn't do its job. `Display` is the message to
/// show; `kind` is the exit kind a command reports it with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopError {
    pub kind: ErrorKind,
    message: String,
}

impl DesktopError {
    fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for DesktopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DesktopError {}

/// `copyToClipboard`: `text` into `pbcopy`. The messages are the TS CLI's.
pub fn copy_to_clipboard(text: &str) -> Result<(), DesktopError> {
    if !cfg!(target_os = "macos") {
        return Err(DesktopError::new(
            ErrorKind::Unsupported,
            "--copy uses pbcopy and only works on macOS.",
        ));
    }
    let failed = || DesktopError::new(ErrorKind::Internal, "pbcopy failed.");
    let mut child = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
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

/// `Bun.spawn(["open", url])`: open chat `id` in the browser without
/// waiting for it. Its output never reaches the terminal (a TUI owns it).
pub fn open_chat(id: &str) -> Result<(), DesktopError> {
    let mut child = Command::new("open")
        .arg(chat_url(id))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            DesktopError::new(ErrorKind::Internal, format!("can't run open: {error}"))
        })?;
    // Reaped off the caller's thread, so it never lingers as a zombie.
    std::thread::spawn(move || child.wait());
    Ok(())
}
