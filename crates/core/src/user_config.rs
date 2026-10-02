//! The user config, `~/.config/chatgpt-cli/config.json`: model API keys
//! (`chatgpt configure`) and the daemon's `auto_jev` switch. Read and
//! written as the TS CLI's `src/auth/config.ts` @ 1b8c950 does: the same
//! path, the same checks and messages, written whole to a 0600 file in a
//! 0700 directory through a rename.
//!
//! `auto_jev` is this port's own: the TS CLI ignores the key, and `chatgpt
//! configure` here keeps it when it rewrites the file (the TS CLI's would
//! drop it).
//!
//! Keys are never logged or shown; [`UserConfig`]'s `Debug` redacts them.

use std::fmt;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// A model provider `chatgpt configure` stores a key for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Jev,
    OpenAi,
    Anthropic,
}

impl Provider {
    /// In the order `readConfig` reads them and `configure` lists them.
    pub const ALL: [Self; 3] = [Self::Jev, Self::OpenAi, Self::Anthropic];

    /// The name in the config file and on the command line.
    pub fn name(self) -> &'static str {
        match self {
            Self::Jev => "jev",
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
        }
    }

    /// The environment variable that takes precedence over the file.
    pub fn env_name(self) -> &'static str {
        match self {
            Self::Jev => "TYPESAFE_API_KEY",
            Self::OpenAi => "OPENAI_API_KEY",
            Self::Anthropic => "ANTHROPIC_API_KEY",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|provider| provider.name() == name)
    }
}

/// The config as read: each key trimmed, a blank one absent.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct UserConfig {
    keys: [Option<String>; 3],
    /// `None` when the file doesn't say.
    pub auto_jev: Option<bool>,
}

impl fmt::Debug for UserConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let configured: Vec<&str> = Provider::ALL
            .into_iter()
            .filter(|provider| self.key(*provider).is_some())
            .map(Provider::name)
            .collect();
        f.debug_struct("UserConfig")
            .field("configured", &configured)
            .field("auto_jev", &self.auto_jev)
            .finish()
    }
}

impl UserConfig {
    pub fn key(&self, provider: Provider) -> Option<&str> {
        self.keys[provider as usize].as_deref()
    }

    fn set(&mut self, provider: Provider, key: Option<String>) {
        self.keys[provider as usize] = key;
    }
}

/// `configPath()`: `$XDG_CONFIG_HOME/chatgpt-cli/config.json`, else under
/// `~/.config`.
pub fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")))?;
    Some(base.join("chatgpt-cli").join("config.json"))
}

/// `readConfig(path)`: a missing file is an empty config; a malformed one
/// is an error naming the file (never quoting it).
pub fn read_config(path: &Path) -> Result<UserConfig, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(UserConfig::default());
        }
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let shown = path.display();
    let parsed: Value =
        serde_json::from_str(&raw).map_err(|_| format!("Invalid JSON in {shown}."))?;
    let Value::Object(object) = parsed else {
        return Err(format!("Invalid config in {shown}."));
    };
    let mut config = UserConfig::default();
    for provider in Provider::ALL {
        match object.get(provider.name()) {
            None => {}
            Some(Value::String(value)) => {
                let trimmed = crate::js::trim(value);
                if !trimmed.is_empty() {
                    config.set(provider, Some(trimmed.to_owned()));
                }
            }
            Some(_) => return Err(format!("Invalid {} key in {shown}.", provider.name())),
        }
    }
    config.auto_jev = match object.get("auto_jev") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(on)) => Some(*on),
        Some(_) => return Err(format!("Invalid auto_jev in {shown}: use true or false.")),
    };
    Ok(config)
}

