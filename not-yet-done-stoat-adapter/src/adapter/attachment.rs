//! `stoat:attachment` — one uploaded file below a message.
//!
//! The Jira counterpart (`jira:attachment`) is the model: a metadata-only
//! leaf whose `open` action downloads the bytes into a temp dir and hands
//! the path to the frontend, which launches the OS viewer (`xdg-open`).
//! Node ids are **composite** — `<channel>/msg/<message>/file/<file_id>` —
//! so `get_by_id` can re-fetch the parent message and recover the file
//! without any server-side per-attachment endpoint (Revolt has none: files
//! only ever appear embedded in their message).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use not_yet_done_content::{
    ActionInput, ActionOutcome, ContentError, FormFieldSpec, InputSpec, Metadata, MetadataField,
    Node, NodeAction, NodeType, Result,
};

use super::message::split_composite;
use super::other_err;
use super::types::attachment_type;
use crate::client::{Attachment, StoatClient};

/// Build the composite node id the tree uses for an attachment.
pub(super) fn composite_id(channel_id: &str, message_id: &str, file_id: &str) -> String {
    format!("{channel_id}/msg/{message_id}/file/{file_id}")
}

/// Split a composite attachment id into `(channel_id, message_id, file_id)`.
/// Returns `None` for ids without the `/file/` marker — note this must be
/// tried **before** [`split_composite`], whose `/msg/` marker also matches
/// an attachment id.
pub(super) fn split_attachment_composite(id: &str) -> Option<(&str, &str, &str)> {
    let (message_part, file_id) = id.rsplit_once("/file/")?;
    let (channel_id, message_id) = split_composite(message_part)?;
    if file_id.is_empty() {
        return None;
    }
    Some((channel_id, message_id, file_id))
}

/// Actions an attachment exposes. `open` is the workhorse (download once
/// into a temp dir, then the OS viewer); `download_all` saves **every**
/// file of the parent message into a directory the user names — the same
/// pair the Jira attachment node offers. There is no `delete`: Revolt
/// cannot remove a single file from a message, only the whole message.
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
    ]
}

