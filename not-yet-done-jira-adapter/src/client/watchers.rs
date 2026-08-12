//! Watcher add/remove + isWatching probe + toggle wrapper.

use not_yet_done_content::http_log;
use serde::Deserialize;

use super::JiraClient;

#[derive(Deserialize)]
struct WatchersResponse {
    #[serde(rename = "isWatching", default)]
    is_watching: bool,
}

impl JiraClient {
    /// Whether the authenticated user is currently watching `key`.
    pub async fn is_watching(&self, key: &str) -> Result<bool, String> {
        let url = format!("{}/rest/api/2/issue/{}/watchers", self.base_url, key);
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

        let data: WatchersResponse = serde_json::from_str(&body_text)
            .map_err(|e| format!("Failed to parse watchers: {e}"))?;
        Ok(data.is_watching)
    }

    /// Add the authenticated user as a watcher.
    pub async fn add_watcher(&self, key: &str) -> Result<(), String> {
        let username = self.current_username().await?.to_string();
        let url = format!("{}/rest/api/2/issue/{}/watchers", self.base_url, key);
        http_log::log_request("POST", &url);
        // Server/DC takes the username as a JSON-encoded string literal in the
        // request body — not an object.
        let resp = self
            .http
            .post(&url)
            .json(&username)
            .send()
            .await
            .map_err(|e| http_log::network_error("POST", &url, e))?;
        self.check_status("POST", &url, resp).await?;
        Ok(())
    }

    /// Remove the authenticated user from the watcher list.
    pub async fn remove_watcher(&self, key: &str) -> Result<(), String> {
        let username = self.current_username().await?.to_string();
        let url = format!("{}/rest/api/2/issue/{}/watchers", self.base_url, key);
        http_log::log_request("DELETE", &format!("{url}?username={username}"));
        let resp = self
            .http
            .delete(&url)
            .query(&[("username", username.as_str())])
            .send()
            .await
            .map_err(|e| http_log::network_error("DELETE", &url, e))?;
        self.check_status("DELETE", &url, resp).await?;
        Ok(())
    }

    /// Toggle the authenticated user's watch state. Returns the new state
    /// (`true` = now watching, `false` = no longer watching).
    pub async fn toggle_watch(&self, key: &str) -> Result<bool, String> {
        if self.is_watching(key).await? {
            self.remove_watcher(key).await?;
            Ok(false)
        } else {
            self.add_watcher(key).await?;
            Ok(true)
        }
    }
}
