//! TrackingsView — container for all tracking sub-views (Normal, Condensed, Tree).
//!
//! Unlike TasksView which has separate sub-view components, TrackingsView
//! handles all three modes internally because they share the same DataTable
//! and TrackingsState.

use std::sync::Arc;

use ratatui::layout::Rect;
use ratatui::Frame;
use tuirealm::command::{Cmd, CmdResult, Direction, Position};
use tuirealm::component::Component;

use crate::app::SavedQuery;
use crate::components::action_bar::{ActionBarComponent, ActionHint};
use crate::components::cmdline::CmdlineComponent;
use crate::components::data_table::DataTable;
use crate::components::query_menu::{QueryMenuComponent, QueryMenuEntry, QueryMenuMessage};
use crate::components::search::SearchComponent;
use crate::config::keybindings::{KeyBindingSection, QueryMenuAction};
use crate::config::{CommonAction, TrackingsAction, KeyBindingConfig};
use crate::tabs::TrackingsSubView;
use crate::tabs::trackings_state::{TrackingsState, TrackingGrouping, DisplayRow, CondensedDisplayRow, TreeDisplayRow};
use crate::ui::tasks::forest::TaskForest;
use crate::ui::theme::Theme;
use crate::views::{
    BarHint, CmdlineKeyResult, CmdlineState, HasCmdline, SearchKeyResult, SearchState,
    Searchable, SubViewMessage, ViewRequest,
};
use not_yet_done_core::filter::FilterExpr;
use not_yet_done_core::repository::{SavedQueryRepository, SettingsRepository, TrackingRepository};

pub struct TrackingsView {
    pub table: DataTable,
    pub state: TrackingsState,
    theme: Arc<Theme>,
    keybindings: KeyBindingConfig,
    pub tracking_repo: Arc<dyn TrackingRepository>,
    pub saved_query_repo: Arc<dyn SavedQueryRepository>,
    pub settings_repo: Arc<dyn SettingsRepository>,
    pub search: SearchComponent,
    pub cmdline: CmdlineComponent,
    pub query_menu: QueryMenuComponent,
    query_menu_kb: KeyBindingSection<QueryMenuAction>,
    pub action_bar: ActionBarComponent,

    // ── Filter state ──────────────────────────────────────────────────
    pub active_filter: Option<FilterExpr>,
    pub active_filter_json: Option<String>,
    pub active_filter_name: Option<String>,
    pub column_config: Vec<String>,
    pub favorites: Vec<SavedQuery>,
    /// Name of the saved query marked as default (★ in the query
    /// menu). Persisted by the App as a settings row; applied on app
    /// start instead of the last-active filter.
    pub default_query_name: Option<String>,

    /// Tracking grouping popup cursor (None = closed).
    pub group_popup: Option<usize>,

    /// Separator string interleaved between task-path segments in the
    /// `taskpath` column. Sourced from `tui.yaml: tracking.taskpath_separator`.
    taskpath_separator: String,

    /// Snapshot of [`App::link_refs`] used to render the `links` column.
    /// App syncs this via [`Self::set_link_refs`] whenever its own cache
    /// changes — keeps `rebuild_table` argument-free for the many call
    /// sites that fire from inside the view (toggle/group/popup).
    link_refs: std::collections::HashSet<String>,
}

