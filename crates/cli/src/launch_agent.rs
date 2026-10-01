// Adapted from ms-todo crates/cli/src/daemon_service.rs @ 72a406042a4d46c2f8cc7c3afe6bda12e11f923b
// (only files are written, never `launchctl`; the installed instance only;
// write only when different). Changes: macOS only; no systemd unit.

//! `daemon install|uninstall`: start the daemon at login with a LaunchAgent,
//! `~/Library/LaunchAgents/com.planetaryescape.chatgpt-cli.plist`. It runs
//! this binary's `daemon run` for the installed instance, and again after a
//! crash but not after a clean stop. Only the file is written or removed, so
//! nothing changes in the running session; the output says how to load it
//! now.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chatgpt_core::{ErrorKind, Instance, Paths};
use chatgpt_launcher::ClientError;

use crate::output::{io_error, note};

pub const LABEL: &str = "com.planetaryescape.chatgpt-cli";

fn plist_path() -> Result<PathBuf, ClientError> {
    if !cfg!(target_os = "macos") {
        return Err(ClientError::new(
            ErrorKind::Unsupported,
            "starting the daemon at login is built for macOS (launchd) only",
        ));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
        .ok_or_else(|| ClientError::new(ErrorKind::Internal, "HOME isn't set to a directory"))?;
    Ok(home
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist")))
}

/// The agent: `<exe> daemon run --instance default`, kept alive after a
/// crash, with the installing shell's PATH so the daemon finds bun for the
/// TS sync.
fn plist(program: &Path, stderr: &Path, path_var: Option<&str>) -> Result<String, ClientError> {
    use plist::{Dictionary, Value};
    let text = |value: &str| Value::String(value.to_owned());
    let mut agent = Dictionary::new();
    agent.insert("Label".into(), text(LABEL));
    agent.insert(
        "ProgramArguments".into(),
        Value::Array(
            [
                &program.display().to_string(),
                "daemon",
                "run",
                "--instance",
                "default",
            ]
            .map(&text)
            .to_vec(),
        ),
    );
    if let Some(path) = path_var {
        let mut environment = Dictionary::new();
        environment.insert("PATH".into(), text(path));
        agent.insert(
            "EnvironmentVariables".into(),
            Value::Dictionary(environment),
        );
    }
    agent.insert("RunAtLoad".into(), Value::Boolean(true));
    let mut keep_alive = Dictionary::new();
    keep_alive.insert("SuccessfulExit".into(), Value::Boolean(false));
    agent.insert("KeepAlive".into(), Value::Dictionary(keep_alive));
    agent.insert("ProcessType".into(), text("Background"));
    agent.insert(
        "StandardErrorPath".into(),
        text(&stderr.display().to_string()),
    );
    let mut xml = Vec::new();
    Value::Dictionary(agent)
        .to_writer_xml(&mut xml)
        .map_err(|error| {
            ClientError::new(
                ErrorKind::Internal,
                format!("cannot write the agent: {error}"),
            )
        })?;
    Ok(String::from_utf8_lossy(&xml).into_owned())
}

pub fn install(paths: &Paths) -> Result<ExitCode, ClientError> {
    if paths.instance != Instance::Default {
        return Err(ClientError::new(
            ErrorKind::InvalidInput,
            format!(
                "the daemon started at login is the installed one (the default instance), not {:?}; run the installed chatgpt, or pass `--instance default`",
                paths.instance.label()
            ),
        ));
    }
    let file = plist_path()?;
    let program = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|error| {
            ClientError::new(
                ErrorKind::Internal,
                format!("cannot find this executable: {error}"),
            )
        })?;
    // launchd opens StandardErrorPath itself, creating it with its own
    // permissions: make the directory (0700) and the file (0600) first.
    chatgpt_launcher::open_log(&paths.daemon_stderr_file())
        .map_err(|error| io_error(&paths.daemon_stderr_file(), &error))?;
    let contents = plist(
        &program,
        &paths.daemon_stderr_file(),
        std::env::var("PATH").ok().as_deref(),
    )?;
    let unchanged = std::fs::read_to_string(&file).is_ok_and(|current| current == contents);
    if !unchanged {
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(|error| io_error(dir, &error))?;
        }
        std::fs::write(&file, contents).map_err(|error| io_error(&file, &error))?;
    }
    note(&format!(
        "{} {}. To start it now: launchctl bootstrap gui/$(id -u) '{}'",
        if unchanged {
            "Already installed:"
        } else {
            "Installed"
        },
        file.display(),
        file.display()
    ));
    Ok(ExitCode::SUCCESS)
}

pub fn uninstall() -> Result<ExitCode, ClientError> {
    let file = plist_path()?;
    match std::fs::remove_file(&file) {
        Ok(()) => note(&format!(
            "Removed {}. To stop it now: launchctl bootout gui/$(id -u)/{LABEL}",
            file.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            note(&format!(
                "Not installed ({} doesn't exist).",
                file.display()
            ));
        }
        Err(error) => return Err(io_error(&file, &error)),
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_runs_the_installed_daemon() {
        let text = plist(
            Path::new("/Users/a&b/.local/bin/chatgpt"),
            Path::new("/tmp/d.stderr"),
            Some("/usr/bin:/bin"),
        )
        .expect("plist");
        let agent = plist::Value::from_reader_xml(text.as_bytes()).expect("valid XML");
        let agent = agent.as_dictionary().expect("a dictionary");
        let arguments: Vec<&str> = agent["ProgramArguments"]
            .as_array()
            .expect("arguments")
            .iter()
            .filter_map(plist::Value::as_string)
            .collect();
        assert_eq!(
            arguments,
            [
                "/Users/a&b/.local/bin/chatgpt",
                "daemon",
                "run",
                "--instance",
                "default"
            ]
        );
        assert_eq!(
            agent["EnvironmentVariables"].as_dictionary().expect("env")["PATH"].as_string(),
            Some("/usr/bin:/bin")
        );
    }
}
