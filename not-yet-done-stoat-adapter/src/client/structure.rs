//! Server-structure mutations: creating channels and editing the
//! server's category list.
//!
//! Two endpoints back the "new channel / new category" actions:
//!
//! - `POST /api/servers/{server}/channels` — creates a channel. The body
//!   `{ "type": "Text", "name": … }` is enough; the new channel lands in
//!   the server's `channels` list **uncategorized**.
//! - `PATCH /api/servers/{server}` with a `categories` field — Stoat has
//!   **no** dedicated create-category endpoint. Categories live on the
//!   server object and are edited as a **full-list replacement**. So both
//!   "add a category" and "drop a freshly-made channel into a category"
//!   are the same call: send the whole desired `categories` array. The
//!   caller (adapter) builds that array from the live `StoatState`.
//!
//! All three shapes were verified against the live instance (Stoat
//! 0.13.7) before wiring: see the adapter's `server`/`category` nodes.

use serde::Deserialize;

use not_yet_done_content::http_log;

use super::StoatClient;
use crate::gateway::protocol::Category;

#[derive(Deserialize)]
struct CreatedChannel {
    #[serde(rename = "_id")]
    id: String,
}

impl StoatClient {
    /// Create a text channel in `server_id`. Returns the new channel's id.
    /// The channel is created uncategorized; placing it under a category
    /// is a second [`update_server_categories`](Self::update_server_categories)
    /// call (Stoat has no atomic "create in category").
    pub async fn create_channel(&self, server_id: &str, name: &str) -> Result<String, String> {
        let url = format!("{}/api/servers/{}/channels", self.base_url(), server_id);
        http_log::log_request("POST", &url);
        let resp = self
            .http
            .post(&url)
            .headers(self.auth_headers()?)
            .json(&serde_json::json!({ "type": "Text", "name": name }))
            .send()
            .await
            .map_err(|e| http_log::network_error("POST", &url, e))?;
        let resp = http_log::check_status("POST", &url, resp).await?;
        let created = resp
            .json::<CreatedChannel>()
            .await
            .map_err(|e| format!("parse created channel: {e}"))?;
        Ok(created.id)
    }

    /// Replace the server's entire category list via `PATCH`. This is the
    /// only way to add a category or move a channel between categories —
    /// the API takes the full desired list, not a delta. The gateway
    /// echoes the change back as a `ServerUpdate`, refreshing the tree.
    pub async fn update_server_categories(
        &self,
        server_id: &str,
        categories: &[Category],
    ) -> Result<(), String> {
        let url = format!("{}/api/servers/{}", self.base_url(), server_id);
        http_log::log_request("PATCH", &url);
        let resp = self
            .http
            .patch(&url)
            .headers(self.auth_headers()?)
            .json(&serde_json::json!({ "categories": categories }))
            .send()
            .await
            .map_err(|e| http_log::network_error("PATCH", &url, e))?;
        http_log::check_status("PATCH", &url, resp).await?;
        Ok(())
    }
}
