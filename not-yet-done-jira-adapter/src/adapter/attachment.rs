//! Read-only attachment node: metadata fields + `xdg-open` action that
//! downloads (cached) and launches the system viewer.

use std::sync::Arc;

use async_trait::async_trait;

use not_yet_done_content::*;

use crate::client::{JiraAttachment, JiraClient};

use super::types::attachment_node_type;
use super::util::{format_file_size, other_err};

pub(super) fn attachment_actions() -> Vec<NodeAction> {
    vec![NodeAction::new("open", "open", InputSpec::None)]
}

pub(super) struct JiraAttachmentNode {
    client: Arc<JiraClient>,
    attachment: JiraAttachment,
    /// Composite ID: `{issue_key}/attachment/{attachment_id}` for use in `get_by_id`.
    composite_id: String,
    cached_metadata: Metadata,
}

impl JiraAttachmentNode {
    pub(super) fn new(client: Arc<JiraClient>, attachment: JiraAttachment, issue_key: String) -> Self {
        let cached_metadata = Metadata {
            fields: vec![
                MetadataField {
                    key: "filename".into(),
                    value: attachment.filename.clone(),
                    display_label: "Filename".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "author".into(),
                    value: attachment.author.clone(),
                    display_label: "Author".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "size".into(),
                    value: format_file_size(attachment.size),
                    display_label: "Size".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "mime_type".into(),
                    value: attachment.mime_type.clone(),
                    display_label: "Type".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "created".into(),
                    value: attachment.created.clone(),
                    display_label: "Created".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "issue".into(),
                    value: issue_key.clone(),
                    display_label: "Issue".into(),
                    editable: false,
                    allowed_values: None,
                },
            ],
        };
        let composite_id = format!("{}/attachment/{}", issue_key, attachment.id);
        Self {
            client,
            attachment,
            composite_id,
            cached_metadata,
        }
    }

    /// Download the attachment to a per-user temp dir and spawn
    /// `xdg-open` on it. The file is reused across invocations
    /// (filename = `<id>-<filename>`) so re-opening doesn't re-download.
    /// xdg-open runs detached: we don't wait for the viewer to exit.
    async fn open_via_xdg(&self) -> Result<ActionOutcome> {
        let mut dir = std::env::temp_dir();
        dir.push("not_yet_done");
        dir.push("jira_attachments");
        std::fs::create_dir_all(&dir)
            .map_err(|e| other_err(format!("create temp dir: {e}")))?;

        let safe_name = self
            .attachment
            .filename
            .replace(['/', '\\'], "_");
        let mut path = dir;
        path.push(format!("{}-{}", self.attachment.id, safe_name));

        if !path.exists() {
            let bytes = self
                .client
                .download_attachment(&self.attachment.content_url)
                .await
                .map_err(other_err)?;
            std::fs::write(&path, &bytes)
                .map_err(|e| other_err(format!("write attachment to {}: {e}", path.display())))?;
        }

        std::process::Command::new("xdg-open")
            .arg(&path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| other_err(format!("spawn xdg-open: {e}")))?;

        Ok(ActionOutcome::Done {
            message: Some(format!("opened {}", self.attachment.filename)),
        })
    }
}

#[async_trait]
impl Node for JiraAttachmentNode {
    fn id(&self) -> &str {
        &self.composite_id
    }

    fn label(&self) -> &str {
        &self.attachment.filename
    }

    fn node_type(&self) -> &NodeType {
        static ATTACH_TYPE: std::sync::LazyLock<NodeType> =
            std::sync::LazyLock::new(attachment_node_type);
        &ATTACH_TYPE
    }

    fn metadata(&self) -> &Metadata {
        &self.cached_metadata
    }

    fn children_types(&self) -> Vec<NodeType> {
        vec![]
    }

    async fn list(&self, _params: ListParams) -> Result<ListResult> {
        Ok(ListResult {
            items: vec![],
            applied_sort: Vec::new(),
            page: None,
            batch_download_available: false,
            downloaded: vec![],
        })
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        Err(ContentError::NotFound(format!("No child: {id}")))
    }

    fn content(&self) -> Option<&dyn Content> {
        None
    }

    fn actions(&self) -> Vec<NodeAction> {
        attachment_actions()
    }

    async fn execute(
        &mut self,
        action_id: &str,
        input: ActionInput,
    ) -> Result<ActionOutcome> {
        match (action_id, input) {
            ("open", ActionInput::None) => self.open_via_xdg().await,
            (id, _) => Err(ContentError::NotSupported(format!(
                "JiraAttachmentNode action `{id}` not supported"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_client() -> Arc<JiraClient> {
        Arc::new(
            JiraClient::new("http://localhost:0", None, None, Some("test"), false).unwrap(),
        )
    }

    fn sample_attachment() -> JiraAttachment {
        JiraAttachment {
            id: "20001".into(),
            filename: "screenshot.png".into(),
            author: "alice".into(),
            created: "2025-07-01T12:00:00.000+0000".into(),
            size: 1_048_576,
            mime_type: "image/png".into(),
            content_url: "https://jira.example.com/attachment/20001/screenshot.png".into(),
        }
    }

    #[test]
    fn attachment_node_metadata() {
        let node = JiraAttachmentNode::new(test_client(), sample_attachment(), "PROJ-42".into());
        assert_eq!(node.id(), "PROJ-42/attachment/20001");
        assert_eq!(node.label(), "screenshot.png");

        let meta = node.metadata();
        assert_eq!(meta.fields.len(), 6);
        assert_eq!(meta.fields[0].key, "filename");
        assert_eq!(meta.fields[0].value, "screenshot.png");
        assert_eq!(meta.fields[2].key, "size");
        assert_eq!(meta.fields[2].value, "1.0 MB");
        assert_eq!(meta.fields[3].key, "mime_type");
        assert_eq!(meta.fields[3].value, "image/png");
        assert_eq!(meta.fields[5].key, "issue");
        assert_eq!(meta.fields[5].value, "PROJ-42");
    }

    #[test]
    fn attachment_node_has_no_children() {
        let node = JiraAttachmentNode::new(test_client(), sample_attachment(), "PROJ-42".into());
        assert!(node.children_types().is_empty());
    }

    #[test]
    fn attachment_node_no_content() {
        let node = JiraAttachmentNode::new(test_client(), sample_attachment(), "PROJ-42".into());
        assert!(node.content().is_none());
    }

    #[test]
    fn attachment_node_exposes_open_action() {
        let node = JiraAttachmentNode::new(test_client(), sample_attachment(), "PROJ-42".into());
        let actions = node.actions();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].id, "open");
        assert!(matches!(actions[0].input, InputSpec::None));
    }
}
