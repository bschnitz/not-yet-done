//! Table layout computation — column widths, cell fitting, alignment.

use std::collections::HashMap;
use std::hash::Hash;
use std::ops::Range;

use crate::cell::{CellAlignment, fit_aligned, fit_aligned_with_highlights};
use crate::column::{ColSizer, ColumnId};
use crate::row::{ComputedLine, ComputedMultiRow, ComputedRow, Row};

/// Configuration for table layout.
pub struct TableConfig {
    pub max_width: usize,
    pub separator: String,
    pub sizer: Box<dyn ColSizer>,
}

/// The result of [`compute_table`].
pub struct ComputedTable<Id: Eq + Hash + Clone> {
    /// Optional header row.
    pub header: Option<ComputedRow<Id>>,
    /// Data rows in display order.
    pub rows: Vec<ComputedRow<Id>>,
    /// Column widths (absolute, in display columns).
    pub col_widths: Vec<usize>,
}

/// Compute a fully laid-out table from rows and column definitions.
///
/// Each cell is fitted to its computed column width with the appropriate
/// alignment. Highlight ranges (from `StyledSpan`s) are projected onto
/// the fitted strings.
pub fn compute_table<Id>(
    rows: &[Row<Id>],
    config: &TableConfig,
    cols: &[ColumnId],
    header: Option<&Row<Id>>,
) -> ComputedTable<Id>
where
    Id: Eq + Hash + Clone,
{
    // Build plain-string cell maps for column-width sizing.
    let sizing_cells: Vec<HashMap<ColumnId, String>> = rows
        .iter()
        .map(|row| {
            row.cells.iter()
                .map(|(k, v)| (k.clone(), v.text.clone()))
                .collect()
        })
        .collect();
    let sizing_refs: Vec<&HashMap<ColumnId, String>> = sizing_cells.iter().collect();
    let header_map: Option<HashMap<ColumnId, String>> = header.map(|h| {
        h.cells
            .iter()
            .map(|(k, v)| (k.clone(), v.text.clone()))
            .collect()
    });

    let col_widths = config.sizer.col_widths(
        cols,
        &sizing_refs,
        header_map.as_ref(),
        config.max_width,
        &config.separator,
    );

    let fit_row = |row: &Row<Id>| -> ComputedRow<Id> {
        let (cells, highlights) = fit_cells(row, cols, &col_widths);
        ComputedRow {
            id: row.id.clone(),
            cells,
            selectable: row.selectable,
            highlights,
        }
    };

    let rendered_header = header.map(fit_row);
    let rendered_rows: Vec<ComputedRow<Id>> = rows.iter().map(fit_row).collect();

    ComputedTable {
        header: rendered_header,
        rows: rendered_rows,
        col_widths,
    }
}

/// Fit one row's `cols` to the given `col_widths`, returning the fitted
/// strings and their projected highlight ranges (parallel vecs). Shared by
/// the single-line ([`compute_table`]) and multi-line
/// ([`compute_multiline_table`]) paths.
fn fit_cells<Id>(
    row: &Row<Id>,
    cols: &[ColumnId],
    col_widths: &[usize],
) -> (Vec<String>, Vec<Vec<Range<usize>>>)
where
    Id: Eq + Hash,
{
    let mut cells = Vec::with_capacity(cols.len());
    let mut highlights = Vec::with_capacity(cols.len());

    for (col_id, &width) in cols.iter().zip(col_widths.iter()) {
        let content = row.cells.get(col_id);
        let (text, alignment, hl_ranges) = match content {
            Some(c) => {
                let ranges: Vec<Range<usize>> = c.spans.iter().map(|s| s.range.clone()).collect();
                (c.text.as_str(), c.alignment, ranges)
            }
            None => ("", CellAlignment::Left, vec![]),
        };

        if hl_ranges.is_empty() {
            cells.push(fit_aligned(text, width, alignment));
            highlights.push(vec![]);
        } else {
            let (fitted, projected) =
                fit_aligned_with_highlights(text, width, alignment, &hl_ranges);
            cells.push(fitted);
            highlights.push(projected);
        }
    }

    (cells, highlights)
}

