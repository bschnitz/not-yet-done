//! TasksTreeView — hierarchical tree view of tasks.
//!
//! Owns a DataTable, handles its own keybindings, communicates
//! via SubViewMessage to its parent (TasksView).

use std::collections::HashSet;
use std::sync::Arc;

use ratatui::layout::Rect;
use ratatui::Frame;
use uuid::Uuid;

use tuirealm::command::{Cmd, Direction, Position};
use tuirealm::component::Component;

use crate::components::data_table::DataTable;
use crate::components::sort_header::{render_direction_picker_overlay, HeaderOverlay};
use crate::components::task_table::{build_tree_rows, sortable_key_for};
use crate::config::{CommonAction, TasksAction, KeyBindingConfig};
use crate::tabs::columns::{column_meta, resolve_color};
use crate::ui::tasks::forest::{LocalUuid, TaskForest};
use crate::ui::theme::Theme;
use crate::views::tasks_tree_state::TasksTreeState;
use crate::views::{BarHint, SubViewMessage, ViewRequest};

use not_yet_done_content::SortKey;
use not_yet_done_core::entity::task::Model as Task;
use not_yet_done_forest::TreeRenderOptions;
use not_yet_done_ratatui::{
    TableWidgetRow, TableStyle, TableStyleType, ColumnStyles, StyleMap,
};
use ratatui::style::Style;

pub struct TasksTreeView {
    table: DataTable,
    theme: Arc<Theme>,
    keybindings: KeyBindingConfig,
    last_col_widths: Vec<usize>,
    last_column_keys: Vec<String>,
    last_overlay: HeaderOverlay,
    tree_state: TasksTreeState,
}

impl TasksTreeView {
    pub fn new(theme: Arc<Theme>, keybindings: KeyBindingConfig, default_expand_depth: u32) -> Self {
        Self {
            table: DataTable::new(),
            theme,
            keybindings,
            last_col_widths: Vec::new(),
            last_column_keys: Vec::new(),
            last_overlay: HeaderOverlay::None,
            tree_state: TasksTreeState::new(default_expand_depth),
        }
    }

    /// Apply a new default expand depth (config-reload entry point).
    pub fn set_default_expand_depth(&mut self, depth: u32) {
        self.tree_state.set_default_expand_depth(depth);
    }

    pub fn table(&self) -> &DataTable { &self.table }
    pub fn table_mut(&mut self) -> &mut DataTable { &mut self.table }

    /// Auto-expand the ancestor path to `target` so that a `/`-search hit
    /// in a collapsed branch becomes visible. Replaces any prior
    /// transient expansion (so navigating to the next match collapses
    /// the previous path again).
    pub fn set_transient_open_for(&mut self, target: Uuid, task_rows: &[Task]) {
        let parent_of: std::collections::HashMap<Uuid, Uuid> = task_rows
            .iter()
            .filter_map(|t| t.parent_id.map(|p| (t.id, p)))
            .collect();
        let mut ancestors: HashSet<Uuid> = HashSet::new();
        let mut cur = parent_of.get(&target).copied();
        while let Some(p) = cur {
            if !ancestors.insert(p) {
                break; // cycle guard
            }
            cur = parent_of.get(&p).copied();
        }
        self.tree_state.set_transient_open(ancestors);
    }

    /// Drop the transient expansion without locking anything in. Used
    /// when `/`-search is cancelled.
    pub fn clear_transient_open(&mut self) {
        self.tree_state.clear_transient_open();
    }

    /// Lock the current transient expansion into `flipped`. Called when
    /// the user moves on from `/`-search (Enter / any non-`n`/`N` key).
    /// No-op when nothing's transient. Walks `task_rows` to compute each
    /// node's depth so the state can decide which ones need promotion.
    pub fn commit_transient_open(&mut self, task_rows: &[Task]) {
        if !self.tree_state.has_transient_open() {
            return;
        }
        let depths = compute_depths(task_rows);
        self.tree_state.commit_transient_open(&depths);
    }

    pub fn has_transient_open(&self) -> bool {
        self.tree_state.has_transient_open()
    }

