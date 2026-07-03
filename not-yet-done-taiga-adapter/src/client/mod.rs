//! Taiga REST client. Auth via username/password → JWT.
//!
//! The login round-trip and the constructor that takes a stored session
//! are split: [`perform_login`] does only the HTTP exchange and returns
//! a [`TaigaSession`] (suitable for serialising into the auth
//! orchestrator's session blob); [`TaigaClient::from_session`] turns
//! such a blob back into a live client without an additional round-trip.

use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use not_yet_done_content::http_log;
use reqwest::{Client as HttpClient, header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue}};
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;
use uuid::Uuid;

pub mod query;
mod history;
mod actions;
mod notifications;
mod project_meta;
mod edit;
mod create;
mod convert;

pub use query::{
    ItemSummary, ItemType, ParsedTaigaQuery, QuerySpec, apply_sort as apply_query_sort,
    default_sort, parse_query_yaml, parse_taiga_query, run_queries, sortable_column_keys,
};
pub use history::{TaigaComment, fetch_comments};
pub use actions::{
    TaigaAttachment, delete_attachment, download_attachment, edit_comment, list_attachments,
    toggle_watch, upload_attachment, upload_attachment_bytes,
};
pub use convert::{
    delete_item, fetch_id_name_map, fetch_raw_detail, promote_issue_to_us, userstory_id_by_ref,
};
pub use project_meta::{TaigaMember, TaigaStatus};
pub use edit::{
    EditFields, ItemPatch, PatchOutcome, add_comment, delete_comment, patch_item,
};
pub use create::{CreateFields, CreatedItem, create_item};
pub use notifications::{
    NotificationEvent, NotificationPage, NotificationTarget, TaigaNotification,
    fetch_all_web_notifications, fetch_notifications_page, mark_notification_as_read,
};

pub(crate) use project_meta::ProjectMetaCache;

/// Cached `/users/me` response (display + username + ID).
pub(super) struct MyselfData {
    pub(super) id: u64,
    pub(super) username: String,
    #[allow(dead_code)] // surfaced via API later (display purposes)
    pub(super) full_name: String,
}

/// Live JWT pair. Once `auth_token` expires Taiga returns 401, the caller
/// has to re-login (refresh-token flow is not implemented yet — keeping the
/// surface small until we hit the actual lifetime ceiling).
struct Tokens {
    auth_token: String,
    #[allow(dead_code)] // refresh flow not wired yet
    refresh_token: Option<String>,
}

/// Persistable login session — what the auth orchestrator writes into
/// its session blob and reads back on cache hit. Contains the JWT pair
/// plus the user identity returned by `/auth` so a restored client can
/// answer `current_user_id` / `current_username` without an extra
/// `/users/me` round-trip.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TaigaSession {
    pub auth_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub user_id: u64,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub full_name: String,
}

pub struct TaigaClient {
    pub(super) base_url: String,
    pub(super) http: HttpClient,
    tokens: StdMutex<Option<Tokens>>,
    myself: OnceCell<MyselfData>,
    pub(super) project_meta: ProjectMetaCache,
    pub(super) db: Arc<DatabaseConnection>,
    pub(super) scope_id: Uuid,
}

#[derive(Serialize)]
struct AuthRequest<'a> {
    #[serde(rename = "type")]
    auth_type: &'a str,
    username: &'a str,
    password: &'a str,
}

#[derive(Deserialize)]
struct AuthResponse {
    auth_token: String,
    #[serde(default)]
    refresh: Option<String>,
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    full_name: Option<String>,
    #[serde(default, rename = "full_name_display")]
    full_name_display: Option<String>,
}

/// HTTP timeout budget for the Taiga client. Both are hard ceilings and
/// guard against the "wait forever on a dead socket" hang; see
/// [`crate::adapter::config::TaigaConfig`] (`request_timeout_secs` /
/// `connect_timeout_secs`) for the rationale and defaults.
#[derive(Clone, Copy)]
pub struct HttpTimeouts {
    /// Whole-request ceiling (headers + body).
    pub request_secs: u64,
    /// Connection-establishment ceiling (DNS + TCP + TLS).
    pub connect_secs: u64,
}

/// Build the shared reqwest client with timeouts applied. The connect
/// ceiling is separate from the overall request budget so an unreachable
/// host fails fast instead of eating the full budget just to open a
/// socket — while a high-latency link can still lift it via config.
fn build_http_client(timeouts: HttpTimeouts) -> Result<HttpClient, String> {
    HttpClient::builder()
        .timeout(Duration::from_secs(timeouts.request_secs.max(1)))
        .connect_timeout(Duration::from_secs(timeouts.connect_secs.max(1)))
        .build()
        .map_err(|e| format!("build http client: {e}"))
}

