//! Where the TS CLI and bun are. Commands the Rust CLI hasn't ported run as
//! `bun <cli.ts> <args…>` (the bridge), and the daemon runs the TS `sync` so
//! bridged commands see fresh data. Always by path, never by looking up
//! `chatgpt` on PATH, which may be this binary.

use std::path::{Path, PathBuf};

/// The TS CLI's entry point, `…/src/cli.ts`.
pub const TS_CLI_ENV: &str = "CHATGPT_TS_CLI";
/// The program that runs it. For tests, which run a shell script instead.
pub const BUN_ENV: &str = "CHATGPT_BUN";
/// Set on every process the bridge or the daemon starts from the TS CLI. A
/// Rust chatgpt that finds it set refuses to bridge again, so a
/// misconfigured `CHATGPT_TS_CLI` that points back here can't loop.
pub const BRIDGED_ENV: &str = "CHATGPT_BRIDGED";

/// Where `bun add -g` / `bun link` put the TS CLI.
const BUN_GLOBAL_CLI: &str = ".bun/install/global/node_modules/chatgpt-cli/src/cli.ts";
const BUN_HOME_BIN: &str = ".bun/bin/bun";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsCli {
    /// The program to run: bun.
    pub bun: PathBuf,
    /// `cli.ts`, symlinks resolved.
    pub entry: PathBuf,
}

impl TsCli {
    /// The TS sources' root (`src/`), whose classification constants the
    /// daemon reads.
    pub fn source_dir(&self) -> Option<&Path> {
        self.entry.parent()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TsCliError {
    #[error("{TS_CLI_ENV} is {0}, which doesn't exist")]
    MissingOverride(PathBuf),
    #[error(
        "this command still runs in the TS chatgpt CLI, which isn't installed (looked for ~/{BUN_GLOBAL_CLI}). Install it with `bun link` in a chatgpt-cli checkout, or set {TS_CLI_ENV} to its src/cli.ts"
    )]
    NotInstalled,
    #[error("the TS chatgpt CLI needs bun, which isn't on PATH or at ~/{BUN_HOME_BIN}")]
    NoBun,
}

/// Find the TS CLI and bun: `$CHATGPT_TS_CLI`, else bun's global install.
pub fn locate() -> Result<TsCli, TsCliError> {
    let home = dirs::home_dir();
    let entry = match std::env::var_os(TS_CLI_ENV).filter(|value| !value.is_empty()) {
        Some(path) => {
            let path = PathBuf::from(path);
            std::fs::canonicalize(&path).map_err(|_| TsCliError::MissingOverride(path))?
        }
        None => home
            .as_deref()
            .map(|home| home.join(BUN_GLOBAL_CLI))
            .and_then(|path| std::fs::canonicalize(path).ok())
            .ok_or(TsCliError::NotInstalled)?,
    };
    let bun = find_bun(
        std::env::var_os(BUN_ENV).as_deref().map(Path::new),
        std::env::var_os("PATH").as_deref(),
        home.as_deref(),
    )
    .ok_or(TsCliError::NoBun)?;
    Ok(TsCli { bun, entry })
}

fn find_bun(
    override_path: Option<&Path>,
    path_var: Option<&std::ffi::OsStr>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(path) = override_path.filter(|path| !path.as_os_str().is_empty()) {
        return Some(path.to_path_buf());
    }
    let on_path = path_var.into_iter().flat_map(std::env::split_paths);
    on_path
        .map(|dir| dir.join("bun"))
        .chain(home.map(|home| home.join(BUN_HOME_BIN)))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bun_comes_from_the_override_then_path_then_home() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).expect("mkdir");
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".bun/bin")).expect("mkdir");
        std::fs::write(home.join(BUN_HOME_BIN), "").expect("write");

        assert_eq!(
            find_bun(Some(Path::new("/bin/sh")), None, Some(&home)),
            Some(PathBuf::from("/bin/sh"))
        );
        let path_var = std::env::join_paths([&bin]).expect("join");
        assert_eq!(
            find_bun(None, Some(&path_var), Some(&home)),
            Some(home.join(BUN_HOME_BIN))
        );
        std::fs::write(bin.join("bun"), "").expect("write");
        assert_eq!(
            find_bun(None, Some(&path_var), Some(&home)),
            Some(bin.join("bun"))
        );
        assert_eq!(find_bun(None, None, Some(Path::new("/nowhere"))), None);
    }
}
