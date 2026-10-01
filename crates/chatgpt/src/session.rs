//! Exchanges the browser session cookie for an API access token.
//!
//! Ported from `token()` in the TS CLI's `src/api/client.ts` @ 1b8c950.

use serde::Deserialize;

use crate::http::{HttpClient, HttpError, Secret};

pub const SESSION_PATH: &str = "/api/auth/session";

/// A bearer token for `/backend-api`. It lives in memory only; Debug never prints it.
#[derive(Clone, Debug)]
pub struct AccessToken {
    token: Secret,
    account: Option<String>,
}

impl AccessToken {
    pub fn expose(&self) -> &str {
        self.token.expose()
    }

    /// The `authorization` header value.
    pub fn bearer(&self) -> Secret {
        Secret::new(format!("Bearer {}", self.token.expose()))
    }

    /// The ChatGPT account the session belongs to (`user.id`), when the
    /// session says.
    pub fn account(&self) -> Option<&str> {
        self.account.as_deref()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Http(#[from] HttpError),
    #[error(
        "ChatGPT session in {source_name} has expired. Open chatgpt.com there to refresh it, then retry."
    )]
    Expired { source_name: String },
    /// Only the parse position: serde's message would echo string values from
    /// a body that can hold tokens.
    #[error(
        "{SESSION_PATH} returned something other than session JSON ({category:?} error at line {line}, column {column})"
    )]
    Malformed {
        category: serde_json::error::Category,
        line: usize,
        column: usize,
    },
}

#[derive(Deserialize)]
struct SessionBody {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
    user: Option<SessionUser>,
}

#[derive(Deserialize)]
struct SessionUser {
    id: Option<String>,
}

/// `source_name` names the browser session in the expiry message, e.g. `dia profile "Default"`.
pub async fn exchange_session(
    http: &HttpClient,
    source_name: &str,
) -> Result<AccessToken, SessionError> {
    let body = http.get(SESSION_PATH, &[]).await?;
    let session: SessionBody =
        serde_json::from_str(&body).map_err(|error| SessionError::Malformed {
            category: error.classify(),
            line: error.line(),
            column: error.column(),
        })?;
    let account = session.user.and_then(|user| user.id);
    session
        .access_token
        .filter(|token| !token.is_empty())
        .map(|token| AccessToken {
            token: Secret::new(token),
            account,
        })
        .ok_or_else(|| SessionError::Expired {
            source_name: source_name.to_owned(),
        })
}
