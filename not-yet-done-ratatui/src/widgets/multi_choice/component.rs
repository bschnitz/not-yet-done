use ratatui::{Frame, layout::Rect};
use tuirealm::command::{Cmd, CmdResult, Direction};
use tuirealm::component::{AppComponent, Component};
use tuirealm::event::{Event, Key, KeyEvent, KeyModifiers, NoUserEvent};
use tuirealm::props::{AttrValue, Attribute, PropPayload, PropValue, QueryResult};
use tuirealm::state::{State, StateValue};

use crate::widgets::common::types::{SelectionMarker, SelectionMode};

use super::{
    MultiChoiceKeymap,
    render::{MultiChoiceViewData, render},
    state::MultiChoiceEvent,
    style::MultiChoiceStyle,
};

/// [`Attribute::Custom`] key for the selected indices slot.
pub const ATTR_SELECTED: &str = "selected";

const CMD_SELECT_ALL: &str = "select_all";
const CMD_SELECT_NONE: &str = "select_none";
const CMD_FILTER_DELETE: &str = "filter_delete";
const CMD_FILTER_CLEAR: &str = "filter_clear";
const CMD_FILTER_LEFT: &str = "filter_left";
const CMD_FILTER_RIGHT: &str = "filter_right";
const CMD_ORDER_UP: &str = "order_up";
const CMD_ORDER_DOWN: &str = "order_down";

/// A multiple‑choice dropdown widget implementing tuirealm's [`Component`] and
/// [`AppComponent<MultiChoiceEvent, NoUserEvent>`].
///
/// By default it behaves exactly as before: no markers, no filter, no footer,
/// no scroll limit, no ordering. New features are opt-in via builder methods.
pub struct MultiChoice {
    // --- framework state ---
    focused: bool,

    // --- internal state ---
    open: bool,
    cursor: usize,
    scroll_offset: usize,
    selected: Vec<bool>,
    filter_query: String,
    filter_cursor: usize,
    filtered_indices: Vec<usize>,
    /// Current item order. `order[i]` is the original index of the item at
    /// display position `i`. Only meaningful when `ordering` is true.
    order: Vec<usize>,

    // --- configuration ---
    title: String,
    choices: Vec<String>,
    placeholder: String,
    width: Option<u16>,
    marker: SelectionMarker,
    mode: SelectionMode,
    show_filter: bool,
    show_footer: bool,
    cursor_on_empty: bool,
    /// Enable item reordering via order_up/order_down keybindings.
    ordering: bool,
    /// Show position numbers ("1. ", " 2. " etc.) next to items.
    show_order: bool,
    /// Maximum visible item rows when expanded. `None` = show all (no scroll).
    max_height: Option<u16>,
    inactive_style: MultiChoiceStyle,
    active_style: MultiChoiceStyle,
    keymap: MultiChoiceKeymap,
    /// Optional shortcut character per choice. When pressed, toggles that item.
    shortcuts: Vec<Option<char>>,
}

impl Default for MultiChoice {
    fn default() -> Self {
        Self {
            focused: false,
            open: false,
            cursor: 0,
            scroll_offset: 0,
            selected: Vec::new(),
            filter_query: String::new(),
            filter_cursor: 0,
            filtered_indices: Vec::new(),
            order: Vec::new(),
            title: String::new(),
            choices: Vec::new(),
            placeholder: String::new(),
            width: None,
            marker: SelectionMarker::default(),
            mode: SelectionMode::default(),
            show_filter: false,
            show_footer: false,
            cursor_on_empty: false,
            ordering: false,
            show_order: false,
            max_height: None,
            keymap: MultiChoiceKeymap::default(),
            inactive_style: MultiChoiceStyle::default(),
            active_style: MultiChoiceStyle::default(),
            shortcuts: Vec::new(),
        }
    }
}

impl MultiChoice {
    /// Sets the title displayed above the dropdown.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Sets the list of choices.
    pub fn with_choices(mut self, choices: Vec<impl Into<String>>) -> Self {
        self.choices = choices.into_iter().map(|c| c.into()).collect();
        self.selected = vec![false; self.choices.len()];
        self.order = (0..self.choices.len()).collect();
        self.cursor = 0;
        self.refilter();
        self
    }

    /// Placeholder shown when no items are selected (collapsed state).
    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Overrides the width of the widget.
    pub fn with_width(mut self, width: u16) -> Self {
        self.width = Some(width);
        self
    }

