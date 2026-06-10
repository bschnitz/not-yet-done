//! Column identification and sizing strategies.

use std::collections::HashMap;
use unicode_width::UnicodeWidthStr;

/// Identifies a column by name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ColumnId(pub String);

impl ColumnId {
    pub fn new(s: impl Into<String>) -> Self {
        ColumnId(s.into())
    }
}

/// Per-column sizing strategy.
#[derive(Debug, Clone)]
pub enum ColStrategy {
    /// Always exactly `n` display columns wide.
    Fixed(usize),
    /// As wide as the longest cell content (including the header).
    Max,
    /// Gets a proportional share of remaining space.
    Flex(usize),
    /// `min(content_max, remaining)` — as wide as the content needs, but
    /// never wider than the space left after all `Fixed`/`Max`/`Auto`
    /// columns are placed. Unlike `Flex` it does not stretch to fill the
    /// leftover, and unlike `Max` it is deferred (claims space only after
    /// every fixed-width column is sized), so a `Fit` column in the middle
    /// never pushes trailing columns off-screen. Mirrors CSS `fit-content`.
    Fit,
    /// `clamp(max(header_width, content_max), min, max)`. Auto columns
    /// take their natural width within bounds and ignore the pane-width
    /// budget — overflow is expected to be handled by horizontal scroll.
    Auto { min: usize, max: usize },
}

/// Determines absolute character widths for a set of columns.
pub trait ColSizer {
    fn col_widths(
        &self,
        cols: &[ColumnId],
        cells: &[&HashMap<ColumnId, String>],
        header: Option<&HashMap<ColumnId, String>>,
        max_width: usize,
        separator: &str,
    ) -> Vec<usize>;
}

/// Simple sizer: fixed width per column, independent of content.
pub struct FixedColSizer {
    pub widths: HashMap<ColumnId, usize>,
}

impl ColSizer for FixedColSizer {
    fn col_widths(
        &self,
        cols: &[ColumnId],
        _cells: &[&HashMap<ColumnId, String>],
        _header: Option<&HashMap<ColumnId, String>>,
        _max_width: usize,
        _separator: &str,
    ) -> Vec<usize> {
        cols.iter()
            .map(|col| self.widths.get(col).copied().unwrap_or(0))
            .collect()
    }
}

/// Flexible sizer with three strategies per column.
pub struct MixedColSizer {
    pub strategies: HashMap<ColumnId, ColStrategy>,
}

