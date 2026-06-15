//! `confluence:attachment` node + `download` action.
//!
//! Read-only leaf: lists carry filename / author / size / mime-type /
//! creation-timestamp metadata; the single action downloads the
//! attachment bytes into a per-user temp dir (reused across invocations
//! by `<id>-<filename>`) and spawns `xdg-open` detached on the cached
//! file. Same shape as the Jira attachment node — duplicated rather
//! than shared because the planned `not-yet-done-adapter-common` crate
//! (R-2) is deferred until a third adapter motivates the abstraction.

use std::sync::Arc;

use async_trait::async_trait;

use not_yet_done_content::*;

use crate::client::{AttachmentMeta, ConfluenceClient};

use super::other_err;

pub(super) fn attachment_node_type() -> NodeType {
    NodeType {
        type_id: "confluence:attachment".into(),
        mime_type: "application/octet-stream".into(),
        syntax: None,
        file_extension: String::new(),
        display_name: "Confluence Attachment".into(),
    }
}

/// Static superset of actions exposed for `confluence:attachment`. The
/// single `download` action is surfaced via both `Node::actions()` and
/// `ContentAdapter::actions_for_type()` so the TUI's shortcut-hint
/// resolver can populate the action bar without instantiating a node.
pub(super) fn attachment_actions() -> Vec<NodeAction> {
    vec![
        // download is fire-and-forget (no input, no popup) → never "active",
        // so it stays in the status bar (default placement).
        NodeAction::new("download", "download", InputSpec::None)
            .with_default_key('d'),
    ]
}

/// Format a file size in bytes to a human-readable string. Local copy
/// of the same helper in `not-yet-done-jira-adapter` — see module doc
/// for the cross-adapter dedup decision.
fn format_file_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

pub(super) struct ConfluenceAttachmentNode {
    client: Arc<ConfluenceClient>,
    attachment: AttachmentMeta,
    /// Composite id for `get_by_id` round-trips: `{page_id}/attachment/{att_id}`.
    /// Attachments live in a global id namespace, but the composite form
    /// preserves the originating page context for breadcrumb rendering
    /// and for the page-relative download path (which the wire payload
    /// hands us as a self-contained URL, so the page-id is informational
    /// only here).
    composite_id: String,
    cached_metadata: Metadata,
}

impl ConfluenceAttachmentNode {
    pub(super) fn new(
        client: Arc<ConfluenceClient>,
        attachment: AttachmentMeta,
        page_id: &str,
    ) -> Self {
        let composite_id = format!("{}/attachment/{}", page_id, attachment.id);
        let cached_metadata = Metadata {
            fields: vec![
                MetadataField {
                    key: "filename".into(),
                    value: attachment.title.clone(),
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
                    value: format_file_size(attachment.file_size),
                    display_label: "Size".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "mime_type".into(),
                    value: attachment.media_type.clone(),
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
                    key: "page".into(),
                    value: page_id.to_string(),
                    display_label: "Page".into(),
                    editable: false,
                    allowed_values: None,
                },
            ],
        };
        Self {
            client,
            attachment,
            composite_id,
            cached_metadata,
        }
    }

    /// Download (cached by id+filename) into a per-user temp dir, then
    /// spawn `xdg-open` detached. The cache key includes the attachment
    /// id so two attachments with the same filename on different pages
    /// don't collide, and re-opening the same attachment doesn't
    /// re-download.
    async fn download_and_open(&self) -> Result<ActionOutcome> {
        if self.attachment.download_path.is_empty() {
            return Err(other_err(format!(
                "Attachment {} has no download link",
                self.attachment.id
            )));
        }
        let mut dir = std::env::temp_dir();
        dir.push("not_yet_done");
        dir.push("confluence_attachments");
        std::fs::create_dir_all(&dir)
            .map_err(|e| other_err(format!("create temp dir: {e}")))?;

        let safe_name = self.attachment.title.replace(['/', '\\'], "_");
        let mut path = dir;
        path.push(format!("{}-{}", self.attachment.id, safe_name));

        if !path.exists() {
            let bytes = self
                .client
                .download_attachment(&self.attachment.download_path)
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
            message: Some(format!("opened {}", self.attachment.title)),
        })
    }
}

