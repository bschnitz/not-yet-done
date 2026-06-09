use ratatui::layout::Constraint;

// ── BorderChars ───────────────────────────────────────────────────────────────

/// A complete set of Unicode box-drawing characters for one border style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BorderChars {
    pub horizontal:   char,
    pub vertical:     char,
    pub cross:        char,
    pub top_left:     char,
    pub top_right:    char,
    pub bottom_left:  char,
    pub bottom_right: char,
    pub t_left:       char,
    pub t_right:      char,
    pub t_top:        char,
    pub t_bottom:     char,
    /// Half-ending: line starts here going downward (top cap of a partial vertical border).
    pub half_top:     char,
    /// Half-ending: line ends here going upward (bottom cap of a partial vertical border).
    pub half_bottom:  char,
    /// Half-ending: line starts here going right (left cap of a partial horizontal border).
    pub half_left:    char,
    /// Half-ending: line ends here going left (right cap of a partial horizontal border).
    pub half_right:   char,
}

impl BorderChars {
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        horizontal: char, vertical: char, cross: char,
        top_left: char, top_right: char, bottom_left: char, bottom_right: char,
        t_left: char, t_right: char, t_top: char, t_bottom: char,
        half_top: char, half_bottom: char, half_left: char, half_right: char,
    ) -> Self {
        Self {
            horizontal, vertical, cross,
            top_left, top_right, bottom_left, bottom_right,
            t_left, t_right, t_top, t_bottom,
            half_top, half_bottom, half_left, half_right,
        }
    }
}

// ── Predefined border styles ──────────────────────────────────────────────────

pub static BORDER_SIMPLE: BorderChars = BorderChars::new(
    '─', '│', '┼', '┌', '┐', '└', '┘', '├', '┤', '┬', '┴', '╷', '╵', '╶', '╴',
);
pub static BORDER_SIMPLE_EXTENDED: BorderChars = BorderChars::new(
    '─', '│', '┼', '┌', '┐', '└', '┘', '├', '┤', '┬', '┴', '│', '│', '─', '─',
);
pub static BORDER_DOUBLE_EXTENDED: BorderChars = BorderChars::new(
    '═', '║', '╬', '╔', '╗', '╚', '╝', '╠', '╣', '╦', '╩', '║', '║', '═', '═',
);
pub static BORDER_THICK_EXTENDED: BorderChars = BorderChars::new(
    '━', '┃', '╋', '┏', '┓', '┗', '┛', '┣', '┫', '┳', '┻', '┃', '┃', '━', '━',
);
pub static BORDER_ROUNDED: BorderChars = BorderChars::new(
    '─', '│', '┼', '╭', '╮', '╰', '╯', '├', '┤', '┬', '┴', '╷', '╵', '╶', '╴',
);
pub static BORDER_ROUNDED_EXTENDED: BorderChars = BorderChars::new(
    '─', '│', '┼', '╭', '╮', '╰', '╯', '├', '┤', '┬', '┴', '│', '│', '─', '─',
);
pub static BORDER_DASHED: BorderChars = BorderChars::new(
    '┄', '┆', '┼', '┌', '┐', '└', '┘', '├', '┤', '┬', '┴', '╷', '╵', '╶', '╴',
);
pub static BORDER_DASHED_EXTENDED: BorderChars = BorderChars::new(
    '┄', '┆', '┼', '┌', '┐', '└', '┘', '├', '┤', '┬', '┴', '│', '│', '─', '─',
);
pub static BORDER_DOTTED: BorderChars = BorderChars::new(
    '┈', '┊', '┼', '┌', '┐', '└', '┘', '├', '┤', '┬', '┴', '╷', '╵', '╶', '╴',
);
pub static BORDER_DOTTED_EXTENDED: BorderChars = BorderChars::new(
    '┈', '┊', '┼', '┌', '┐', '└', '┘', '├', '┤', '┬', '┴', '│', '│', '─', '─',
);

// ── BorderPos ─────────────────────────────────────────────────────────────────

