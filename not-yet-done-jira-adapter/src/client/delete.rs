//! Delete an issue via `DELETE /rest/api/2/issue/{key}`.
//!
//! Subtasks are deliberately *not* swept along: the request goes out with
//! `deleteSubtasks=false`, so a parent with children fails loudly (Jira
//! answers 400) instead of quietly taking the children with it. Deleting
//! the subtasks first is the same action per subtask key.

use not_yet_done_content::http_log;

use super::JiraClient;

impl JiraClient {
    /// DELETE the issue. Irreversible — the caller owns the confirmation.
    ///
    /// The server decides whether it may happen at all: without the "Delete
    /// Issues" project permission Jira answers 403, and its message reaches
    /// the caller through [`JiraClient::check_status`].
    pub async fn delete_issue(&self, key: &str) -> Result<(), String> {
        let url = format!(
            "{}/rest/api/2/issue/{}?deleteSubtasks=false",
            self.base_url, key
        );

        http_log::log_request("DELETE", &url);
        let resp = self
            .http
            .delete(&url)
            .send()
            .await
            .map_err(|e| http_log::network_error("DELETE", &url, e))?;
        self.check_status("DELETE", &url, resp).await?;

        Ok(())
    }
}
