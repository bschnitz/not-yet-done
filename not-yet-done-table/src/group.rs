//! Vertical row grouping with column aggregation — framework-agnostic.
//!
//! Where [`crate::grouping`] merges columns *horizontally* within one row,
//! this module partitions a sequence of items *vertically* into groups and
//! sums one or more aggregate columns per group, plus a grand total.
//!
//! Grouping is **nested**: [`group_nested`] takes, per item, a *vector* of
//! group labels — one per nesting level (outermost first) — and emits a
//! pre-order layout of headers (each tagged with its [`PlanRow::level`]) with
//! a subtotal at every level. The flat single-level [`group`] is a thin
//! wrapper over it, kept for callers (and tests) that only group on one key.
//!
//! The mechanism is deliberately untyped: the caller supplies each item's
//! group **key(s)** (display strings) and the already-extracted integer
//! **values** to sum. All typed parsing — durations, date buckets, which
//! column is the key — stays in the caller (the TUI's `group_aggregate`
//! module). That keeps this crate free of any view-config or chrono
//! dependency while the genuinely reusable part — partition order, per-group
//! and grand totals, the `summary_only` collapse, the footer toggle — lives
//! here with its own tests.

use std::collections::HashMap;

/// One row in a computed group layout, in final display order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanRow {
    /// A group header. `group` indexes into [`GroupPlan::group_totals`] for
    /// this group's per-aggregate-column totals; `label` is the group key;
    /// `level` is the nesting depth (0 = outermost). `representative` is the
    /// item index (into the input `keys` slice) of the group's first member —
    /// useful when `summary_only` collapses the innermost level to one row and
    /// the caller wants to render that row from a representative item.
    Header { label: String, level: usize, group: usize, representative: usize },
    /// An original data item, by its index into the input `keys` slice.
    /// Items are emitted in grouped order (their original order is preserved
    /// *within* each group).
    Item { index: usize },
    /// The grand-total footer — only present when `footer` is set. Its
    /// per-column totals live in [`GroupPlan::grand_totals`].
    GrandTotal,
}

/// The result of [`group`] / [`group_nested`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupPlan {
    /// Header / item / footer rows in display order.
    pub rows: Vec<PlanRow>,
    /// `group_totals[g][c]` = the sum of aggregate column `c` over every item
    /// in the group whose header carries `group == g`. Groups are numbered in
    /// pre-order (the order their headers are emitted), so a nested layout's
    /// inner groups get higher indices than the outer group that contains
    /// them. One inner entry per aggregate column.
    pub group_totals: Vec<Vec<i64>>,
    /// `grand_totals[c]` = sum of aggregate column `c` across every item.
    /// One entry per aggregate column.
    pub grand_totals: Vec<i64>,
}

/// Partition `keys` into a single level of groups and sum the aggregate
/// columns — the flat case of [`group_nested`].
///
/// - `keys[i]` is item `i`'s group identity *and* the group-header label.
/// - `values[c][i]` is item `i`'s value for aggregate column `c`.
/// - `summary_only`: emit only the [`PlanRow::Header`] rows.
/// - `footer`: append a trailing [`PlanRow::GrandTotal`] row.
///
/// See [`group_nested`] for the full contract (ordering, `None` handling,
/// empty input).
pub fn group(
    keys: &[String],
    values: &[&[Option<i64>]],
    summary_only: bool,
    footer: bool,
) -> GroupPlan {
    let nested: Vec<Vec<String>> = keys.iter().map(|k| vec![k.clone()]).collect();
    group_nested(&nested, values, summary_only, footer)
}

