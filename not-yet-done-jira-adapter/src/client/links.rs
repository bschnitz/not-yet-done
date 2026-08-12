//! Issue links: list the available link types + create a link between two
//! issues via `POST /rest/api/2/issueLink`.
//!
//! Jira models a link type as a name plus two directional phrasings, e.g.
//! `Blocks` → outward `"blocks"`, inward `"is blocked by"`. A concrete link
//! names the type and which issue sits on the outward vs. inward side:
//! `outwardIssue "blocks" inwardIssue`.

use not_yet_done_content::http_log;
use serde::Deserialize;

use super::JiraClient;

/// A Jira issue-link type with its two directional phrasings.
#[derive(Debug, Clone)]
pub struct JiraLinkType {
    pub name: String,
    /// Phrasing when this side is the inward issue (e.g. `"is blocked by"`).
    pub inward: String,
    /// Phrasing when this side is the outward issue (e.g. `"blocks"`).
    pub outward: String,
}

#[derive(Deserialize)]
struct LinkTypesResponse {
    #[serde(rename = "issueLinkTypes")]
    issue_link_types: Vec<RawLinkType>,
}

#[derive(Deserialize)]
struct RawLinkType {
    name: String,
    #[serde(default)]
    inward: String,
    #[serde(default)]
    outward: String,
}

impl JiraClient {
    /// Fetch the instance's configured issue-link types.
    pub async fn get_issue_link_types(&self) -> Result<Vec<JiraLinkType>, String> {
        let url = format!("{}/rest/api/2/issueLinkType", self.base_url);

        http_log::log_request("GET", &url);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| http_log::network_error("GET", &url, e))?;
        let resp = self.check_status("GET", &url, resp).await?;
        let body_text = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;

        let data: LinkTypesResponse = serde_json::from_str(&body_text)
            .map_err(|e| format!("Failed to parse link types: {e}"))?;

        Ok(data
            .issue_link_types
            .into_iter()
            .map(|t| JiraLinkType {
                name: t.name,
                inward: t.inward,
                outward: t.outward,
            })
            .collect())
    }

    /// Create a link of `type_name` where `outward_key` is the outward issue
    /// and `inward_key` the inward one (`outward_key <outward-phrase>
    /// inward_key`, e.g. `PROJ-1 "blocks" PROJ-2`).
    pub async fn create_issue_link(
        &self,
        type_name: &str,
        outward_key: &str,
        inward_key: &str,
    ) -> Result<(), String> {
        let url = format!("{}/rest/api/2/issueLink", self.base_url);

        let body = serde_json::json!({
            "type": { "name": type_name },
            "outwardIssue": { "key": outward_key },
            "inwardIssue": { "key": inward_key },
        });

        http_log::log_request("POST", &url);
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| http_log::network_error("POST", &url, e))?;
        self.check_status("POST", &url, resp).await?;

        Ok(())
    }
}
