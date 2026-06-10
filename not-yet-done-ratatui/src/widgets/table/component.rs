use ratatui::{Frame, layout::Rect};
use tuirealm::command::{Cmd, CmdResult, Direction, Position};
use tuirealm::component::{AppComponent, Component};
use tuirealm::event::{Event, NoUserEvent};
use tuirealm::props::{AttrValue, Attribute, QueryResult};
use tuirealm::state::{State, StateValue};

use super::{
    ColumnStyles, StyleMap, TableWidgetRow,
    keymap::TableKeymap,
    render::{RenderData, render},
    state::TableEvent,
    style::TableStyle,
};

/// A scrollable, selectable table widget.
///
/// The widget owns the selection and scroll state. Consumers feed data via
/// `set_data()` and delegate key events via `on()`. The widget emits
/// [`TableEvent`]s when the selection changes.
///
/// Fixed header rows are always rendered at the top, fixed footer rows at
/// the bottom. Only the data rows in between scroll.
pub struct Table {
    // --- framework state ---
    pub(crate) focused: bool,

    // --- selection state (owned by widget) ---
    pub(crate) selected_row: usize,
    /// Optional column cursor. `None` disables the column-cursor feature
    /// entirely — no column highlight is rendered and column-nav methods
    /// are no-ops. `Some(idx)` enables it; `idx` is clamped to the column
    /// count of the currently rendered row.
    pub(crate) selected_column: Option<usize>,
    pub(crate) scroll_offset: usize,
    /// Smooth (line-wise) scrolling: number of leading physical lines of the
    /// row at `scroll_offset` that are clipped above the viewport top.
    /// Always 0 in the default discrete mode, where rows sit flush against
    /// the top edge. Together with `scroll_offset` it forms a single global
    /// physical-line scroll offset — see [`super::smooth`].
    pub(crate) scroll_sub_line: usize,
    /// Opt-in smooth scrolling. When `true`, navigation moves the viewport
    /// one physical line at a time (content glides continuously) and the
    /// selection stays on its row while that row is *fully* visible, handing
    /// off to the adjacent row in the scroll direction as soon as a pan clips
    /// it at an edge (early hand-off, triggered only by the *current* row
    /// leaving — the next row need not be fully visible yet). When nothing can
    /// scroll (everything fits, or at an edge) `j`/`k` step the selection to
    /// the next/previous row instead. `false` = classic discrete row-by-row
    /// scrolling.
    pub(crate) smooth_scroll: bool,
    /// Data-region height in physical lines from the last render. Smooth
    /// scrolling needs the line budget in event handlers (which run outside
    /// `view()`); discrete mode uses `last_visible_data_rows` instead.
    pub(crate) last_line_budget: usize,
    /// Rows visible in the last render (set in view(), used by adjust_scroll).
    pub(crate) last_visible_data_rows: usize,
    /// Number of leading columns hidden by horizontal scroll. Active only
    /// when `selected_column.is_some()` (column-cursor opt-in). Recomputed
    /// in `view()` so the active column is always fully visible.
    pub(crate) scroll_col_offset: usize,
    /// True when there are hidden columns to the right of the viewport.
    /// Set in `view()`; consumed by the header `›` indicator.
    pub(crate) has_more_right: bool,
    /// Width (in terminal columns) of the area this table last rendered
    /// into. Set in `view()`. The content view fits its columns to this
    /// width on rebuild (instead of a fixed budget), so a flex column
    /// fills exactly to the pane edge and trailing columns stay on-screen
    /// — matching the native render-time layout. 0 until the first paint.
    pub(crate) last_render_width: u16,

    // --- data ---
    /// Fixed rows always shown at the top (e.g. column headers).
    pub(crate) fixed_header_rows: Vec<TableWidgetRow>,
    /// Scrollable data rows.
    pub(crate) rows: Vec<TableWidgetRow>,
    /// Fixed rows always shown at the bottom (e.g. summary).
    pub(crate) fixed_footer_rows: Vec<TableWidgetRow>,

    // --- configuration ---
    pub(crate) separator: String,
    pub(crate) col_styles: ColumnStyles,
    pub(crate) style_map: StyleMap,
    pub(crate) style: TableStyle,
    pub(crate) keymap: TableKeymap,

    // --- hop-style jump navigation ---
    /// Characters used to generate jump labels.
    pub(crate) nav_chars: Vec<char>,
    /// Current jump phase.
    pub(crate) jump_phase: JumpPhase,
}

/// Jump navigation state machine.
#[derive(Debug, Clone)]
pub enum JumpPhase {
    /// Not active.
    Off,
    /// Waiting for the user to type a search character.
    WaitingForChar,
    /// Matches found — labels displayed, waiting for label input.
    ShowingLabels {
        search_char: char,
        /// (data_row_index, all_match_char_positions, label)
        matches: Vec<(usize, Vec<usize>, String)>,
        /// Partial label input so far.
        input: String,
    },
}

