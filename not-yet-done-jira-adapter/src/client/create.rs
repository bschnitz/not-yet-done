//! Create a new Jira issue via `POST /rest/api/2/issue`.
//!
//! Used by the `clone` action — the editor template seeds summary,
//! description, labels, and (read-only) issue type / priority from a source
//! issue, then the user saves to produce a brand-new ticket in the same
//! project.

use not_yet_done_content::http_log;
use serde::Deserialize;

use super::JiraClient;

/// Inputs for `create_issue`. Strings here are already in their server-bound
/// form (labels are raw label names, `assignee_key` is the Jira-username,
/// not a `uu-…` slug).
pub struct CreateIssueFields {
    pub project_key: String,
    pub summary: String,
    pub description: String,
    pub issue_type: String,
    pub priority: String,
    pub labels: Vec<String>,
    /// Empty → leave unassigned (let the server pick the default).
    pub assignee_key: String,
}

#[derive(Deserialize)]
struct CreateResponse {
    key: String,
}

impl JiraClient {
    /// POST a new issue. Returns the freshly assigned key (e.g. `PROJ-123`).
    pub async fn create_issue(&self, input: &CreateIssueFields) -> Result<String, String> {
        let url = format!("{}/rest/api/2/issue", self.base_url);

        let mut fields = serde_json::Map::new();
        fields.insert(
            "project".into(),
            serde_json::json!({ "key": input.project_key }),
        );
        fields.insert(
            "summary".into(),
            serde_json::Value::String(input.summary.clone()),
        );
        if !input.description.is_empty() {
            fields.insert(
                "description".into(),
                serde_json::Value::String(input.description.clone()),
            );
        }
        if !input.issue_type.is_empty() {
            fields.insert(
                "issuetype".into(),
                serde_json::json!({ "name": input.issue_type }),
            );
        }
        if !input.priority.is_empty() {
            fields.insert(
                "priority".into(),
                serde_json::json!({ "name": input.priority }),
            );
        }
        if !input.labels.is_empty() {
            let arr: Vec<serde_json::Value> = input
                .labels
                .iter()
                .map(|l| serde_json::Value::String(l.clone()))
                .collect();
            fields.insert("labels".into(), serde_json::Value::Array(arr));
        }
        if !input.assignee_key.is_empty() {
            fields.insert(
                "assignee".into(),
                serde_json::json!({ "name": input.assignee_key }),
            );
        }

        let body = serde_json::json!({ "fields": fields });

        http_log::log_request("POST", &url);
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| http_log::network_error("POST", &url, e))?;
        let resp = self.check_status("POST", &url, resp).await?;
        let body_text = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;
        let data: CreateResponse = serde_json::from_str(&body_text)
            .map_err(|e| format!("Failed to parse create response: {e}"))?;
        Ok(data.key)
    }
}
