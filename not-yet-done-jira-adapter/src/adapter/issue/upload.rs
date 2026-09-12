//! `upload-attachment` action for `jira:issue`: POST every file the picker
//! handed back to `/rest/api/2/issue/{key}/attachments`.
//!
//! One request per file — Jira takes several in one multipart body, but a
//! batch that fails halfway says only that it failed, not which file was
//! the problem. Failures are collected instead of aborting the run, so one
//! unreadable path does not cost the rest of the selection.

use std::path::PathBuf;

use not_yet_done_content::{ActionOutcome, ContentError, Result};

use super::JiraIssueNode;

impl JiraIssueNode {
    pub(super) async fn execute_upload_attachment(
        &self,
        paths: Vec<PathBuf>,
    ) -> Result<ActionOutcome> {
        if paths.is_empty() {
            return Ok(ActionOutcome::NoChanges);
        }
        let mut uploaded: Vec<String> = Vec::new();
        let mut failures: Vec<String> = Vec::new();
        for path in &paths {
            match self.client.upload_attachment(&self.key, path).await {
                Ok(a) => uploaded.push(a.filename),
                Err(e) => failures.push(format!("{}: {e}", path.display())),
            }
        }
        if !failures.is_empty() {
            return Err(ContentError::Other(
                format!(
                    "uploaded {}/{}; failures: {}",
                    uploaded.len(),
                    paths.len(),
                    failures.join("; ")
                )
                .into(),
            ));
        }
        Ok(ActionOutcome::Done {
            message: Some(format!("{}: attached {}", self.key, uploaded.join(", "))),
        })
    }
}
