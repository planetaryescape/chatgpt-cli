//! The Jev API key, found as the TS CLI's `requireSecret("TYPESAFE_API_KEY")`
//! (`src/auth/secrets.ts` and `config.ts` @ 1b8c950) finds it: the
//! environment, then the user config. The environment is the requesting
//! client's (it passes its `TYPESAFE_API_KEY` on), not the daemon's, which
//! is whichever client happened to start it. The config file is read on
//! every call, so `chatgpt configure jev` takes effect at once. The key is
//! never stored, logged or echoed.

use std::path::PathBuf;

use chatgpt_protocol::Secret;
use serde_json::Value;
use typesafe_client::ApiKey;

const MISSING: &str =
    "TYPESAFE_API_KEY is not configured. Run `chatgpt configure jev` or set TYPESAFE_API_KEY.";

/// `configPath()`: `$XDG_CONFIG_HOME/chatgpt-cli/config.json`, else under
/// `~/.config`.
fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")))?;
    Some(base.join("chatgpt-cli").join("config.json"))
}

/// The key, or the TS CLI's message for why there's none.
pub fn api_key(forwarded: Option<&Secret>) -> Result<ApiKey, String> {
    if let Some(key) = forwarded.and_then(|secret| ApiKey::new(secret.expose())) {
        return Ok(key);
    }
    let Some(path) = config_path() else {
        return Err(MISSING.to_owned());
    };
    match read_config_key(&path)? {
        Some(key) => Ok(key),
        None => Err(MISSING.to_owned()),
    }
}

/// `readConfig(path).jev`: a missing file has no key; a malformed one is
/// an error naming the file.
fn read_config_key(path: &std::path::Path) -> Result<Option<ApiKey>, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let shown = path.display();
    let parsed: Value =
        serde_json::from_str(&raw).map_err(|_| format!("Invalid JSON in {shown}."))?;
    let Some(object) = parsed.as_object() else {
        return Err(format!("Invalid config in {shown}."));
    };
    let mut jev = None;
    for name in ["jev", "openai", "anthropic"] {
        match object.get(name) {
            None => {}
            Some(Value::String(value)) if name == "jev" => jev = ApiKey::new(value),
            Some(Value::String(_)) => {}
            Some(_) => return Err(format!("Invalid {name} key in {shown}.")),
        }
    }
    Ok(jev)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_config_file_is_read_as_the_ts_cli_reads_it() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("config.json");
        assert!(matches!(read_config_key(&path), Ok(None)));
        std::fs::write(&path, r#"{"jev":"  key-1  ","openai":"x"}"#).expect("write");
        assert!(matches!(read_config_key(&path), Ok(Some(_))));
        std::fs::write(&path, r#"{"jev":"   "}"#).expect("write");
        assert!(matches!(read_config_key(&path), Ok(None)));
        std::fs::write(&path, r#"{"anthropic":5}"#).expect("write");
        assert_eq!(
            read_config_key(&path).err(),
            Some(format!("Invalid anthropic key in {}.", path.display()))
        );
        std::fs::write(&path, "[]").expect("write");
        assert_eq!(
            read_config_key(&path).err(),
            Some(format!("Invalid config in {}.", path.display()))
        );
        std::fs::write(&path, "{").expect("write");
        assert_eq!(
            read_config_key(&path).err(),
            Some(format!("Invalid JSON in {}.", path.display()))
        );
    }

    #[test]
    fn a_forwarded_key_comes_first_and_a_blank_one_counts_as_none() {
        assert!(api_key(Some(&Secret::new(" k ".into()))).is_ok());
    }
}