// ---------------------------------------------------------------------------
// Multi-line rows
// ---------------------------------------------------------------------------

/// One physical line of a [`RowTemplate`]: the ordered columns rendered on
/// that line, plus whether the line participates in the selection highlight.
#[derive(Debug, Clone)]
pub struct LineTemplate {
    pub columns: Vec<ColumnId>,
    /// `true` → painted with the selection style when the row is selected.
    pub highlight_on_select: bool,
}

impl LineTemplate {
    /// A line carrying `columns`. Non-empty lines highlight on select by
    /// default; an empty line (spacer) does not.
    pub fn new(columns: Vec<ColumnId>) -> Self {
        let highlight_on_select = !columns.is_empty();
        Self {
            columns,
            highlight_on_select,
        }
    }

    pub fn with_highlight_on_select(mut self, v: bool) -> Self {
        self.highlight_on_select = v;
        self
    }
}

/// Bauplan, wie die Spalten einer logischen Row auf physische Zeilen verteilt
/// werden. Ein einzeiliges Template entspricht der klassischen Tabelle.
#[derive(Debug, Clone)]
pub struct RowTemplate {
    pub lines: Vec<LineTemplate>,
}

impl RowTemplate {
    /// The degenerate single-line template: all columns on one line.
    pub fn single_line(columns: Vec<ColumnId>) -> Self {
        Self {
            lines: vec![LineTemplate {
                columns,
                highlight_on_select: true,
            }],
        }
    }
}

/// The result of [`compute_multiline_table`].
pub struct ComputedMultiTable<Id: Eq + Hash + Clone> {
    pub header: Option<ComputedMultiRow<Id>>,
    pub rows: Vec<ComputedMultiRow<Id>>,
    /// Column widths per template line (`line_col_widths[line][col]`).
    pub line_col_widths: Vec<Vec<usize>>,
}

