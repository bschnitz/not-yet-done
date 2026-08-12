//! Persistent per-ticket workspace: `<base>/<KEY>-<slug>/ticket.md` plus an
//! `attachments/` subfolder. Shared by two entry points:
//!
//! - `edit_markdown` (the `E` action) opens `ticket.md` in place instead of a
//!   throwaway `$TMPDIR` file and syncs attachments on demand;
//! - `export_workspace` materialises the same folder without an editor or a
//!   Jira write-back.
//!
//! Attachments are named by their plain Jira filename (via
//! [`safe_attachment_name`]) so the local path `attachments/<name>` matches the
//! `!name!` wiki embed exactly — that's what lets the image conversion in
//! [`super::wiki_md`] resolve embeds to local links with no external map.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use not_yet_done_content::Result;

use super::super::util::{other_err, safe_attachment_name};
use crate::client::JiraClient;

/// Sidecar recording which attachment ids we have already fetched. Jira
/// attachments are immutable (added/removed, never edited), so an id present
/// here — with its local file still on disk — needs no re-download.
const SIDECAR: &str = ".attachments.json";
/// Subfolder holding the downloaded attachment files.
const ATTACH_SUBDIR: &str = "attachments";
/// The single Markdown file inside a ticket folder.
pub(in crate::adapter) const TICKET_FILE: &str = "ticket.md";

#[derive(Default, Serialize, Deserialize)]
struct Sidecar {
    /// Attachment id → what we downloaded for it.
    attachments: BTreeMap<String, Record>,
}

#[derive(Serialize, Deserialize)]
struct Record {
    filename: String,
    /// Jira `created` timestamp — the immutable "modification stamp".
    created: String,
    /// On-disk name under `attachments/`.
    local_name: String,
}

/// Slugify a ticket title into a filesystem-friendly suffix: ASCII
/// alphanumerics lowercased, every other run collapsed to a single `-`,
/// trimmed, and capped to 60 characters.
pub(in crate::adapter) fn slugify(title: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !slug.is_empty() && !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    slug.truncate(
        slug.char_indices()
            .nth(60)
            .map(|(i, _)| i)
            .unwrap_or(slug.len()),
    );
    slug.trim_matches('-').to_string()
}

/// `<base>/<KEY>-<slug>` — the persistent folder for one ticket. Falls back to
/// the bare key when the title slugifies to nothing.
pub(in crate::adapter) fn ticket_dir(base: &Path, key: &str, title: &str) -> PathBuf {
    let slug = slugify(title);
    let name = if slug.is_empty() {
        key.to_string()
    } else {
        format!("{key}-{slug}")
    };
    base.join(name)
}

/// Download this issue's attachments into `<dir>/attachments/`, skipping any
/// id already recorded in the sidecar whose local file still exists. Returns
/// the number of files newly downloaded. Missing/removed remote attachments
/// are simply not fetched; stale local files are left untouched.
pub(in crate::adapter) async fn sync_attachments(
    client: &JiraClient,
    key: &str,
    dir: &Path,
) -> Result<usize> {
    let attachments = client.get_attachments(key).await.map_err(other_err)?;
    let sidecar_path = dir.join(SIDECAR);
    let mut sidecar = read_sidecar(&sidecar_path);
    let attach_dir = dir.join(ATTACH_SUBDIR);
    let mut downloaded = 0usize;

    for a in &attachments {
        let local_name = safe_attachment_name(&a.filename);
        let already = sidecar
            .attachments
            .get(&a.id)
            .map(|r| attach_dir.join(&r.local_name).exists())
            .unwrap_or(false);
        if already {
            continue;
        }
        std::fs::create_dir_all(&attach_dir)
            .map_err(|e| other_err(format!("create {}: {e}", attach_dir.display())))?;
        let bytes = client
            .download_attachment(&a.content_url)
            .await
            .map_err(other_err)?;
        let path = attach_dir.join(&local_name);
        std::fs::write(&path, &bytes)
            .map_err(|e| other_err(format!("write {}: {e}", path.display())))?;
        sidecar.attachments.insert(
            a.id.clone(),
            Record {
                filename: a.filename.clone(),
                created: a.created.clone(),
                local_name,
            },
        );
        downloaded += 1;
    }

    if downloaded > 0 {
        write_sidecar(&sidecar_path, &sidecar)?;
    }
    Ok(downloaded)
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
    let dir = ticket_dir(base, key, title);
    std::fs::create_dir_all(&dir)
        .map_err(|e| other_err(format!("create {}: {e}", dir.display())))?;
    let md_path = dir.join(TICKET_FILE);
    std::fs::write(&md_path, markdown)
        .map_err(|e| other_err(format!("write {}: {e}", md_path.display())))?;
    sync_attachments(client, key, &dir).await?;
    Ok(dir)
}

fn read_sidecar(path: &Path) -> Sidecar {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_sidecar(path: &Path, sidecar: &Sidecar) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| other_err(format!("create {}: {e}", parent.display())))?;
    }
    let json = serde_json::to_string_pretty(sidecar)
        .map_err(|e| other_err(format!("serialize attachment sidecar: {e}")))?;
    std::fs::write(path, json).map_err(|e| other_err(format!("write {}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Fix the login bug!"), "fix-the-login-bug");
        assert_eq!(slugify("  Trailing / slashes  "), "trailing-slashes");
        assert_eq!(slugify("***"), "");
        assert_eq!(slugify("CamelCase123"), "camelcase123");
    }

    #[test]
    fn slugify_caps_length() {
        let long = "a".repeat(200);
        assert_eq!(slugify(&long).len(), 60);
    }

    #[test]
    fn ticket_dir_uses_key_and_slug() {
        let base = Path::new("/base");
        assert_eq!(
            ticket_dir(base, "PROJ-1", "Hello World"),
            Path::new("/base/PROJ-1-hello-world")
        );
        // Empty slug → bare key.
        assert_eq!(ticket_dir(base, "PROJ-1", "***"), Path::new("/base/PROJ-1"));
    }
}
