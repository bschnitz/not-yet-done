// ---------------------------------------------------------------------------
// Main tabs
// ---------------------------------------------------------------------------

use crate::ui::tasks::forest::{find_task_in_forest, LocalUuid, TaskForest, TaskQuery};
use not_yet_done_forest::{
    ColSizerEnum, ColStrategy, ColumnId, IntoRow, MixedColSizer, RenderableTree, Row, TableLayout,
    TableRow, render_table, TREE_COLUMN,
};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Welcome,
    Tasks,
    Trackings,
}

impl Tab {
    pub const ALL: &'static [Tab] = &[Tab::Welcome, Tab::Tasks, Tab::Trackings];

    pub fn title(&self) -> &'static str {
        match self {
            Tab::Welcome => "Welcome",
            Tab::Tasks => "Tasks",
            Tab::Trackings => "Trackings",
        }
    }

    #[allow(dead_code)]
    pub fn index(&self) -> usize {
        match self {
            Tab::Welcome => 0,
            Tab::Tasks => 1,
            Tab::Trackings => 2,
        }
    }

    pub fn next(&self) -> Tab {
        match self {
            Tab::Welcome => Tab::Tasks,
            Tab::Tasks => Tab::Trackings,
            Tab::Trackings => Tab::Welcome,
        }
    }

    pub fn prev(&self) -> Tab {
        match self {
            Tab::Welcome => Tab::Trackings,
            Tab::Tasks => Tab::Welcome,
            Tab::Trackings => Tab::Tasks,
        }
    }
}

// ---------------------------------------------------------------------------
// Tasks sub-tabs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TasksView {
    List,
    Tree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TasksForm {
    Filter,
    Add,
    Delete,
}

// ---------------------------------------------------------------------------
// FilterField
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterField {
    CreatedAfter,
    CreatedBefore,
    Description,
    Status,
    Priority,
    ShowDeleted,
}

impl Default for FilterField {
    fn default() -> Self {
        FilterField::CreatedAfter
    }
}

