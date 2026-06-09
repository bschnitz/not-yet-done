//! Read-only attachment node — `xdg-open` action that downloads the file
//! once into a per-user temp dir and launches the system viewer detached.

use std::sync::Arc;

use async_trait::async_trait;

use not_yet_done_content::*;

use super::types::attachment_type;
use crate::client::{TaigaAttachment, TaigaClient, download_attachment};

pub(super) fn attachment_actions() -> Vec<NodeAction> {
    vec![NodeAction::new("open", "open", InputSpec::None)]
}

pub(super) struct TaigaAttachmentNode {
    client: Arc<TaigaClient>,
    attachment: TaigaAttachment,
    composite_id: String,
    metadata: Metadata,
}

impl TaigaAttachmentNode {
    pub(super) fn new(
        client: Arc<TaigaClient>,
        attachment: TaigaAttachment,
        parent_id: String,
    ) -> Self {
        let composite_id = format!("{parent_id}/attachment/{}", attachment.id);
        let metadata = Metadata {
            fields: vec![
                MetadataField {
                    key: "filename".into(),
                    value: attachment.name.clone(),
                    display_label: "Filename".into(),
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
                    key: "created".into(),
                    value: attachment.created_date.clone(),
                    display_label: "Created".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "description".into(),
                    value: attachment.description.clone(),
                    display_label: "Description".into(),
                    editable: false,
                    allowed_values: None,
                },
            ],
        };
        Self { client, attachment, composite_id, metadata }
    }

    /// Cache directory: `$TMPDIR/not_yet_done/taiga_attachments`. File
    /// reused across re-opens (`<id>-<name>`); xdg-open runs detached.
    async fn open_via_xdg(&self) -> Result<ActionOutcome> {
        let mut dir = std::env::temp_dir();
        dir.push("not_yet_done");
        dir.push("taiga_attachments");
        std::fs::create_dir_all(&dir).map_err(|e| {
            ContentError::Other(format!("create temp dir {}: {e}", dir.display()).into())
        })?;

        let safe_name = self.attachment.name.replace(['/', '\\'], "_");
        let mut path = dir;
        path.push(format!("{}-{}", self.attachment.id, safe_name));

        if !path.exists() {
            let bytes = download_attachment(&self.client, &self.attachment.url)
                .await
                .map_err(|e| ContentError::Other(e.into()))?;
            std::fs::write(&path, &bytes).map_err(|e| {
                ContentError::Other(format!("write {}: {e}", path.display()).into())
            })?;
        }

        std::process::Command::new("xdg-open")
            .arg(&path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| ContentError::Other(format!("spawn xdg-open: {e}").into()))?;

        Ok(ActionOutcome::Done {
            message: Some(format!("opened {}", self.attachment.name)),
        })
    }
}

#[async_trait]
impl Node for TaigaAttachmentNode {
    fn id(&self) -> &str {
        &self.composite_id
    }

    fn label(&self) -> &str {
        &self.attachment.name
    }

    fn node_type(&self) -> &NodeType {
        attachment_type()
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
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
                "TaigaAttachmentNode: unknown action {id}"
            ))),
        }
    }
}

fn format_file_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
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
