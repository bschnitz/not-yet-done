//! issue↔userstory conversion primitives.
//!
//! - [`delete_item`] removes the source item once migration succeeds.
//! - [`promote_issue_to_us`] uses Taiga's native `promote_to_us` endpoint to
//!   turn an issue into a user story, then resolves the resulting id.
//! - [`userstory_id_by_ref`] maps a `ref` back to a numeric id.
//! - [`fetch_id_name_map`] / [`fetch_raw_detail`] feed the "what has no
//!   equivalent on the target type" note; both are best-effort.

use std::collections::HashMap;

use not_yet_done_content::http_log;
use serde_json::{Value, json};

use super::TaigaClient;
use super::query::ItemType;

/// `DELETE /api/v1/{seg}/{id}`. Returns `Ok(())` on any success status.
pub async fn delete_item(
    client: &TaigaClient,
    item_type: ItemType,
    item_id: u64,
) -> Result<(), String> {
    let url = format!(
        "{}/api/v1/{}/{item_id}",
        client.base_url,
        item_type.url_segment(),
    );
    let headers = client.auth_headers()?;
    http_log::log_request("DELETE", &url);
    let resp = client
        .send_retrying("DELETE", &url, || {
            client.http.delete(&url).headers(headers.clone())
        })
        .await?;
    http_log::check_status("DELETE", &url, resp).await?;
    Ok(())
}

/// Natively promote an issue to a user story via
/// `POST /api/v1/issues/{id}/promote_to_us` and return the new user story's
/// numeric id.
///
/// The response shape has drifted across Taiga versions — current servers
/// return `[ref, …]` (a list of the generated user story refs), but older or
/// customised deployments may return a bare number or an object carrying
/// `id`/`ref`. We parse all of these defensively; when only a `ref` is
/// available we resolve it to an id via [`userstory_id_by_ref`].
pub async fn promote_issue_to_us(
    client: &TaigaClient,
    issue_id: u64,
    project_id: u64,
) -> Result<u64, String> {
    let url = format!("{}/api/v1/issues/{issue_id}/promote_to_us", client.base_url);
    let headers = client.auth_headers()?;
    // `project_id` is harmless if ignored and required by some versions.
    let payload = json!({ "project_id": project_id });
    http_log::log_request("POST", &url);
    let resp = client
        .send_retrying("POST", &url, || {
            client.http.post(&url).headers(headers.clone()).json(&payload)
        })
        .await?;
    let status = resp.status();
    http_log::log_response("POST", &url, status.as_u16());
    let body_text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let msg = format!("POST {url} -> {status}: {body_text}");
        http_log::log_error("POST", &msg);
        return Err(msg);
    }
    let v: Value = serde_json::from_str(&body_text)
        .map_err(|e| format!("promote_to_us parse: {e}"))?;
    resolve_promoted_id(client, project_id, &v).await
}

/// Extract the promoted user story's id from the (version-dependent)
/// `promote_to_us` response body.
async fn resolve_promoted_id(
    client: &TaigaClient,
    project_id: u64,
    v: &Value,
) -> Result<u64, String> {
    // Object with a direct id.
    if let Some(id) = v.get("id").and_then(|x| x.as_u64()) {
        return Ok(id);
    }
    // Array: first element is either a ref (number) or an object {id|ref}.
    if let Some(first) = v.as_array().and_then(|a| a.first()) {
        if let Some(id) = first.get("id").and_then(|x| x.as_u64()) {
            return Ok(id);
        }
        if let Some(r) = first
            .as_u64()
            .or_else(|| first.get("ref").and_then(|x| x.as_u64()))
        {
            return userstory_id_by_ref(client, project_id, r).await;
        }
    }
    // Bare number = ref.
    if let Some(r) = v.as_u64() {
        return userstory_id_by_ref(client, project_id, r).await;
    }
    // Object with a ref.
    if let Some(r) = v.get("ref").and_then(|x| x.as_u64()) {
        return userstory_id_by_ref(client, project_id, r).await;
    }
    Err(format!(
        "promote_to_us: could not extract a user story id/ref from response: {v}"
    ))
}

/// Resolve a user story `ref` (project-scoped, human-facing number) to its
/// numeric id via `GET /api/v1/userstories/by_ref`.
pub async fn userstory_id_by_ref(
    client: &TaigaClient,
    project_id: u64,
    ref_num: u64,
) -> Result<u64, String> {
    let url = format!(
        "{}/api/v1/userstories/by_ref?project={project_id}&ref={ref_num}",
        client.base_url,
    );
    let headers = client.auth_headers()?;
    http_log::log_request("GET", &url);
    let resp = client
        .send_retrying("GET", &url, || client.http.get(&url).headers(headers.clone()))
        .await?;
    let resp = http_log::check_status("GET", &url, resp).await?;
    let v: Value = resp
        .json()
        .await
        .map_err(|e| format!("userstories/by_ref parse: {e}"))?;
    v.get("id")
        .and_then(|x| x.as_u64())
        .ok_or_else(|| format!("userstories/by_ref: no id for ref {ref_num}"))
}

/// Fetch a small `id → name` map from a project-scoped catalogue endpoint
/// (e.g. `issue-types`, `severities`, `priorities`). Best-effort: any error
/// yields an empty map so the caller (a display-only note) never fails.
pub async fn fetch_id_name_map(
    client: &TaigaClient,
    endpoint_seg: &str,
    project_id: u64,
) -> HashMap<u64, String> {
    let url = format!(
        "{}/api/v1/{endpoint_seg}?project={project_id}",
        client.base_url,
    );
    let headers = match client.auth_headers() {
        Ok(h) => h,
        Err(_) => return HashMap::new(),
    };
    http_log::log_request("GET", &url);
    let resp = match client
        .send_retrying("GET", &url, || client.http.get(&url).headers(headers.clone()))
        .await
    {
        Ok(r) => r,
        Err(_) => return HashMap::new(),
    };
    let resp = match http_log::check_status("GET", &url, resp).await {
        Ok(r) => r,
        Err(_) => return HashMap::new(),
    };
    let raw: Vec<Value> = match resp.json().await {
        Ok(r) => r,
        Err(_) => return HashMap::new(),
    };
    raw.into_iter()
        .filter_map(|v| {
            let id = v.get("id").and_then(|x| x.as_u64())?;
            let name = v.get("name").and_then(|x| x.as_str())?.to_string();
            Some((id, name))
        })
        .collect()
}

/// GET the raw detail JSON of an item — used to read type-specific fields
/// (severity/priority/points/…) that [`crate::adapter`]'s structured
/// `ItemDetail` intentionally omits.
pub async fn fetch_raw_detail(
    client: &TaigaClient,
    item_type: ItemType,
    item_id: u64,
) -> Result<Value, String> {
    let url = format!(
        "{}/api/v1/{}/{item_id}",
        client.base_url,
        item_type.url_segment(),
    );
    let headers = client.auth_headers()?;
    http_log::log_request("GET", &url);
    let resp = client
        .send_retrying("GET", &url, || client.http.get(&url).headers(headers.clone()))
        .await?;
    let resp = http_log::check_status("GET", &url, resp).await?;
    resp.json()
        .await
        .map_err(|e| format!("raw detail parse: {e}"))
}
