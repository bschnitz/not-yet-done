//! DataTable: unified table component with row-ID mapping and fuzzy filter.
//!
//! Wraps the Table widget from not-yet-done-ratatui. Owns selection state,
//! scroll state, row-to-UUID mapping, pending focus restoration, and fuzzy
//! filter state. Used for both task views and tracking views.

use ratatui::layout::Rect;
use ratatui::Frame;
use uuid::Uuid;

use tuirealm::command::{Cmd, CmdResult, Direction, Position};
use tuirealm::props::{Attribute, AttrValue, QueryResult};
use tuirealm::component::Component;
use tuirealm::state::{State, StateValue};

use not_yet_done_ratatui::{
    Table as TableWidget, TableWidgetRow,
    TableStyle, ColumnStyles, StyleMap,
};

pub struct DataTable {
    table: TableWidget,
    /// Maps each data-row index to a domain UUID.
    row_ids: Vec<Uuid>,
    /// After the next `set_data`, jump to this ID.
    pending_focus_id: Option<Uuid>,

    // ── Fuzzy filter state ───────────────────────────────────────────
    pub fuzzy_active: bool,
    pub fuzzy_query: String,
    pub fuzzy_cursor: usize,
    /// The applied filter string (set on fuzzy_close / fuzzy_insert).
    pub filter_text: String,
}

impl DataTable {
    pub fn new() -> Self {
        Self {
            table: TableWidget::default(),
            row_ids: Vec::new(),
            pending_focus_id: None,
            fuzzy_active: false,
            fuzzy_query: String::new(),
            fuzzy_cursor: 0,
            filter_text: String::new(),
        }
    }

    /// Configure the characters used for quick-jump navigation labels.
    pub fn set_nav_chars(&mut self, chars: &[char]) {
        self.table.set_nav_chars(chars);
    }

    /// Width (terminal columns) of the area this table last rendered into,
    /// or 0 before the first paint. The content view fits its column layout
    /// to this width on rebuild.
    pub fn last_render_width(&self) -> u16 {
        self.table.last_render_width()
    }

    /// Activate jump mode (phase 1 — waiting for search char).
    pub fn jump_mode_open(&mut self) {
        self.table.jump_mode_open();
    }

    /// Cancel jump mode.
    pub fn jump_mode_close(&mut self) {
        self.table.jump_mode_close();
    }

    /// Whether jump mode is in any active phase.
    pub fn jump_active(&self) -> bool {
        self.table.is_jump_active()
    }

    /// Whether jump mode is in the "waiting for search char" phase.
    pub fn jump_waiting_for_char(&self) -> bool {
        self.table.is_jump_waiting_for_char()
    }

    /// Phase 1: search for a character in visible rows.
    pub fn jump_mode_search(&mut self, ch: char) -> bool {
        self.table.jump_mode_search(ch)
    }

    /// Phase 2: feed label input. Returns true if jumped.
    pub fn jump_mode_label_input(&mut self, ch: char) -> bool {
        self.table.jump_mode_label_input(ch).is_some()
    }

    /// Update all table data. Preserves visible_rows and restores selection
    /// by pending_focus_id or previously selected ID.
    pub fn set_data(
        &mut self,
        rows: Vec<TableWidgetRow>,
        row_ids: Vec<Uuid>,
        headers: Vec<TableWidgetRow>,
        footers: Vec<TableWidgetRow>,
        col_styles: ColumnStyles,
        table_style: TableStyle,
        style_map: StyleMap,
        separator: &str,
    ) {
        let restore_id = self.pending_focus_id.take()
            .or_else(|| self.selected_id());
        let prev_idx = self.table.selected_row();

        self.table.set_rows(rows);
        self.table.set_fixed_headers(headers);
        self.table.set_fixed_footers(footers);
        self.table.set_col_styles(col_styles);
        self.table.set_table_style(table_style);
        self.table.set_style_map(style_map);
        self.table.set_separator(separator);
        self.table.set_focused(true);

        self.row_ids = row_ids;

        // Restore selection: by ID first, fallback to previous index.
        // `restore_selected` preserves the scroll position in smooth mode so
        // this per-frame rebuild does not undo line-wise scrolling.
        if let Some(focus_id) = restore_id {
            if let Some(idx) = self.row_ids.iter().position(|&id| id == focus_id) {
                self.table.restore_selected(idx);
            } else {
                self.table.restore_selected(prev_idx);
            }
        } else {
            self.table.restore_selected(prev_idx);
        }
    }

    /// UUID of the currently selected row, if any.
    pub fn selected_id(&self) -> Option<Uuid> {
        self.row_ids.get(self.table.selected_row()).copied()
    }

    /// UUID of the row at a given index, if any.
    pub fn selected_id_at(&self, row: usize) -> Option<Uuid> {
        self.row_ids.get(row).copied()
    }