/// Partition `keys` into **nested** groups (preserving first-appearance order
/// at every level) and sum the aggregate columns.
///
/// - `keys[i]` is item `i`'s vector of group labels, outermost level first.
///   All items must carry the same number of levels; an empty inner vector
///   means "ungrouped" (a single implicit level). Items with an equal label
///   path coalesce; group order at each level follows first appearance, so the
///   caller controls ordering by pre-sorting its items.
/// - `values[c][i]` is item `i`'s value for aggregate column `c` (`None`
///   contributes nothing). Inner slices shorter than `keys` are treated as
///   `None` past their end, so an empty `values` (no aggregates) is valid.
/// - `summary_only`: omit individual [`PlanRow::Item`] rows (the innermost
///   headers stand in for them). Headers — and therefore totals — at every
///   level are unaffected.
/// - `footer`: append a trailing [`PlanRow::GrandTotal`] row.
///
/// Headers are emitted in pre-order: an outer header, then its inner headers
/// (and, unless `summary_only`, the leaf items) before the next outer header.
/// Each header's `group` field indexes [`GroupPlan::group_totals`]; the total
/// is summed over **all** the items beneath that header (its whole subtree).
///
/// An empty `keys` yields an empty plan (and, if `footer`, no footer row —
/// there is nothing to total).
pub fn group_nested(
    keys: &[Vec<String>],
    values: &[&[Option<i64>]],
    summary_only: bool,
    footer: bool,
) -> GroupPlan {
    let n_agg = values.len();
    let value_at = |col: usize, item: usize| -> i64 {
        values
            .get(col)
            .and_then(|slice| slice.get(item))
            .and_then(|v| *v)
            .unwrap_or(0)
    };

    // How many nesting levels? All items are expected to agree; we take the
    // max so a stray short vector never panics (its missing levels behave as
    // an empty-label group, which the caller's keys never produce in practice).
    let n_levels = keys.iter().map(|k| k.len()).max().unwrap_or(0);

    let mut rows: Vec<PlanRow> = Vec::new();
    let mut group_totals: Vec<Vec<i64>> = Vec::new();
    let mut grand_totals = vec![0i64; n_agg];

    // Recursively partition `items` by their label at `depth`, emitting a
    // header (with its subtree total) per group and recursing or laying out
    // leaf items. `label_at` reads a level's label, tolerating short vectors.
    fn label_at(keys: &[Vec<String>], item: usize, depth: usize) -> &str {
        keys[item].get(depth).map(|s| s.as_str()).unwrap_or("")
    }

    #[allow(clippy::too_many_arguments)]
    fn recurse(
        items: &[usize],
        depth: usize,
        n_levels: usize,
        n_agg: usize,
        keys: &[Vec<String>],
        summary_only: bool,
        value_at: &dyn Fn(usize, usize) -> i64,
        rows: &mut Vec<PlanRow>,
        group_totals: &mut Vec<Vec<i64>>,
    ) {
        if depth >= n_levels {
            // No grouping at all (n_levels == 0): just lay items out flat.
            if !summary_only {
                for &item in items {
                    rows.push(PlanRow::Item { index: item });
                }
            }
            return;
        }

        // First-appearance order of this level's labels + their members.
        let mut order: Vec<&str> = Vec::new();
        let mut index_of: HashMap<&str, usize> = HashMap::new();
        let mut members: Vec<Vec<usize>> = Vec::new();
        for &item in items {
            let key = label_at(keys, item, depth);
            let g = *index_of.entry(key).or_insert_with(|| {
                order.push(key);
                members.push(Vec::new());
                order.len() - 1
            });
            members[g].push(item);
        }

        for (g, label) in order.iter().enumerate() {
            let member_indices = &members[g];
            // Subtree total over every item beneath this header.
            let mut totals = vec![0i64; n_agg];
            for &item in member_indices {
                for c in 0..n_agg {
                    totals[c] += value_at(c, item);
                }
            }
            let group_idx = group_totals.len();
            group_totals.push(totals);
            rows.push(PlanRow::Header {
                label: (*label).to_string(),
                level: depth,
                group: group_idx,
                representative: *member_indices.first().expect("group has ≥1 member"),
            });

            if depth + 1 < n_levels {
                recurse(
                    member_indices,
                    depth + 1,
                    n_levels,
                    n_agg,
                    keys,
                    summary_only,
                    value_at,
                    rows,
                    group_totals,
                );
            } else if !summary_only {
                for &item in member_indices {
                    rows.push(PlanRow::Item { index: item });
                }
            }
        }
    }

    let all: Vec<usize> = (0..keys.len()).collect();
    recurse(
        &all,
        0,
        n_levels,
        n_agg,
        keys,
        summary_only,
        &value_at,
        &mut rows,
        &mut group_totals,
    );

    // Grand totals over every item, independent of nesting.
    for item in 0..keys.len() {
        for c in 0..n_agg {
            grand_totals[c] += value_at(c, item);
        }
    }

    if footer && !keys.is_empty() {
        rows.push(PlanRow::GrandTotal);
    }

    GroupPlan { rows, group_totals, grand_totals }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn nested(v: &[&[&str]]) -> Vec<Vec<String>> {
        v.iter().map(|levels| keys(levels)).collect()
    }

    #[test]
    fn empty_input_yields_empty_plan() {
        let plan = group(&[], &[], false, true);
        assert!(plan.rows.is_empty());
        assert!(plan.group_totals.is_empty());
        assert!(plan.grand_totals.is_empty());
    }

    #[test]
    fn single_group_lists_header_then_items() {
        let k = keys(&["A", "A", "A"]);
        let vals: &[Option<i64>] = &[Some(10), Some(20), Some(30)];
        let plan = group(&k, &[vals], false, false);
        assert_eq!(
            plan.rows,
            vec![
                PlanRow::Header { label: "A".into(), level: 0, group: 0, representative: 0 },
                PlanRow::Item { index: 0 },
                PlanRow::Item { index: 1 },
                PlanRow::Item { index: 2 },
            ]
        );
        assert_eq!(plan.group_totals, vec![vec![60]]);
        assert_eq!(plan.grand_totals, vec![60]);
    }

    #[test]
    fn groups_keep_first_appearance_order() {
        // B appears before A; interleaved members still coalesce per group,
        // and the group order is B, A (first appearance), not sorted.
        let k = keys(&["B", "A", "B", "A"]);
        let vals: &[Option<i64>] = &[Some(1), Some(2), Some(4), Some(8)];
        let plan = group(&k, &[vals], false, false);
        assert_eq!(
            plan.rows,
            vec![
                PlanRow::Header { label: "B".into(), level: 0, group: 0, representative: 0 },
                PlanRow::Item { index: 0 },
                PlanRow::Item { index: 2 },
                PlanRow::Header { label: "A".into(), level: 0, group: 1, representative: 1 },
                PlanRow::Item { index: 1 },
                PlanRow::Item { index: 3 },
            ]
        );
        assert_eq!(plan.group_totals, vec![vec![5], vec![10]]);
        assert_eq!(plan.grand_totals, vec![15]);
    }

    #[test]
    fn summary_only_collapses_to_one_header_per_group() {
        let k = keys(&["A", "A", "B"]);
        let vals: &[Option<i64>] = &[Some(10), Some(20), Some(5)];
        let plan = group(&k, &[vals], true, false);
        assert_eq!(
            plan.rows,
            vec![
                PlanRow::Header { label: "A".into(), level: 0, group: 0, representative: 0 },
                PlanRow::Header { label: "B".into(), level: 0, group: 1, representative: 2 },
            ]
        );
        // Totals are unaffected by collapsing.
        assert_eq!(plan.group_totals, vec![vec![30], vec![5]]);
        assert_eq!(plan.grand_totals, vec![35]);
    }

    #[test]
    fn footer_appends_grand_total_row() {
        let k = keys(&["A", "B"]);
        let vals: &[Option<i64>] = &[Some(3), Some(4)];
        let with = group(&k, &[vals], false, true);
        assert_eq!(with.rows.last(), Some(&PlanRow::GrandTotal));
        let without = group(&k, &[vals], false, false);
        assert!(!without.rows.contains(&PlanRow::GrandTotal));
    }

    #[test]
    fn none_values_contribute_nothing() {
        let k = keys(&["A", "A"]);
        let vals: &[Option<i64>] = &[None, Some(7)];
        let plan = group(&k, &[vals], false, false);
        assert_eq!(plan.group_totals, vec![vec![7]]);
        assert_eq!(plan.grand_totals, vec![7]);
    }

    #[test]
    fn multiple_aggregate_columns_sum_independently() {
        let k = keys(&["A", "A", "B"]);
        let dur: &[Option<i64>] = &[Some(100), Some(200), Some(50)];
        let count: &[Option<i64>] = &[Some(1), Some(1), Some(1)];
        let plan = group(&k, &[dur, count], false, true);
        assert_eq!(plan.group_totals, vec![vec![300, 2], vec![50, 1]]);
        assert_eq!(plan.grand_totals, vec![350, 3]);
    }

    #[test]
    fn no_aggregate_columns_still_partitions() {
        let k = keys(&["A", "B", "A"]);
        let plan = group(&k, &[], false, false);
        assert_eq!(
            plan.rows,
            vec![
                PlanRow::Header { label: "A".into(), level: 0, group: 0, representative: 0 },
                PlanRow::Item { index: 0 },
                PlanRow::Item { index: 2 },
                PlanRow::Header { label: "B".into(), level: 0, group: 1, representative: 1 },
                PlanRow::Item { index: 1 },
            ]
        );
        assert_eq!(plan.group_totals, vec![Vec::<i64>::new(), Vec::<i64>::new()]);
        assert!(plan.grand_totals.is_empty());
    }

    #[test]
    fn shorter_value_slice_treated_as_none_past_end() {
        let k = keys(&["A", "A", "A"]);
        // Only two values for three items → third contributes 0.
        let vals: &[Option<i64>] = &[Some(5), Some(5)];
        let plan = group(&k, &[vals], false, false);
        assert_eq!(plan.grand_totals, vec![10]);
    }

    // ── Nested grouping ──────────────────────────────────────────────────

    #[test]
    fn two_levels_emit_outer_then_inner_headers_in_preorder() {
        // (Day, Task): two days, one with two tasks. Pre-order: Day1 header,
        // its two task headers (each with its items), then Day2 header etc.
        let k = nested(&[
            &["D1", "T-build"],
            &["D1", "T-build"],
            &["D1", "T-docs"],
            &["D2", "T-build"],
        ]);
        let vals: &[Option<i64>] = &[Some(90), Some(45), Some(120), Some(20)];
        let plan = group_nested(&k, &[vals], false, true);
        assert_eq!(
            plan.rows,
            vec![
                PlanRow::Header { label: "D1".into(), level: 0, group: 0, representative: 0 },
                PlanRow::Header { label: "T-build".into(), level: 1, group: 1, representative: 0 },
                PlanRow::Item { index: 0 },
                PlanRow::Item { index: 1 },
                PlanRow::Header { label: "T-docs".into(), level: 1, group: 2, representative: 2 },
                PlanRow::Item { index: 2 },
                PlanRow::Header { label: "D2".into(), level: 0, group: 3, representative: 3 },
                PlanRow::Header { label: "T-build".into(), level: 1, group: 4, representative: 3 },
                PlanRow::Item { index: 3 },
                PlanRow::GrandTotal,
            ]
        );
        // Outer subtree totals roll up the inner ones.
        assert_eq!(plan.group_totals[0], vec![255]); // D1 = 90+45+120
        assert_eq!(plan.group_totals[1], vec![135]); // D1/T-build = 90+45
        assert_eq!(plan.group_totals[2], vec![120]); // D1/T-docs
        assert_eq!(plan.group_totals[3], vec![20]); // D2
        assert_eq!(plan.group_totals[4], vec![20]); // D2/T-build
        assert_eq!(plan.grand_totals, vec![275]);
    }

    #[test]
    fn two_levels_summary_only_drops_items_keeps_all_headers() {
        // Condensed-with-grouping: per (Day, Task) one inner header, no items.
        let k = nested(&[
            &["D1", "T-build"],
            &["D1", "T-build"],
            &["D1", "T-docs"],
        ]);
        let vals: &[Option<i64>] = &[Some(90), Some(45), Some(120)];
        let plan = group_nested(&k, &[vals], true, false);
        assert_eq!(
            plan.rows,
            vec![
                PlanRow::Header { label: "D1".into(), level: 0, group: 0, representative: 0 },
                PlanRow::Header { label: "T-build".into(), level: 1, group: 1, representative: 0 },
                PlanRow::Header { label: "T-docs".into(), level: 1, group: 2, representative: 2 },
            ]
        );
        assert_eq!(plan.group_totals[1], vec![135]);
        assert_eq!(plan.group_totals[2], vec![120]);
    }

    #[test]
    fn representative_is_first_member_in_input_order() {
        // The representative points at the *first* item of each group, so a
        // summary-only caller can render the row from a member.
        let k = nested(&[&["A"], &["B"], &["A"]]);
        let plan = group_nested(&k, &[], true, false);
        match &plan.rows[0] {
            PlanRow::Header { label, representative, .. } => {
                assert_eq!(label, "A");
                assert_eq!(*representative, 0);
            }
            other => panic!("expected header, got {other:?}"),
        }
        match &plan.rows[1] {
            PlanRow::Header { label, representative, .. } => {
                assert_eq!(label, "B");
                assert_eq!(*representative, 1);
            }
            other => panic!("expected header, got {other:?}"),
        }
    }
}
