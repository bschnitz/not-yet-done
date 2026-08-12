//! Row types for input and output.

use std::collections::HashMap;
use std::hash::Hash;
use std::ops::Range;

use crate::cell::CellContent;
use crate::column::ColumnId;

/// A raw data row with a typed ID and per-column cell content.
#[derive(Debug, Clone)]
pub struct Row<Id: Eq + Hash> {
    pub id: Id,
    pub cells: HashMap<ColumnId, CellContent>,
    /// Whether this row can be selected/highlighted by the user.
    pub selectable: bool,
}

impl<Id: Eq + Hash> Row<Id> {
    pub fn new(id: Id) -> Self {
        Self {
            id,
            cells: HashMap::new(),
            selectable: true,
        }
    }

    pub fn cell(mut self, col: impl Into<String>, content: impl Into<CellContent>) -> Self {
        self.cells.insert(ColumnId::new(col), content.into());
        self
    }

    pub fn not_selectable(mut self) -> Self {
        self.selectable = false;
        self
    }
}

/// A fully laid-out row ready for rendering.
///
/// `cells` contains one fitted string per column, in the same order as the
/// `cols` slice passed to [`compute_table`](crate::layout::compute_table).
#[derive(Debug, Clone)]
pub struct ComputedRow<Id: Eq + Hash + Clone> {
    pub id: Id,
    /// One fitted string per column, in column order.
    pub cells: Vec<String>,
    /// Whether this row can be selected by the user.
    pub selectable: bool,
    /// Per-cell highlight ranges (char-index ranges into the fitted strings).
    /// Outer vec is parallel to `cells`.
    pub highlights: Vec<Vec<Range<usize>>>,
}

/// One physical line of a multi-line row (see
/// [`compute_multiline_table`](crate::layout::compute_multiline_table)).
///
/// `cells` holds the fitted strings of the columns assigned to this line, in
/// the order of the line's template. An empty `cells` makes the line a blank
/// spacer.
#[derive(Debug, Clone)]
pub struct ComputedLine {
    /// One fitted string per column on this line, in line-template order.
    pub cells: Vec<String>,
    /// Per-cell highlight ranges, parallel to `cells`.
    pub highlights: Vec<Vec<Range<usize>>>,
    /// Whether this line is painted with the selection style when its row
    /// is selected. `false` lets a line (e.g. a spacer) stay visually
    /// "outside" the selection block.
    pub highlight_on_select: bool,
}

/// A fully laid-out multi-line row: one logical row rendered as a stack of
/// [`ComputedLine`]s. A single-element `lines` is the single-line case.
#[derive(Debug, Clone)]
pub struct ComputedMultiRow<Id: Eq + Hash + Clone> {
    pub id: Id,
    /// Physical lines, top to bottom.
    pub lines: Vec<ComputedLine>,
    /// Whether this row can be selected by the user.
    pub selectable: bool,
}