impl TrackingsView {
    pub fn new(
        theme: Arc<Theme>,
        keybindings: KeyBindingConfig,
        tracking_repo: Arc<dyn TrackingRepository>,
        saved_query_repo: Arc<dyn SavedQueryRepository>,
        settings_repo: Arc<dyn SettingsRepository>,
    ) -> Self {
        let query_menu = QueryMenuComponent::new(Arc::clone(&theme), "Saved tracking queries")
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
            table: DataTable::new(),
            state: TrackingsState::new(),
            theme,
            keybindings,
            tracking_repo,
            saved_query_repo,
            settings_repo,
            search: SearchComponent::new(),
            cmdline: CmdlineComponent::new(),
            query_menu,
            query_menu_kb,
            action_bar,
            active_filter: None,
            active_filter_json: None,
            active_filter_name: None,
            column_config: crate::tabs::columns::default_tracking_column_ids(),
            favorites: Vec::new(),
            default_query_name: None,
            group_popup: None,
            taskpath_separator: "/".to_string(),
            link_refs: std::collections::HashSet::new(),
        };
        view.action_bar.set_hints(view.bar_hints(None, false));
        view
    }

    /// Override the taskpath separator string (default `/`). App calls
    /// this once after construction with the value from `tui.yaml`.
    pub fn set_taskpath_separator(&mut self, sep: String) {
        self.taskpath_separator = sep;
    }

    /// Replace the local `link_refs` snapshot. App calls this whenever
    /// its own `App::link_refs` cache changes (on startup + after every
    /// link create/delete). The next `rebuild_table` picks up the new
    /// set automatically.
    pub fn set_link_refs(&mut self, refs: &std::collections::HashSet<String>) {
        self.link_refs = refs.clone();
    }

    /// Hints for the action bar (without the leading "Fuzzy Filter" entry,
    /// which the bar renders via its `fuzzy_label`). Each hint's `active`
    /// flag is stamped from the cross-cutting state: the editor hint whose
    /// description matches `active_editor`, and the "track" hint while a
    /// tracking runs.
    fn bar_hints(&self, active_editor: Option<&str>, tracking_active: bool) -> Vec<ActionHint> {
        let ckb = &self.keybindings.common;
        let tkb = &self.keybindings.trackings;
        let mk = |key: String, desc: &str| {
            let active = active_editor == Some(desc) || (desc == "track" && tracking_active);
            ActionHint::new(key, desc).active(active)
        };
        vec![
            mk(ckb.label(&CommonAction::SavedFilterSelect), "queries"),
            mk(tkb.label(&TrackingsAction::TrackingScriptRun), "scripts"),
            mk(ckb.label(&CommonAction::TrackingToggle), "track"),
        ]
    }

    /// Push current view state into the bar. Called by App once per frame.
    pub fn sync_action_bar(&mut self, active_editor: Option<&str>, tracking_active: bool) {
        self.action_bar.set_hints(self.bar_hints(active_editor, tracking_active));
        self.action_bar.set_active_filter_name(self.active_filter_name.clone());
        let favs: Vec<(String, String)> = self.favorites.iter()
            .filter_map(|f| f.shortcut.as_ref().map(|s| (f.name.clone(), s.clone())))
            .collect();
        self.action_bar.set_favorites(favs);
        self.action_bar.set_fuzzy(
            self.state.fuzzy_active,
            &self.state.fuzzy_query,
            self.state.fuzzy_cursor,
        );
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
        let scope = "tracking".to_string();
        let noop = Some(SubViewMessage::SelectionChanged(None));
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

    // ── Column config ────────────────────────────────────────────────

    /// Which tracking columns are visible for the current sub-view.
    /// `taskpath` is offered in Normal/Condensed only — Tree mode renders
    /// the hierarchy via the tree column itself, so a path column would
    /// duplicate that information.
    fn active_columns(&self, column_config: &[String]) -> Vec<String> {
        let available: &[&str] = match self.state.sub_view {
            TrackingsSubView::Normal => &["marker", "taskpath", "task", "started", "ended", "duration"],
            TrackingsSubView::Condensed => &["marker", "taskpath", "task", "duration"],
            TrackingsSubView::Tree => &["marker", "task", "own", "cumulated"],
        };
        let mut result: Vec<String> = column_config.iter()
            .filter(|id| available.contains(&id.as_str()))
            .cloned()
            .collect();
        // Ensure rows are identifiable: if the user hid both `task` and
        // `taskpath`, fall back to `taskpath` (or `task` in Tree, which
        // doesn't carry `taskpath`).
        let identifies = |id: &str| id == "task" || id == "taskpath";
        if !result.iter().any(|id| identifies(id)) {
            let fallback = if self.state.sub_view == TrackingsSubView::Tree { "task" } else { "taskpath" };
            result.insert(0, fallback.to_string());
        }
        result
    }

    // ── Table rebuild ────────────────────────────────────────────────

    /// Rebuild the table widget from current state data.
    pub fn rebuild_table(&mut self) {
        let column_config = self.column_config.clone();
        if self.state.sub_view == TrackingsSubView::Tree {
            self.rebuild_table_tree(&column_config);
        } else {
            self.rebuild_table_normal_condensed(&column_config);
        }
    }

    fn rebuild_table_tree(&mut self, column_config: &[String]) {
        // Snapshot the link cache so the closures below can read it
        // without holding a `&self` borrow that conflicts with `&mut self`.
        let link_refs = self.link_refs.clone();
        use not_yet_done_ratatui::{
            TableWidgetCell, TableWidgetRow, TableStyle, TableStyleType,
            ColumnStyles, StyleMap,
        };
        use not_yet_done_table::{
            ColumnId as TColumnId, ColStrategy, MixedColSizer, TableConfig,
            Row as TRow, compute_table,
        };
        use not_yet_done_table::cell::{CellAlignment, CellContent};
        use ratatui::style::Style;
        use crate::tabs::trackings_state::{format_duration, TreeDisplayRow, TrackingGrouping};

        fn cc_right(s: &str) -> CellContent {
            CellContent::aligned(s, CellAlignment::Right)
        }

        let t = &*self.theme;
        let ts = &self.state;
        let grouped = ts.grouping != TrackingGrouping::None;
        let has_total_col = grouped;
        let active_cols = self.active_columns(column_config);

        let mut col_ids: Vec<TColumnId> = active_cols.iter()
            .map(|id| TColumnId::new(id))
            .collect();
        if has_total_col {
            col_ids.push(TColumnId::new("total"));
        }

        let mut strategies = std::collections::HashMap::new();
        for col in &col_ids {
            let strategy = if col.0 == "task" { ColStrategy::Flex(1) } else { ColStrategy::Max };
            strategies.insert(col.clone(), strategy);
        }

        let config = TableConfig {
            max_width: 200,
            separator: "  ".to_string(),
            sizer: Box::new(MixedColSizer { strategies }),
        };

        let mut header = TRow::new(0u32).not_selectable();
        for col in &active_cols {
            header = match col.as_str() {
                "marker" => header.cell("marker", "⏱"),
                "task" => header.cell("task", "Task"),
                "own" => header.cell("own", cc_right("Own")),
                "cumulated" => header.cell("cumulated", cc_right("Cumulated")),
                "links" => header.cell("links", "🔗"),
                _ => header,
            };
        }
        if has_total_col {
            header = header.cell("total", cc_right("Total"));
        }

        let mut data_rows: Vec<TRow<u32>> = Vec::new();
        let mut connector_map: Vec<usize> = Vec::new();
        let mut group_row_labels: std::collections::HashMap<usize, String> = std::collections::HashMap::new();

        for (i, tr) in ts.tree_rows.iter().enumerate() {
            match tr {
                TreeDisplayRow::GroupHeader { label, .. } => {
                    let row = TRow::new(i as u32).not_selectable()
                        .cell("marker", "");
                    let idx = data_rows.len();
                    data_rows.push(row);
                    connector_map.push(0);
                    group_row_labels.insert(idx, label.clone());
                }
                TreeDisplayRow::Entry { task_id, task_description, own_duration, cumulated_duration, active, tree_cell, connector_chars, group_total, .. } => {
                    let marker = if *active { "⏱" } else { " " };
                    let task_cell = format!("{}{}", tree_cell, task_description);
                    let own = format_duration(*own_duration);
                    let cum = format_duration(*cumulated_duration);
                    let mut row = TRow::new(i as u32)
                        .cell("marker", marker)
                        .cell("task", task_cell.as_str())
                        .cell("own", cc_right(&own))
                        .cell("cumulated", cc_right(&cum));
                    if active_cols.iter().any(|c| c == "links") {
                        let icon = if link_refs.contains(&format!("tasks/{task_id}")) {
                            "🔗"
                        } else {
                            " "
                        };
                        row = row.cell("links", icon);
                    }
                    if has_total_col {
                        let total_str = match group_total {
                            Some(d) => format_duration(*d),
                            None => String::new(),
                        };
                        row = row.cell("total", cc_right(&total_str));
                    }
                    data_rows.push(row);
                    connector_map.push(*connector_chars);
                }
            }
        }

        let computed = compute_table(&data_rows, &config, &col_ids, Some(&header));

        let computed_header = computed.header.map(|h| {
            TableWidgetRow::new(h.cells.into_iter().map(TableWidgetCell::plain).collect()).not_selectable()
        });

        let task_col_idx = col_ids.iter().position(|c| c.0 == "task").unwrap_or(1);

        let widget_rows: Vec<TableWidgetRow> = computed.rows.into_iter().enumerate().map(|(i, cr)| {
            if let Some(label) = group_row_labels.get(&i) {
                let total_cols = col_ids.len();
                let cells = vec![
                    TableWidgetCell::grouped(format!("── {} ", label), total_cols - 1).with_style(0),
                    TableWidgetCell::plain(cr.cells.last().cloned().unwrap_or_default()),
                ];
                TableWidgetRow::new(cells).not_selectable()
            } else {
                let cc = connector_map[i];
                let cells: Vec<TableWidgetCell> = cr.cells.into_iter().enumerate().map(|(ci, text)| {
                    if ci == task_col_idx && cc > 0 {
                        TableWidgetCell::with_prefix(text, cc)
                    } else {
                        TableWidgetCell::plain(text)
                    }
                }).collect();
                TableWidgetRow::new(cells)
            }
        }).collect();

        let total = ts.total_duration();
        let total_str = format_duration(total);
        let separator_line = "─".repeat(total_str.chars().count());
        let num_cols = col_ids.len();

        let make_footer_row = |last_cell: &str| -> TableWidgetRow {
            let cells: Vec<TableWidgetCell> = (0..num_cols).map(|ci| {
                if ci == num_cols - 1 {
                    TableWidgetCell::plain(format!("{:>width$}", last_cell, width = computed.col_widths[ci]))
                } else {
                    TableWidgetCell::plain(" ".repeat(computed.col_widths[ci]))
                }
            }).collect();
            TableWidgetRow::new(cells).not_selectable()
        };

        let footer_sep = make_footer_row(&separator_line);
        let footer_total = make_footer_row(&total_str);

        use crate::tabs::columns::{tracking_column_meta, resolve_color};
        let col_style_list: Vec<Style> = col_ids.iter().map(|cid| {
            let fg = tracking_column_meta(&cid.0)
                .map(|m| resolve_color(m.color_key, t))
                .unwrap_or(t.accent());
            Style::default().fg(fg)
        }).collect();

        let style_map = StyleMap::new(vec![Style::default().fg(t.accent())]);

        let table_style = TableStyle::new()
            .set_style(TableStyleType::Header, Style::default().bg(t.surface()))
            .set_style(TableStyleType::Row, Style::default().fg(t.text_med()).bg(t.bg()))
            .set_style(TableStyleType::RowSelected, Style::default().fg(t.text_high()).bg(t.surface_2()))
            .set_style(TableStyleType::Prefix, Style::default().fg(t.tree_connector()));

        let headers = computed_header.map(|h| vec![h]).unwrap_or_default();
        self.table.set_data(
            widget_rows, vec![], headers, vec![footer_sep, footer_total],
            ColumnStyles::new(col_style_list), table_style, style_map, "  ",
        );
    }

    fn rebuild_table_normal_condensed(&mut self, column_config: &[String]) {
        // Snapshot the link cache so the closures below can read it
        // without holding a `&self` borrow that conflicts with `&mut self`.
        let link_refs = self.link_refs.clone();
        use not_yet_done_ratatui::{
            TableWidgetCell, TableWidgetRow, TableStyle, TableStyleType,
            ColumnStyles, StyleMap,
        };
        use ratatui::style::{Modifier, Style};
        use crate::tabs::trackings_state::{format_duration, DisplayRow, CondensedDisplayRow, TrackingGrouping};

        let t = &*self.theme;
        let ts = &self.state;
        let grouped = ts.grouping != TrackingGrouping::None;
        let has_total_col = grouped;
        let active_cols = self.active_columns(column_config);
        let mut all_cols = active_cols.clone();
        if has_total_col { all_cols.push("total".to_string()); }
        let num_cols = all_cols.len();
        let is_condensed = ts.sub_view == TrackingsSubView::Condensed;

        // Fixed widths for everything except `taskpath`, which is sized
        // dynamically below.
        let fixed_col_width = |id: &str| -> usize {
            match id {
                "marker" => 2,
                "task" => 30,
                "started" | "ended" => 17,
                "duration" | "own" | "cumulated" | "total" => 10,
                "links" => 2,
                _ => 10,
            }
        };

        // Dynamic taskpath width: take what the longest visible path needs,
        // capped by the available room (table budget minus all other column
        // widths and the separators between them). Floor at the header
        // label width so the column is never narrower than its title.
        const MAX_TABLE_WIDTH: usize = 200;
        const COL_SEPARATOR_WIDTH: usize = 2; // matches the "  " passed to set_data
        let taskpath_header_w = "Taskpath".chars().count();
        let taskpath_w = if all_cols.iter().any(|c| c == "taskpath") {
            let other_total: usize = all_cols.iter()
                .filter(|c| c.as_str() != "taskpath")
                .map(|c| fixed_col_width(c))
                .sum();
            let sep_total = num_cols.saturating_sub(1) * COL_SEPARATOR_WIDTH;
            let available = MAX_TABLE_WIDTH
                .saturating_sub(other_total)
                .saturating_sub(sep_total);

            let sep_chars = self.taskpath_separator.chars().count();
            // Layout: leading separator + parts joined by separators.
            // Empty path → just the leading separator.
            let path_natural = |path: &[String]| -> usize {
                let parts: usize = path.iter().map(|s| s.chars().count()).sum();
                let leading = sep_chars;
                let inter = path.len().saturating_sub(1) * sep_chars;
                parts + leading + inter
            };
            let mut max_natural = taskpath_header_w;
            if is_condensed {
                for cr in &ts.condensed_rows {
                    if let CondensedDisplayRow::Entry { task_path, .. } = cr {
                        max_natural = max_natural.max(path_natural(task_path));
                    }
                }
            } else if grouped {
                for dr in &ts.display_rows {
                    if let DisplayRow::Entry { row_idx, .. } = dr {
                        max_natural = max_natural.max(path_natural(&ts.rows[*row_idx].task_path));
                    }
                }
            } else {
                for &idx in &ts.filtered_indices {
                    max_natural = max_natural.max(path_natural(&ts.rows[idx].task_path));
                }
            }
            max_natural.min(available).max(taskpath_header_w)
        } else {
            taskpath_header_w
        };

        let col_width = |id: &str| -> usize {
            if id == "taskpath" { taskpath_w } else { fixed_col_width(id) }
        };

        let truncate_left = |s: &str, max: usize| -> String {
            if s.chars().count() > max {
                let t: String = s.chars().take(max - 1).collect();
                format!("{t}…")
            } else {
                format!("{:<max$}", s, max = max)
            }
        };

        let header_labels: std::collections::HashMap<&str, &str> = [
            ("marker", "⏱"), ("task", "Task"), ("taskpath", "Taskpath"),
            ("started", "Started"), ("ended", "Ended"),
            ("duration", "Duration"), ("total", "Total"),
            ("links", "🔗"),
        ].into_iter().collect();

        let header_cells: Vec<TableWidgetCell> = all_cols.iter().map(|id| {
            let w = col_width(id);
            let id_str = id.as_str();
            let label = header_labels.get(id_str).unwrap_or(&id_str);
            if id == "task" || id == "taskpath" || id == "marker" || id == "started" || id == "ended" || id == "links" {
                TableWidgetCell::plain(format!("{:<w$}", label, w = w))
            } else {
                TableWidgetCell::plain(format!("{:>w$}", label, w = w))
            }
        }).collect();

        // Style index for the taskpath separator span. Kept in sync with
        // the `StyleMap::new(...)` call further down.
        const STYLE_TASKPATH_SEPARATOR: usize = 1;

        let separator = self.taskpath_separator.clone();
        let taskpath_max_w = col_width("taskpath");

        // Build the inline-styled segments for the `taskpath` cell:
        // always leads with a separator (so root tasks render as just "/"),
        // then parent segments joined by separators. Text segments use the
        // cell default style, separators carry STYLE_TASKPATH_SEPARATOR.
        // Long paths are truncated with `…` and padded to column width with
        // default-styled spaces.
        let build_taskpath_segments = |path: &[String]| -> Vec<(String, Option<usize>)> {
            let mut segs: Vec<(String, Option<usize>)> = Vec::new();
            let mut used: usize = 0;
            let sep_chars = separator.chars().count();
            // Leading separator
            if used + sep_chars <= taskpath_max_w {
                segs.push((separator.clone(), Some(STYLE_TASKPATH_SEPARATOR)));
                used += sep_chars;
            }
            for (i, part) in path.iter().enumerate() {
                if i > 0 {
                    if used + sep_chars > taskpath_max_w { break; }
                    segs.push((separator.clone(), Some(STYLE_TASKPATH_SEPARATOR)));
                    used += sep_chars;
                }
                let part_chars = part.chars().count();
                if used + part_chars <= taskpath_max_w {
                    segs.push((part.clone(), None));
                    used += part_chars;
                } else {
                    let remaining = taskpath_max_w.saturating_sub(used);
                    if remaining > 0 {
                        let truncated: String = part.chars().take(remaining.saturating_sub(1)).collect();
                        segs.push((format!("{truncated}…"), None));
                        used = taskpath_max_w;
                    }
                    break;
                }
            }
            if used < taskpath_max_w {
                segs.push((" ".repeat(taskpath_max_w - used), None));
            }
            segs
        };

        // Normal mode rows have a stable `tracking_id` → check
        // `tracking/<id>`. Condensed/grouped-by-task rows aggregate
        // multiple trackings under a single task, so they fall back to
        // `tasks/<task_id>` instead — surfacing whether the underlying
        // task is linked.
        let build_entry_cells = |marker_active: bool, desc: &str, path: &[String],
            started: Option<&str>, ended: Option<&str>,
            duration: Option<&str>, group_total: Option<&str>,
            tracking_id: Option<uuid::Uuid>,
            task_id_fallback: Option<uuid::Uuid>|
        -> Vec<TableWidgetCell> {
            all_cols.iter().map(|id| {
                match id.as_str() {
                    "marker" => {
                        let m = if marker_active { "⏱ " } else { "  " };
                        TableWidgetCell::plain(m.to_string())
                    }
                    "task" => TableWidgetCell::plain(truncate_left(desc, col_width("task"))),
                    "taskpath" => TableWidgetCell::from_segments(build_taskpath_segments(path)),
                    "started" => TableWidgetCell::plain(format!("{:<17}", started.unwrap_or(""))),
                    "ended" => TableWidgetCell::plain(format!("{:<17}", ended.unwrap_or(""))),
                    "duration" => TableWidgetCell::plain(format!("{:>10}", duration.unwrap_or(""))),
                    "total" => TableWidgetCell::plain(format!("{:>10}", group_total.unwrap_or(""))),
                    "links" => {
                        let has = tracking_id
                            .map(|tid| link_refs.contains(&format!("tracking/{tid}")))
                            .unwrap_or(false)
                            || task_id_fallback
                                .map(|t| link_refs.contains(&format!("tasks/{t}")))
                                .unwrap_or(false);
                        let icon = if has { "🔗" } else { " " };
                        TableWidgetCell::plain(format!("{:<2}", icon))
                    }
                    _ => TableWidgetCell::plain(String::new()),
                }
            }).collect()
        };

        let rows: Vec<TableWidgetRow> = if is_condensed {
            ts.condensed_rows.iter().map(|cr| match cr {
                CondensedDisplayRow::GroupHeader { label, .. } => {
                    let cells = vec![
                        TableWidgetCell::grouped(format!("── {} ", label), num_cols - 1).with_style(0),
                        TableWidgetCell::plain(format!("{:>10}", "")),
                    ];
                    TableWidgetRow::new(cells).not_selectable()
                }
                CondensedDisplayRow::Entry { task_id, task_description, task_path, duration, active, group_total, .. } => {
                    let dur = format_duration(*duration);
                    let total = group_total.map(|d| format_duration(d));
                    let cells = build_entry_cells(
                        *active, task_description, task_path,
                        None, None, Some(&dur),
                        total.as_deref(),
                        None,
                        Some(*task_id),
                    );
                    TableWidgetRow::new(cells)
                }
            }).collect()
        } else if grouped {
            ts.display_rows.iter().map(|dr| match dr {
                DisplayRow::GroupHeader { label, .. } => {
                    let cells = vec![
                        TableWidgetCell::grouped(format!("── {} ", label), num_cols - 1).with_style(0),
                        TableWidgetCell::plain(format!("{:>10}", "")),
                    ];
                    TableWidgetRow::new(cells).not_selectable()
                }
                DisplayRow::Entry { row_idx, group_total } => {
                    let row = &ts.rows[*row_idx];
                    let started = row.started_local().format("%Y-%m-%d %H:%M").to_string();
                    let ended = row.ended_local()
                        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_else(|| "running".to_string());
                    let dur = row.duration_display();
                    let total = group_total.map(|d| format_duration(d));
                    let cells = build_entry_cells(
                        row.active, &row.task_description, &row.task_path,
                        Some(&started), Some(&ended), Some(&dur),
                        total.as_deref(),
                        Some(row.id),
                        Some(row.task_id),
                    );
                    TableWidgetRow::new(cells)
                }
            }).collect()
        } else {
            ts.filtered_indices.iter().map(|&idx| {
                let row = &ts.rows[idx];
                let started = row.started_local().format("%Y-%m-%d %H:%M").to_string();
                let ended = row.ended_local()
                    .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "running".to_string());
                let dur = row.duration_display();
                let cells = build_entry_cells(
                    row.active, &row.task_description, &row.task_path,
                    Some(&started), Some(&ended), Some(&dur),
                    None,
                    Some(row.id),
                    Some(row.task_id),
                );
                TableWidgetRow::new(cells)
            }).collect()
        };

        let total = ts.total_duration();
        let total_str = format_duration(total);
        let separator_line = "─".repeat(total_str.chars().count());

        // Compute earliest start and latest end across filtered trackings.
        let earliest_start = ts.filtered_indices.iter()
            .map(|&i| ts.rows[i].started_local())
            .min();
        let has_running = ts.filtered_indices.iter().any(|&i| ts.rows[i].active);
        let latest_end = if has_running {
            None // "running"
        } else {
            ts.filtered_indices.iter()
                .filter_map(|&i| ts.rows[i].ended_local())
                .max()
        };
        let earliest_str = earliest_start
            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default();
        let latest_str = if has_running {
            "running".to_string()
        } else {
            latest_end
                .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default()
        };

        let footer_sep_cells: Vec<TableWidgetCell> = all_cols.iter().enumerate().map(|(i, id)| {
            let w = col_width(id);
            if i == num_cols - 1 {
                TableWidgetCell::plain(format!("{:>w$}", separator_line, w = w))
            } else {
                TableWidgetCell::plain(" ".repeat(w))
            }
        }).collect();
        let footer_total_cells: Vec<TableWidgetCell> = all_cols.iter().enumerate().map(|(i, id)| {
            let w = col_width(id);
            match id.as_str() {
                "started" => TableWidgetCell::plain(format!("{:<w$}", earliest_str, w = w)),
                "ended" => TableWidgetCell::plain(format!("{:<w$}", latest_str, w = w)),
                _ if i == num_cols - 1 => TableWidgetCell::plain(format!("{:>w$}", total_str, w = w)),
                _ => TableWidgetCell::plain(" ".repeat(w)),
            }
        }).collect();

        use crate::tabs::columns::{tracking_column_meta, resolve_color};
        let col_style_list: Vec<Style> = all_cols.iter().map(|id| {
            let fg = tracking_column_meta(id)
                .map(|m| resolve_color(m.color_key, t))
                .unwrap_or(t.accent());
            Style::default().fg(fg)
        }).collect();

        // Index 0 = accent (used by group-header span); index 1 =
        // taskpath separator (referenced via STYLE_TASKPATH_SEPARATOR).
        let style_map = StyleMap::new(vec![
            Style::default().fg(t.accent()),
            Style::default()
                .fg(t.taskpath_separator())
                .add_modifier(Modifier::BOLD),
        ]);

        let table_style = TableStyle::new()
            .set_style(TableStyleType::Header, Style::default().bg(t.surface()))
            .set_style(TableStyleType::Row, Style::default().fg(t.text_med()).bg(t.bg()))
            .set_style(TableStyleType::RowSelected, Style::default().fg(t.text_high()).bg(t.surface_2()));

        self.table.set_data(
            rows, vec![],
            vec![TableWidgetRow::new(header_cells).not_selectable()],
            vec![
                TableWidgetRow::new(footer_sep_cells).not_selectable(),
                TableWidgetRow::new(footer_total_cells).not_selectable(),
            ],
            ColumnStyles::new(col_style_list), table_style, style_map, "  ",
        );
    }

    // ── Bar hints ────────────────────────────────────────────────────

    pub fn action_bar_hints(&self, _sub_view: TrackingsSubView) -> Vec<BarHint> {
        let ckb = &self.keybindings.common;
        let tkb = &self.keybindings.trackings;
        vec![
            (ckb.label(&CommonAction::FuzzyFilterOpen), "Fuzzy Filter".into()),
            (ckb.label(&CommonAction::SavedFilterSelect), "queries".into()),
            (tkb.label(&TrackingsAction::TrackingScriptRun), "scripts".into()),
            (ckb.label(&CommonAction::TrackingToggle), "track".into()),
        ]
    }

    pub fn status_bar_hints(&self, _sub_view: TrackingsSubView) -> Vec<BarHint> {
        let ckb = &self.keybindings.common;
        let tkb = &self.keybindings.trackings;
        vec![
            (ckb.label(&CommonAction::SavedFilterSelect), "queries".into()),
            (tkb.label(&TrackingsAction::TrackingGroup), "group".into()),
            (ckb.label(&CommonAction::ColumnConfig), "columns".into()),
            (tkb.label(&TrackingsAction::TrackingOrderToggle), "order".into()),
            (tkb.label(&TrackingsAction::TrackingCondensedToggle), "condensed".into()),
            (tkb.label(&TrackingsAction::TrackingTreeToggle), "tree".into()),
            (tkb.label(&TrackingsAction::TrackingDelete), "delete".into()),
            (tkb.label(&TrackingsAction::TrackingRestore), "restore".into()),
            (tkb.label(&TrackingsAction::TrackingRestoreAll), "restore all".into()),
        ]
    }

    /// Handle a key event. `forest` is passed from App for tree mode.
    pub fn handle_key(
        &mut self,
        key: &str,
        forest: Option<&TaskForest>,
    ) -> SubViewMessage {
        let ckb = &self.keybindings.common;
        let tkb = &self.keybindings.trackings;

        // --- Common navigation ---
        if ckb.bindings.get(&CommonAction::ListNext).map_or(false, |b| b.matches(key)) {
            self.table.handle_nav(Cmd::Move(Direction::Down));
            return SubViewMessage::SelectionChanged(None);
        }
        if ckb.bindings.get(&CommonAction::ListPrev).map_or(false, |b| b.matches(key)) {
            self.table.handle_nav(Cmd::Move(Direction::Up));
            return SubViewMessage::SelectionChanged(None);
        }
        if ckb.bindings.get(&CommonAction::ListFirst).map_or(false, |b| b.matches(key)) {
            self.table.handle_nav(Cmd::GoTo(Position::Begin));
            return SubViewMessage::SelectionChanged(None);
        }
        if ckb.bindings.get(&CommonAction::ListLast).map_or(false, |b| b.matches(key)) {
            self.table.handle_nav(Cmd::GoTo(Position::End));
            return SubViewMessage::SelectionChanged(None);
        }

        // --- Scroll ---
        if ckb.bindings.get(&CommonAction::ScrollHalfUp).map_or(false, |b| b.matches(key)) {
            let n = (self.table.visible_rows() / 2).max(1) as isize;
            self.table.scroll_by(-n);
            return SubViewMessage::SelectionChanged(None);
        }
        if ckb.bindings.get(&CommonAction::ScrollHalfDown).map_or(false, |b| b.matches(key)) {
            let n = (self.table.visible_rows() / 2).max(1) as isize;
            self.table.scroll_by(n);
            return SubViewMessage::SelectionChanged(None);
        }
        if ckb.bindings.get(&CommonAction::ScrollPageUp).map_or(false, |b| b.matches(key)) {
            let n = self.table.visible_rows().max(1) as isize;
            self.table.scroll_by(-n);
            return SubViewMessage::SelectionChanged(None);
        }
        if ckb.bindings.get(&CommonAction::ScrollPageDown).map_or(false, |b| b.matches(key)) {
            let n = self.table.visible_rows().max(1) as isize;
            self.table.scroll_by(n);
            return SubViewMessage::SelectionChanged(None);
        }

        // --- Fuzzy ---
        if ckb.bindings.get(&CommonAction::FuzzyFilterOpen).map_or(false, |b| b.matches(key)) {
            self.state.fuzzy_open();
            return SubViewMessage::FuzzyStateChanged {
                active: true,
                query: self.state.fuzzy_query.clone(),
                cursor: self.state.fuzzy_cursor,
            };
        }

        // --- Tracking toggle ---
        if ckb.bindings.get(&CommonAction::TrackingToggle).map_or(false, |b| b.matches(key)) {
            let selected = self.table.selected_row();
            if let Some(task_id) = self.state.task_id_at(selected) {
                return SubViewMessage::Request(ViewRequest::ToggleTracking(task_id));
            }
        }

        // --- Popups ---
        // SavedFilterSelect (`q`) is handled at the App level (opens the
        // unified query menu component owned by TrackingsView).
        if ckb.bindings.get(&CommonAction::ColumnConfig).map_or(false, |b| b.matches(key)) {
            return SubViewMessage::Request(ViewRequest::OpenColumnConfig);
        }

        // --- Trackings-only actions (handled internally) ---
        if tkb.bindings.get(&TrackingsAction::TrackingGroup).map_or(false, |b| b.matches(key)) {
            return SubViewMessage::Request(ViewRequest::OpenTrackingGroupPopup);
        }
        if tkb.bindings.get(&TrackingsAction::TrackingOrderToggle).map_or(false, |b| b.matches(key)) {
            self.state.toggle_order();
            self.rebuild_table();
            return SubViewMessage::SelectionChanged(None);
        }
        if tkb.bindings.get(&TrackingsAction::TrackingCondensedToggle).map_or(false, |b| b.matches(key)) {
            let current = self.table.selected_row();
            let new_idx = self.state.toggle_condensed(current);
            self.rebuild_table();
            self.table.set_selected(new_idx);
            return SubViewMessage::SelectionChanged(None);
        }
        if tkb.bindings.get(&TrackingsAction::TrackingTreeToggle).map_or(false, |b| b.matches(key)) {
            if let Some(forest) = forest {
                let current = self.table.selected_row();
                let new_idx = self.state.toggle_tree_mode(current, forest);
                self.rebuild_table();
                self.table.set_selected(new_idx);
            }
            return SubViewMessage::SelectionChanged(None);
        }
        if tkb.bindings.get(&TrackingsAction::TrackingNormalToggle).map_or(false, |b| b.matches(key)) {
            if self.state.sub_view != TrackingsSubView::Normal {
                self.state.sub_view = TrackingsSubView::Normal;
                self.rebuild_table();
            }
            return SubViewMessage::SelectionChanged(None);
        }
        if tkb.bindings.get(&TrackingsAction::TrackingScriptRun).map_or(false, |b| b.matches(key)) {
            return SubViewMessage::Request(ViewRequest::OpenScriptMenuForTrackings);
        }
        if tkb.bindings.get(&TrackingsAction::TrackingDelete).map_or(false, |b| b.matches(key)) {
            return SubViewMessage::Request(ViewRequest::DeleteTracking);
        }
        if tkb.bindings.get(&TrackingsAction::TrackingRestore).map_or(false, |b| b.matches(key)) {
            return SubViewMessage::Request(ViewRequest::RestoreTracking);
        }
        if tkb.bindings.get(&TrackingsAction::TrackingRestoreAll).map_or(false, |b| b.matches(key)) {
            return SubViewMessage::Request(ViewRequest::RestoreAllTrackings);
        }

        SubViewMessage::Unhandled
    }
}

