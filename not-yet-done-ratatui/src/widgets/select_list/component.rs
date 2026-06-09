use ratatui::{Frame, layout::Rect};
use tuirealm::command::{Cmd, CmdResult, Direction};
use tuirealm::component::{AppComponent, Component};
use tuirealm::event::{Event, Key, KeyEvent, KeyModifiers, NoUserEvent};
use tuirealm::props::{AttrValue, Attribute, PropPayload, PropValue, QueryResult};
use tuirealm::state::{State, StateValue};

use super::{
    render::{RenderData, SelectListItem as RenderItem, render},
    state::SelectListEvent,
    style::SelectListStyle,
    keymap::SelectListKeymap,
};

/// Custom attribute key for reading/writing the selected indices.
pub const ATTR_SELECTED: &str = "selected";
/// Custom attribute key for reading/writing items.
pub const ATTR_ITEMS: &str = "items";

const CMD_TOGGLE: &str = "toggle";
const CMD_SELECT_ALL: &str = "select_all";
const CMD_SELECT_NONE: &str = "select_none";
const CMD_FILTER_DELETE: &str = "filter_delete";
const CMD_FILTER_CLEAR: &str = "filter_clear";
const CMD_FILTER_LEFT: &str = "filter_left";
const CMD_FILTER_RIGHT: &str = "filter_right";

pub use crate::widgets::common::types::{FilterMode, SelectionMarker, SelectionMode};

/// A stored item with all metadata.
#[derive(Debug, Clone)]
pub struct SelectListItemData {
    pub label: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub group: Option<String>,
    pub is_group_header: bool,
}

/// A scrollable, filterable selection list.
///
/// Supports multi/single selection, configurable markers, optional fuzzy
/// filter input, and group headers.
///
/// ```rust
/// use not_yet_done_ratatui::widgets::select_list::SelectList;
/// use not_yet_done_ratatui::SelectionMarker;
///
/// let list = SelectList::default()
///     .with_items(vec!["Alpha", "Beta", "Gamma"])
///     .with_marker(SelectionMarker::Checkbox)
///     .with_show_filter(true)
///     .with_show_footer(true);
/// ```
pub struct SelectList {
    // --- framework state ---
    pub(crate) focused: bool,

    // --- internal state ---
    pub(crate) cursor: usize,
    pub(crate) scroll_offset: usize,
    pub(crate) selected: Vec<bool>,
    pub(crate) filter_query: String,
    pub(crate) filter_cursor: usize,
    /// Indices into `items` that match the current filter.
    pub(crate) filtered_indices: Vec<usize>,

    // --- data ---
    pub(crate) items: Vec<SelectListItemData>,

    // --- configuration ---
    pub(crate) marker: SelectionMarker,
    pub(crate) mode: SelectionMode,
    pub(crate) show_filter: bool,
    pub(crate) show_footer: bool,
    pub(crate) cursor_on_empty: bool,
    pub(crate) filter_mode: FilterMode,
    pub(crate) inactive_style: SelectListStyle,
    pub(crate) active_style: SelectListStyle,
    pub(crate) keymap: SelectListKeymap,
}

impl Default for SelectList {
    fn default() -> Self {
        Self {
            focused: false,
            cursor: 0,
            scroll_offset: 0,
            selected: Vec::new(),
            filter_query: String::new(),
            filter_cursor: 0,
            filtered_indices: Vec::new(),
            items: Vec::new(),
            marker: SelectionMarker::default(),
            mode: SelectionMode::default(),
            show_filter: false,
            show_footer: false,
            cursor_on_empty: false,
            filter_mode: FilterMode::default(),
            inactive_style: SelectListStyle::default(),
            active_style: SelectListStyle::default(),
            keymap: SelectListKeymap::default(),
        }
    }
}

impl SelectList {
    /// Set items from simple string labels.
    pub fn with_items(mut self, labels: Vec<impl Into<String>>) -> Self {
        self.items = labels
            .into_iter()
            .map(|l| SelectListItemData {
                label: l.into(),
                description: None,
                icon: None,
                group: None,
                is_group_header: false,
            })
            .collect();
        self.selected = vec![false; self.items.len()];
        self.refilter();
        self
    }

