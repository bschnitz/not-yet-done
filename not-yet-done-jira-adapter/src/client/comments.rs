//! Issue comments: list, add, update, delete.

use serde::Deserialize;

use super::{Assignee, JiraClient, normalize_eol};

/// A Jira comment.
#[derive(Debug, Clone)]
pub struct JiraComment {
    pub id: String,
    pub author: String,
    /// Jira-username of the comment author (same value as inside `[~name]`).
    /// Empty when Jira didn't return one.
    pub author_key: String,
    pub body: String,
    pub created: String,
    pub updated: String,
}

#[derive(Deserialize)]
struct CommentIssueResponse {
    fields: CommentIssueFields,
}

#[derive(Deserialize)]
struct CommentIssueFields {
    comment: Option<CommentContainer>,
}

#[derive(Deserialize)]
struct CommentContainer {
    comments: Vec<RawComment>,
}

#[derive(Deserialize)]
struct RawComment {
    id: String,
    author: Option<Assignee>,
    body: Option<String>,
    created: Option<String>,
    updated: Option<String>,
}

fn raw_comment_to_public(raw: RawComment) -> JiraComment {
    let (author, author_key) = raw
        .author
        .map(|a| {
            (
                a.display_name.unwrap_or_default(),
                a.name.unwrap_or_default(),
            )
        })
        .unwrap_or_default();
    JiraComment {
        id: raw.id,
        author,
        author_key,
        body: normalize_eol(raw.body.unwrap_or_default()),
        created: raw.created.unwrap_or_default(),
        updated: raw.updated.unwrap_or_default(),
    }
}

impl JiraClient {
    /// Fetch comments for an issue.
    pub async fn get_comments(&self, key: &str) -> Result<Vec<JiraComment>, String> {
        let url = format!("{}/rest/api/2/issue/{}?fields=comment", self.base_url, key);

        let resp = self.send("GET", &url, self.http.get(&url)).await?;
        let resp = self.check_status("GET", &url, resp).await?;
        let body_text = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;

        let data: CommentIssueResponse = serde_json::from_str(&body_text)
            .map_err(|e| format!("Failed to parse comments: {e}"))?;

        let comments = data.fields.comment.map(|c| c.comments).unwrap_or_default();

        Ok(comments.into_iter().map(raw_comment_to_public).collect())
    }

    /// Add a new comment to an issue. Returns the created comment.
    pub async fn add_comment(&self, key: &str, body: &str) -> Result<JiraComment, String> {
        let url = format!("{}/rest/api/2/issue/{}/comment", self.base_url, key);

        let payload = serde_json::json!({ "body": body });

        let resp = self
            .send("POST", &url, self.http.post(&url).json(&payload))
            .await?;
        let resp = self.check_status("POST", &url, resp).await?;
        let body_text = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;

        let raw: RawComment = serde_json::from_str(&body_text)
            .map_err(|e| format!("Failed to parse comment: {e}"))?;

        Ok(raw_comment_to_public(raw))
    }

    /// Update an existing comment on an issue. Returns the updated comment.
    pub async fn update_comment(
        &self,
        key: &str,
        comment_id: &str,
        body: &str,
    ) -> Result<JiraComment, String> {
        let url = format!(
            "{}/rest/api/2/issue/{}/comment/{}",
            self.base_url, key, comment_id
        );

        let payload = serde_json::json!({ "body": body });

        let resp = self
            .send("PUT", &url, self.http.put(&url).json(&payload))
            .await?;
        let resp = self.check_status("PUT", &url, resp).await?;
        let body_text = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;

        let raw: RawComment = serde_json::from_str(&body_text)
            .map_err(|e| format!("Failed to parse comment: {e}"))?;

        Ok(raw_comment_to_public(raw))
    }

    /// Delete a comment from an issue.
    pub async fn delete_comment(&self, key: &str, comment_id: &str) -> Result<(), String> {
        let url = format!(
            "{}/rest/api/2/issue/{}/comment/{}",
            self.base_url, key, comment_id
        );

        let resp = self.send("DELETE", &url, self.http.delete(&url)).await?;
        self.check_status("DELETE", &url, resp).await?;

        Ok(())
    }
}