// ── Tracking group popup ─────────────────────────────────────────────

impl TrackingsView {
    pub fn has_group_popup(&self) -> bool {
        self.group_popup.is_some()
    }

    pub fn open_group_popup(&mut self) {
        let current = self.state.grouping;
        let idx = TrackingGrouping::ALL.iter().position(|&g| g == current).unwrap_or(0);
        self.group_popup = Some(idx);
    }

    /// Handle a key while the grouping popup is open.
    /// Returns messages for App (e.g. save grouping).
    pub fn handle_group_popup_key(&mut self, key: &str) -> Vec<SubViewMessage> {
        let Some(ref mut cursor) = self.group_popup else {
            return vec![];
        };
        let options = TrackingGrouping::ALL;

        if self.keybindings.common.bindings.get(&CommonAction::ListPrev).map_or(false, |b| b.matches(key))
            || key == "up"
        {
            if *cursor > 0 { *cursor -= 1; }
        } else if self.keybindings.common.bindings.get(&CommonAction::ListNext).map_or(false, |b| b.matches(key))
            || key == "down"
        {
            if *cursor + 1 < options.len() { *cursor += 1; }
        } else if key == " " || key == "enter" {
            let selected = options[*cursor];
            let cur = self.table.selected_row();
            let new_idx = self.state.set_grouping(selected, cur);
            self.rebuild_table();
            self.table.set_selected(new_idx);
            self.group_popup = None;
            return vec![SubViewMessage::Request(ViewRequest::SaveTrackingGrouping(selected.label().to_string()))];
        } else if self.keybindings.common.bindings.get(&CommonAction::FormClose).map_or(false, |b| b.matches(key)) {
            self.group_popup = None;
        } else if key.chars().count() == 1 {
            let ch = key.chars().next().unwrap();
            for (i, opt) in options.iter().enumerate() {
                if opt.shortcut() == Some(ch) {
                    let selected = options[i];
                    let cur = self.table.selected_row();
                    let new_idx = self.state.set_grouping(selected, cur);
                    self.rebuild_table();
                    self.table.set_selected(new_idx);
                    self.group_popup = None;
                    return vec![SubViewMessage::Request(ViewRequest::SaveTrackingGrouping(selected.label().to_string()))];
                }
            }
        }
        vec![]
    }
}

