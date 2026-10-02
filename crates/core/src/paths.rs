// Adapted from ms-todo crates/core/src/paths.rs @ 72a406042a4d46c2f8cc7c3afe6bda12e11f923b
// (itself from spotuify's paths.rs: instance naming and the Cargo `target/`
// detection). Changes: chatgpt-cli names, no config file (the Rust CLI reads
// no config yet), logs in a directory for the daily-rotated daemon log.

//! Where the Rust chatgpt keeps its files.
//!
//! Dev and installed builds are kept apart by an instance name, so an agent
//! running a local build never touches the installed daemon or its index:
//!
//! - `CHATGPT_INSTANCE=<name>` (or `--instance <name>`) uses
//!   `chatgpt-cli-<name>`, except `default`, which picks the installed
//!   instance from any build.
//! - A binary run from Cargo's `target/` tree or any other Cargo output
//!   directory (a custom `CARGO_TARGET_DIR`), or any debug build, defaults
//!   to the `dev` instance.
//! - Everything else uses plain `chatgpt-cli`.
//!
//! The data directory is `dirs::data_dir()` (`~/Library/Application Support`
//! on macOS), so it never collides with the TS CLI's
//! `~/.local/share/chatgpt-cli/index.db`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "chatgpt-cli";
pub const INSTANCE_ENV: &str = "CHATGPT_INSTANCE";
/// The daemon's log files are `<prefix>.YYYY-MM-DD` in [`Paths::log_dir`].
pub const DAEMON_LOG_PREFIX: &str = "daemon.log";
const DEV_INSTANCE: &str = "dev";
/// The name that selects [`Instance::Default`], matching its label.
const DEFAULT_INSTANCE: &str = "default";

/// Which copy of the data a process uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Instance {
    /// The installed build: `<data_dir>/chatgpt-cli`.
    Default,
    /// `<data_dir>/chatgpt-cli-<name>`.
    Named(String),
}

#[derive(Debug, thiserror::Error)]
#[error("invalid instance name {0:?}: use letters, digits, '-' or '_' (at most 64 characters)")]
pub struct InvalidInstanceName(pub String);

impl Instance {
    /// Resolve from `--instance`, then `CHATGPT_INSTANCE`, then the build kind.
    pub fn detect(flag: Option<&str>) -> Result<Self, InvalidInstanceName> {
        let env = std::env::var(INSTANCE_ENV).ok();
        Self::resolve(
            flag,
            env.as_deref(),
            cfg!(debug_assertions) || current_exe_is_cargo_target_build(),
        )
    }

    /// Pure resolution rule, separated from the environment for tests.
    pub fn resolve(
        flag: Option<&str>,
        env: Option<&str>,
        is_target_build: bool,
    ) -> Result<Self, InvalidInstanceName> {
        match flag.or(env).filter(|name| !name.is_empty()) {
            // Lets a target/ build reach the installed instance, and lets the
            // daemon be spawned with the exact instance its client resolved.
            Some(DEFAULT_INSTANCE) => Ok(Self::Default),
            Some(name) => validate(name).map(|()| Self::Named(name.to_owned())),
            None if is_target_build => Ok(Self::Named(DEV_INSTANCE.to_owned())),
            None => Ok(Self::Default),
        }
    }

    /// The directory name under the platform data directory.
    pub fn dir_name(&self) -> String {
        match self {
            Self::Default => APP_NAME.to_owned(),
            Self::Named(name) => format!("{APP_NAME}-{name}"),
        }
    }

    /// The name shown to users; `default` for the installed instance.
    /// Passing it back to [`Instance::resolve`] gives the same instance.
    pub fn label(&self) -> &str {
        match self {
            Self::Default => DEFAULT_INSTANCE,
            Self::Named(name) => name,
        }
    }
}

