//! Path-based task lookup, shared by `:focus-task` (TUI) and
//! `task show --path` (CLI). Walks the task hierarchy segment by
//! segment, applying substring or regex matchers per segment.

use regex::Regex;
use uuid::Uuid;

use crate::entity::task;

/// Per-segment matcher. Default form is a substring check; `re:<pat>`
/// opts that segment into a Rust `regex` pattern. Case-folding is
/// decided once for the whole path via the `case_insensitive` flag.
#[derive(Debug)]
pub enum SegmentMatcher {
    Substring { needle: String, case_insensitive: bool },
    Regex(Regex),
}

impl SegmentMatcher {
    pub fn parse(seg: &str, case_insensitive: bool) -> Result<Self, String> {
        if let Some(pat) = seg.strip_prefix("re:") {
            let full = if case_insensitive {
                format!("(?i){pat}")
            } else {
                pat.to_string()
            };
            Regex::new(&full)
                .map(SegmentMatcher::Regex)
                .map_err(|e| format!("invalid regex 're:{pat}' — {e}"))
        } else {
            let needle = if case_insensitive {
                seg.to_ascii_lowercase()
            } else {
                seg.to_string()
            };
            Ok(SegmentMatcher::Substring { needle, case_insensitive })
        }
    }

    pub fn matches(&self, text: &str) -> bool {
        match self {
            SegmentMatcher::Substring { needle, case_insensitive: true } => {
                text.to_ascii_lowercase().contains(needle.as_str())
            }
            SegmentMatcher::Substring { needle, case_insensitive: false } => {
                text.contains(needle.as_str())
            }
            SegmentMatcher::Regex(re) => re.is_match(text),
        }
    }
}

/// Result of walking a `/seg/seg/...` path through the task hierarchy.
///
/// `last_matched` on `NotFound` is the parent that the *previous*
/// segment resolved to (or `None` if the very first segment failed at
/// root level). Create-if-missing flows need that to know where to
/// attach a new leaf.
#[derive(Debug)]
pub enum WalkOutcome {
    Found(Uuid),
    NotFound {
        last_matched: Option<Uuid>,
        depth: usize,
        seg: String,
    },
    Ambiguous {
        depth: usize,
        seg: String,
        candidates: Vec<Uuid>,
    },
    BadRegex {
        depth: usize,
        seg: String,
        msg: String,
    },
    EmptyPath,
    MissingLeadingSlash,
}

