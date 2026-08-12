//! issue↔userstory conversion primitives.
//!
//! - [`delete_item`] removes the source item once migration succeeds.
//! - [`fetch_id_name_map`] / [`fetch_raw_detail`] feed the "what has no
//!   equivalent on the target type" note; both are best-effort.
//!
//! There is deliberately no "promote issue to user story" primitive: Taiga's
//! native `promote_to_us` endpoint is missing on some deployments (returns a
//! plain-HTML 404), so both conversion directions create the target through
//! the normal create endpoint instead — see `adapter::item::convert`.

use std::collections::HashMap;

use not_yet_done_content::http_log;
use serde_json::Value;

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
        .send_retrying("GET", &url, || {
            client.http.get(&url).headers(headers.clone())
        })
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
        .send_retrying("GET", &url, || {
            client.http.get(&url).headers(headers.clone())
        })
        .await?;
    let resp = http_log::check_status("GET", &url, resp).await?;
    resp.json()
        .await
        .map_err(|e| format!("raw detail parse: {e}"))
}
