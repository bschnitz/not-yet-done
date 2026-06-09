//! Confluence Server / Data-Center REST client.
//!
//! CF-2a slice: just the struct, constructor, default headers, and one
//! `current_user()` health probe. Per-concern submodules (spaces, pages,
//! attachments, comments, CQL) join in CF-3+ once auth wiring is in
//! place.

use std::time::Duration;

use not_yet_done_content::http_log;
use reqwest::header::{ACCEPT, COOKIE, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};

mod attachment;
mod comment;
mod page;
mod search;
mod space;

pub use attachment::{AttachmentList, AttachmentMeta};
pub use comment::{CommentList, CommentMeta};
pub use page::{
    CreatedPage, PageAncestor, PageDetail, PageList, PageMeta, UpdatePageError, UpdatedPage,
};
pub use search::{AncestorMeta, SearchResultMeta, SearchResults};
pub use space::{SpaceMeta, SpacePage};

/// Per-request timeout. Confluence's `body.storage` payloads on large
/// pages can take several seconds to render server-side, so the cap is
/// generous; combined with App-level cancellation this is the
/// belt-and-braces ceiling for any single REST call.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Persistable Confluence auth session — what the auth orchestrator
/// writes into its session blob and reads back on cache hit. Only the
/// cookie shape is supported for now (Crowd SSO + JSESSIONID). Future
/// mechanisms (PAT, basic-auth) can extend this struct with additional
/// optional fields.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ConfluenceSession {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookie: Option<String>,
}

/// Subset of `/rest/api/user/current` we actually consume. Confluence
/// returns more than this; everything else is ignored.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ConfluenceUser {
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub user_key: Option<String>,
}

pub struct ConfluenceClient {
    base_url: String,
    http: reqwest::Client,
}

impl ConfluenceClient {
    /// Build a client from explicit parameters. `cookie_header` is the
    /// raw value of the `Cookie:` header (`"JSESSIONID=...; crowd.token_key=..."`)
    /// — adapters never split it back into individual cookies; Confluence
    /// only cares about the concatenated form.
    pub fn new(
        base_url: &str,
        cookie_header: &str,
        accept_invalid_certs: bool,
    ) -> Result<Self, String> {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            COOKIE,
            HeaderValue::from_str(cookie_header)
                .map_err(|e| format!("Invalid cookie header: {e}"))?,
        );
        // Confluence Server requires `X-Atlassian-Token: no-check` on
        // every mutating call to satisfy the XSRF check. Setting it as
        // a default header keeps later POST/PUT/DELETE call-sites quiet
        // — GETs ignore it.
        headers.insert(
            HeaderName::from_static("x-atlassian-token"),
            HeaderValue::from_static("no-check"),
        );

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .danger_accept_invalid_certs(accept_invalid_certs)
            .timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http,
        })
    }

    /// Build a client from a stored session. Thin convenience wrapper
    /// over [`ConfluenceClient::new`] that picks the relevant field
    /// based on which session-blob fields are populated.
    pub fn from_session(
        base_url: &str,
        session: ConfluenceSession,
        accept_invalid_certs: bool,
    ) -> Result<Self, String> {
        let cookie = session
            .cookie
            .ok_or_else(|| "Confluence session: missing cookie field".to_string())?;
        Self::new(base_url, &cookie, accept_invalid_certs)
    }

    /// Base URL with the Atlassian context-path included, no trailing slash.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Accessor for the configured `reqwest` client. Crate-private — the
    /// per-concern submodules ([`space`], later `pages`/`comments`/…) call
    /// REST endpoints through here while the headers + timeout + TLS
    /// policy stay in one place. The accessor is named differently from
    /// the underlying field so call sites read as method invocations
    /// (Rust resolves `self.http` to the field first and only falls back
    /// to a method when the names differ).
    pub(crate) fn inner_http(&self) -> &reqwest::Client {
        &self.http
    }

    /// Health-probe / current-user lookup. Calls
    /// `GET /rest/api/user/current` and parses the relevant fields. Used
    /// by the auth bridge (CF-2b) as the session-validation endpoint;
    /// CF-2a only exercises it from tests.
    pub async fn current_user(&self) -> Result<ConfluenceUser, String> {
        let url = format!("{}/rest/api/user/current", self.base_url);
        http_log::log_request("GET", &url);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| http_log::network_error("GET", &url, e))?;
        let resp = http_log::check_status("GET", &url, resp).await?;
        let body = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;
        serde_json::from_str(&body)
            .map_err(|e| format!("Failed to parse /user/current: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_cookie_with_control_chars() {
        let err = ConfluenceClient::new(
            "https://wiki.example.invalid",
            "JSESSIONID=bad\nvalue",
            false,
        )
        .err()
        .expect("control chars must be rejected");
        assert!(err.contains("cookie"), "error mentions cookie: {err}");
    }

    #[test]
    fn trims_trailing_slash_from_base_url() {
        let client = ConfluenceClient::new(
            "https://wiki.example.invalid/confluence/",
            "JSESSIONID=synthetic",
            false,
        )
        .expect("builds");
        assert_eq!(client.base_url(), "https://wiki.example.invalid/confluence");
    }

    #[test]
    fn from_session_requires_cookie_field() {
        let err = ConfluenceClient::from_session(
            "https://wiki.example.invalid",
            ConfluenceSession::default(),
            false,
        )
        .err()
        .expect("missing cookie must error");
        assert!(err.contains("cookie"), "error mentions cookie: {err}");
    }
}
