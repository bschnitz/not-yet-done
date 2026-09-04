//! Persistent per-item workspace: `<base>/<KEY>-<slug>/ticket.md` plus an
//! `attachments/` subfolder and a `.attachments.json` sidecar.
//!
//! Adapters that can hand out a Markdown rendering of one item (Jira issues,
//! Taiga items) materialise it here so a *file* exists that other tools — the
//! `E` editor, the `o p` HTML preview — can work on. The layout is the
//! contract those tools rely on, so it lives in one place rather than once per
//! adapter; only the two adapter-specific halves are passed in: the Markdown
//! itself and the list of remote attachments (plus how to download one).
//!
//! Attachments are stored under their plain remote file name (via
//! [`crate::download::safe_attachment_name`]) so a local path
//! `attachments/<name>` matches an embed that names the file — that is what
//! lets an adapter rewrite embeds to local links without an external map.
//!
//! [`TICKET_FILE`] and [`EDIT_FILE`] are two files because they are two
//! contracts. The first is a *mirror*: every export overwrites it from the
//! server, and that is the point. The second is a *draft*: it holds text the
//! user has typed and nobody else may write it. Sharing one path made an
//! export silently eat an open editor's buffer — the editor's `mv` then handed
//! the frontend the export, which of course compared equal to what it opened
//! with, so the edit vanished without an error. Both live in the same folder
//! so `attachments/<name>` resolves from either.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ContentError;
use crate::download::safe_attachment_name;

/// Sidecar recording which attachment ids have already been fetched.
const SIDECAR: &str = ".attachments.json";
/// Subfolder holding the downloaded attachment files.
pub const ATTACH_SUBDIR: &str = "attachments";
/// The item's Markdown mirror — rewritten from the server on every export.
pub const TICKET_FILE: &str = "ticket.md";
/// The editor's own buffer inside the same folder. Never written by an
/// export, so an editing session cannot lose its text to one.
pub const EDIT_FILE: &str = "ticket.edit.md";

#[derive(Default, Serialize, Deserialize)]
struct Sidecar {
    /// Attachment id → what was downloaded for it.
    attachments: BTreeMap<String, Record>,
}

#[derive(Serialize, Deserialize)]
struct Record {
    filename: String,
    /// The remote "modification stamp" — whatever the adapter considers
    /// immutable per file (Jira: `created`; Taiga: `modified_date`).
    created: String,
    /// On-disk name under `attachments/`.
    local_name: String,
}

/// One remote attachment in the shape [`sync_attachments`] needs. Each adapter
/// maps its own DTO onto this; nothing here knows an adapter's HTTP client.
pub struct RemoteAttachment {
    /// Stable id, the sidecar key.
    pub id: String,
    /// Remote file name, as shown to the user.
    pub filename: String,
    /// Stamp that changes when the remote file changes (see [`Record::created`]).
    pub created: String,
    /// What the `fetch` closure is handed to get the bytes.
    pub url: String,
}

impl RemoteAttachment {
    /// The name this attachment gets under `attachments/`.
    pub fn local_name(&self) -> String {
        safe_attachment_name(&self.filename)
    }
}

fn io_err(msg: String) -> ContentError {
    ContentError::Other(msg.into())
}

