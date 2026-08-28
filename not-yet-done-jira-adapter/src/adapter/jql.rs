//! JQL helpers shared by the issue list path: column→field mapping for
//! sort, ORDER-BY rewriting on top of the user-supplied JQL, and the
//! column declaration returned by [`crate::adapter::JiraRoot`].

use not_yet_done_content::{ColumnSchema, SortDirection, SortKey};

/// Map a public column key onto the JQL field name. Returns `None` for
/// columns we cannot sort on server-side.
pub(super) fn jql_field_for_column(column: &str) -> Option<&'static str> {
    match column {
        "key" => Some("key"),
        "type" => Some("issuetype"),
        "status" => Some("status"),
        "priority" => Some("priority"),
        "assignee" => Some("assignee"),
        "creator" => Some("creator"),
        // Ordering by `labels` compares the issue's label list as Jira renders
        // it, so an unlabelled issue sorts as the empty value (first ascending).
        "labels" => Some("labels"),
        // JQL names the field in the singular even though an issue can carry
        // several versions. Ordering follows the project's version *sequence*,
        // not the version name — so `1.9` sorts before `1.10` when the project
        // has them in that order.
        "fix_versions" => Some("fixVersion"),
        "summary" => Some("summary"),
        "updated" => Some("updated"),
        // `story_points` is deliberately absent: it is a custom field, so
        // its JQL clause (`cf[<id>]`) is only known once the client has
        // discovered the instance's field id. See [`STORY_POINTS`].
        _ => None,
    }
}

/// Column key of the story-points estimate. Server-sortable like the rest,
/// but only via a clause resolved at runtime — hence its own constant
/// instead of an arm in [`jql_field_for_column`].
pub(super) const STORY_POINTS: &str = "story_points";

/// Whether the server can order by `column`. Everything
/// [`jql_field_for_column`] names, plus [`STORY_POINTS`], whose clause the
/// caller supplies per request.
fn server_sortable(column: &str) -> bool {
    jql_field_for_column(column).is_some() || column == STORY_POINTS
}

/// Every column an issue row carries, with its type. The order matches the
/// metadata fields `issue_summary` builds, so this list and that projection
/// are read side by side.
fn issue_row_columns() -> Vec<ColumnSchema> {
    vec![
        ColumnSchema::new("bookmarked", "Bookmark"),
        ColumnSchema::new("key", "Key"),
        ColumnSchema::new("summary", "Summary"),
        ColumnSchema::new("type", "Type"),
        ColumnSchema::new("status", "Status"),
        ColumnSchema::new("priority", "Priority"),
        ColumnSchema::new("assignee", "Assignee"),
        ColumnSchema::new("creator", "Creator"),
        ColumnSchema::new("labels", "Labels"),
        ColumnSchema::new("fix_versions", "Fix Versions"),
        ColumnSchema::new(STORY_POINTS, "Story Points").typed("number"),
        ColumnSchema::new("updated", "Updated").typed("datetime"),
        ColumnSchema::new("attachments", "Attachm.").typed("number"),
    ]
}

/// The issue list's columns. Every one is carried in the rows; a column is
/// **sortable** exactly when [`jql_field_for_column`] can name it in an
/// `ORDER BY`, because this list is ordered by the server — the rows we get
/// back are one page of a much larger result, so sorting them locally would
/// order the page, not the query.
pub(super) fn issue_columns() -> Vec<ColumnSchema> {
    issue_row_columns()
        .into_iter()
        .map(|c| {
            if server_sortable(&c.key) {
                c
            } else {
                c.unsortable()
            }
        })
        .collect()
}

