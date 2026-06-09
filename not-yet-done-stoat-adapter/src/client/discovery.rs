//! Server self-discovery: `GET /api/` (unauthenticated) returns the
//! instance config, including the WebSocket URL we hand to the gateway
//! and the file/embed servers (autumn/january). The adapter config only
//! needs the base domain; everything else is discovered here.

use serde::Deserialize;

use not_yet_done_content::http_log;

/// The subset of `GET /api/` we consume. The real payload also carries
/// `features`, `app`, `vapid`, … which we ignore for now.
#[derive(Deserialize, Debug, Clone)]
pub struct RootInfo {
    /// Backend version string (e.g. "0.11.5").
    #[serde(default)]
    pub revolt: String,
    /// Absolute WebSocket URL the gateway connects to.
    pub ws: String,
}

/// Fetch the instance config from `{base_url}/api/`.
pub async fn fetch_root_info(
    http: &reqwest::Client,
    base_url: &str,
) -> Result<RootInfo, String> {
    let url = format!("{}/api/", base_url.trim_end_matches('/'));
    http_log::log_request("GET", &url);
    let resp = http
        .get(&url)
        .send()
        .await
        .map_err(|e| http_log::network_error("GET", &url, e))?;
    let resp = http_log::check_status("GET", &url, resp).await?;
    resp.json::<RootInfo>()
        .await
        .map_err(|e| format!("parse /api/ root info: {e}"))
}
