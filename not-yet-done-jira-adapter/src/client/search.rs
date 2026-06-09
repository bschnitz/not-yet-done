//! Issue search (JQL) + single-issue fetch + field updates.

use not_yet_done_content::http_log;
use serde::Deserialize;

use super::{Assignee, JiraClient, NameField, normalize_eol};

/// A simplified Jira ticket for display.
#[derive(Debug, Clone)]
pub struct JiraTicket {
    pub key: String,
    pub summary: String,
    pub status: String,
    pub priority: String,
    pub assignee: String,
    pub issue_type: String,
    pub updated: String,
    pub attachments_count: u64,
}

/// Full issue details for editing.
#[derive(Debug, Clone)]
pub struct JiraIssueDetail {
    pub key: String,
    pub summary: String,
    pub description: String,
    pub status: String,
    /// Stable status id used as workflow-cache key. Empty when the
    /// detail came from a path that didn't request the id (e.g. older
    /// list responses); workflow-edge writers skip rows with empty ids.
    pub status_id: String,
    pub priority: String,
    pub issue_type: String,
    /// Stable issuetype id used as workflow-cache key (see `status_id`).
    pub issue_type_id: String,
    pub assignee: String,
    /// Jira-username of the current assignee (same value as inside `[~name]`).
    /// Empty when unassigned.
    pub assignee_key: String,
    pub reporter: String,
    pub reporter_key: String,
    pub creator: String,
    pub creator_key: String,
    pub labels: Vec<String>,
    pub updated: String,
}

#[derive(Deserialize)]
struct SearchResponse {
    issues: Option<Vec<Issue>>,
    #[serde(default)]
    total: Option<u64>,
    #[serde(default, rename = "startAt")]
    start_at: Option<u32>,
    #[serde(default, rename = "maxResults")]
    max_results: Option<u32>,
}

/// One page of issue search results.
#[derive(Debug)]
pub struct SearchPage {
    pub tickets: Vec<JiraTicket>,
    pub total: Option<u64>,
    pub start_at: u32,
    pub max_results: u32,
}

#[derive(Deserialize)]
struct Issue {
    key: String,
    fields: IssueFields,
}

#[derive(Deserialize)]
struct IssueFields {
    summary: Option<String>,
    status: Option<NameField>,
    priority: Option<NameField>,
    assignee: Option<Assignee>,
    issuetype: Option<NameField>,
    #[serde(default)]
    updated: Option<String>,
    #[serde(default)]
    attachment: Option<Vec<serde_json::Value>>,
}

#[derive(Deserialize)]
struct IssueDetail {
    key: String,
    fields: IssueDetailFields,
}

#[derive(Deserialize)]
struct IssueDetailFields {
    summary: Option<String>,
    description: Option<String>,
    status: Option<NameField>,
    priority: Option<NameField>,
    issuetype: Option<NameField>,
    assignee: Option<Assignee>,
    #[serde(default)]
    reporter: Option<Assignee>,
    #[serde(default)]
    creator: Option<Assignee>,
    labels: Option<Vec<String>>,
    updated: Option<String>,
}

impl JiraClient {
    /// Search for issues using JQL. Returns one page of tickets plus the
    /// pagination metadata reported by the server.
    pub async fn search(
        &self,
        jql: &str,
        start_at: u32,
        max_results: u32,
    ) -> Result<SearchPage, String> {
        let url = format!("{}/rest/api/2/search", self.base_url);

        let body = serde_json::json!({
            "jql": jql,
            "startAt": start_at,
            "maxResults": max_results,
            "fields": ["summary", "status", "priority", "assignee", "issuetype", "updated", "attachment"]
        });

        http_log::log_request("POST", &url);
        let resp = self.http.post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| http_log::network_error("POST", &url, e))?;
        let resp = http_log::check_status("POST", &url, resp).await?;
        let body_text = resp.text().await
            .map_err(|e| format!("Failed to read response: {e}"))?;

        let data: SearchResponse = serde_json::from_str(&body_text)
            .map_err(|e| format!("Failed to parse Jira response: {e}"))?;

        let tickets = data.issues.unwrap_or_default().into_iter().map(|issue| {
            JiraTicket {
                key: issue.key,
                summary: issue.fields.summary.unwrap_or_default(),
                status: issue.fields.status.and_then(|s| s.name).unwrap_or_default(),
                priority: issue.fields.priority.and_then(|p| p.name).unwrap_or_default(),
                assignee: issue.fields.assignee.and_then(|a| a.display_name).unwrap_or_default(),
                issue_type: issue.fields.issuetype.and_then(|t| t.name).unwrap_or_default(),
                updated: issue.fields.updated.unwrap_or_default(),
                attachments_count: issue.fields.attachment.map(|v| v.len() as u64).unwrap_or(0),
            }
        }).collect();