impl Default for Table {
    fn default() -> Self {
        Self {
            focused: false,
            selected_row: 0,
            selected_column: None,
            scroll_offset: 0,
            scroll_sub_line: 0,
            smooth_scroll: false,
            last_line_budget: 20,
            last_visible_data_rows: 20,
            scroll_col_offset: 0,
            has_more_right: false,
            last_render_width: 0,
            fixed_header_rows: Vec::new(),
            rows: Vec::new(),
            fixed_footer_rows: Vec::new(),
            separator: "  ".to_string(),
            col_styles: ColumnStyles::default(),
            style_map: StyleMap::default(),
            style: TableStyle::default(),
            keymap: TableKeymap::default(),
            nav_chars: Vec::new(),
            jump_phase: JumpPhase::Off,
        }
    }
}

impl Table {
    /// Whether jump mode is in any active phase.
    pub fn is_jump_active(&self) -> bool {
        !matches!(self.jump_phase, JumpPhase::Off)
    }

    /// Whether jump mode is waiting for the initial search character.
    pub fn is_jump_waiting_for_char(&self) -> bool {
        matches!(self.jump_phase, JumpPhase::WaitingForChar)
    }

    /// Configure the characters used for jump labels.
    pub fn set_nav_chars(&mut self, chars: &[char]) {
        self.nav_chars = chars.to_vec();
    }

    /// Enter jump mode phase 1 (waiting for search char).
    pub fn jump_mode_open(&mut self) {
        if !self.nav_chars.is_empty() {
            self.jump_phase = JumpPhase::WaitingForChar;
        }
    }

    /// Cancel jump mode.
    pub fn jump_mode_close(&mut self) {
        self.jump_phase = JumpPhase::Off;
    }

    /// Phase 1: user typed the search char. Find matches in visible rows
    /// and transition to phase 2. Returns true if any matches found.
    pub fn jump_mode_search(&mut self, ch: char) -> bool {
        let search = ch.to_lowercase().next().unwrap_or(ch);
        let sep_len = self.separator.chars().count();

        // First pass: collect (row_idx, all_match_positions).
        let mut raw_matches: Vec<(usize, Vec<usize>)> = Vec::new();
        for (vi, row) in self.rows.iter()
            .skip(self.scroll_offset)
            .take(self.last_visible_data_rows)
            .enumerate()
        {
            if !row.selectable { continue; }
            let mut positions = Vec::new();
            let mut char_offset = 0usize;
            for (ci, cell) in row.primary_line().iter().enumerate() {
                if ci > 0 { char_offset += sep_len; }
                for (i, c) in cell.text.chars().enumerate() {
                    if c.to_lowercase().next() == Some(search) {
                        positions.push(char_offset + i);
                    }
                }
                char_offset += cell.text.chars().count();
            }
            if !positions.is_empty() {
                raw_matches.push((self.scroll_offset + vi, positions));
            }
        }

        // Second pass: assign labels based on total match count.
        let total = raw_matches.len();
        let mut matches: Vec<(usize, Vec<usize>, String)> = Vec::new();
        for (i, (row_idx, positions)) in raw_matches.into_iter().enumerate() {
            let label = self.nth_label_of(i, total);
            if !label.is_empty() {
                matches.push((row_idx, positions, label));
            }
        }

        if matches.is_empty() {
            self.jump_phase = JumpPhase::Off;
            false
        } else if matches.len() == 1 {
            let row_idx = matches[0].0;
            self.jump_phase = JumpPhase::Off;
            self.set_selected(row_idx);
            true
        } else {
            self.jump_phase = JumpPhase::ShowingLabels {
                search_char: ch,
                matches,
                input: String::new(),
            };
            true
        }
    }

    /// Phase 2: user typed a label char. Returns `Some(row_index)` if resolved.
    pub fn jump_mode_label_input(&mut self, ch: char) -> Option<usize> {
        let (search_char, matches, mut input) = match std::mem::replace(&mut self.jump_phase, JumpPhase::Off) {
            JumpPhase::ShowingLabels { search_char, matches, input } => (search_char, matches, input),
            other => { self.jump_phase = other; return None; }
        };

        input.push(ch);
        let label_len = self.label_len_for_count(matches.len());

        if input.len() >= label_len {
            let found = matches.iter().find(|(_, _, label)| *label == input);
            if let Some((row_idx, _, _)) = found {
                self.set_selected(*row_idx);
                return Some(*row_idx);
            }
            None
        } else {
            let still_valid: Vec<_> = matches.iter()
                .filter(|(_, _, label)| label.starts_with(&input))
                .collect();
            if still_valid.is_empty() {
                None
            } else if still_valid.len() == 1 {
                let row_idx = still_valid[0].0;
                self.set_selected(row_idx);
                Some(row_idx)
            } else {
                self.jump_phase = JumpPhase::ShowingLabels { search_char, matches, input };
                None
            }
        }
    }

    fn label_len_for_count(&self, count: usize) -> usize {
        let base = self.nav_chars.len();
        if base == 0 { return 0; }
        if count <= base { 1 } else { 2 }
    }

