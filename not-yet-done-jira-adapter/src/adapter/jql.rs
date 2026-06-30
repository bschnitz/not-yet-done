//! JQL helpers shared by the issue list path: column→field mapping for
//! sort, ORDER-BY rewriting on top of the user-supplied JQL, and the
//! `SortableColumn` advertisement returned by [`crate::adapter::JiraRoot`].

use not_yet_done_content::{SortDirection, SortKey, SortKind, SortableColumn};

/// Map a public column key (as exposed by [`sortable_columns`]) onto the
/// JQL field name. Returns `None` for columns we cannot sort on
/// server-side.
pub(super) fn jql_field_for_column(column: &str) -> Option<&'static str> {
    match column {
        "key" => Some("key"),
        "type" => Some("issuetype"),
        "status" => Some("status"),
        "priority" => Some("priority"),
        "assignee" => Some("assignee"),
        "summary" => Some("summary"),
        "updated" => Some("updated"),
        _ => None,
    }
}

/// Columns the issue list can be server-side sorted on.
pub(super) fn issue_sortable_columns() -> Vec<SortableColumn> {
    [
        ("key", "Key"),
        ("type", "Type"),
        ("status", "Status"),
        ("priority", "Priority"),
        ("assignee", "Assignee"),
        ("summary", "Summary"),
        ("updated", "Updated"),
    ]
    .into_iter()
    .map(|(key, label)| SortableColumn {
        key: key.into(),
        label: label.into(),
        // Jira sorts server-side via JQL ORDER BY, so the kind is unused here.
        kind: SortKind::Text,
    })
    .collect()
}

/// Columns the bookmarks list can be sorted on. Same set as the normal
/// issue list, plus the synthetic `bookmarked_at` column. Unlike the normal
/// list (server-side JQL `ORDER BY`), the bookmarks list sorts **locally**
/// via [`not_yet_done_content::apply_sort`], so the [`SortKind`] matters
/// here: `bookmarked_at` is RFC3339 and sorts as a `DateTime`.
pub(super) fn bookmark_sortable_columns() -> Vec<SortableColumn> {
    let mut cols = issue_sortable_columns();
    cols.push(SortableColumn {
        key: "bookmarked_at".into(),
        label: "Bookmarked".into(),
        kind: SortKind::DateTime,
    });
    cols
}

/// Result of folding a `Vec<SortKey>` into a JQL `ORDER BY` clause.
pub(super) struct OrderByClause {
    /// `ORDER BY ...` (no leading space) — empty if no column was honoured.
    pub clause: String,
    /// Subset of the input that was actually mapped onto a JQL field.
    pub applied: Vec<SortKey>,
}

/// Build an `ORDER BY ...` clause from sort keys, dropping columns that
/// don't have a JQL field mapping.
pub(super) fn build_order_by(sort: &[SortKey]) -> OrderByClause {
    let mut parts = Vec::new();
    let mut applied = Vec::new();
    for key in sort {
        if let Some(field) = jql_field_for_column(&key.column) {
            let dir = match key.direction {
                SortDirection::Asc => "ASC",
                SortDirection::Desc => "DESC",
            };
            parts.push(format!("{field} {dir}"));
            applied.push(key.clone());
        }
    }
    let clause = if parts.is_empty() {
        String::new()
    } else {
        format!("ORDER BY {}", parts.join(", "))
    };
    OrderByClause { clause, applied }
}

/// Strip a trailing `ORDER BY ...` from a JQL string, case-insensitively.
/// Used when the caller wants to override the embedded sort.
pub(super) fn strip_order_by(jql: &str) -> String {
    // JQL `ORDER BY` is always at the end of the where-clause. We look for
    // the last whitespace-separated occurrence.
    let lower = jql.to_ascii_lowercase();
    if let Some(idx) = lower.rfind("order by") {
        // Make sure it's preceded by whitespace or start-of-string.
        let preceded_ok = idx == 0
            || jql
                .as_bytes()
                .get(idx - 1)
                .copied()
                .is_some_and(|b| b.is_ascii_whitespace());
        if preceded_ok {
            return jql[..idx].trim_end().to_string();
        }
    }
    jql.to_string()
}

