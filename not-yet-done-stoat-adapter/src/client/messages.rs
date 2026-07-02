//! Message history (REST pull side of the tree).
//!
//! The gateway gives us the server/channel structure via `Ready`, but
//! message bodies are pulled on demand from
//! `GET /api/channels/{id}/messages`. We always pass `include_users=true`
//! so author names resolve from the same round-trip instead of a second
//! `/users/{id}` request per author. Message `_id`s are ULIDs, so the
//! creation timestamp is decoded from the id itself — Revolt carries no
//! separate `created_at` field.
//!
//! Phase 1 fetches the most-recent page (`sort=Latest`). Backfill of
//! older messages (cursor `before=<ulid>`) is accepted as a parameter
//! here but not yet wired into the TUI (see `project_cursor_pagination_plan`).

use serde::Deserialize;

use not_yet_done_content::http_log;

use super::StoatClient;

/// Hard ceiling the Revolt API enforces on `limit`.
const MAX_MESSAGE_LIMIT: u32 = 100;

/// A message flattened for the tree: content plus the bits the UI shows
/// (author name, timestamp, edited flag). Authors are resolved to a
/// display name at fetch time; the raw `author_id` is kept for metadata.
#[derive(Debug, Clone)]
pub struct MessageView {
    pub id: String,
    pub channel_id: String,
    pub content: String,
    pub author_id: String,
    pub author_name: String,
    pub edited: bool,
    pub timestamp_ms: Option<u64>,
    /// Uploaded files carried by the message. Rendered as placeholder
    /// lines in the body (the terminal can't show the image inline); each
    /// image/file is openable via link-hop when its `url` resolved.
    pub attachments: Vec<Attachment>,
}

/// One uploaded file on a message, resolved for display.
#[derive(Debug, Clone)]
pub struct Attachment {
    pub filename: String,
    /// Absolute download URL (`{autumn}/{tag}/{id}/{filename}`), or `None`
    /// when the autumn base URL hasn't been discovered yet.
    pub url: Option<String>,
    /// True for image attachments (metadata `type: Image` or an
    /// `image/*` content type) — drives the 🖼 vs 📎 placeholder glyph.
    pub is_image: bool,
}

#[derive(Deserialize)]
struct MessagesWithUsers {
    #[serde(default)]
    messages: Vec<RawMessage>,
    #[serde(default)]
    users: Vec<RawUser>,
}

#[derive(Deserialize)]
struct RawMessage {
    #[serde(rename = "_id")]
    id: String,
    #[serde(default)]
    channel: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    content: Option<String>,
    /// Present (an ISO timestamp) when the message has been edited; we
    /// only care that it exists.
    #[serde(default)]
    edited: Option<serde_json::Value>,
    /// Present for system messages (join/leave/topic-change …), which
    /// carry no `content`.
    #[serde(default)]
    system: Option<serde_json::Value>,
    /// Uploaded files (Revolt `File` objects). Empty for most messages.
    #[serde(default)]
    attachments: Vec<RawFile>,
}

/// A Revolt `File` object as it appears in a message's `attachments`.
#[derive(Deserialize)]
struct RawFile {
    #[serde(rename = "_id")]
    id: String,
    /// Storage bucket (`attachments`, `avatars`, …). Part of the URL path.
    #[serde(default)]
    tag: String,
    #[serde(default)]
    filename: String,
    #[serde(default)]
    content_type: String,
    #[serde(default)]
    metadata: RawFileMetadata,
}

#[derive(Deserialize, Default)]
struct RawFileMetadata {
    /// `Image`, `Video`, `Audio`, `File`, …
    #[serde(rename = "type", default)]
    kind: String,
}

impl RawFile {
    /// Resolve into a display [`Attachment`]. Builds the download URL from
    /// the autumn base when known: `{autumn}/{tag}/{id}/{filename}` with the
    /// filename percent-encoded so spaces/unicode stay a valid URL.
    fn into_attachment(self, autumn: Option<&str>) -> Attachment {
        let is_image =
            self.metadata.kind == "Image" || self.content_type.starts_with("image/");
        let tag = if self.tag.is_empty() {
            "attachments"
        } else {
            &self.tag
        };
        let url = autumn.map(|base| {
            format!(
                "{}/{}/{}/{}",
                base.trim_end_matches('/'),
                tag,
                self.id,
                percent_encode_segment(&self.filename),
            )
        });
        Attachment {
            filename: self.filename,
            url,
            is_image,
        }
    }
}