    /// Set items with full metadata.
    pub fn with_item_data(mut self, items: Vec<SelectListItemData>) -> Self {
        self.selected = vec![false; items.len()];
        self.items = items;
        self.refilter();
        self
    }

    /// Replace items in-place from simple string labels. Resets selection,
    /// cursor, and filter state. Used to repopulate the list at runtime
    /// (e.g. after directory navigation in a composite widget).
    pub fn set_items(&mut self, labels: Vec<String>) {
        self.items = labels
            .into_iter()
            .map(|l| SelectListItemData {
                label: l,
                description: None,
                icon: None,
                group: None,
                is_group_header: false,
            })
            .collect();
        self.selected = vec![false; self.items.len()];
        self.cursor = 0;
        self.scroll_offset = 0;
        self.refilter();
    }

    pub fn with_marker(mut self, marker: SelectionMarker) -> Self {
        self.marker = marker;
        self
    }

    pub fn with_mode(mut self, mode: SelectionMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn with_show_filter(mut self, show: bool) -> Self {
        self.show_filter = show;
        self
    }

    pub fn with_show_footer(mut self, show: bool) -> Self {
        self.show_footer = show;
        self
    }

    /// Show the terminal cursor even when the filter input is empty (default: false).
    pub fn with_cursor_on_empty(mut self, show: bool) -> Self {
        self.cursor_on_empty = show;
        self
    }

    /// Choose how the filter input matches items (default: substring).
    pub fn with_filter_mode(mut self, mode: FilterMode) -> Self {
        self.filter_mode = mode;
        self
    }

    pub fn with_inactive_style(mut self, style: SelectListStyle) -> Self {
        self.inactive_style = style;
        self
    }

    pub fn with_active_style(mut self, style: SelectListStyle) -> Self {
        self.active_style = style;
        self
    }

    pub fn with_keymap(mut self, keymap: SelectListKeymap) -> Self {
        self.keymap = keymap;
        self
    }

    // --- internal helpers ---

    pub(crate) fn selected_indices(&self) -> Vec<usize> {
        self.selected
            .iter()
            .enumerate()
            .filter_map(|(i, &s)| if s { Some(i) } else { None })
            .collect()
    }

    fn toggle_at_cursor(&mut self) {
        let Some(&real_idx) = self.filtered_indices.get(self.cursor) else {
            return;
        };
        let item = &self.items[real_idx];
        if item.is_group_header {
            return;
        }

        match self.mode {
            SelectionMode::Multi => {
                self.selected[real_idx] = !self.selected[real_idx];
            }
            SelectionMode::Single => {
                let was = self.selected[real_idx];
                self.selected.fill(false);
                self.selected[real_idx] = !was;
            }
        }
    }

    fn select_all(&mut self) {
        for &idx in &self.filtered_indices {
            if !self.items[idx].is_group_header {
                self.selected[idx] = true;
            }
        }
    }

    fn select_none(&mut self) {
        self.selected.fill(false);
    }

    fn move_cursor_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            // Skip group headers.
            while self.cursor > 0 {
                if let Some(&idx) = self.filtered_indices.get(self.cursor) {
                    if self.items[idx].is_group_header {
                        self.cursor -= 1;
                        continue;
                    }
                }
                break;
            }
            self.adjust_scroll();
        }
    }

    fn move_cursor_down(&mut self) {
        let max = self.filtered_indices.len().saturating_sub(1);
        if self.cursor < max {
            self.cursor += 1;
            // Skip group headers.
            while self.cursor < max {
                if let Some(&idx) = self.filtered_indices.get(self.cursor) {
                    if self.items[idx].is_group_header {
                        self.cursor += 1;
                        continue;
                    }
                }
                break;
            }
            self.adjust_scroll();
        }
    }

    fn adjust_scroll(&mut self) {
        // Keep scroll_offset so cursor is visible (simple logic; view() knows the area).
        if self.cursor < self.scroll_offset {
            self.scroll_offset = self.cursor;
        }
    }

    fn refilter(&mut self) {
        if self.filter_query.is_empty() {
            self.filtered_indices = (0..self.items.len()).collect();
        } else {
            self.filtered_indices = match self.filter_mode {
                FilterMode::Substring => self.filter_substring(),
                FilterMode::Fuzzy => self.filter_fuzzy(),
            };
        }

        if !self.filtered_indices.is_empty() {
            if self.cursor >= self.filtered_indices.len() {
                self.cursor = self.filtered_indices.len() - 1;
            }
        } else {
            self.cursor = 0;
        }
        self.scroll_offset = 0;
    }

    fn filter_substring(&self) -> Vec<usize> {
        let q = self.filter_query.to_lowercase();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                if item.is_group_header {
                    return true;
                }
                item.label.to_lowercase().contains(&q)
                    || item
                        .description
                        .as_deref()
                        .map(|d| d.to_lowercase().contains(&q))
                        .unwrap_or(false)
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn filter_fuzzy(&self) -> Vec<usize> {
        use fuzzy_matcher::FuzzyMatcher;
        use fuzzy_matcher::skim::SkimMatcherV2;
        let matcher = SkimMatcherV2::default();
        let q = &self.filter_query;

        // Group headers are dropped in fuzzy mode: their original position
        // becomes meaningless once items are reordered by score.
        let mut matched: Vec<(usize, i64)> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| !item.is_group_header)
            .filter_map(|(i, item)| {
                let label_score = matcher.fuzzy_match(&item.label, q);
                let desc_score = item
                    .description
                    .as_deref()
                    .and_then(|d| matcher.fuzzy_match(d, q));
                let best = match (label_score, desc_score) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                };
                best.map(|s| (i, s))
            })
            .collect();
        matched.sort_by(|a, b| b.1.cmp(&a.1));
        matched.into_iter().map(|(i, _)| i).collect()
    }

    fn filter_push_char(&mut self, c: char) {
        self.filter_query.insert(self.filter_cursor, c);
        self.filter_cursor += c.len_utf8();
        self.refilter();
    }

    fn filter_pop_char(&mut self) {
        if self.filter_cursor == 0 {
            return;
        }
        let mut pos = self.filter_cursor - 1;
        while !self.filter_query.is_char_boundary(pos) {
            pos -= 1;
        }
        self.filter_query.remove(pos);
        self.filter_cursor = pos;
        self.refilter();
    }

    /// Reset the filter query to empty. Exposed crate-wide so composite
    /// widgets (e.g. `FilePicker`) can wire their own picker-level
    /// keybinding to it.
    pub(crate) fn filter_clear(&mut self) {
        self.filter_query.clear();
        self.filter_cursor = 0;
        self.refilter();
    }

    fn filter_cursor_left(&mut self) {
        if self.filter_cursor == 0 { return; }
        let mut pos = self.filter_cursor - 1;
        while !self.filter_query.is_char_boundary(pos) { pos -= 1; }
        self.filter_cursor = pos;
    }

    fn filter_cursor_right(&mut self) {
        if self.filter_cursor >= self.filter_query.len() { return; }
        let mut pos = self.filter_cursor + 1;
        while pos <= self.filter_query.len() && !self.filter_query.is_char_boundary(pos) { pos += 1; }
        self.filter_cursor = pos;
    }
}

