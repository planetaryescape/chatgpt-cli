//! `$PAGER` with a transcript, for `v` in the TUI and in `review`: the
//! terminal is handed to it as a shell hands it to a foreground job, and
//! taken back when it quits.

use std::fs::File;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use nix::errno::Errno;
use nix::sys::signal::{SigSet, Signal, killpg, raise};
use nix::sys::wait::{WaitPidFlag, WaitStatus, waitpid};
use nix::unistd::{Pid, getpgrp, tcgetpgrp, tcsetpgrp};

/// `$PAGER` (else `less`) with `-R`, `text` on its stdin without its
/// control characters ([`pageable`]), until it quits. A `PAGER` with
/// arguments (`less -S`) keeps them. The caller hands it the terminal.
///
/// The pager runs as a shell runs a job in the foreground: in a process
/// group of its own that is given the terminal ([`in_foreground`]), so
/// Ctrl-C and Ctrl-Z there reach the pager alone. Sharing the caller's
/// group, a Ctrl-C that ended the pager also ended the TUI.
pub fn page(text: &str) -> std::io::Result<()> {
    let pager = std::env::var("PAGER")
        .ok()
        .filter(|pager| !pager.trim().is_empty())
        .unwrap_or_else(|| "less".to_owned());
    let mut words = pager.split_whitespace();
    let Some(program) = words.next() else {
        return Ok(());
    };
    let terminal = foreground_terminal();
    let mut command = Command::new(program);
    command.args(words).arg("-R").stdin(Stdio::piped());
    if terminal.is_some() {
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|_| std::io::Error::other(format!("couldn't start the pager {program}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        let text = pageable(text);
        // Off this thread: a pager stopped with Ctrl-Z stops reading, and
        // this one must still wait for it. A pager quit early stops
        // reading too; that's fine.
        std::thread::spawn(move || {
            let _ = stdin.write_all(text.as_bytes());
        });
    }
    match (terminal, i32::try_from(child.id())) {
        (Some(terminal), Ok(pid)) => in_foreground(&terminal, Pid::from_raw(pid)),
        _ => {
            let _ = child.wait();
        }
    }
    Ok(())
}

/// The controlling terminal, when this process's group is the one in its
/// foreground: only then can it give the terminal to a pager.
fn foreground_terminal() -> Option<File> {
    let terminal = File::options()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    (tcgetpgrp(&terminal).ok()? == getpgrp()).then_some(terminal)
}

/// Wait for `pager`, the leader of its own process group, with `terminal`
/// given to that group, and take the terminal back once it's gone.
///
/// Ctrl-Z stops only the pager, so it's passed on for the shell to see:
/// this process stops too, with the terminal back, and once continued
/// (`fg`) gives the pager the terminal and continues it. A process run
/// through a wrapper (`cargo run`) stops alone, and a second Ctrl-Z stops
/// the wrapper. In a group no shell controls, the stop is ignored and the
/// pager is continued at once.
fn in_foreground(terminal: &File, pager: Pid) {
    let ours = getpgrp();
    give(terminal, pager);
    // A pager that touched the terminal before it had it was stopped.
    let _ = killpg(pager, Signal::SIGCONT);
    loop {
        match waitpid(pager, Some(WaitPidFlag::WUNTRACED)) {
            Ok(WaitStatus::Stopped(..)) => {
                give(terminal, ours);
                // Stops this process before it returns. A stop sent to the
                // whole group (`killpg`) lands on any thread, and could stop
                // the process again after `fg` had continued it.
                let _ = raise(Signal::SIGTSTP);
                give(terminal, pager);
                let _ = killpg(pager, Signal::SIGCONT);
            }
            Err(Errno::EINTR) => {}
            _ => break,
        }
    }
    give(terminal, ours);
}

/// `tcsetpgrp`, which a process outside the terminal's foreground may
/// call only with SIGTTOU blocked.
fn give(terminal: &File, group: Pid) {
    let mut ttou = SigSet::empty();
    ttou.add(Signal::SIGTTOU);
    let _ = ttou.thread_block();
    let _ = tcsetpgrp(terminal, group);
    let _ = ttou.thread_unblock();
}

/// `text` with newlines and tabs but no other control character. A chat
/// can carry escape sequences (pasted, or echoed from a web page), and a
/// pager other than `less -R` would hand them to the terminal, which can
/// set the clipboard or the window title, or draw over what's shown. The
/// TUI's preview drops them too.
fn pageable(text: &str) -> String {
    text.chars()
        .filter(|&c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::pageable;

    #[test]
    fn a_paged_transcript_keeps_its_lines_and_tabs() {
        assert_eq!(pageable("# One\n\n\tcode ✓\n"), "# One\n\n\tcode ✓\n");
    }

    #[test]
    fn a_paged_transcript_loses_escape_sequences_and_other_controls() {
        // OSC 52 (clipboard), a title, SGR, a C1 CSI, a carriage return and a bell.
        let chat = "a\x1b]52;c;cHduZWQ=\x07b\x1b]0;title\x1b\\c\x1b[31md\u{9b}2Je\rf\x07";
        assert_eq!(pageable(chat), "a]52;c;cHduZWQ=b]0;title\\c[31md2Jef");
    }
}
