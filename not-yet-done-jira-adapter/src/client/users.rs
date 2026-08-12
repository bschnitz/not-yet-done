//! Users + labels: lookup, listing, mention resolution.

use not_yet_done_content::http_log;
use serde::Deserialize;

use super::JiraClient;

/// A Jira user as returned by the assignable search API.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JiraUser {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub email_address: Option<String>,
}

impl JiraClient {
    /// Query the label-suggest endpoint for a single query string. Returns the
    /// raw label strings Jira offers (max ~20 per query, matched case-
    /// insensitively as a substring). A non-success status yields an empty
    /// list rather than an error, so callers can treat "no match" and "server
    /// said no" alike. Used by [`all_labels`] (fanned out across prefixes) and
    /// for on-demand canonical-label lookup when resolving an edited
    /// `labels:` line.
    pub async fn suggest_labels(&self, query: &str) -> Result<Vec<String>, String> {
        #[derive(Deserialize)]
        struct Suggestion {
            label: String,
        }
        #[derive(Deserialize)]
        struct SuggestResponse {
            suggestions: Vec<Suggestion>,
        }

        let url = format!("{}/rest/api/1.0/labels/suggest", self.base_url);
        http_log::log_request("GET", &format!("{url}?query={query}"));
        let resp = self
            .http
            .get(&url)
            .query(&[("query", query)])
            .send()
            .await
            .map_err(|e| http_log::network_error("GET", &url, e))?;
        http_log::log_response("GET", &url, resp.status().as_u16());
        if !resp.status().is_success() {
            return Ok(Vec::new());
        }
        let body = resp.text().await.unwrap_or_default();
        Ok(serde_json::from_str::<SuggestResponse>(&body)
            .map(|d| d.suggestions.into_iter().map(|s| s.label).collect())
            .unwrap_or_default())
    }

    /// Fetch all labels (global). Fans out across alphanumeric prefixes
    /// since the suggest endpoint returns max 20 per query.
    pub async fn all_labels(&self) -> Result<Vec<String>, String> {
        let mut all = std::collections::BTreeSet::new();

        let prefixes = ('A'..='Z')
            .chain('0'..='9')
            .map(|c| c.to_string())
            .chain(std::iter::once("_".to_string()))
            .chain(std::iter::once(String::new()));

        for prefix in prefixes {
            for label in self.suggest_labels(&prefix).await? {
                all.insert(label);
            }
        }

        Ok(all.into_iter().collect())
    }

    /// Look up a single user by their Jira-username (the value inside
    /// `[~name]` mentions). Used to resolve mentions of users who haven't
    /// otherwise appeared in any browsed issue / comment yet.
    pub async fn get_user_by_name(&self, username: &str) -> Result<JiraUser, String> {
        let url = format!("{}/rest/api/2/user", self.base_url);
        http_log::log_request("GET", &format!("{url}?username={username}"));
        let resp = self
            .http
            .get(&url)
            .query(&[("username", username)])
            .send()
            .await
            .map_err(|e| http_log::network_error("GET", &url, e))?;
        let resp = self.check_status("GET", &url, resp).await?;
        let body_text = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read response: {e}"))?;

        serde_json::from_str(&body_text).map_err(|e| format!("Failed to parse user: {e}"))
    }

    /// Fetch all users globally. Paginates until no more results.
    pub async fn all_users(&self) -> Result<Vec<JiraUser>, String> {
        let mut all = Vec::new();
        let mut start_at = 0u32;
        let page_size = 1000u32;

        loop {
            let url = format!(
                "{}/rest/api/2/user/search?username=.&maxResults={page_size}&startAt={start_at}",
                self.base_url
            );

            http_log::log_request("GET", &url);
            let resp = self
                .http
                .get(&url)
                .send()
                .await
                .map_err(|e| http_log::network_error("GET", &url, e))?;
            let resp = self.check_status("GET", &url, resp).await?;
            let body = resp.text().await.unwrap_or_default();

            let page: Vec<JiraUser> =
                serde_json::from_str(&body).map_err(|e| format!("Failed to parse users: {e}"))?;

            let count = page.len();
            all.extend(page);

            if (count as u32) < page_size {
                break;
            }
            start_at += page_size;
        }

        Ok(all)
    }
}