/// Render a byte count as a short human-readable size (`1.4 MB`). Sizes
/// are display-only, so the 1024-based rounding is deliberately coarse.
fn format_file_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Reduce a string to a safe single path component: keep `[A-Za-z0-9._-]`,
/// replace anything else with `_`.
fn sanitize_component(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Strip directory parts off a server-supplied filename and make it safe
/// to write. Falls back to a stable stem for empty / separator-only names.
pub(super) fn safe_file_name(filename: &str) -> String {
    let base = filename
        .rsplit(['/', '\\'])
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("file");
    sanitize_component(base)
}

/// Resolve `dir_input` into a usable directory: expand a leading `~`,
/// create the path (incl. parents) when missing, and reject an existing
/// non-directory before any download starts.
fn prepare_target_dir(dir_input: &str) -> Result<PathBuf> {
    let trimmed = dir_input.trim();
    if trimmed.is_empty() {
        return Err(other_err("no target directory given"));
    }
    let expanded = match trimmed.strip_prefix('~') {
        Some(rest) => {
            let home =
                dirs::home_dir().ok_or_else(|| other_err("cannot resolve home directory"))?;
            home.join(rest.trim_start_matches('/'))
        }
        None => PathBuf::from(trimmed),
    };
    if expanded.exists() && !expanded.is_dir() {
        return Err(other_err(format!(
            "{} is not a directory",
            expanded.display()
        )));
    }
    std::fs::create_dir_all(&expanded)
        .map_err(|e| other_err(format!("create {}: {e}", expanded.display())))?;
    Ok(expanded)
}

/// Per-message temp directory for opened attachments. Keyed by the message
/// id so all files of one message land together (the OS viewer's
/// sibling-navigation then pages through them) and re-opening reuses the
/// already-downloaded bytes.
fn attachment_temp_dir(message_id: &str) -> std::io::Result<PathBuf> {
    let mut dir = std::env::temp_dir();
    dir.push("not_yet_done_stoat");
    dir.push(sanitize_component(message_id));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub(super) struct StoatAttachmentNode {
    client: Arc<StoatClient>,
    channel_id: String,
    message_id: String,
    attachment: Attachment,
    composite_id: String,
    metadata: Metadata,
}

impl StoatAttachmentNode {
    pub(super) fn new(
        client: Arc<StoatClient>,
        channel_id: String,
        message_id: String,
        attachment: Attachment,
    ) -> Self {
        let composite_id = composite_id(&channel_id, &message_id, &attachment.id);
        let metadata = Metadata {
            fields: vec![
                MetadataField {
                    key: "filename".into(),
                    value: attachment.filename.clone(),
                    display_label: "Filename".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "size".into(),
                    value: attachment.size.map(format_file_size).unwrap_or_default(),
                    display_label: "Size".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "content_type".into(),
                    value: attachment.content_type.clone(),
                    display_label: "Type".into(),
                    editable: false,
                    allowed_values: None,
                },
                // Drives the row glyph in a view column and tells a future
                // inline-image renderer what it may draw.
                MetadataField {
                    key: "is_image".into(),
                    value: if attachment.is_image { "yes" } else { "no" }.into(),
                    display_label: "Image".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "url".into(),
                    value: attachment.url.clone().unwrap_or_default(),
                    display_label: "URL".into(),
                    editable: false,
                    allowed_values: None,
                },
            ],
        };
        Self {
            client,
            channel_id,
            message_id,
            attachment,
            composite_id,
            metadata,
        }
    }

    /// Download this file into the message's temp dir (reusing an existing
    /// copy) and hand the path to the frontend, which opens it with the
    /// system viewer. The adapter says WHAT to open, the frontend HOW —
    /// same contract as the message node's `open-images`.
    async fn open_external(&self) -> Result<ActionOutcome> {
        let Some(url) = self.attachment.url.as_deref() else {
            return Ok(ActionOutcome::Done {
                message: Some("File server URL not available — cannot download".into()),
            });
        };
        let dir = attachment_temp_dir(&self.message_id).map_err(|e| other_err(e.to_string()))?;
        // The autumn file id prefixes the name so two attachments sharing a
        // filename never overwrite each other.
        let path = dir.join(format!(
            "{}_{}",
            sanitize_component(&self.attachment.id),
            safe_file_name(&self.attachment.filename)
        ));
        if !path.exists() {
            let bytes = self.client.download_bytes(url).await.map_err(other_err)?;
            tokio::fs::write(&path, &bytes)
                .await
                .map_err(|e| other_err(format!("write {}: {e}", path.display())))?;
        }
        Ok(ActionOutcome::OpenExternal {
            target: path.to_string_lossy().into_owned(),
            message: Some(format!("Opening {}", self.attachment.filename)),
        })
    }

    /// Save every attachment of the parent message into `dir_input`. The
    /// sibling list is re-fetched (a node only knows its own file), so this
    /// works from any attachment row.
    async fn download_all(&self, dir_input: &str) -> Result<ActionOutcome> {
        let dir = prepare_target_dir(dir_input)?;
        let view = self
            .client
            .fetch_message(&self.channel_id, &self.message_id, None)
            .await
            .map_err(other_err)?;
        if view.attachments.is_empty() {
            return Ok(ActionOutcome::Done {
                message: Some("No attachments on this message".into()),
            });
        }
        let (saved, total, failures) =
            write_attachments(&self.client, &view.attachments, &dir).await;
        Ok(ActionOutcome::Done {
            message: Some(download_summary(&dir, saved, total, &failures)),
        })
    }
}

/// List a message's attachments as tree rows. Single fetch source behind
/// the adapter's `childs` entry for `stoat:message`; the parent message is
/// re-read because Revolt only ever ships files embedded in their message.
pub(super) async fn list_message_attachments(
    client: &Arc<StoatClient>,
    channel_id: &str,
    message_id: &str,
) -> Result<not_yet_done_content::ListResult> {
    let view = client
        .fetch_message(channel_id, message_id, None)
        .await
        .map_err(other_err)?;
    let items = view
        .attachments
        .into_iter()
        .map(|att| {
            let node = StoatAttachmentNode::new(
                Arc::clone(client),
                channel_id.to_string(),
                message_id.to_string(),
                att,
            );
            not_yet_done_content::NodeSummary {
                id: node.id().to_string(),
                label: node.label().to_string(),
                node_type: attachment_type().clone(),
                metadata: node.metadata().clone(),
                // Files are leaves.
                has_children: Some(false),
            }
        })
        .collect();
    Ok(not_yet_done_content::ListResult {
        items,
        applied_sort: Vec::new(),
        page: None,
        batch_download_available: false,
        downloaded: Vec::new(),
    })
}

/// Write every attachment into `dir`, naming each file `<file_id>_<name>`
/// so names are stable and collision-free. A per-file failure is collected
/// instead of aborting the batch. Returns `(saved, total, failures)`.
pub(super) async fn write_attachments(
    client: &StoatClient,
    attachments: &[Attachment],
    dir: &Path,
) -> (usize, usize, Vec<String>) {
    let total = attachments.len();
    let mut saved = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for att in attachments {
        let Some(url) = att.url.as_deref() else {
            failures.push(format!("{}: no download URL", att.filename));
            continue;
        };
        let path = dir.join(format!(
            "{}_{}",
            sanitize_component(&att.id),
            safe_file_name(&att.filename)
        ));
        match client.download_bytes(url).await {
            Ok(bytes) => match tokio::fs::write(&path, &bytes).await {
                Ok(()) => saved += 1,
                Err(e) => failures.push(format!("{}: {e}", att.filename)),
            },
            Err(e) => failures.push(format!("{}: {e}", att.filename)),
        }
    }
    (saved, total, failures)
}

/// Build the user-facing summary line for a batch download.
fn download_summary(dir: &Path, saved: usize, total: usize, failures: &[String]) -> String {
    let mut message = format!("Saved {saved}/{total} attachment(s) to {}", dir.display());
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
impl Node for StoatAttachmentNode {
    fn id(&self) -> &str {
        &self.composite_id
    }

    fn label(&self) -> &str {
        &self.attachment.filename
    }

    fn node_type(&self) -> &NodeType {
        attachment_type()
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        match (action_id, input) {
            ("open", ActionInput::None) => self.open_external().await,
            ("download_all", input) => {
                let dir = super::form_field(&input, "dir")?;
                self.download_all(&dir).await
            }
            (other, _) => Err(ContentError::NotSupported(format!(
                "execute: unsupported action/input for {other}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::StoatSession;

    fn test_client() -> Arc<StoatClient> {
        StoatClient::from_session(
            "https://chat.example.invalid",
            StoatSession {
                token: "synthetic".into(),
                user_id: "U0".into(),
                session_id: "S0".into(),
                session_name: "test".into(),
            },
        )
        .expect("client")
    }

    fn sample_attachment() -> Attachment {
        Attachment {
            id: "F1".into(),
            filename: "screenshot.png".into(),
            url: Some("https://autumn.example/attachments/F1/screenshot.png".into()),
            is_image: true,
            content_type: "image/png".into(),
            size: Some(1_048_576),
        }
    }

    fn node() -> StoatAttachmentNode {
        StoatAttachmentNode::new(test_client(), "C1".into(), "M1".into(), sample_attachment())
    }

    #[test]
    fn composite_id_roundtrips_and_beats_the_message_marker() {
        let id = composite_id("C1", "M1", "F1");
        assert_eq!(id, "C1/msg/M1/file/F1");
        assert_eq!(split_attachment_composite(&id), Some(("C1", "M1", "F1")));
        // A plain message id carries no `/file/` marker …
        assert_eq!(split_attachment_composite("C1/msg/M1"), None);
        // … while the message split would happily mis-read an attachment id,
        // which is why the attachment check must run first in `get_by_id`.
        assert_eq!(split_composite(&id), Some(("C1", "M1/file/F1")));
    }

    #[test]
    fn node_exposes_filename_label_and_metadata() {
        let node = node();
        assert_eq!(node.id(), "C1/msg/M1/file/F1");
        assert_eq!(node.label(), "screenshot.png");
        let fields = &node.metadata().fields;
        assert_eq!(fields[0].value, "screenshot.png");
        assert_eq!(fields[1].value, "1.0 MB");
        assert_eq!(fields[2].value, "image/png");
        assert_eq!(fields[3].value, "yes");
        assert!(fields[4].value.ends_with("/F1/screenshot.png"));
    }

    #[test]
    fn exposes_open_and_download_all_actions() {
        let ids: Vec<String> = attachment_actions().into_iter().map(|a| a.id).collect();
        assert_eq!(ids, vec!["open", "download_all"]);
    }

    #[test]
    fn file_size_is_human_readable() {
        assert_eq!(format_file_size(512), "512 B");
        assert_eq!(format_file_size(2048), "2.0 KB");
        assert_eq!(format_file_size(1_048_576), "1.0 MB");
    }

    #[test]
    fn safe_file_name_strips_paths() {
        assert_eq!(safe_file_name("a/b\\c d.png"), "c_d.png");
        assert_eq!(safe_file_name("   "), "file");
    }

    #[tokio::test]
    async fn open_without_url_reports_instead_of_failing() {
        let mut att = sample_attachment();
        att.url = None;
        let node = StoatAttachmentNode::new(test_client(), "C1".into(), "M1".into(), att);
        match node.open_external().await.unwrap() {
            ActionOutcome::Done { message } => {
                assert!(message.unwrap().contains("cannot download"));
            }
            _ => panic!("expected Done"),
        }
    }

    #[tokio::test]
    async fn download_all_validates_dir_before_network() {
        // An existing non-directory path must fail on validation — before
        // the (unreachable `.invalid`) message fetch.
        let node = node();
        let mut file = std::env::temp_dir();
        file.push("nyd_stoat_dl_not_a_dir");
        std::fs::write(&file, b"x").unwrap();
        let outcome = node.download_all(file.to_str().unwrap()).await;
        std::fs::remove_file(&file).unwrap();
        match outcome {
            Err(e) => assert!(format!("{e:?}").contains("not a directory")),
            Ok(_) => panic!("expected a validation error for a non-directory path"),
        }
    }
}