/// Addresses a border or gap position within the grid.
///
/// `After*(i)` and `Before*(i+1)` address the same physical slot.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BorderPos {
    /// Outer frame around the entire grid.
    Grid,
    AfterCol(usize),
    BeforeCol(usize),
    AfterRow(usize),
    BeforeRow(usize),
    AfterColSpanned  { col: usize, row_start: usize, row_end: usize },
    BeforeColSpanned { col: usize, row_start: usize, row_end: usize },
    AfterRowSpanned  { row: usize, col_start: usize, col_end: usize },
    BeforeRowSpanned { row: usize, col_start: usize, col_end: usize },
}

// ── GapPos ────────────────────────────────────────────────────────────────────

/// Addresses a gap (empty space) position within the grid.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GapPos {
    /// All inner gaps (no outer frame).
    Grid,
    AfterCol(usize),
    BeforeCol(usize),
    AfterRow(usize),
    BeforeRow(usize),
}

// ── CellGroup ─────────────────────────────────────────────────────────────────

/// Describes a group of cells that behave as a single logical cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellGroup {
    Row(usize),
    Col(usize),
    ColSpan { row: usize, first_col: usize, last_col: usize },
    RowSpan { col: usize, first_row: usize, last_row: usize },
    Span    { first_row: usize, first_col: usize, last_row: usize, last_col: usize },
}

// ── TextAnchor ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAnchor {
    Start,
    End,
}

// ── BorderText ────────────────────────────────────────────────────────────────

/// A text overlay placed inside a gap/border area.
#[derive(Debug, Clone)]
pub struct BorderText {
    pub anchor: TextAnchor,
    pub offset: usize,
    pub text:   String,
}

// ── GapSlot ───────────────────────────────────────────────────────────────────

/// Configuration for one full-span gap line (vertical gap column or horizontal
/// gap row).
#[derive(Debug, Clone)]
pub struct GapSlot {
    /// Border character set, if a border is drawn in this gap.
    pub border: Option<&'static BorderChars>,
    /// Optional text overlay written on top of the border/gap characters.
    pub text:   Option<BorderText>,
}

// ── SpannedBorder ─────────────────────────────────────────────────────────────

/// A border that only spans part of a gap column or gap row.
#[derive(Debug, Clone)]
pub struct SpannedBorder {
    /// Index into `v_gaps` or `h_gaps` (0 = after column/row 0).
    pub gap_index: usize,
    /// Start of span (inclusive, in cells).
    pub start: usize,
    /// End of span (inclusive, in cells).
    pub end: usize,
    pub border: Option<&'static BorderChars>,
    pub text:   Option<BorderText>,
}

// ── GridConfig ────────────────────────────────────────────────────────────────

/// All render-relevant configuration for a grid.
///
/// This is the canonical, backend-independent representation used by both
/// `not-yet-done-grid-core` (rendering) and `grid-render-sim` (text-mode
/// simulation).  The `not-yet-done-ratatui` crate builds a `GridConfig` from
/// its `Grid` widget state before each render pass.
#[derive(Debug, Clone)]
pub struct GridConfig {
    pub rows: usize,
    pub cols: usize,
    pub col_constraints: Vec<Constraint>,
    pub row_constraints: Vec<Constraint>,

    /// `v_gaps[i]` is the gap after column `i` (between col `i` and col `i+1`).
    /// Length = `cols − 1`.
    pub v_gaps: Vec<Option<GapSlot>>,

    /// `h_gaps[i]` is the gap after row `i` (between row `i` and row `i+1`).
    /// Length = `rows − 1`.
    pub h_gaps: Vec<Option<GapSlot>>,

    /// Outer frame border, if set.
    pub outer_border: Option<&'static BorderChars>,

    /// Spanned vertical borders.
    pub v_spanned: Vec<SpannedBorder>,

    /// Spanned horizontal borders.
    pub h_spanned: Vec<SpannedBorder>,

    /// Cell groups (merged cells).
    pub groups: Vec<CellGroup>,

    /// Optional text overlay on the outer border's top edge.
    pub outer_border_text: Option<BorderText>,
}

