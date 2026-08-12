//! `:focus-node` — scroll a content view's cursor to the first row whose
//! configured column matches a user-supplied pattern.
//!
//! Path syntax (analogous to `:focus-task`, but each segment may include
//! an explicit column hint):
//!
//! ```text
//! :focus-node [-i] <tab>[:<view>] /<seg>[/<seg>...]
//!
//! <seg>  := <col>|<pattern>   match `metadata.<col>` (or `id`/`label`) against pattern
//!         | <pattern>         match `label` + all metadata values
//! <pattern> := <substring>            default
//!            | re:<regex>             opt-in to regex
//! ```
//!
//! Multi-segment paths are reserved for tree drill-down content views
//! (Postgres `schemas → tables → rows`) and currently return
//! [`FocusError::MultiSegmentUnsupported`].

use std::collections::BTreeSet;

use regex::RegexBuilder;

use not_yet_done_content::NodeSummary;

/// Single-segment matcher: either substring or regex.
#[derive(Debug)]
enum Pattern {
    Substring {
        needle: String,
        case_insensitive: bool,
    },
    Regex(regex::Regex),
}

impl Pattern {
    fn parse(input: &str, case_insensitive: bool) -> Result<Self, String> {
        if let Some(rx) = input.strip_prefix("re:") {
            RegexBuilder::new(rx)
                .case_insensitive(case_insensitive)
                .build()
                .map(Pattern::Regex)
                .map_err(|e| format!("bad regex '{rx}': {e}"))
        } else {
            Ok(Pattern::Substring {
                needle: input.to_string(),
                case_insensitive,
            })
        }
    }

    fn matches(&self, text: &str) -> bool {
        match self {
            Pattern::Substring {
                needle,
                case_insensitive: true,
            } => text.to_lowercase().contains(&needle.to_lowercase()),
            Pattern::Substring {
                needle,
                case_insensitive: false,
            } => text.contains(needle),
            Pattern::Regex(rx) => rx.is_match(text),
        }
    }
}

#[derive(Debug)]
pub struct FocusSegment {
    /// `None` → match against the concatenation of label + all metadata values.
    /// `Some("id")` / `Some("label")` → match those node-summary fields directly.
    /// `Some(other)` → match `metadata.fields[key == other].value`.
    pub column: Option<String>,
    pattern: Pattern,
    /// Original `col|pattern` (or just `pattern`) substring, for error reporting.
    pub raw: String,
}

/// Failure modes surfaced to the user.
#[derive(Debug)]
pub enum FocusError {
    MissingLeadingSlash,
    EmptyPath,
    BadRegex { seg: String, msg: String },
    NotFound { seg: String },
    Ambiguous { seg: String, preview: Vec<String> },
    UnknownColumn { col: String, available: Vec<String> },
    MultiSegmentUnsupported,
}

/// Parse a `/`-rooted, `/`-separated path with `col|pattern` segments.
pub fn parse_path(raw: &str, case_insensitive: bool) -> Result<Vec<FocusSegment>, FocusError> {
    if raw.is_empty() {
        return Err(FocusError::EmptyPath);
    }
    if !raw.starts_with('/') {
        return Err(FocusError::MissingLeadingSlash);
    }
    let tail = &raw[1..];
    if tail.trim().is_empty() {
        return Err(FocusError::EmptyPath);
    }
    let mut segs = Vec::new();
    for raw_seg in tail.split('/') {
        let raw_seg = raw_seg.trim();
        if raw_seg.is_empty() {
            return Err(FocusError::EmptyPath);
        }
        let (column, pattern_str) = match raw_seg.split_once('|') {
            Some((col, pat)) => {
                let col = col.trim();
                if col.is_empty() {
                    (None, pat)
                } else {
                    (Some(col.to_string()), pat)
                }
            }
            None => (None, raw_seg),
        };
        let pattern =
            Pattern::parse(pattern_str, case_insensitive).map_err(|msg| FocusError::BadRegex {
                seg: raw_seg.to_string(),
                msg,
            })?;
        segs.push(FocusSegment {
            column,
            pattern,
            raw: raw_seg.to_string(),
        });
    }
    Ok(segs)
}

