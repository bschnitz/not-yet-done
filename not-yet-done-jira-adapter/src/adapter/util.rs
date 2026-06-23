//! Small, dependency-free helpers shared across the adapter submodules.

use not_yet_done_content::ContentError;

pub(super) fn other_err(msg: String) -> ContentError {
    ContentError::Other(msg.into())
}

/// Expand a leading `~` / `~/` in a user-entered path to `$HOME`. Any other
/// shape (including a bare `~user`, which we don't resolve) is returned as-is.
pub(super) fn expand_tilde(input: &str) -> String {
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
pub(super) fn prepare_target_dir(input: &str) -> Result<std::path::PathBuf, ContentError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(other_err("target directory is required".to_string()));
    }
    let dir = std::path::PathBuf::from(expand_tilde(trimmed));
    match std::fs::metadata(&dir) {
        Ok(meta) if meta.is_dir() => Ok(dir),
        Ok(_) => Err(other_err(format!(
            "{} already exists and is not a directory",
            dir.display()
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(&dir)
                .map_err(|e| other_err(format!("cannot create directory {}: {e}", dir.display())))?;
            Ok(dir)
        }
        Err(e) => Err(other_err(format!("cannot access {}: {e}", dir.display()))),
    }
}

/// Sanitise an attachment filename into a single path component (drop path
/// separators). Mirrors the single-attachment `open` cache naming.
pub(super) fn safe_attachment_name(filename: &str) -> String {
    filename.replace(['/', '\\'], "_")
}

/// Format a file size in bytes to a human-readable string.
pub(super) fn format_file_size(bytes: u64) -> String {
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

/// Truncate a body string to `max_len` chars, replacing newlines with spaces.
pub(super) fn truncate_body(body: &str, max_len: usize) -> String {
    let flat: String = body
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    if flat.len() <= max_len {
        flat
    } else {
        format!("{}…", &flat[..max_len])
    }
}

/// Ensure a string ends with exactly one `\n`. `diffy::merge` is line-aware
/// and gets confused by missing trailing newlines.
pub(super) fn ensure_trailing_newline(mut s: String) -> String {
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Collapse runs of blank/whitespace-only lines into a single empty line and
/// trim each line's trailing whitespace. Used to make body diffs robust to
/// the Jira backend reformatting descriptions (adding blank lines between
/// paragraphs etc.) — without this, a server-side reformat of the upstream
/// body made every paragraph look "changed" and diffy collapsed the entire
/// body into one giant conflict block.
pub(super) fn normalize_blank_lines(text: &str) -> String {
    let mut out = String::new();
    let mut prev_blank = false;
    for line in text.lines() {
        let trimmed = line.trim_end();
        let is_blank = trimmed.is_empty();
        if is_blank && prev_blank {
            continue;
        }
        out.push_str(trimmed);
        out.push('\n');
        prev_blank = is_blank;
    }
    out
}

/// Truncate a Jira-style ISO timestamp (`2025-06-01T10:00:00.000+0000`) to
/// `YYYY-MM-DDThh:mm`, leaving anything else untouched. Used in the
/// `edit_with_comments` per-comment header for readability.
pub(super) fn short_ts(ts: &str) -> String {
    let max = "2025-06-01T10:00".len();
    if ts.len() >= max {
        let head = &ts[..max];
        // Cheap sanity check: positions 4, 7, 10, 13 should be `-`/`-`/`T`/`:`.
        let bytes = head.as_bytes();
        if bytes.len() == max
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes[10] == b'T'
            && bytes[13] == b':'
        {
            return head.to_string();
        }
    }
    ts.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_body_short() {
        assert_eq!(truncate_body("hello", 10), "hello");
    }

    #[test]
    fn truncate_body_long() {
        let long = "a".repeat(100);
        let result = truncate_body(&long, 10);
        assert_eq!(result.len(), "aaaaaaaaaa…".len());
        assert!(result.ends_with('…'));
    }

    #[test]
    fn truncate_body_newlines() {
        assert_eq!(truncate_body("line1\nline2\nline3", 80), "line1 line2 line3");
    }

    #[test]
    fn format_file_size_bytes() {
        assert_eq!(format_file_size(500), "500 B");
    }

    #[test]
    fn format_file_size_kb() {
        assert_eq!(format_file_size(2048), "2.0 KB");
    }

    #[test]
    fn format_file_size_mb() {
        assert_eq!(format_file_size(5_242_880), "5.0 MB");
    }

    #[test]
    fn format_file_size_gb() {
        assert_eq!(format_file_size(2_147_483_648), "2.0 GB");
    }

    #[test]
    fn normalize_blank_lines_collapses_runs() {
        let input = "a\n\n\n\nb\n\n  \n\nc\n";
        assert_eq!(normalize_blank_lines(input), "a\n\nb\n\nc\n");
    }

    #[test]
    fn short_ts_truncates_iso_timestamp() {
        assert_eq!(short_ts("2025-06-01T10:00:00.000+0000"), "2025-06-01T10:00");
        assert_eq!(short_ts("not-a-timestamp"), "not-a-timestamp");
        assert_eq!(short_ts(""), "");
    }

    #[test]
    fn safe_attachment_name_strips_separators() {
        assert_eq!(safe_attachment_name("a/b\\c.png"), "a_b_c.png");
        assert_eq!(safe_attachment_name("plain.txt"), "plain.txt");
    }

    /// A unique scratch path under the temp dir, without `Date`/random (which
    /// aren't the constraint here, but keeps test paths collision-free anyway).
    fn scratch(tag: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("nyd_jira_dl_test_{tag}"));
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
