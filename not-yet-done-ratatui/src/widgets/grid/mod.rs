mod component;
mod layout;
mod render;

pub mod border;
pub mod keymap;
pub mod state;

pub use border::{
    BORDER_DASHED, BORDER_DASHED_EXTENDED, BORDER_DOTTED, BORDER_DOTTED_EXTENDED,
    BORDER_DOUBLE_EXTENDED, BORDER_ROUNDED, BORDER_ROUNDED_EXTENDED, BORDER_SIMPLE,
    BORDER_SIMPLE_EXTENDED, BORDER_THICK_EXTENDED, BorderChars, BorderPos, CellGroup, GapPos,
    TextAnchor,
};
pub use keymap::GridKeymap;
pub use state::GridEvent;

use ratatui::style::Style;
use tuirealm::event::KeyEvent;

use border::{BorderPosExt, NormalisedPos};

// ---------------------------------------------------------------------------
// Internal supporting types
// ---------------------------------------------------------------------------

/// Text overlay written into a gap/border row or column.
#[derive(Debug, Clone)]
pub(super) struct GapText {
    pub anchor: TextAnchor,
    pub offset: usize,
    pub text: String,
}

/// A full-width/height border entry for a gap line.
#[derive(Debug, Clone)]
pub(super) struct BorderEntry {
    pub chars: &'static BorderChars,
    pub style: Option<Style>,
    pub text: Option<GapText>,
}

/// A partial (spanned) border overlay on a gap line.
#[derive(Debug, Clone)]
pub(super) struct SpannedBorderEntry {
    /// For a vertical gap: row_start / row_end.
    /// For a horizontal gap: col_start / col_end.
    pub start: usize,
    pub end: usize,
    pub chars: &'static BorderChars,
    pub style: Option<Style>,
    pub text: Option<GapText>,
}

/// Configuration for a single vertical gap (between column `idx` and `idx+1`).
#[derive(Debug, Clone, Default)]
pub(super) struct ColGapConfig {
    pub has_gap: bool,
    pub full: Option<BorderEntry>,
    pub spans: Vec<SpannedBorderEntry>,
}

/// Configuration for a single horizontal gap (between row `idx` and `idx+1`).
#[derive(Debug, Clone, Default)]
pub(super) struct RowGapConfig {
    pub has_gap: bool,
    pub full: Option<BorderEntry>,
    pub spans: Vec<SpannedBorderEntry>,
}

/// Outer-frame (BorderPos::Grid) configuration.
#[derive(Debug, Clone, Default)]
pub(super) struct OuterConfig {
    pub enabled: bool,
    pub chars: Option<&'static BorderChars>,
    pub style: Option<Style>,
    pub text: Option<GapText>,
}

/// A resolved group of cells (canonical bounding box + member set).
#[derive(Debug, Clone)]
pub(super) struct GroupDef {
    pub first_row: usize,
    pub first_col: usize,
    pub last_row: usize,
    pub last_col: usize,
}

impl GroupDef {
    pub fn contains(&self, row: usize, col: usize) -> bool {
        row >= self.first_row
            && row <= self.last_row
            && col >= self.first_col
            && col <= self.last_col
    }

    /// Returns true if `other` is fully contained inside `self`.
    pub fn contains_group(&self, other: &GroupDef) -> bool {
        other.first_row >= self.first_row
            && other.last_row <= self.last_row
            && other.first_col >= self.first_col
            && other.last_col <= self.last_col
    }

    pub fn overlaps(&self, other: &GroupDef) -> bool {
        self.first_row <= other.last_row
            && self.last_row >= other.first_row
            && self.first_col <= other.last_col
            && self.last_col >= other.first_col
    }
}

// ---------------------------------------------------------------------------
// GridChild trait
// ---------------------------------------------------------------------------

/// Supertrait for any component that can be placed inside a [`Grid`] cell.
///
/// The grid calls [`on_key`](GridChild::on_key) instead of
/// [`AppComponent::on`](tuirealm::component::AppComponent) so that keyboard
/// routing is fully controlled by the grid.  Return `true` if the key was
/// consumed by the child; return `false` to let the grid handle it as a
/// navigation key.
///
/// Blanket implementations for the widgets in this crate are provided in
/// [`component`].
pub trait GridChild: tuirealm::component::Component {
    /// Process a key event.  Returns `true` when consumed, `false` otherwise.
    fn on_key(&mut self, key: KeyEvent) -> bool;
}

