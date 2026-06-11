//! TasksView — container that switches between TasksListView and TasksTreeView.
//!
//! Owns both sub-views, knows which is active, handles the sub-view switch
//! keybinding (l/t). Everything else is delegated to the active sub-view.

use std::collections::HashSet;
use std::sync::Arc;

use ratatui::layout::Rect;
use ratatui::Frame;
use uuid::Uuid;

use tuirealm::command::{Cmd, CmdResult};
use tuirealm::component::Component;

use crate::app::SavedQuery;
use crate::components::action_bar::ActionBarComponent;
use crate::components::cmdline::CmdlineComponent;
use crate::components::data_table::DataTable;
use crate::components::query_menu::{QueryMenuComponent, QueryMenuEntry, QueryMenuMessage};
use crate::components::search::SearchComponent;
use crate::components::sort_header::HeaderOverlay;
use crate::config::keybindings::{KeyBindingSection, QueryMenuAction};
use crate::config::{CommonAction, TasksAction, KeyBindingConfig};
use crate::query_filter::QueryOptions;
use crate::tabs::TasksSubView;
use crate::ui::tasks::forest::TaskForest;
use crate::ui::theme::Theme;
use crate::views::{
    BarHint, CmdlineKeyResult, CmdlineState, HasCmdline, SearchKeyResult, SearchState,
    Searchable, SortableView, SubViewMessage, ViewRequest,
};

use not_yet_done_content::{SortKey, SortableColumn};
use crate::tabs::columns::task_sortable_columns;
use crate::views::tasks_list_view::TasksListView;
use crate::views::tasks_tree_view::TasksTreeView;

use not_yet_done_core::entity::task::Model as Task;
use not_yet_done_core::filter::FilterExpr;
use not_yet_done_core::service::TaskService;
use crate::tabs::TasksState;

pub struct TasksView {
    list_view: TasksListView,
    tree_view: TasksTreeView,
    sub_view: TasksSubView,
    keybindings: KeyBindingConfig,
    pub state: TasksState,
    pub task_service: Arc<dyn TaskService>,
    pub search: SearchComponent,
    pub cmdline: CmdlineComponent,
    pub query_menu: QueryMenuComponent,
    query_menu_kb: KeyBindingSection<QueryMenuAction>,
    pub action_bar: ActionBarComponent,

    // ── Filter state ──────────────────────────────────────────────────
    pub active_filter: Option<FilterExpr>,
    pub active_filter_options: QueryOptions,
    pub active_filter_json: Option<String>,
    pub active_filter_name: Option<String>,
    pub column_config: Vec<String>,
    pub favorites: Vec<SavedQuery>,
    /// Name of the saved query marked as default (★ in the query
    /// menu). Persisted by the App as a settings row; applied on app
    /// start instead of the last-active filter.
    pub default_query_name: Option<String>,

    /// Visual overlay applied to the column header row — drives the
    /// sort-hint mode (column picker / direction picker). Pushed in by
    /// `App` before each `refresh_from_own_state`.
    pub header_overlay: HeaderOverlay,
}

impl TasksView {
    pub fn new(
        theme: Arc<Theme>,
        keybindings: KeyBindingConfig,
        task_service: Arc<dyn TaskService>,
        tree_default_expand_depth: u32,
    ) -> Self {
        let query_menu = QueryMenuComponent::new(Arc::clone(&theme), "Saved queries")
            .with_popup_kb(keybindings.popup.clone(), keybindings.key_icons.clone());
        let query_menu_kb = keybindings.query_menu.clone();
        let mut action_bar = ActionBarComponent::new(Arc::clone(&theme));
        let fuzzy_label = format!(
            "{} Fuzzy Filter",
            keybindings.common.label(&CommonAction::FuzzyFilterOpen),
        );
        let exit_label = format!(
            "{} accept  {} cancel",
            keybindings.common.label(&CommonAction::FuzzyFilterAccept),
            keybindings.common.label(&CommonAction::FuzzyFilterCancel),
        );
        action_bar.set_fuzzy_label(Some(fuzzy_label), Some(exit_label));
        let mut view = Self {
            list_view: TasksListView::new(Arc::clone(&theme), keybindings.clone()),
            tree_view: TasksTreeView::new(Arc::clone(&theme), keybindings.clone(), tree_default_expand_depth),
            sub_view: TasksSubView::Tree,
            keybindings,
            state: TasksState::new(),
            task_service,
            search: SearchComponent::new(),
            cmdline: CmdlineComponent::new(),
            query_menu,
            query_menu_kb,
            action_bar,
            active_filter: None,
            active_filter_options: Default::default(),
            active_filter_json: None,
            active_filter_name: None,
            column_config: crate::tabs::columns::default_column_ids(),
            favorites: Vec::new(),
            default_query_name: None,
            header_overlay: HeaderOverlay::default(),
        };
        view.action_bar.set_hints(view.bar_hints());
        view
    }

