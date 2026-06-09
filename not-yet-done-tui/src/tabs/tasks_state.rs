//! Tasks tab state: view mode, form mode, selection, fuzzy filter, task data.

use not_yet_done_content::SortKey;

use crate::tabs::columns::sort_tasks;
use crate::ui::tasks::forest::{build_forest, TaskForest};
use super::filter_state::FilterState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum TasksForm {
    Filter,
    Add,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadState {
    Idle,
    Loading,
    Loaded,
    Error(String),
}

pub struct TasksState {
    pub active_form: Option<TasksForm>,
    pub filter: FilterState,

    pub task_rows: Vec<not_yet_done_core::entity::task::Model>,
    pub forest: Option<TaskForest>,
    /// Per-task tag list, populated asynchronously after each task
    /// reload. Empty map when no tags are loaded yet — the table
    /// renders blank tag columns in that case.
    pub task_tags:
        std::collections::HashMap<uuid::Uuid, Vec<not_yet_done_core::repository::ResolvedTag>>,
    pub load_state: LoadState,

    /// Sort the user has currently requested. Empty = preserve service
    /// order. Applied in-memory after every load.
    pub current_sort: Vec<SortKey>,
    /// Mirror of `current_sort` recorded at the moment data was last
    /// (re-)sorted. Drives the sort-arrow indicators in the table header.
    pub last_applied_sort: Vec<SortKey>,
}

impl TasksState {
    pub fn new() -> Self {
        Self {
            active_form: None,
            filter: FilterState::new(),
            task_rows: Vec::new(),
            forest: None,
            task_tags: std::collections::HashMap::new(),
            load_state: LoadState::Idle,
            current_sort: Vec::new(),
            last_applied_sort: Vec::new(),
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

    pub fn set_tasks(&mut self, mut tasks: Vec<not_yet_done_core::entity::task::Model>) {
        sort_tasks(&mut tasks, &self.current_sort);
        self.forest = Some(build_forest(tasks.clone()));
        self.task_rows = tasks;
        self.last_applied_sort = self.current_sort.clone();
        self.load_state = LoadState::Loaded;
    }

    /// Replace the active sort and re-sort the in-memory rows.
    /// Returns `true` if the value changed.
    pub fn set_current_sort(&mut self, sort: Vec<SortKey>) -> bool {
        if self.current_sort == sort {
            return false;
        }
        self.current_sort = sort;
        sort_tasks(&mut self.task_rows, &self.current_sort);
        self.forest = Some(build_forest(self.task_rows.clone()));
        self.last_applied_sort = self.current_sort.clone();
        true
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state() {
        let ts = TasksState::new();
        assert!(ts.active_form.is_none());
    }

    #[test]
    fn form_open_close() {
        let mut ts = TasksState::new();
        assert!(!ts.form_visible());
        ts.active_form = Some(TasksForm::Filter);
        assert!(ts.form_visible());
        ts.close_form();
        assert!(!ts.form_visible());
    }
}

