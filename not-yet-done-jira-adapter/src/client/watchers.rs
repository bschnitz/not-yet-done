//! Watcher add/remove + isWatching probe + toggle wrapper.

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
        let resp = self.send("GET", &url, self.http.get(&url)).await?;
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
        // Server/DC takes the username as a JSON-encoded string literal in the
        // request body — not an object.
        let resp = self
            .send("POST", &url, self.http.post(&url).json(&username))
            .await?;
        self.check_status("POST", &url, resp).await?;
        Ok(())
    }

    /// Remove the authenticated user from the watcher list.
    pub async fn remove_watcher(&self, key: &str) -> Result<(), String> {
        let username = self.current_username().await?.to_string();
        let url = format!("{}/rest/api/2/issue/{}/watchers", self.base_url, key);
        let resp = self
            .send(
                "DELETE",
                &format!("{url}?username={username}"),
                self.http
                    .delete(&url)
                    .query(&[("username", username.as_str())]),
            )
            .await?;
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