    /// Hints for the action bar (without the leading "Fuzzy Filter" entry,
    /// which the bar renders via its `fuzzy_label`).
    fn bar_hints(&self) -> Vec<BarHint> {
        let ckb = &self.keybindings.common;
        let tkb = &self.keybindings.tasks;
        vec![
            (ckb.label(&CommonAction::SavedFilterSelect), "queries".into()),
            (tkb.label(&TasksAction::FormAdd), "add".into()),
            (tkb.label(&TasksAction::FormEdit), "edit".into()),
            (tkb.label(&TasksAction::FormEditNode), "edit node".into()),
            (tkb.label(&TasksAction::OpenNotes), "notes".into()),
            (ckb.label(&CommonAction::TrackingToggle), "track".into()),
        ]
    }

    /// Push current view state into the bar. Called by App once per frame.
    /// `active_editor` is the description of the open editor (if any).
    /// `tracking_active` is true if any task is currently being tracked.
    pub fn sync_action_bar(&mut self, active_editor: Option<&str>, tracking_active: bool) {
        self.action_bar.set_active_editor(active_editor);
        self.action_bar.set_tracking_active(tracking_active);
        self.action_bar.set_active_filter_name(self.active_filter_name.clone());
        let favs: Vec<(String, String)> = self.favorites.iter()
            .filter_map(|f| f.shortcut.as_ref().map(|s| (f.name.clone(), s.clone())))
            .collect();
        self.action_bar.set_favorites(favs);
        let (q, c, a) = self.fuzzy_state();
        self.action_bar.set_fuzzy(a, &q, c);
        let s = self.search.state();
        self.action_bar.set_search(s.active, &s.query, s.cursor, s.current, s.match_count);
        let cl = self.cmdline.state();
        self.action_bar.set_cmdline(cl.active, &cl.query, cl.cursor);
    }

    pub fn action_bar_height(&self, width: u16) -> u16 {
        self.action_bar.required_height(width)
    }

    pub fn render_action_bar(&mut self, frame: &mut Frame, area: Rect) {
        self.action_bar.view(frame, area);
    }

    // ── Query menu ───────────────────────────────────────────────────

    pub fn has_query_menu(&self) -> bool {
        self.query_menu.is_open()
    }

    pub fn open_query_menu(&mut self) {
        let entries: Vec<QueryMenuEntry> = self.favorites.iter().map(|f| QueryMenuEntry {
            name: f.name.clone(),
            query: f.query.clone(),
            shortcut: f.shortcut.clone(),
            is_default: self.default_query_name.as_deref() == Some(f.name.as_str()),
        }).collect();
        self.query_menu.open(&entries, &self.query_menu_kb);
    }

    pub fn handle_query_menu_key(&mut self, key: &str) -> Option<SubViewMessage> {
        if !self.query_menu.is_open() { return None; }
        let msg = self.query_menu.handle_key(key, &self.query_menu_kb);
        let scope = "task".to_string();
        let noop = Some(SubViewMessage::SelectionChanged(self.selected_id()));
        let request = match msg {
            QueryMenuMessage::Unhandled | QueryMenuMessage::Handled | QueryMenuMessage::Closed => return noop,
            QueryMenuMessage::Apply { name: _, query } => {
                ViewRequest::ApplySavedQuery { scope, content: query }
            }
            QueryMenuMessage::EditExisting { name, query } => {
                ViewRequest::OpenSavedQueryEditor {
                    scope, name, current_query: Some(query), is_new: false,
                }
            }
            QueryMenuMessage::Delete { name } => {
                ViewRequest::DeleteSavedQuery { scope, name }
            }
            QueryMenuMessage::EditShortcut { name, query } => {
                ViewRequest::PromptSavedQueryShortcut { scope, name, query }
            }
            QueryMenuMessage::SetDefault { name } => {
                ViewRequest::SetDefaultSavedQuery { scope, name }
            }
            QueryMenuMessage::CreateNew { name } => {
                ViewRequest::OpenSavedQueryEditor {
                    scope, name, current_query: None, is_new: true,
                }
            }
        };
        Some(SubViewMessage::Request(request))
    }