        Ok(SearchPage {
            tickets,
            total: data.total,
            start_at: data.start_at.unwrap_or(start_at),
            max_results: data.max_results.unwrap_or(max_results),
        })
    }

    /// Fetch the first page of tickets assigned to the current user.
    pub async fn my_tickets(&self, max_results: u32) -> Result<SearchPage, String> {
        self.search("assignee = currentUser() ORDER BY updated DESC", 0, max_results).await
    }

    /// Fetch full details of a single issue (for editing).
    pub async fn get_issue(&self, key: &str) -> Result<JiraIssueDetail, String> {
        let url = format!(
            "{}/rest/api/2/issue/{}?fields=summary,description,status,priority,\
             issuetype,assignee,reporter,creator,labels,updated",
            self.base_url, key
        );

        http_log::log_request("GET", &url);
        let resp = self.http.get(&url)
            .send()
            .await
            .map_err(|e| http_log::network_error("GET", &url, e))?;
        let resp = http_log::check_status("GET", &url, resp).await?;
        let body_text = resp.text().await
            .map_err(|e| format!("Failed to read response: {e}"))?;

        let data: IssueDetail = serde_json::from_str(&body_text)
            .map_err(|e| format!("Failed to parse issue: {e}"))?;

        let split = |a: Option<Assignee>| -> (String, String) {
            let display = a.as_ref().and_then(|x| x.display_name.clone()).unwrap_or_default();
            let key = a.and_then(|x| x.name).unwrap_or_default();
            (display, key)
        };
        let (assignee, assignee_key) = split(data.fields.assignee);
        let (reporter, reporter_key) = split(data.fields.reporter);
        let (creator, creator_key) = split(data.fields.creator);

        let status_field = data.fields.status;
        let issuetype_field = data.fields.issuetype;
        let status_id = status_field.as_ref().and_then(|s| s.id.clone()).unwrap_or_default();
        let status_name = status_field.and_then(|s| s.name).unwrap_or_default();
        let issue_type_id = issuetype_field.as_ref().and_then(|t| t.id.clone()).unwrap_or_default();
        let issue_type_name = issuetype_field.and_then(|t| t.name).unwrap_or_default();
        Ok(JiraIssueDetail {
            key: data.key,
            summary: normalize_eol(data.fields.summary.unwrap_or_default()),
            description: normalize_eol(data.fields.description.unwrap_or_default()),
            status: status_name,
            status_id,
            priority: data.fields.priority.and_then(|p| p.name).unwrap_or_default(),
            issue_type: issue_type_name,
            issue_type_id,
            assignee,
            assignee_key,
            reporter,
            reporter_key,
            creator,
            creator_key,
            labels: data.fields.labels.unwrap_or_default(),
            updated: data.fields.updated.unwrap_or_default(),
        })
    }

    /// Update summary and description of an issue.
    pub async fn update_issue(&self, key: &str, summary: &str, description: &str) -> Result<(), String> {
        self.update_issue_full(key, Some(summary), Some(description), None, None).await
    }

    /// Update arbitrary subset of issue fields. `labels = Some(_)` overwrites
    /// the full label list. `assignee_key = Some("")` un-assigns; `Some("foo")`
    /// sets to that Jira-username.
    pub async fn update_issue_full(
        &self,
        key: &str,
        summary: Option<&str>,
        description: Option<&str>,
        labels: Option<&[String]>,
        assignee_key: Option<&str>,
    ) -> Result<(), String> {
        let mut fields = serde_json::Map::new();
        if let Some(s) = summary {
            fields.insert("summary".into(), serde_json::Value::String(s.to_string()));
        }
        if let Some(d) = description {
            fields.insert("description".into(), serde_json::Value::String(d.to_string()));
        }
        if let Some(ls) = labels {
            let arr: Vec<serde_json::Value> = ls.iter()
                .map(|l| serde_json::Value::String(l.clone()))
                .collect();
            fields.insert("labels".into(), serde_json::Value::Array(arr));
        }
        if let Some(ak) = assignee_key {
            // Server/DC: { "name": "..." }; empty key → unassign via { "name": null }.
            let value = if ak.is_empty() {
                serde_json::json!({ "name": null })
            } else {
                serde_json::json!({ "name": ak })
            };
            fields.insert("assignee".into(), value);
        }
        self.update_fields(key, fields).await
    }

    /// Update arbitrary fields on an issue via PUT.
    pub async fn update_fields(&self, key: &str, fields: serde_json::Map<String, serde_json::Value>) -> Result<(), String> {
        let url = format!("{}/rest/api/2/issue/{}", self.base_url, key);

        let body = serde_json::json!({ "fields": fields });

        http_log::log_request("PUT", &url);
        let resp = self.http.put(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| http_log::network_error("PUT", &url, e))?;
        http_log::check_status("PUT", &url, resp).await?;

        Ok(())
    }
}
