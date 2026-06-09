//! Workflow transitions: list available + execute by id.

use std::collections::BTreeMap;

use not_yet_done_content::http_log;
use serde::Deserialize;

use super::{JiraClient, NameField};

/// A Jira workflow transition.
#[derive(Debug, Clone)]
pub struct JiraTransition {
    pub id: String,
    pub name: String,
    pub to_status: String,
    /// Stable id of the target status (workflow-cache key). Empty when
    /// the API didn't return one — recording skips those rows.
    pub to_status_id: String,
    /// Names of fields the API flagged `required: true` for this
    /// transition (from `expand=transitions.fields`). Empty for
    /// unconditional transitions.
    pub required_fields: Vec<String>,
}

#[derive(Deserialize)]
struct TransitionsResponse {
    transitions: Vec<RawTransition>,
}

#[derive(Deserialize)]
struct RawTransition {
    id: String,
    name: String,
    to: Option<NameField>,
    #[serde(default)]
    fields: Option<BTreeMap<String, RawTransitionField>>,
}

#[derive(Deserialize)]
struct RawTransitionField {
    #[serde(default)]
    required: bool,
    #[serde(default)]
    name: Option<String>,
}

impl JiraClient {
    /// Fetch available transitions for an issue. Uses
    /// `expand=transitions.fields` so the workflow recorder can capture
    /// which fields are required without a second round-trip.
    pub async fn get_transitions(&self, key: &str) -> Result<Vec<JiraTransition>, String> {
        let url = format!(
            "{}/rest/api/2/issue/{}/transitions?expand=transitions.fields",
            self.base_url, key
        );

        http_log::log_request("GET", &url);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| http_log::network_error("GET", &url, e))?;
        let resp = http_log::check_status("GET", &url, resp).await?;
        let body_text = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;

        let data: TransitionsResponse = serde_json::from_str(&body_text)
            .map_err(|e| format!("Failed to parse transitions: {e}"))?;

        Ok(data
            .transitions
            .into_iter()
            .map(|t| {
                let (to_status_id, to_status) = t
                    .to
                    .map(|s| (s.id.unwrap_or_default(), s.name.unwrap_or_default()))
                    .unwrap_or_default();
                let required_fields: Vec<String> = t
                    .fields
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|(key, f)| {
                        if !f.required {
                            return None;
                        }
                        Some(f.name.unwrap_or(key))
                    })
                    .collect();
                JiraTransition {
                    id: t.id,
                    name: t.name,
                    to_status,
                    to_status_id,
                    required_fields,
                }
            })
            .collect())
    }

    /// Execute a transition on an issue.
    pub async fn do_transition(&self, key: &str, transition_id: &str) -> Result<(), String> {
        let url = format!(
            "{}/rest/api/2/issue/{}/transitions",
            self.base_url, key
        );

        let body = serde_json::json!({
            "transition": { "id": transition_id }
        });

        http_log::log_request("POST", &url);
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| http_log::network_error("POST", &url, e))?;
        http_log::check_status("POST", &url, resp).await?;

        Ok(())
    }
}