#[derive(Deserialize)]
struct RawUser {
    #[serde(rename = "_id")]
    id: String,
    #[serde(default)]
    username: String,
}

impl RawMessage {
    /// Resolve into a `MessageView`, looking the author up in `name_of`
    /// (falling back to the raw id) and decoding the ULID timestamp.
    /// `autumn` is the file server base URL used to build attachment links;
    /// pass `None` before discovery has run.
    fn into_view(
        self,
        name_of: &dyn Fn(&str) -> Option<String>,
        autumn: Option<&str>,
    ) -> MessageView {
        let content = match self.content {
            Some(c) if !c.is_empty() => c,
            // System messages have no body; show a stable placeholder so
            // the row isn't blank.
            _ if self.system.is_some() => "[system message]".to_string(),
            _ => String::new(),
        };
        let author_name = name_of(&self.author).unwrap_or_else(|| self.author.clone());
        let timestamp_ms = ulid_timestamp_ms(&self.id);
        let attachments = self
            .attachments
            .into_iter()
            .map(|f| f.into_attachment(autumn))
            .collect();
        MessageView {
            id: self.id,
            channel_id: self.channel,
            content,
            author_id: self.author,
            author_name,
            edited: self.edited.is_some(),
            timestamp_ms,
            attachments,
        }
    }
}

impl StoatClient {
    /// Fetch a page of messages for a channel, newest-first from the
    /// server, returned **oldest-first** (chat convention: newest at the
    /// bottom). `before` requests messages strictly older than the given
    /// ULID (for backfill); `None` fetches the latest page.
    pub async fn list_messages(
        &self,
        channel_id: &str,
        limit: u32,
        before: Option<&str>,
    ) -> Result<Vec<MessageView>, String> {
        let limit = limit.clamp(1, MAX_MESSAGE_LIMIT).to_string();
        let url = format!("{}/api/channels/{}/messages", self.base_url(), channel_id);
        http_log::log_request("GET", &url);

        let mut query: Vec<(&str, &str)> = vec![
            ("limit", limit.as_str()),
            ("sort", "Latest"),
            ("include_users", "true"),
        ];
        if let Some(b) = before {
            query.push(("before", b));
        }

        let resp = self
            .http
            .get(&url)
            .headers(self.auth_headers()?)
            .query(&query)
            .send()
            .await
            .map_err(|e| http_log::network_error("GET", &url, e))?;
        let resp = http_log::check_status("GET", &url, resp).await?;
        let body = resp
            .json::<MessagesWithUsers>()
            .await
            .map_err(|e| format!("parse messages: {e}"))?;

        let names: std::collections::HashMap<String, String> = body
            .users
            .into_iter()
            .map(|u| (u.id, u.username))
            .collect();
        let name_of = |id: &str| names.get(id).filter(|n| !n.is_empty()).cloned();

        let autumn = self.autumn_url();
        let mut views: Vec<MessageView> = body
            .messages
            .into_iter()
            .map(|m| m.into_view(&name_of, autumn))
            .collect();
        // Server returns newest-first; flip so the newest sits at the
        // bottom of the list, like every chat client.
        views.reverse();
        Ok(views)
    }

    /// Post a new message to `channel_id`. Returns the created message's
    /// id. `nonce` is intentionally omitted — the Revolt API treats it as
    /// optional client-side idempotency (verified against the live
    /// instance: a body of just `{content}` is accepted), and the gateway
    /// echoes the new message back as a live event regardless.
    pub async fn send_message(&self, channel_id: &str, content: &str) -> Result<String, String> {
        let url = format!("{}/api/channels/{}/messages", self.base_url(), channel_id);
        http_log::log_request("POST", &url);
        let resp = self
            .http
            .post(&url)
            .headers(self.auth_headers()?)
            .json(&serde_json::json!({ "content": content }))
            .send()
            .await
            .map_err(|e| http_log::network_error("POST", &url, e))?;
        let resp = http_log::check_status("POST", &url, resp).await?;
        let raw = resp
            .json::<RawMessage>()
            .await
            .map_err(|e| format!("parse sent message: {e}"))?;
        Ok(raw.id)
    }

