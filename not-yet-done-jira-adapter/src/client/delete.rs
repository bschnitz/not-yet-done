//! Delete an issue via `DELETE /rest/api/2/issue/{key}`.
//!
//! Subtasks are deliberately *not* swept along: the request goes out with
//! `deleteSubtasks=false`, so a parent with children fails loudly (Jira
//! answers 400) instead of quietly taking the children with it. Deleting
//! the subtasks first is the same action per subtask key.

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

        let resp = self.send("DELETE", &url, self.http.delete(&url)).await?;
        self.check_status("DELETE", &url, resp).await?;

        Ok(())
    }
}
