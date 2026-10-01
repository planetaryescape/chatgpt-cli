//! Exchanges the browser session cookie for an API access token.
//!
//! Ported from `token()` in the TS CLI's `src/api/client.ts` @ 1b8c950.

use serde::Deserialize;

use crate::http::{HttpClient, HttpError, Secret};

pub const SESSION_PATH: &str = "/api/auth/session";

/// A bearer token for `/backend-api`. It lives in memory only; Debug never prints it.
#[derive(Clone, Debug)]
pub struct AccessToken(Secret);

impl AccessToken {
    pub fn expose(&self) -> &str {
        self.0.expose()
    }

    /// The `authorization` header value.
    pub fn bearer(&self) -> Secret {
        Secret::new(format!("Bearer {}", self.0.expose()))
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
    #[error("{SESSION_PATH} returned something other than session JSON: {0}")]
    Malformed(serde_json::Error),
}

#[derive(Deserialize)]
struct SessionBody {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
}

/// `source_name` names the browser session in the expiry message, e.g. `dia profile "Default"`.
pub async fn exchange_session(
    http: &HttpClient,
    source_name: &str,
) -> Result<AccessToken, SessionError> {
    let body = http.get(SESSION_PATH, &[]).await?;
    let session: SessionBody = serde_json::from_str(&body).map_err(SessionError::Malformed)?;
    session
        .access_token
        .filter(|token| !token.is_empty())
        .map(|token| AccessToken(Secret::new(token)))
        .ok_or_else(|| SessionError::Expired {
            source_name: source_name.to_owned(),
        })
}