    /// Rebuild the table from forest + task data.
    pub fn refresh(
        &mut self,
        forest: &TaskForest,
        task_rows: &[Task],
        filter_text: &str,
        tracked_ids: &HashSet<Uuid>,
        link_refs: &HashSet<String>,
        column_config: &[String],
        tags_by_task: &std::collections::HashMap<Uuid, Vec<not_yet_done_core::repository::ResolvedTag>>,
        applied_sort: &[SortKey],
        overlay: &HeaderOverlay,
    ) {
        // Tree-render options: when a fuzzy filter is active, the ghost
        // tree is already pruned to matches + ancestors — show everything
        // (no collapsed branches would just hide matches). Otherwise feed
        // the user's expand state in.
        let tree_options: TreeRenderOptions<LocalUuid> = if filter_text.is_empty() {
            let state = self.tree_state.clone();
            TreeRenderOptions {
                is_expanded: Box::new(move |id: &LocalUuid, depth: usize| {
                    state.is_open(&id.0, depth as u32)
                }),
                child_counts: forest.child_counts(),
            }
        } else {
            TreeRenderOptions::all_visible()
        };

        let built = build_tree_rows(
            forest, filter_text, 200, tracked_ids, link_refs, column_config, task_rows,
            tags_by_task, applied_sort, overlay, &tree_options,
        );
        self.last_col_widths = built.col_widths;
        self.last_column_keys = column_config.iter()
            .map(|c| sortable_key_for(c).to_string())
            .collect();
        self.last_overlay = overlay.clone();

        let t = &self.theme;
        let col_styles = ColumnStyles::new(
            column_config.iter().map(|id| {
                let fg = column_meta(id)
                    .map(|meta| resolve_color(meta.color_key, t))
                    .unwrap_or(t.text_dim());
                Style::default().fg(fg)
            }).collect()
        );
        let table_style = TableStyle::new()
            .set_style(TableStyleType::Header, Style::default().bg(t.surface()))
            .set_style(TableStyleType::Row, Style::default().fg(t.text_med()).bg(t.bg()))
            .set_style(TableStyleType::RowSelected, Style::default().fg(t.text_high()).bg(t.surface_2()))
            .set_style(TableStyleType::Highlight, Style::default().fg(t.accent()))
            .set_style(TableStyleType::Prefix, Style::default().fg(t.tree_connector()));

        // Slot 0 (DIM_STYLE_ID) holds the dim color for sort-mode overlay.
        let style_map = StyleMap::new(vec![Style::default().fg(t.text_dim())]);
        let headers = built.header.map(|h| vec![TableWidgetRow::new(h).not_selectable()]).unwrap_or_default();

        self.table.set_data(
            built.rows, built.row_ids, headers, vec![],
            col_styles, table_style, style_map, "  ",
        );
    }

    pub fn action_bar_hints(&self) -> Vec<BarHint> {
        let ckb = &self.keybindings.common;
        let tkb = &self.keybindings.tasks;
        vec![
            (ckb.label(&CommonAction::FuzzyFilterOpen), "Fuzzy Filter".into()),
            (ckb.label(&CommonAction::SavedFilterSelect), "queries".into()),
            (tkb.label(&TasksAction::TreeToggle), "expand/collapse".into()),
            (tkb.label(&TasksAction::FormAdd), "add".into()),
            (tkb.label(&TasksAction::FormEdit), "edit".into()),
            (tkb.label(&TasksAction::FormEditNode), "edit node".into()),
            (tkb.label(&TasksAction::OpenNotes), "notes".into()),
            (ckb.label(&CommonAction::TrackingToggle), "track".into()),
        ]
    }

    pub fn status_bar_hints(&self) -> Vec<BarHint> {
        let ckb = &self.keybindings.common;
        let tkb = &self.keybindings.tasks;
        vec![
            (ckb.label(&CommonAction::SavedFilterSelect), "queries".into()),
            (tkb.label(&TasksAction::FormAdd), "add".into()),
            (tkb.label(&TasksAction::FormEdit), "edit".into()),
            (tkb.label(&TasksAction::FormEditNode), "edit node".into()),
            (tkb.label(&TasksAction::Delete), "delete".into()),
            (ckb.label(&CommonAction::ColumnConfig), "columns".into()),
        ]
    }

