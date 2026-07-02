//! Kimai REST API client.
//!
//! Speaks the two auth forms Kimai has used across versions:
//!
//! - `user-api-token`: the `X-AUTH-USER` / `X-AUTH-TOKEN` header pair
//!   (the only API auth up to Kimai 2.13; the token is the user's "API
//!   password" from the profile page).
//! - `bearer-token`: `Authorization: Bearer <token>` with an API token
//!   (Kimai 2.14+).
//!
//! Endpoints used: `/api/version` (session validation), `/api/timesheets`
//! (paged list), `/api/projects` + `/api/activities` (id → name lookups —
//! the timesheet list carries only numeric ids).

use std::time::Duration;

use not_yet_done_content::http_log;
use reqwest::StatusCode;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};

/// Page size for the timesheet listing. Kimai pages server-side; the
/// client loops until a short page signals the end.
const PAGE_SIZE: usize = 100;

/// Persistable Kimai auth session — what the auth orchestrator writes
/// into its session blob and reads back on cache hit. `username` is
/// populated for the header-pair mechanism and absent for bearer tokens.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct KimaiSession {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    pub token: String,
}

/// One timesheet record as returned by `GET /api/timesheets`. `project`,
/// `activity` and `user` come as numeric ids only — resolve names via
/// [`KimaiClient::projects`] / [`KimaiClient::activities`].
#[derive(Deserialize, Clone, Debug)]
pub struct KimaiTimesheet {
    pub id: u64,
    pub project: u64,
    pub activity: u64,
    pub begin: String,
    /// `None` while the timesheet is still running.
    #[serde(default)]
    pub end: Option<String>,
    /// Seconds. `None`/0 while running.
    #[serde(default)]
    pub duration: Option<i64>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// One entry of `GET /api/projects`. `parent_title` is the customer name.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct KimaiProject {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub parent_title: Option<String>,
}

/// One entry of `GET /api/activities`. The API also carries a
/// `parentTitle` (the owning project for project-bound activities), but
/// that is redundant with the timesheet's own project id — not mapped.
#[derive(Deserialize, Clone, Debug)]
pub struct KimaiActivity {
    pub id: u64,
    pub name: String,
}

#[derive(Deserialize)]
struct VersionResponse {
    version: String,
}

/// HTTP timeouts, resolved by the factory from the adapter config.
#[derive(Clone, Copy, Debug)]
pub struct HttpTimeouts {
    pub request_secs: u64,
    pub connect_secs: u64,
}

pub struct KimaiClient {
    base_url: String,
    http: reqwest::Client,
}

impl KimaiClient {
    /// Build a client with the auth headers baked in. `session.username`
    /// present → `X-AUTH-USER`/`X-AUTH-TOKEN` header pair; absent →
    /// `Authorization: Bearer`.
    pub fn from_session(
        url: &str,
        session: &KimaiSession,
        timeouts: HttpTimeouts,
    ) -> Result<Self, String> {
        let mut headers = HeaderMap::new();
        match &session.username {
            Some(user) => {
                headers.insert(
                    "X-AUTH-USER",
                    HeaderValue::from_str(user).map_err(|e| format!("invalid username: {e}"))?,
                );
                headers.insert(
                    "X-AUTH-TOKEN",
                    HeaderValue::from_str(&session.token)
                        .map_err(|e| format!("invalid token: {e}"))?,
                );
            }
            None => {
                let bearer = format!("Bearer {}", session.token);
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_str(&bearer).map_err(|e| format!("invalid token: {e}"))?,
                );
            }
        }

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(timeouts.request_secs))
            .connect_timeout(Duration::from_secs(timeouts.connect_secs))
            .build()
            .map_err(|e| format!("failed to create HTTP client: {e}"))?;

        Ok(Self {
            base_url: url.trim_end_matches('/').to_string(),
            http,
        })
    }

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        query: &[(&str, &str)],
    ) -> Result<T, String> {
        http_log::log_request("GET", url);
        let resp = self
            .http
            .get(url)
            .query(query)
            .send()
            .await
            .map_err(|e| http_log::network_error("GET", url, e))?;
        let resp = http_log::check_status("GET", url, resp).await?;
        let body = resp
            .text()
            .await
            .map_err(|e| format!("failed to read response: {e}"))?;
        serde_json::from_str(&body).map_err(|e| format!("failed to parse {url}: {e}"))
    }

    /// Session validation: any authenticated round-trip works; `/api/version`
    /// is the cheapest. Returns the server version string.
    pub async fn version(&self) -> Result<String, String> {
        let url = format!("{}/api/version", self.base_url);
        let v: VersionResponse = self.get_json(&url, &[]).await?;
        Ok(v.version)
    }

    /// All timesheets of the authenticated user starting at `begin_local`
    /// (Kimai expects local time, `YYYY-MM-DDTHH:mm:ss`). Pages through the
    /// listing until a short page; a 404 past page 1 also counts as the end
    /// (some Kimai versions 404 on out-of-range pages instead of returning
    /// an empty array).
    pub async fn timesheets_since(
        &self,
        begin_local: &str,
    ) -> Result<Vec<KimaiTimesheet>, String> {
        let url = format!("{}/api/timesheets", self.base_url);
        let size = PAGE_SIZE.to_string();
        let mut out: Vec<KimaiTimesheet> = Vec::new();
        let mut page: u32 = 1;
        loop {
            let page_str = page.to_string();
            let query = [
                ("begin", begin_local),
                ("size", size.as_str()),
                ("page", page_str.as_str()),
            ];

            http_log::log_request("GET", &url);
            let resp = self
                .http
                .get(&url)
                .query(&query)
                .send()
                .await
                .map_err(|e| http_log::network_error("GET", &url, e))?;
            if page > 1 && resp.status() == StatusCode::NOT_FOUND {
                break;
            }
            let resp = http_log::check_status("GET", &url, resp).await?;
            let body = resp
                .text()
                .await
                .map_err(|e| format!("failed to read response: {e}"))?;
            let items: Vec<KimaiTimesheet> = serde_json::from_str(&body)
                .map_err(|e| format!("failed to parse timesheets: {e}"))?;

            let n = items.len();
            out.extend(items);
            if n < PAGE_SIZE {
                break;
            }
            page += 1;
        }
        Ok(out)
    }

    /// One timesheet by id.
    pub async fn timesheet(&self, id: u64) -> Result<KimaiTimesheet, String> {
        let url = format!("{}/api/timesheets/{id}", self.base_url);
        self.get_json(&url, &[]).await
    }

    /// All projects visible to the user (for the id → name lookup).
    pub async fn projects(&self) -> Result<Vec<KimaiProject>, String> {
        let url = format!("{}/api/projects", self.base_url);
        self.get_json(&url, &[]).await
    }

    /// All activities visible to the user (for the id → name lookup).
    pub async fn activities(&self) -> Result<Vec<KimaiActivity>, String> {
        let url = format!("{}/api/activities", self.base_url);
        self.get_json(&url, &[]).await
    }
}
