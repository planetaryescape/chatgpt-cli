//! Model keys and tools, found as the TS CLI finds them
//! (`findSecret`/`requireSecret` in `src/auth/secrets.ts` and `Bun.which`
//! @ 1b8c950): a key from the environment, else from the user config; a
//! tool on `PATH`.
//!
//! The environment and `PATH` are the requesting client's, which it passes
//! on with the request ([`ModelAccess`]): the daemon's own are whichever
//! client happened to start it, or launchd's. The config file is read on
//! every lookup, so `chatgpt configure` takes effect at once. The
//! background Jev ([`Access::config_only`]) uses the config file alone.
//! Keys are never stored, logged or echoed.

use std::path::PathBuf;

use chatgpt_core::user_config::{self, Provider, UserConfig};
use chatgpt_protocol::ModelAccess;

/// In debug builds, the APIs to use instead of the real ones (tests).
const TYPESAFE_URL_ENV: &str = "TYPESAFE_BASE_URL";
const OPENAI_URL_ENV: &str = "CHATGPT_TEST_OPENAI_URL";
const ANTHROPIC_URL_ENV: &str = "CHATGPT_TEST_ANTHROPIC_URL";

/// Where a run's keys and tools come from.
#[derive(Clone, Debug, Default)]
pub struct Access {
    forwarded: ModelAccess,
    /// The background Jev: never a forwarded key.
    config_only: bool,
}

/// A debug-build override of `default`, from `env`.
fn base_url(env: &str, default: &str) -> String {
    std::env::var(env)
        .ok()
        .filter(|base| cfg!(debug_assertions) && !base.is_empty())
        .unwrap_or_else(|| default.to_owned())
}

impl Access {
    pub fn for_client(forwarded: ModelAccess) -> Self {
        Self {
            forwarded,
            config_only: false,
        }
    }

    /// The user config's keys only, and the daemon's own `PATH`.
    pub fn config_only() -> Self {
        Self {
            forwarded: ModelAccess::default(),
            config_only: true,
        }
    }

    pub fn config(&self) -> Result<UserConfig, String> {
        match user_config::config_path() {
            Some(path) => user_config::read_config(&path),
            None => Ok(UserConfig::default()),
        }
    }

    fn forwarded(&self, provider: Provider) -> Option<&str> {
        if self.config_only {
            return None;
        }
        let secret = match provider {
            Provider::Jev => self.forwarded.typesafe.as_ref(),
            Provider::OpenAi => self.forwarded.openai.as_ref(),
            Provider::Anthropic => self.forwarded.anthropic.as_ref(),
        }?;
        Some(chatgpt_core::js::trim(secret.expose())).filter(|key| !key.is_empty())
    }

    /// `findSecret`: the client's environment, then the config file (read
    /// only when the environment has none, as `||` does).
    pub fn find(&self, provider: Provider) -> Result<Option<String>, String> {
        if let Some(key) = self.forwarded(provider) {
            return Ok(Some(key.to_owned()));
        }
        Ok(self.config()?.key(provider).map(str::to_owned))
    }

    /// `requireSecret`: the key, or the TS CLI's message for why there's
    /// none.
    fn require(&self, provider: Provider) -> Result<String, String> {
        self.find(provider)?.ok_or_else(|| {
            format!(
                "{} is not configured. Run `chatgpt configure {}` or set {}.",
                provider.env_name(),
                provider.name(),
                provider.env_name()
            )
        })
    }

    /// The Jev client (`new TypeSafeClient({ apiKey: requireSecret(…) })`).
    pub fn jev(&self) -> Result<typesafe_client::Client, String> {
        let key = typesafe_client::ApiKey::new(&self.require(Provider::Jev)?)
            .map_err(|error| error.to_string())?
            .ok_or("TYPESAFE_API_KEY is not configured. Run `chatgpt configure jev` or set TYPESAFE_API_KEY.")?;
        let base = base_url(TYPESAFE_URL_ENV, typesafe_client::DEFAULT_BASE_URL);
        typesafe_client::Client::new(&base, key, typesafe_client::RetryPolicy::default())
            .map_err(|error| error.to_string())
    }

    /// OpenAI's API, when a key is configured.
    pub fn openai(&self) -> Result<Option<model_api::OpenAi>, String> {
        let Some(key) = self.find(Provider::OpenAi)? else {
            return Ok(None);
        };
        let Some(key) = model_api::ApiKey::new(&key, "OpenAI").map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let base = base_url(OPENAI_URL_ENV, model_api::OPENAI_BASE_URL);
        model_api::OpenAi::new(&base, key)
            .map(Some)
            .map_err(|error| error.to_string())
    }

    /// Anthropic's API, when a key is configured.
    pub fn anthropic(&self) -> Result<Option<model_api::Anthropic>, String> {
        let Some(key) = self.find(Provider::Anthropic)? else {
            return Ok(None);
        };
        let Some(key) = model_api::ApiKey::new(&key, "Anthropic").map_err(|e| e.to_string())?
        else {
            return Ok(None);
        };
        let base = base_url(ANTHROPIC_URL_ENV, model_api::ANTHROPIC_BASE_URL);
        model_api::Anthropic::new(&base, key)
            .map(Some)
            .map_err(|error| error.to_string())
    }

    /// The `PATH` tools are looked for on and run with: the client's, else
    /// the daemon's.
    pub fn path_var(&self) -> Option<String> {
        self.forwarded
            .path
            .clone()
            .filter(|path| !self.config_only && !path.is_empty())
            .or_else(|| std::env::var("PATH").ok())
    }

    /// `Bun.which(name)`: the first executable `name` on the `PATH`.
    pub fn which(&self, name: &str) -> Option<PathBuf> {
        use std::os::unix::fs::PermissionsExt;
        let path = self.path_var()?;
        std::env::split_paths(&path)
            .filter(|dir| !dir.as_os_str().is_empty())
            .map(|dir| dir.join(name))
            .find(|candidate| {
                std::fs::metadata(candidate)
                    .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            })
    }
}

#[cfg(test)]
mod tests {
    use chatgpt_protocol::Secret;

    use super::*;

    #[test]
    fn a_forwarded_key_comes_first_and_the_background_never_takes_one() {
        let access = Access::for_client(ModelAccess {
            openai: Some(Secret::new("  sk-1 ".into())),
            anthropic: Some(Secret::new("   ".into())),
            ..ModelAccess::default()
        });
        assert_eq!(access.forwarded(Provider::OpenAi), Some("sk-1"));
        assert_eq!(access.forwarded(Provider::Anthropic), None);
        let background = Access {
            config_only: true,
            ..access
        };
        assert_eq!(background.forwarded(Provider::OpenAi), None);
    }

    #[test]
    fn a_broken_jev_key_is_refused_unseen() {
        let access = Access::for_client(ModelAccess {
            typesafe: Some(Secret::new("SENTINEL\nKEY".into())),
            ..ModelAccess::default()
        });
        let error = access.jev().expect_err("refused");
        assert!(!error.contains("SENTINEL"), "{error}");
    }

    #[test]
    fn tools_are_found_on_the_clients_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("dir");
        let tool = dir.path().join("codex");
        std::fs::write(&tool, "#!/bin/sh\n").expect("write");
        let not_executable = dir.path().join("claude");
        std::fs::write(&not_executable, "").expect("write");
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let access = Access::for_client(ModelAccess {
            path: Some(format!("/nonexistent:{}", dir.path().display())),
            ..ModelAccess::default()
        });
        assert_eq!(access.which("codex"), Some(tool));
        assert_eq!(access.which("claude"), None);
    }
}
