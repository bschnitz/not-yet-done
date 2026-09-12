//! Project metadata: the issue types a project offers.
//!
//! A type is named, not numbered, everywhere an issue is written (`create`
//! posts `issuetype: { name }`), so what a caller needs from here is the
//! list of names it may use — and a wrong name answered before the POST
//! rather than as a server-side rejection afterwards.

use serde::Deserialize;

use super::JiraClient;

/// One issue type of a project, as `GET /rest/api/2/project/{key}` lists it.
#[derive(Debug, Clone)]
pub struct JiraIssueType {
    pub id: String,
    pub name: String,
    /// Sub-task types cannot stand on their own — a create without a parent
    /// is rejected — so callers that offer a plain "new issue" filter them.
    pub subtask: bool,
}

#[derive(Deserialize)]
struct ProjectResponse {
    id: String,
    #[serde(rename = "issueTypes")]
    issue_types: Option<Vec<RawIssueType>>,
}

#[derive(Deserialize)]
struct RawIssueType {
    id: String,
    name: Option<String>,
    subtask: Option<bool>,
}

impl JiraClient {
    /// The project's id and its issue types. The project key is taken as
    /// given (`PROJ`); Jira accepts the numeric id there just as well.
    pub async fn get_project_issue_types(
        &self,
        project: &str,
    ) -> Result<(String, Vec<JiraIssueType>), String> {
        let url = format!("{}/rest/api/2/project/{}", self.base_url, project);

        let resp = self.send("GET", &url, self.http.get(&url)).await?;
        let resp = self.check_status("GET", &url, resp).await?;
        let body_text = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;

        let data: ProjectResponse = serde_json::from_str(&body_text)
            .map_err(|e| format!("Failed to parse project: {e}"))?;

        let types = data
            .issue_types
            .unwrap_or_default()
            .into_iter()
            .map(|t| JiraIssueType {
                id: t.id,
                name: t.name.unwrap_or_default(),
                subtask: t.subtask.unwrap_or(false),
            })
            .collect();

        Ok((data.id, types))
    }
}