// ---------------------------------------------------------------------------
// Grid
// ---------------------------------------------------------------------------

/// A layout component that arranges child widgets in an n×m grid.
///
/// # Quick start
///
/// ```rust
/// use tuirealm::event::{Key, KeyEvent, KeyModifiers};
/// use ratatui::layout::Constraint;
/// use not_yet_done_ratatui::widgets::grid::{Grid, GridKeymap, GapPos, BorderPos, BORDER_SIMPLE};
///
/// let mut grid = Grid::new(2, 2)
///     .with_column_constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
///     .with_row_constraints([Constraint::Length(3), Constraint::Length(3)])
///     .with_keymap(GridKeymap {
///         next_cell: Some(KeyEvent { code: Key::Tab,     modifiers: KeyModifiers::NONE }),
///         prev_cell: Some(KeyEvent { code: Key::BackTab, modifiers: KeyModifiers::SHIFT }),
///         ..GridKeymap::default()
///     });
///
/// grid.set_gap(GapPos::AfterCol(0));
/// grid.set_border(BorderPos::Grid, &BORDER_SIMPLE);
/// ```
pub struct Grid {
    // --- dimensions ---
    pub(super) rows: usize,
    pub(super) cols: usize,

    // --- layout ---
    pub(super) col_constraints: Vec<ratatui::layout::Constraint>,
    pub(super) row_constraints: Vec<ratatui::layout::Constraint>,

    // --- gaps & borders ---
    /// `v_gaps[i]` = gap between column `i` and column `i+1`; length = cols−1.
    pub(super) v_gaps: Vec<ColGapConfig>,
    /// `h_gaps[i]` = gap between row `i` and row `i+1`; length = rows−1.
    pub(super) h_gaps: Vec<RowGapConfig>,
    pub(super) outer: OuterConfig,

    // --- groups ---
    pub(super) groups: Vec<GroupDef>,

    // --- styling ---
    /// Per-cell styles, row-major (index = row * cols + col).
    pub(super) cell_styles: Vec<Option<Style>>,
    pub(super) global_style: Style,

    // --- focus ---
    /// Whether the Grid itself has been focused by the tuirealm framework.
    pub(super) focused: bool,
    /// Currently focused cell `(row, col)`.
    pub(super) focus_cell: (usize, usize),
    /// Navigation anchor: the concrete cell position used when computing the
    /// next navigation step from within a group.
    pub(super) focus_anchor: (usize, usize),

    // --- children ---
    /// Row-major; index = row * cols + col.
    pub(super) children: Vec<Option<Box<dyn GridChild>>>,