    /// Generate the label for the nth match out of `total` matches.
    fn nth_label_of(&self, n: usize, total: usize) -> String {
        let base = self.nav_chars.len();
        if base == 0 { return String::new(); }
        let len = self.label_len_for_count(total);
        if len == 1 {
            if n < base { self.nav_chars[n].to_string() } else { String::new() }
        } else {
            let first = n / base;
            let second = n % base;
            if first < base {
                format!("{}{}", self.nav_chars[first], self.nav_chars[second])
            } else {
                String::new()
            }
        }
    }

    /// Set fixed header rows (always visible at top).
    pub fn with_fixed_headers(mut self, rows: Vec<TableWidgetRow>) -> Self {
        self.fixed_header_rows = rows;
        self
    }

    /// Set fixed footer rows (always visible at bottom).
    pub fn with_fixed_footers(mut self, rows: Vec<TableWidgetRow>) -> Self {
        self.fixed_footer_rows = rows;
        self
    }

    pub fn with_rows(mut self, rows: Vec<TableWidgetRow>) -> Self {
        self.rows = rows;
        self.clamp_selection();
        self
    }

    pub fn with_separator(mut self, sep: impl Into<String>) -> Self {
        self.separator = sep.into();
        self
    }

    pub fn with_col_styles(mut self, styles: ColumnStyles) -> Self {
        self.col_styles = styles;
        self
    }

    pub fn with_style_map(mut self, map: StyleMap) -> Self {
        self.style_map = map;
        self
    }

    pub fn with_style(mut self, style: TableStyle) -> Self {
        self.style = style;
        self
    }

    pub fn with_keymap(mut self, keymap: TableKeymap) -> Self {
        self.keymap = keymap;
        self
    }

    /// Enable smooth (line-wise) scrolling. See [`super::smooth`].
    pub fn with_smooth_scroll(mut self, enabled: bool) -> Self {
        self.smooth_scroll = enabled;
        self
    }

    /// Replace scrollable data rows. Preserves selection if possible.
    /// Resets horizontal scroll — column widths may change with new data,
    /// so the cached offset is no longer meaningful; `view()` will
    /// re-snap to the selected column on next render.
    pub fn set_rows(&mut self, rows: Vec<TableWidgetRow>) {
        self.rows = rows;
        if self.smooth_scroll {
            // Preserve the line-wise scroll position across the rebuild,
            // re-clamping it to the new content and re-deriving the anchor.
            self.resync_smooth(self.last_line_budget);
        } else {
            self.clamp_selection();
        }
        self.clamp_column();
        self.scroll_col_offset = 0;
    }

    /// Replace fixed header rows.
    pub fn set_fixed_headers(&mut self, rows: Vec<TableWidgetRow>) {
        self.fixed_header_rows = rows;
    }

    /// Replace fixed footer rows.
    pub fn set_fixed_footers(&mut self, rows: Vec<TableWidgetRow>) {
        self.fixed_footer_rows = rows;
    }

    /// Replace separator string.
    pub fn set_separator(&mut self, sep: impl Into<String>) {
        self.separator = sep.into();
    }

    /// Replace column styles.
    pub fn set_col_styles(&mut self, styles: ColumnStyles) {
        self.col_styles = styles;
    }

    /// Replace the style map.
    pub fn set_style_map(&mut self, map: StyleMap) {
        self.style_map = map;
    }

    /// Replace the table style.
    pub fn set_table_style(&mut self, style: TableStyle) {
        self.style = style;
    }

    /// Enable / disable smooth (line-wise) scrolling at runtime. Toggling
    /// it off resets the sub-line clip so the table snaps back to flush
    /// row-aligned scrolling. See [`super::smooth`] for the mode's
    /// behaviour.
    pub fn set_smooth_scroll(&mut self, enabled: bool) {
        if self.smooth_scroll != enabled {
            self.smooth_scroll = enabled;
            self.scroll_sub_line = 0;
        }
    }

    /// Get the currently selected data row index.
    pub fn selected_row(&self) -> usize {
        self.selected_row
    }

    /// Programmatically set the selected row (e.g. after a reload).
    /// Snaps to nearest selectable row.
    ///
    /// In smooth mode the selection is sticky (attached to a row, not a
    /// screen position), so this scrolls the target *minimally* into view
    /// rather than forcing it to the top.
    pub fn set_selected(&mut self, row: usize) {
        self.selected_row = row;
        self.clamp_selection();
        if self.smooth_scroll {
            self.scroll_selection_into_view();
        } else {
            self.adjust_scroll();
        }
    }

    /// Restore the selection to `row` after a data rebuild **without** moving
    /// the viewport in smooth mode. The scroll position is authoritative
    /// there and independent of the selection, so re-selecting the same
    /// (possibly edge-clipped) row on every frame's rebuild must not pull it
    /// back into full view — that would fight line-wise scrolling. The
    /// selection is only nudged if its row ended up fully off-screen (e.g.
    /// the data set changed). Discrete mode keeps the classic
    /// scroll-into-view via [`adjust_scroll`](Self::adjust_scroll).
    pub fn restore_selected(&mut self, row: usize) {
        self.selected_row = row;
        self.clamp_selection();
        if self.smooth_scroll {
            self.reattach_selection_if_offscreen(self.last_line_budget);
        } else {
            self.adjust_scroll();
        }
    }

