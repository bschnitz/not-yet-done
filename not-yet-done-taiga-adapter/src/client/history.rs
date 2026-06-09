//! History endpoint — comments live in `/history/{type}/{id}` mixed with
//! all other change events. We filter for entries with non-empty
//! `comment` field.

use not_yet_done_content::http_log;

use super::TaigaClient;
use super::query::ItemType;

#[derive(Clone, Debug)]
pub struct TaigaComment {
    pub id: String,
    /// Display name (or `username` if name is empty). Surfaced in the UI.
    pub author: String,
    /// Authoritative `user.username` for ownership checks against the
    /// authenticated user. Optional because some history entries elide
    /// the user object (e.g. system events).
    pub author_username: Option<String>,
    pub created: String,
    pub body: String,
}

impl ItemType {
    /// Singular path segment for the history endpoint:
    /// `task`, `issue`, `epic`, `userstory`.
    pub fn history_segment(self) -> &'static str {
        match self {
            ItemType::Task => "task",
            ItemType::Issue => "issue",
            ItemType::Epic => "epic",
            ItemType::UserStory => "userstory",
        }
    }
}

pub async fn fetch_comments(
    client: &TaigaClient,
    item_type: ItemType,
    item_id: u64,
) -> Result<Vec<TaigaComment>, String> {
    let url = format!(
        "{}/api/v1/history/{}/{}",
        client.base_url,
        item_type.history_segment(),
        item_id,
    );
    let headers = client.auth_headers()?;
    http_log::log_request("GET", &url);
    let resp = client
        .send_retrying("GET", &url, || client.http.get(&url).headers(headers.clone()))
        .await?;
    let resp = http_log::check_status("GET", &url, resp).await?;
    let raw: Vec<serde_json::Value> = resp
        .json()
        .await
        .map_err(|e| format!("history parse: {e}"))?;
    Ok(raw
        .into_iter()
        .filter_map(parse_comment)
        .collect())
}

fn parse_comment(entry: serde_json::Value) -> Option<TaigaComment> {
    let body = entry.get("comment")?.as_str()?.to_string();
    if body.is_empty() {
        return None;
    }
    if entry
        .get("delete_comment_date")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .is_some()
    {
        return None;
    }
    let id = entry
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let user_obj = entry.get("user");
    let author_username = user_obj
        .and_then(|u| u.get("username"))
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());
    let author = user_obj
        .and_then(|u| u.get("name"))
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| author_username.clone())
        .unwrap_or_default();
    let created = entry
        .get("created_at")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    Some(TaigaComment { id, author, author_username, created, body })
}
