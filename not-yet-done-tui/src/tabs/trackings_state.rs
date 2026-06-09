//! Trackings tab state: data, navigation, fuzzy filter.

use std::collections::{HashMap, HashSet};
use chrono::{DateTime, Datelike, Duration, Local, Utc};
use uuid::Uuid;

use crate::tabs::TrackingsSubView;
use crate::ui::tasks::forest::{TaskForest, TaskItem, LocalUuid};

/// Format a duration as `H:MM:SS`, `MM:SS`, or `SS` depending on magnitude.
/// Hours can exceed 24 (accumulated time).
pub fn format_duration(d: Duration) -> String {
    let total_secs = d.num_seconds().max(0);
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else if m > 0 {
        format!("{m:02}:{s:02}")
    } else {
        format!("{s:02}")
    }
}

/// How to group trackings for summary display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackingGrouping {
    None,
    Day,
    Week,
    Month,
    Year,
}

impl TrackingGrouping {
    pub const ALL: &'static [TrackingGrouping] = &[
        TrackingGrouping::None,
        TrackingGrouping::Day,
        TrackingGrouping::Week,
        TrackingGrouping::Month,
        TrackingGrouping::Year,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Self::None => "No grouping",
            Self::Day => "Day",
            Self::Week => "Week",
            Self::Month => "Month",
            Self::Year => "Year",
        }
    }

    /// Shortcut character: first letter of the label, lowercased.
    pub fn shortcut(&self) -> Option<char> {
        self.label().chars().next().map(|c| c.to_lowercase().next().unwrap_or(c))
    }
}

/// A display row — either a group header or a tracking entry.
#[derive(Debug, Clone)]
pub enum DisplayRow {
    /// Group header: label + total duration of the group.
    GroupHeader { label: String, total: Duration },
    /// A tracking entry (index into `rows`).
    Entry { row_idx: usize, group_total: Option<Duration> },
}

/// A tree display row — task in tree structure with cumulated times.
#[derive(Debug, Clone)]
pub enum TreeDisplayRow {
    GroupHeader { label: String, total: Duration },
    Entry {
        task_id: Uuid,
        task_description: String,
        /// Own duration (only this task's trackings in the group).
        own_duration: Duration,
        /// Cumulated duration (own + all descendants).
        cumulated_duration: Duration,
        active: bool,
        tree_cell: String,
        connector_chars: usize,
        group_total: Option<Duration>,
    },
}

/// A condensed display row — one per task per group.
#[derive(Debug, Clone)]
pub enum CondensedDisplayRow {
    GroupHeader { label: String, total: Duration },
    Entry {
        task_id: Uuid,
        task_description: String,
        /// Path segments from root → self (inclusive); the view joins them
        /// with the user-configured separator at render time.
        task_path: Vec<String>,
        duration: Duration,
        active: bool,
        group_total: Option<Duration>,
    },
}

/// A single row in the trackings table, pre-joined with task description.
#[derive(Debug, Clone)]
pub struct TrackingRow {
    pub id: Uuid,
    pub task_id: Uuid,
    pub task_description: String,
    /// Path segments from root → self (inclusive). The view interleaves
    /// these with the user-configured separator at render time so the
    /// separator can be styled independently.
    pub task_path: Vec<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration: Duration,
    pub active: bool,
}

impl TrackingRow {
    pub fn started_local(&self) -> DateTime<Local> {
        self.started_at.with_timezone(&Local)
    }

    pub fn ended_local(&self) -> Option<DateTime<Local>> {
        self.ended_at.map(|dt| dt.with_timezone(&Local))
    }

    pub fn duration_display(&self) -> String {
        format_duration(self.duration)
    }

    /// Fuzzy-matchable text for filtering.
    pub fn filter_text(&self) -> String {
        format!(
            "{} {} {}",
            self.task_description,
            self.started_local().format("%Y-%m-%d %H:%M"),
            self.duration_display(),
        )
    }
}

pub struct TrackingsState {
    pub rows: Vec<TrackingRow>,
    pub filtered_indices: Vec<usize>,
    pub display_rows: Vec<DisplayRow>,

