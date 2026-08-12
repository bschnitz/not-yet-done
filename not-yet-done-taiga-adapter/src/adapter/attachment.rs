//! Read-only attachment node — `xdg-open` action that downloads the file
//! once into a per-user temp dir and launches the system viewer detached.

use std::sync::Arc;

use async_trait::async_trait;

use not_yet_done_content::*;

use not_yet_done_content::download::{prepare_target_dir, safe_attachment_name};

use super::types::attachment_type;
use crate::client::{
    ItemType, TaigaAttachment, TaigaClient, delete_attachment, download_attachment,
    list_attachments,
};

pub(super) fn attachment_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new("open", "open", InputSpec::None),
        NodeAction::new(
            "download_all",
            "download all",
            InputSpec::Form {
                fields: vec![FormFieldSpec::text("dir", "Target directory")],
            },
        ),
        NodeAction::new("delete", "delete", InputSpec::None),
    ]
}

pub(super) struct TaigaAttachmentNode {
    client: Arc<TaigaClient>,
    attachment: TaigaAttachment,
    /// Owning item's type — the attachment delete endpoint segment
    /// (`/api/v1/{seg}/attachments/{id}`) is item-type specific.
    item_type: ItemType,
    /// Owning item's numeric id — needed (with `project_id`) to list **all**
    /// sibling attachments for the `download_all` action.
    item_id: u64,
    /// Owning project's id — the Taiga attachment-list endpoint requires both
    /// `object_id` and `project` filters.
    project_id: u64,
    /// Parent item's composite id (e.g. `task:1`) — used in the batch summary.
    parent_id: String,
    composite_id: String,
    metadata: Metadata,
}

impl TaigaAttachmentNode {
    pub(super) fn new(
        client: Arc<TaigaClient>,
        attachment: TaigaAttachment,
        item_type: ItemType,
        parent_id: String,
        item_id: u64,
        project_id: u64,
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
        Self {
            client,
            attachment,
            item_type,
            item_id,
            project_id,
            parent_id,
            composite_id,
            metadata,
        }
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

    /// Download **every** attachment of the parent item into `dir_input`.
    ///
    /// The target directory is resolved and validated first
    /// ([`prepare_target_dir`]): a leading `~` is expanded, a missing
    /// directory (incl. parents) is created, an existing non-directory or an
    /// inaccessible path is reported as an error before any download starts.
    /// The full sibling set is then listed (Taiga needs both the item id and
    /// the project id) and each file is fetched and written via the shared
    /// [`write_attachments`] helper (id-prefixed name `<id>-<name>`); the
    /// summary reports how many of how many succeeded plus any failures.
    async fn download_all(&self, dir_input: &str) -> Result<ActionOutcome> {
        let dir = prepare_target_dir(dir_input)?;

        let attachments =
            list_attachments(&self.client, self.item_type, self.item_id, self.project_id)
                .await
                .map_err(|e| ContentError::Other(e.into()))?;
        if attachments.is_empty() {
            return Ok(ActionOutcome::Done {
                message: Some(format!("{}: no attachments to download", self.parent_id)),
            });
        }

        let (saved, total, failures) = write_attachments(&self.client, &attachments, &dir).await;
        Ok(ActionOutcome::Done {
            message: Some(download_summary(
                &self.parent_id,
                &dir,
                saved,
                total,
                &failures,
            )),
        })
    }
}

/// Write every attachment into `dir`, naming each file `<id>-<name>`. The id
/// prefix is applied unconditionally (not only on collision), so names are
/// stable and collision-free and match the single-attachment `open` cache
/// naming. A per-file network/IO failure is collected instead of aborting the
/// batch. Returns `(saved, total, failures)`.
async fn write_attachments(
    client: &TaigaClient,
    attachments: &[TaigaAttachment],
    dir: &std::path::Path,
) -> (usize, usize, Vec<String>) {
    let total = attachments.len();
    let mut saved = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for a in attachments {
        let name = format!("{}-{}", a.id, safe_attachment_name(&a.name));
        let path = dir.join(&name);
        match download_attachment(client, &a.url).await {
            Ok(bytes) => match std::fs::write(&path, &bytes) {
                Ok(()) => saved += 1,
                Err(e) => failures.push(format!("{}: {e}", a.name)),
            },
            Err(e) => failures.push(format!("{}: {e}", a.name)),
        }
    }

    (saved, total, failures)
}

/// Build the user-facing summary line for a batch download.
fn download_summary(
    parent_id: &str,
    dir: &std::path::Path,
    saved: usize,
    total: usize,
    failures: &[String],
) -> String {
    let mut message = format!(
        "{parent_id}: saved {saved}/{total} attachment(s) to {}",
        dir.display()
    );
    if !failures.is_empty() {
        message.push_str(&format!(
            " — {} failed ({})",
            failures.len(),
            failures.join("; ")
        ));
    }
    message
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
    /// `delete` opts into the frontend's generic delete plumbing: returning
    /// [`ActionDispatch::DeleteSelf`] makes the TUI show a `(y/n)` prompt and,
    /// on confirm, call `execute("delete", None)` here. The adapter authors
    /// the prompt because only it knows the attachment's filename.
    async fn invoke_action(&self, name: &str, _ctx: &ActionContext) -> Result<ActionDispatch> {
        match name {
            "delete" => Ok(ActionDispatch::DeleteSelf {
                confirm: Some(format!(
                    "Delete attachment '{}'? (y/n)",
                    self.attachment.name
                )),
            }),
            _ => Ok(ActionDispatch::Noop),
        }
    }

    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        match (action_id, input) {
            ("open", ActionInput::None) => self.open_via_xdg().await,
            ("download_all", ActionInput::Form(values)) => {
                let dir = values.get("dir").map(String::as_str).unwrap_or("");
                self.download_all(dir).await
            }
            ("delete", ActionInput::None) => {
                delete_attachment(&self.client, self.item_type, self.attachment.id)
                    .await
                    .map_err(|e| ContentError::Other(e.into()))?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("deleted {}", self.attachment.name)),
                })
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_actions_expose_open_download_all_and_delete() {
        let actions = attachment_actions();
        let ids: Vec<&str> = actions.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["open", "download_all", "delete"]);
        assert!(matches!(actions[0].input, InputSpec::None));
        assert!(matches!(actions[1].input, InputSpec::Form { .. }));
        assert!(matches!(actions[2].input, InputSpec::None));
    }

    #[test]
    fn download_summary_reports_counts() {
        let dir = std::path::Path::new("/tmp/out");
        let msg = download_summary("task:1", dir, 3, 3, &[]);
        assert_eq!(msg, "task:1: saved 3/3 attachment(s) to /tmp/out");
    }

    #[test]
    fn download_summary_appends_failures() {
        let dir = std::path::Path::new("/tmp/out");
        let failures = vec!["a.png: boom".to_string()];
        let msg = download_summary("task:1", dir, 1, 2, &failures);
        assert!(msg.starts_with("task:1: saved 1/2 attachment(s) to /tmp/out"));
        assert!(msg.contains("1 failed (a.png: boom)"));
    }
}