    pub fn render_query_menu(&mut self, frame: &mut Frame, area: Rect) {
        self.query_menu.render(frame, area);
    }

    pub fn sub_view(&self) -> TasksSubView {
        self.sub_view
    }

    /// Programmatic sub-view switch used by `:jump Tasks:<sub>`.
    /// Returns true when the sub-view actually changed (caller should
    /// spawn a load); false when already on that sub-view.
    pub fn set_sub_view(&mut self, sub: TasksSubView) -> bool {
        if self.sub_view == sub {
            return false;
        }
        let focus_id = self.selected_id();
        self.sub_view = sub;
        if let Some(id) = focus_id {
            match sub {
                TasksSubView::List => self.list_view.table_mut().set_pending_focus(id),
                TasksSubView::Tree => self.tree_view.table_mut().set_pending_focus(id),
            }
        }
        true
    }

    pub fn list_view_table(&self) -> &DataTable { self.list_view.table() }
    pub fn list_view_table_mut(&mut self) -> &mut DataTable { self.list_view.table_mut() }
    pub fn tree_view_table(&self) -> &DataTable { self.tree_view.table() }
    pub fn tree_view_table_mut(&mut self) -> &mut DataTable { self.tree_view.table_mut() }

    /// Auto-expand the path to `target` (used by `:focus-task` and
    /// `/`-search). Delegates to the tree sub-view; no-op when called
    /// in list mode.
    pub fn tree_set_transient_open_for(&mut self, target: Uuid, task_rows: &[Task]) {
        self.tree_view.set_transient_open_for(target, task_rows);
    }

    /// Lock the current transient expansion so the path stays open
    /// after subsequent rebuilds.
    pub fn tree_commit_transient_open(&mut self, task_rows: &[Task]) {
        self.tree_view.commit_transient_open(task_rows);
    }

    /// Get the currently active sub-view's selected task ID.
    pub fn selected_id(&self) -> Option<Uuid> {
        match self.sub_view {
            TasksSubView::List => self.list_view.table().selected_id(),
            TasksSubView::Tree => self.tree_view.table().selected_id(),
        }
    }

    /// Check if fuzzy filter is active on the current sub-view.
    pub fn fuzzy_active(&self) -> bool {
        match self.sub_view {
            TasksSubView::List => self.list_view.table().fuzzy_active,
            TasksSubView::Tree => self.tree_view.table().fuzzy_active,
        }
    }

    /// Get fuzzy state from the active sub-view.
    pub fn fuzzy_state(&self) -> (String, usize, bool) {
        match self.sub_view {
            TasksSubView::List => {
                let t = self.list_view.table();
                (t.fuzzy_query.clone(), t.fuzzy_cursor, t.fuzzy_active)
            }
            TasksSubView::Tree => {
                let t = self.tree_view.table();
                (t.fuzzy_query.clone(), t.fuzzy_cursor, t.fuzzy_active)
            }
        }
    }

    /// Get action bar hints from the active sub-view.
    pub fn action_bar_hints(&self) -> Vec<BarHint> {
        match self.sub_view {
            TasksSubView::List => self.list_view.action_bar_hints(),
            TasksSubView::Tree => self.tree_view.action_bar_hints(),
        }
    }

    /// Get status bar hints from the active sub-view.
    pub fn status_bar_hints(&self) -> Vec<BarHint> {
        match self.sub_view {
            TasksSubView::List => self.list_view.status_bar_hints(),
            TasksSubView::Tree => self.tree_view.status_bar_hints(),
        }
    }

    /// Set pending focus on the active sub-view.
    pub fn set_nav_chars(&mut self, chars: &[char]) {
        self.list_view.table_mut().set_nav_chars(chars);
        self.tree_view.table_mut().set_nav_chars(chars);
    }

    pub fn set_pending_focus(&mut self, id: Uuid) {
        match self.sub_view {
            TasksSubView::List => self.list_view.table_mut().set_pending_focus(id),
            TasksSubView::Tree => self.tree_view.table_mut().set_pending_focus(id),
        }
    }