/// `setConfigKey`: store `key` for `provider` (`None` removes it), keeping
/// the other keys and `auto_jev`.
pub fn set_config_key(path: &Path, provider: Provider, key: Option<&str>) -> Result<(), String> {
    let mut config = read_config(path)?;
    // `readConfig` rebuilds the object in the providers' order, and a key
    // set anew goes last, as JS object insertion does.
    let mut order: Vec<Provider> = Provider::ALL
        .into_iter()
        .filter(|name| config.key(*name).is_some())
        .collect();
    if !order.contains(&provider) {
        order.push(provider);
    }
    config.set(provider, key.map(|key| crate::js::trim(key).to_owned()));
    let mut object = Map::new();
    for name in order {
        if let Some(value) = config.key(name) {
            object.insert(name.name().to_owned(), Value::String(value.to_owned()));
        }
    }
    if let Some(on) = config.auto_jev {
        object.insert("auto_jev".to_owned(), Value::Bool(on));
    }
    let text = serde_json::to_string_pretty(&Value::Object(object))
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    write_private(path, &format!("{text}\n"))
}

/// Write `text` to `path` through a 0600 temporary file and a rename, in a
/// 0700 directory, as `setConfigKey` does.
fn write_private(path: &Path, text: &str) -> Result<(), String> {
    let describe = |at: &Path, error: std::io::Error| format!("{}: {error}", at.display());
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|error| describe(dir, error))?;
    let temporary = dir.join(format!(
        ".config-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos())
    ));
    let written = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
    })();
    let _ = std::fs::remove_file(&temporary);
    written.map_err(|error| describe(path, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_is_read_as_the_ts_cli_reads_it() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("config.json");
        assert_eq!(read_config(&path), Ok(UserConfig::default()));
        std::fs::write(
            &path,
            r#"{"jev":"  key-1  ","openai":"   ","auto_jev":false}"#,
        )
        .expect("write");
        let config = read_config(&path).expect("config");
        assert_eq!(config.key(Provider::Jev), Some("key-1"));
        assert_eq!(config.key(Provider::OpenAi), None);
        assert_eq!(config.auto_jev, Some(false));
        let shown = format!("{config:?}");
        assert!(!shown.contains("key-1"), "{shown}");
        for (raw, error) in [
            (r#"{"anthropic":5}"#, "Invalid anthropic key in"),
            ("[]", "Invalid config in"),
            ("{", "Invalid JSON in"),
            (r#"{"auto_jev":"no"}"#, "Invalid auto_jev in"),
        ] {
            std::fs::write(&path, raw).expect("write");
            let message = read_config(&path).expect_err("refused");
            assert!(message.starts_with(error), "{message}");
            assert!(message.contains(&path.display().to_string()), "{message}");
        }
    }

    #[test]
    fn keys_are_written_privately_in_the_ts_clis_layout() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("sub/config.json");
        set_config_key(&path, Provider::Anthropic, Some(" a-key ")).expect("set");
        set_config_key(&path, Provider::Jev, Some("j-key")).expect("set");
        // A key set anew goes last; existing ones keep the providers' order.
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "{\n  \"anthropic\": \"a-key\",\n  \"jev\": \"j-key\"\n}\n"
        );
        set_config_key(&path, Provider::OpenAi, Some("o-key")).expect("set");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "{\n  \"jev\": \"j-key\",\n  \"anthropic\": \"a-key\",\n  \"openai\": \"o-key\"\n}\n"
        );
        let mode = |at: &Path| std::fs::metadata(at).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().expect("dir")), 0o700);
        // auto_jev survives a rewrite; a removed key goes; the rest are
        // read back in the providers' order.
        let raw = std::fs::read_to_string(&path).expect("read");
        std::fs::write(&path, raw.replace("\n}", ",\n  \"auto_jev\": false\n}")).expect("write");
        set_config_key(&path, Provider::Jev, None).expect("remove");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "{\n  \"openai\": \"o-key\",\n  \"anthropic\": \"a-key\",\n  \"auto_jev\": false\n}\n"
        );
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().expect("dir"))
            .expect("dir")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }
}