/// Walk a flat list of nodes (no tree). Returns the matched node's `id`.
///
/// Treats every segment as "must collapse to exactly one row"; multi-segment
/// paths are not supported for flat views and yield
/// [`FocusError::MultiSegmentUnsupported`].
pub fn focus_in_flat_items(
    items: &[NodeSummary],
    segments: &[FocusSegment],
) -> Result<String, FocusError> {
    if segments.is_empty() {
        return Err(FocusError::EmptyPath);
    }
    if segments.len() > 1 {
        return Err(FocusError::MultiSegmentUnsupported);
    }
    let seg = &segments[0];

    if let Some(col) = &seg.column {
        if !column_exists(items, col) {
            let mut available: BTreeSet<String> = BTreeSet::new();
            for it in items {
                for f in &it.metadata.fields {
                    available.insert(f.key.clone());
                }
            }
            available.insert("id".to_string());
            available.insert("label".to_string());
            return Err(FocusError::UnknownColumn {
                col: col.clone(),
                available: available.into_iter().collect(),
            });
        }
    }

    let mut matches: Vec<String> = Vec::new();
    for item in items {
        let text = item_text_for_column(item, seg.column.as_deref());
        if seg.pattern.matches(&text) {
            matches.push(item.id.clone());
        }
    }
    match matches.len() {
        0 => Err(FocusError::NotFound {
            seg: seg.raw.clone(),
        }),
        1 => Ok(matches.into_iter().next().unwrap()),
        _ => {
            let preview: Vec<String> = matches.iter().take(5).cloned().collect();
            Err(FocusError::Ambiguous {
                seg: seg.raw.clone(),
                preview,
            })
        }
    }
}

fn column_exists(items: &[NodeSummary], col: &str) -> bool {
    if col == "id" || col == "label" {
        return true;
    }
    items
        .iter()
        .any(|it| it.metadata.fields.iter().any(|f| f.key == col))
}

