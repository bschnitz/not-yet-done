//! Parse a markdown checkbox tree back into structured items.
//!
//! Handles inconsistent indentation robustly by tracking the indent
//! width per depth level top-down.

use not_yet_done_task_core::entity::task::TaskStatus;

/// A single parsed item from the markdown tree.
/// Known flags that can appear between the marker and description.
const KNOWN_FLAGS: &[char] = &['t'];

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedItem {
    pub short_id: Option<String>,
    pub description: String,
    pub status: TaskStatus,
    pub deleted: bool,
    pub flags: Vec<char>,
    pub priority: Option<i32>,
    /// Depth in the tree (0 = root).
    pub depth: usize,
}

impl ParsedItem {
    pub fn has_flag(&self, flag: char) -> bool {
        self.flags.contains(&flag)
    }
}

/// Parse error with line number.
#[derive(Debug, Clone)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Line {}: {}", self.line, self.message)
    }
}

/// Parse the markdown content into a flat list of items with depth info.
pub fn parse(content: &str) -> Result<Vec<ParsedItem>, ParseError> {
    let mut items = Vec::new();
    let mut indent_levels: Vec<usize> = Vec::new();

    // Strip BOM if present.
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);

    for (line_idx, line) in content.lines().enumerate() {
        let line_num = line_idx + 1;
        let trimmed = line.trim();

        // Skip blank lines and comments.
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Must start with "- [" after indentation.
        let indent = count_leading_spaces(line);
        let rest = line[indent..].trim_start();

        if !rest.starts_with("- [") {
            return Err(ParseError {
                line: line_num,
                message: format!("Expected '- [' but got: {}", truncate(rest, 40)),
            });
        }

        // Parse marker: - [X] ...
        let after_dash = &rest[2..]; // skip "- "
        let close = after_dash.find(']').ok_or_else(|| ParseError {
            line: line_num,
            message: "Missing closing ']'".into(),
        })?;
        let marker_str = &after_dash[1..close]; // between [ and ]
        let (status, deleted) = parse_marker(marker_str).ok_or_else(|| ParseError {
            line: line_num,
            message: format!(
                "Unknown marker '[{}]' (bytes: {:?}). Use [ ], [x], [X], [~], [/], [-], [D]",
                marker_str,
                marker_str.as_bytes(),
            ),
        })?;

        // Everything after "] "
        let after_bracket = after_dash[close + 1..].trim_start();

        // Parse flags (e.g. "-t -f") before the description.
        let (flags, rest_after_flags) = parse_flags(after_bracket, line_num)?;

        // Parse description and optional (p=... id=...) suffix.
        let (description, short_id, priority) = parse_suffix(rest_after_flags);

        if description.is_empty() {
            return Err(ParseError {
                line: line_num,
                message: "Description is empty".into(),
            });
        }

        // Determine depth from indentation.
        let depth = resolve_depth(indent, &mut indent_levels);

        items.push(ParsedItem {
            short_id,
            description,
            status,
            deleted,
            flags,
            priority,
            depth,
        });
    }

    Ok(items)
}

/// Resolve the tree depth for a given indent column count.
///
/// Strategy (top-down, adaptive):
/// 1. Exact match with a known level → use that depth.
/// 2. Greater than all known levels → new child depth.
/// 3. Between two known levels → snap to the deeper one and update its
///    indent (the user changed their indentation style mid-tree).
/// 4. Less than or equal to the shallowest known level → depth 0.
fn resolve_depth(indent: usize, levels: &mut Vec<usize>) -> usize {
    if levels.is_empty() {
        levels.push(indent);
        return 0;
    }

    // Exact match.
    for (depth, &level_indent) in levels.iter().enumerate() {
        if indent == level_indent {
            return depth;
        }
    }

    // Greater than deepest → new child.
    if indent > levels.last().copied().unwrap_or(0) {
        let depth = levels.len();
        levels.push(indent);
        return depth;
    }

    // Between known levels: find the first level whose indent is strictly
    // greater than ours. That level (or the one before it) is our depth.
    // Then update the level's indent to this new value.
    for depth in (1..levels.len()).rev() {
        if indent < levels[depth] && indent > levels[depth - 1] {
            // User redefined what this depth looks like.
            levels[depth] = indent;
            // Trim deeper levels — they're invalidated.
            levels.truncate(depth + 1);
            return depth;
        }
    }

    // Fallback: shallower than everything known → depth 0.
    levels[0] = indent;
    0
}

/// Returns (status, deleted).
fn parse_marker(s: &str) -> Option<(TaskStatus, bool)> {
    let s = s.trim();
    match s {
        "" | " " => Some((TaskStatus::Todo, false)),
        "x" | "X" => Some((TaskStatus::Done, false)),
        "~" | "/" => Some((TaskStatus::InProgress, false)),
        "-" => Some((TaskStatus::Cancelled, false)),
        "D" | "d" => Some((TaskStatus::Todo, true)),
        _ => None,
    }
}

