//! Table widget: scrollable, selectable row table with per-column styling,
//! tree connector support, cell grouping, and fuzzy match highlights.

mod component;
mod render;
mod smooth;
pub mod keymap;
pub mod state;
pub mod style;

pub use component::{Table, JumpPhase};
// Legacy compat — remove set_data, consumers use set_rows + set_fixed_headers/footers.
pub use keymap::TableKeymap;
pub use state::TableEvent;
pub use style::{TableStyle, TableStyleType};

// Multi-line row support is re-exported via the crate root.

use std::ops::Range;
use ratatui::style::Style;

/// A single cell in a table row.
#[derive(Debug, Clone)]
pub struct TableWidgetCell {
    /// Fitted text for this cell.
    pub text: String,
    /// Char-index ranges to highlight (e.g. fuzzy match).
    pub highlights: Vec<Range<usize>>,
    /// Number of leading chars rendered with the "prefix" style
    /// (e.g. tree connectors). 0 for normal cells.
    pub prefix_len: usize,
    /// Number of columns this cell spans. 1 = normal, >1 = grouped.
    /// When >1, the next `col_span - 1` cells in the row are skipped.
    pub col_span: usize,
    /// Optional style override. When set, this style is used instead of
    /// the column style. Resolved from the Table's `style_map`.
    pub style_id: Option<usize>,
    /// Optional inline segments. When non-empty, the cell is rendered as a
    /// sequence of styled spans — each segment carries its own style id
    /// (resolved via `StyleMap`). `None` means "use cell's default style".
    /// `text` should equal the concatenation of all segments. Mutually
    /// exclusive with `prefix_len > 0` and non-empty `highlights`.
    pub segments: Vec<(String, Option<usize>)>,
}

impl TableWidgetCell {
    pub fn plain(text: impl Into<String>) -> Self {
        Self { text: text.into(), highlights: vec![], prefix_len: 0, col_span: 1, style_id: None, segments: vec![] }
    }

    pub fn with_highlights(text: impl Into<String>, highlights: Vec<Range<usize>>) -> Self {
        Self { text: text.into(), highlights, prefix_len: 0, col_span: 1, style_id: None, segments: vec![] }
    }

    pub fn with_prefix(text: impl Into<String>, prefix_len: usize) -> Self {
        Self { text: text.into(), highlights: vec![], prefix_len, col_span: 1, style_id: None, segments: vec![] }
    }

    pub fn tree(text: impl Into<String>, connector_chars: usize, highlights: Vec<Range<usize>>) -> Self {
        Self { text: text.into(), highlights, prefix_len: connector_chars, col_span: 1, style_id: None, segments: vec![] }
    }

    /// Create a cell that spans multiple columns.
    pub fn grouped(text: impl Into<String>, col_span: usize) -> Self {
        Self { text: text.into(), highlights: vec![], prefix_len: 0, col_span: col_span.max(1), style_id: None, segments: vec![] }
    }

    /// Create a cell rendered as a sequence of inline-styled segments.
    /// `text` is auto-derived from the segments for layout/jump purposes.
    pub fn from_segments(segments: Vec<(String, Option<usize>)>) -> Self {
        let text: String = segments.iter().map(|(s, _)| s.as_str()).collect();
        Self { text, highlights: vec![], prefix_len: 0, col_span: 1, style_id: None, segments }
    }

    /// Set a style override for this cell.
    pub fn with_style(mut self, style_id: usize) -> Self {
        self.style_id = Some(style_id);
        self
    }
}

/// One physical line of a (possibly multi-line) table row.
///
/// A classic single-line row has exactly one of these. Multi-line rows
/// (e.g. a chat layout: meta line + body line + spacer) stack several.
#[derive(Debug, Clone)]
pub struct TableWidgetLine {
    pub cells: Vec<TableWidgetCell>,
    /// Whether this line is painted with the selection style when its row
    /// is selected. `false` keeps a line (e.g. a spacer) visually outside
    /// the selection block.
    pub highlight_on_select: bool,
}

impl TableWidgetLine {
    pub fn new(cells: Vec<TableWidgetCell>) -> Self {
        Self { cells, highlight_on_select: true }
    }

    pub fn with_highlight_on_select(mut self, v: bool) -> Self {
        self.highlight_on_select = v;
        self
    }
}

/// A single row in the table, rendered as a stack of one or more physical
/// [`TableWidgetLine`]s. A row with one line is the classic single-line case;
/// `height() == 1` makes the widget behave exactly as before.
#[derive(Debug, Clone)]
pub struct TableWidgetRow {
    pub lines: Vec<TableWidgetLine>,
    /// Whether this row can be selected by the user.
    pub selectable: bool,
}

impl TableWidgetRow {
    /// A single-line row from a flat list of cells (the common case).
    pub fn new(cells: Vec<TableWidgetCell>) -> Self {
        Self { lines: vec![TableWidgetLine::new(cells)], selectable: true }
    }

    /// A multi-line row from explicit physical lines.
    pub fn multiline(lines: Vec<TableWidgetLine>) -> Self {
        Self { lines, selectable: true }
    }

    pub fn not_selectable(mut self) -> Self {
        self.selectable = false;
        self
    }

    /// Cells of the first physical line — the single-line view used by
    /// jump-mode, the column cursor, and width computation (all of which
    /// are single-line-only features).
    pub fn primary_line(&self) -> &[TableWidgetCell] {
        self.lines.first().map(|l| l.cells.as_slice()).unwrap_or(&[])
    }

    /// Number of physical lines this row occupies (always ≥ 1).
    pub fn height(&self) -> usize {
        self.lines.len().max(1)
    }
}

/// Per-column style configuration.
#[derive(Debug, Clone, Default)]
pub struct ColumnStyles(pub Vec<Style>);

impl ColumnStyles {
    pub fn new(styles: Vec<Style>) -> Self {
        Self(styles)
    }

    pub fn get(&self, idx: usize) -> Style {
        self.0.get(idx).copied().unwrap_or_default()
    }
}

/// Map of style IDs to ratatui Styles.
///
/// Used by cells with `style_id` set to override the column default.
#[derive(Debug, Clone, Default)]
pub struct StyleMap(pub Vec<Style>);

impl StyleMap {
    pub fn new(styles: Vec<Style>) -> Self {
        Self(styles)
    }

    pub fn get(&self, id: usize) -> Option<Style> {
        self.0.get(id).copied()
    }
}