impl FilterField {
    pub const ALL: &'static [FilterField] = &[
        FilterField::CreatedAfter,
        FilterField::CreatedBefore,
        FilterField::Description,
        FilterField::Status,
        FilterField::Priority,
        FilterField::ShowDeleted,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            FilterField::CreatedAfter => "Created after",
            FilterField::CreatedBefore => "Created before",
            FilterField::Description => "Description",
            FilterField::Status => "Status",
            FilterField::Priority => "Priority ≥",
            FilterField::ShowDeleted => "Include deleted",
        }
    }

    pub fn next(&self) -> FilterField {
        let idx = Self::ALL.iter().position(|f| f == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    pub fn prev(&self) -> FilterField {
        let idx = Self::ALL.iter().position(|f| f == self).unwrap_or(0);
        Self::ALL[(idx + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

// ---------------------------------------------------------------------------
// StatusFilter
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct StatusFilter {
    pub todo: bool,
    pub in_progress: bool,
    pub done: bool,
    pub cancelled: bool,
}

impl StatusFilter {
    pub fn is_empty(&self) -> bool {
        !self.todo && !self.in_progress && !self.done && !self.cancelled
    }
}

// ---------------------------------------------------------------------------
// FilterState
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct FilterState {
    pub created_after_raw: String,
    pub created_before_raw: String,
    pub description_like: String,
    pub status: StatusFilter,
    pub priority_min_raw: String,
    pub show_deleted: bool,

    pub created_after_err: Option<String>,
    pub created_before_err: Option<String>,
    pub priority_err: Option<String>,

    pub focused_field: FilterField,
    pub cursor_pos: usize,
    pub status_cursor: usize,
}

impl FilterState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn focused_text_mut(&mut self) -> Option<&mut String> {
        match self.focused_field {
            FilterField::CreatedAfter => Some(&mut self.created_after_raw),
            FilterField::CreatedBefore => Some(&mut self.created_before_raw),
            FilterField::Description => Some(&mut self.description_like),
            FilterField::Priority => Some(&mut self.priority_min_raw),
            FilterField::Status | FilterField::ShowDeleted => None,
        }
    }

    pub fn insert_char(&mut self, c: char) {
        let pos = self.cursor_pos;
        if let Some(s) = self.focused_text_mut() {
            let byte_pos = s.char_indices().nth(pos).map(|(i, _)| i).unwrap_or(s.len());
            s.insert(byte_pos, c);
            self.cursor_pos += 1;
        }
    }

    pub fn backspace(&mut self) {
        if self.cursor_pos == 0 {
            return;
        }
        let pos = self.cursor_pos;
        if let Some(s) = self.focused_text_mut() {
            if s.is_empty() {
                return;
            }
            let byte_pos = s.char_indices().nth(pos - 1).map(|(i, _)| i).unwrap_or(0);
            s.remove(byte_pos);
            self.cursor_pos -= 1;
        }
    }

    pub fn cursor_left(&mut self) {
        if self.cursor_pos > 0 {
            self.cursor_pos -= 1;
        }
    }

    pub fn cursor_right(&mut self) {
        let len = self.focused_text_len();
        if self.cursor_pos < len {
            self.cursor_pos += 1;
        }
    }

    fn focused_text_len(&self) -> usize {
        match self.focused_field {
            FilterField::CreatedAfter => self.created_after_raw.chars().count(),
            FilterField::CreatedBefore => self.created_before_raw.chars().count(),
            FilterField::Description => self.description_like.chars().count(),
            FilterField::Priority => self.priority_min_raw.chars().count(),
            FilterField::Status | FilterField::ShowDeleted => 0,
        }
    }

    pub fn focus_next(&mut self) {
        self.focused_field = self.focused_field.next();
        self.clamp_cursor();
    }

    pub fn focus_prev(&mut self) {
        self.focused_field = self.focused_field.prev();
        self.clamp_cursor();
    }

    fn clamp_cursor(&mut self) {
        let max = self.focused_text_len();
        if self.cursor_pos > max {
            self.cursor_pos = max;
        }
    }

    pub fn toggle_status_cursor(&mut self) {
        match self.status_cursor {
            0 => self.status.todo = !self.status.todo,
            1 => self.status.in_progress = !self.status.in_progress,
            2 => self.status.done = !self.status.done,
            3 => self.status.cancelled = !self.status.cancelled,
            _ => {}
        }
    }

    pub fn status_cursor_next(&mut self) {
        self.status_cursor = (self.status_cursor + 1) % 4;
    }

    pub fn status_cursor_prev(&mut self) {
        self.status_cursor = (self.status_cursor + 3) % 4;
    }

    pub fn toggle_show_deleted(&mut self) {
        self.show_deleted = !self.show_deleted;
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

// ---------------------------------------------------------------------------
// LoadState
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadState {
    Idle,
    Loading,
    Loaded,
    Error(String),
}

// ---------------------------------------------------------------------------
// TasksState
// ---------------------------------------------------------------------------

pub struct TasksState {
    pub active_view: TasksView,
    pub active_form: Option<TasksForm>,
    pub filter: FilterState,

    pub tree_filter: String,
    pub tree_filter_cursor: usize,
    pub tree_filter_focused: bool,

    pub task_rows: Vec<not_yet_done_core::entity::task::Model>,
    pub forest: Option<TaskForest>,
    pub selected_row: usize,
    pub scroll_offset: usize,
    pub load_state: LoadState,

    /// Cached rendered table rows (header at index 0, data rows after).
    /// Keyed by `cached_tree_filter` — invalidated when filter or forest changes.
    tree_rows_cache: Option<Vec<TableRow<LocalUuid>>>,
    cached_tree_filter: String,
    /// Width used when the cache was built — invalidated on resize.
    cached_width: usize,
}

impl TasksState {
    pub fn new() -> Self {
        Self {
            active_view: TasksView::List,
            active_form: None,
            filter: FilterState::new(),
            tree_filter: String::new(),
            tree_filter_cursor: 0,
            tree_filter_focused: false,
            task_rows: Vec::new(),
            forest: None,
            selected_row: 0,
            scroll_offset: 0,
            load_state: LoadState::Idle,
            tree_rows_cache: None,
            cached_tree_filter: String::new(),
            cached_width: 0,
        }
    }

    pub fn form_visible(&self) -> bool {
        self.active_form.is_some()
    }

    pub fn open_form(&mut self, form: TasksForm) {
        self.active_form = Some(form);
    }

    pub fn close_form(&mut self) {
        self.active_form = None;
    }

    // ── Tree filter helpers ──────────────────────────────────────────────

    pub fn tree_filter_insert(&mut self, c: char) {
        let pos = self.tree_filter_cursor;
        let byte_pos = self
            .tree_filter
            .char_indices()
            .nth(pos)
            .map(|(i, _)| i)
            .unwrap_or(self.tree_filter.len());
        self.tree_filter.insert(byte_pos, c);
        self.tree_filter_cursor += 1;
        self.tree_rows_cache = None; // invalidate
    }

    pub fn tree_filter_backspace(&mut self) {
        if self.tree_filter_cursor == 0 || self.tree_filter.is_empty() {
            return;
        }
        let pos = self.tree_filter_cursor;
        let byte_pos = self
            .tree_filter
            .char_indices()
            .nth(pos - 1)
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.tree_filter.remove(byte_pos);
        self.tree_filter_cursor -= 1;
        self.tree_rows_cache = None; // invalidate
    }

    pub fn tree_filter_cursor_left(&mut self) {
        if self.tree_filter_cursor > 0 {
            self.tree_filter_cursor -= 1;
        }
    }

    pub fn tree_filter_cursor_right(&mut self) {
        let max = self.tree_filter.chars().count();
        if self.tree_filter_cursor < max {
            self.tree_filter_cursor += 1;
        }
    }

    pub fn tree_filter_clear(&mut self) {
        self.tree_filter.clear();
        self.tree_filter_cursor = 0;
        self.tree_rows_cache = None; // invalidate
    }

    // ── List navigation ──────────────────────────────────────────────────

    pub fn select_next(&mut self, visible_rows: usize) {
        if self.task_rows.is_empty() {
            return;
        }
        let max = self.task_rows.len() - 1;
        if self.selected_row < max {
            self.selected_row += 1;
            if self.selected_row >= self.scroll_offset + visible_rows {
                self.scroll_offset = self.selected_row + 1 - visible_rows;
            }
        }
    }

    pub fn select_prev(&mut self) {
        if self.selected_row > 0 {
            self.selected_row -= 1;
            if self.selected_row < self.scroll_offset {
                self.scroll_offset = self.selected_row;
            }
        }
    }

    pub fn set_tasks(&mut self, tasks: Vec<not_yet_done_core::entity::task::Model>) {
        use crate::ui::tasks::forest::build_forest;
        self.forest = Some(build_forest(tasks.clone()));
        self.task_rows = tasks;

        if !self.task_rows.is_empty() && self.selected_row >= self.task_rows.len() {
            self.selected_row = self.task_rows.len() - 1;
        }
        self.load_state = LoadState::Loaded;
        self.tree_rows_cache = None;
        self.cached_tree_filter.clear();
        self.cached_width = 0;
    }

    pub fn set_load_error(&mut self, msg: String) {
        self.load_state = LoadState::Error(msg);
    }
}

impl Default for TasksState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// build_table_rows — the actual render pipeline
// ---------------------------------------------------------------------------

/// Columns shown in the tree table, in display order.
pub const TREE_TABLE_COLS: &[&str] = &[TREE_COLUMN, "priority", "created_at", "updated_at"];

pub fn build_table_rows(
    forest: &TaskForest,
    query_str: &str,
    area_width: usize,
) -> Vec<TableRow<LocalUuid>> {
    let query = TaskQuery::new(query_str, 20);

    let cols: Vec<ColumnId> = TREE_TABLE_COLS.iter().map(|s| ColumnId::new(*s)).collect();

    // Tree column width: connector baseline + room for descriptions.
    let tree_min = forest.inner().tree_min_width(&query);
    let tree_col_width = (tree_min + 20).max(30).min(area_width * 3 / 5);

    // 1. Tree cells (connector + description, fitted + highlights shifted).
    let tree_rows = RenderableTree::tree_rows::<LocalUuid>(forest, &query, tree_col_width);

    if tree_rows.is_empty() {
        return vec![];
    }

    // 2. Data rows (non-tree columns), same order as tree_rows.
    let data_rows: Vec<Row<LocalUuid>> = tree_rows
        .iter()
        .filter_map(|tr| find_task_in_forest(forest, tr.id.0).map(|item| item.into_row()))
        .collect();

    // 3. Header row.
    let header = {
        let mut cells = std::collections::HashMap::new();
        cells.insert(ColumnId::new(TREE_COLUMN), "Task".to_string());
        cells.insert(ColumnId::new("priority"), "Pri".to_string());
        cells.insert(ColumnId::new("created_at"), "Created".to_string());
        cells.insert(ColumnId::new("updated_at"), "Updated".to_string());
        Row {
            id: LocalUuid(Uuid::nil()),
            cells,
        }
    };

    // 4. Layout.
    let layout = TableLayout {
        max_width: area_width,
        separator: " ".to_string(),
        sizer: ColSizerEnum::Mixed(MixedColSizer {
            strategies: {
                let mut m = std::collections::HashMap::new();
                m.insert(ColumnId::new(TREE_COLUMN), ColStrategy::Fixed(tree_col_width));
                m.insert(ColumnId::new("priority"), ColStrategy::Max);
                m.insert(ColumnId::new("created_at"), ColStrategy::Max);
                m.insert(ColumnId::new("updated_at"), ColStrategy::Max);
                m
            },
        }),
    };

    // 5. Combine everything.
    render_table(tree_rows, data_rows, &layout, &cols, Some(header))
}