/// Lay out `rows` as multi-line rows per `template`.
///
/// Each template line is sized independently: the existing [`ColSizer`] runs
/// over *that line's* columns against the full pane width, so columns at the
/// same line index stay vertically aligned across rows (line 0's `author`
/// column is one width for every message, line 1's `content` gets the full
/// width, etc.). Reuses the same cell-fitting as [`compute_table`]; a
/// single-line template reproduces the classic table exactly.
pub fn compute_multiline_table<Id>(
    rows: &[Row<Id>],
    config: &TableConfig,
    template: &RowTemplate,
    header: Option<&Row<Id>>,
) -> ComputedMultiTable<Id>
where
    Id: Eq + Hash + Clone,
{
    let sizing_cells: Vec<HashMap<ColumnId, String>> = rows
        .iter()
        .map(|row| {
            row.cells
                .iter()
                .map(|(k, v)| (k.clone(), v.text.clone()))
                .collect()
        })
        .collect();
    let sizing_refs: Vec<&HashMap<ColumnId, String>> = sizing_cells.iter().collect();
    let header_map: Option<HashMap<ColumnId, String>> = header.map(|h| {
        h.cells
            .iter()
            .map(|(k, v)| (k.clone(), v.text.clone()))
            .collect()
    });

    // One width vector per template line, sized over that line's columns.
    let line_col_widths: Vec<Vec<usize>> = template
        .lines
        .iter()
        .map(|line| {
            config.sizer.col_widths(
                &line.columns,
                &sizing_refs,
                header_map.as_ref(),
                config.max_width,
                &config.separator,
            )
        })
        .collect();

    let fit_multirow = |row: &Row<Id>| -> ComputedMultiRow<Id> {
        let lines = template
            .lines
            .iter()
            .zip(line_col_widths.iter())
            .map(|(line, widths)| {
                let (cells, highlights) = fit_cells(row, &line.columns, widths);
                ComputedLine {
                    cells,
                    highlights,
                    highlight_on_select: line.highlight_on_select,
                }
            })
            .collect();
        ComputedMultiRow {
            id: row.id.clone(),
            lines,
            selectable: row.selectable,
        }
    };

    let rendered_header = header.map(fit_multirow);
    let rendered_rows: Vec<ComputedMultiRow<Id>> = rows.iter().map(fit_multirow).collect();

    ComputedMultiTable {
        header: rendered_header,
        rows: rendered_rows,
        line_col_widths,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::column::{ColStrategy, MixedColSizer};

    fn make_config(max_width: usize, strategies: Vec<(&str, ColStrategy)>) -> TableConfig {
        let mut m = HashMap::new();
        for (name, strat) in strategies {
            m.insert(ColumnId::new(name), strat);
        }
        TableConfig {
            max_width,
            separator: "  ".to_string(),
            sizer: Box::new(MixedColSizer { strategies: m }),
        }
    }

    #[test]
    fn basic_two_columns() {
        let cols = vec![ColumnId::new("name"), ColumnId::new("val")];
        let config = make_config(20, vec![
            ("name", ColStrategy::Flex(1)),
            ("val", ColStrategy::Max),
        ]);

        let rows = vec![
            Row::new(1u32).cell("name", "Alice").cell("val", "42"),
            Row::new(2u32).cell("name", "Bob").cell("val", "7"),
        ];

        let table = compute_table(&rows, &config, &cols, None);

        // val=Max→2, name=Flex→20-2-2=16
        assert_eq!(table.col_widths, vec![16, 2]);
        assert_eq!(table.rows[0].cells[0], "Alice           ");
        assert_eq!(table.rows[0].cells[1], "42");
        assert_eq!(table.rows[1].cells[0], "Bob             ");
        assert_eq!(table.rows[1].cells[1], "7 ");
    }

    #[test]
    fn right_aligned_column() {
        let cols = vec![ColumnId::new("name"), ColumnId::new("num")];
        let config = make_config(20, vec![
            ("name", ColStrategy::Fixed(10)),
            ("num", ColStrategy::Fixed(6)),
        ]);

        let rows = vec![
            Row::new(1u32)
                .cell("name", "Alice")
                .cell("num", crate::cell::CellContent::aligned("42", crate::cell::CellAlignment::Right)),
        ];

        let table = compute_table(&rows, &config, &cols, None);
        assert_eq!(table.rows[0].cells[1], "    42");
    }

    #[test]
    fn non_selectable_row() {
        let cols = vec![ColumnId::new("a")];
        let config = make_config(20, vec![("a", ColStrategy::Flex(1))]);

        let rows = vec![
            Row::new(1u32).cell("a", "header").not_selectable(),
            Row::new(2u32).cell("a", "data"),
        ];

        let table = compute_table(&rows, &config, &cols, None);
        assert!(!table.rows[0].selectable);
        assert!(table.rows[1].selectable);
    }

    #[test]
    fn header_included_in_sizing() {
        let cols = vec![ColumnId::new("x")];
        let config = make_config(40, vec![("x", ColStrategy::Max)]);

        let rows = vec![
            Row::new(1u32).cell("x", "ab"),
        ];
        let header = Row::new(0u32).cell("x", "LongHeader");

        let table = compute_table(&rows, &config, &cols, Some(&header));
        // Max should be 10 (from header), not 2 (from data).
        assert_eq!(table.col_widths, vec![10]);
        assert_eq!(table.header.unwrap().cells[0], "LongHeader");
        assert_eq!(table.rows[0].cells[0], "ab        ");
    }

    #[test]
    fn auto_strategy_applies_header_floor() {
        // Header "LongHeader" (10) wider than content "ab" (2) and within
        // bounds 5..20 → 10. Cells fit to header width.
        let cols = vec![ColumnId::new("x")];
        let config = make_config(40, vec![("x", ColStrategy::Auto { min: 5, max: 20 })]);

        let rows = vec![Row::new(1u32).cell("x", "ab")];
        let header = Row::new(0u32).cell("x", "LongHeader");

        let table = compute_table(&rows, &config, &cols, Some(&header));
        assert_eq!(table.col_widths, vec![10]);
        assert_eq!(table.header.unwrap().cells[0], "LongHeader");
        assert_eq!(table.rows[0].cells[0], "ab        ");
    }

    #[test]
    fn multiline_single_line_template_matches_compute_table() {
        // A single-line template must reproduce compute_table's layout.
        let cols = vec![ColumnId::new("name"), ColumnId::new("val")];
        let config = make_config(20, vec![
            ("name", ColStrategy::Flex(1)),
            ("val", ColStrategy::Max),
        ]);
        let rows = vec![Row::new(1u32).cell("name", "Alice").cell("val", "42")];

        let template = RowTemplate::single_line(cols.clone());
        let multi = compute_multiline_table(&rows, &config, &template, None);

        assert_eq!(multi.rows[0].lines.len(), 1);
        assert_eq!(multi.rows[0].lines[0].cells[0], "Alice           ");
        assert_eq!(multi.rows[0].lines[0].cells[1], "42");
        assert!(multi.rows[0].lines[0].highlight_on_select);
    }

    #[test]
    fn multiline_chat_layout_three_lines() {
        // Chat layout: line0 = [author, time], line1 = [content], line2 = spacer.
        let config = make_config(40, vec![
            ("author", ColStrategy::Max),
            ("time", ColStrategy::Max),
            ("content", ColStrategy::Flex(1)),
        ]);
        let rows = vec![
            Row::new(1u32)
                .cell("author", "alice")
                .cell("time", "2016-07-30 22:36")
                .cell("content", "hello world"),
        ];
        let template = RowTemplate {
            lines: vec![
                LineTemplate::new(vec![ColumnId::new("author"), ColumnId::new("time")]),
                LineTemplate::new(vec![ColumnId::new("content")]),
                LineTemplate::new(vec![]), // spacer
            ],
        };

        let multi = compute_multiline_table(&rows, &config, &template, None);
        let r = &multi.rows[0];
        assert_eq!(r.lines.len(), 3);
        // Line 0: author + time, both present and highlighted.
        assert_eq!(r.lines[0].cells.len(), 2);
        assert!(r.lines[0].cells[0].starts_with("alice"));
        assert!(r.lines[0].cells[1].contains("2016-07-30"));
        assert!(r.lines[0].highlight_on_select);
        // Line 1: content spans (flex) — present.
        assert_eq!(r.lines[1].cells.len(), 1);
        assert!(r.lines[1].cells[0].starts_with("hello world"));
        assert!(r.lines[1].highlight_on_select);
        // Line 2: spacer — no cells, not highlighted.
        assert!(r.lines[2].cells.is_empty());
        assert!(!r.lines[2].highlight_on_select);
    }

    #[test]
    fn truncation_with_separator() {
        let cols = vec![ColumnId::new("a"), ColumnId::new("b")];
        let config = make_config(12, vec![
            ("a", ColStrategy::Fixed(5)),
            ("b", ColStrategy::Flex(1)),
        ]);

        let rows = vec![
            Row::new(1u32).cell("a", "hello").cell("b", "world of rust programming"),
        ];

        let table = compute_table(&rows, &config, &cols, None);
        // a=5, sep=2, b=12-2-5=5
        assert_eq!(table.col_widths, vec![5, 5]);
        assert_eq!(table.rows[0].cells[0], "hello");
        assert_eq!(table.rows[0].cells[1], "worl…");
    }
}