impl GridConfig {
    pub fn new(rows: usize, cols: usize) -> Self {
        Self {
            rows,
            cols,
            col_constraints: vec![Constraint::Fill(1); cols],
            row_constraints: vec![Constraint::Fill(1); rows],
            v_gaps:          vec![None; cols.saturating_sub(1)],
            h_gaps:          vec![None; rows.saturating_sub(1)],
            outer_border:    None,
            v_spanned:       Vec::new(),
            h_spanned:       Vec::new(),
            groups:          Vec::new(),
            outer_border_text: None,
        }
    }

    // ── Gap helpers ───────────────────────────────────────────────────────────

    pub fn set_v_gap(&mut self, col: usize) {
        assert!(col < self.cols - 1, "col {col} out of range for v_gap");
        if self.v_gaps[col].is_none() {
            self.v_gaps[col] = Some(GapSlot { border: None, text: None });
        }
    }

    pub fn set_h_gap(&mut self, row: usize) {
        assert!(row < self.rows - 1, "row {row} out of range for h_gap");
        if self.h_gaps[row].is_none() {
            self.h_gaps[row] = Some(GapSlot { border: None, text: None });
        }
    }

    // ── Border helpers ────────────────────────────────────────────────────────

    pub fn set_v_border(&mut self, col: usize, border: &'static BorderChars) {
        self.set_v_gap(col);
        self.v_gaps[col].as_mut().unwrap().border = Some(border);
    }

    pub fn set_h_border(&mut self, row: usize, border: &'static BorderChars) {
        self.set_h_gap(row);
        self.h_gaps[row].as_mut().unwrap().border = Some(border);
    }

    pub fn set_outer_border(&mut self, border: &'static BorderChars) {
        self.outer_border = Some(border);
    }

    pub fn set_v_spanned(
        &mut self, col: usize, row_start: usize, row_end: usize,
        border: &'static BorderChars,
    ) {
        self.set_v_gap(col);
        self.v_spanned.push(SpannedBorder {
            gap_index: col, start: row_start, end: row_end,
            border: Some(border), text: None,
        });
    }

    pub fn set_h_spanned(
        &mut self, row: usize, col_start: usize, col_end: usize,
        border: &'static BorderChars,
    ) {
        self.set_h_gap(row);
        self.h_spanned.push(SpannedBorder {
            gap_index: row, start: col_start, end: col_end,
            border: Some(border), text: None,
        });
    }

    /// Apply a `BorderPos`, normalising `Before*` variants to their `After*`
    /// equivalents.
    pub fn apply_border_pos(&mut self, pos: &BorderPos, border: &'static BorderChars) {
        match pos {
            BorderPos::Grid               => self.set_outer_border(border),
            BorderPos::AfterCol(i)        => self.set_v_border(*i, border),
            BorderPos::BeforeCol(i)       => self.set_v_border(i - 1, border),
            BorderPos::AfterRow(i)        => self.set_h_border(*i, border),
            BorderPos::BeforeRow(i)       => self.set_h_border(i - 1, border),
            BorderPos::AfterColSpanned  { col, row_start, row_end } =>
                self.set_v_spanned(*col, *row_start, *row_end, border),
            BorderPos::BeforeColSpanned { col, row_start, row_end } =>
                self.set_v_spanned(col - 1, *row_start, *row_end, border),
            BorderPos::AfterRowSpanned  { row, col_start, col_end } =>
                self.set_h_spanned(*row, *col_start, *col_end, border),
            BorderPos::BeforeRowSpanned { row, col_start, col_end } =>
                self.set_h_spanned(row - 1, *col_start, *col_end, border),
        }
    }

    pub fn apply_gap_pos(&mut self, pos: &GapPos) {
        match pos {
            GapPos::Grid => {
                for i in 0..self.cols.saturating_sub(1) { self.set_v_gap(i); }
                for i in 0..self.rows.saturating_sub(1) { self.set_h_gap(i); }
            }
            GapPos::AfterCol(i)  => self.set_v_gap(*i),
            GapPos::BeforeCol(i) => self.set_v_gap(i - 1),
            GapPos::AfterRow(i)  => self.set_h_gap(*i),
            GapPos::BeforeRow(i) => self.set_h_gap(i - 1),
        }
    }

