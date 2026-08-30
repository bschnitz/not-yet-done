//! Taiga REST client. Auth via username/password → JWT.
//!
//! The login round-trip and the constructor that takes a stored session
//! are split: [`perform_login`] does only the HTTP exchange and returns
//! a [`TaigaSession`] (suitable for serialising into the auth
//! orchestrator's session blob); [`TaigaClient::from_session`] turns
//! such a blob back into a live client without an additional round-trip.

use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use not_yet_done_content::http_send::{Repeat, RetryConfig};
use not_yet_done_content::{http_log, http_send};
use reqwest::{
    Client as HttpClient,
    header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue},
};
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;
use uuid::Uuid;

mod actions;
mod convert;
mod create;
mod edit;
mod history;
mod notifications;
mod project_meta;
pub mod query;

pub use actions::{
    TaigaAttachment, delete_attachment, download_attachment, edit_comment, list_attachments,
    toggle_watch, upload_attachment, upload_attachment_bytes,
};
pub use convert::{delete_item, fetch_id_name_map, fetch_raw_detail};
pub use create::{CreateFields, CreatedItem, create_item};
pub use edit::{EditFields, ItemPatch, PatchOutcome, add_comment, delete_comment, patch_item};
pub use history::{TaigaComment, fetch_comments};
pub use notifications::{
    NotificationEvent, NotificationPage, NotificationTarget, TaigaNotification,
    fetch_all_web_notifications, fetch_notifications_page, mark_notification_as_read,
};
pub use project_meta::{TaigaMember, TaigaStatus};
pub use query::{
    ItemSummary, ItemType, ParsedTaigaQuery, QuerySpec, apply_sort as apply_query_sort,
    default_sort, parse_query_yaml, parse_taiga_query, run_queries, sortable_column_keys,
};
/// Crate-internal: the detail path reuses the list path's person-name
/// resolution so a row and its ticket buffer never disagree on a name.
pub(crate) use query::{member_display_name, owner_display_name, tag_names};

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
    /// How often a request may be repeated after a transport failure,
    /// from the adapter config. See [`http_send`].
    retry: RetryConfig,
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
    retry: &RetryConfig,
) -> Result<TaigaSession, String> {
    let http = build_http_client(timeouts)?;
    let base = base_url.trim_end_matches('/').to_string();
    let req = AuthRequest {
        auth_type: "normal",
        username,
        password,
    };
    let url = format!("{base}/api/v1/auth");
    let resp = http_send::send(
        retry,
        Repeat::of_method("POST"),
        "POST",
        &url,
        http.post(&url).json(&req),
    )
    .await?;
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
        retry: RetryConfig,
    ) -> Result<Arc<Self>, String> {
        let http = build_http_client(timeouts)?;
        let base = base_url.trim_end_matches('/').to_string();
        let client = Arc::new(Self {
            base_url: base,
            http,
            retry,
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

    /// Send a request, repeating it on a transport failure as far as the
    /// configured [`RetryConfig`] and the HTTP method allow.
    ///
    /// The method decides: a connection that was never established can
    /// always be dialled again, but after a timeout the server may well
    /// have carried the request out and only the answer was lost — so a
    /// `POST`/`PATCH`/`DELETE` is repeated only in the first case. A read
    /// that travels as a POST can say so via [`Self::send_read`].
    pub(super) async fn send(
        &self,
        method: &str,
        url: &str,
        req: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, String> {
        http_send::send(&self.retry, Repeat::of_method(method), method, url, req).await
    }

    /// Like [`Self::send`], but for a call the caller knows has no side
    /// effects — it is repeated after a timeout as well, whatever the
    /// method says.
    #[allow(dead_code)] // for reads that travel as a POST
    pub(super) async fn send_read(
        &self,
        method: &str,
        url: &str,
        req: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, String> {
        http_send::send(&self.retry, Repeat::Safe, method, url, req).await
    }

    /// Cached `/users/me` lookup. Used to resolve the `$me` placeholder
    /// and to validate a restored session.
    pub(super) async fn myself(&self) -> Result<&MyselfData, String> {
        self.myself
            .get_or_try_init(|| async {
                let url = format!("{}/api/v1/users/me", self.base_url);
                let headers = self.auth_headers()?;
                let resp = self
                    .send("GET", &url, self.http.get(&url).headers(headers))
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
