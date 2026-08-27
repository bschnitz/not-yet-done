//! Taiga's half of the persistent per-item workspace
//! `<base>/<ref>-<slug>/ticket.md` + `attachments/`.
//!
//! The layout, the sidecar and the slug rule live in
//! [`not_yet_done_content::workspace`] — shared with the Jira adapter, which
//! writes the same folder shape, so one HTML preview script can render either.
//! What stays here is only what needs a [`TaigaClient`]: mapping Taiga's
//! attachment DTOs onto [`RemoteAttachment`] and downloading one.

use std::path::{Path, PathBuf};

use not_yet_done_content::Result;
use not_yet_done_content::workspace::{self as shared, RemoteAttachment};

use crate::client::{TaigaAttachment, TaigaClient, download_attachment};

/// Taiga attachments *can* be replaced in place, so the sidecar compares
/// `modified_date` rather than the creation stamp.
fn remote(attachments: &[TaigaAttachment]) -> Vec<RemoteAttachment> {
    attachments
        .iter()
        .map(|a| RemoteAttachment {
            id: a.id.to_string(),
            filename: a.name.clone(),
            created: a.modified_date.clone(),
            url: a.url.clone(),
        })
        .collect()
}

/// Create the item folder, write `ticket.md`, and download the attachments —
/// the whole `export_workspace` action. Returns the folder written.
pub(super) async fn materialize(
    client: &TaigaClient,
    key: &str,
    title: &str,
    markdown: &str,
    attachments: &[TaigaAttachment],
    base: &Path,
) -> Result<PathBuf> {
    let remote = remote(attachments);
    shared::materialize(base, key, title, markdown, &remote, |url| async move {
        download_attachment(client, &url)
            .await
            .map_err(|e| not_yet_done_content::ContentError::Other(e.into()))
    })
    .await
}