    pub fn set_border_text(
        &mut self,
        pos: &BorderPos,
        anchor: TextAnchor,
        offset: usize,
        text: &str,
    ) {
        let entry = BorderText { anchor, offset, text: text.to_string() };
        match pos {
            BorderPos::Grid => {
                self.outer_border_text = Some(entry);
            }
            BorderPos::AfterRow(i) => {
                self.set_h_gap(*i);
                if let Some(Some(slot)) = self.h_gaps.get_mut(*i) { slot.text = Some(entry); }
            }
            BorderPos::BeforeRow(i) => {
                self.set_h_gap(i - 1);
                if let Some(Some(slot)) = self.h_gaps.get_mut(i - 1) { slot.text = Some(entry); }
            }
            BorderPos::AfterCol(i) => {
                self.set_v_gap(*i);
                if let Some(Some(slot)) = self.v_gaps.get_mut(*i) { slot.text = Some(entry); }
            }
            BorderPos::BeforeCol(i) => {
                self.set_v_gap(i - 1);
                if let Some(Some(slot)) = self.v_gaps.get_mut(i - 1) { slot.text = Some(entry); }
            }
            BorderPos::AfterRowSpanned { row, col_start, col_end } => {
                self.set_h_gap(*row);
                if let Some(span) = self.h_spanned.iter_mut().find(|s| s.gap_index == *row) {
                    span.text = Some(entry);
                } else {
                    self.h_spanned.push(SpannedBorder {
                        gap_index: *row, start: *col_start, end: *col_end,
                        border: None, text: Some(entry),
                    });
                }
            }
            BorderPos::BeforeRowSpanned { row, col_start, col_end } => {
                self.set_h_gap(row - 1);
                let idx = row - 1;
                if let Some(span) = self.h_spanned.iter_mut().find(|s| s.gap_index == idx) {
                    span.text = Some(entry);
                } else {
                    self.h_spanned.push(SpannedBorder {
                        gap_index: idx, start: *col_start, end: *col_end,
                        border: None, text: Some(entry),
                    });
                }
            }
            BorderPos::AfterColSpanned { col, row_start, row_end } => {
                self.set_v_gap(*col);
                if let Some(span) = self.v_spanned.iter_mut().find(|s| s.gap_index == *col) {
                    span.text = Some(entry);
                } else {
                    self.v_spanned.push(SpannedBorder {
                        gap_index: *col, start: *row_start, end: *row_end,
                        border: None, text: Some(entry),
                    });
                }
            }
            BorderPos::BeforeColSpanned { col, row_start, row_end } => {
                self.set_v_gap(col - 1);
                let idx = col - 1;
                if let Some(span) = self.v_spanned.iter_mut().find(|s| s.gap_index == idx) {
                    span.text = Some(entry);
                } else {
                    self.v_spanned.push(SpannedBorder {
                        gap_index: idx, start: *row_start, end: *row_end,
                        border: None, text: Some(entry),
                    });
                }
            }
        }
    }

    // ── Total size hints (for tests with Length constraints only) ─────────────

    pub fn total_width_hint(&self) -> u16 {
        let cell_w: u16 = self.col_constraints.iter().map(|c| match c {
            Constraint::Length(n) => *n,
            _ => 7,
        }).sum();
        let gap_w: u16 = self.v_gaps.iter().filter(|g| g.is_some()).count() as u16;
        let border_w: u16 = if self.outer_border.is_some() { 2 } else { 0 };
        cell_w + gap_w + border_w
    }

    pub fn total_height_hint(&self) -> u16 {
        let cell_h: u16 = self.row_constraints.iter().map(|c| match c {
            Constraint::Length(n) => *n,
            _ => 3,
        }).sum();
        let gap_h: u16 = self.h_gaps.iter().filter(|g| g.is_some()).count() as u16;
        let border_h: u16 = if self.outer_border.is_some() { 2 } else { 0 };
        cell_h + gap_h + border_h
    }

