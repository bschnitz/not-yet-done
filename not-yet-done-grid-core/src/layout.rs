use crate::types::{CellGroup, GridConfig};
use ratatui::layout::{Constraint, Direction, Layout, Rect};

// ── GridLayout ────────────────────────────────────────────────────────────────

/// Computed pixel positions for every column, row, and gap of a grid.
///
/// All coordinates are absolute (relative to the terminal origin), matching
/// the values stored in a ratatui `Buffer`.
#[derive(Debug, Clone)]
pub struct GridLayout {
    /// Absolute x-offset of each column. Length = `cfg.cols`.
    pub col_x: Vec<u16>,
    /// Width of each column. Length = `cfg.cols`.
    pub col_w: Vec<u16>,

    /// Absolute y-offset of each row. Length = `cfg.rows`.
    pub row_y: Vec<u16>,
    /// Height of each row. Length = `cfg.rows`.
    pub row_h: Vec<u16>,

    /// Absolute x-position of each vertical gap column.
    /// `None` when no gap exists at that index.  Length = `cfg.cols − 1`.
    pub v_gap_x: Vec<Option<u16>>,

    /// Absolute y-position of each horizontal gap row.
    /// `None` when no gap exists at that index.  Length = `cfg.rows − 1`.
    pub h_gap_y: Vec<Option<u16>>,

    /// Absolute x where grid content (cells) starts (= area.x + 1 if outer border).
    pub content_x: u16,
    /// Absolute y where grid content (cells) starts (= area.y + 1 if outer border).
    pub content_y: u16,

    /// Total width of the area passed to [`compute_layout`].
    pub total_width: u16,
    /// Total height of the area passed to [`compute_layout`].
    pub total_height: u16,
}

impl GridLayout {
    /// Absolute `Rect` of cell `(row, col)` — does not account for groups.
    pub fn cell_rect(&self, row: usize, col: usize) -> Rect {
        Rect::new(
            self.col_x[col],
            self.row_y[row],
            self.col_w[col],
            self.row_h[row],
        )
    }

    /// Absolute `Rect` of the bounding box `(fr, fc) .. (lr, lc)`, spanning
    /// all member cells **and** any gap columns/rows between them.
    pub fn group_rect(&self, fr: usize, fc: usize, lr: usize, lc: usize) -> Rect {
        let x = self.col_x[fc];
        let y = self.row_y[fr];
        let w = (self.col_x[lc] + self.col_w[lc]).saturating_sub(x);
        let h = (self.row_y[lr] + self.row_h[lr]).saturating_sub(y);
        Rect::new(x, y, w, h)
    }

    /// Convenience: compute `group_rect` from a `CellGroup` using `cfg`.
    pub fn group_rect_for(&self, cfg: &GridConfig, group: &CellGroup) -> Rect {
        let (fr, fc, lr, lc) = GridConfig::group_bounds(cfg.rows, cfg.cols, group);
        self.group_rect(fr, fc, lr, lc)
    }

    /// `Rect` for cell `(row, col)`, expanded to its group if it has one.
    pub fn effective_cell_rect(&self, cfg: &GridConfig, row: usize, col: usize) -> Rect {
        if let Some(g) = cfg.group_of(row, col) {
            let (fr, fc, lr, lc) = GridConfig::group_bounds(cfg.rows, cfg.cols, g);
            self.group_rect(fr, fc, lr, lc)
        } else {
            self.cell_rect(row, col)
        }
    }
}

// ── compute_layout ────────────────────────────────────────────────────────────

/// Compute the full pixel layout for `cfg` given the available `area`.
pub fn compute_layout(cfg: &GridConfig, area: Rect) -> GridLayout {
    let has_outer = cfg.outer_border.is_some();

    // Inner area: shrink by 1 on each side when an outer border is present.
    let inner = if has_outer {
        Rect::new(
            area.x + 1,
            area.y + 1,
            area.width.saturating_sub(2),
            area.height.saturating_sub(2),
        )
    } else {
        area
    };

    // Build interleaved constraint lists (cells interleaved with gap slots).
    // Each present gap occupies exactly 1 character.
    let h_constraints = interleave_constraints(&cfg.col_constraints, &cfg.v_gaps);
    let v_constraints = interleave_constraints(&cfg.row_constraints, &cfg.h_gaps);

    let h_rects = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(h_constraints)
        .split(inner);

    let v_rects = Layout::default()
        .direction(Direction::Vertical)
        .constraints(v_constraints)
        .split(inner);

    let (col_x, col_w, v_gap_x) = extract_cells_and_gaps(&h_rects, cfg.cols, &cfg.v_gaps, true);
    let (row_y, row_h, h_gap_y) = extract_cells_and_gaps(&v_rects, cfg.rows, &cfg.h_gaps, false);

    GridLayout {
        col_x,
        col_w,
        row_y,
        row_h,
        v_gap_x,
        h_gap_y,
        content_x: inner.x,
        content_y: inner.y,
        total_width: area.width,
        total_height: area.height,
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build a constraint list interleaving cell constraints with gap constraints
/// (each present gap = `Length(1)`, absent gap = omitted).
fn interleave_constraints<T>(
    cell_constraints: &[Constraint],
    gaps: &[Option<T>],
) -> Vec<Constraint> {
    let mut result = Vec::with_capacity(cell_constraints.len() * 2);
    for (i, cell_c) in cell_constraints.iter().enumerate() {
        result.push(*cell_c);
        if i < gaps.len() && gaps[i].is_some() {
            result.push(Constraint::Length(1));
        }
    }
    result
}

/// Given the flat list of rects from `Layout::split` (interleaved cell+gap
/// rects), extract separate vecs for cell positions and gap positions.
///
/// `use_x = true`  → extract x/width  (horizontal layout).
/// `use_x = false` → extract y/height (vertical layout).
fn extract_cells_and_gaps<T>(
    rects: &[Rect],
    cell_count: usize,
    gaps: &[Option<T>],
    use_x: bool,
) -> (Vec<u16>, Vec<u16>, Vec<Option<u16>>) {
    let mut positions = Vec::with_capacity(cell_count);
    let mut sizes = Vec::with_capacity(cell_count);
    let mut gap_pos: Vec<Option<u16>> = vec![None; gaps.len()];

    let mut rect_idx = 0usize;
    for cell_idx in 0..cell_count {
        let r = rects[rect_idx];
        if use_x {
            positions.push(r.x);
            sizes.push(r.width);
        } else {
            positions.push(r.y);
            sizes.push(r.height);
        }
        rect_idx += 1;

        if cell_idx < gaps.len() && gaps[cell_idx].is_some() {
            let gr = rects[rect_idx];
            gap_pos[cell_idx] = Some(if use_x { gr.x } else { gr.y });
            rect_idx += 1;
        }
    }

    (positions, sizes, gap_pos)
}
