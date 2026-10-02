//! Confirmation prompts, as the TS CLI's `ask` (`src/commands/mutate.ts`
//! @ 1b8c950) asks them: the question on stderr, one line of answer from
//! the terminal. When ids came in on stdin, stdin is spent, so the answer
//! comes from `/dev/tty`.

use std::io::{BufRead, IsTerminal, Write};

use chatgpt_core::ErrorKind;
use chatgpt_launcher::ClientError;

/// Ask `question` and return the answer, trimmed. End of input is an empty
/// answer.
pub fn ask(question: &str) -> Result<String, ClientError> {
    // Never take an answer to a question that wasn't shown.
    let mut stderr = std::io::stderr().lock();
    write!(stderr, "{question}")
        .and_then(|()| stderr.flush())
        .map_err(|error| {
            ClientError::new(
                ErrorKind::InvalidInput,
                format!("can't show the confirmation prompt ({error}); pass -y to skip it"),
            )
        })?;
    drop(stderr);
    let mut line = String::new();
    let stdin = std::io::stdin();
    let read = if stdin.is_terminal() {
        stdin.lock().read_line(&mut line)
    } else {
        std::fs::File::open("/dev/tty")
            .map(std::io::BufReader::new)
            .and_then(|mut tty| tty.read_line(&mut line))
    };
    read.map_err(|error| {
        // A partial prompt line would run into the error.
        eprintln!();
        ClientError::new(
            ErrorKind::InvalidInput,
            format!(
                "can't read an answer from the terminal ({error}); pass -y to skip the confirmation"
            ),
        )
    })?;
    Ok(chatgpt_core::js::trim(&line).to_owned())
}

/// `/^y(es)?$/i`.
pub fn is_yes(answer: &str) -> bool {
    answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes")
}

/// A `[y/N]` question: yes or no.
pub fn confirm(question: &str) -> Result<bool, ClientError> {
    Ok(is_yes(&ask(question)?))
}

/// "Type N to confirm": yes only for exactly `count`.
pub fn confirm_count(question: &str, count: usize) -> Result<bool, ClientError> {
    Ok(ask(question)? == count.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_y_and_yes_in_any_case_mean_yes() {
        for yes in ["y", "Y", "yes", "YeS"] {
            assert!(is_yes(yes), "{yes}");
        }
        for no in ["", "n", "ye", "yess", " y"] {
            assert!(!is_yes(no), "{no}");
        }
    }
}
