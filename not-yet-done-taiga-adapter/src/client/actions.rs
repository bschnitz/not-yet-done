//! Write-side endpoints: watch/unwatch, edit a history-entry comment,
//! list and download attachments. Read-only listing is in [`super::query`];
//! detail-fetching for items lives in `adapter::item`.

use not_yet_done_content::http_log;
use serde::Deserialize;
use serde_json::json;

use super::TaigaClient;
use super::query::ItemType;

// ---------------------------------------------------------------------------
// Watch / unwatch
// ---------------------------------------------------------------------------

/// Toggle the watch status of an item. Reads the current `is_watcher`
/// flag from a fresh GET, then POSTs to `/watch` or `/unwatch`. Returns
/// the **new** state.
pub async fn toggle_watch(
    client: &TaigaClient,
    item_type: ItemType,
    item_id: u64,
) -> Result<bool, String> {
    let detail_url = format!(
        "{}/api/v1/{}/{item_id}",
        client.base_url,
        item_type.url_segment(),
    );
    let headers = client.auth_headers()?;
    http_log::log_request("GET", &detail_url);
    let detail_resp = client
        .send_retrying("GET", &detail_url, || {
            client.http.get(&detail_url).headers(headers.clone())
        })
        .await?;
    let detail_resp = http_log::check_status("GET", &detail_url, detail_resp).await?;
    let detail: serde_json::Value = detail_resp
        .json()
        .await
        .map_err(|e| format!("watch precheck parse: {e}"))?;
    let was_watching = detail
        .get("is_watcher")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);

    let action_path = if was_watching { "unwatch" } else { "watch" };
    let url = format!(
        "{}/api/v1/{}/{item_id}/{action_path}",
        client.base_url,
        item_type.url_segment(),
    );
    http_log::log_request("POST", &url);
    let resp = client
        .send_retrying("POST", &url, || client.http.post(&url).headers(headers.clone()))
        .await?;
    http_log::check_status("POST", &url, resp).await?;
    Ok(!was_watching)
}

// ---------------------------------------------------------------------------
// Edit a history-entry comment
// ---------------------------------------------------------------------------

/// Replace the body of an existing comment. Comment IDs are the
/// stringified history-entry UUIDs returned by [`super::fetch_comments`].
pub async fn edit_comment(
    client: &TaigaClient,
    item_type: ItemType,
    item_id: u64,
    comment_id: &str,
    new_body: &str,
) -> Result<(), String> {
    let url = format!(
        "{}/api/v1/history/{}/{item_id}/edit_comment?id={}",
        client.base_url,
        item_type.history_segment(),
        urlencode(comment_id),
    );
    let headers = client.auth_headers()?;
    let payload = json!({ "comment": new_body });
    http_log::log_request("POST", &url);
    let resp = client
        .send_retrying("POST", &url, || {
            client.http.post(&url).headers(headers.clone()).json(&payload)
        })
        .await?;
    http_log::check_status("POST", &url, resp).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Attachments
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct TaigaAttachment {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub description: String,
    pub created_date: String,
    pub modified_date: String,
    pub owner: u64,
    /// Storage URL — typically served by nginx/S3, not gated by the
    /// bearer token. Sending the auth header on download is harmless.
    pub url: String,
    pub thumbnail_url: Option<String>,
}

#[derive(Deserialize)]
struct AttachmentDto {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    description: String,
    #[serde(default)]
    created_date: String,
    #[serde(default)]
    modified_date: String,
    #[serde(default)]
    owner: u64,
    #[serde(default)]
    url: String,
    #[serde(default)]
    attached_file: String,
    #[serde(default)]
    thumbnail_card_url: Option<String>,
}

/// List attachments for one item. Both `object_id` and `project` filters
/// are required by the Taiga router for these endpoints.
pub async fn list_attachments(
    client: &TaigaClient,
    item_type: ItemType,
    item_id: u64,
    project_id: u64,
) -> Result<Vec<TaigaAttachment>, String> {
    let url = format!(
        "{}/api/v1/{}/attachments?object_id={item_id}&project={project_id}",
        client.base_url,
        item_type.url_segment(),
    );
    let headers = client.auth_headers()?;
    http_log::log_request("GET", &url);
    let resp = client
        .send_retrying("GET", &url, || client.http.get(&url).headers(headers.clone()))
        .await?;
    let resp = http_log::check_status("GET", &url, resp).await?;
    let dtos: Vec<AttachmentDto> = resp
        .json()
        .await
        .map_err(|e| format!("attachments parse: {e}"))?;
    Ok(dtos
        .into_iter()
        .map(|d| TaigaAttachment {
            id: d.id,
            name: d.name,
            size: d.size,
            description: d.description,
            created_date: d.created_date,
            modified_date: d.modified_date,
            owner: d.owner,
            // Prefer `url` (cache-busted) but fall back to raw storage URL.
            url: if d.url.is_empty() { d.attached_file } else { d.url },
            thumbnail_url: d.thumbnail_card_url,
        })
        .collect())
}

/// Upload a single file as an attachment on the given item. Taiga accepts
/// one file per request, so callers loop for multi-select.
pub async fn upload_attachment(
    client: &TaigaClient,
    item_type: ItemType,
    item_id: u64,
    project_id: u64,
    file_path: &std::path::Path,
) -> Result<TaigaAttachment, String> {
    let bytes = tokio::fs::read(file_path)
        .await
        .map_err(|e| format!("read {}: {e}", file_path.display()))?;
    let filename = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("non-UTF-8 filename: {}", file_path.display()))?
        .to_string();

    let part = reqwest::multipart::Part::bytes(bytes).file_name(filename.clone());
    let form = reqwest::multipart::Form::new()
        .part("attached_file", part)
        .text("project", project_id.to_string())
        .text("object_id", item_id.to_string());

    let url = format!(
        "{}/api/v1/{}/attachments",
        client.base_url,
        item_type.url_segment(),
    );
    http_log::log_request("POST", &url);
    let resp = client
        .http
        .post(&url)
        .headers(client.auth_headers()?)
        .multipart(form)
        .send()
        .await
        .map_err(|e| http_log::network_error("POST", &url, e))?;
    let resp = http_log::check_status("POST", &url, resp).await?;
    let dto: AttachmentDto = resp
        .json()
        .await
        .map_err(|e| format!("upload response parse: {e}"))?;
    Ok(TaigaAttachment {
        id: dto.id,
        name: dto.name,
        size: dto.size,
        description: dto.description,
        created_date: dto.created_date,
        modified_date: dto.modified_date,
        owner: dto.owner,
        url: if dto.url.is_empty() { dto.attached_file } else { dto.url },
        thumbnail_url: dto.thumbnail_card_url,
    })
}

/// GET the binary content of an attachment.
pub async fn download_attachment(
    client: &TaigaClient,
    url: &str,
) -> Result<Vec<u8>, String> {
    let headers = client.auth_headers()?;
    http_log::log_request("GET", url);
    let resp = client
        .send_retrying("GET", url, || client.http.get(url).headers(headers.clone()))
        .await?;
    let resp = http_log::check_status("GET", url, resp).await?;
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("download body: {e}"))?;
    Ok(bytes.to_vec())
}

/// Minimal URL-encoder for query-string values. Same shape as the one in
/// `query.rs`; kept private here to avoid pub-leaking that helper.
pub(super) fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(ch),
            ' ' => out.push_str("%20"),
            other => {
                let mut buf = [0u8; 4];
                let s = other.encode_utf8(&mut buf);
                for b in s.bytes() {
                    out.push_str(&format!("%{b:02X}"));
                }
            }
        }
    }
    out
}
