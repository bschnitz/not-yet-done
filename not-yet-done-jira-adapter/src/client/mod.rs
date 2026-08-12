//! Jira REST API client — fetches issues via JQL.
//!
//! Split into submodules by concern: `search` (issues + JQL), `comments`,
//! `attachments`, `transitions`, `users`. The client itself (struct + auth +
//! `current_user`) lives in this file along with private DTOs shared across
//! submodules (`Assignee`, `NameField`).

use std::time::Duration;

use not_yet_done_content::http_log;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, COOKIE, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};

mod attachments;
mod comments;
mod create;
mod links;
mod search;
mod transitions;
mod users;
mod watchers;

pub use attachments::JiraAttachment;
pub use comments::JiraComment;
pub use create::CreateIssueFields;
pub use links::JiraLinkType;
pub use search::{JiraIssueDetail, JiraTicket};
pub use transitions::JiraTransition;
pub use users::JiraUser;

/// Per-request timeout. Caps the worst-case latency of any single Jira call
/// so a slow / unresponsive server can't keep a request hanging indefinitely.
/// Combined with the App's background-commit task this is a belt-and-braces
/// safety net — even if everything else fails, a single call dies after this.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Normalize line endings to LF. Jira accepts edits via its web UI which
/// posts CRLFs through HTML forms; the REST API echoes that back verbatim.
/// Once those CRs leak into the editor template they show up as `^M` and
/// break syntax highlighting. We strip them at the API boundary so every
/// downstream consumer (template render, diff, content().read()) sees the
/// same `\n`-only form. Diff detection stays stable: both sides of the
/// comparison have always been routed through `text.lines()` /
/// `normalize_blank_lines`, which already trim CRs from line ends.
pub(super) fn normalize_eol(s: String) -> String {
    if s.contains('\r') {
        s.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        s
    }
}

/// Extract the Jira base URL from a URL that may contain paths like
/// `/browse/TICKET-123` or `/rest/api/...`.
fn normalize_base_url(url: &str) -> String {
    let url = url.trim_end_matches('/');
    // Strip known Jira UI/API suffixes.
    for marker in &["/browse/", "/rest/", "/projects/", "/issues/", "/secure/"] {
        if let Some(pos) = url.find(marker) {
            return url[..pos].to_string();
        }
    }
    url.to_string()
}

/// Generic `{ "id": "...", "name": "..." }` field used by `status`,
/// `priority`, `issuetype`, etc. `id` is captured opportunistically — most
/// callers only need `name`, but workflow recording stores ids as stable
/// keys (display names can be renamed without breaking the cache).
#[derive(Deserialize)]
pub(super) struct NameField {
    #[serde(default)]
    pub(super) id: Option<String>,
    pub(super) name: Option<String>,
}

