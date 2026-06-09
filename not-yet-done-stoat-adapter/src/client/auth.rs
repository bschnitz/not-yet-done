//! Login flow: `POST /api/auth/session/login`.
//!
//! Exchanges email + password for a session token. The token is the
//! only thing we persist (never the password). MFA is **not** supported
//! yet — a login that returns an MFA ticket instead of a token fails
//! with a clear message (the test account has no MFA; see plan §9).

use serde::{Deserialize, Serialize};

use not_yet_done_content::http_log;

/// Persistable login session — what the auth orchestrator writes into
/// its session blob and reads back on cache hit.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoatSession {
    /// `X-Session-Token` for all authenticated requests + the gateway.
    pub token: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub session_name: String,
}

#[derive(Serialize)]
struct LoginRequest<'a> {
    email: &'a str,
    password: &'a str,
    friendly_name: &'a str,
}

#[derive(Deserialize, Debug)]
struct LoginResponse {
    /// "Success" on the happy path; "MFA" when a ticket is required.
    #[serde(default)]
    result: Option<String>,
    #[serde(rename = "_id", default)]
    session_id: Option<String>,
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    name: Option<String>,
    /// Present on the MFA path instead of a token.
    #[serde(default)]
    ticket: Option<String>,
}

/// Perform a password login against `{base_url}/api/auth/session/login`.
pub async fn perform_login(
    base_url: &str,
    email: &str,
    password: &str,
) -> Result<StoatSession, String> {
    let http = reqwest::Client::builder()
        .build()
        .map_err(|e| format!("build http client: {e}"))?;
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/api/auth/session/login");
    let body = LoginRequest {
        email,
        password,
        friendly_name: "not-yet-done",
    };
    http_log::log_request("POST", &url);
    let resp = http
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| http_log::network_error("POST", &url, e))?;
    let resp = http_log::check_status("POST", &url, resp).await?;
    let parsed: LoginResponse = resp
        .json()
        .await
        .map_err(|e| format!("parse login response: {e}"))?;

    if parsed.ticket.is_some() || parsed.result.as_deref() == Some("MFA") {
        return Err(
            "this account requires multi-factor authentication, which is not yet supported".into(),
        );
    }

    let token = parsed
        .token
        .filter(|t| !t.is_empty())
        .ok_or_else(|| "login response contained no session token".to_string())?;

    Ok(StoatSession {
        token,
        user_id: parsed.user_id.unwrap_or_default(),
        session_id: parsed.session_id.unwrap_or_default(),
        session_name: parsed.name.unwrap_or_default(),
    })
}