    pub fn handle_key(&mut self, key: &str) -> SubViewMessage {
        let ckb = &self.keybindings.common;
        let tkb = &self.keybindings.tasks;

        // --- Common navigation ---
        if ckb.bindings.get(&CommonAction::ListNext).map_or(false, |b| b.matches(key)) {
            self.table.handle_nav(Cmd::Move(Direction::Down));
            return SubViewMessage::SelectionChanged(self.table.selected_id());
        }
        if ckb.bindings.get(&CommonAction::ListPrev).map_or(false, |b| b.matches(key)) {
            self.table.handle_nav(Cmd::Move(Direction::Up));
            return SubViewMessage::SelectionChanged(self.table.selected_id());
        }
        if ckb.bindings.get(&CommonAction::ListFirst).map_or(false, |b| b.matches(key)) {
            self.table.handle_nav(Cmd::GoTo(Position::Begin));
            return SubViewMessage::SelectionChanged(self.table.selected_id());
        }
        if ckb.bindings.get(&CommonAction::ListLast).map_or(false, |b| b.matches(key)) {
            self.table.handle_nav(Cmd::GoTo(Position::End));
            return SubViewMessage::SelectionChanged(self.table.selected_id());
        }

        // --- Scroll ---
        if ckb.bindings.get(&CommonAction::ScrollHalfUp).map_or(false, |b| b.matches(key)) {
            let n = (self.table.visible_rows() / 2).max(1) as isize;
            self.table.scroll_by(-n);
            return SubViewMessage::SelectionChanged(self.table.selected_id());
        }
        if ckb.bindings.get(&CommonAction::ScrollHalfDown).map_or(false, |b| b.matches(key)) {
            let n = (self.table.visible_rows() / 2).max(1) as isize;
            self.table.scroll_by(n);
            return SubViewMessage::SelectionChanged(self.table.selected_id());
        }
        if ckb.bindings.get(&CommonAction::ScrollPageUp).map_or(false, |b| b.matches(key)) {
            let n = self.table.visible_rows().max(1) as isize;
            self.table.scroll_by(-n);
            return SubViewMessage::SelectionChanged(self.table.selected_id());
        }
        if ckb.bindings.get(&CommonAction::ScrollPageDown).map_or(false, |b| b.matches(key)) {
            let n = self.table.visible_rows().max(1) as isize;
            self.table.scroll_by(n);
            return SubViewMessage::SelectionChanged(self.table.selected_id());
        }

        // --- Fuzzy ---
        if ckb.bindings.get(&CommonAction::FuzzyFilterOpen).map_or(false, |b| b.matches(key)) {
            self.table.fuzzy_open();
            return SubViewMessage::FuzzyStateChanged {
                active: true,
                query: self.table.fuzzy_query.clone(),
                cursor: self.table.fuzzy_cursor,
            };
        }

        // --- Tree expand/collapse ---
        if tkb.bindings.get(&TasksAction::TreeToggle).map_or(false, |b| b.matches(key)) {
            // Toggle expand/collapse on the focused node. Leaves no-op
            // silently (TasksTreeState::toggle is harmless on leaves —
            // they have no rendered effect).
            if let Some(id) = self.table.selected_id() {
                self.tree_state.toggle(id);
            }
            return SubViewMessage::RefreshRequested;
        }
        if tkb.bindings.get(&TasksAction::TreeExpandAll).map_or(false, |b| b.matches(key)) {
            self.tree_state.expand_all();
            return SubViewMessage::RefreshRequested;
        }
        if tkb.bindings.get(&TasksAction::TreeCollapseAll).map_or(false, |b| b.matches(key)) {
            self.tree_state.reset_to_default();
            return SubViewMessage::RefreshRequested;
        }

        // --- Tasks-only actions ---
        if tkb.bindings.get(&TasksAction::FormAdd).map_or(false, |b| b.matches(key)) {
            // In tree view, add as child of selected task.
            let parent_id = self.table.selected_id();
            return SubViewMessage::Request(ViewRequest::OpenEditorForAdd { parent_id });
        }
        if tkb.bindings.get(&TasksAction::FormEdit).map_or(false, |b| b.matches(key)) {
            if let Some(id) = self.table.selected_id() {
                return SubViewMessage::Request(ViewRequest::OpenEditorForEdit(id));
            }
        }
        if tkb.bindings.get(&TasksAction::FormEditNode).map_or(false, |b| b.matches(key)) {
            if let Some(id) = self.table.selected_id() {
                return SubViewMessage::Request(ViewRequest::OpenEditorForEditNode(id));
            }
        }
        if tkb.bindings.get(&TasksAction::Delete).map_or(false, |b| b.matches(key)) {
            if let Some(id) = self.table.selected_id() {
                return SubViewMessage::Request(ViewRequest::DeleteTaskRecursive(id));
            }
        }
        if tkb.bindings.get(&TasksAction::Undelete).map_or(false, |b| b.matches(key)) {
            return SubViewMessage::Request(ViewRequest::Undelete);
        }
        if tkb.bindings.get(&TasksAction::OpenNotes).map_or(false, |b| b.matches(key)) {
            if let Some(id) = self.table.selected_id() {
                return SubViewMessage::Request(ViewRequest::OpenEditorForNotes(id));
            }
        }
        if ckb.bindings.get(&CommonAction::TrackingToggle).map_or(false, |b| b.matches(key)) {
            if let Some(id) = self.table.selected_id() {
                return SubViewMessage::Request(ViewRequest::ToggleTracking(id));
            }
        }

        // --- Popups ---
        // SavedFilterSelect (`q`) is handled at the App level (opens the
        // unified query menu component owned by TasksView).
        if ckb.bindings.get(&CommonAction::ColumnConfig).map_or(false, |b| b.matches(key)) {
            return SubViewMessage::Request(ViewRequest::OpenColumnConfig);
        }

        SubViewMessage::Unhandled
    }