// The name becomes a path component, so it must not be able to escape the
// data directory.
fn validate(name: &str) -> Result<(), InvalidInstanceName> {
    let ok = name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(InvalidInstanceName(name.to_owned()))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PathsError {
    #[error("cannot find the {0} directory for this user (is HOME set?)")]
    NoPlatformDir(&'static str),
}

/// Resolved file locations for one instance.
#[derive(Clone, Debug)]
pub struct Paths {
    pub instance: Instance,
    /// `<data_dir>/chatgpt-cli[-<instance>]`, kept 0700.
    pub data_dir: PathBuf,
    /// Kept 0700; holds the daemon's socket (0600), pid file and lock.
    /// `<runtime_dir>/chatgpt-cli[-<instance>]` where the platform has a
    /// runtime directory (Linux), otherwise `<data_dir>/run`.
    pub run_dir: PathBuf,
}

impl Paths {
    pub fn resolve(instance: Instance) -> Result<Self, PathsError> {
        let data_base = dirs::data_dir().ok_or(PathsError::NoPlatformDir("data"))?;
        Ok(Self::under(
            instance,
            &data_base,
            dirs::runtime_dir().as_deref(),
        ))
    }

    pub fn under(instance: Instance, data_base: &Path, runtime_base: Option<&Path>) -> Self {
        let data_dir = data_base.join(instance.dir_name());
        let run_dir = match runtime_base {
            Some(runtime) => runtime.join(instance.dir_name()),
            None => data_dir.join("run"),
        };
        Self {
            data_dir,
            run_dir,
            instance,
        }
    }

    /// The daemon's Unix socket.
    pub fn socket_path(&self) -> PathBuf {
        self.run_dir.join("daemon.sock")
    }

    /// Holds the running daemon's PID, written by the daemon itself.
    pub fn pid_file(&self) -> PathBuf {
        self.run_dir.join("daemon.pid")
    }

    /// The running daemon holds an exclusive lock on this file for its whole
    /// life, so "is a daemon running?" never depends on a reused PID.
    pub fn daemon_lock_file(&self) -> PathBuf {
        self.run_dir.join("daemon.lock")
    }

    /// The index: SQLite in WAL mode, owned by the daemon. Not `index.db`:
    /// on Linux the data directory is the TS CLI's `~/.local/share`.
    pub fn database_file(&self) -> PathBuf {
        self.data_dir.join("chatgpt.db")
    }

    /// The daemon's daily-rotated log files, `daemon.log.YYYY-MM-DD`.
    pub fn log_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    /// The daemon's stderr: anything it prints before its log is open,
    /// such as a panic during startup.
    pub fn daemon_stderr_file(&self) -> PathBuf {
        self.log_dir().join("daemon.stderr")
    }
}

fn current_exe_is_cargo_target_build() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    path_has_cargo_target_profile_ancestor(&exe) || in_cargo_output_dir(&exe)
}

/// Whether `exe` sits in a Cargo profile directory, whatever the target
/// directory is called (`CARGO_TARGET_DIR=~/.cache/cargo`): Cargo keeps
/// its `.fingerprint` directory next to the binaries it builds (and one
/// level up from `deps/` and `examples/`). An installed binary
/// (`~/.local/bin`, Homebrew, `cargo install`) has none.
fn in_cargo_output_dir(exe: &Path) -> bool {
    exe.ancestors()
        .skip(1)
        .take(2)
        .any(|dir| dir.join(".fingerprint").is_dir())
}

fn path_has_cargo_target_profile_ancestor(path: &Path) -> bool {
    path.ancestors().skip(1).any(|dir| {
        let is_profile = dir
            .file_name()
            .is_some_and(|name| name == OsStr::new("debug") || name == OsStr::new("release"));
        is_profile && has_target_parent(dir)
    })
}

// `target/release` or, when cross-compiling, `target/<triple>/release`.
fn has_target_parent(profile_dir: &Path) -> bool {
    profile_dir
        .ancestors()
        .skip(1)
        .take(2)
        .any(|dir| dir.file_name().is_some_and(is_cargo_target_dir_name))
}

fn is_cargo_target_dir_name(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    name == "target" || name.starts_with("target-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_builds_default_to_the_dev_instance() {
        let instance = Instance::resolve(None, None, true).expect("valid");
        assert_eq!(instance.dir_name(), "chatgpt-cli-dev");
    }

    #[test]
    fn installed_builds_use_the_plain_name() {
        let instance = Instance::resolve(None, None, false).expect("valid");
        assert_eq!(instance, Instance::Default);
        assert_eq!(instance.dir_name(), "chatgpt-cli");
    }

    #[test]
    fn flag_beats_env_and_env_beats_build_kind() {
        let from_flag = Instance::resolve(Some("a"), Some("b"), true).expect("valid");
        assert_eq!(from_flag.dir_name(), "chatgpt-cli-a");
        let from_env = Instance::resolve(None, Some("b"), false).expect("valid");
        assert_eq!(from_env.dir_name(), "chatgpt-cli-b");
    }

    #[test]
    fn instance_names_cannot_escape_the_data_dir() {
        for bad in ["../x", "a/b", "a b", "."] {
            assert!(Instance::resolve(Some(bad), None, false).is_err(), "{bad}");
        }
    }

    #[test]
    fn default_selects_the_installed_instance_from_any_build() {
        let instance = Instance::resolve(Some("default"), None, true).expect("valid");
        assert_eq!(instance, Instance::Default);
        let round_trip = Instance::resolve(Some(instance.label()), None, true).expect("valid");
        assert_eq!(round_trip, instance);
    }

    #[test]
    fn detects_cargo_target_profiles() {
        assert!(path_has_cargo_target_profile_ancestor(Path::new(
            "/repo/target/debug/chatgpt"
        )));
        assert!(path_has_cargo_target_profile_ancestor(Path::new(
            "/repo/target/aarch64-apple-darwin/release/chatgpt"
        )));
        assert!(!path_has_cargo_target_profile_ancestor(Path::new(
            "/home/bk/.local/bin/chatgpt"
        )));
    }

    #[test]
    fn detects_a_build_under_a_custom_target_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let profile = dir.path().join("cargo-cache/release");
        std::fs::create_dir_all(profile.join(".fingerprint")).expect("mkdir");
        std::fs::create_dir_all(profile.join("deps")).expect("mkdir");
        assert!(!path_has_cargo_target_profile_ancestor(
            &profile.join("chatgpt")
        ));
        assert!(in_cargo_output_dir(&profile.join("chatgpt")));
        assert!(in_cargo_output_dir(&profile.join("deps/chatgpt-0123")));
        let installed = dir.path().join("bin");
        std::fs::create_dir_all(&installed).expect("mkdir");
        assert!(!in_cargo_output_dir(&installed.join("chatgpt")));
    }

    #[test]
    fn the_index_never_shares_the_ts_clis_file() {
        let paths = Paths::under(Instance::Default, Path::new("/data"), None);
        assert_eq!(
            paths.database_file(),
            Path::new("/data/chatgpt-cli/chatgpt.db")
        );
        assert_eq!(
            paths.socket_path(),
            Path::new("/data/chatgpt-cli/run/daemon.sock")
        );
        let linux = Paths::under(
            Instance::Named("dev".into()),
            Path::new("/data"),
            Some(Path::new("/run/user/1000")),
        );
        assert_eq!(
            linux.socket_path(),
            Path::new("/run/user/1000/chatgpt-cli-dev/daemon.sock")
        );
    }
}