    /// Currently selected row index.
    pub fn selected_row(&self) -> usize {
        self.table.selected_row()
    }

    /// Set selected row index programmatically.
    pub fn set_selected(&mut self, row: usize) {
        self.table.set_selected(row);
    }

    /// Currently selected column. `None` = column-cursor feature off.
    pub fn selected_column(&self) -> Option<usize> {
        self.table.selected_column()
    }

    /// Enable / disable / move the optional column cursor. See
    /// [`crate::components::data_table::DataTable::move_column_left`] for
    /// the navigation entry points.
    pub fn set_selected_column(&mut self, col: Option<usize>) {
        self.table.set_selected_column(col);
    }

    /// Move the column cursor one cell to the left (no-op if disabled).
    pub fn move_column_left(&mut self) {
        self.table.move_column_left();
    }

    /// Move the column cursor one cell to the right (no-op if disabled).
    pub fn move_column_right(&mut self) {
        self.table.move_column_right();
    }

    /// Number of rendered data rows.
    pub fn row_count(&self) -> usize {
        self.table.row_count()
    }

    /// Number of visible data rows from the last render.
    pub fn visible_rows(&self) -> usize {
        self.table.visible_rows()
    }

    /// Whether the table has no data rows.
    pub fn is_empty(&self) -> bool {
        self.row_ids.is_empty()
    }

    /// Set pending focus: the next `set_data` will jump to this ID.
    pub fn set_pending_focus(&mut self, id: Uuid) {
        self.pending_focus_id = Some(id);
    }

    /// Scroll by n rows (positive = down, negative = up).
    pub fn scroll_by(&mut self, n: isize) {
        self.table.scroll_by(n);
    }

    /// Enable / disable smooth (line-wise) scrolling for this table. Driven
    /// by the active view's `smooth_scroll` config on every rebuild.
    pub fn set_smooth_scroll(&mut self, enabled: bool) {
        self.table.set_smooth_scroll(enabled);
    }

    /// Scroll by half a viewport (`down` = towards the end). The step unit
    /// (physical lines vs whole rows) follows the smooth-scroll mode.
    pub fn scroll_half_page(&mut self, down: bool) {
        self.table.scroll_half_page(down);
    }

    /// Scroll by a full viewport. See [`scroll_half_page`](Self::scroll_half_page).
    pub fn scroll_full_page(&mut self, down: bool) {
        self.table.scroll_full_page(down);
    }

    /// Handle a navigation command. Returns true if handled.
    pub fn handle_nav(&mut self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::Move(Direction::Up) | Cmd::Move(Direction::Down)
            | Cmd::GoTo(Position::Begin) | Cmd::GoTo(Position::End) => {
                self.table.perform(cmd);
                true
            }
            _ => false,
        }
    }

    // ── Fuzzy filter ─────────────────────────────────────────────────

    pub fn fuzzy_open(&mut self) {
        self.fuzzy_active = true;
        self.fuzzy_cursor = self.fuzzy_query.chars().count();
    }

    pub fn fuzzy_close(&mut self) {
        self.fuzzy_active = false;
        self.filter_text = self.fuzzy_query.clone();
    }

    pub fn fuzzy_insert(&mut self, c: char) {
        let byte_pos = self.fuzzy_query.char_indices()
            .nth(self.fuzzy_cursor).map(|(i, _)| i)
            .unwrap_or(self.fuzzy_query.len());
        self.fuzzy_query.insert(byte_pos, c);
        self.fuzzy_cursor += 1;
        self.filter_text = self.fuzzy_query.clone();
    }

    pub fn fuzzy_backspace(&mut self) {
        if self.fuzzy_cursor == 0 || self.fuzzy_query.is_empty() { return; }
        let byte_pos = self.fuzzy_query.char_indices()
            .nth(self.fuzzy_cursor - 1).map(|(i, _)| i)
            .unwrap_or(0);
        self.fuzzy_query.remove(byte_pos);
        self.fuzzy_cursor -= 1;
        self.filter_text = self.fuzzy_query.clone();
    }

    pub fn fuzzy_cursor_left(&mut self) {
        if self.fuzzy_cursor > 0 { self.fuzzy_cursor -= 1; }
    }

    pub fn fuzzy_cursor_right(&mut self) {
        let max = self.fuzzy_query.chars().count();
        if self.fuzzy_cursor < max { self.fuzzy_cursor += 1; }
    }
}

impl Component for DataTable {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        self.table.view(frame, area);
    }

    fn query(&self, attr: Attribute) -> Option<QueryResult<'_>> {
        self.table.query(attr)
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        self.table.attr(attr, value);
    }

    fn state(&self) -> State {
        State::Single(StateValue::Usize(self.table.selected_row()))
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        self.table.perform(cmd)
    }
}