    /// Get the currently selected column. `None` = column cursor disabled.
    pub fn selected_column(&self) -> Option<usize> {
        self.selected_column
    }

    /// Enable / disable the optional column cursor and / or move it
    /// programmatically. `None` disables it, hiding the column highlight
    /// and making column-nav methods no-ops. The index is clamped to the
    /// current row's column count on every render via [`clamp_column`].
    /// Disabling the cursor also resets the horizontal scroll offset.
    pub fn set_selected_column(&mut self, col: Option<usize>) {
        self.selected_column = col;
        self.clamp_column();
        if col.is_none() {
            self.scroll_col_offset = 0;
        }
    }

    /// Move the column cursor one cell to the left. No-op when the
    /// cursor is disabled or already at column 0.
    /// Width (terminal columns) of the area this table last rendered into,
    /// or 0 before the first paint. The content view fits its columns to
    /// this width on rebuild — see the field docs.
    pub fn last_render_width(&self) -> u16 {
        self.last_render_width
    }

    pub fn move_column_left(&mut self) {
        if let Some(c) = self.selected_column {
            if c > 0 {
                self.selected_column = Some(c - 1);
            }
        }
    }

    /// Move the column cursor one cell to the right, capped at the
    /// last cell of the currently selected row. No-op when the cursor
    /// is disabled.
    pub fn move_column_right(&mut self) {
        let Some(c) = self.selected_column else { return; };
        let Some(max) = self.column_count().checked_sub(1) else { return; };
        if c < max {
            self.selected_column = Some(c + 1);
        }
    }

    /// Number of cells in the currently selected data row, or 0 if no
    /// selectable row exists. Used to clamp the column cursor.
    fn column_count(&self) -> usize {
        self.rows
            .get(self.selected_row)
            .map(|r| r.primary_line().len())
            .unwrap_or(0)
    }