impl ColSizer for MixedColSizer {
    fn col_widths(
        &self,
        cols: &[ColumnId],
        cells: &[&HashMap<ColumnId, String>],
        header: Option<&HashMap<ColumnId, String>>,
        max_width: usize,
        separator: &str,
    ) -> Vec<usize> {
        let n = cols.len();
        if n == 0 {
            return vec![];
        }

        let sep_total = separator.width() * n.saturating_sub(1);
        let usable = max_width.saturating_sub(sep_total);

        let header_w = |col: &ColumnId| -> usize {
            header
                .and_then(|h| h.get(col))
                .map(|s| s.width())
                .unwrap_or(0)
        };
        let content_max = |col: &ColumnId| -> usize {
            cells
                .iter()
                .map(|row| row.get(col).map(|s| s.width()).unwrap_or(0))
                .max()
                .unwrap_or(0)
        };

        let mut widths = vec![0usize; n];
        let mut used = 0usize;
        let mut flex_total_weight = 0usize;
        let mut flex_indices: Vec<(usize, usize)> = Vec::new();
        let mut fit_indices: Vec<usize> = Vec::new();

        for (i, col) in cols.iter().enumerate() {
            let strategy = self
                .strategies
                .get(col)
                .cloned()
                .unwrap_or(ColStrategy::Flex(1));

            match strategy {
                ColStrategy::Fixed(w) => {
                    widths[i] = w.min(usable.saturating_sub(used));
                    used += widths[i];
                }
                ColStrategy::Max => {
                    let raw = content_max(col).max(header_w(col));
                    let w = raw.min(usable.saturating_sub(used));
                    widths[i] = w;
                    used += w;
                }
                ColStrategy::Flex(weight) => {
                    flex_indices.push((i, weight));
                    flex_total_weight += weight;
                }
                ColStrategy::Fit => {
                    // Deferred: sized after the positional pass so trailing
                    // fixed-width columns are never starved (see enum docs).
                    fit_indices.push(i);
                }
                ColStrategy::Auto { min, max } => {
                    let natural = header_w(col).max(content_max(col));
                    let w = natural.clamp(min, max);
                    widths[i] = w;
                    // Auto ignores the pane-width budget — H-Scroll
                    // catches overflow. Don't add to `used`.
                }
            }
        }

        let mut remaining = usable.saturating_sub(used);

        // `Fit` columns take min(content, what's left) before flex fills the
        // rest, so they cap at their content width without stretching and
        // without crowding out the proportional flex columns.
        for &i in &fit_indices {
            let col = &cols[i];
            let natural = content_max(col).max(header_w(col));
            let w = natural.min(remaining);
            widths[i] = w;
            remaining -= w;
        }

        if flex_total_weight > 0 {
            let mut distributed = 0usize;
            let flex_count = flex_indices.len();
            for (k, (i, weight)) in flex_indices.iter().enumerate() {
                let w = if k == flex_count - 1 {
                    remaining - distributed
                } else {
                    remaining * weight / flex_total_weight
                };
                widths[*i] = w;
                distributed += w;
            }
        }

        widths
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_sizer() {
        let mut widths = HashMap::new();
        widths.insert(ColumnId::new("a"), 5);
        widths.insert(ColumnId::new("b"), 10);
        let sizer = FixedColSizer { widths };
        let cols = vec![ColumnId::new("a"), ColumnId::new("b")];
        let result = sizer.col_widths(&cols, &[], None, 100, " ");
        assert_eq!(result, vec![5, 10]);
    }

    #[test]
    fn mixed_sizer_max_and_flex() {
        let mut strategies = HashMap::new();
        strategies.insert(ColumnId::new("id"), ColStrategy::Max);
        strategies.insert(ColumnId::new("name"), ColStrategy::Flex(1));
        let sizer = MixedColSizer { strategies };
        let cols = vec![ColumnId::new("id"), ColumnId::new("name")];

        let mut row1 = HashMap::new();
        row1.insert(ColumnId::new("id"), "123".to_string());
        row1.insert(ColumnId::new("name"), "Alice".to_string());
        let mut row2 = HashMap::new();
        row2.insert(ColumnId::new("id"), "4".to_string());
        row2.insert(ColumnId::new("name"), "Bob".to_string());

        let cells: Vec<&HashMap<ColumnId, String>> = vec![&row1, &row2];
        // max_width=20, sep=" " (1 char), id=Max→3, name=Flex→20-1-3=16
        let result = sizer.col_widths(&cols, &cells, None, 20, " ");
        assert_eq!(result, vec![3, 16]);
    }

    #[test]
    fn mixed_sizer_two_char_separator() {
        let mut strategies = HashMap::new();
        strategies.insert(ColumnId::new("a"), ColStrategy::Fixed(5));
        strategies.insert(ColumnId::new("b"), ColStrategy::Flex(1));
        let sizer = MixedColSizer { strategies };
        let cols = vec![ColumnId::new("a"), ColumnId::new("b")];
        // max_width=20, sep="  " (2 chars), a=Fixed(5), b=Flex→20-2-5=13
        let result = sizer.col_widths(&cols, &[], None, 20, "  ");
        assert_eq!(result, vec![5, 13]);
    }

    #[test]
    fn flex_in_middle_keeps_trailing_columns_on_screen() {
        // Regression for the content-table layout: a `flex` column that is
        // *not* last (the Tasks "Task" column sits in the middle, with date
        // columns after it). When the budget equals the real pane width, flex
        // must absorb only the *leftover* — the trailing Max columns keep
        // their natural widths and the whole row fits the budget. (The bug
        // was passing a fixed budget of 300 instead of the pane width, so
        // flex ballooned and the render clipped everything after it.)
        let mut strategies = HashMap::new();
        strategies.insert(ColumnId::new("a"), ColStrategy::Max);
        strategies.insert(ColumnId::new("task"), ColStrategy::Flex(1));
        strategies.insert(ColumnId::new("c"), ColStrategy::Max);
        strategies.insert(ColumnId::new("d"), ColStrategy::Max);
        let sizer = MixedColSizer { strategies };
        let cols = vec![
            ColumnId::new("a"),
            ColumnId::new("task"),
            ColumnId::new("c"),
            ColumnId::new("d"),
        ];
        let mut row = HashMap::new();
        row.insert(ColumnId::new("a"), "AA".to_string()); // 2
        row.insert(ColumnId::new("task"), "ignored-for-flex".to_string());
        row.insert(ColumnId::new("c"), "CCCC".to_string()); // 4
        row.insert(ColumnId::new("d"), "DD".to_string()); // 2
        let cells = vec![&row];
        // max_width=20, sep="  " (2) × 3 gaps = 6 → usable 14.
        // Max: a=2, c=4, d=2 (used 8) → flex "task" = 14-8 = 6.
        let result = sizer.col_widths(&cols, &cells, None, 20, "  ");
        assert_eq!(result, vec![2, 6, 4, 2]);
        // Trailing Max columns are non-zero (on-screen)…
        assert!(result[2] > 0 && result[3] > 0);
        // …and the layout exactly fills the pane budget (no overflow/scroll).
        let sep_total = 2 * (cols.len() - 1);
        assert_eq!(result.iter().sum::<usize>() + sep_total, 20);
    }

    #[test]
    fn fit_caps_at_content_and_yields_space_to_others() {
        // A `fit` column in the middle takes only the width its content needs,
        // never stretching to fill the leftover (unlike flex) and never
        // starving the trailing Max columns (unlike a positional Max). Mirrors
        // CSS fit-content = min(content, available).
        let mut strategies = HashMap::new();
        strategies.insert(ColumnId::new("a"), ColStrategy::Max);
        strategies.insert(ColumnId::new("task"), ColStrategy::Fit);
        strategies.insert(ColumnId::new("c"), ColStrategy::Max);
        strategies.insert(ColumnId::new("d"), ColStrategy::Max);
        let sizer = MixedColSizer { strategies };
        let cols = vec![
            ColumnId::new("a"),
            ColumnId::new("task"),
            ColumnId::new("c"),
            ColumnId::new("d"),
        ];
        let mut row = HashMap::new();
        row.insert(ColumnId::new("a"), "AA".to_string()); // 2
        row.insert(ColumnId::new("task"), "Hello".to_string()); // 5 — fits
        row.insert(ColumnId::new("c"), "CCCC".to_string()); // 4
        row.insert(ColumnId::new("d"), "DD".to_string()); // 2
        let cells = vec![&row];
        // usable 14; Max a=2,c=4,d=2 (used 8) → remaining 6. fit content=5 ≤ 6
        // → task=5. Leftover 1 is simply unused (no flex to absorb it), so the
        // table is *narrower* than the pane — exactly "as wide as needed".
        let result = sizer.col_widths(&cols, &cells, None, 20, "  ");
        assert_eq!(result, vec![2, 5, 4, 2]);

        // When the content is wider than the leftover, fit caps at the leftover
        // (6 here) and the trailing columns still keep their widths.
        let mut wide = HashMap::new();
        wide.insert(ColumnId::new("a"), "AA".to_string());
        wide.insert(ColumnId::new("task"), "a-very-long-task-title".to_string());
        wide.insert(ColumnId::new("c"), "CCCC".to_string());
        wide.insert(ColumnId::new("d"), "DD".to_string());
        let result = sizer.col_widths(&cols, &[&wide], None, 20, "  ");
        assert_eq!(result, vec![2, 6, 4, 2]);
        assert!(result[2] > 0 && result[3] > 0);
    }

    fn auto_sizer(min: usize, max: usize) -> MixedColSizer {
        let mut strategies = HashMap::new();
        strategies.insert(ColumnId::new("c"), ColStrategy::Auto { min, max });
        MixedColSizer { strategies }
    }

    #[test]
    fn auto_header_floor_when_header_wider_than_content() {
        // Header "longheader" (10) > content "x" (1) → 10 (within bounds 5..20).
        let sizer = auto_sizer(5, 20);
        let cols = vec![ColumnId::new("c")];
        let mut row = HashMap::new();
        row.insert(ColumnId::new("c"), "x".to_string());
        let cells = vec![&row];
        let mut header = HashMap::new();
        header.insert(ColumnId::new("c"), "longheader".to_string());
        let result = sizer.col_widths(&cols, &cells, Some(&header), 100, " ");
        assert_eq!(result, vec![10]);
    }

    #[test]
    fn auto_grows_with_content_within_bounds() {
        // Content "abcdefg" (7) > header "h" (1) → 7 (within 5..11).
        let sizer = auto_sizer(5, 11);
        let cols = vec![ColumnId::new("c")];
        let mut row = HashMap::new();
        row.insert(ColumnId::new("c"), "abcdefg".to_string());
        let cells = vec![&row];
        let mut header = HashMap::new();
        header.insert(ColumnId::new("c"), "h".to_string());
        let result = sizer.col_widths(&cols, &cells, Some(&header), 100, " ");
        assert_eq!(result, vec![7]);
    }

    #[test]
    fn auto_min_floor_when_content_short() {
        // Content "ab" (2), header "h" (1) → natural=2, clamped up to min=5.
        let sizer = auto_sizer(5, 11);
        let cols = vec![ColumnId::new("c")];
        let mut row = HashMap::new();
        row.insert(ColumnId::new("c"), "ab".to_string());
        let cells = vec![&row];
        let mut header = HashMap::new();
        header.insert(ColumnId::new("c"), "h".to_string());
        let result = sizer.col_widths(&cols, &cells, Some(&header), 100, " ");
        assert_eq!(result, vec![5]);
    }

    #[test]
    fn auto_max_cap_truncates_header() {
        // Header "verylongheader" (14) > max=11 → 11 (header truncated).
        let sizer = auto_sizer(5, 11);
        let cols = vec![ColumnId::new("c")];
        let mut row = HashMap::new();
        row.insert(ColumnId::new("c"), "x".to_string());
        let cells = vec![&row];
        let mut header = HashMap::new();
        header.insert(ColumnId::new("c"), "verylongheader".to_string());
        let result = sizer.col_widths(&cols, &cells, Some(&header), 100, " ");
        assert_eq!(result, vec![11]);
    }

    #[test]
    fn auto_ignores_pane_budget_no_crash() {
        // Three Auto columns, each natural=11, total > max_width=10 (impossible
        // budget). Auto must still emit 11/11/11 without crashing — H-Scroll
        // catches overflow.
        let mut strategies = HashMap::new();
        strategies.insert(ColumnId::new("a"), ColStrategy::Auto { min: 5, max: 11 });
        strategies.insert(ColumnId::new("b"), ColStrategy::Auto { min: 5, max: 11 });
        strategies.insert(ColumnId::new("c"), ColStrategy::Auto { min: 5, max: 11 });
        let sizer = MixedColSizer { strategies };
        let cols = vec![ColumnId::new("a"), ColumnId::new("b"), ColumnId::new("c")];

        let mut row = HashMap::new();
        row.insert(ColumnId::new("a"), "abcdefghijk".to_string()); // 11
        row.insert(ColumnId::new("b"), "abcdefghijk".to_string());
        row.insert(ColumnId::new("c"), "abcdefghijk".to_string());
        let cells = vec![&row];

        let result = sizer.col_widths(&cols, &cells, None, 10, " ");
        assert_eq!(result, vec![11, 11, 11]);
    }
}