    /// Refresh the active sub-view using the view's own state.
    pub fn refresh_from_own_state(
        &mut self,
        tracked_ids: &HashSet<Uuid>,
        link_refs: &HashSet<String>,
    ) {
        let column_config = self.column_config.clone();
        let applied_sort = self.state.last_applied_sort.clone();
        let overlay = self.header_overlay.clone();
        let tags_by_task = self.state.task_tags.clone();
        match self.sub_view {
            TasksSubView::List => {
                self.list_view.refresh(
                    &self.state.task_rows, tracked_ids, link_refs, &column_config,
                    &tags_by_task, &applied_sort, &overlay,
                );
            }
            TasksSubView::Tree => {
                if let Some(ref forest) = self.state.forest {
                    let filter_text = self.tree_view.table().filter_text.clone();
                    self.tree_view.refresh(
                        forest, &self.state.task_rows, &filter_text, tracked_ids, link_refs,
                        &column_config, &tags_by_task, &applied_sort, &overlay,
                    );
                }
            }
        }
    }

    /// Refresh the active sub-view's table data (external data).
    pub fn refresh(
        &mut self,
        forest: Option<&TaskForest>,
        task_rows: &[Task],
        tracked_ids: &HashSet<Uuid>,
        link_refs: &HashSet<String>,
        column_config: &[String],
    ) {
        let applied_sort = self.state.last_applied_sort.clone();
        let overlay = self.header_overlay.clone();
        let tags_by_task = self.state.task_tags.clone();
        match self.sub_view {
            TasksSubView::List => {
                self.list_view.refresh(
                    task_rows, tracked_ids, link_refs, column_config, &tags_by_task,
                    &applied_sort, &overlay,
                );
            }
            TasksSubView::Tree => {
                if let Some(forest) = forest {
                    let filter_text = self.tree_view.table().filter_text.clone();
                    self.tree_view.refresh(
                        forest, task_rows, &filter_text, tracked_ids, link_refs,
                        column_config, &tags_by_task, &applied_sort, &overlay,
                    );
                }
            }
        }
    }

    /// Handle a key event. Returns SubViewMessage for the App.
    pub fn handle_key(&mut self, key: &str) -> SubViewMessage {
        // Check sub-view switch first (only this view knows about l/t).
        let tkb = &self.keybindings.tasks;
        if tkb.bindings.get(&TasksAction::ViewList).map_or(false, |b| b.matches(key)) {
            if self.sub_view != TasksSubView::List {
                let focus_id = self.selected_id();
                self.sub_view = TasksSubView::List;
                if let Some(id) = focus_id {
                    self.list_view.table_mut().set_pending_focus(id);
                }
                return SubViewMessage::Request(ViewRequest::SpawnLoad);
            }
            return SubViewMessage::SelectionChanged(self.selected_id());
        }
        if tkb.bindings.get(&TasksAction::ViewTree).map_or(false, |b| b.matches(key)) {
            if self.sub_view != TasksSubView::Tree {
                let focus_id = self.selected_id();
                self.sub_view = TasksSubView::Tree;
                if let Some(id) = focus_id {
                    self.tree_view.table_mut().set_pending_focus(id);
                }
                return SubViewMessage::Request(ViewRequest::SpawnLoad);
            }
            return SubViewMessage::SelectionChanged(self.selected_id());
        }

        // :script menu open — handled at the TasksView level so list +
        // tree share the same binding (default `x`) and the same Tasks-
        // wide scripts directory. App reads the selected task from
        // `tasks_view.selected_id()` and walks `parent_id` for the
        // ancestor chain in `open_script_menu_for_tasks`.
        if tkb.bindings.get(&TasksAction::OpenScriptMenu).map_or(false, |b| b.matches(key)) {
            return SubViewMessage::Request(ViewRequest::OpenScriptMenuForTasks);
        }

        // Tree-collapse jumps the cursor to the parent of the current
        // selection so the user lands on a still-visible row after the
        // collapse drops everything below `default_expand_depth`. Has to
        // happen before delegation because the tree_view sub-component
        // doesn't see `state.task_rows`.
        if self.sub_view == TasksSubView::Tree
            && tkb.bindings.get(&TasksAction::TreeCollapseAll).map_or(false, |b| b.matches(key))
        {
            if let Some(current) = self.tree_view.table().selected_id() {
                if let Some(parent) = self
                    .state
                    .task_rows
                    .iter()
                    .find(|t| t.id == current)
                    .and_then(|t| t.parent_id)
                {
                    self.tree_view.table_mut().set_pending_focus(parent);
                }
            }
        }

        // Delegate to active sub-view.
        match self.sub_view {
            TasksSubView::List => self.list_view.handle_key(key),
            TasksSubView::Tree => self.tree_view.handle_key(key),
        }
    }