    /// Handle a key during fuzzy mode.
    pub fn handle_fuzzy_key(&mut self, key: &str) -> Option<SubViewMessage> {
        let ckb = &self.keybindings.common;

        if ckb.bindings.get(&CommonAction::FuzzyFilterAccept).map_or(false, |b| b.matches(key)) {
            self.table.fuzzy_close();
            return Some(SubViewMessage::FuzzyStateChanged {
                active: false, query: self.table.fuzzy_query.clone(), cursor: 0,
            });
        }
        if ckb.bindings.get(&CommonAction::FuzzyFilterCancel).map_or(false, |b| b.matches(key)) {
            if self.table.fuzzy_query.is_empty() {
                self.table.fuzzy_close();
            } else {
                self.table.fuzzy_query.clear();
                self.table.fuzzy_cursor = 0;
                self.table.filter_text.clear();
            }
            return Some(SubViewMessage::FuzzyStateChanged {
                active: self.table.fuzzy_active,
                query: self.table.fuzzy_query.clone(),
                cursor: self.table.fuzzy_cursor,
            });
        }
        if ckb.bindings.get(&CommonAction::FuzzyFilterClear).map_or(false, |b| b.matches(key)) {
            self.table.fuzzy_query.clear();
            self.table.fuzzy_cursor = 0;
            self.table.filter_text.clear();
            return Some(SubViewMessage::FuzzyStateChanged {
                active: true, query: String::new(), cursor: 0,
            });
        }

        match key {
            "backspace" => { self.table.fuzzy_backspace(); }
            "left" => { self.table.fuzzy_cursor_left(); }
            "right" => { self.table.fuzzy_cursor_right(); }
            "space" => { self.table.fuzzy_insert(' '); }
            ch if ch.chars().count() == 1 && !ch.chars().next().unwrap().is_control() => {
                self.table.fuzzy_insert(ch.chars().next().unwrap());
            }
            _ => return None,
        }

        Some(SubViewMessage::FuzzyStateChanged {
            active: true,
            query: self.table.fuzzy_query.clone(),
            cursor: self.table.fuzzy_cursor,
        })
    }
}

/// Tree depth for every task, computed by walking the parent-id chain.
/// Root tasks (no parent) have depth 0.
fn compute_depths(task_rows: &[Task]) -> std::collections::HashMap<Uuid, u32> {
    let parent_of: std::collections::HashMap<Uuid, Uuid> = task_rows
        .iter()
        .filter_map(|t| t.parent_id.map(|p| (t.id, p)))
        .collect();
    let mut depths = std::collections::HashMap::with_capacity(task_rows.len());
    for t in task_rows {
        let mut d = 0u32;
        let mut cur = parent_of.get(&t.id).copied();
        // Bound the walk to defeat any accidental cycles.
        for _ in 0..task_rows.len() {
            match cur {
                Some(p) => {
                    d += 1;
                    cur = parent_of.get(&p).copied();
                }
                None => break,
            }
        }
        depths.insert(t.id, d);
    }
    depths
}

impl Component for TasksTreeView {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        self.table.view(frame, area);
        if self.last_overlay.is_active() {
            let keys: Vec<&str> = self.last_column_keys.iter().map(|s| s.as_str()).collect();
            let style = Style::default().fg(self.theme.accent());
            render_direction_picker_overlay(
                frame, area, &keys, &self.last_col_widths, 2,
                &self.last_overlay, style,
            );
        }
    }

    fn query(&self, attr: tuirealm::props::Attribute) -> Option<tuirealm::props::QueryResult<'_>> {
        self.table.query(attr)
    }

    fn attr(&mut self, attr: tuirealm::props::Attribute, value: tuirealm::props::AttrValue) {
        self.table.attr(attr, value);
    }

    fn state(&self) -> tuirealm::state::State {
        self.table.state()
    }

    fn perform(&mut self, cmd: Cmd) -> tuirealm::command::CmdResult {
        self.table.perform(cmd)
    }
}