    /// Sets the selection marker style (default: None).
    pub fn with_marker(mut self, marker: SelectionMarker) -> Self {
        self.marker = marker;
        self
    }

    /// Sets single or multi selection mode (default: Multi).
    pub fn with_mode(mut self, mode: SelectionMode) -> Self {
        self.mode = mode;
        self
    }

    /// Enable a filter input row in the expanded dropdown.
    pub fn with_show_filter(mut self, show: bool) -> Self {
        self.show_filter = show;
        self
    }

    /// Enable a footer row showing selection count.
    pub fn with_show_footer(mut self, show: bool) -> Self {
        self.show_footer = show;
        self
    }

    /// Show the terminal cursor even when the filter input is empty (default: false).
    pub fn with_cursor_on_empty(mut self, show: bool) -> Self {
        self.cursor_on_empty = show;
        self
    }

    /// Enable item reordering (default: false). When enabled, the user can
    /// move items up/down with the `order_up`/`order_down` keybindings.
    pub fn with_ordering(mut self, enabled: bool) -> Self {
        self.ordering = enabled;
        self
    }

    /// Show position numbers next to items (default: false). Only meaningful
    /// when ordering is enabled. Padding adjusts automatically for >9 items.
    pub fn with_show_order(mut self, show: bool) -> Self {
        self.show_order = show;
        self
    }

    /// Set shortcut characters per choice. When a shortcut key is pressed,
    /// that choice is toggled. The shortcut character is highlighted in the label.
    pub fn with_shortcuts(mut self, shortcuts: Vec<Option<char>>) -> Self {
        self.shortcuts = shortcuts;
        self
    }

    /// Limit the number of visible item rows when expanded. Enables scrolling.
    /// `None` (default) shows all items without scrolling.
    pub fn with_max_height(mut self, max: u16) -> Self {
        self.max_height = Some(max);
        self
    }

    pub fn with_keymap(mut self, keymap: MultiChoiceKeymap) -> Self {
        self.keymap = keymap;
        self
    }

    pub fn with_inactive_style(mut self, style: MultiChoiceStyle) -> Self {
        self.inactive_style = style;
        self
    }

    pub fn with_active_style(mut self, style: MultiChoiceStyle) -> Self {
        self.active_style = style;
        self
    }

    /// Get the current item order (indices into original choices vec).
    pub fn current_order(&self) -> &[usize] {
        &self.order
    }

    // --- internal helpers ---

    fn selected_indices(&self) -> Vec<usize> {
        self.selected
            .iter()
            .enumerate()
            .filter_map(|(i, &s)| if s { Some(i) } else { None })
            .collect()
    }