fn item_text_for_column(item: &NodeSummary, col: Option<&str>) -> String {
    match col {
        Some("id") => item.id.clone(),
        Some("label") => item.label.clone(),
        Some(key) => item
            .metadata
            .fields
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.value.clone())
            .unwrap_or_default(),
        None => {
            let mut s = item.label.clone();
            for f in &item.metadata.fields {
                s.push(' ');
                s.push_str(&f.value);
            }
            s
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_content::{Metadata, MetadataField, NodeSummary, NodeType};

    fn item(id: &str, label: &str, fields: &[(&str, &str)]) -> NodeSummary {
        NodeSummary {
            id: id.to_string(),
            label: label.to_string(),
            node_type: NodeType {
                type_id: "test:item".to_string(),
                mime_type: "text/plain".to_string(),
                syntax: None,
                file_extension: ".txt".to_string(),
                display_name: "Item".to_string(),
            },
            metadata: Metadata {
                fields: fields
                    .iter()
                    .map(|(k, v)| MetadataField {
                        key: k.to_string(),
                        value: v.to_string(),
                        display_label: k.to_string(),
                        editable: false,
                        allowed_values: None,
                    })
                    .collect(),
            },
            has_children: None,
        }
    }

    #[test]
    fn parse_missing_leading_slash() {
        assert!(matches!(
            parse_path("foo", false),
            Err(FocusError::MissingLeadingSlash)
        ));
    }

    #[test]
    fn parse_empty_path() {
        assert!(matches!(parse_path("", false), Err(FocusError::EmptyPath)));
        assert!(matches!(parse_path("/", false), Err(FocusError::EmptyPath)));
        assert!(matches!(
            parse_path("/foo//bar", false),
            Err(FocusError::EmptyPath)
        ));
    }

    #[test]
    fn parse_with_column_hint() {
        let segs = parse_path("/ref|acme#42", false).unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].column.as_deref(), Some("ref"));
        assert_eq!(segs[0].raw, "ref|acme#42");
    }

    #[test]
    fn parse_without_column_hint() {
        let segs = parse_path("/acme#42", false).unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].column, None);
    }

    #[test]
    fn parse_empty_column_falls_back_to_default() {
        // `|foo` (empty column before pipe) = same as no column hint
        let segs = parse_path("/|acme#42", false).unwrap();
        assert_eq!(segs[0].column, None);
    }

    #[test]
    fn parse_bad_regex() {
        let err = parse_path("/ref|re:[unclosed", false).unwrap_err();
        match err {
            FocusError::BadRegex { seg, .. } => assert_eq!(seg, "ref|re:[unclosed"),
            other => panic!("expected BadRegex, got {other:?}"),
        }
    }

    #[test]
    fn flat_finds_unique_by_column() {
        let items = vec![
            item("userstory:1", "First", &[("ref", "acme#41")]),
            item("userstory:2", "Second", &[("ref", "acme#42")]),
            item("userstory:3", "Third", &[("ref", "acme#43")]),
        ];
        let segs = parse_path("/ref|acme#42", false).unwrap();
        let id = focus_in_flat_items(&items, &segs).unwrap();
        assert_eq!(id, "userstory:2");
    }

    #[test]
    fn flat_default_column_matches_label_and_fields() {
        let items = vec![
            item("a", "Alpha", &[("ref", "p#1")]),
            item("b", "Bravo", &[("ref", "p#2")]),
        ];
        let segs = parse_path("/p#2", false).unwrap();
        let id = focus_in_flat_items(&items, &segs).unwrap();
        assert_eq!(id, "b");
    }

    #[test]
    fn flat_no_match() {
        let items = vec![item("a", "Alpha", &[("ref", "p#1")])];
        let segs = parse_path("/ref|p#9", false).unwrap();
        let err = focus_in_flat_items(&items, &segs).unwrap_err();
        assert!(matches!(err, FocusError::NotFound { .. }));
    }

    #[test]
    fn flat_ambiguous_lists_preview() {
        let items = vec![
            item("a", "Alpha", &[("ref", "p#1")]),
            item("b", "Beta", &[("ref", "p#1")]),
        ];
        let segs = parse_path("/ref|p#1", false).unwrap();
        let err = focus_in_flat_items(&items, &segs).unwrap_err();
        match err {
            FocusError::Ambiguous { preview, .. } => {
                assert_eq!(preview.len(), 2);
                assert!(preview.contains(&"a".to_string()));
                assert!(preview.contains(&"b".to_string()));
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn flat_unknown_column_lists_available() {
        let items = vec![item("a", "Alpha", &[("ref", "p#1"), ("status", "open")])];
        let segs = parse_path("/foo|p#1", false).unwrap();
        let err = focus_in_flat_items(&items, &segs).unwrap_err();
        match err {
            FocusError::UnknownColumn { col, available } => {
                assert_eq!(col, "foo");
                assert!(available.contains(&"ref".to_string()));
                assert!(available.contains(&"status".to_string()));
                assert!(available.contains(&"id".to_string()));
                assert!(available.contains(&"label".to_string()));
            }
            other => panic!("expected UnknownColumn, got {other:?}"),
        }
    }

    #[test]
    fn flat_id_and_label_columns() {
        let items = vec![item("userstory:7", "Some Story", &[])];
        let segs = parse_path("/id|userstory:7", false).unwrap();
        assert_eq!(focus_in_flat_items(&items, &segs).unwrap(), "userstory:7");
        let segs = parse_path("/label|Some Story", false).unwrap();
        assert_eq!(focus_in_flat_items(&items, &segs).unwrap(), "userstory:7");
    }

    #[test]
    fn flat_regex_word_boundary() {
        let items = vec![
            item("a", "x", &[("ref", "acme#42")]),
            item("b", "x", &[("ref", "acme#420")]),
        ];
        let segs = parse_path(r"/ref|re:\b42\b", false).unwrap();
        assert_eq!(focus_in_flat_items(&items, &segs).unwrap(), "a");
    }

    #[test]
    fn flat_case_insensitive_flag_applies_to_substring_and_regex() {
        let items = vec![item("a", "x", &[("ref", "ACME#42")])];
        let segs = parse_path("/ref|acme#42", true).unwrap();
        assert_eq!(focus_in_flat_items(&items, &segs).unwrap(), "a");
        let segs = parse_path("/ref|re:acme", true).unwrap();
        assert_eq!(focus_in_flat_items(&items, &segs).unwrap(), "a");
    }

    #[test]
    fn flat_multi_segment_unsupported() {
        let items = vec![item("a", "x", &[])];
        let segs = parse_path("/a/b", false).unwrap();
        assert!(matches!(
            focus_in_flat_items(&items, &segs),
            Err(FocusError::MultiSegmentUnsupported)
        ));
    }
}
