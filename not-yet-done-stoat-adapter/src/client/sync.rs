//! Read-state sync: `GET /api/sync/unreads` + per-channel ack.
//!
//! Revolt tracks unread state server-side as a per-(channel,user) "last
//! read" message id. We pull the whole set once after each `Ready` to seed
//! [`StoatState::reads`](crate::gateway::StoatState), and ack a channel up
//! to a message when the user reads it (sends, or moves the cursor onto the
//! newest message). Acks also arrive over the WS as `ChannelAck`, keeping
//! the markers in sync across the user's other clients.

use serde::Deserialize;

use not_yet_done_content::http_log;

use super::StoatClient;

/// One entry of `GET /api/sync/unreads`: the channel/user pair plus the id
/// of the last message the user has read in it. `last_id` is absent for a
/// tracked channel that has never been read.
#[derive(Deserialize, Debug, Clone)]
pub struct ChannelUnread {
    #[serde(rename = "_id")]
    pub id: UnreadId,
    #[serde(default)]
    pub last_id: Option<String>,
}

/// The composite key of a [`ChannelUnread`]. We only read `channel`.
#[derive(Deserialize, Debug, Clone)]
pub struct UnreadId {
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub user: String,
}

impl ChannelUnread {
    /// Flatten to the `(channel_id, last_read_id)` pair
    /// [`StoatState::apply_unreads`](crate::gateway::StoatState::apply_unreads)
    /// consumes.
    pub fn into_pair(self) -> (String, Option<String>) {
        (self.id.channel, self.last_id)
    }
}

impl StoatClient {
    /// `GET /api/sync/unreads` → every channel's last-read marker for the
    /// authenticated user. Channels the user has never interacted with are
    /// simply absent from the list.
    pub async fn get_unreads(&self) -> Result<Vec<ChannelUnread>, String> {
        let url = format!("{}/api/sync/unreads", self.base_url());
        http_log::log_request("GET", &url);
        let resp = self
            .http
            .get(&url)
            .headers(self.auth_headers()?)
            .send()
            .await
            .map_err(|e| http_log::network_error("GET", &url, e))?;
        let resp = http_log::check_status("GET", &url, resp).await?;
        resp.json::<Vec<ChannelUnread>>()
            .await
            .map_err(|e| format!("parse unreads: {e}"))
    }

    /// `PUT /api/channels/{channel}/ack/{message}` — mark the channel read
    /// up to `message_id`. The API answers `204 No Content`. Best-effort:
    /// the caller treats a failure as non-fatal (the WS `ChannelAck` echo,
    /// or the next `Ready` resync, repairs the marker).
    pub async fn ack(&self, channel_id: &str, message_id: &str) -> Result<(), String> {
        let url = format!(
            "{}/api/channels/{}/ack/{}",
            self.base_url(),
            channel_id,
            message_id
        );
        http_log::log_request("PUT", &url);
        let resp = self
            .http
            .put(&url)
            .headers(self.auth_headers()?)
            .send()
            .await
            .map_err(|e| http_log::network_error("PUT", &url, e))?;
        http_log::check_status("PUT", &url, resp).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unreads_with_invented_data() {
        // Fully invented ids — no real instance data.
        let json = r#"[
            {"_id":{"channel":"C0001","user":"U0001"},"last_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV"},
            {"_id":{"channel":"C0002","user":"U0001"}}
        ]"#;
        let parsed: Vec<ChannelUnread> = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.len(), 2);
        let (ch, last) = parsed[0].clone().into_pair();
        assert_eq!(ch, "C0001");
        assert_eq!(last.as_deref(), Some("01ARZ3NDEKTSV4RRFFQ69G5FAV"));
        // A tracked-but-never-read channel carries no last_id.
        let (ch2, last2) = parsed[1].clone().into_pair();
        assert_eq!(ch2, "C0002");
        assert!(last2.is_none());
    }
}
