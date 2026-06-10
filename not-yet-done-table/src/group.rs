//! Vertical row grouping with column aggregation — framework-agnostic.
//!
//! Where [`crate::grouping`] merges columns *horizontally* within one row,
//! this module partitions a sequence of items *vertically* into groups and
//! sums one or more aggregate columns per group, plus a grand total.
//!
//! The mechanism is deliberately untyped: the caller supplies each item's
//! group **key** (a display string) and the already-extracted integer
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
    /// this group's per-aggregate-column totals; `label` is the group key.
    Header { label: String, group: usize },
    /// An original data item, by its index into the input `keys` slice.
    /// Items are emitted in grouped order (their original order is preserved
    /// *within* each group).
    Item { index: usize },
    /// The grand-total footer — only present when `footer` is set. Its
    /// per-column totals live in [`GroupPlan::grand_totals`].
    GrandTotal,
}

/// The result of [`group`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupPlan {
    /// Header / item / footer rows in display order.
    pub rows: Vec<PlanRow>,
    /// `group_totals[g][c]` = group `g`'s sum of aggregate column `c`. One
    /// outer entry per group (in first-appearance order), one inner entry per
    /// aggregate column.
    pub group_totals: Vec<Vec<i64>>,
    /// `grand_totals[c]` = sum of aggregate column `c` across every item.
    /// One entry per aggregate column.
    pub grand_totals: Vec<i64>,
}

/// Partition `keys` into groups (preserving first-appearance order) and sum
/// the aggregate columns.
///
/// - `keys[i]` is item `i`'s group identity *and* the group-header label.
///   Items with equal keys form one group; group order follows first
///   appearance, so the caller controls ordering by pre-sorting its items.
/// - `values[c][i]` is item `i`'s value for aggregate column `c` (`None`
///   contributes nothing). Inner slices shorter than `keys` are treated as
///   `None` past their end, so an empty `values` (no aggregates) is valid.
/// - `summary_only`: emit only the per-group [`PlanRow::Header`] rows
///   (collapse each group to a single line); omit individual
///   [`PlanRow::Item`] rows. Totals are unaffected.
/// - `footer`: append a trailing [`PlanRow::GrandTotal`] row.
///
/// An empty `keys` yields an empty plan (and, if `footer`, no footer row —
/// there is nothing to total).
pub fn group(
    keys: &[String],
    values: &[&[Option<i64>]],
    summary_only: bool,
    footer: bool,
) -> GroupPlan {
    // First-appearance order of group labels + their member item indices.
    let mut order: Vec<String> = Vec::new();
    let mut index_of: HashMap<&str, usize> = HashMap::new();
    let mut members: Vec<Vec<usize>> = Vec::new();
    for (i, key) in keys.iter().enumerate() {
        let g = *index_of.entry(key.as_str()).or_insert_with(|| {
            order.push(key.clone());
            members.push(Vec::new());
            order.len() - 1
        });
        members[g].push(i);
    }

    let n_agg = values.len();
    let value_at = |col: usize, item: usize| -> i64 {
        values
            .get(col)
            .and_then(|slice| slice.get(item))
            .and_then(|v| *v)
            .unwrap_or(0)
    };

    // Per-group and grand totals.
    let mut group_totals: Vec<Vec<i64>> = Vec::with_capacity(order.len());
    let mut grand_totals = vec![0i64; n_agg];
    for member_indices in &members {
        let mut totals = vec![0i64; n_agg];
        for &item in member_indices {
            for c in 0..n_agg {
                let v = value_at(c, item);
                totals[c] += v;
                grand_totals[c] += v;
            }
        }
        group_totals.push(totals);
    }

    // Emit rows in display order.
    let mut rows: Vec<PlanRow> = Vec::new();
    for (g, label) in order.iter().enumerate() {
        rows.push(PlanRow::Header { label: label.clone(), group: g });
        if !summary_only {
            for &item in &members[g] {
                rows.push(PlanRow::Item { index: item });
            }
        }
    }
    if footer && !order.is_empty() {
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
                PlanRow::Header { label: "A".into(), group: 0 },
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
                PlanRow::Header { label: "B".into(), group: 0 },
                PlanRow::Item { index: 0 },
                PlanRow::Item { index: 2 },
                PlanRow::Header { label: "A".into(), group: 1 },
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
                PlanRow::Header { label: "A".into(), group: 0 },
                PlanRow::Header { label: "B".into(), group: 1 },
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
                PlanRow::Header { label: "A".into(), group: 0 },
                PlanRow::Item { index: 0 },
                PlanRow::Item { index: 2 },
                PlanRow::Header { label: "B".into(), group: 1 },
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
}