/// Run the `/auth` round-trip and return the resulting session. No DB
/// writes, no client construction — pure HTTP. Callers (the orchestrator
/// login fn) serialise this into the orchestrator's session blob.
pub async fn perform_login(
    base_url: &str,
    username: &str,
    password: &str,
    timeouts: HttpTimeouts,
) -> Result<TaigaSession, String> {
    let http = build_http_client(timeouts)?;
    let base = base_url.trim_end_matches('/').to_string();
    let req = AuthRequest {
        auth_type: "normal",
        username,
        password,
    };
    let url = format!("{base}/api/v1/auth");
    http_log::log_request("POST", &url);
    let resp = http
        .post(&url)
        .json(&req)
        .send()
        .await
        .map_err(|e| http_log::network_error("POST", &url, e))?;
    let resp = http_log::check_status("POST", &url, resp).await?;
    let body: AuthResponse = resp
        .json()
        .await
        .map_err(|e| format!("parse login response: {e}"))?;
    Ok(TaigaSession {
        auth_token: body.auth_token,
        refresh_token: body.refresh,
        user_id: body.id.unwrap_or(0),
        username: body.username.unwrap_or_default(),
        full_name: body
            .full_name_display
            .or(body.full_name)
            .unwrap_or_default(),
    })
}

impl TaigaClient {
    /// Build a client from a stored session. No HTTP — primes the
    /// `MyselfData` cache from the session if present so callers don't
    /// have to refetch `/users/me` for already-known identity fields.
    pub fn from_session(
        base_url: &str,
        session: TaigaSession,
        db: Arc<DatabaseConnection>,
        scope_id: Uuid,
        timeouts: HttpTimeouts,
    ) -> Result<Arc<Self>, String> {
        let http = build_http_client(timeouts)?;
        let base = base_url.trim_end_matches('/').to_string();
        let client = Arc::new(Self {
            base_url: base,
            http,
            tokens: StdMutex::new(Some(Tokens {
                auth_token: session.auth_token,
                refresh_token: session.refresh_token,
            })),
            myself: OnceCell::new(),
            project_meta: ProjectMetaCache::default(),
            db,
            scope_id,
        });
        if session.user_id != 0 {
            let _ = client.myself.set(MyselfData {
                id: session.user_id,
                username: session.username,
                full_name: session.full_name,
            });
        }
        Ok(client)
    }

    /// Snapshot of the current JWT pair, for persistence.
    pub fn token_snapshot(&self) -> Option<(String, Option<String>)> {
        self.tokens
            .lock()
            .unwrap()
            .as_ref()
            .map(|t| (t.auth_token.clone(), t.refresh_token.clone()))
    }

    /// Request headers populated with the current bearer token.
    pub(super) fn auth_headers(&self) -> Result<HeaderMap, String> {
        let token = self
            .tokens
            .lock()
            .unwrap()
            .as_ref()
            .map(|t| t.auth_token.clone())
            .ok_or_else(|| "not logged in".to_string())?;
        let mut h = HeaderMap::new();
        h.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|e| format!("auth header: {e}"))?,
        );
        h.insert(ACCEPT, HeaderValue::from_static("application/json"));
        Ok(h)
    }

    /// Send a request with one automatic retry on transport failure (the
    /// adapter-level "reconnect").
    ///
    /// `build` is invoked afresh for each attempt — a `RequestBuilder` is
    /// consumed by `send`, and re-issuing the request lets reqwest drop a
    /// dead keep-alive socket from its pool and open a new connection,
    /// which is exactly what recovers the "connection silently went away"
    /// case the user hits. Only *transport* errors (timeout, connection
    /// reset/refused) trigger the retry; an HTTP status response — even
    /// 4xx/5xx — is handed back untouched for the caller's
    /// [`http_log::check_status`], because re-sending wouldn't change it.
    ///
    /// The per-request timeout from [`build_http_client`] bounds each of
    /// the (at most two) attempts, so the worst case is roughly twice the
    /// configured timeout, then a clean error — never a permanent hang.
    pub(super) async fn send_retrying(
        &self,
        method: &str,
        url: &str,
        build: impl Fn() -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, String> {
        match build().send().await {
            Ok(resp) => Ok(resp),
            Err(first) => {
                http_log::log_error(
                    "taiga http",
                    &format!("{method} {url}: {first}; reconnecting and retrying once"),
                );
                build()
                    .send()
                    .await
                    .map_err(|e| http_log::network_error(method, url, e))
            }
        }
    }

    /// Cached `/users/me` lookup. Used to resolve the `$me` placeholder
    /// and to validate a restored session.
    pub(super) async fn myself(&self) -> Result<&MyselfData, String> {
        self.myself
            .get_or_try_init(|| async {
                let url = format!("{}/api/v1/users/me", self.base_url);
                let headers = self.auth_headers()?;
                http_log::log_request("GET", &url);
                let resp = self
                    .send_retrying("GET", &url, || self.http.get(&url).headers(headers.clone()))
                    .await?;
                let resp = http_log::check_status("GET", &url, resp).await?;
                let raw: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| format!("/users/me parse: {e}"))?;
                Ok(MyselfData {
                    id: raw.get("id").and_then(|v| v.as_u64()).unwrap_or(0),
                    username: raw
                        .get("username")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    full_name: raw
                        .get("full_name_display")
                        .and_then(|v| v.as_str())
                        .or_else(|| raw.get("full_name").and_then(|v| v.as_str()))
                        .unwrap_or_default()
                        .to_string(),
                })
            })
            .await
    }

    pub async fn current_user_id(&self) -> Result<u64, String> {
        Ok(self.myself().await?.id)
    }

    pub async fn current_username(&self) -> Result<&str, String> {
        Ok(self.myself().await?.username.as_str())
    }
}