    /// Clamp `selected_column` to the current column count. Called
    /// after `set_rows` / `set_data` so a shrinking column count never
    /// leaves the cursor pointing past the end.
    fn clamp_column(&mut self) {
        let Some(c) = self.selected_column else { return; };
        let count = self.column_count();
        if count == 0 {
            return;
        }
        if c >= count {
            self.selected_column = Some(count - 1);
        }
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// Number of data rows visible in the last render.
    pub fn visible_rows(&self) -> usize {
        self.last_visible_data_rows
    }

    /// Set the visible data rows hint (used to preserve scroll state across rebuilds).
    pub fn set_visible_rows(&mut self, n: usize) {
        self.last_visible_data_rows = n;
    }

    /// Total number of data rows (excluding fixed header/footer).
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Number of whole data rows that fit in `line_budget` terminal lines
    /// starting at row `start`, honouring each row's physical height. For
    /// single-line rows this equals `min(line_budget, rows - start)`, so the
    /// classic behaviour is unchanged. At least one row is reported when the
    /// budget is non-zero (a row taller than the pane renders clipped rather
    /// than vanishing).
    fn visible_row_count_from(&self, start: usize, line_budget: usize) -> usize {
        if line_budget == 0 {
            return 0;
        }
        let mut used = 0usize;
        let mut count = 0usize;
        for row in self.rows.iter().skip(start) {
            let h = row.height();
            if used + h > line_budget {
                break;
            }
            used += h;
            count += 1;
        }
        if count == 0 && start < self.rows.len() {
            1
        } else {
            count
        }
    }

    // --- internal ---

    fn move_up(&mut self) {
        if self.rows.is_empty() { return; }
        if self.smooth_scroll {
            // Scroll one physical line; if the viewport is already clamped
            // (everything fits, or at the top edge) the virtual cursor steps
            // to the previous message instead of standing still.
            if !self.scroll_lines(-1) {
                self.step_selection_up();
            }
            return;
        }
        let mut target = self.selected_row;
        loop {
            if target == 0 { break; }
            target -= 1;
            if self.rows[target].selectable {
                self.selected_row = target;
                break;
            }
        }
        self.adjust_scroll();
    }

    /// Move selection by `n` rows (positive = down, negative = up).
    /// Skips non-selectable rows.
    pub fn scroll_by(&mut self, n: isize) {
        if self.rows.is_empty() { return; }
        let max = self.rows.len() - 1;
        let target = if n >= 0 {
            (self.selected_row as isize + n).min(max as isize) as usize
        } else {
            (self.selected_row as isize + n).max(0) as usize
        };
        self.selected_row = target;
        self.clamp_selection();
        self.adjust_scroll();
    }

    /// Scroll by half a viewport towards (`down`) or away from the end.
    /// Smooth mode steps by physical lines; discrete mode by whole rows.
    pub fn scroll_half_page(&mut self, down: bool) {
        self.page_scroll(down, true);
    }

    /// Scroll by a full viewport. Unit follows the scroll mode (lines vs
    /// rows), exactly like [`scroll_half_page`](Self::scroll_half_page).
    pub fn scroll_full_page(&mut self, down: bool) {
        self.page_scroll(down, false);
    }

    /// Shared paging primitive: chooses the step unit (physical lines in
    /// smooth mode, whole rows otherwise) and the magnitude (half or full
    /// viewport), then delegates to the matching scroll path.
    fn page_scroll(&mut self, down: bool, half: bool) {
        if self.smooth_scroll {
            let budget = self.last_line_budget.max(1);
            let step = if half { (budget / 2).max(1) } else { budget } as isize;
            self.scroll_lines(if down { step } else { -step });
        } else {
            let visible = self.last_visible_data_rows.max(1);
            let step = if half { (visible / 2).max(1) } else { visible } as isize;
            self.scroll_by(if down { step } else { -step });
        }
    }

    fn move_first(&mut self) {
        if self.smooth_scroll {
            self.scroll_to_start();
            return;
        }
        self.selected_row = 0;
        self.clamp_selection();
        self.adjust_scroll();
    }

    fn move_last(&mut self) {
        if self.rows.is_empty() { return; }
        if self.smooth_scroll {
            self.scroll_to_end();
            return;
        }
        self.selected_row = self.rows.len() - 1;
        self.clamp_selection();
        self.adjust_scroll();
    }

    fn move_down(&mut self) {
        if self.rows.is_empty() { return; }
        if self.smooth_scroll {
            // Scroll one physical line; if the viewport is already clamped
            // (everything fits, or at the bottom edge) the virtual cursor
            // steps to the next message instead of standing still.
            if !self.scroll_lines(1) {
                self.step_selection_down();
            }
            return;
        }
        let max = self.rows.len() - 1;
        let mut target = self.selected_row;
        loop {
            if target >= max { break; }
            target += 1;
            if self.rows[target].selectable {
                self.selected_row = target;
                break;
            }
        }
        self.adjust_scroll();
    }

    /// Compute the rendered width of each logical column across header,
    /// data, and footer rows. The widget assumes pre-fitted text
    /// (`compute_table` produces uniform widths) but tolerates ragged rows
    /// by taking the max char count per column. Cells with `col_span > 1`
    /// span multiple logical columns; their width is attributed to the
    /// starting column only (good enough for the auto-scroll heuristic).
    pub(crate) fn compute_col_widths(&self) -> Vec<usize> {
        let mut widths: Vec<usize> = Vec::new();
        let mut consider = |row: &TableWidgetRow| {
            let mut col = 0usize;
            for cell in row.primary_line() {
                let span = cell.col_span.max(1);
                let w = cell.text.chars().count();
                if widths.len() <= col {
                    widths.resize(col + 1, 0);
                }
                if w > widths[col] {
                    widths[col] = w;
                }
                col += span;
            }
        };
        for row in &self.fixed_header_rows { consider(row); }
        for row in &self.rows { consider(row); }
        for row in &self.fixed_footer_rows { consider(row); }
        widths
    }

    /// Visible right-edge x-coordinate (relative to area.left()) of the
    /// `target` column, given a leading scroll of `s` columns.
    fn visible_right_of(s: usize, target: usize, widths: &[usize], sep: usize) -> usize {
        if target < s { return 0; }
        let mut total = 0usize;
        for i in s..=target {
            if i > s { total += sep; }
            total += widths.get(i).copied().unwrap_or(0);
        }
        total
    }

    /// Re-snap `scroll_col_offset` so the column cursor stays fully
    /// visible inside `area_width`. No-op when the column cursor is
    /// disabled. Updates `has_more_right` for the indicator overlay.
    /// Snap granularity is one whole column.
    pub(crate) fn adjust_horizontal_scroll(&mut self, area_width: u16) {
        let Some(target) = self.selected_column else {
            self.scroll_col_offset = 0;
            self.has_more_right = false;
            return;
        };
        let widths = self.compute_col_widths();
        if widths.is_empty() {
            self.scroll_col_offset = 0;
            self.has_more_right = false;
            return;
        }
        let sep = self.separator.chars().count();
        let area_w = area_width as usize;
        let target = target.min(widths.len() - 1);

        // Scroll left to keep target in view, scroll right until target's
        // right edge fits inside the viewport.
        let mut s = self.scroll_col_offset.min(target);
        while s < target {
            let r = Self::visible_right_of(s, target, &widths, sep);
            if r <= area_w { break; }
            s += 1;
        }
        self.scroll_col_offset = s;

        // Anything still visible to the right of `target`?
        let last = widths.len() - 1;
        self.has_more_right = if last > target {
            Self::visible_right_of(s, last, &widths, sep) > area_w
        } else {
            false
        };
    }

    fn adjust_scroll(&mut self) {
        // When scrolling up, also show any preceding non-selectable rows
        // (group headers) that belong to the selected row.
        let mut target = self.selected_row;
        while target > 0 && !self.rows[target - 1].selectable {
            target -= 1;
        }
        if target < self.scroll_offset {
            self.scroll_offset = target;
        }
        let visible = self.last_visible_data_rows;
        if visible > 0 && self.selected_row >= self.scroll_offset + visible {
            self.scroll_offset = self.selected_row + 1 - visible;
        }
    }

    fn clamp_selection(&mut self) {
        if self.rows.is_empty() {
            self.selected_row = 0;
            self.scroll_offset = 0;
            return;
        }
        if self.selected_row >= self.rows.len() {
            self.selected_row = self.rows.len() - 1;
        }
        if !self.rows[self.selected_row].selectable {
            let mut found = false;
            for i in self.selected_row..self.rows.len() {
                if self.rows[i].selectable {
                    self.selected_row = i;
                    found = true;
                    break;
                }
            }
            if !found {
                for i in (0..self.selected_row).rev() {
                    if self.rows[i].selectable {
                        self.selected_row = i;
                        break;
                    }
                }
            }
        }
    }
}

impl Component for Table {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let fixed_top = self.fixed_header_rows.len() as u16;
        let fixed_bottom = self.fixed_footer_rows.len() as u16;
        // Budget is in terminal LINES; with variable row heights the number
        // of whole rows that fit depends on the scroll position, so it's
        // derived below rather than being a constant.
        let line_budget = area.height
            .saturating_sub(fixed_top)
            .saturating_sub(fixed_bottom) as usize;
        // Remember the line budget for event handlers (smooth scrolling
        // runs outside view() and needs it to clamp / page by lines).
        self.last_line_budget = line_budget;