    /// Handle a key during fuzzy mode.
    pub fn handle_fuzzy_key(&mut self, key: &str) -> Option<SubViewMessage> {
        match self.sub_view {
            TasksSubView::List => self.list_view.handle_fuzzy_key(key),
            TasksSubView::Tree => self.tree_view.handle_fuzzy_key(key),
        }
    }
}

// ── Searchable trait ──────────────────────────────────────────────────

impl TasksView {
    /// Build (description-index, description) pairs for search matching.
    ///
    /// In list mode the index is the table row index — match-jump becomes
    /// a simple `set_selected`. In tree mode the corpus is *all* tasks
    /// (collapsed-or-not), and the index is just the position in
    /// `state.task_rows`; `tree_search_id_for` maps it back to a UUID
    /// the jump path then expands and focuses by id.
    fn search_descriptions(&self) -> Vec<(usize, String)> {
        match self.sub_view {
            TasksSubView::List => {
                let table = self.list_view.table();
                let row_count = table.row_count();
                let mut descs = Vec::with_capacity(row_count);
                for i in 0..row_count {
                    if let Some(id) = table.selected_id_at(i) {
                        if let Some(task) = self.state.task_rows.iter().find(|t| t.id == id) {
                            descs.push((i, task.description.clone()));
                        }
                    }
                }
                descs
            }
            TasksSubView::Tree => {
                // Tree mode: when a fuzzy filter is active, the tree is
                // already pruned to matches+ancestors with everything
                // shown — fall back to the visible-row corpus so jumps
                // land on real table rows. Otherwise search across every
                // task so collapsed-hidden matches are still findable.
                let table = self.tree_view.table();
                if !table.filter_text.is_empty() {
                    let row_count = table.row_count();
                    let mut descs = Vec::with_capacity(row_count);
                    for i in 0..row_count {
                        if let Some(id) = table.selected_id_at(i) {
                            if let Some(task) = self.state.task_rows.iter().find(|t| t.id == id) {
                                descs.push((i, task.description.clone()));
                            }
                        }
                    }
                    descs
                } else {
                    self.state
                        .task_rows
                        .iter()
                        .enumerate()
                        .map(|(i, t)| (i, t.description.clone()))
                        .collect()
                }
            }
        }
    }

    /// In tree mode, map a description-index returned by `SearchComponent`
    /// back to the task UUID it stands for.
    fn tree_search_id_for(&self, index: usize) -> Option<Uuid> {
        self.state.task_rows.get(index).map(|t| t.id)
    }
}

impl Searchable for TasksView {
    fn search_active(&self) -> bool {
        self.search.active()
    }

    fn search_state(&self) -> SearchState {
        self.search.state()
    }

    fn search_open(&mut self) {
        self.search.open();
    }

    fn search_close(&mut self) {
        // Enter just closes the input — the transient auto-expansion is
        // intentionally left in place so the user can keep cycling with
        // n/N. Commit happens later, when the user touches any non-search
        // key (handled in App::dispatch).
        self.search.close();
    }

    fn search_clear(&mut self) {
        // Esc → drop the auto-expansion so the user gets their original
        // collapse state back.
        self.tree_view.clear_transient_open();
        self.search.clear();
    }

    fn search_handle_key(&mut self, key: &str) -> SearchKeyResult {
        let result = self.search.handle_key(key);
        match result {
            SearchKeyResult::QueryChanged => {
                let descs = self.search_descriptions();
                let refs: Vec<(usize, &str)> = descs.iter().map(|(i, s)| (*i, s.as_str())).collect();
                self.search.update_matches(&refs);
                let first = self.search.first_match();
                self.jump_to_search_index(first);
            }
            SearchKeyResult::Cancelled => {
                // Esc — discard the auto-expansion regardless of whether
                // the user cleared the query or closed the search.
                self.tree_view.clear_transient_open();
            }
            // Accepted (Enter) intentionally leaves the transient alone:
            // the n/N cycle continues, the first non-search keystroke
            // (j/k/space/…) is what locks the current path open via
            // `commit_search_transient` in App::dispatch.
            SearchKeyResult::Accepted | SearchKeyResult::Handled => {}
        }
        result
    }