/// Walk a `/seg/seg/...` path through `rows`, matching each segment
/// against task `description`. The first segment matches tasks with
/// `parent_id = None`; subsequent segments are filtered to children of
/// the previous match.
pub fn walk_task_path(
    rows: &[task::Model],
    path: &str,
    case_insensitive: bool,
) -> WalkOutcome {
    let Some(stripped) = path.strip_prefix('/') else {
        return WalkOutcome::MissingLeadingSlash;
    };
    let segments: Vec<&str> = stripped
        .split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if segments.is_empty() {
        return WalkOutcome::EmptyPath;
    }

    // Pre-compile every matcher so a regex error in any segment is
    // reported before any tree walking happens.
    let mut matchers = Vec::with_capacity(segments.len());
    for (depth, seg) in segments.iter().enumerate() {
        match SegmentMatcher::parse(seg, case_insensitive) {
            Ok(m) => matchers.push(m),
            Err(msg) => {
                return WalkOutcome::BadRegex {
                    depth,
                    seg: (*seg).to_string(),
                    msg,
                };
            }
        }
    }

    let mut parent_filter: Option<Uuid> = None;
    let mut last_matched: Option<Uuid> = None;
    for (depth, (seg, matcher)) in segments.iter().zip(matchers.iter()).enumerate() {
        let matches: Vec<&task::Model> = rows
            .iter()
            .filter(|t| t.parent_id == parent_filter)
            .filter(|t| matcher.matches(&t.description))
            .collect();
        match matches.as_slice() {
            [] => {
                return WalkOutcome::NotFound {
                    last_matched,
                    depth,
                    seg: (*seg).to_string(),
                };
            }
            [only] => {
                last_matched = Some(only.id);
                parent_filter = Some(only.id);
            }
            multiple => {
                return WalkOutcome::Ambiguous {
                    depth,
                    seg: (*seg).to_string(),
                    candidates: multiple.iter().map(|t| t.id).collect(),
                };
            }
        }
    }
    WalkOutcome::Found(last_matched.expect("at least one segment matched"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::task::TaskStatus;
    use chrono::Utc;

    fn t(desc: &str, parent: Option<Uuid>) -> task::Model {
        task::Model {
            id: Uuid::new_v4(),
            description: desc.to_string(),
            status: TaskStatus::Todo,
            deleted: false,
            deleted_at: None,
            priority: 0,
            parent_id: parent,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_tracked_at: None,
            path: None,
        }
    }

    #[test]
    fn missing_leading_slash() {
        let rows: Vec<task::Model> = vec![];
        assert!(matches!(walk_task_path(&rows, "foo/bar", false), WalkOutcome::MissingLeadingSlash));
    }

    #[test]
    fn empty_path() {
        let rows: Vec<task::Model> = vec![];
        assert!(matches!(walk_task_path(&rows, "/", false), WalkOutcome::EmptyPath));
        assert!(matches!(walk_task_path(&rows, "///", false), WalkOutcome::EmptyPath));
    }

    #[test]
    fn finds_nested() {
        let root = t("Work", None);
        let child = t("Clients", Some(root.id));
        let leaf = t("Tickets", Some(child.id));
        let rows = vec![root.clone(), child.clone(), leaf.clone()];
        match walk_task_path(&rows, "/Work/Clients/Tickets", false) {
            WalkOutcome::Found(id) => assert_eq!(id, leaf.id),
            other => panic!("expected Found, got {other:?}"),
        }
    }

    #[test]
    fn case_sensitive_default() {
        let root = t("Work", None);
        let rows = vec![root.clone()];
        assert!(matches!(walk_task_path(&rows, "/work", false), WalkOutcome::NotFound { .. }));
        assert!(matches!(walk_task_path(&rows, "/work", true), WalkOutcome::Found(_)));
    }

    #[test]
    fn regex_word_boundary() {
        let root = t("Work", None);
        let t42 = t("#42 Foo", Some(root.id));
        let t420 = t("#420 Bar", Some(root.id));
        let rows = vec![root.clone(), t42.clone(), t420.clone()];
        match walk_task_path(&rows, r"/Work/re:\b42\b", false) {
            WalkOutcome::Found(id) => assert_eq!(id, t42.id),
            other => panic!("expected Found, got {other:?}"),
        }
    }

    #[test]
    fn not_found_returns_last_matched() {
        let root = t("Work", None);
        let child = t("Clients", Some(root.id));
        let rows = vec![root.clone(), child.clone()];
        match walk_task_path(&rows, "/Work/Clients/Tickets", false) {
            WalkOutcome::NotFound { last_matched, depth, seg } => {
                assert_eq!(last_matched, Some(child.id));
                assert_eq!(depth, 2);
                assert_eq!(seg, "Tickets");
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn not_found_at_root_has_none_last_matched() {
        let rows: Vec<task::Model> = vec![];
        match walk_task_path(&rows, "/Nope", false) {
            WalkOutcome::NotFound { last_matched, depth, .. } => {
                assert_eq!(last_matched, None);
                assert_eq!(depth, 0);
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn ambiguous() {
        let a = t("Foo Alpha", None);
        let b = t("Foo Beta", None);
        let rows = vec![a.clone(), b.clone()];
        match walk_task_path(&rows, "/Foo", false) {
            WalkOutcome::Ambiguous { candidates, depth, seg } => {
                assert_eq!(depth, 0);
                assert_eq!(seg, "Foo");
                assert_eq!(candidates.len(), 2);
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn bad_regex() {
        let rows: Vec<task::Model> = vec![];
        match walk_task_path(&rows, "/re:[unclosed", false) {
            WalkOutcome::BadRegex { depth, seg, msg } => {
                assert_eq!(depth, 0);
                assert_eq!(seg, "re:[unclosed");
                assert!(msg.contains("invalid regex"));
            }
            other => panic!("expected BadRegex, got {other:?}"),
        }
    }

    #[test]
    fn ci_flag_propagates_to_regex() {
        let root = t("FOO", None);
        let rows = vec![root.clone()];
        // Without -i, regex /foo/ wouldn't match FOO.
        assert!(matches!(walk_task_path(&rows, "/re:foo", false), WalkOutcome::NotFound { .. }));
        // With -i, it should match.
        match walk_task_path(&rows, "/re:foo", true) {
            WalkOutcome::Found(_) => {}
            other => panic!("expected Found with -i, got {other:?}"),
        }
    }
}