    /// Edit a message's `content` in place via `PATCH`. The server rejects
    /// edits to other users' messages (403); the caller surfaces that as a
    /// clean error rather than filtering the action per-instance.
    pub async fn edit_message(
        &self,
        channel_id: &str,
        message_id: &str,
        content: &str,
    ) -> Result<(), String> {
        let url = format!(
            "{}/api/channels/{}/messages/{}",
            self.base_url(),
            channel_id,
            message_id
        );
        http_log::log_request("PATCH", &url);
        let resp = self
            .http
            .patch(&url)
            .headers(self.auth_headers()?)
            .json(&serde_json::json!({ "content": content }))
            .send()
            .await
            .map_err(|e| http_log::network_error("PATCH", &url, e))?;
        http_log::check_status("PATCH", &url, resp).await?;
        Ok(())
    }

    /// Delete a message. Returns `Ok(())` on the API's `204 No Content`.
    pub async fn delete_message(
        &self,
        channel_id: &str,
        message_id: &str,
    ) -> Result<(), String> {
        let url = format!(
            "{}/api/channels/{}/messages/{}",
            self.base_url(),
            channel_id,
            message_id
        );
        http_log::log_request("DELETE", &url);
        let resp = self
            .http
            .delete(&url)
            .headers(self.auth_headers()?)
            .send()
            .await
            .map_err(|e| http_log::network_error("DELETE", &url, e))?;
        http_log::check_status("DELETE", &url, resp).await?;
        Ok(())
    }

