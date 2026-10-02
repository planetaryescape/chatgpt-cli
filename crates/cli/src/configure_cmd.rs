//! `configure [jev|openai|anthropic] [--remove]`, as the TS CLI's
//! (`src/cli.ts` and `src/auth/config.ts` @ 1b8c950): without a provider,
//! which keys the user config holds; with one, a key read from stdin
//! (hidden at a terminal) saved to it, or with `--remove` taken out. The
//! key is never echoed or logged. Runs here, without the daemon, which
//! reads the file afresh on every call.

use std::io::{IsTerminal, Read, Write};
use std::process::ExitCode;

use chatgpt_core::ErrorKind;
use chatgpt_core::user_config::{self, Provider};
use chatgpt_launcher::ClientError;

use crate::output::{data, note};
use crate::reads::invalid;

fn internal(message: String) -> ClientError {
    ClientError::new(ErrorKind::Internal, message)
}

pub fn configure(provider: Option<String>, remove: bool) -> Result<ExitCode, ClientError> {
    let path = user_config::config_path()
        .ok_or_else(|| internal("can't find the home directory for the user config".into()))?;
    let Some(provider) = provider else {
        if remove {
            return Err(invalid(
                "Choose a provider to remove: jev, openai, or anthropic.",
            ));
        }
        let config = user_config::read_config(&path).map_err(internal)?;
        let mut lines = vec![format!("Config: {}", path.display())];
        for provider in Provider::ALL {
            lines.push(format!(
                "{}: {}",
                provider.name(),
                if config.key(provider).is_some() {
                    "configured"
                } else {
                    "not configured"
                }
            ));
        }
        data(&format!("{}\n", lines.join("\n")));
        return Ok(ExitCode::SUCCESS);
    };
    let provider = Provider::parse(&provider)
        .ok_or_else(|| invalid("Provider must be jev, openai, or anthropic."))?;
    let name = provider.name();
    if remove {
        user_config::set_config_key(&path, provider, None).map_err(internal)?;
        note(&format!("Removed stored {name} key."));
        return Ok(ExitCode::SUCCESS);
    }
    let terminal = std::io::stdin().is_terminal();
    if terminal {
        note(&format!(
            "Enter {name} API key (hidden; press Enter to save):"
        ));
    }
    let key = if terminal {
        read_hidden()?
    } else {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|error| internal(format!("can't read the key from stdin: {error}")))?;
        chatgpt_core::js::trim(&text).to_owned()
    };
    if key.is_empty() {
        return Err(invalid("API key cannot be empty."));
    }
    user_config::set_config_key(&path, provider, Some(&key)).map_err(internal)?;
    note(&format!("Saved {name} key to {}.", path.display()));
    Ok(ExitCode::SUCCESS)
}

/// `readKeyFromStdin` at a terminal: raw mode, nothing echoed; Enter ends
/// it, Backspace takes the last character back, Ctrl-C cancels. The
/// terminal is restored whatever happens, and a line ends the prompt.
fn read_hidden() -> Result<String, ClientError> {
    use nix::sys::termios::{SetArg, tcgetattr, tcsetattr};
    let stdin = std::io::stdin();
    let saved = tcgetattr(&stdin)
        .map_err(|error| internal(format!("can't read the terminal's settings: {error}")))?;
    let raw = crate::prompt::node_raw_mode(&saved);
    tcsetattr(&stdin, SetArg::TCSANOW, &raw)
        .map_err(|error| internal(format!("can't hide the input: {error}")))?;
    let read = read_until_enter(&mut stdin.lock());
    let _ = tcsetattr(&stdin, SetArg::TCSANOW, &saved);
    let mut stderr = std::io::stderr();
    let _ = stderr.write_all(b"\n").and_then(|()| stderr.flush());
    read.map(|key| chatgpt_core::js::trim(&key).to_owned())
}

/// The key typed before Enter, from raw bytes.
fn read_until_enter(input: &mut impl Read) -> Result<String, ClientError> {
    let mut bytes = Vec::new();
    let mut value = String::new();
    let mut buffer = [0u8; 256];
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|error| internal(format!("can't read the key: {error}")))?;
        if read == 0 {
            return Ok(value);
        }
        bytes.extend_from_slice(&buffer[..read]);
        // Characters complete so far; a split one waits for its rest.
        let complete = match std::str::from_utf8(&bytes) {
            Ok(text) => text.len(),
            Err(error) => error.valid_up_to(),
        };
        let text = String::from_utf8_lossy(&bytes[..complete]).into_owned();
        bytes.drain(..complete);
        for c in text.chars() {
            match c {
                '\r' | '\n' => return Ok(value),
                '\u{3}' => {
                    return Err(ClientError::new(ErrorKind::InvalidInput, "Cancelled."));
                }
                '\u{7f}' => {
                    value.pop();
                }
                other => value.push(other),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_input_is_read_as_the_ts_cli_reads_it() {
        let read = |bytes: &[u8]| read_until_enter(&mut &bytes[..]);
        assert_eq!(read(b"sk-abc\rignored").ok().as_deref(), Some("sk-abc"));
        assert_eq!(read(b"ab\x7fc\n").ok().as_deref(), Some("ac"));
        assert_eq!(
            read("k\u{e9}y\r".as_bytes()).ok().as_deref(),
            Some("k\u{e9}y")
        );
        assert_eq!(read(b"partial").ok().as_deref(), Some("partial"));
        assert_eq!(
            read(b"x\x03").err().map(|error| error.message),
            Some("Cancelled.".to_owned())
        );
    }
}