// ── Searchable trait ──────────────────────────────────────────────────

impl TrackingsView {
    /// Build (row_index, description) pairs for search matching.
    fn search_descriptions(&self) -> Vec<(usize, String)> {
        let ts = &self.state;
        match ts.sub_view {
            TrackingsSubView::Normal => {
                if ts.grouping == TrackingGrouping::None {
                    ts.filtered_indices.iter().enumerate()
                        .map(|(i, &idx)| (i, ts.rows[idx].task_description.clone()))
                        .collect()
                } else {
                    ts.display_rows.iter().enumerate()
                        .filter_map(|(i, dr)| {
                            if let DisplayRow::Entry { row_idx, .. } = dr {
                                Some((i, ts.rows[*row_idx].task_description.clone()))
                            } else {
                                None
                            }
                        })
                        .collect()
                }
            }
            TrackingsSubView::Condensed => {
                ts.condensed_rows.iter().enumerate()
                    .filter_map(|(i, cr)| {
                        if let CondensedDisplayRow::Entry { task_description, .. } = cr {
                            Some((i, task_description.clone()))
                        } else {
                            None
                        }
                    })
                    .collect()
            }
            TrackingsSubView::Tree => {
                ts.tree_rows.iter().enumerate()
                    .filter_map(|(i, tr)| {
                        if let TreeDisplayRow::Entry { task_description, .. } = tr {
                            Some((i, task_description.clone()))
                        } else {
                            None
                        }
                    })
                    .collect()
            }
        }
    }
}

impl Searchable for TrackingsView {
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
        self.search.close();
    }

    fn search_clear(&mut self) {
        self.search.clear();
    }

    fn search_handle_key(&mut self, key: &str) -> SearchKeyResult {
        let result = self.search.handle_key(key);
        if matches!(result, SearchKeyResult::QueryChanged) {
            let descs = self.search_descriptions();
            let refs: Vec<(usize, &str)> = descs.iter().map(|(i, s)| (*i, s.as_str())).collect();
            self.search.update_matches(&refs);
            if let Some(row) = self.search.first_match() {
                self.table.set_selected(row);
            }
        }
        result
    }

    fn search_jump(&mut self, direction: isize) {
        if let Some(row) = self.search.jump(direction) {
            self.table.set_selected(row);
        }
    }
}

impl HasCmdline for TrackingsView {
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

impl Component for TrackingsView {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        self.table.view(frame, area);
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

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        self.table.perform(cmd)
    }
}