#[async_trait]
impl Node for ConfluenceAttachmentNode {
    fn id(&self) -> &str {
        &self.composite_id
    }

    fn label(&self) -> &str {
        &self.attachment.title
    }

    fn node_type(&self) -> &NodeType {
        static T: std::sync::LazyLock<NodeType> = std::sync::LazyLock::new(attachment_node_type);
        &T
    }

    fn metadata(&self) -> &Metadata {
        &self.cached_metadata
    }

    fn actions(&self) -> Vec<NodeAction> {
        attachment_actions()
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

    async fn execute(
        &mut self,
        action_id: &str,
        input: ActionInput,
    ) -> Result<ActionOutcome> {
        match (action_id, input) {
            ("download", ActionInput::None) => self.download_and_open().await,
            (id, _) => Err(ContentError::NotSupported(format!(
                "ConfluenceAttachmentNode action `{id}` not supported"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn sample_attachment() -> AttachmentMeta {
        AttachmentMeta {
            id: "att56789".into(),
            title: "design.pdf".into(),
            attachment_type: "attachment".into(),
            file_size: 1_048_576,
            media_type: "application/pdf".into(),
            author: "Alice Example".into(),
            created: "2026-05-01T10:00:00.000Z".into(),
            download_path: "/download/attachments/12345/design.pdf?version=1".into(),
        }
    }

    #[test]
    fn attachment_metadata_carries_all_fields() {
        let node = ConfluenceAttachmentNode::new(synthetic_client(), sample_attachment(), "12345");
        assert_eq!(node.id(), "12345/attachment/att56789");
        assert_eq!(node.label(), "design.pdf");

        let meta = node.metadata();
        assert_eq!(meta.fields.len(), 6);
        assert_eq!(meta.fields[0].key, "filename");
        assert_eq!(meta.fields[0].value, "design.pdf");
        assert_eq!(meta.fields[2].key, "size");
        assert_eq!(meta.fields[2].value, "1.0 MB");
        assert_eq!(meta.fields[3].key, "mime_type");
        assert_eq!(meta.fields[3].value, "application/pdf");
        assert_eq!(meta.fields[5].key, "page");
        assert_eq!(meta.fields[5].value, "12345");
    }

    #[test]
    fn attachment_has_no_children() {
        let node = ConfluenceAttachmentNode::new(synthetic_client(), sample_attachment(), "12345");
        assert!(node.children_types().is_empty());
    }

    #[test]
    fn attachment_actions_includes_download_d() {
        let actions = attachment_actions();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].id, "download");
        assert_eq!(actions[0].default_key, Some('d'));
        assert!(matches!(actions[0].input, InputSpec::None));
    }

    #[test]
    fn node_type_advertises_octet_stream() {
        let t = attachment_node_type();
        assert_eq!(t.type_id, "confluence:attachment");
        assert_eq!(t.mime_type, "application/octet-stream");
        assert!(t.syntax.is_none());
    }

    #[tokio::test]
    async fn execute_rejects_unknown_action() {
        let mut node =
            ConfluenceAttachmentNode::new(synthetic_client(), sample_attachment(), "12345");
        match node.execute("nope", ActionInput::None).await {
            Err(e) => assert!(format!("{e}").contains("nope")),
            Ok(_) => panic!("unknown action must be rejected"),
        }
    }

    #[tokio::test]
    async fn execute_rejects_download_when_link_missing() {
        let att = AttachmentMeta {
            id: "x".into(),
            title: "noplace.bin".into(),
            attachment_type: "attachment".into(),
            file_size: 0,
            media_type: String::new(),
            author: String::new(),
            created: String::new(),
            download_path: String::new(),
        };
        let mut node = ConfluenceAttachmentNode::new(synthetic_client(), att, "12345");
        match node.execute("download", ActionInput::None).await {
            Err(e) => assert!(format!("{e}").contains("x"), "error mentions att id: {e}"),
            Ok(_) => panic!("missing download link must be rejected"),
        }
    }

    #[test]
    fn format_file_size_buckets() {
        assert_eq!(format_file_size(500), "500 B");
        assert_eq!(format_file_size(2048), "2.0 KB");
        assert_eq!(format_file_size(5_242_880), "5.0 MB");
        assert_eq!(format_file_size(2_147_483_648), "2.0 GB");
    }
}
