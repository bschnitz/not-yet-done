//! Filesystem helpers for adapters that download attachments to a
//! user-chosen directory (the `download all` batch action). Pure and
//! dependency-free, so every adapter with attachments shares the same
//! path-resolution and filename-sanitisation semantics.

use crate::ContentError;

/// Expand a leading `~` / `~/` in a user-entered path to `$HOME`. Any other
/// shape (including a bare `~user`, which we don't resolve) is returned as-is.
pub fn expand_tilde(input: &str) -> String {
    if input == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return home;
        }
    } else if let Some(rest) = input.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{}/{}", home.trim_end_matches('/'), rest);
        }
    }
    input.to_string()
}

/// Resolve and validate a user-entered target directory for a batch download:
/// trim + tilde-expand, then ensure it is a usable directory. Creates it (and
/// any missing parents) when absent; errors when the path already exists but
/// is **not** a directory, or when it can't be created/accessed (e.g.
/// permission denied). Returns the resolved, existing directory path.
pub fn prepare_target_dir(input: &str) -> Result<std::path::PathBuf, ContentError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ContentError::Other("target directory is required".to_string().into()));
    }
    let dir = std::path::PathBuf::from(expand_tilde(trimmed));
    match std::fs::metadata(&dir) {
        Ok(meta) if meta.is_dir() => Ok(dir),
        Ok(_) => Err(ContentError::Other(
            format!("{} already exists and is not a directory", dir.display()).into(),
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(&dir).map_err(|e| {
                ContentError::Other(format!("cannot create directory {}: {e}", dir.display()).into())
            })?;
            Ok(dir)
        }
        Err(e) => {
            Err(ContentError::Other(format!("cannot access {}: {e}", dir.display()).into()))
        }
    }
}

/// Sanitise an attachment filename into a single path component (drop path
/// separators). Mirrors the single-attachment `open` cache naming.
pub fn safe_attachment_name(filename: &str) -> String {
    filename.replace(['/', '\\'], "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_attachment_name_strips_separators() {
        assert_eq!(safe_attachment_name("a/b\\c.png"), "a_b_c.png");
        assert_eq!(safe_attachment_name("plain.txt"), "plain.txt");
    }

    /// A unique scratch path under the temp dir, without `Date`/random (which
    /// aren't the constraint here, but keeps test paths collision-free anyway).
    fn scratch(tag: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("nyd_download_test_{tag}"));
        p
    }

    #[test]
    fn prepare_target_dir_rejects_empty() {
        assert!(prepare_target_dir("   ").is_err());
    }

    #[test]
    fn prepare_target_dir_creates_missing() {
        let dir = scratch("creates_missing");
        let _ = std::fs::remove_dir_all(&dir);
        let nested = dir.join("a/b/c");
        let resolved = prepare_target_dir(nested.to_str().unwrap()).unwrap();
        assert!(resolved.is_dir());
        assert_eq!(resolved, nested);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prepare_target_dir_accepts_existing_dir() {
        let dir = scratch("existing_dir");
        std::fs::create_dir_all(&dir).unwrap();
        let resolved = prepare_target_dir(dir.to_str().unwrap()).unwrap();
        assert_eq!(resolved, dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prepare_target_dir_rejects_existing_file() {
        let dir = scratch("existing_file_parent");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("not_a_dir");
        std::fs::write(&file, b"x").unwrap();
        let err = prepare_target_dir(file.to_str().unwrap()).unwrap_err();
        assert!(format!("{err:?}").contains("not a directory"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