    // --- keymap ---
    pub(super) keymap: GridKeymap,
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

impl Grid {
    /// Creates a new empty grid with `rows` rows and `cols` columns.
    pub fn new(rows: usize, cols: usize) -> Self {
        let cells = rows * cols;
        let v_count = cols.saturating_sub(1);
        let h_count = rows.saturating_sub(1);
        Self {
            rows,
            cols,
            col_constraints: vec![ratatui::layout::Constraint::Ratio(1, cols as u32); cols],
            row_constraints: vec![ratatui::layout::Constraint::Ratio(1, rows as u32); rows],
            v_gaps: vec![ColGapConfig::default(); v_count],
            h_gaps: vec![RowGapConfig::default(); h_count],
            outer: OuterConfig::default(),
            groups: Vec::new(),
            cell_styles: vec![None; cells],
            global_style: Style::default(),
            focused: false,
            focus_cell: (0, 0),
            focus_anchor: (0, 0),
            children: (0..cells).map(|_| None).collect(),
            keymap: GridKeymap::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Builder methods (consume self, return Self for chaining)
// ---------------------------------------------------------------------------

impl Grid {
    /// Sets column-width constraints (analogous to ratatui `Layout`).
    pub fn with_column_constraints(
        mut self,
        constraints: impl IntoIterator<Item = ratatui::layout::Constraint>,
    ) -> Self {
        self.col_constraints = constraints.into_iter().collect();
        self
    }

    /// Sets row-height constraints.
    pub fn with_row_constraints(
        mut self,
        constraints: impl IntoIterator<Item = ratatui::layout::Constraint>,
    ) -> Self {
        self.row_constraints = constraints.into_iter().collect();
        self
    }

    /// Sets the keyboard navigation keymap.
    pub fn with_keymap(mut self, keymap: GridKeymap) -> Self {
        self.keymap = keymap;
        self
    }

    /// Sets the global default style (for gaps and unset borders).
    pub fn with_style(mut self, style: Style) -> Self {
        self.global_style = style;
        self
    }
}

// ---------------------------------------------------------------------------
// Mutation methods (mutable, return ())
// ---------------------------------------------------------------------------

impl Grid {
    // --- child widgets ---

    /// Inserts a child widget into cell `(row, col)`.
    pub fn set_child(&mut self, row: usize, col: usize, child: Box<dyn GridChild>) {
        if let Some(slot) = self.children.get_mut(row * self.cols + col) {
            *slot = Some(child);
        }
    }

    // --- gaps ---

    /// Adds a 1-character gap at `pos`.
    ///
    /// `GapPos::Grid` sets all inner gaps.
    /// `set_border` implicitly calls this for the relevant position.
    pub fn set_gap(&mut self, pos: GapPos) {
        match pos {
            GapPos::Grid => {
                for g in &mut self.v_gaps {
                    g.has_gap = true;
                }
                for g in &mut self.h_gaps {
                    g.has_gap = true;
                }
            }
            GapPos::AfterCol(i) => {
                if let Some(g) = self.v_gaps.get_mut(i) {
                    g.has_gap = true;
                }
            }
            GapPos::BeforeCol(i) if i > 0 => {
                if let Some(g) = self.v_gaps.get_mut(i - 1) {
                    g.has_gap = true;
                }
            }
            GapPos::AfterRow(i) => {
                if let Some(g) = self.h_gaps.get_mut(i) {
                    g.has_gap = true;
                }
            }
            GapPos::BeforeRow(i) if i > 0 => {
                if let Some(g) = self.h_gaps.get_mut(i - 1) {
                    g.has_gap = true;
                }
            }
            _ => {}
        }
    }

    /// Removes a gap and all borders within it.
    pub fn remove_gap(&mut self, pos: GapPos) {
        match pos {
            GapPos::Grid => {
                for g in &mut self.v_gaps {
                    *g = ColGapConfig::default();
                }
                for g in &mut self.h_gaps {
                    *g = RowGapConfig::default();
                }
            }
            GapPos::AfterCol(i) => {
                if let Some(g) = self.v_gaps.get_mut(i) {
                    *g = ColGapConfig::default();
                }
            }
            GapPos::BeforeCol(i) if i > 0 => {
                if let Some(g) = self.v_gaps.get_mut(i - 1) {
                    *g = ColGapConfig::default();
                }
            }
            GapPos::AfterRow(i) => {
                if let Some(g) = self.h_gaps.get_mut(i) {
                    *g = RowGapConfig::default();
                }
            }
            GapPos::BeforeRow(i) if i > 0 => {
                if let Some(g) = self.h_gaps.get_mut(i - 1) {
                    *g = RowGapConfig::default();
                }
            }
            _ => {}
        }
    }

    // --- borders ---

    /// Sets a border at `pos` (implicitly creates a gap if none exists).
    pub fn set_border(&mut self, pos: BorderPos, chars: &'static BorderChars) {
        let rows = self.rows;
        let cols = self.cols;
        let Some(norm) = pos.normalise(rows, cols) else {
            return;
        };
        match norm {
            NormalisedPos::Outer => {
                self.outer.enabled = true;
                self.outer.chars = Some(chars);
            }
            NormalisedPos::VGap(i) => {
                if let Some(g) = self.v_gaps.get_mut(i) {
                    g.has_gap = true;
                    g.full = Some(BorderEntry {
                        chars,
                        style: None,
                        text: None,
                    });
                }
            }
            NormalisedPos::HGap(i) => {
                if let Some(g) = self.h_gaps.get_mut(i) {
                    g.has_gap = true;
                    g.full = Some(BorderEntry {
                        chars,
                        style: None,
                        text: None,
                    });
                }
            }
            NormalisedPos::VGapSpanned {
                gap_idx,
                start,
                end,
            } => {
                if let Some(g) = self.v_gaps.get_mut(gap_idx) {
                    g.has_gap = true;
                    // Replace existing span for same range, otherwise push.
                    if let Some(span) = g
                        .spans
                        .iter_mut()
                        .find(|s| s.start == start && s.end == end)
                    {
                        span.chars = chars;
                    } else {
                        g.spans.push(SpannedBorderEntry {
                            start,
                            end,
                            chars,
                            style: None,
                            text: None,
                        });
                    }
                }
            }
            NormalisedPos::HGapSpanned {
                gap_idx,
                start,
                end,
            } => {
                if let Some(g) = self.h_gaps.get_mut(gap_idx) {
                    g.has_gap = true;
                    if let Some(span) = g
                        .spans
                        .iter_mut()
                        .find(|s| s.start == start && s.end == end)
                    {
                        span.chars = chars;
                    } else {
                        g.spans.push(SpannedBorderEntry {
                            start,
                            end,
                            chars,
                            style: None,
                            text: None,
                        });
                    }
                }
            }
        }
    }

    /// Removes border decoration from `pos`; the gap (whitespace) remains.
    pub fn remove_border(&mut self, pos: BorderPos) {
        let rows = self.rows;
        let cols = self.cols;
        let Some(norm) = pos.normalise(rows, cols) else {
            return;
        };
        match norm {
            NormalisedPos::Outer => {
                self.outer.chars = None;
            }
            NormalisedPos::VGap(i) => {
                if let Some(g) = self.v_gaps.get_mut(i) {
                    g.full = None;
                }
            }
            NormalisedPos::HGap(i) => {
                if let Some(g) = self.h_gaps.get_mut(i) {
                    g.full = None;
                }
            }
            NormalisedPos::VGapSpanned {
                gap_idx,
                start,
                end,
            } => {
                if let Some(g) = self.v_gaps.get_mut(gap_idx) {
                    g.spans.retain(|s| !(s.start == start && s.end == end));
                }
            }
            NormalisedPos::HGapSpanned {
                gap_idx,
                start,
                end,
            } => {
                if let Some(g) = self.h_gaps.get_mut(gap_idx) {
                    g.spans.retain(|s| !(s.start == start && s.end == end));
                }
            }
        }
    }

    /// Sets a style for the gap/border at `pos`.
    pub fn set_border_style(&mut self, pos: BorderPos, style: Style) {
        let rows = self.rows;
        let cols = self.cols;
        let Some(norm) = pos.normalise(rows, cols) else {
            return;
        };
        match norm {
            NormalisedPos::Outer => {
                self.outer.style = Some(style);
            }
            NormalisedPos::VGap(i) => {
                if let Some(g) = self.v_gaps.get_mut(i) {
                    if let Some(b) = &mut g.full {
                        b.style = Some(style);
                    }
                    // Also set as gap default when no border is set yet.
                }
            }
            NormalisedPos::HGap(i) => {
                if let Some(g) = self.h_gaps.get_mut(i) {
                    if let Some(b) = &mut g.full {
                        b.style = Some(style);
                    }
                }
            }
            NormalisedPos::VGapSpanned {
                gap_idx,
                start,
                end,
            } => {
                if let Some(g) = self.v_gaps.get_mut(gap_idx) {
                    if let Some(span) = g
                        .spans
                        .iter_mut()
                        .find(|s| s.start == start && s.end == end)
                    {
                        span.style = Some(style);
                    }
                }
            }
            NormalisedPos::HGapSpanned {
                gap_idx,
                start,
                end,
            } => {
                if let Some(g) = self.h_gaps.get_mut(gap_idx) {
                    if let Some(span) = g
                        .spans
                        .iter_mut()
                        .find(|s| s.start == start && s.end == end)
                    {
                        span.style = Some(style);
                    }
                }
            }
        }
    }

    /// Writes text into the gap/border area at `pos`.
    pub fn set_border_text(
        &mut self,
        pos: BorderPos,
        anchor: TextAnchor,
        offset: usize,
        text: impl Into<String>,
    ) {
        let rows = self.rows;
        let cols = self.cols;
        let Some(norm) = pos.normalise(rows, cols) else {
            return;
        };
        let gt = GapText {
            anchor,
            offset,
            text: text.into(),
        };
        match norm {
            NormalisedPos::Outer => {
                self.outer.text = Some(gt);
            }
            NormalisedPos::VGap(i) => {
                if let Some(g) = self.v_gaps.get_mut(i) {
                    if let Some(b) = &mut g.full {
                        b.text = Some(gt);
                    }
                }
            }
            NormalisedPos::HGap(i) => {
                if let Some(g) = self.h_gaps.get_mut(i) {
                    if let Some(b) = &mut g.full {
                        b.text = Some(gt);
                    }
                }
            }
            NormalisedPos::VGapSpanned {
                gap_idx,
                start,
                end,
            } => {
                if let Some(g) = self.v_gaps.get_mut(gap_idx) {
                    if let Some(span) = g
                        .spans
                        .iter_mut()
                        .find(|s| s.start == start && s.end == end)
                    {
                        span.text = Some(gt);
                    }
                }
            }
            NormalisedPos::HGapSpanned {
                gap_idx,
                start,
                end,
            } => {
                if let Some(g) = self.h_gaps.get_mut(gap_idx) {
                    if let Some(span) = g
                        .spans
                        .iter_mut()
                        .find(|s| s.start == start && s.end == end)
                    {
                        span.text = Some(gt);
                    }
                }
            }
        }
    }

    /// Removes text overlay from `pos`, restoring border/gap characters.
    pub fn remove_border_text(&mut self, pos: BorderPos) {
        let rows = self.rows;
        let cols = self.cols;
        let Some(norm) = pos.normalise(rows, cols) else {
            return;
        };
        match norm {
            NormalisedPos::Outer => {
                self.outer.text = None;
            }
            NormalisedPos::VGap(i) => {
                if let Some(g) = self.v_gaps.get_mut(i) {
                    if let Some(b) = &mut g.full {
                        b.text = None;
                    }
                }
            }
            NormalisedPos::HGap(i) => {
                if let Some(g) = self.h_gaps.get_mut(i) {
                    if let Some(b) = &mut g.full {
                        b.text = None;
                    }
                }
            }
            NormalisedPos::VGapSpanned {
                gap_idx,
                start,
                end,
            } => {
                if let Some(g) = self.v_gaps.get_mut(gap_idx) {
                    if let Some(span) = g
                        .spans
                        .iter_mut()
                        .find(|s| s.start == start && s.end == end)
                    {
                        span.text = None;
                    }
                }
            }
            NormalisedPos::HGapSpanned {
                gap_idx,
                start,
                end,
            } => {
                if let Some(g) = self.h_gaps.get_mut(gap_idx) {
                    if let Some(span) = g
                        .spans
                        .iter_mut()
                        .find(|s| s.start == start && s.end == end)
                    {
                        span.text = None;
                    }
                }
            }
        }
    }

    // --- cell styling ---

    /// Sets a style for cell `(row, col)`.
    pub fn configure_cell_style(&mut self, row: usize, col: usize, style: Style) {
        if let Some(s) = self.cell_styles.get_mut(row * self.cols + col) {
            *s = Some(style);
        }
    }

    // --- cell groups ---

    /// Groups the specified cells into a single logical unit.
    ///
    /// # Panics (debug builds)
    /// Panics when the new group partially overlaps an existing group without
    /// fully containing it or being fully contained by it.
    pub fn group_cells(&mut self, group: CellGroup) {
        let new_def = self.cell_group_to_def(group);
        // Check overlap rules.
        let mut i = 0;
        while i < self.groups.len() {
            let existing = &self.groups[i];
            if existing.overlaps(&new_def) {
                if new_def.contains_group(existing) {
                    // New group is bigger: remove the smaller one.
                    self.groups.remove(i);
                    continue;
                } else if existing.contains_group(&new_def) {
                    // Existing group is bigger: new group is subsumed, do nothing.
                    return;
                } else {
                    // Partial overlap — invalid.
                    debug_assert!(
                        false,
                        "group_cells: partial overlap between new group \
                         ({},{})–({},{}) and existing ({},{})–({},{})",
                        new_def.first_row,
                        new_def.first_col,
                        new_def.last_row,
                        new_def.last_col,
                        existing.first_row,
                        existing.first_col,
                        existing.last_row,
                        existing.last_col,
                    );
                    return;
                }
            }
            i += 1;
        }
        self.groups.push(new_def);
    }

    /// Dissolves the group that contains cell `(row, col)`.
    pub fn ungroup_cells(&mut self, row: usize, col: usize) {
        self.groups.retain(|g| !g.contains(row, col));
    }

    // --- keymap convenience setters ---

    pub fn set_key_next(&mut self, key: KeyEvent) {
        self.keymap.next_cell = Some(key);
    }
    pub fn set_key_prev(&mut self, key: KeyEvent) {
        self.keymap.prev_cell = Some(key);
    }
    pub fn set_key_next_row(&mut self, key: KeyEvent) {
        self.keymap.next_in_row = Some(key);
    }
    pub fn set_key_prev_row(&mut self, key: KeyEvent) {
        self.keymap.prev_in_row = Some(key);
    }
    pub fn set_key_next_col(&mut self, key: KeyEvent) {
        self.keymap.next_in_col = Some(key);
    }
    pub fn set_key_prev_col(&mut self, key: KeyEvent) {
        self.keymap.prev_in_col = Some(key);
    }

    // --- focus query ---

    /// Returns the currently focused cell as `(row, col)`.
    pub fn focused_cell(&self) -> (usize, usize) {
        self.focus_cell
    }

    /// Returns the [`tuirealm::state::State`] of the child widget at `(row, col)`.
    ///
    /// Returns [`tuirealm::state::State::None`] when no child is mounted there.
    pub fn child_state(&self, row: usize, col: usize) -> tuirealm::state::State {
        self.children
            .get(row * self.cols + col)
            .and_then(|c| c.as_ref())
            .map(|c| c.state())
            .unwrap_or(tuirealm::state::State::None)
    }

    // --- programmatic focus navigation ---

    /// Moves focus to the next cell in zig-zag order (row-major, wrapping).
    pub fn focus_next(&mut self) {
        let anchor = self.focus_anchor;
        let next = self.next_navigable_after(anchor, 1);
        self.set_focus(next);
    }

    /// Moves focus to the previous cell in zig-zag order (wrapping).
    pub fn focus_prev(&mut self) {
        let anchor = self.focus_anchor;
        let prev = self.next_navigable_after(anchor, -1);
        self.set_focus(prev);
    }

    /// Moves focus one cell to the right in the current row (wrapping).
    pub fn focus_next_in_row(&mut self) {
        let (row, col) = self.focus_anchor;
        let next_col = (col + 1) % self.cols;
        let target = (row, next_col);
        self.set_focus(target);
    }

    /// Moves focus one cell to the left in the current row (wrapping).
    pub fn focus_prev_in_row(&mut self) {
        let (row, col) = self.focus_anchor;
        let prev_col = if col == 0 { self.cols - 1 } else { col - 1 };
        let target = (row, prev_col);
        self.set_focus(target);
    }

    /// Moves focus one cell down in the current column (wrapping).
    pub fn focus_next_in_col(&mut self) {
        let (row, col) = self.focus_anchor;
        let next_row = (row + 1) % self.rows;
        let target = (next_row, col);
        self.set_focus(target);
    }

    /// Moves focus one cell up in the current column (wrapping).
    pub fn focus_prev_in_col(&mut self) {
        let (row, col) = self.focus_anchor;
        let prev_row = if row == 0 { self.rows - 1 } else { row - 1 };
        let target = (prev_row, col);
        self.set_focus(target);
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

impl Grid {
    /// Converts a `CellGroup` enum value to a `GroupDef`.
    fn cell_group_to_def(&self, group: CellGroup) -> GroupDef {
        match group {
            CellGroup::Row(r) => GroupDef {
                first_row: r,
                last_row: r,
                first_col: 0,
                last_col: self.cols.saturating_sub(1),
            },
            CellGroup::Col(c) => GroupDef {
                first_row: 0,
                last_row: self.rows.saturating_sub(1),
                first_col: c,
                last_col: c,
            },
            CellGroup::ColSpan {
                row,
                first_col,
                last_col,
            } => GroupDef {
                first_row: row,
                last_row: row,
                first_col,
                last_col,
            },
            CellGroup::RowSpan {
                col,
                first_row,
                last_row,
            } => GroupDef {
                first_row,
                last_row,
                first_col: col,
                last_col: col,
            },
            CellGroup::Span {
                first_row,
                first_col,
                last_row,
                last_col,
            } => GroupDef {
                first_row,
                first_col,
                last_row,
                last_col,
            },
        }
    }

    /// Returns the `GroupDef` that contains `(row, col)`, if any.
    pub(super) fn group_for(&self, row: usize, col: usize) -> Option<&GroupDef> {
        self.groups.iter().find(|g| g.contains(row, col))
    }

    /// Returns whether `(row, col)` is the top-left anchor of its group
    /// (or has no group).
    pub(super) fn is_group_origin(&self, row: usize, col: usize) -> bool {
        match self.group_for(row, col) {
            None => true,
            Some(g) => g.first_row == row && g.first_col == col,
        }
    }

    /// Returns the list of "navigation positions" — one entry per group or
    /// ungrouped cell, in row-major order.
    pub(super) fn nav_positions(&self) -> Vec<(usize, usize)> {
        let mut seen_groups: Vec<usize> = Vec::new();
        let mut positions = Vec::new();
        for row in 0..self.rows {
            for col in 0..self.cols {
                if let Some(g_idx) = self.groups.iter().position(|g| g.contains(row, col)) {
                    if !seen_groups.contains(&g_idx) {
                        seen_groups.push(g_idx);
                        let g = &self.groups[g_idx];
                        positions.push((g.first_row, g.first_col));
                    }
                } else {
                    positions.push((row, col));
                }
            }
        }
        positions
    }

    /// Moves focus to `target`, updating `focus_cell` and `focus_anchor`.
    pub(super) fn set_focus(&mut self, target: (usize, usize)) {
        let (row, col) = target;
        self.focus_anchor = (row, col);
        // If the target belongs to a group, focus_cell is the group origin.
        self.focus_cell = self
            .group_for(row, col)
            .map(|g| (g.first_row, g.first_col))
            .unwrap_or((row, col));
        // Update child focus flags.
        self.update_child_focus();
    }

    /// Calls `attr(Focus, …)` on all children, giving focus only to the
    /// focused cell's child.
    fn update_child_focus(&mut self) {
        let focused_cell = self.focus_cell;
        let n = self.children.len();
        for idx in 0..n {
            let row = idx / self.cols;
            let col = idx % self.cols;
            let should_focus = self.focused && (row, col) == focused_cell;
            if let Some(child) = &mut self.children[idx] {
                child.attr(
                    tuirealm::props::Attribute::Focus,
                    tuirealm::props::AttrValue::Flag(should_focus),
                );
            }
        }
    }

    /// Advances `anchor` by `delta` (+1 or -1) through the nav positions.
    fn next_navigable_after(&self, anchor: (usize, usize), delta: i32) -> (usize, usize) {
        let positions = self.nav_positions();
        if positions.is_empty() {
            return anchor;
        }
        // Find the current position index (prefer exact match; fall back to closest).
        let current_idx = positions
            .iter()
            .position(|&p| p == anchor)
            .or_else(|| {
                // anchor may be inside a group — find the group's nav entry
                self.group_for(anchor.0, anchor.1).and_then(|g| {
                    positions
                        .iter()
                        .position(|&p| p == (g.first_row, g.first_col))
                })
            })
            .unwrap_or(0);

        let len = positions.len() as i32;
        let next_idx = ((current_idx as i32 + delta).rem_euclid(len)) as usize;
        positions[next_idx]
    }
}