    /// Add an emoji reaction to a message (`PUT …/reactions/{emoji}`). The
    /// emoji travels as a **path segment**, so its UTF-8 bytes are
    /// percent-encoded (a raw `👍` in a URL path is invalid). Unicode
    /// emoji and custom-emoji ids both work; the API answers `204`.
    pub async fn add_reaction(
        &self,
        channel_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<(), String> {
        let url = format!(
            "{}/api/channels/{}/messages/{}/reactions/{}",
            self.base_url(),
            channel_id,
            message_id,
            percent_encode_segment(emoji)
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

    /// Fetch a single message by id. Used by the preview path, which
    /// only has the composite node id to work with and no `users[]`
    /// array — the caller supplies the author display name it resolved
    /// from `StoatState`.
    pub async fn fetch_message(
        &self,
        channel_id: &str,
        message_id: &str,
        author_name: Option<String>,
    ) -> Result<MessageView, String> {
        let url = format!(
            "{}/api/channels/{}/messages/{}",
            self.base_url(),
            channel_id,
            message_id
        );
        http_log::log_request("GET", &url);
        let resp = self
            .http
            .get(&url)
            .headers(self.auth_headers()?)
            .send()
            .await
            .map_err(|e| http_log::network_error("GET", &url, e))?;
        let resp = http_log::check_status("GET", &url, resp).await?;
        let raw = resp
            .json::<RawMessage>()
            .await
            .map_err(|e| format!("parse message: {e}"))?;
        let author = author_name.clone();
        Ok(raw.into_view(&|_id| author.clone(), self.autumn_url()))
    }
}

/// Percent-encode a string for use as a single URL **path segment**.
/// Keeps the unreserved set (`A–Z a–z 0–9 - _ . ~`) verbatim and escapes
/// every other byte as `%XX` — enough to carry a UTF-8 emoji safely in
/// `…/reactions/{emoji}` without pulling in a URL-encoding dependency.
fn percent_encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Decode the 48-bit millisecond timestamp embedded in a ULID's first 10
/// Crockford-base32 characters. Returns `None` if the id is too short or
/// contains a non-alphabet character (e.g. a non-ULID id), so callers can
/// gracefully omit the timestamp rather than fail the whole listing.
pub fn ulid_timestamp_ms(id: &str) -> Option<u64> {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    if id.len() < 10 {
        return None;
    }
    let mut ts: u64 = 0;
    for c in id.as_bytes()[..10].iter() {
        let up = c.to_ascii_uppercase();
        let val = ALPHABET.iter().position(|&a| a == up)? as u64;
        ts = ts.checked_mul(32)?.checked_add(val)?;
    }
    Some(ts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encode_keeps_unreserved_and_escapes_emoji() {
        assert_eq!(percent_encode_segment("aZ0-_.~"), "aZ0-_.~");
        // 👍 is U+1F44D → UTF-8 F0 9F 91 8D.
        assert_eq!(percent_encode_segment("👍"), "%F0%9F%91%8D");
        // ASCII reserved chars escape too.
        assert_eq!(percent_encode_segment("a/b"), "a%2Fb");
    }

    #[test]
    fn ulid_timestamp_decodes_known_value() {
        // First 10 chars "01ARZ3NDEK" decode (Crockford base32) to
        // 1469922850259 ms ≈ 2016-07-31 00:34 UTC.
        let ms = ulid_timestamp_ms("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
        assert_eq!(ms, 1469922850259);
    }

    #[test]
    fn ulid_timestamp_rejects_short_and_invalid() {
        assert!(ulid_timestamp_ms("short").is_none());
        // 'I' / 'L' / 'O' / 'U' are excluded from Crockford base32.
        assert!(ulid_timestamp_ms("IIIIIIIIII0000000000000000").is_none());
    }

    #[test]
    fn raw_message_into_view_resolves_author_and_orders() {
        // Fully invented payload — no real instance data.
        // Server sorts newest-first (`sort=Latest`); "world" (higher
        // ULID) precedes "hello". list_messages reverses to oldest-first.
        let body: MessagesWithUsers = serde_json::from_str(
            r#"{
                "messages": [
                    {"_id":"01ARZ3NDEKTSV4RRFFQ69G5FAW","channel":"C1","author":"U2","content":"world","edited":"2026-01-01T00:00:00Z"},
                    {"_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","channel":"C1","author":"U1","content":"hello"}
                ],
                "users": [
                    {"_id":"U1","username":"alice"},
                    {"_id":"U2","username":"bob"}
                ]
            }"#,
        )
        .unwrap();
        let names: std::collections::HashMap<String, String> =
            body.users.into_iter().map(|u| (u.id, u.username)).collect();
        let name_of = |id: &str| names.get(id).cloned();
        let mut views: Vec<MessageView> =
            body.messages.into_iter().map(|m| m.into_view(&name_of, None)).collect();
        views.reverse();
        // Reversed → oldest first.
        assert_eq!(views[0].author_name, "alice");
        assert_eq!(views[0].content, "hello");
        assert!(!views[0].edited);
        assert_eq!(views[1].author_name, "bob");
        assert!(views[1].edited);
        assert!(views[0].timestamp_ms.is_some());
    }

    #[test]
    fn attachment_builds_url_and_flags_image() {
        // Fully invented payload — no real instance data.
        let raw: RawMessage = serde_json::from_str(
            r#"{
                "_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","channel":"C1","author":"U1","content":"see this",
                "attachments":[
                    {"_id":"F1","tag":"attachments","filename":"my photo.png","content_type":"image/png","metadata":{"type":"Image"}},
                    {"_id":"F2","tag":"attachments","filename":"notes.pdf","content_type":"application/pdf","metadata":{"type":"File"}}
                ]
            }"#,
        )
        .unwrap();
        let view = raw.into_view(&|_| None, Some("https://autumn.example/"));
        assert_eq!(view.attachments.len(), 2);
        let img = &view.attachments[0];
        assert!(img.is_image);
        // Trailing slash on the base is trimmed; the space is percent-encoded.
        assert_eq!(
            img.url.as_deref(),
            Some("https://autumn.example/attachments/F1/my%20photo.png")
        );
        assert!(!view.attachments[1].is_image);
    }

    #[test]
    fn attachment_url_is_none_before_discovery() {
        let raw: RawMessage = serde_json::from_str(
            r#"{"_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","channel":"C1","author":"U1",
                "attachments":[{"_id":"F1","filename":"x.png","content_type":"image/png"}]}"#,
        )
        .unwrap();
        // No autumn base yet → the attachment is listed but has no link, and
        // an empty `tag` falls back to the `attachments` bucket.
        let view = raw.into_view(&|_| None, None);
        assert_eq!(view.attachments.len(), 1);
        assert!(view.attachments[0].url.is_none());
        assert!(view.attachments[0].is_image);
    }

    #[test]
    fn system_message_gets_placeholder() {
        let raw: RawMessage = serde_json::from_str(
            r#"{"_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","channel":"C1","author":"U1","system":{"type":"user_joined"}}"#,
        )
        .unwrap();
        let view = raw.into_view(&|_| None, None);
        assert_eq!(view.content, "[system message]");
        // Author falls back to the raw id when unresolved.
        assert_eq!(view.author_name, "U1");
    }
}