/// Slugify an item title into a filesystem-friendly suffix: ASCII
/// alphanumerics lowercased, every other run collapsed to a single `-`,
/// trimmed, and capped to 60 characters.
pub fn slugify(title: &str) -> String {
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

/// `<base>/<KEY>-<slug>` — the persistent folder for one item. Falls back to
/// the bare key when the title slugifies to nothing.
pub fn ticket_dir(base: &Path, key: &str, title: &str) -> PathBuf {
    let slug = slugify(title);
    let name = if slug.is_empty() {
        key.to_string()
    } else {
        format!("{key}-{slug}")
    };
    base.join(name)
}

/// Download `remote` into `<dir>/attachments/`, skipping any id already
/// recorded in the sidecar with the same stamp whose local file still exists.
/// Returns the number of files newly downloaded. Remote entries that fail to
/// download abort the sync; stale local files are left untouched.
pub async fn sync_attachments<F, Fut>(
    remote: &[RemoteAttachment],
    dir: &Path,
    fetch: F,
) -> Result<usize, ContentError>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<u8>, ContentError>>,
{
    let sidecar_path = dir.join(SIDECAR);
    let mut sidecar = read_sidecar(&sidecar_path);
    let attach_dir = dir.join(ATTACH_SUBDIR);
    let mut downloaded = 0usize;

    for a in remote {
        let local_name = a.local_name();
        let already = sidecar
            .attachments
            .get(&a.id)
            .map(|r| r.created == a.created && attach_dir.join(&r.local_name).exists())
            .unwrap_or(false);
        if already {
            continue;
        }
        std::fs::create_dir_all(&attach_dir)
            .map_err(|e| io_err(format!("create {}: {e}", attach_dir.display())))?;
        let bytes = fetch(a.url.clone()).await?;
        let path = attach_dir.join(&local_name);
        std::fs::write(&path, &bytes)
            .map_err(|e| io_err(format!("write {}: {e}", path.display())))?;
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

/// Create the item folder and write `ticket.md`. Returns the folder.
pub fn write_ticket(
    base: &Path,
    key: &str,
    title: &str,
    markdown: &str,
) -> Result<PathBuf, ContentError> {
    let dir = ticket_dir(base, key, title);
    std::fs::create_dir_all(&dir).map_err(|e| io_err(format!("create {}: {e}", dir.display())))?;
    let md_path = dir.join(TICKET_FILE);
    std::fs::write(&md_path, markdown)
        .map_err(|e| io_err(format!("write {}: {e}", md_path.display())))?;
    Ok(dir)
}

/// Write the folder *and* sync its attachments — the whole `export workspace`
/// action in one call. Returns the folder written.
pub async fn materialize<F, Fut>(
    base: &Path,
    key: &str,
    title: &str,
    markdown: &str,
    remote: &[RemoteAttachment],
    fetch: F,
) -> Result<PathBuf, ContentError>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<u8>, ContentError>>,
{
    let dir = write_ticket(base, key, title, markdown)?;
    sync_attachments(remote, &dir, fetch).await?;
    Ok(dir)
}

fn read_sidecar(path: &Path) -> Sidecar {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_sidecar(path: &Path, sidecar: &Sidecar) -> Result<(), ContentError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| io_err(format!("create {}: {e}", parent.display())))?;
    }
    let json = serde_json::to_string_pretty(sidecar)
        .map_err(|e| io_err(format!("serialize attachment sidecar: {e}")))?;
    std::fs::write(path, json).map_err(|e| io_err(format!("write {}: {e}", path.display())))
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

    #[tokio::test]
    async fn sync_skips_known_and_refetches_changed() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let one = |stamp: &str| RemoteAttachment {
            id: "1".into(),
            filename: "shot.png".into(),
            created: stamp.into(),
            url: "https://example.invalid/shot.png".into(),
        };
        let fetch = |_url: String| async { Ok(b"bytes".to_vec()) };

        assert_eq!(sync_attachments(&[one("a")], dir, fetch).await.unwrap(), 1);
        assert!(dir.join(ATTACH_SUBDIR).join("shot.png").is_file());
        // Same stamp, file still there → nothing to do.
        assert_eq!(sync_attachments(&[one("a")], dir, fetch).await.unwrap(), 0);
        // Changed stamp → fetched again.
        assert_eq!(sync_attachments(&[one("b")], dir, fetch).await.unwrap(), 1);
    }

    #[test]
    fn write_ticket_creates_folder_and_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = write_ticket(tmp.path(), "PROJ-1", "Hello World", "# hi\n").unwrap();
        assert_eq!(dir, tmp.path().join("PROJ-1-hello-world"));
        assert_eq!(
            std::fs::read_to_string(dir.join(TICKET_FILE)).unwrap(),
            "# hi\n"
        );
    }
}