        if self.smooth_scroll {
            // Selection is anchor-derived: just re-clamp the continuous
            // scroll position to the current data + viewport.
            self.resync_smooth(line_budget);
        } else {
            // Scroll up first so the selected row — and any preceding
            // non-selectable rows (group headers) belonging to it — are
            // visible.
            let mut target = self.selected_row;
            while target > 0 && !self.rows[target - 1].selectable {
                target -= 1;
            }
            if target < self.scroll_offset {
                self.scroll_offset = target;
            }

            // Scroll down until the selected row falls inside the
            // height-aware visible window. For single-line rows this reduces
            // to the old `scroll_offset = selected_row + 1 - visible`
            // arithmetic.
            while self.scroll_offset < self.selected_row
                && self.visible_row_count_from(self.scroll_offset, line_budget)
                    <= self.selected_row - self.scroll_offset
            {
                self.scroll_offset += 1;
            }
        }

        let visible_data = self.visible_row_count_from(self.scroll_offset, line_budget);
        self.last_visible_data_rows = visible_data;
        // Remember the render width so the next column-layout rebuild fits to
        // the real pane width (see `last_render_width` docs).
        self.last_render_width = area.width;

        // Re-snap horizontal scroll to keep the column cursor in view.
        // No-op when the column-cursor feature is off.
        self.adjust_horizontal_scroll(area.width);

        // Extract jump state for rendering.
        let empty_matches: Vec<(usize, Vec<usize>, String)> = Vec::new();
        let (jump_matches, jump_showing_labels, jump_input) = match &self.jump_phase {
            JumpPhase::ShowingLabels { matches, input, .. } => {
                // Convert data_row_index to visible_index for the renderer.
                let visible: Vec<(usize, Vec<usize>, String)> = matches.iter()
                    .filter_map(|(row_idx, positions, label)| {
                        let vi = row_idx.checked_sub(self.scroll_offset)?;
                        if vi < visible_data { Some((vi, positions.clone(), label.clone())) } else { None }
                    })
                    .collect();
                (visible, true, input.as_str())
            }
            _ => (empty_matches, false, ""),
        };

        // Number of leading characters hidden by horizontal scroll —
        // used to shift jump-mode label positions into the visible area.
        let scrolled_chars = if self.scroll_col_offset > 0 {
            let widths = self.compute_col_widths();
            let sep = self.separator.chars().count();
            let n = self.scroll_col_offset.min(widths.len());
            widths.iter().take(n).sum::<usize>() + n * sep
        } else {
            0
        };

        let data = RenderData {
            fixed_header_rows: &self.fixed_header_rows,
            rows: &self.rows,
            fixed_footer_rows: &self.fixed_footer_rows,
            selected_row: self.selected_row,
            selected_column: self.selected_column,
            scroll_offset: self.scroll_offset,
            scroll_sub_line: self.scroll_sub_line,
            scroll_col_offset: self.scroll_col_offset,
            scrolled_chars,
            has_more_right: self.has_more_right,
            separator: &self.separator,
            col_styles: &self.col_styles,
            style_map: &self.style_map,
            style: &self.style,
            focused: self.focused,
            jump_matches: &jump_matches,
            jump_showing_labels,
            jump_input,
        };
        render(frame.buffer_mut(), area, &data);
    }

