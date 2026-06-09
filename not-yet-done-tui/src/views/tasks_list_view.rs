//! TasksListView — flat list view of tasks.
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
use crate::components::task_table::{build_flat_rows, sortable_key_for};
use crate::config::{CommonAction, TasksAction, KeyBindingConfig};
use crate::tabs::columns::{column_meta, resolve_color};
use crate::ui::theme::Theme;
use crate::views::{BarHint, SubViewMessage, ViewRequest};

use not_yet_done_content::SortKey;
use not_yet_done_core::entity::task::Model as Task;
use not_yet_done_ratatui::{
    TableWidgetRow, TableStyle, TableStyleType, ColumnStyles, StyleMap,
};
use ratatui::style::Style;

pub struct TasksListView {
    table: DataTable,
    theme: Arc<Theme>,
    keybindings: KeyBindingConfig,
    /// Column widths from the most recent layout — needed to position
    /// the sort-mode direction-picker overlay correctly.
    last_col_widths: Vec<usize>,
    last_column_keys: Vec<String>,
    last_overlay: HeaderOverlay,
}

impl TasksListView {
    pub fn new(theme: Arc<Theme>, keybindings: KeyBindingConfig) -> Self {
        Self {
            table: DataTable::new(),
            theme,
            keybindings,
            last_col_widths: Vec::new(),
            last_column_keys: Vec::new(),
            last_overlay: HeaderOverlay::None,
        }
    }

    /// Access the inner DataTable.
    pub fn table(&self) -> &DataTable { &self.table }
    pub fn table_mut(&mut self) -> &mut DataTable { &mut self.table }

    /// Rebuild the table from task data.
    pub fn refresh(
        &mut self,
        task_rows: &[Task],
        tracked_ids: &HashSet<Uuid>,
        link_refs: &HashSet<String>,
        column_config: &[String],
        tags_by_task: &std::collections::HashMap<Uuid, Vec<not_yet_done_core::repository::ResolvedTag>>,
        applied_sort: &[SortKey],
        overlay: &HeaderOverlay,
    ) {
        let built = build_flat_rows(
            task_rows, tracked_ids, link_refs, column_config, 200, task_rows,
            tags_by_task, applied_sort, overlay,
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

        // Slot 0 (DIM_STYLE_ID) holds the dim color used by the sort-mode
        // overlay to gray out non-candidate columns.
        let style_map = StyleMap::new(vec![Style::default().fg(t.text_dim())]);
        let headers = built.header.map(|h| vec![TableWidgetRow::new(h).not_selectable()]).unwrap_or_default();

        self.table.set_data(
            built.rows, built.row_ids, headers, vec![],
            col_styles, table_style, style_map, "  ",
        );
    }

    /// Return action bar hints for this view.
    pub fn action_bar_hints(&self) -> Vec<BarHint> {
        let ckb = &self.keybindings.common;
        let tkb = &self.keybindings.tasks;
        vec![
            (ckb.label(&CommonAction::FuzzyFilterOpen), "Fuzzy Filter".into()),
            (ckb.label(&CommonAction::SavedFilterSelect), "queries".into()),
            (tkb.label(&TasksAction::FormAdd), "add".into()),
            (tkb.label(&TasksAction::FormEdit), "edit".into()),
            (tkb.label(&TasksAction::OpenNotes), "notes".into()),
            (ckb.label(&CommonAction::TrackingToggle), "track".into()),
        ]
    }

    /// Return status bar hints for this view.
    pub fn status_bar_hints(&self) -> Vec<BarHint> {
        let ckb = &self.keybindings.common;
        let tkb = &self.keybindings.tasks;
        vec![
            (ckb.label(&CommonAction::SavedFilterSelect), "queries".into()),
            (tkb.label(&TasksAction::FormAdd), "add".into()),
            (tkb.label(&TasksAction::FormEdit), "edit".into()),
            (tkb.label(&TasksAction::Delete), "delete".into()),
            (ckb.label(&CommonAction::ColumnConfig), "columns".into()),
        ]
    }

    /// Handle a key event. Returns a message for the parent.
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

        // --- Tasks-only actions ---
        if tkb.bindings.get(&TasksAction::FormAdd).map_or(false, |b| b.matches(key)) {
            return SubViewMessage::Request(ViewRequest::OpenEditorForAdd { parent_id: None });
        }
        if tkb.bindings.get(&TasksAction::FormEdit).map_or(false, |b| b.matches(key)) {
            if let Some(id) = self.table.selected_id() {
                return SubViewMessage::Request(ViewRequest::OpenEditorForEdit(id));
            }
        }
        if tkb.bindings.get(&TasksAction::Delete).map_or(false, |b| b.matches(key)) {
            if let Some(id) = self.table.selected_id() {
                return SubViewMessage::Request(ViewRequest::DeleteTask(id));
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

        // --- Not handled ---
        SubViewMessage::Unhandled
    }

    /// Handle a key during fuzzy mode. Returns true if handled.
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

impl Component for TasksListView {
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
