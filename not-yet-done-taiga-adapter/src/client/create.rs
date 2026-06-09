//! POST endpoint for creating new items (clone target).
//!
//! Status is omitted from the payload so Taiga assigns the project's
//! default. Assignee likewise omitted (or `None`) → unassigned.

use not_yet_done_content::http_log;
use serde_json::{Map, Value, json};

use super::TaigaClient;
use super::query::ItemType;

#[derive(Default, Debug)]
pub struct CreateFields {
    pub project_id: u64,
    pub subject: String,
    pub description: String,
    pub tags: Vec<String>,
    /// Tasks only: parent user_story id. Ignored for other item types.
    pub user_story_id: Option<u64>,
    /// `Some(id)` sets the primary assignee; `None` → unassigned. Set to
    /// `assigned_users.first()` for legacy-single-assignee compatibility.
    pub assigned_to: Option<u64>,
    /// Full multi-assignee list. Empty Vec → no assignees.
    pub assigned_users: Vec<u64>,
}

#[derive(Debug)]
pub struct CreatedItem {
    pub id: u64,
    pub r#ref: u64,
}

pub async fn create_item(
    client: &TaigaClient,
    item_type: ItemType,
    fields: CreateFields,
) -> Result<CreatedItem, String> {
    let url = format!(
        "{}/api/v1/{}",
        client.base_url,
        item_type.url_segment(),
    );

    let mut body: Map<String, Value> = Map::new();
    body.insert("project".into(), json!(fields.project_id));
    body.insert("subject".into(), json!(fields.subject));
    if !fields.description.is_empty() {
        body.insert("description".into(), json!(fields.description));
    }
    if !fields.tags.is_empty() {
        let arr: Vec<Value> = fields
            .tags
            .iter()
            .map(|name| json!([name, Value::Null]))
            .collect();
        body.insert("tags".into(), Value::Array(arr));
    }
    if let Some(id) = fields.assigned_to {
        body.insert("assigned_to".into(), json!(id));
    }
    if !fields.assigned_users.is_empty() {
        body.insert(
            "assigned_users".into(),
            Value::Array(fields.assigned_users.iter().map(|id| json!(id)).collect()),
        );
    }
    if matches!(item_type, ItemType::Task) {
        if let Some(us) = fields.user_story_id {
            body.insert("user_story".into(), json!(us));
        }
    }

    let headers = client.auth_headers()?;
    let payload = Value::Object(body);
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
        .map_err(|e| format!("POST parse: {e}"))?;
    let id = v
        .get("id")
        .and_then(|x| x.as_u64())
        .ok_or_else(|| "create response missing `id`".to_string())?;
    let r#ref = v
        .get("ref")
        .and_then(|x| x.as_u64())
        .ok_or_else(|| "create response missing `ref`".to_string())?;
    Ok(CreatedItem { id, r#ref })
}