/// Splice `sort` onto a JQL string. If `sort` is empty, the JQL is
/// returned unchanged (preserving any embedded `ORDER BY`). Otherwise
/// any existing `ORDER BY` is stripped and our clause is appended.
pub(super) fn apply_sort(jql: &str, sort: &[SortKey]) -> (String, Vec<SortKey>) {
    if sort.is_empty() {
        return (jql.to_string(), Vec::new());
    }
    let order = build_order_by(sort);
    if order.clause.is_empty() {
        return (jql.to_string(), Vec::new());
    }
    let base = strip_order_by(jql);
    let combined = if base.is_empty() {
        order.clause
    } else {
        format!("{base} {}", order.clause)
    };
    (combined, order.applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: &str, d: SortDirection) -> SortKey {
        SortKey { column: c.into(), direction: d }
    }

    #[test]
    fn bookmark_columns_extend_issue_columns_with_datetime() {
        let issue = issue_sortable_columns();
        let bookmark = bookmark_sortable_columns();
        // Same issue columns, plus exactly one extra (bookmarked_at).
        assert_eq!(bookmark.len(), issue.len() + 1);
        let stamp = bookmark
            .iter()
            .find(|c| c.key == "bookmarked_at")
            .expect("bookmarked_at column present");
        assert_eq!(stamp.kind, SortKind::DateTime);
    }

    #[test]
    fn build_order_by_maps_known_columns() {
        let order = build_order_by(&[
            key("status", SortDirection::Asc),
            key("updated", SortDirection::Desc),
        ]);
        assert_eq!(order.clause, "ORDER BY status ASC, updated DESC");
        assert_eq!(order.applied.len(), 2);
    }

    #[test]
    fn build_order_by_drops_unknown_columns() {
        let order = build_order_by(&[
            key("nonsense", SortDirection::Asc),
            key("priority", SortDirection::Desc),
        ]);
        assert_eq!(order.clause, "ORDER BY priority DESC");
        assert_eq!(order.applied.len(), 1);
        assert_eq!(order.applied[0].column, "priority");
    }

    #[test]
    fn build_order_by_empty_input_yields_empty_clause() {
        let order = build_order_by(&[]);
        assert!(order.clause.is_empty());
        assert!(order.applied.is_empty());
    }

    #[test]
    fn strip_order_by_removes_trailing_clause() {
        assert_eq!(
            strip_order_by("project = FOO ORDER BY updated DESC"),
            "project = FOO"
        );
        assert_eq!(
            strip_order_by("project = FOO order by updated DESC"),
            "project = FOO"
        );
    }

    #[test]
    fn strip_order_by_leaves_jql_without_clause() {
        assert_eq!(strip_order_by("project = FOO"), "project = FOO");
    }

    #[test]
    fn strip_order_by_does_not_mangle_substrings() {
        // Field name happens to start with "order" — must not be treated
        // as ORDER BY because it isn't preceded by whitespace.
        assert_eq!(
            strip_order_by("project = FOO AND foorder by_x = 1"),
            "project = FOO AND foorder by_x = 1"
        );
    }

    #[test]
    fn apply_sort_appends_when_jql_has_no_order_by() {
        let (jql, applied) = apply_sort(
            "project = FOO",
            &[key("status", SortDirection::Asc)],
        );
        assert_eq!(jql, "project = FOO ORDER BY status ASC");
        assert_eq!(applied.len(), 1);
    }

    #[test]
    fn apply_sort_overrides_existing_order_by() {
        let (jql, _) = apply_sort(
            "project = FOO ORDER BY updated DESC",
            &[key("status", SortDirection::Asc)],
        );
        assert_eq!(jql, "project = FOO ORDER BY status ASC");
    }

    #[test]
    fn apply_sort_preserves_existing_when_input_empty() {
        let (jql, applied) = apply_sort(
            "project = FOO ORDER BY updated DESC",
            &[],
        );
        assert_eq!(jql, "project = FOO ORDER BY updated DESC");
        assert!(applied.is_empty());
    }

    #[test]
    fn apply_sort_with_only_unknown_columns_keeps_original() {
        let (jql, applied) = apply_sort(
            "project = FOO ORDER BY updated DESC",
            &[key("nonsense", SortDirection::Asc)],
        );
        assert_eq!(jql, "project = FOO ORDER BY updated DESC");
        assert!(applied.is_empty());
    }

    #[test]
    fn apply_sort_handles_empty_jql_with_sort() {
        let (jql, applied) = apply_sort("", &[key("status", SortDirection::Asc)]);
        assert_eq!(jql, "ORDER BY status ASC");
        assert_eq!(applied.len(), 1);
    }

    #[test]
    fn issue_sortable_columns_includes_key_and_updated() {
        let cols = issue_sortable_columns();
        assert!(cols.iter().any(|c| c.key == "key"));
        assert!(cols.iter().any(|c| c.key == "updated"));
    }
}
