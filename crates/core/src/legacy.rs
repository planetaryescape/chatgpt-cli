//! Where the TS CLI keeps its index, which the daemon imports judgments,
//! titles, summaries and transcripts from while the bridge exists (D2).

use std::path::{Path, PathBuf};

/// Overrides the TS index's location, for tests.
pub const LEGACY_DB_ENV: &str = "CHATGPT_LEGACY_DB";

/// The TS CLI's `index.db`: `$CHATGPT_LEGACY_DB`, else
/// `$XDG_DATA_HOME/chatgpt-cli/index.db`, else
/// `~/.local/share/chatgpt-cli/index.db`, as `src/index/store.ts` resolves it.
pub fn legacy_index_path() -> Option<PathBuf> {
    resolve(
        std::env::var_os(LEGACY_DB_ENV).as_deref().map(Path::new),
        std::env::var_os("XDG_DATA_HOME").as_deref().map(Path::new),
        dirs::home_dir().as_deref(),
    )
}

fn resolve(
    override_path: Option<&Path>,
    xdg_data_home: Option<&Path>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(path) = override_path.filter(|path| !path.as_os_str().is_empty()) {
        return Some(path.to_path_buf());
    }
    // The TS CLI uses XDG_DATA_HOME whenever it's set, even when empty or
    // relative (`process.env.XDG_DATA_HOME ?? …`).
    let base = match xdg_data_home {
        Some(xdg) => xdg.to_path_buf(),
        None => home?.join(".local/share"),
    };
    Some(base.join("chatgpt-cli").join("index.db"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_the_ts_clis_rule() {
        let home = Some(Path::new("/home/bk"));
        assert_eq!(
            resolve(None, None, home),
            Some(PathBuf::from("/home/bk/.local/share/chatgpt-cli/index.db"))
        );
        assert_eq!(
            resolve(None, Some(Path::new("/xdg")), home),
            Some(PathBuf::from("/xdg/chatgpt-cli/index.db"))
        );
        assert_eq!(
            resolve(
                Some(Path::new("/fixture.db")),
                Some(Path::new("/xdg")),
                home
            ),
            Some(PathBuf::from("/fixture.db"))
        );
    }
}
