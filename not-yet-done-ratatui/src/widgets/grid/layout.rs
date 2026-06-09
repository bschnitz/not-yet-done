use ratatui::layout::{Constraint, Layout, Rect};

use super::{Grid, GroupDef};

/// Fully computed layout for one render pass.
pub(super) struct GridLayout {
    /// Absolute x-positions and widths for each column. Length = `cols`.
    pub col_rects: Vec<ColBand>,
    /// Absolute y-positions and heights for each row. Length = `rows`.
    pub row_rects: Vec<RowBand>,
    /// X-positions of vertical gap columns. Length = `cols − 1`.
    /// `None` when the gap is disabled (zero width).
    pub v_gap_x: Vec<Option<u16>>,
    /// Y-positions of horizontal gap rows. Length = `rows − 1`.
    pub h_gap_y: Vec<Option<u16>>,
    /// Whether the outer border takes up 1 character on each side.
    pub has_outer: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ColBand {
    pub x: u16,
    pub width: u16,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct RowBand {
    pub y: u16,
    pub height: u16,
}

impl GridLayout {
    /// Returns the `Rect` for cell `(row, col)`, ignoring groups.
    pub fn cell_rect(&self, row: usize, col: usize) -> Rect {
        let cb = self.col_rects[col];
        let rb = self.row_rects[row];
        Rect { x: cb.x, y: rb.y, width: cb.width, height: rb.height }
    }

    /// Returns the merged `Rect` for a group (spans the full bounding box
    /// including internal gaps).
    pub fn group_rect(&self, g: &GroupDef) -> Rect {
        let first_cb = self.col_rects[g.first_col];
        let last_cb = self.col_rects[g.last_col];
        let first_rb = self.row_rects[g.first_row];
        let last_rb = self.row_rects[g.last_row];

        let x = first_cb.x;
        let y = first_rb.y;
        let right = last_cb.x + last_cb.width;
        let bottom = last_rb.y + last_rb.height;
        Rect { x, y, width: right - x, height: bottom - y }
    }

    /// Returns the `Rect` for a cell, expanding to its group if it has one.
    pub fn effective_rect(&self, row: usize, col: usize, grid: &Grid) -> Rect {
        if let Some(g) = grid.group_for(row, col) {
            self.group_rect(g)
        } else {
            self.cell_rect(row, col)
        }
    }
}

/// Computes the full grid layout for `area`.
pub(super) fn compute_layout(grid: &Grid, area: Rect) -> GridLayout {
    let has_outer = grid.outer.enabled;
    let outer_pad: u16 = if has_outer { 1 } else { 0 };

    // Inner area after removing outer border.
    let inner = Rect {
        x: area.x + outer_pad,
        y: area.y + outer_pad,
        width: area.width.saturating_sub(2 * outer_pad),
        height: area.height.saturating_sub(2 * outer_pad),
    };

    // --- columns ---
    let v_gap_total: u16 = grid.v_gaps.iter().map(|g| if g.has_gap { 1 } else { 0 }).sum();
    let col_available = inner.width.saturating_sub(v_gap_total);

    let col_rects = split_constraints(&grid.col_constraints, col_available);

    // Assign x-positions to columns and record gap x-positions.
    let mut col_bands = Vec::with_capacity(grid.cols);
    let mut v_gap_x = Vec::with_capacity(grid.v_gaps.len());

    let mut x = inner.x;
    for (ci, &w) in col_rects.iter().enumerate() {
        col_bands.push(ColBand { x, width: w });
        x += w;
        if let Some(gap) = grid.v_gaps.get(ci) {
            if gap.has_gap {
                v_gap_x.push(Some(x));
                x += 1;
            } else {
                v_gap_x.push(None);
            }
        }
    }

    // --- rows ---
    let h_gap_total: u16 = grid.h_gaps.iter().map(|g| if g.has_gap { 1 } else { 0 }).sum();
    let row_available = inner.height.saturating_sub(h_gap_total);

    let row_heights = split_constraints(&grid.row_constraints, row_available);

    let mut row_bands = Vec::with_capacity(grid.rows);
    let mut h_gap_y = Vec::with_capacity(grid.h_gaps.len());

    let mut y = inner.y;
    for (ri, &h) in row_heights.iter().enumerate() {
        row_bands.push(RowBand { y, height: h });
        y += h;
        if let Some(gap) = grid.h_gaps.get(ri) {
            if gap.has_gap {
                h_gap_y.push(Some(y));
                y += 1;
            } else {
                h_gap_y.push(None);
            }
        }
    }

    GridLayout {
        col_rects: col_bands,
        row_rects: row_bands,
        v_gap_x,
        h_gap_y,
        has_outer,
    }
}

/// Uses ratatui `Layout` to split `available` pixels according to `constraints`.
/// Returns one width/height per slot.
fn split_constraints(constraints: &[Constraint], available: u16) -> Vec<u16> {
    if constraints.is_empty() {
        return Vec::new();
    }
    let dummy = Rect { x: 0, y: 0, width: available, height: 1 };
    let rects = Layout::horizontal(constraints).split(dummy);
    rects.iter().map(|r| r.width).collect()
}
