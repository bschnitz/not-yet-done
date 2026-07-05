//! `upload-attachment` action for `confluence:page`: receive the file
//! paths the TUI's FilePicker handed back, POST each one to
//! `/rest/api/content/{id}/child/attachment` as multipart, and surface
//! a per-file success/failure summary via [`ActionOutcome::Done`] (or
//! [`ActionOutcome::NoChanges`] when the user closed the picker without
//! selecting anything).
//!
//! Confluence's upload endpoint accepts one file per request, so the
//! loop here is unavoidable — multi-select on the picker turns into N
//! sequential POSTs. Aggregate failures are bundled into one error
//! message rather than aborting on the first failure, so the user
//! doesn't lose the rest of the batch when one mid-list file is
//! unreadable or rejected server-side.

use std::path::PathBuf;

use not_yet_done_content::{ActionOutcome, ContentError, Result};

use super::ConfluencePageNode;

impl ConfluencePageNode {
    /// Loop through every FilePicker selection and POST it as an
    /// attachment. Returns `NoChanges` if the user closed the picker
    /// without selecting anything. Mixed success/failure surfaces as an
    /// error containing all failed paths so the user knows what didn't
    /// make it; pure success returns the upload count plus the page id
    /// in the notification.
    pub(super) async fn execute_upload_attachment(
        &self,
        paths: Vec<PathBuf>,
    ) -> Result<ActionOutcome> {
        if paths.is_empty() {
            return Ok(ActionOutcome::NoChanges);
        }
        let mut uploaded = 0usize;
        let mut failures: Vec<String> = Vec::new();
        for path in &paths {
            match self.client.upload_attachment(&self.page.id, path).await {
                Ok(_) => uploaded += 1,
                Err(e) => failures.push(format!("{}: {e}", path.display())),
            }
        }
        if !failures.is_empty() {
            return Err(ContentError::Other(
                format!(
                    "uploaded {}/{}; failures: {}",
                    uploaded,
                    paths.len(),
                    failures.join("; ")
                )
                .into(),
            ));
        }
        Ok(ActionOutcome::Done {
            message: Some(format!(
                "Uploaded {} attachment(s) to page {} (id {})",
                uploaded, self.page.title, self.page.id
            )),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{ConfluenceClient, PageMeta};
    use std::sync::Arc;

    fn synthetic_client() -> Arc<ConfluenceClient> {
        Arc::new(
            ConfluenceClient::new(
                "https://wiki.example.invalid/confluence",
                "JSESSIONID=synthetic",
                false,
            )
            .expect("client"),
        )
    }

    fn bare_page_node() -> ConfluencePageNode {
        ConfluencePageNode::new(
            synthetic_client(),
            "https://wiki.example.invalid/confluence",
            PageMeta {
                id: "777".into(),
                title: "Parent".into(),
                page_type: "page".into(),
                webui: "/spaces/DEMO/pages/777/Parent".into(),
                has_children: None,
            },
        )
    }

    #[tokio::test]
    async fn upload_no_paths_yields_no_changes() {
        // FilePicker closed without selecting anything — the action
        // should be a no-op rather than firing an empty POST or erroring.
        let node = bare_page_node();
        match node.execute_upload_attachment(Vec::new()).await {
            Ok(ActionOutcome::NoChanges) => {}
            Ok(ActionOutcome::Done { .. }) => {
                panic!("expected NoChanges for empty selection, got Done")
            }
            Ok(ActionOutcome::Reopen { .. }) => {
                panic!("expected NoChanges for empty selection, got Reopen")
            }
            Ok(ActionOutcome::Navigate { .. }) => {
                panic!("expected NoChanges for empty selection, got Navigate")
            }
            Ok(ActionOutcome::OpenExternal { .. }) => {
                panic!("expected NoChanges for empty selection, got OpenExternal")
            }
            Ok(ActionOutcome::OpenEditor { .. }) => {
                panic!("expected NoChanges for empty selection, got OpenEditor")
            }
            Err(e) => panic!("expected NoChanges, got Err: {e}"),
        }
    }

    #[tokio::test]
    async fn upload_aggregates_failures_with_paths() {
        // Pointing at unreadable paths means every POST setup will fail
        // before the wire — the action bundles all per-file errors into
        // one ContentError so the user can see exactly what didn't make
        // it. (Empty bytes would still need a network round-trip; missing
        // files short-circuit in `tokio::fs::read`.)
        let node = bare_page_node();
        let paths = vec![
            PathBuf::from("/definitely/does/not/exist/a.bin"),
            PathBuf::from("/definitely/does/not/exist/b.bin"),
        ];
        match node.execute_upload_attachment(paths).await {
            Ok(_) => panic!("expected Err for unreadable files"),
            Err(e) => {
                let msg = format!("{e}");
                assert!(
                    msg.contains("uploaded 0/2"),
                    "error summarises ratio: {msg}"
                );
                assert!(msg.contains("a.bin"), "names first file: {msg}");
                assert!(msg.contains("b.bin"), "names second file: {msg}");
            }
        }
    }
}