impl Component for SelectList {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        // Adjust scroll for visible area.
        let filter_rows = if self.show_filter { 1 } else { 0 };
        let footer_rows = if self.show_footer { 1 } else { 0 };
        let items_height = area
            .height
            .saturating_sub(filter_rows)
            .saturating_sub(footer_rows) as usize;
        if items_height > 0 && self.cursor >= self.scroll_offset + items_height {
            self.scroll_offset = self.cursor + 1 - items_height;
        }

        let render_items: Vec<RenderItem> = self
            .filtered_indices
            .iter()
            .map(|&idx| {
                let item = &self.items[idx];
                RenderItem {
                    label: &item.label,
                    description: item.description.as_deref(),
                    icon: item.icon.as_deref(),
                    group: item.group.as_deref(),
                    selected: self.selected[idx],
                    is_group_header: item.is_group_header,
                }
            })
            .collect();

        let selected_count = self.selected.iter().filter(|&&s| s).count();

        let data = RenderData {
            items: &render_items,
            cursor: self.cursor,
            scroll_offset: self.scroll_offset,
            filter_query: &self.filter_query,
            filter_cursor: self.filter_cursor,
            show_filter: self.show_filter,
            selected_count,
            total_count: self.items.len(),
            show_footer: self.show_footer,
            focused: self.focused,
        };

