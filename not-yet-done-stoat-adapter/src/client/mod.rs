//! Stateless REST client for the Stoat/Revolt HTTP API.
//!
//! Carries the `X-Session-Token` on every request. Responsible for
//! login (see [`auth`]), server self-discovery (see [`discovery`]), and
//! — from Phase 1 on — message history. The gateway owns the WebSocket;
//! this client owns the pull-model request/response side.

pub mod auth;
pub mod discovery;
pub mod members;
pub mod messages;
pub mod structure;

use std::sync::Arc;

use reqwest::Client as HttpClient;
use reqwest::header::{ACCEPT, HeaderMap, HeaderName, HeaderValue};
use serde::Deserialize;

use not_yet_done_content::http_log;

pub use auth::{StoatSession, perform_login};
pub use discovery::{RootInfo, fetch_root_info};
pub use messages::{MessageView, ulid_timestamp_ms};

const SESSION_TOKEN_HEADER: &str = "x-session-token";

/// Identity returned by `GET /api/users/@me` — used to validate a
/// restored session token.
#[derive(Deserialize, Debug, Clone)]
pub struct MeData {
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(default)]
    pub username: String,
}

pub struct StoatClient {
    base_url: String,
    http: HttpClient,
    token: String,
    #[allow(dead_code)]
    user_id: String,
}

impl StoatClient {
    /// Build a client from a stored session. No HTTP is performed here.
    pub fn from_session(base_url: &str, session: StoatSession) -> Result<Arc<Self>, String> {
        let http = HttpClient::builder()
            .build()
            .map_err(|e| format!("build http client: {e}"))?;
        Ok(Arc::new(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http,
            token: session.token,
            user_id: session.user_id,
        }))
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The session token, for handing to the gateway.
    pub fn token(&self) -> &str {
        &self.token
    }

    fn auth_headers(&self) -> Result<HeaderMap, String> {
        let mut h = HeaderMap::new();
        h.insert(
            HeaderName::from_static(SESSION_TOKEN_HEADER),
            HeaderValue::from_str(&self.token).map_err(|e| format!("session token header: {e}"))?,
        );
        h.insert(ACCEPT, HeaderValue::from_static("application/json"));
        Ok(h)
    }

    /// `GET /api/users/@me`. Validates the session token; a 401 here is
    /// the auth bridge's signal to drop the cached session and re-login.
    pub async fn me(&self) -> Result<MeData, String> {
        let url = format!("{}/api/users/@me", self.base_url);
        http_log::log_request("GET", &url);
        let resp = self
            .http
            .get(&url)
            .headers(self.auth_headers()?)
            .send()
            .await
            .map_err(|e| http_log::network_error("GET", &url, e))?;
        let resp = http_log::check_status("GET", &url, resp).await?;
        resp.json::<MeData>()
            .await
            .map_err(|e| format!("parse /users/@me: {e}"))
    }

    /// Discover the WebSocket URL via `GET /api/`.
    pub async fn discover_ws_url(&self) -> Result<String, String> {
        let info = fetch_root_info(&self.http, &self.base_url).await?;
        Ok(info.ws)
    }
}