    // ── Group query helpers ───────────────────────────────────────────────────

    /// Normalise any `CellGroup` to `(first_row, first_col, last_row, last_col)`.
    pub fn group_bounds(rows: usize, cols: usize, group: &CellGroup)
        -> (usize, usize, usize, usize)
    {
        match group {
            CellGroup::Row(r)  => (*r, 0, *r, cols - 1),
            CellGroup::Col(c)  => (0, *c, rows - 1, *c),
            CellGroup::ColSpan { row, first_col, last_col } =>
                (*row, *first_col, *row, *last_col),
            CellGroup::RowSpan { col, first_row, last_row } =>
                (*first_row, *col, *last_row, *col),
            CellGroup::Span { first_row, first_col, last_row, last_col } =>
                (*first_row, *first_col, *last_row, *last_col),
        }
    }

    pub fn group_of(&self, row: usize, col: usize) -> Option<&CellGroup> {
        self.groups.iter().find(|g| {
            let (fr, fc, lr, lc) = Self::group_bounds(self.rows, self.cols, g);
            row >= fr && row <= lr && col >= fc && col <= lc
        })
    }

    pub fn is_group_origin(&self, row: usize, col: usize) -> bool {
        match self.group_of(row, col) {
            None    => true,
            Some(g) => {
                let (fr, fc, _, _) = Self::group_bounds(self.rows, self.cols, g);
                row == fr && col == fc
            }
        }
    }

    /// Returns `true` when the vertical gap after column `v_gap_idx` in `row`
    /// is suppressed by a group (both neighbouring columns belong to the same group).
    pub fn is_inside_h_group(&self, row: usize, v_gap_idx: usize) -> bool {
        let col_right = v_gap_idx + 1;
        if col_right >= self.cols { return false; }
        match (self.group_of(row, v_gap_idx), self.group_of(row, col_right)) {
            (Some(gl), Some(gr)) => std::ptr::eq(gl, gr),
            _ => false,
        }
    }

    /// Returns `true` when the horizontal gap after row `h_gap_idx` in `col`
    /// is suppressed by a group (both neighbouring rows belong to the same group).
    pub fn is_inside_v_group(&self, h_gap_idx: usize, col: usize) -> bool {
        let row_below = h_gap_idx + 1;
        if row_below >= self.rows { return false; }
        match (self.group_of(h_gap_idx, col), self.group_of(row_below, col)) {
            (Some(ga), Some(gb)) => std::ptr::eq(ga, gb),
            _ => false,
        }
    }

    /// Add a cell group with overlap validation.
    pub fn group_cells(&mut self, group: CellGroup) {
        let (nfr, nfc, nlr, nlc) = Self::group_bounds(self.rows, self.cols, &group);
        let mut to_remove: Vec<usize> = Vec::new();
        for (idx, existing) in self.groups.iter().enumerate() {
            let (efr, efc, elr, elc) = Self::group_bounds(self.rows, self.cols, existing);
            let overlap = nfr.max(efr) <= nlr.min(elr) && nfc.max(efc) <= nlc.min(elc);
            if !overlap { continue; }
            let new_contains = nfr <= efr && nfc <= efc && nlr >= elr && nlc >= elc;
            let existing_contains = efr <= nfr && efc <= nfc && elr >= nlr && elc >= nlc;
            if existing_contains { return; }
            if new_contains { to_remove.push(idx); }
            else {
                panic!(
                    "group_cells: partial overlap between \
                     ({nfr},{nfc})..({nlr},{nlc}) and ({efr},{efc})..({elr},{elc})"
                );
            }
        }
        for idx in to_remove.into_iter().rev() { self.groups.remove(idx); }
        self.groups.push(group);
    }

    pub fn ungroup_cells(&mut self, row: usize, col: usize) {
        self.groups.retain(|g| {
            let (fr, fc, lr, lc) = Self::group_bounds(self.rows, self.cols, g);
            !(row >= fr && row <= lr && col >= fc && col <= lc)
        });
    }
}