    fn toggle_selection(&mut self) {
        let Some(&real_idx) = self.filtered_indices.get(self.cursor) else {
            return;
        };
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
            self.selected[idx] = true;
        }
    }

    fn select_none(&mut self) {
        self.selected.fill(false);
    }

    fn move_cursor_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.adjust_scroll();
        }
    }

    fn move_cursor_down(&mut self) {
        let max = self.filtered_indices.len().saturating_sub(1);
        if self.cursor < max {
            self.cursor += 1;
            self.adjust_scroll();
        }
    }

    fn move_order_up(&mut self) {
        if !self.ordering {
            return;
        }
        if self.cursor == 0 {
            return;
        }
        // Swap in the order vec.
        let a = self.filtered_indices[self.cursor];
        let b = self.filtered_indices[self.cursor - 1];
        let pos_a = self.order.iter().position(|&x| x == a);
        let pos_b = self.order.iter().position(|&x| x == b);
        if let (Some(pa), Some(pb)) = (pos_a, pos_b) {
            self.order.swap(pa, pb);
        }
        self.cursor -= 1;
        self.refilter_preserving_order();
        self.adjust_scroll();
    }

    fn move_order_down(&mut self) {
        if !self.ordering {
            return;
        }
        let max = self.filtered_indices.len().saturating_sub(1);
        if self.cursor >= max {
            return;
        }
        // Swap in the order vec.
        let a = self.filtered_indices[self.cursor];
        let b = self.filtered_indices[self.cursor + 1];
        let pos_a = self.order.iter().position(|&x| x == a);
        let pos_b = self.order.iter().position(|&x| x == b);
        if let (Some(pa), Some(pb)) = (pos_a, pos_b) {
            self.order.swap(pa, pb);
        }
        self.cursor += 1;
        self.refilter_preserving_order();
        self.adjust_scroll();
    }

    fn adjust_scroll(&mut self) {
        if self.cursor < self.scroll_offset {
            self.scroll_offset = self.cursor;
        }
        if let Some(max_h) = self.max_height {
            let max_h = max_h as usize;
            if max_h > 0 && self.cursor >= self.scroll_offset + max_h {
                self.scroll_offset = self.cursor + 1 - max_h;
            }
        }
    }

    fn refilter(&mut self) {
        if self.ordering {
            // Use order vec as the base sequence.
            self.refilter_preserving_order();
        } else if !self.show_filter || self.filter_query.is_empty() {
            self.filtered_indices = (0..self.choices.len()).collect();
        } else {
            let q = self.filter_query.to_lowercase();
            self.filtered_indices = self
                .choices
                .iter()
                .enumerate()
                .filter(|(_, c)| c.to_lowercase().contains(&q))
                .map(|(i, _)| i)
                .collect();
        }
        self.clamp_cursor();
    }

    fn refilter_preserving_order(&mut self) {
        if !self.show_filter || self.filter_query.is_empty() {
            self.filtered_indices = self.order.clone();
        } else {
            let q = self.filter_query.to_lowercase();
            self.filtered_indices = self
                .order
                .iter()
                .copied()
                .filter(|&i| self.choices[i].to_lowercase().contains(&q))
                .collect();
        }
        self.clamp_cursor();
    }

    fn clamp_cursor(&mut self) {
        if !self.filtered_indices.is_empty() {
            if self.cursor >= self.filtered_indices.len() {
                self.cursor = self.filtered_indices.len() - 1;
            }
        } else {
            self.cursor = 0;
        }
        self.scroll_offset = self
            .scroll_offset
            .min(self.filtered_indices.len().saturating_sub(1));
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

    fn filter_clear(&mut self) {
        self.filter_query.clear();
        self.filter_cursor = 0;
        self.refilter();
    }

    fn filter_cursor_left(&mut self) {
        if self.filter_cursor == 0 {
            return;
        }
        let mut pos = self.filter_cursor - 1;
        while !self.filter_query.is_char_boundary(pos) {
            pos -= 1;
        }
        self.filter_cursor = pos;
    }

    fn filter_cursor_right(&mut self) {
        if self.filter_cursor >= self.filter_query.len() {
            return;
        }
        let mut pos = self.filter_cursor + 1;
        while pos <= self.filter_query.len() && !self.filter_query.is_char_boundary(pos) {
            pos += 1;
        }
        self.filter_cursor = pos;
    }

    /// Find the original choice index for a shortcut character.
    fn shortcut_index(&self, c: char) -> Option<usize> {
        self.shortcuts.iter().position(|s| *s == Some(c))
    }

    fn order_state(&self) -> State {
        State::Vec(self.order.iter().copied().map(StateValue::Usize).collect())
    }
}