/// The bookmarks list's columns: the issue columns plus the synthetic
/// `bookmarked_at`. This list is held in full and sorted **locally** via
/// [`not_yet_done_content::apply_sort`], so every column in the rows is
/// sortable — including the two JQL cannot order by — and the declared
/// `value_type` is what the comparison actually uses.
pub(super) fn bookmark_columns() -> Vec<ColumnSchema> {
    let mut cols = issue_row_columns();
    cols.push(ColumnSchema::new("bookmarked_at", "Bookmarked").typed("datetime"));
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
///
/// `story_points_clause` is the runtime-resolved `cf[<id>]` for the
/// story-points column, or `None` when the instance has no such field (or
/// its discovery failed) — in which case a sort on that column is dropped
/// like any other unmappable one, leaving the pane's order untouched
/// rather than failing the search.
pub(super) fn build_order_by(sort: &[SortKey], story_points_clause: Option<&str>) -> OrderByClause {
    let mut parts = Vec::new();
    let mut applied = Vec::new();
    for key in sort {
        let field = match key.column.as_str() {
            STORY_POINTS => story_points_clause,
            other => jql_field_for_column(other),
        };
        if let Some(field) = field {
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

/// Splice `sort` onto a JQL string as an `ORDER BY` clause — this does not
/// sort anything itself, it hands the ordering to the server. If `sort` is
/// empty, the JQL is returned unchanged (preserving any embedded `ORDER BY`).
/// Otherwise any existing `ORDER BY` is stripped and our clause is appended.
pub(super) fn apply_order_by(
    jql: &str,
    sort: &[SortKey],
    story_points_clause: Option<&str>,
) -> (String, Vec<SortKey>) {
    if sort.is_empty() {
        return (jql.to_string(), Vec::new());
    }
    let order = build_order_by(sort, story_points_clause);
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
        SortKey {
            column: c.into(),
            direction: d,
        }
    }

    #[test]
    fn bookmark_columns_extend_issue_columns_with_datetime() {
        let issue = issue_columns();
        let bookmark = bookmark_columns();
        // Same issue columns, plus exactly one extra (bookmarked_at).
        assert_eq!(bookmark.len(), issue.len() + 1);
        let stamp = bookmark
            .iter()
            .find(|c| c.key == "bookmarked_at")
            .expect("bookmarked_at column present");
        assert_eq!(stamp.sort_kind(), not_yet_done_content::SortKind::DateTime);
    }

    #[test]
    fn the_issue_list_only_advertises_what_jql_can_order_by() {
        for col in issue_columns() {
            assert_eq!(
                col.sortable,
                server_sortable(&col.key),
                "column '{}' promises a sort JQL cannot deliver",
                col.key
            );
            assert!(col.in_rows, "column '{}' must be in the rows", col.key);
        }
    }

    #[test]
    fn the_bookmarks_list_sorts_locally_so_every_column_is_sortable() {
        // Held in full and sorted by `apply_sort`, so even the columns JQL
        // has no field for (bookmark marker, attachment count) are honest.
        for col in bookmark_columns() {
            assert!(col.sortable && col.in_rows, "column '{}'", col.key);
        }
    }

    #[test]
    fn build_order_by_maps_known_columns() {
        let order = build_order_by(
            &[
                key("status", SortDirection::Asc),
                key("updated", SortDirection::Desc),
            ],
            None,
        );
        assert_eq!(order.clause, "ORDER BY status ASC, updated DESC");
        assert_eq!(order.applied.len(), 2);
    }

    #[test]
    fn build_order_by_drops_unknown_columns() {
        let order = build_order_by(
            &[
                key("nonsense", SortDirection::Asc),
                key("priority", SortDirection::Desc),
            ],
            None,
        );
        assert_eq!(order.clause, "ORDER BY priority DESC");
        assert_eq!(order.applied.len(), 1);
        assert_eq!(order.applied[0].column, "priority");
    }

    #[test]
    fn build_order_by_empty_input_yields_empty_clause() {
        let order = build_order_by(&[], None);
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
    fn apply_order_by_appends_when_jql_has_no_order_by() {
        let (jql, applied) =
            apply_order_by("project = FOO", &[key("status", SortDirection::Asc)], None);
        assert_eq!(jql, "project = FOO ORDER BY status ASC");
        assert_eq!(applied.len(), 1);
    }

    #[test]
    fn apply_order_by_overrides_existing_order_by() {
        let (jql, _) = apply_order_by(
            "project = FOO ORDER BY updated DESC",
            &[key("status", SortDirection::Asc)],
            None,
        );
        assert_eq!(jql, "project = FOO ORDER BY status ASC");
    }

    #[test]
    fn apply_order_by_preserves_existing_when_input_empty() {
        let (jql, applied) = apply_order_by("project = FOO ORDER BY updated DESC", &[], None);
        assert_eq!(jql, "project = FOO ORDER BY updated DESC");
        assert!(applied.is_empty());
    }

    #[test]
    fn apply_order_by_with_only_unknown_columns_keeps_original() {
        let (jql, applied) = apply_order_by(
            "project = FOO ORDER BY updated DESC",
            &[key("nonsense", SortDirection::Asc)],
            None,
        );
        assert_eq!(jql, "project = FOO ORDER BY updated DESC");
        assert!(applied.is_empty());
    }

    #[test]
    fn apply_order_by_handles_empty_jql_with_sort() {
        let (jql, applied) = apply_order_by("", &[key("status", SortDirection::Asc)], None);
        assert_eq!(jql, "ORDER BY status ASC");
        assert_eq!(applied.len(), 1);
    }

    /// `fix_versions` is the column key (plural, mirroring Jira's `fixVersions`
    /// field), but JQL only knows the singular `fixVersion`. Sorting on the
    /// column must emit the JQL spelling, or the server rejects the query.
    #[test]
    fn fix_versions_column_maps_to_the_singular_jql_field() {
        assert_eq!(jql_field_for_column("fix_versions"), Some("fixVersion"));
        let order = build_order_by(&[key("fix_versions", SortDirection::Desc)], None);
        assert_eq!(order.clause, "ORDER BY fixVersion DESC");
        // Advertised as sortable, so the sort menu offers it at all.
        assert!(issue_columns().iter().any(|c| c.key == "fix_versions"));
    }

    /// Story points is a custom field, so its ORDER BY is the runtime
    /// `cf[<id>]` the client discovered — not a name baked into the
    /// column table.
    #[test]
    fn story_points_orders_by_the_discovered_custom_field_clause() {
        let order = build_order_by(&[key(STORY_POINTS, SortDirection::Desc)], Some("cf[10006]"));
        assert_eq!(order.clause, "ORDER BY cf[10006] DESC");
        assert_eq!(order.applied.len(), 1);
        assert!(
            issue_columns()
                .iter()
                .any(|c| c.key == STORY_POINTS && c.sortable)
        );
        // Local sorting on the bookmarks list has to compare it as a number,
        // or `10` would sort before `9`.
        let col = bookmark_columns()
            .into_iter()
            .find(|c| c.key == STORY_POINTS)
            .expect("bookmarks carry the column too");
        assert_eq!(col.sort_kind(), not_yet_done_content::SortKind::Number);
    }

    /// An instance without the field (or a failed discovery) must leave the
    /// pane's order untouched rather than emit a clause the server rejects.
    #[test]
    fn story_points_sort_is_dropped_when_the_instance_has_no_such_field() {
        let (jql, applied) = apply_order_by(
            "project = FOO ORDER BY updated DESC",
            &[key(STORY_POINTS, SortDirection::Asc)],
            None,
        );
        assert_eq!(jql, "project = FOO ORDER BY updated DESC");
        assert!(applied.is_empty());
    }

    /// The label column is the one Jira field a row carries as a *list*. It
    /// keeps its own name in JQL, so sorting is server-side like every other
    /// column — and the sort menu must offer it.
    #[test]
    fn labels_column_sorts_server_side() {
        assert_eq!(jql_field_for_column("labels"), Some("labels"));
        let order = build_order_by(&[key("labels", SortDirection::Asc)], None);
        assert_eq!(order.clause, "ORDER BY labels ASC");
        assert!(
            issue_columns()
                .iter()
                .any(|c| c.key == "labels" && c.sortable)
        );
        // The bookmarks list sorts locally, so it carries the column too.
        assert!(bookmark_columns().iter().any(|c| c.key == "labels"));
    }

    #[test]
    fn issue_columns_include_key_and_updated() {
        let cols = issue_columns();
        assert!(cols.iter().any(|c| c.key == "key"));
        assert!(cols.iter().any(|c| c.key == "updated"));
    }
}