/// Shared user-shaped object. Used by `assignee`, `reporter`, `creator`,
/// `comment.author`, `attachment.author`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Assignee {
    pub(super) display_name: Option<String>,
    /// Server/DC username — same value that appears inside `[~name]` mentions.
    pub(super) name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MyselfResponse {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

/// Persistable Jira auth session — what the auth orchestrator writes
/// into its session blob and reads back on cache hit. One of the two
/// shapes is populated depending on the configured mechanism: `cookie`
/// for the `cookie` mechanism, `email`+`token` for `basic-auth`. The
/// bridge picks the right `JiraClient` constructor argument set from
/// whichever fields are present.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct JiraSession {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookie: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// Authenticated-user metadata cached per `JiraClient`.
pub(super) struct MyselfData {
    /// Human-readable name (e.g. `"Bob Example"`). Empty when Jira returns no
    /// display name — fall back to `username` in that case.
    pub(super) display_name: String,
    /// Login name / Server-DC `name` field — what appears inside `[~name]`
    /// mentions and what the watcher API expects.
    pub(super) username: String,
}

pub struct JiraClient {
    pub(super) base_url: String,
    pub(super) http: reqwest::Client,
    myself: tokio::sync::OnceCell<MyselfData>,
    /// Set when the server rejects this client's session; read by the auth
    /// bridge, which then throws the client away and logs in again.
    rejection: http_log::AuthRejection,
}

impl JiraClient {
    /// [`http_log::check_status`] for this client. Every REST call in the
    /// submodules goes through here rather than the free function, so a
    /// rejected session is noticed where it happens instead of being
    /// re-reported on every later call (see [`JiraClient::auth_rejected`]).
    pub(super) async fn check_status(
        &self,
        method: &str,
        url: &str,
        resp: reqwest::Response,
    ) -> Result<reqwest::Response, String> {
        self.rejection.check_status(method, url, resp).await
    }

    /// Whether the server has rejected this client's session — an expired
    /// cookie, a revoked token, or an SSO deployment bouncing the API call
    /// into its login flow. The auth bridge drops a client that reports
    /// `true`.
    pub fn auth_rejected(&self) -> bool {
        self.rejection.is_rejected()
    }

    /// Build a client from explicit parameters.
    pub fn new(
        url: &str,
        email: Option<&str>,
        token: Option<&str>,
        session_id: Option<&str>,
        accept_invalid_certs: bool,
    ) -> Result<Self, String> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        if let Some(session_id) = session_id {
            let cookie = if session_id.contains('=') {
                session_id.to_string()
            } else {
                format!("JSESSIONID={session_id}")
            };
            headers.insert(
                COOKIE,
                HeaderValue::from_str(&cookie).map_err(|e| format!("Invalid cookie: {e}"))?,
            );
        } else if let (Some(email), Some(token)) = (email, token) {
            use base64::Engine;
            let credentials = format!("{email}:{token}");
            let encoded = base64::engine::general_purpose::STANDARD.encode(&credentials);
            let auth = format!("Basic {encoded}");
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&auth).map_err(|e| format!("Invalid auth header: {e}"))?,
            );
        } else {
            return Err("No authentication configured".into());
        }

        let base_url = normalize_base_url(url);

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .danger_accept_invalid_certs(accept_invalid_certs)
            .redirect(http_log::api_redirect_policy())
            .timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

        Ok(Self {
            base_url,
            http,
            myself: tokio::sync::OnceCell::new(),
            rejection: http_log::AuthRejection::new(),
        })
    }

    /// Build a client from a stored session. Thin convenience wrapper
    /// over [`JiraClient::new`] that picks the relevant arguments based
    /// on which session-blob fields are populated.
    pub fn from_session(
        url: &str,
        session: JiraSession,
        accept_invalid_certs: bool,
    ) -> Result<Self, String> {
        Self::new(
            url,
            session.email.as_deref(),
            session.token.as_deref(),
            session.cookie.as_deref(),
            accept_invalid_certs,
        )
    }

    async fn myself(&self) -> Result<&MyselfData, String> {
        self.myself
            .get_or_try_init(|| async {
                let url = format!("{}/rest/api/2/myself", self.base_url);
                http_log::log_request("GET", &url);
                let resp = self
                    .http
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| http_log::network_error("GET", &url, e))?;
                let resp = self.check_status("GET", &url, resp).await?;

                let body_text = resp
                    .text()
                    .await
                    .map_err(|e| format!("Failed to read response: {e}"))?;

                let raw: MyselfResponse = serde_json::from_str(&body_text)
                    .map_err(|e| format!("Failed to parse /myself: {e}"))?;

                Ok(MyselfData {
                    display_name: raw.display_name.unwrap_or_default(),
                    username: raw.name.unwrap_or_default(),
                })
            })
            .await
    }

    /// Display name of the authenticated user. Cached for the lifetime of
    /// this client (one HTTP call on first access). Used to gate per-author
    /// actions on comments etc.
    pub async fn current_user(&self) -> Result<&str, String> {
        let me = self.myself().await?;
        if !me.display_name.is_empty() {
            Ok(&me.display_name)
        } else {
            Ok(&me.username)
        }
    }

    /// Server-DC login name (`name` field of `/myself`) of the authenticated
    /// user. Required by the watcher API and other endpoints that key off the
    /// internal username instead of a display string.
    pub async fn current_username(&self) -> Result<&str, String> {
        Ok(&self.myself().await?.username)
    }

    /// Synchronously read the cached display name, if it has been fetched.
    /// Returns `None` until the first successful `current_user()` call.
    pub fn current_user_cached(&self) -> Option<&str> {
        self.myself.get().map(|me| {
            if !me.display_name.is_empty() {
                me.display_name.as_str()
            } else {
                me.username.as_str()
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_eol_strips_crlf() {
        assert_eq!(normalize_eol("a\r\nb\r\n".into()), "a\nb\n");
    }

    #[test]
    fn normalize_eol_strips_bare_cr() {
        // Old Mac line endings — unlikely from Jira, but handled for free.
        assert_eq!(normalize_eol("a\rb\rc".into()), "a\nb\nc");
    }

    #[test]
    fn normalize_eol_passthrough_when_clean() {
        // Avoid the allocation if there's nothing to do.
        let input = String::from("plain text\nwith newlines\n");
        let cloned = input.clone();
        let normalized = normalize_eol(input);
        assert_eq!(normalized, cloned);
    }

    #[test]
    fn normalize_eol_handles_mixed() {
        assert_eq!(normalize_eol("a\r\nb\rc\n".into()), "a\nb\nc\n");
    }
}
