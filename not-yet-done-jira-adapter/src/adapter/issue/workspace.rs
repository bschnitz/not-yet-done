//! Jira's half of the persistent per-ticket workspace
//! `<base>/<KEY>-<slug>/ticket.md` + `attachments/`. The layout, the sidecar
//! and the slug rule live in [`not_yet_done_content::workspace`]; what stays
//! here is only what needs a [`JiraClient`]: turning Jira's attachment DTOs
//! into [`RemoteAttachment`]s and downloading one.
//!
//! Two entry points use it:
//!
//! - `edit_markdown` (the `E` action) opens `ticket.md` in place instead of a
//!   throwaway `$TMPDIR` file and syncs attachments on demand;
//! - `export_workspace` materialises the same folder without an editor or a
//!   Jira write-back.
//!
//! Attachments keep their plain Jira filename so the local path
//! `attachments/<name>` matches the `!name!` wiki embed exactly — that's what
//! lets the image conversion in [`super::wiki_md`] resolve embeds to local
//! links with no external map.

use std::path::{Path, PathBuf};

use not_yet_done_content::Result;
use not_yet_done_content::workspace::{self as shared, RemoteAttachment};

use super::super::util::other_err;
use crate::client::JiraClient;

/// The single Markdown file inside a ticket folder.
pub(in crate::adapter) const TICKET_FILE: &str = shared::TICKET_FILE;

pub(in crate::adapter) use shared::ticket_dir;

/// Jira attachments are immutable (added/removed, never edited), so `created`
/// doubles as the stamp the sidecar compares against.
async fn remote_attachments(client: &JiraClient, key: &str) -> Result<Vec<RemoteAttachment>> {
    let attachments = client.get_attachments(key).await.map_err(other_err)?;
    Ok(attachments
        .into_iter()
        .map(|a| RemoteAttachment {
            id: a.id,
            filename: a.filename,
            created: a.created,
            url: a.content_url,
        })
        .collect())
}

/// Download this issue's attachments into `<dir>/attachments/`, skipping any
/// id already recorded in the sidecar whose local file still exists. Returns
/// the number of files newly downloaded.
pub(in crate::adapter) async fn sync_attachments(
    client: &JiraClient,
    key: &str,
    dir: &Path,
) -> Result<usize> {
    let remote = remote_attachments(client, key).await?;
    shared::sync_attachments(&remote, dir, |url| async move {
        client.download_attachment(&url).await.map_err(other_err)
    })
    .await
}

/// Create the ticket folder, write `ticket.md`, and sync attachments — the
/// non-editor path behind `export_workspace`. Returns the folder written.
pub(in crate::adapter) async fn materialize(
    client: &JiraClient,
    key: &str,
    title: &str,
    markdown: &str,
    base: &Path,
) -> Result<PathBuf> {
    let remote = remote_attachments(client, key).await?;
    shared::materialize(base, key, title, markdown, &remote, |url| async move {
        client.download_attachment(&url).await.map_err(other_err)
    })
    .await
}