/// Parse flags like `-t`, `-f` at the start of the text after `] `.
/// Returns (flags, remaining text). Errors on unknown flags.
fn parse_flags(text: &str, line_num: usize) -> Result<(Vec<char>, &str), ParseError> {
    let mut flags = Vec::new();
    let mut rest = text;

    loop {
        let trimmed = rest.trim_start();
        if !trimmed.starts_with('-') || trimmed.len() < 2 {
            break;
        }
        // Check if this is a flag: "-X " where X is a single letter.
        let flag_candidate = &trimmed[1..];
        let ch = flag_candidate.chars().next().unwrap();
        // Must be a letter followed by whitespace or end — not a description word.
        let after_ch = &flag_candidate[ch.len_utf8()..];
        if !ch.is_ascii_alphabetic() || (!after_ch.is_empty() && !after_ch.starts_with(' ')) {
            break;
        }

        if !KNOWN_FLAGS.contains(&ch) {
            return Err(ParseError {
                line: line_num,
                message: format!(
                    "Unknown flag '-{ch}'. Known flags: {}",
                    KNOWN_FLAGS
                        .iter()
                        .map(|f| format!("-{f}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        }

        flags.push(ch);
        rest = after_ch.trim_start();
    }

    Ok((flags, rest))
}

/// Parse the text after `] `, extracting description and optional `(p=... id=...)`.
fn parse_suffix(text: &str) -> (String, Option<String>, Option<i32>) {
    // Look for trailing parenthesized metadata: (p=... id=...)
    let text = text.trim();
    if let Some(paren_start) = text.rfind('(') {
        if text.ends_with(')') {
            let meta = &text[paren_start + 1..text.len() - 1];
            let desc = text[..paren_start].trim().to_string();
            let (id, priority) = parse_meta(meta);
            return (desc, id, priority);
        }
    }
    // No metadata.
    (text.to_string(), None, None)
}

/// Parse metadata like `p=5  id=1d88c39d`.
fn parse_meta(meta: &str) -> (Option<String>, Option<i32>) {
    let mut id = None;
    let mut priority = None;

    for part in meta.split_whitespace() {
        if let Some(val) = part.strip_prefix("id=") {
            if !val.is_empty() {
                id = Some(val.to_string());
            }
        } else if let Some(val) = part.strip_prefix("p=") {
            priority = val.parse().ok();
        }
    }
    (id, priority)
}

fn count_leading_spaces(s: &str) -> usize {
    s.len() - s.trim_start_matches(' ').len()
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}…", &s[..max])
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple() {
        let input = "- [ ] Hello  (p=3  id=abcd1234)\n";
        let items = parse(input).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].description, "Hello");
        assert_eq!(items[0].status, TaskStatus::Todo);
        assert_eq!(items[0].priority, Some(3));
        assert_eq!(items[0].short_id, Some("abcd1234".into()));
        assert_eq!(items[0].depth, 0);
    }

    #[test]
    fn parse_nested() {
        let input = "\
- [ ] Root  (id=aaa)
  - [x] Child 1  (id=bbb)
    - [~] Grandchild  (id=ccc)
  - [-] Child 2  (id=ddd)
";
        let items = parse(input).unwrap();
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].depth, 0);
        assert_eq!(items[1].depth, 1);
        assert_eq!(items[2].depth, 2);
        assert_eq!(items[3].depth, 1);
        assert_eq!(items[2].status, TaskStatus::InProgress);
        assert_eq!(items[3].status, TaskStatus::Cancelled);
    }

    #[test]
    fn parse_inconsistent_indent() {
        let input = "\
- [ ] Root
    - [ ] Child 0
  - [ ] Child 1
    - [ ] Grandchild under 1
";
        let items = parse(input).unwrap();
        assert_eq!(items[1].depth, 1); // indent 4 → new depth 1
        assert_eq!(items[2].depth, 1); // indent 2 → between 0 and 4, snaps to depth 1 (updated to 2)
        assert_eq!(items[3].depth, 2); // indent 4 → new depth 2 (after level 1 was redefined to 2)
    }

    #[test]
    fn parse_new_item_no_metadata() {
        let input = "- [ ] New task without metadata\n";
        let items = parse(input).unwrap();
        assert_eq!(items[0].short_id, None);
        assert_eq!(items[0].priority, None);
        assert_eq!(items[0].description, "New task without metadata");
    }

    #[test]
    fn parse_slash_marker() {
        let input = "- [/] In progress task  (id=abc)\n";
        let items = parse(input).unwrap();
        assert_eq!(items[0].status, TaskStatus::InProgress);
    }

    #[test]
    fn parse_deleted_marker() {
        let input = "- [D] Deleted task  (id=abc)\n";
        let items = parse(input).unwrap();
        assert_eq!(items[0].status, TaskStatus::Todo);
        assert!(items[0].deleted);
    }

    #[test]
    fn parse_non_deleted_items_are_not_deleted() {
        let input = "- [ ] Normal task\n";
        let items = parse(input).unwrap();
        assert!(!items[0].deleted);
    }

    #[test]
    fn error_on_bad_marker() {
        let input = "- [?] Unknown marker\n";
        assert!(parse(input).is_err());
    }

    #[test]
    fn error_on_empty_description() {
        let input = "- [ ]   (id=abc)\n";
        assert!(parse(input).is_err());
    }
}
