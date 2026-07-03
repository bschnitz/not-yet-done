//! Small, dependency-free helpers shared across the adapter submodules.

use not_yet_done_content::ContentError;

// Path resolution + filename sanitisation for the `download all` batch action
// are shared across adapters — re-export the canonical helpers from the
// content crate so existing `use super::util::{…}` sites keep resolving.
pub(super) use not_yet_done_content::download::{prepare_target_dir, safe_attachment_name};

pub(super) fn other_err(msg: String) -> ContentError {
    ContentError::Other(msg.into())
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

    // Path-resolution + filename-sanitisation helpers moved to
    // `not_yet_done_content::download`; their tests live there now.
}
