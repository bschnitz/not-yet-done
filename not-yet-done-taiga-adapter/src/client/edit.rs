//! Write-side endpoints for item edits and comment lifecycle:
//! - PATCH item (with optimistic-locking `version`)
//! - DELETE comment (history entry)
//! - "add comment" via PATCH item with `comment` field
//!
//! [`super::actions::edit_comment`] handles existing-comment edits.

use not_yet_done_content::http_log;
use serde_json::{Map, Value, json};

use super::TaigaClient;
use super::actions::urlencode;
use super::query::ItemType;

/// Specifies which fields of `EditFields` should actually be sent in the
/// PATCH body. We can't rely on `Option<T>` alone because for tags the
/// empty `Vec` (`tags: []`) is a meaningful "clear all tags" payload, while
/// "do not touch tags" is a different state.
#[derive(Default, Debug)]
pub struct EditFields {
    pub subject: Option<String>,
    pub description: Option<String>,
    pub status_id: Option<u64>,
    /// `Some(None)` clears the primary assignee; `Some(Some(id))` sets it;
    /// `None` means "do not touch". Kept alongside `assigned_users` for
    /// legacy single-assignee compatibility — we set it to the first
    /// element of the new list (or `None` for an empty list).
    pub assigned_to: Option<Option<u64>>,
    /// `Some(vec)` replaces the full assignee set (empty vec clears all);
    /// `None` skips the field entirely.
    pub assigned_users: Option<Vec<u64>>,
    /// `Some(vec)` sets the tag list (empty vec clears); `None` skips the
    /// field entirely.
    pub tags: Option<Vec<String>>,
}

impl EditFields {
    pub fn is_empty(&self) -> bool {
        self.subject.is_none()
            && self.description.is_none()
            && self.status_id.is_none()
            && self.assigned_to.is_none()
            && self.assigned_users.is_none()
            && self.tags.is_none()
    }
}

/// One PATCH payload — used both for plain field edits and for "edit field
/// + add comment in the same call". Each PATCH consumes one `version`.
pub struct ItemPatch<'a> {
    pub item_type: ItemType,
    pub item_id: u64,
    pub version: u64,
    pub fields: &'a EditFields,
    pub comment: Option<&'a str>,
}

/// PATCH `/api/v1/{type}/{id}` with the supplied fields + version.
///
/// Returns the new `version` from the response so callers can chain
/// further updates without an extra GET. On version-conflict the server
/// returns 412 Precondition Failed; we surface that distinctly so the
/// adapter can map it to `ContentError::Conflict`.
pub async fn patch_item(
    client: &TaigaClient,
    patch: ItemPatch<'_>,
) -> Result<PatchOutcome, String> {
    let url = format!(
        "{}/api/v1/{}/{}",
        client.base_url,
        patch.item_type.url_segment(),
        patch.item_id,
    );

    let mut body: Map<String, Value> = Map::new();
    body.insert("version".into(), json!(patch.version));
    if let Some(s) = &patch.fields.subject {
        body.insert("subject".into(), json!(s));
    }
    if let Some(d) = &patch.fields.description {
        body.insert("description".into(), json!(d));
    }
    if let Some(id) = patch.fields.status_id {
        body.insert("status".into(), json!(id));
    }
    if let Some(opt) = patch.fields.assigned_to {
        match opt {
            Some(id) => body.insert("assigned_to".into(), json!(id)),
            None => body.insert("assigned_to".into(), Value::Null),
        };
    }
    if let Some(ids) = &patch.fields.assigned_users {
        body.insert(
            "assigned_users".into(),
            Value::Array(ids.iter().map(|id| json!(id)).collect()),
        );
    }
    if let Some(tags) = &patch.fields.tags {
        // Send `[["name", null], ...]` so Taiga preserves/auto-assigns colors.
        // Empty array is a legitimate "clear all tags" payload.
        let arr: Vec<Value> = tags
            .iter()
            .map(|name| json!([name, Value::Null]))
            .collect();
        body.insert("tags".into(), Value::Array(arr));
    }
    if let Some(c) = patch.comment {
        body.insert("comment".into(), json!(c));
    }

    let headers = client.auth_headers()?;
    let payload = Value::Object(body);
    http_log::log_request("PATCH", &url);
    let resp = client
        .send_retrying("PATCH", &url, || {
            client.http.patch(&url).headers(headers.clone()).json(&payload)
        })
        .await?;

    let status = resp.status();
    http_log::log_response("PATCH", &url, status.as_u16());
    let body_text = resp.text().await.unwrap_or_default();

    if status.as_u16() == 412 {
        return Ok(PatchOutcome::VersionConflict { server_message: body_text });
    }
    if !status.is_success() {
        let msg = format!("PATCH {url} -> {status}: {body_text}");
        http_log::log_error("PATCH", &msg);
        return Err(msg);
    }
    let v: Value = serde_json::from_str(&body_text)
        .map_err(|e| format!("PATCH parse: {e}"))?;
    let new_version = v
        .get("version")
        .and_then(|x| x.as_u64())
        .ok_or_else(|| "PATCH response missing `version`".to_string())?;
    Ok(PatchOutcome::Updated { new_version })
}

#[derive(Debug)]
pub enum PatchOutcome {
    Updated { new_version: u64 },
    VersionConflict { server_message: String },
}

/// Append a comment via PATCH (Taiga's "add comment" path is the same
/// endpoint as the field-edit; `comment` is just another updatable field).
/// Caller is responsible for passing a fresh `version`.
pub async fn add_comment(
    client: &TaigaClient,
    item_type: ItemType,
    item_id: u64,
    version: u64,
    body: &str,
) -> Result<PatchOutcome, String> {
    let empty = EditFields::default();
    patch_item(
        client,
        ItemPatch {
            item_type,
            item_id,
            version,
            fields: &empty,
            comment: Some(body),
        },
    )
    .await
}

/// DELETE a comment via the history endpoint.
pub async fn delete_comment(
    client: &TaigaClient,
    item_type: ItemType,
    item_id: u64,
    comment_id: &str,
) -> Result<(), String> {
    let url = format!(
        "{}/api/v1/history/{}/{item_id}/delete_comment?id={}",
        client.base_url,
        item_type.history_segment(),
        urlencode(comment_id),
    );
    let headers = client.auth_headers()?;
    http_log::log_request("POST", &url);
    let resp = client
        .send_retrying("POST", &url, || client.http.post(&url).headers(headers.clone()))
        .await?;
    http_log::check_status("POST", &url, resp).await?;
    Ok(())
}