    fn query(&self, attr: Attribute) -> Option<QueryResult<'_>> {
        match attr {
            Attribute::Focus => Some(QueryResult::Owned(AttrValue::Flag(self.focused))),
            _ => None,
        }
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        if let Attribute::Focus = attr {
            if let AttrValue::Flag(f) = value {
                self.focused = f;
            }
        }
    }

    fn state(&self) -> State {
        State::Single(StateValue::Usize(self.selected_row))
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        match cmd {
            Cmd::Move(Direction::Up) => {
                self.move_up();
                CmdResult::Changed(State::Single(StateValue::Usize(self.selected_row)))
            }
            Cmd::Move(Direction::Down) => {
                self.move_down();
                CmdResult::Changed(State::Single(StateValue::Usize(self.selected_row)))
            }
            Cmd::GoTo(Position::Begin) => {
                self.move_first();
                CmdResult::Changed(State::Single(StateValue::Usize(self.selected_row)))
            }
            Cmd::GoTo(Position::End) => {
                self.move_last();
                CmdResult::Changed(State::Single(StateValue::Usize(self.selected_row)))
            }
            Cmd::Submit => {
                CmdResult::Submit(State::Single(StateValue::Usize(self.selected_row)))
            }
            Cmd::Cancel => {
                CmdResult::Batch(vec![])
            }
            _ => CmdResult::NoChange,
        }
    }
}

