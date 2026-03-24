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
            Tab::Welcome   => "Welcome",
            Tab::Tasks     => "Tasks",
            Tab::Trackings => "Trackings",
        }
    }

    #[allow(dead_code)]
    pub fn index(&self) -> usize {
        match self {
            Tab::Welcome   => 0,
            Tab::Tasks     => 1,
            Tab::Trackings => 2,
        }
    }

    pub fn next(&self) -> Tab {
        match self {
            Tab::Welcome   => Tab::Tasks,
            Tab::Tasks     => Tab::Trackings,
            Tab::Trackings => Tab::Welcome,
        }
    }

    pub fn prev(&self) -> Tab {
        match self {
            Tab::Welcome   => Tab::Trackings,
            Tab::Tasks     => Tab::Welcome,
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
            TasksForm::Add    => "Add",
            TasksForm::Delete => "Delete",
        }
    }
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
}

impl TasksState {
    pub fn new() -> Self {
        Self {
            active_view: TasksView::List,
            active_form: None,
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
}

impl Default for TasksState {
    fn default() -> Self { Self::new() }
}