        if let Some(pos) = render(frame.buffer_mut(), area, &data, self) {
            frame.set_cursor_position(pos);
        }
    }

    fn query(&self, attr: Attribute) -> Option<QueryResult<'_>> {
        match attr {
            Attribute::Focus => Some(QueryResult::Owned(AttrValue::Flag(self.focused))),
            Attribute::Custom(key) if key == ATTR_SELECTED => {
                Some(QueryResult::Owned(AttrValue::Payload(PropPayload::Vec(
                    self.selected_indices()
                        .into_iter()
                        .map(PropValue::Usize)
                        .collect(),
                ))))
            }
            _ => None,
        }
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        match attr {
            Attribute::Focus => {
                if let AttrValue::Flag(f) = value {
                    self.focused = f;
                }
            }
            Attribute::Custom(key) if key == ATTR_SELECTED => {
                if let AttrValue::Payload(PropPayload::Vec(values)) = value {
                    self.selected.fill(false);
                    for v in values {
                        if let PropValue::Usize(i) = v {
                            if i < self.selected.len() {
                                self.selected[i] = true;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn state(&self) -> State {
        State::Vec(
            self.selected_indices()
                .into_iter()
                .map(StateValue::Usize)
                .collect(),
        )
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        match cmd {
            Cmd::Move(Direction::Up) => {
                self.move_cursor_up();
                CmdResult::Changed(State::Single(StateValue::Usize(self.cursor)))
            }
            Cmd::Move(Direction::Down) => {
                self.move_cursor_down();
                CmdResult::Changed(State::Single(StateValue::Usize(self.cursor)))
            }
            Cmd::Custom(CMD_TOGGLE) => {
                self.toggle_at_cursor();
                CmdResult::Changed(self.state())
            }
            Cmd::Custom(CMD_SELECT_ALL) => {
                self.select_all();
                CmdResult::Changed(self.state())
            }
            Cmd::Custom(CMD_SELECT_NONE) => {
                self.select_none();
                CmdResult::Changed(self.state())
            }
            Cmd::Submit => CmdResult::Submit(self.state()),
            Cmd::Cancel => CmdResult::Batch(vec![]),
            Cmd::Custom(CMD_FILTER_DELETE) => {
                self.filter_pop_char();
                CmdResult::NoChange
            }
            Cmd::Custom(CMD_FILTER_CLEAR) => {
                self.filter_clear();
                CmdResult::NoChange
            }
            Cmd::Custom(CMD_FILTER_LEFT) => {
                self.filter_cursor_left();
                CmdResult::NoChange
            }
            Cmd::Custom(CMD_FILTER_RIGHT) => {
                self.filter_cursor_right();
                CmdResult::NoChange
            }
            Cmd::Type(c) if self.show_filter => {
                self.filter_push_char(c);
                CmdResult::NoChange
            }
            _ => CmdResult::NoChange,
        }
    }
}

impl AppComponent<SelectListEvent, NoUserEvent> for SelectList {
    fn on(&mut self, ev: &Event<NoUserEvent>) -> Option<SelectListEvent> {
        let Event::Keyboard(key_ev) = ev else {
            return None;
        };
        let key_ev = *key_ev;

        let cmd = if self.keymap.move_up.matches(&key_ev) {
            Cmd::Move(Direction::Up)
        } else if self.keymap.move_down.matches(&key_ev) {
            Cmd::Move(Direction::Down)
        } else if self.keymap.toggle.matches(&key_ev) {
            Cmd::Custom(CMD_TOGGLE)
        } else if self.keymap.confirm.matches(&key_ev) {
            Cmd::Submit
        } else if self.keymap.cancel.matches(&key_ev) {
            Cmd::Cancel
        } else if self.keymap.select_all.matches(&key_ev) {
            Cmd::Custom(CMD_SELECT_ALL)
        } else if self.keymap.select_none.matches(&key_ev) {
            Cmd::Custom(CMD_SELECT_NONE)
        } else if self.show_filter && self.keymap.filter_cursor_left.matches(&key_ev) {
            Cmd::Custom(CMD_FILTER_LEFT)
        } else if self.show_filter && self.keymap.filter_cursor_right.matches(&key_ev) {
            Cmd::Custom(CMD_FILTER_RIGHT)
        } else if self.show_filter && self.keymap.filter_delete.matches(&key_ev) {
            Cmd::Custom(CMD_FILTER_DELETE)
        } else if self.show_filter && self.keymap.filter_clear.matches(&key_ev) {
            Cmd::Custom(CMD_FILTER_CLEAR)
        } else if self.show_filter {
            match key_ev {
                KeyEvent {
                    code: Key::Char(c),
                    modifiers: KeyModifiers::NONE,
                }
                | KeyEvent {
                    code: Key::Char(c),
                    modifiers: KeyModifiers::SHIFT,
                } => Cmd::Type(c),
                _ => return None,
            }
        } else {
            return None;
        };

        match self.perform(cmd) {
            CmdResult::Changed(State::Vec(values)) => {
                let indices: Vec<usize> = values
                    .into_iter()
                    .filter_map(|v| {
                        if let StateValue::Usize(i) = v {
                            Some(i)
                        } else {
                            None
                        }
                    })
                    .collect();
                Some(SelectListEvent::SelectionChanged(indices))
            }
            CmdResult::Changed(State::Single(StateValue::Usize(idx))) => {
                Some(SelectListEvent::CursorChanged(idx))
            }
            CmdResult::Submit(State::Vec(values)) => {
                let indices: Vec<usize> = values
                    .into_iter()
                    .filter_map(|v| {
                        if let StateValue::Usize(i) = v {
                            Some(i)
                        } else {
                            None
                        }
                    })
                    .collect();
                Some(SelectListEvent::Confirmed(indices))
            }
            CmdResult::Batch(_) => Some(SelectListEvent::Cancelled),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list_with(items: Vec<&str>) -> SelectList {
        SelectList::default()
            .with_items(items.into_iter().map(String::from).collect())
            .with_show_filter(true)
    }

    #[test]
    fn substring_default_filter() {
        let mut list = list_with(vec!["alpha", "beta", "gamma"]);
        list.filter_push_char('l');
        // Only "alpha" contains 'l'; order preserved.
        assert_eq!(list.filtered_indices, vec![0]);
    }

    #[test]
    fn substring_no_match_empties_list() {
        let mut list = list_with(vec!["alpha", "beta"]);
        list.filter_push_char('z');
        assert!(list.filtered_indices.is_empty());
    }

    #[test]
    fn fuzzy_matches_subsequence() {
        let mut list = list_with(vec!["alpha", "beta", "gamma"])
            .with_filter_mode(FilterMode::Fuzzy);
        list.filter_push_char('g');
        list.filter_push_char('m');
        // "gamma" matches the subsequence g..m..a, alpha/beta do not.
        assert_eq!(list.filtered_indices, vec![2]);
    }

    #[test]
    fn fuzzy_ranks_better_match_first() {
        let mut list = list_with(vec!["file_picker", "picker", "pickled"])
            .with_filter_mode(FilterMode::Fuzzy);
        list.filter_push_char('p');
        list.filter_push_char('i');
        list.filter_push_char('c');
        list.filter_push_char('k');
        // "picker" (exact prefix) should rank above "file_picker" (offset
        // match) and "pickled" (broken sequence after pick).
        assert_eq!(list.filtered_indices.first().copied(), Some(1));
    }

    #[test]
    fn fuzzy_empty_query_shows_all_in_original_order() {
        let list = list_with(vec!["alpha", "beta", "gamma"])
            .with_filter_mode(FilterMode::Fuzzy);
        assert_eq!(list.filtered_indices, vec![0, 1, 2]);
    }
}