impl AppComponent<TableEvent, NoUserEvent> for Table {
    fn on(&mut self, ev: &Event<NoUserEvent>) -> Option<TableEvent> {
        let Event::Keyboard(key_ev) = ev else {
            return None;
        };
        let key_ev = *key_ev;

        // Scroll commands are handled directly (not via Cmd). The step
        // unit (physical lines vs whole rows) is chosen inside the scroll
        // helpers based on the smooth-scroll mode.
        if self.keymap.half_page_up.matches(&key_ev) {
            self.scroll_half_page(false);
            return Some(TableEvent::CursorChanged(self.selected_row));
        }
        if self.keymap.half_page_down.matches(&key_ev) {
            self.scroll_half_page(true);
            return Some(TableEvent::CursorChanged(self.selected_row));
        }
        if self.keymap.page_up.matches(&key_ev) {
            self.scroll_full_page(false);
            return Some(TableEvent::CursorChanged(self.selected_row));
        }
        if self.keymap.page_down.matches(&key_ev) {
            self.scroll_full_page(true);
            return Some(TableEvent::CursorChanged(self.selected_row));
        }

        let cmd = if self.keymap.move_up.matches(&key_ev) {
            Cmd::Move(Direction::Up)
        } else if self.keymap.move_down.matches(&key_ev) {
            Cmd::Move(Direction::Down)
        } else if self.keymap.move_first.matches(&key_ev) {
            Cmd::GoTo(Position::Begin)
        } else if self.keymap.move_last.matches(&key_ev) {
            Cmd::GoTo(Position::End)
        } else if self.keymap.confirm.matches(&key_ev) {
            Cmd::Submit
        } else if self.keymap.cancel.matches(&key_ev) {
            Cmd::Cancel
        } else {
            return None;
        };

        match self.perform(cmd) {
            CmdResult::Changed(State::Single(StateValue::Usize(idx))) => {
                Some(TableEvent::CursorChanged(idx))
            }
            CmdResult::Submit(State::Single(StateValue::Usize(idx))) => {
                Some(TableEvent::Confirmed(idx))
            }
            CmdResult::Batch(_) => Some(TableEvent::Cancelled),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::TableWidgetCell;

    fn make_rows() -> Vec<TableWidgetRow> {
        vec![
            TableWidgetRow::new(vec![TableWidgetCell::plain("A")]),
            TableWidgetRow::new(vec![TableWidgetCell::plain("B")]).not_selectable(),
            TableWidgetRow::new(vec![TableWidgetCell::plain("C")]),
            TableWidgetRow::new(vec![TableWidgetCell::plain("D")]),
        ]
    }

    #[test]
    fn move_down_skips_non_selectable() {
        let mut table = Table::default().with_rows(make_rows());
        table.focused = true;
        assert_eq!(table.selected_row, 0);

        table.move_down();
        assert_eq!(table.selected_row, 2);
    }

    #[test]
    fn move_up_skips_non_selectable() {
        let mut table = Table::default().with_rows(make_rows());
        table.focused = true;
        table.selected_row = 2;

        table.move_up();
        assert_eq!(table.selected_row, 0);
    }

    #[test]
    fn clamp_to_selectable_on_set_data() {
        let mut table = Table::default();
        table.selected_row = 1;
        table.set_rows(make_rows());
        assert_eq!(table.selected_row, 2);
    }

    #[test]
    fn scroll_adjusts_on_move_down() {
        let mut table = Table::default().with_rows(
            (0..20).map(|i| TableWidgetRow::new(vec![TableWidgetCell::plain(format!("Row {i}"))]))
                .collect()
        );
        table.last_visible_data_rows = 5;
        table.focused = true;

        // Move to row 6 — should scroll.
        for _ in 0..6 {
            table.move_down();
        }
        assert_eq!(table.selected_row, 6);
        assert_eq!(table.scroll_offset, 2); // 6 + 1 - 5 = 2
    }

    fn make_wide_rows() -> Vec<TableWidgetRow> {
        // 5 columns, 10 chars each, separator "  " (2) → row width 58.
        (0..3).map(|r| {
            TableWidgetRow::new((0..5).map(|c| {
                TableWidgetCell::plain(format!("r{r}c{c}aaaaaa"))
            }).collect())
        }).collect()
    }

    #[test]
    fn h_scroll_noop_without_column_cursor() {
        let mut t = Table::default().with_rows(make_wide_rows());
        t.adjust_horizontal_scroll(20);
        assert_eq!(t.scroll_col_offset, 0);
        assert!(!t.has_more_right);
    }

    #[test]
    fn h_scroll_advances_to_keep_target_visible() {
        let mut t = Table::default().with_rows(make_wide_rows());
        t.set_selected_column(Some(3)); // last fully visible col
        // Area too narrow for cols 0..3 (10+2+10+2+10+2+10 = 46) → must scroll.
        t.adjust_horizontal_scroll(20);
        assert!(t.scroll_col_offset > 0);
        // After scroll, target column 3 must fit: width(3) + sep + width(...) ≤ 20.
        let widths = t.compute_col_widths();
        let visible = Table::visible_right_of(t.scroll_col_offset, 3, &widths, 2);
        assert!(visible <= 20, "target right={visible} exceeds width 20");
    }

    #[test]
    fn h_scroll_reset_on_column_cursor_off() {
        let mut t = Table::default().with_rows(make_wide_rows());
        t.set_selected_column(Some(4));
        t.adjust_horizontal_scroll(15);
        assert!(t.scroll_col_offset > 0);
        t.set_selected_column(None);
        assert_eq!(t.scroll_col_offset, 0);
    }

    #[test]
    fn h_scroll_reset_on_set_rows() {
        let mut t = Table::default().with_rows(make_wide_rows());
        t.set_selected_column(Some(4));
        t.adjust_horizontal_scroll(15);
        assert!(t.scroll_col_offset > 0);
        t.set_rows(make_wide_rows());
        assert_eq!(t.scroll_col_offset, 0);
    }

    #[test]
    fn h_scroll_left_when_target_before_offset() {
        let mut t = Table::default().with_rows(make_wide_rows());
        t.set_selected_column(Some(4));
        t.adjust_horizontal_scroll(15);
        let scrolled_right = t.scroll_col_offset;
        assert!(scrolled_right > 0);
        // Now move cursor to col 0 — should scroll back fully left.
        t.set_selected_column(Some(0));
        t.adjust_horizontal_scroll(15);
        assert_eq!(t.scroll_col_offset, 0);
    }

    #[test]
    fn h_scroll_has_more_right_flag() {
        let mut t = Table::default().with_rows(make_wide_rows());
        t.set_selected_column(Some(0));
        t.adjust_horizontal_scroll(15);
        // With col 0 in view and width 15, cols 1..4 are still right-hidden.
        assert!(t.has_more_right);
        // Wide enough for everything → no indicator.
        t.adjust_horizontal_scroll(200);
        assert!(!t.has_more_right);
    }

    #[test]
    fn set_selected_adjusts_scroll() {
        let mut table = Table::default().with_rows(
            (0..20).map(|i| TableWidgetRow::new(vec![TableWidgetCell::plain(format!("Row {i}"))]))
                .collect()
        );
        table.last_visible_data_rows = 5;

        table.set_selected(15);
        assert_eq!(table.selected_row, 15);
        assert_eq!(table.scroll_offset, 11); // 15 + 1 - 5 = 11
    }

    // --- multi-line rows ---

    fn chat_row(text: &str) -> TableWidgetRow {
        // 3 physical lines: meta, body, spacer (spacer not highlighted).
        TableWidgetRow::multiline(vec![
            super::super::TableWidgetLine::new(vec![TableWidgetCell::plain(format!("meta {text}"))]),
            super::super::TableWidgetLine::new(vec![TableWidgetCell::plain(text)]),
            super::super::TableWidgetLine::new(vec![]).with_highlight_on_select(false),
        ])
    }

    #[test]
    fn single_line_row_height_is_one() {
        let row = TableWidgetRow::new(vec![TableWidgetCell::plain("x")]);
        assert_eq!(row.height(), 1);
        assert_eq!(row.primary_line().len(), 1);
    }

    #[test]
    fn multiline_row_height_and_primary_line() {
        let row = chat_row("hi");
        assert_eq!(row.height(), 3);
        // Jump/column-cursor see only the primary (first) line.
        assert_eq!(row.primary_line().len(), 1);
        assert_eq!(row.primary_line()[0].text, "meta hi");
        // The spacer line opts out of selection styling.
        assert!(!row.lines[2].highlight_on_select);
    }

    #[test]
    fn visible_row_count_honours_height() {
        // Three 3-line rows, budget 7 lines → only 2 whole rows fit (6 lines).
        let table = Table::default().with_rows(
            (0..3).map(|i| chat_row(&format!("m{i}"))).collect(),
        );
        assert_eq!(table.visible_row_count_from(0, 7), 2);
        assert_eq!(table.visible_row_count_from(0, 9), 3);
        // A budget smaller than one row still shows that row (clipped).
        assert_eq!(table.visible_row_count_from(0, 2), 1);
        assert_eq!(table.visible_row_count_from(0, 0), 0);
    }
}