    pub fuzzy_active: bool,
    pub fuzzy_query: String,
    pub fuzzy_cursor: usize,

    pub reversed: bool,
    pub sub_view: TrackingsSubView,
    pub condensed_rows: Vec<CondensedDisplayRow>,
    pub tree_rows: Vec<TreeDisplayRow>,
    /// When toggling Normal→Condensed: remember (task_id, group_label, normal_index).
    condensed_switch_context: Option<(Uuid, Option<String>, usize)>,
    /// When toggling Normal→Tree: remember (task_id, group_label, normal_index).
    tree_switch_context: Option<(Uuid, Option<String>, usize)>,

    /// If set, the next `set_rows` will try to select this tracking ID.
    pub pending_focus_id: Option<Uuid>,

    pub grouping: TrackingGrouping,
    pub load_state: super::LoadState,
}

impl TrackingsState {
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            filtered_indices: Vec::new(),
            display_rows: Vec::new(),
            fuzzy_active: false,
            fuzzy_query: String::new(),
            fuzzy_cursor: 0,
            reversed: false,
            sub_view: TrackingsSubView::Normal,
            condensed_rows: Vec::new(),
            tree_rows: Vec::new(),
            condensed_switch_context: None,
            tree_switch_context: None,
            pending_focus_id: None,
            grouping: TrackingGrouping::None,
            load_state: super::LoadState::Idle,
        }
    }

    /// Set new tracking rows. Returns the suggested selection index
    /// (restoring pending_focus_id if set).
    pub fn set_rows(&mut self, rows: Vec<TrackingRow>) -> Option<usize> {
        self.rows = rows;
        self.refilter();

        let focus_idx = if let Some(focus_id) = self.pending_focus_id.take() {
            self.filtered_indices.iter().position(|&i| self.rows[i].id == focus_id)
        } else {
            None
        };
        self.load_state = super::LoadState::Loaded;
        focus_idx
    }

    pub fn set_load_error(&mut self, msg: String) {
        self.load_state = super::LoadState::Error(msg);
    }

    /// Get the UUID of the tracking entry at the given display index.
    pub fn tracking_id_at(&self, selected: usize) -> Option<Uuid> {
        let row_idx = if self.sub_view != TrackingsSubView::Normal {
            return None; // Condensed/tree rows don't map to individual trackings.
        } else if self.grouping == TrackingGrouping::None {
            self.filtered_indices.get(selected).copied()
        } else {
            match self.display_rows.get(selected) {
                Some(DisplayRow::Entry { row_idx, .. }) => Some(*row_idx),
                _ => None,
            }
        };
        row_idx.and_then(|i| self.rows.get(i)).map(|r| r.id)
    }

    /// Get the task_id at the given display index (works in all modes).
    pub fn task_id_at(&self, selected: usize) -> Option<Uuid> {
        if self.sub_view == TrackingsSubView::Tree {
            match self.tree_rows.get(selected) {
                Some(TreeDisplayRow::Entry { task_id, .. }) => Some(*task_id),
                _ => None,
            }
        } else if self.sub_view == TrackingsSubView::Condensed {
            match self.condensed_rows.get(selected) {
                Some(CondensedDisplayRow::Entry { task_id, .. }) => Some(*task_id),
                _ => None,
            }
        } else if self.grouping == TrackingGrouping::None {
            self.filtered_indices.get(selected)
                .map(|&i| self.rows[i].task_id)
        } else {
            match self.display_rows.get(selected) {
                Some(DisplayRow::Entry { row_idx, .. }) => Some(self.rows[*row_idx].task_id),
                _ => None,
            }
        }
    }

    /// Get the task description at the given display index.
    pub fn task_description_at(&self, selected: usize) -> Option<String> {
        if self.sub_view == TrackingsSubView::Tree {
            match self.tree_rows.get(selected) {
                Some(TreeDisplayRow::Entry { task_description, .. }) => Some(task_description.clone()),
                _ => None,
            }
        } else if self.sub_view == TrackingsSubView::Condensed {
            match self.condensed_rows.get(selected) {
                Some(CondensedDisplayRow::Entry { task_description, .. }) => Some(task_description.clone()),
                _ => None,
            }
        } else if self.grouping == TrackingGrouping::None {
            self.filtered_indices.get(selected)
                .map(|&i| self.rows[i].task_description.clone())
        } else {
            match self.display_rows.get(selected) {
                Some(DisplayRow::Entry { row_idx, .. }) => Some(self.rows[*row_idx].task_description.clone()),
                _ => None,
            }
        }
    }

    /// Check if the entry at the given display index has an active tracking.
    pub fn is_active_at(&self, selected: usize) -> bool {
        if self.sub_view == TrackingsSubView::Tree {
            matches!(self.tree_rows.get(selected), Some(TreeDisplayRow::Entry { active: true, .. }))
        } else if self.sub_view == TrackingsSubView::Condensed {
            matches!(self.condensed_rows.get(selected), Some(CondensedDisplayRow::Entry { active: true, .. }))
        } else if self.grouping == TrackingGrouping::None {
            self.filtered_indices.get(selected)
                .map(|&i| self.rows[i].active)
                .unwrap_or(false)
        } else {
            match self.display_rows.get(selected) {
                Some(DisplayRow::Entry { row_idx, .. }) => self.rows[*row_idx].active,
                _ => false,
            }
        }
    }

    /// Total duration of all filtered trackings.
    pub fn total_duration(&self) -> Duration {
        self.filtered_indices.iter()
            .map(|&i| self.rows[i].duration)
            .fold(Duration::zero(), |acc, d| acc + d)
    }

    /// Set grouping. Takes the current selection index and returns the new one.
    pub fn set_grouping(&mut self, grouping: TrackingGrouping, current_selected: usize) -> usize {
        // Remember which data row is currently selected.
        let prev_row_idx = self.data_row_idx_at(current_selected);

        self.grouping = grouping;
        self.rebuild_display_rows();

        // Restore selection to the same data row in the new layout.
        if let Some(idx) = prev_row_idx {
            self.find_display_index_for_row(idx).unwrap_or(0)
        } else {
            0
        }
    }

    /// Get the data row index (into `self.rows`) at the given display index.
    fn data_row_idx_at(&self, selected: usize) -> Option<usize> {
        if self.grouping == TrackingGrouping::None {
            self.filtered_indices.get(selected).copied()
        } else {
            match self.display_rows.get(selected) {
                Some(DisplayRow::Entry { row_idx, .. }) => Some(*row_idx),
                _ => None,
            }
        }
    }

    /// Find the display_rows / filtered_indices index for a given data row index.
    fn find_display_index_for_row(&self, target_row_idx: usize) -> Option<usize> {
        if self.grouping == TrackingGrouping::None {
            self.filtered_indices.iter().position(|&i| i == target_row_idx)
        } else {
            self.display_rows.iter().position(|dr| {
                matches!(dr, DisplayRow::Entry { row_idx, .. } if *row_idx == target_row_idx)
            })
        }
    }

    /// Build display rows from filtered indices + current grouping.
    pub(crate) fn rebuild_display_rows(&mut self) {
        if self.grouping == TrackingGrouping::None {
            self.display_rows = self.filtered_indices.iter()
                .map(|&idx| DisplayRow::Entry { row_idx: idx, group_total: None })
                .collect();
            return;
        }

        // First pass: collect groups with their indices and sums.
        let mut groups: Vec<(String, Vec<usize>, Duration)> = Vec::new();
        let mut current_key: Option<String> = None;

        for &idx in &self.filtered_indices {
            let row = &self.rows[idx];
            let local = row.started_at.with_timezone(&Local);
            let key = match self.grouping {
                TrackingGrouping::Day => {
                    let iso = local.iso_week();
                    format!("W{:02} {} {}",
                        iso.week(),
                        local.format("%Y-%m-%d"),
                        local.format("%a"))
                }
                TrackingGrouping::Week => {
                    let iso = local.iso_week();
                    format!("W{:02} {}", iso.week(), local.format("%Y"))
                }
                TrackingGrouping::Month => local.format("%Y-%m").to_string(),
                TrackingGrouping::Year => local.format("%Y").to_string(),
                TrackingGrouping::None => unreachable!(),
            };

            if current_key.as_deref() != Some(&key) {
                current_key = Some(key.clone());
                groups.push((key, Vec::new(), Duration::zero()));
            }

            let group = groups.last_mut().unwrap();
            group.1.push(idx);
            group.2 = group.2 + row.duration;
        }

        // Second pass: build display rows.
        let mut result = Vec::new();
        for (label, indices, total) in groups {
            result.push(DisplayRow::GroupHeader { label, total });
            let last_idx = indices.len().saturating_sub(1);
            for (i, row_idx) in indices.into_iter().enumerate() {
                let group_total = if i == last_idx { Some(total) } else { None };
                result.push(DisplayRow::Entry { row_idx, group_total });
            }
        }

        self.display_rows = result;
    }

    // ── Fuzzy filter ─────────────────────────────────────────────────

    pub fn fuzzy_open(&mut self) {
        self.fuzzy_active = true;
        self.fuzzy_cursor = self.fuzzy_query.chars().count();
    }

    pub fn fuzzy_close(&mut self) {
        self.fuzzy_active = false;
        self.refilter();
    }

    pub fn fuzzy_insert(&mut self, c: char) {
        let byte_pos = self.fuzzy_query.char_indices()
            .nth(self.fuzzy_cursor).map(|(i, _)| i)
            .unwrap_or(self.fuzzy_query.len());
        self.fuzzy_query.insert(byte_pos, c);
        self.fuzzy_cursor += 1;
        self.refilter();
    }

    pub fn fuzzy_backspace(&mut self) {
        if self.fuzzy_cursor == 0 || self.fuzzy_query.is_empty() { return; }
        let byte_pos = self.fuzzy_query.char_indices()
            .nth(self.fuzzy_cursor - 1).map(|(i, _)| i)
            .unwrap_or(0);
        self.fuzzy_query.remove(byte_pos);
        self.fuzzy_cursor -= 1;
        self.refilter();
    }

    pub fn fuzzy_cursor_left(&mut self) {
        if self.fuzzy_cursor > 0 { self.fuzzy_cursor -= 1; }
    }

    pub fn fuzzy_cursor_right(&mut self) {
        let max = self.fuzzy_query.chars().count();
        if self.fuzzy_cursor < max { self.fuzzy_cursor += 1; }
    }

    // ── Internal ─────────────────────────────────────────────────────

    /// Toggle condensed mode. Takes the current table selection index and
    /// returns the new selection index to set on the table.
    pub fn toggle_condensed(&mut self, current_selected: usize) -> usize {
        if self.sub_view == TrackingsSubView::Condensed {
            // Condensed → Normal: restore index.
            let current_task_id = self.condensed_selected_task_id(current_selected);
            let current_group = self.condensed_selected_group(current_selected);
            self.sub_view = TrackingsSubView::Normal;
            if let Some((switch_task_id, switch_group, original_idx)) = self.condensed_switch_context.take() {
                if current_task_id == Some(switch_task_id) && current_group == switch_group {
                    original_idx
                } else if let Some(task_id) = current_task_id {
                    self.find_first_tracking_of_task_in_group(task_id, &current_group)
                        .unwrap_or(0)
                } else {
                    0
                }
            } else {
                0
            }
        } else {
            // Normal → Condensed: save context.
            let task_id = self.task_id_at(current_selected);
            let group = self.group_label_for_display_idx(current_selected);
            self.sub_view = TrackingsSubView::Condensed;
            self.rebuild_condensed_rows();
            if let Some(tid) = task_id {
                self.condensed_switch_context = Some((tid, group.clone(), current_selected));
                self.find_condensed_task_in_group(tid, &group).unwrap_or(0)
            } else {
                self.condensed_switch_context = None;
                0
            }
        }
    }

    /// Get task_id of a condensed entry at the given index.
    fn condensed_selected_task_id(&self, selected: usize) -> Option<Uuid> {
        match self.condensed_rows.get(selected) {
            Some(CondensedDisplayRow::Entry { task_id, .. }) => Some(*task_id),
            _ => None,
        }
    }

    /// Get the group label of a condensed entry by walking backwards.
    fn condensed_selected_group(&self, selected: usize) -> Option<String> {
        for i in (0..=selected).rev() {
            if let Some(CondensedDisplayRow::GroupHeader { label, .. }) = self.condensed_rows.get(i) {
                return Some(label.clone());
            }
        }
        None
    }

    /// Get the group label for a display_rows index by walking backwards.
    fn group_label_for_display_idx(&self, idx: usize) -> Option<String> {
        if self.grouping == TrackingGrouping::None {
            return None;
        }
        for i in (0..=idx).rev() {
            if let Some(DisplayRow::GroupHeader { label, .. }) = self.display_rows.get(i) {
                return Some(label.clone());
            }
        }
        None
    }

    /// Find first tracking of a task within a specific group in normal mode.
    /// Returns the display index, or None.
    fn find_first_tracking_of_task_in_group(&self, task_id: Uuid, group: &Option<String>) -> Option<usize> {
        if self.grouping == TrackingGrouping::None {
            for (i, &idx) in self.filtered_indices.iter().enumerate() {
                if self.rows[idx].task_id == task_id {
                    return Some(i);
                }
            }
        } else {
            let mut in_group = group.is_none();
            for (i, dr) in self.display_rows.iter().enumerate() {
                match dr {
                    DisplayRow::GroupHeader { label, .. } => {
                        in_group = Some(label.clone()) == *group;
                    }
                    DisplayRow::Entry { row_idx, .. } if in_group => {
                        if self.rows[*row_idx].task_id == task_id {
                            return Some(i);
                        }
                    }
                    _ => {}
                }
            }
        }
        None
    }

    /// Find a task in condensed rows within a specific group.
    /// Returns the condensed row index, or None.
    fn find_condensed_task_in_group(&self, task_id: Uuid, group: &Option<String>) -> Option<usize> {
        let mut in_group = group.is_none();
        for (i, cr) in self.condensed_rows.iter().enumerate() {
            match cr {
                CondensedDisplayRow::GroupHeader { label, .. } => {
                    in_group = Some(label.clone()) == *group;
                }
                CondensedDisplayRow::Entry { task_id: cid, .. } if in_group => {
                    if *cid == task_id {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        None
    }

    pub(crate) fn rebuild_condensed_rows(&mut self) {
        let grouped = self.grouping != TrackingGrouping::None;
        let mut result = Vec::new();

        if !grouped {
            // No grouping: aggregate all filtered trackings by task (preserve encounter order).
            let mut order: Vec<Uuid> = Vec::new();
            let mut by_task: std::collections::HashMap<Uuid, (String, Vec<String>, Duration, bool)> = std::collections::HashMap::new();
            for &idx in &self.filtered_indices {
                let row = &self.rows[idx];
                if !by_task.contains_key(&row.task_id) {
                    order.push(row.task_id);
                }
                let entry = by_task.entry(row.task_id).or_insert_with(|| {
                    (row.task_description.clone(), row.task_path.clone(), Duration::zero(), false)
                });
                entry.2 = entry.2 + row.duration;
                if row.active { entry.3 = true; }
            }
            let total: Duration = by_task.values().map(|(_, _, d, _)| *d).fold(Duration::zero(), |a, b| a + b);
            let entries: Vec<_> = order.into_iter()
                .filter_map(|id| by_task.remove(&id).map(|v| (id, v)))
                .collect();
            let last_idx = entries.len().saturating_sub(1);
            for (i, (task_id, (desc, path, dur, active))) in entries.into_iter().enumerate() {
                result.push(CondensedDisplayRow::Entry {
                    task_id,
                    task_description: desc,
                    task_path: path,
                    duration: dur,
                    active,
                    group_total: if i == last_idx { Some(total) } else { None },
                });
            }
        } else {
            // Grouped: iterate display_rows, aggregate per group.
            let mut current_group: Option<(String, Duration)> = None;
            let mut group_order: Vec<Uuid> = Vec::new();
            let mut group_tasks: std::collections::HashMap<Uuid, (String, Vec<String>, Duration, bool)> = std::collections::HashMap::new();

            for dr in &self.display_rows {
                match dr {
                    DisplayRow::GroupHeader { label, total } => {
                        // Flush previous group.
                        if let Some((grp_label, grp_total)) = current_group.take() {
                            self.flush_condensed_group(&mut result, &grp_label, grp_total, &mut group_order, &mut group_tasks);
                        }
                        current_group = Some((label.clone(), *total));
                        group_order.clear();
                        group_tasks.clear();
                    }
                    DisplayRow::Entry { row_idx, .. } => {
                        let row = &self.rows[*row_idx];
                        if !group_tasks.contains_key(&row.task_id) {
                            group_order.push(row.task_id);
                        }
                        let entry = group_tasks.entry(row.task_id).or_insert_with(|| {
                            (row.task_description.clone(), row.task_path.clone(), Duration::zero(), false)
                        });
                        entry.2 = entry.2 + row.duration;
                        if row.active { entry.3 = true; }
                    }
                }
            }
            // Flush last group.
            if let Some((grp_label, grp_total)) = current_group.take() {
                self.flush_condensed_group(&mut result, &grp_label, grp_total, &mut group_order, &mut group_tasks);
            }
        }

        self.condensed_rows = result;
    }

    fn flush_condensed_group(
        &self,
        result: &mut Vec<CondensedDisplayRow>,
        label: &str,
        total: Duration,
        order: &mut Vec<Uuid>,
        tasks: &mut std::collections::HashMap<Uuid, (String, Vec<String>, Duration, bool)>,
    ) {
        result.push(CondensedDisplayRow::GroupHeader {
            label: label.to_string(),
            total,
        });
        let entries: Vec<_> = std::mem::take(order).into_iter()
            .filter_map(|id| tasks.remove(&id).map(|v| (id, v)))
            .collect();
        let last_idx = entries.len().saturating_sub(1);
        for (i, (task_id, (desc, path, dur, active))) in entries.into_iter().enumerate() {
            result.push(CondensedDisplayRow::Entry {
                task_id,
                task_description: desc,
                task_path: path,
                duration: dur,
                active,
                group_total: if i == last_idx { Some(total) } else { None },
            });
        }
    }

    // ── Tree mode ─────────────────────────────────────────────────────

    /// Rebuild tree rows from filtered tracking data + task forest.
    pub fn rebuild_tree_rows(&mut self, forest: &TaskForest) {
        let grouped = self.grouping != TrackingGrouping::None;
        let mut result = Vec::new();

        if !grouped {
            let (task_durations, active_tasks) = self.collect_task_durations(&self.filtered_indices);
            let total = task_durations.values().copied().fold(Duration::zero(), |a, b| a + b);
            let folded = self.fold_with_forest(forest, &task_durations, &active_tasks);
            let last_idx = folded.len().saturating_sub(1);
            for (i, node) in folded.into_iter().enumerate() {
                let uuid = node.id.0;
                result.push(TreeDisplayRow::Entry {
                    task_id: uuid,
                    task_description: String::new(), // filled below
                    own_duration: task_durations.get(&uuid).copied().unwrap_or(Duration::zero()),
                    cumulated_duration: node.result,
                    active: active_tasks.contains(&uuid),
                    tree_cell: node.tree_cell,
                    connector_chars: node.connector_chars,
                    group_total: if i == last_idx { Some(total) } else { None },
                });
            }
        } else {
            // Process each group from display_rows.
            let mut current_group: Option<(String, Duration, Vec<usize>)> = None;

            for dr in &self.display_rows {
                match dr {
                    DisplayRow::GroupHeader { label, total } => {
                        if let Some((grp_label, grp_total, indices)) = current_group.take() {
                            self.flush_tree_group(&mut result, forest, &grp_label, grp_total, &indices);
                        }
                        current_group = Some((label.clone(), *total, Vec::new()));
                    }
                    DisplayRow::Entry { row_idx, .. } => {
                        if let Some((_, _, ref mut indices)) = current_group {
                            indices.push(*row_idx);
                        }
                    }
                }
            }
            if let Some((grp_label, grp_total, indices)) = current_group.take() {
                self.flush_tree_group(&mut result, forest, &grp_label, grp_total, &indices);
            }
        }

        // Fill descriptions from forest.
        for row in &mut result {
            if let TreeDisplayRow::Entry { task_id, task_description, .. } = row {
                if let Some(item) = forest.0.find_item(&LocalUuid(*task_id)) {
                    *task_description = item.0.description.clone();
                }
            }
        }

        self.tree_rows = result;
    }

    fn flush_tree_group(
        &self,
        result: &mut Vec<TreeDisplayRow>,
        forest: &TaskForest,
        label: &str,
        total: Duration,
        row_indices: &[usize],
    ) {
        result.push(TreeDisplayRow::GroupHeader { label: label.to_string(), total });
        let (task_durations, active_tasks) = self.collect_task_durations(row_indices);
        let folded = self.fold_with_forest(forest, &task_durations, &active_tasks);
        let last_idx = folded.len().saturating_sub(1);
        for (i, node) in folded.into_iter().enumerate() {
            let uuid = node.id.0;
            result.push(TreeDisplayRow::Entry {
                task_id: uuid,
                task_description: String::new(),
                own_duration: task_durations.get(&uuid).copied().unwrap_or(Duration::zero()),
                cumulated_duration: node.result,
                active: active_tasks.contains(&uuid),
                tree_cell: node.tree_cell,
                connector_chars: node.connector_chars,
                group_total: if i == last_idx { Some(total) } else { None },
            });
        }
    }

    /// Collect task_id → total Duration and active task_ids from a set of row indices.
    fn collect_task_durations(&self, indices: &[usize]) -> (HashMap<Uuid, Duration>, HashSet<Uuid>) {
        let mut durations: HashMap<Uuid, Duration> = HashMap::new();
        let mut active: HashSet<Uuid> = HashSet::new();
        for &idx in indices {
            let row = &self.rows[idx];
            let entry = durations.entry(row.task_id).or_insert(Duration::zero());
            *entry = *entry + row.duration;
            if row.active { active.insert(row.task_id); }
        }
        (durations, active)
    }

    /// Extract subtrees and fold with duration accumulation.
    fn fold_with_forest(
        &self,
        forest: &TaskForest,
        task_durations: &HashMap<Uuid, Duration>,
        _active_tasks: &HashSet<Uuid>,
    ) -> Vec<not_yet_done_forest::FoldedNode<LocalUuid, Duration>> {
        let interesting: HashSet<LocalUuid> = task_durations.keys()
            .map(|id| LocalUuid(*id))
            .collect();
        let ghosts = forest.0.extract_subtrees(&interesting);
        not_yet_done_forest::fold_ghost_trees(&ghosts, &|item: &TaskItem, children: Vec<&Duration>| {
            let own = task_durations.get(&item.0.id).copied().unwrap_or(Duration::zero());
            own + children.iter().copied().fold(Duration::zero(), |a, b| a + *b)
        })
    }

    /// Toggle tree mode. Takes current selection, returns new selection index.
    pub fn toggle_tree_mode(&mut self, current_selected: usize, forest: &TaskForest) -> usize {
        if self.sub_view == TrackingsSubView::Tree {
            // Tree → Normal: restore index.
            let current_task_id = self.tree_selected_task_id(current_selected);
            let current_group = self.tree_selected_group(current_selected);
            self.sub_view = TrackingsSubView::Normal;
            if let Some((switch_task_id, switch_group, original_idx)) = self.tree_switch_context.take() {
                if current_task_id == Some(switch_task_id) && current_group == switch_group {
                    original_idx
                } else if let Some(task_id) = current_task_id {
                    self.find_first_tracking_of_task_in_group(task_id, &current_group)
                        .unwrap_or(0)
                } else {
                    0
                }
            } else {
                0
            }
        } else {
            // Normal → Tree: save context + build tree.
            let task_id = self.task_id_at(current_selected);
            let group = self.group_label_for_display_idx(current_selected);
            self.sub_view = TrackingsSubView::Tree;
            self.rebuild_tree_rows(forest);
            if let Some(tid) = task_id {
                self.tree_switch_context = Some((tid, group.clone(), current_selected));
                self.find_tree_task_in_group(tid, &group).unwrap_or(0)
            } else {
                self.tree_switch_context = None;
                0
            }
        }
    }

    fn tree_selected_task_id(&self, selected: usize) -> Option<Uuid> {
        match self.tree_rows.get(selected) {
            Some(TreeDisplayRow::Entry { task_id, .. }) => Some(*task_id),
            _ => None,
        }
    }

    fn tree_selected_group(&self, selected: usize) -> Option<String> {
        for i in (0..=selected).rev() {
            if let Some(TreeDisplayRow::GroupHeader { label, .. }) = self.tree_rows.get(i) {
                return Some(label.clone());
            }
        }
        None
    }

    fn find_tree_task_in_group(&self, task_id: Uuid, group: &Option<String>) -> Option<usize> {
        let mut in_group = group.is_none();
        for (i, tr) in self.tree_rows.iter().enumerate() {
            match tr {
                TreeDisplayRow::GroupHeader { label, .. } => {
                    in_group = Some(label.clone()) == *group;
                }
                TreeDisplayRow::Entry { task_id: tid, .. } if in_group => {
                    if *tid == task_id {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Widget row count for the current mode.
    pub fn widget_row_count(&self) -> usize {
        if self.sub_view == TrackingsSubView::Tree {
            self.tree_rows.len()
        } else if self.sub_view == TrackingsSubView::Condensed {
            self.condensed_rows.len()
        } else if self.grouping == TrackingGrouping::None {
            self.filtered_indices.len()
        } else {
            self.display_rows.len()
        }
    }

    pub fn toggle_order(&mut self) {
        self.reversed = !self.reversed;
        self.refilter();
    }

    pub(crate) fn refilter(&mut self) {
        if self.fuzzy_query.is_empty() {
            self.filtered_indices = (0..self.rows.len()).collect();
        } else {
            let q = self.fuzzy_query.to_lowercase();
            self.filtered_indices = self.rows.iter()
                .enumerate()
                .filter(|(_, r)| r.filter_text().to_lowercase().contains(&q))
                .map(|(i, _)| i)
                .collect();
        }
        if self.reversed {
            self.filtered_indices.reverse();
        }
        self.rebuild_display_rows();
        if self.sub_view == TrackingsSubView::Condensed {
            self.rebuild_condensed_rows();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_seconds_only() {
        assert_eq!(format_duration(Duration::seconds(5)), "05");
        assert_eq!(format_duration(Duration::seconds(45)), "45");
    }

    #[test]
    fn format_minutes_seconds() {
        assert_eq!(format_duration(Duration::seconds(65)), "01:05");
        assert_eq!(format_duration(Duration::seconds(3599)), "59:59");
    }

    #[test]
    fn format_hours_minutes_seconds() {
        assert_eq!(format_duration(Duration::seconds(3600)), "1:00:00");
        assert_eq!(format_duration(Duration::seconds(3661)), "1:01:01");
        assert_eq!(format_duration(Duration::seconds(36000)), "10:00:00");
    }

    #[test]
    fn format_many_hours() {
        // 100 hours
        assert_eq!(format_duration(Duration::seconds(360000)), "100:00:00");
        // 1000 hours
        assert_eq!(format_duration(Duration::seconds(3600000)), "1000:00:00");
    }

    #[test]
    fn format_zero() {
        assert_eq!(format_duration(Duration::zero()), "00");
    }
}

impl Default for TrackingsState {
    fn default() -> Self { Self::new() }
}