    fn search_jump(&mut self, direction: isize) {
        let target = self.search.jump(direction);
        self.jump_to_search_index(target);
    }
}

impl TasksView {
    /// Common landing for first-match (after query change) and n/N jumps.
    /// In list mode (and tree mode while a fuzzy filter is active) the
    /// index *is* the row index — set_selected is enough. In tree mode
    /// without a fuzzy filter, the corpus is `state.task_rows`, so the
    /// index resolves to a task UUID; we transiently expand its ancestor
    /// chain and let the next refresh make the path visible, then
    /// `set_pending_focus` puts the cursor on the match.
    fn jump_to_search_index(&mut self, index: Option<usize>) {
        let Some(idx) = index else { return };
        match self.sub_view {
            TasksSubView::List => self.list_view.table_mut().set_selected(idx),
            TasksSubView::Tree => {
                let fuzzy_active = !self.tree_view.table().filter_text.is_empty();
                if fuzzy_active {
                    self.tree_view.table_mut().set_selected(idx);
                } else if let Some(id) = self.tree_search_id_for(idx) {
                    self.tree_view.set_transient_open_for(id, &self.state.task_rows);
                    self.tree_view.table_mut().set_pending_focus(id);
                }
            }
        }
    }

    /// Lock the current transient expansion in place. Called by App
    /// before dispatching non-search actions so the auto-expanded path
    /// survives j/k/space/etc.
    pub fn commit_search_transient(&mut self) {
        if !self.tree_view.has_transient_open() {
            return;
        }
        let task_rows = self.state.task_rows.clone();
        self.tree_view.commit_transient_open(&task_rows);
    }
}

impl SortableView for TasksView {
    fn sortable_columns(&self) -> Vec<SortableColumn> {
        task_sortable_columns()
    }

    fn current_sort(&self) -> &[SortKey] {
        &self.state.current_sort
    }

    fn set_current_sort(&mut self, sort: Vec<SortKey>) -> bool {
        self.state.set_current_sort(sort)
    }

    fn last_applied_sort(&self) -> &[SortKey] {
        &self.state.last_applied_sort
    }
}

impl HasCmdline for TasksView {
    fn cmdline_active(&self) -> bool {
        self.cmdline.active()
    }

    fn cmdline_state(&self) -> CmdlineState {
        self.cmdline.state()
    }

    fn cmdline_open(&mut self) {
        self.cmdline.open();
    }

    fn cmdline_open_with(&mut self, prefill: &str) {
        self.cmdline.open_with(prefill);
    }

    fn cmdline_close(&mut self) {
        self.cmdline.close();
    }

    fn cmdline_handle_key(&mut self, key: &str) -> CmdlineKeyResult {
        self.cmdline.handle_key(key)
    }
}

impl Component for TasksView {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        match self.sub_view {
            TasksSubView::List => self.list_view.view(frame, area),
            TasksSubView::Tree => self.tree_view.view(frame, area),
        }
    }

    fn query(&self, attr: tuirealm::props::Attribute) -> Option<tuirealm::props::QueryResult<'_>> {
        match self.sub_view {
            TasksSubView::List => self.list_view.query(attr),
            TasksSubView::Tree => self.tree_view.query(attr),
        }
    }

    fn attr(&mut self, attr: tuirealm::props::Attribute, value: tuirealm::props::AttrValue) {
        match self.sub_view {
            TasksSubView::List => self.list_view.attr(attr, value),
            TasksSubView::Tree => self.tree_view.attr(attr, value),
        }
    }

    fn state(&self) -> tuirealm::state::State {
        match self.sub_view {
            TasksSubView::List => self.list_view.state(),
            TasksSubView::Tree => self.tree_view.state(),
        }
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        match self.sub_view {
            TasksSubView::List => self.list_view.perform(cmd),
            TasksSubView::Tree => self.tree_view.perform(cmd),
        }
    }
}
