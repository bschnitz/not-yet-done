//! Read-only attachment node: metadata fields + `xdg-open` action that
//! downloads (cached) and launches the system viewer.

use std::sync::Arc;

use async_trait::async_trait;

use not_yet_done_content::*;

use crate::client::{JiraAttachment, JiraClient};

use super::types::attachment_node_type;
use super::util::{format_file_size, other_err, prepare_target_dir, safe_attachment_name};

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

pub(super) struct JiraAttachmentNode {
    client: Arc<JiraClient>,
    attachment: JiraAttachment,
    /// The parent issue key — needed to list **all** sibling attachments for
    /// the `download_all` action.
    issue_key: String,
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
            issue_key,
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

    /// Download **every** attachment of the parent issue into `dir_input`.
    ///
    /// The target directory is resolved and validated first
    /// ([`prepare_target_dir`]): a leading `~` is expanded, a missing
    /// directory (incl. parents) is created, an existing non-directory or an
    /// inaccessible path is reported as an error before any download starts.
    /// Each file is then fetched and written via the shared
    /// [`write_attachments`] helper (id-prefixed name `<id>-<filename>`), and
    /// the summary reports how many of how many succeeded plus any failures.
    async fn download_all(&self, dir_input: &str) -> Result<ActionOutcome> {
        let dir = prepare_target_dir(dir_input)?;

        let attachments = self
            .client
            .get_attachments(&self.issue_key)
            .await
            .map_err(other_err)?;
        if attachments.is_empty() {
            return Ok(ActionOutcome::Done {
                message: Some(format!("{}: no attachments to download", self.issue_key)),
            });
        }

        let (saved, total, failures) =
            write_attachments(&self.client, &attachments, &dir).await;
        Ok(ActionOutcome::Done {
            message: Some(download_summary(&self.issue_key, &dir, saved, total, &failures)),
        })
    }
}

/// Write every attachment into `dir`, naming each file `<id>-<filename>`. The
/// id prefix is applied unconditionally (not only on collision), so names are
/// stable, collision-free, and match exactly what the issue node's
/// `export-bundle` action reports as `written_name`. A per-file network/IO
/// failure is collected instead of aborting the batch. Returns
/// `(saved, total, failures)`.
pub(super) async fn write_attachments(
    client: &JiraClient,
    attachments: &[JiraAttachment],
    dir: &std::path::Path,
) -> (usize, usize, Vec<String>) {
    let total = attachments.len();
    let mut saved = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for a in attachments {
        let name = format!("{}-{}", a.id, safe_attachment_name(&a.filename));
        let path = dir.join(&name);
        match client.download_attachment(&a.content_url).await {
            Ok(bytes) => match std::fs::write(&path, &bytes) {
                Ok(()) => saved += 1,
                Err(e) => failures.push(format!("{}: {e}", a.filename)),
            },
            Err(e) => failures.push(format!("{}: {e}", a.filename)),
        }
    }

    (saved, total, failures)
}

/// Build the user-facing summary line for a batch download.
pub(super) fn download_summary(
    issue_key: &str,
    dir: &std::path::Path,
    saved: usize,
    total: usize,
    failures: &[String],
) -> String {
    let mut message = format!(
        "{issue_key}: saved {saved}/{total} attachment(s) to {}",
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

    /// `delete` opts into the frontend's generic delete plumbing: returning
    /// [`ActionDispatch::DeleteSelf`] makes the TUI show a `(y/n)` prompt and,
    /// on confirm, call `execute("delete", None)` here — which performs the
    /// REST delete and lets the pane reload. The adapter authors the prompt
    /// because only it knows the attachment's filename.
    async fn invoke_action(&self, name: &str, _ctx: &ActionContext) -> Result<ActionDispatch> {
        match name {
            "delete" => Ok(ActionDispatch::DeleteSelf {
                confirm: Some(format!(
                    "Delete attachment '{}'? (y/n)",
                    self.attachment.filename
                )),
            }),
            _ => Ok(ActionDispatch::Noop),
        }
    }

    async fn execute(
        &mut self,
        action_id: &str,
        input: ActionInput,
    ) -> Result<ActionOutcome> {
        match (action_id, input) {
            ("open", ActionInput::None) => self.open_via_xdg().await,
            ("download_all", ActionInput::Form(values)) => {
                let dir = values.get("dir").map(String::as_str).unwrap_or("");
                self.download_all(dir).await
            }
            ("delete", ActionInput::None) => {
                self.client
                    .delete_attachment(&self.attachment.id)
                    .await
                    .map_err(other_err)?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("deleted {}", self.attachment.filename)),
                })
            }
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
    fn attachment_node_exposes_open_download_all_and_delete_actions() {
        let node = JiraAttachmentNode::new(test_client(), sample_attachment(), "PROJ-42".into());
        let actions = node.actions();
        let ids: Vec<&str> = actions.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["open", "download_all", "delete"]);
        assert!(matches!(actions[0].input, InputSpec::None));
        assert!(matches!(actions[1].input, InputSpec::Form { .. }));
        assert!(matches!(actions[2].input, InputSpec::None));
    }

    #[tokio::test]
    async fn delete_requests_confirmation_with_filename() {
        let node = JiraAttachmentNode::new(test_client(), sample_attachment(), "PROJ-42".into());
        let dispatch = node
            .invoke_action("delete", &ActionContext::default())
            .await
            .unwrap();
        match dispatch {
            ActionDispatch::DeleteSelf { confirm: Some(p) } => {
                assert!(p.contains("screenshot.png"), "{p}");
            }
            other => panic!("expected DeleteSelf with a prompt, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn download_all_validates_dir_before_network() {
        // An existing non-directory path must fail fast on validation — before
        // any client call (the test client points at port 0 and could never
        // download anything).
        let node = JiraAttachmentNode::new(test_client(), sample_attachment(), "PROJ-42".into());
        let mut file = std::env::temp_dir();
        file.push("nyd_jira_dl_node_not_a_dir");
        std::fs::write(&file, b"x").unwrap();
        let outcome = node.download_all(file.to_str().unwrap()).await;
        match outcome {
            Err(e) => assert!(format!("{e:?}").contains("not a directory")),
            Ok(_) => panic!("expected a validation error for a non-directory path"),
        }
        std::fs::remove_file(&file).unwrap();
    }
}