impl Component for MultiChoice {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let style = if self.focused {
            &self.active_style
        } else {
            &self.inactive_style
        };
        let data = MultiChoiceViewData {
            title: &self.title,
            choices: &self.choices,
            selected: &self.selected,
            filtered_indices: &self.filtered_indices,
            cursor: self.cursor,
            scroll_offset: self.scroll_offset,
            open: self.open,
            placeholder: &self.placeholder,
            width: self.width,
            max_height: self.max_height,
            marker: &self.marker,
            show_filter: self.show_filter,
            show_footer: self.show_footer,
            show_order: self.show_order && self.ordering,
            order: &self.order,
            cursor_on_empty: self.cursor_on_empty,
            filter_query: &self.filter_query,
            filter_cursor: self.filter_cursor,
            style,
            shortcuts: &self.shortcuts,
        };
        if let Some(pos) = render(frame, area, &data) {
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
                    self.open = f;
                    if f {
                        self.refilter();
                    }
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
            Cmd::Toggle => {
                self.toggle_selection();
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
            Cmd::Custom(CMD_ORDER_UP) => {
                self.move_order_up();
                CmdResult::Changed(self.order_state())
            }
            Cmd::Custom(CMD_ORDER_DOWN) => {
                self.move_order_down();
                CmdResult::Changed(self.order_state())
            }
            Cmd::Submit => {
                self.open = true;
                CmdResult::NoChange
            }
            Cmd::Cancel => {
                self.open = false;
                CmdResult::NoChange
            }
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

impl AppComponent<MultiChoiceEvent, NoUserEvent> for MultiChoice {
    fn on(&mut self, ev: &Event<NoUserEvent>) -> Option<MultiChoiceEvent> {
        let Event::Keyboard(key_ev) = ev else {
            return None;
        };
        let key_ev = *key_ev;

        if !self.open {
            return None;
        }

        // Close key is checked first so it always works.
        if self.keymap.close.matches(&key_ev) {
            self.open = false;
            return Some(MultiChoiceEvent::Closed);
        }

        // Order keys checked before move keys (Ctrl+Arrow before Arrow).
        let cmd = if self.ordering && self.keymap.order_up.matches(&key_ev) {
            Cmd::Custom(CMD_ORDER_UP)
        } else if self.ordering && self.keymap.order_down.matches(&key_ev) {
            Cmd::Custom(CMD_ORDER_DOWN)
        } else if self.keymap.move_up.matches(&key_ev) {
            Cmd::Move(Direction::Up)
        } else if self.keymap.move_down.matches(&key_ev) {
            Cmd::Move(Direction::Down)
        } else if self.keymap.toggle.matches(&key_ev) {
            Cmd::Toggle
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
        } else if let KeyEvent {
            code: Key::Char(c),
            modifiers: KeyModifiers::NONE,
        } = key_ev
        {
            // Check shortcuts.
            if let Some(idx) = self.shortcut_index(c) {
                let real_idx = idx;
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
                // Move cursor to the shortcut item.
                if let Some(fi) = self.filtered_indices.iter().position(|&x| x == real_idx) {
                    self.cursor = fi;
                    self.adjust_scroll();
                }
                return Some(MultiChoiceEvent::SelectionChanged(self.selected_indices()));
            }
            return None;
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
                // Distinguish order change from selection change by checking
                // if it came from an order command.
                if self.ordering
                    && (self.keymap.order_up.matches(&key_ev)
                        || self.keymap.order_down.matches(&key_ev))
                {
                    Some(MultiChoiceEvent::OrderChanged(indices))
                } else {
                    Some(MultiChoiceEvent::SelectionChanged(indices))
                }
            }
            CmdResult::Changed(State::Single(StateValue::Usize(idx))) => {
                Some(MultiChoiceEvent::HighlightChanged(idx))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering_swap_items() {
        let mut mc = MultiChoice::default()
            .with_choices(vec!["A", "B", "C", "D"])
            .with_ordering(true);
        mc.open = true;

        assert_eq!(mc.order, vec![0, 1, 2, 3]);

        // Move cursor to B (index 1), then move it up.
        mc.cursor = 1;
        mc.move_order_up();
        assert_eq!(mc.order, vec![1, 0, 2, 3]);
        assert_eq!(mc.cursor, 0);

        // Move it back down.
        mc.move_order_down();
        assert_eq!(mc.order, vec![0, 1, 2, 3]);
        assert_eq!(mc.cursor, 1);
    }

    #[test]
    fn ordering_boundaries() {
        let mut mc = MultiChoice::default()
            .with_choices(vec!["A", "B", "C"])
            .with_ordering(true);
        mc.open = true;

        // Move up at top — no change.
        mc.cursor = 0;
        mc.move_order_up();
        assert_eq!(mc.order, vec![0, 1, 2]);
        assert_eq!(mc.cursor, 0);

        // Move down at bottom — no change.
        mc.cursor = 2;
        mc.move_order_down();
        assert_eq!(mc.order, vec![0, 1, 2]);
        assert_eq!(mc.cursor, 2);
    }

    #[test]
    fn ordering_disabled_noop() {
        let mut mc = MultiChoice::default().with_choices(vec!["A", "B", "C"]);
        mc.open = true;
        mc.cursor = 1;
        mc.move_order_up();
        assert_eq!(mc.order, vec![0, 1, 2]);
    }

    #[test]
    fn show_order_requires_ordering() {
        let mc = MultiChoice::default()
            .with_choices(vec!["A", "B"])
            .with_show_order(true); // ordering not enabled
        // show_order has no effect in view without ordering — tested via ViewData
        assert!(!mc.ordering);
        assert!(mc.show_order);
    }
}
