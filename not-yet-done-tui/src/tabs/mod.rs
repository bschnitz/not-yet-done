// ---------------------------------------------------------------------------
// Main tabs
// ---------------------------------------------------------------------------

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

/// The view pane shows task data in one of these modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TasksView {
    List,
    Tree,
}

impl TasksView {
    pub fn title(&self) -> &'static str {
        match self {
            TasksView::List => "List",
            TasksView::Tree => "Tree",
        }
    }
}

/// The form pane shows one of these panels (or nothing if closed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TasksForm {
    Filter,
    Add,
    Delete,
}

impl TasksForm {
    pub fn title(&self) -> &'static str {
        match self {
            TasksForm::Filter => "Filter",
            TasksForm::Add => "Add",
            TasksForm::Delete => "Delete",
        }
    }
}

// ---------------------------------------------------------------------------
// FilterField — which field in the filter form is focused
// ---------------------------------------------------------------------------

/// All fields in the filter form, in tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterField {
    CreatedAfter,
    CreatedBefore,
    Description,
    Status,
    Priority,
    ShowDeleted,
    // Future: Tags, Projects (placeholders — not yet interactive)
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
// StatusFilter — multi-select for task status
// ---------------------------------------------------------------------------

/// Which statuses to include.  An empty set means "all".
#[derive(Debug, Clone, Default)]
pub struct StatusFilter {
    pub todo: bool,
    pub in_progress: bool,
    pub done: bool,
    pub cancelled: bool,
}

impl StatusFilter {
    /// Returns true if no status is explicitly selected (= show all).
    pub fn is_empty(&self) -> bool {
        !self.todo && !self.in_progress && !self.done && !self.cancelled
    }

    /// Cycle the cursor through the four options.
    pub const OPTIONS: &'static [&'static str] = &["todo", "in_progress", "done", "cancelled"];
}

// ---------------------------------------------------------------------------
// FilterState — all user-entered filter values
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct FilterState {
    /// Raw text input for "created after" (parsed on apply).
    pub created_after_raw: String,
    /// Raw text input for "created before" (parsed on apply).
    pub created_before_raw: String,
    /// `description LIKE %value%` — empty means no filter.
    pub description_like: String,
    /// Multi-select status filter.
    pub status: StatusFilter,
    /// Minimum priority (inclusive).  None = no filter.
    pub priority_min_raw: String,
    /// Whether to include soft-deleted tasks.
    pub show_deleted: bool,

    // ── Parse error feedback ─────────────────────────────────────────────
    pub created_after_err: Option<String>,
    pub created_before_err: Option<String>,
    pub priority_err: Option<String>,

    // ── Focus inside the filter form ─────────────────────────────────────
    pub focused_field: FilterField,
    /// Cursor position within the currently focused text field.
    pub cursor_pos: usize,

    // ── Status option cursor (for arrow navigation inside StatusFilter) ──
    /// Which status option is highlighted (0–3).
    pub status_cursor: usize,
}

impl FilterState {
    pub fn new() -> Self {
        Self::default()
    }

    // ── Text field helpers ───────────────────────────────────────────────

    /// Returns a mutable reference to the raw string of the focused text field,
    /// or None if the focused field is not a text field.
    pub fn focused_text_mut(&mut self) -> Option<&mut String> {
        match self.focused_field {
            FilterField::CreatedAfter => Some(&mut self.created_after_raw),
            FilterField::CreatedBefore => Some(&mut self.created_before_raw),
            FilterField::Description => Some(&mut self.description_like),
            FilterField::Priority => Some(&mut self.priority_min_raw),
            FilterField::Status | FilterField::ShowDeleted => None,
        }
    }

    /// Insert a character at the cursor position in the focused text field.
    pub fn insert_char(&mut self, c: char) {
        let pos = self.cursor_pos;
        if let Some(s) = self.focused_text_mut() {
            // byte-safe insert
            let byte_pos = s.char_indices().nth(pos).map(|(i, _)| i).unwrap_or(s.len());
            s.insert(byte_pos, c);
            self.cursor_pos += 1;
        }
    }

    /// Delete the character before the cursor (backspace).
    pub fn backspace(&mut self) {
        if self.cursor_pos == 0 {
            return;
        }
        let pos = self.cursor_pos;
        if let Some(s) = self.focused_text_mut() {
            if s.is_empty() {
                return;
            }
            // find the byte index of the (pos-1)-th char
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

    // ── Field navigation ─────────────────────────────────────────────────

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

    // ── Toggle helpers ────────────────────────────────────────────────────

    /// Toggle the currently highlighted status option (when Status field is focused).
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

    // ── Reset ─────────────────────────────────────────────────────────────

    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

// ---------------------------------------------------------------------------
// LoadState — async task loading
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadState {
    /// Initial state — no load started yet.
    Idle,
    /// A load is in progress.
    Loading,
    /// Last load completed successfully.
    Loaded,
    /// Last load failed with this message.
    Error(String),
}

// ---------------------------------------------------------------------------
// TasksState — owns all mutable state for the Tasks tab
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TasksState {
    /// Which view is currently active in the view pane.
    pub active_view: TasksView,
    /// Which form is currently shown (None = form pane hidden).
    pub active_form: Option<TasksForm>,

    // ── Filter form ──────────────────────────────────────────────────────
    /// All filter inputs.
    pub filter: FilterState,

    // ── View pane ────────────────────────────────────────────────────────
    /// Cached task rows from the last successful load.
    pub task_rows: Vec<not_yet_done_core::entity::task::Model>,
    /// Index of the selected row in the list.
    pub selected_row: usize,
    /// Scroll offset for the task list.
    pub scroll_offset: usize,
    /// Current load state.
    pub load_state: LoadState,
}

impl TasksState {
    pub fn new() -> Self {
        Self {
            active_view: TasksView::List,
            active_form: None,
            filter: FilterState::new(),
            task_rows: Vec::new(),
            selected_row: 0,
            scroll_offset: 0,
            load_state: LoadState::Idle,
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

    // ── List navigation ──────────────────────────────────────────────────

    pub fn select_next(&mut self, visible_rows: usize) {
        if self.task_rows.is_empty() {
            return;
        }
        let max = self.task_rows.len() - 1;
        if self.selected_row < max {
            self.selected_row += 1;
            // scroll down if needed
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
        self.task_rows = tasks;
        // clamp selection
        if !self.task_rows.is_empty() && self.selected_row >= self.task_rows.len() {
            self.selected_row = self.task_rows.len() - 1;
        }
        self.load_state = LoadState::Loaded;
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
