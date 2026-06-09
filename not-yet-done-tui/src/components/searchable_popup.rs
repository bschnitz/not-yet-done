//! Generic searchable list popup — reusable overlay with fuzzy search.
//!
//! Used for saved filter selection, script picker, etc.

use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Widget};
use ratatui::Frame;

use tuirealm::command::{Cmd, CmdResult};
use tuirealm::props::{Attribute, AttrValue, QueryResult};
use tuirealm::component::Component;
use tuirealm::state::{State, StateValue};

use std::sync::Arc;
use crate::ui::theme::Theme;
use crate::config::keybindings::{KeyBindingSection, KeyIconMap, PopupAction};

/// A single item in the searchable list.
pub struct PopupItem {
    pub label: String,
    /// Opaque payload returned when the item is selected.
    pub value: String,
}

/// Result of [`SearchablePopup::handle_key`]. The popup consumes navigation
/// and text-input keys intrinsically; everything else (`Select`/`Close`,
/// embedder-specific actions) is signalled as `Unhandled` so the embedder
/// can dispatch it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupKeyOutcome {
    Handled,
    Unhandled,
}

pub struct SearchablePopup {
    theme: Arc<Theme>,
    title: String,
    query: String,
    cursor: usize,
    items: Vec<PopupItem>,
    filtered: Vec<usize>,
    selected: usize,
    hints: Vec<(String, String)>, // (key_label, description) — embedder-supplied
    /// Popup-intrinsic key bindings (Next/Prev/Backspace/Cursor). When set,
    /// the navigation hints render automatically in the hint bar in front
    /// of the embedder-supplied hints, and `handle_key` is routable.
    popup_kb: Option<KeyBindingSection<PopupAction>>,
    /// Icon map used when rendering intrinsic hints; if `None`, raw key
    /// strings are shown.
    key_icons: Option<KeyIconMap>,
}

impl SearchablePopup {
    pub fn new(theme: Arc<Theme>, title: impl Into<String>, items: Vec<PopupItem>) -> Self {
        let filtered: Vec<usize> = (0..items.len()).collect();
        Self {
            theme,
            title: title.into(),
            query: String::new(),
            cursor: 0,
            items,
            filtered,
            selected: 0,
            hints: Vec::new(),
            popup_kb: None,
            key_icons: None,
        }
    }

    pub fn with_hints(mut self, hints: Vec<(String, String)>) -> Self {
        self.hints = hints;
        self
    }

    /// Attach the popup-intrinsic keybindings + icon map. After this call,
    /// `handle_key` will route `Next/Prev/Backspace/Cursor*` through them
    /// and the hint bar auto-prepends the navigation hints.
    pub fn with_popup_kb(
        mut self,
        kb: KeyBindingSection<PopupAction>,
        icons: KeyIconMap,
    ) -> Self {
        self.popup_kb = Some(kb);
        self.key_icons = Some(icons);
        self
    }

    pub fn insert_char(&mut self, c: char) {
        let byte_pos = self.query.char_indices()
            .nth(self.cursor)
            .map(|(i, _)| i)
            .unwrap_or(self.query.len());
        self.query.insert(byte_pos, c);
        self.cursor += 1;
        self.apply_filter();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 || self.query.is_empty() { return; }
        let byte_pos = self.query.char_indices()
            .nth(self.cursor - 1)
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.query.remove(byte_pos);
        self.cursor -= 1;
        self.apply_filter();
    }

    pub fn cursor_left(&mut self) {
        if self.cursor > 0 { self.cursor -= 1; }
    }

    pub fn cursor_right(&mut self) {
        let max = self.query.chars().count();
        if self.cursor < max { self.cursor += 1; }
    }

    pub fn select_next(&mut self) {
        if self.selected + 1 < self.filtered.len() {
            self.selected += 1;
        }
    }

    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn selected_item(&self) -> Option<&PopupItem> {
        let &idx = self.filtered.get(self.selected)?;
        self.items.get(idx)
    }

    /// The current search query text.
    pub fn query_text(&self) -> &str {
        &self.query
    }

    /// Whether the filtered list is empty (no matches for current query).
    pub fn filtered_is_empty(&self) -> bool {
        self.filtered.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Handle list navigation + search-text input.
    ///
    /// If an intrinsic kb has been attached via [`Self::with_popup_kb`],
    /// the configured `PopupAction` bindings drive next/prev/backspace/
    /// cursor. Otherwise the popup falls back to a built-in set
    /// (`up`/`down`/`backspace`/`left`/`right` plus any typed printable
    /// char) so legacy embedders that don't attach intrinsic bindings
    /// keep working unchanged.
    ///
    /// Returns `Unhandled` for keys the popup does not consume (e.g.
    /// Enter/Esc, embedder-specific actions like Ctrl+E) so the
    /// embedder can dispatch them.
    pub fn handle_key(&mut self, key: &str) -> PopupKeyOutcome {
        if let Some(kb) = self.popup_kb.as_ref() {
            if kb.get(&PopupAction::Next).is_some_and(|b| b.matches(key)) {
                self.select_next();
                return PopupKeyOutcome::Handled;
            }
            if kb.get(&PopupAction::Prev).is_some_and(|b| b.matches(key)) {
                self.select_prev();
                return PopupKeyOutcome::Handled;
            }
            if kb.get(&PopupAction::Backspace).is_some_and(|b| b.matches(key)) {
                self.backspace();
                return PopupKeyOutcome::Handled;
            }
            if kb.get(&PopupAction::CursorLeft).is_some_and(|b| b.matches(key)) {
                self.cursor_left();
                return PopupKeyOutcome::Handled;
            }
            if kb.get(&PopupAction::CursorRight).is_some_and(|b| b.matches(key)) {
                self.cursor_right();
                return PopupKeyOutcome::Handled;
            }
        } else {
            // Legacy fallback — used by embedders that haven't called
            // with_popup_kb (and by all unit tests that exercise the
            // popup without a kb).
            match key {
                "down" => { self.select_next(); return PopupKeyOutcome::Handled; }
                "up" => { self.select_prev(); return PopupKeyOutcome::Handled; }
                "backspace" => { self.backspace(); return PopupKeyOutcome::Handled; }
                "left" => { self.cursor_left(); return PopupKeyOutcome::Handled; }
                "right" => { self.cursor_right(); return PopupKeyOutcome::Handled; }
                _ => {}
            }
        }
        // Plain printable single char → typed into the search query.
        if key.chars().count() == 1 {
            let c = key.chars().next().unwrap();
            if !c.is_control() {
                self.insert_char(c);
                return PopupKeyOutcome::Handled;
            }
        }
        PopupKeyOutcome::Unhandled
    }

    /// Intrinsic hints rendered in front of the embedder-supplied hints.
    /// Empty when no `popup_kb` is attached, so the legacy code paths
    /// (popups built without [`Self::with_popup_kb`]) render exactly as
    /// before.
    fn intrinsic_hints(&self) -> Vec<(String, String)> {
        let (Some(kb), Some(icons)) = (self.popup_kb.as_ref(), self.key_icons.as_ref()) else {
            return Vec::new();
        };
        let mut hints = vec![
            (kb.hint_label(&PopupAction::Next, icons), "next".to_string()),
            (kb.hint_label(&PopupAction::Prev, icons), "prev".to_string()),
        ];
        if !self.query.is_empty() {
            hints.push((
                kb.hint_label(&PopupAction::Backspace, icons),
                "erase".to_string(),
            ));
        }
        hints
    }

    fn apply_filter(&mut self) {
        let q = self.query.to_lowercase();
        self.filtered = self.items.iter().enumerate()
            .filter(|(_, item)| q.is_empty() || item.label.to_lowercase().contains(&q))
            .map(|(i, _)| i)
            .collect();
        if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }
    }
}

impl Component for SearchablePopup {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let t = &self.theme;

        // Intrinsic hints render in front of embedder-supplied ones.
        let intrinsic = self.intrinsic_hints();
        let all_hints: Vec<&(String, String)> = intrinsic
            .iter()
            .chain(self.hints.iter())
            .collect();

        let popup_w = (area.width * 50 / 100).max(30).min(area.width.saturating_sub(4));
        let hints_h = if all_hints.is_empty() { 0u16 } else { 1 };
        let max_items = self.filtered.len() as u16;
        let popup_h = (max_items + 3 + hints_h).min(area.height * 60 / 100).max(5);
        let x = (area.width.saturating_sub(popup_w)) / 2;
        let y = (area.height.saturating_sub(popup_h)) / 2;
        let popup_area = Rect::new(x, y, popup_w, popup_h);

        frame.render_widget(Clear, popup_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.primary()))
            .title(format!(" {} ", self.title))
            .title_style(Style::default().fg(t.accent()).add_modifier(Modifier::BOLD))
            .style(Style::default().bg(t.bg()));

        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        if inner.height == 0 || inner.width == 0 { return; }

        let input_y = inner.y;
        let input_bg = t.surface();
        let cursor_pos;

        {
            let buf = frame.buffer_mut();

            // Search input row.
            for cx in inner.left()..inner.right() {
                if let Some(cell) = buf.cell_mut(Position::new(cx, input_y)) {
                    cell.set_char(' ');
                    cell.set_style(Style::default().bg(input_bg));
                }
            }

            let prefix = " 󰈲 ";
            let mut px = inner.left();
            for ch in prefix.chars() {
                if px >= inner.right() { break; }
                if let Some(cell) = buf.cell_mut(Position::new(px, input_y)) {
                    cell.set_char(ch);
                    cell.set_style(Style::default().fg(t.accent()).bg(input_bg));
                }
                px += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1) as u16;
            }

            let text_start_x = px;
            let max_w = inner.right().saturating_sub(text_start_x) as usize;
            let chars: Vec<char> = self.query.chars().collect();
            let view_start = if self.cursor >= max_w { self.cursor + 1 - max_w } else { 0 };

            for (screen_idx, char_idx) in (view_start..chars.len()).enumerate() {
                if screen_idx >= max_w { break; }
                let cx = text_start_x + screen_idx as u16;
                let ch = chars[char_idx];
                let style = Style::default().fg(t.text_high()).bg(input_bg);
                if let Some(cell) = buf.cell_mut(Position::new(cx, input_y)) {
                    cell.set_char(ch);
                    cell.set_style(style);
                }
            }

            cursor_pos = if !chars.is_empty() {
                let screen_pos = self.cursor.saturating_sub(view_start);
                let cx = text_start_x + screen_pos as u16;
                if cx < inner.right() { Some(Position::new(cx, input_y)) } else { None }
            } else {
                None
            };

            // Items list.
            let list_y = inner.y + 1;
            let list_h = inner.height.saturating_sub(1 + hints_h);

            for (i, &item_idx) in self.filtered.iter().enumerate() {
                if i as u16 >= list_h { break; }
                let item = &self.items[item_idx];
                let is_selected = i == self.selected;
                let style = if is_selected {
                    Style::default().fg(t.bg()).bg(t.primary())
                } else {
                    Style::default().fg(t.text_high()).bg(t.bg())
                };
                let row_area = Rect {
                    x: inner.x,
                    y: list_y + i as u16,
                    width: inner.width,
                    height: 1,
                };
                let display = format!(" {}", item.label);
                let padded = format!("{:width$}", display, width = inner.width as usize);
                Line::from(Span::styled(padded, style)).render(row_area, buf);
            }

            // Hints bar.
            if !all_hints.is_empty() && hints_h > 0 {
                let hints_y = inner.y + inner.height - hints_h;
                // Background.
                for cx in inner.left()..inner.right() {
                    if let Some(cell) = buf.cell_mut(Position::new(cx, hints_y)) {
                        cell.set_char(' ');
                        cell.set_style(Style::default().bg(t.surface()));
                    }
                }
                let mut hx = inner.left() + 1;
                for (key_label, desc) in all_hints.iter().map(|p| (&p.0, &p.1)) {
                    if hx >= inner.right() { break; }
                    let key_style = Style::default().fg(t.text_dim()).bg(t.surface());
                    let desc_style = Style::default().fg(t.text_med()).bg(t.surface());
                    for ch in key_label.chars() {
                        if hx >= inner.right() { break; }
                        if let Some(cell) = buf.cell_mut(Position::new(hx, hints_y)) {
                            cell.set_char(ch);
                            cell.set_style(key_style);
                        }
                        hx += 1;
                    }
                    if hx < inner.right() {
                        if let Some(cell) = buf.cell_mut(Position::new(hx, hints_y)) {
                            cell.set_char(' ');
                            cell.set_style(desc_style);
                        }
                        hx += 1;
                    }
                    for ch in desc.chars() {
                        if hx >= inner.right() { break; }
                        if let Some(cell) = buf.cell_mut(Position::new(hx, hints_y)) {
                            cell.set_char(ch);
                            cell.set_style(desc_style);
                        }
                        hx += 1;
                    }
                    // Gap.
                    hx += 2;
                }
            }
        }

        if let Some(pos) = cursor_pos {
            frame.set_cursor_position(pos);
        }
    }

    fn query(&self, _attr: Attribute) -> Option<QueryResult<'_>> { None }
    fn attr(&mut self, _attr: Attribute, _value: AttrValue) {}
    fn state(&self) -> State { State::Single(StateValue::String(self.query.clone())) }
    fn perform(&mut self, _cmd: Cmd) -> CmdResult { CmdResult::NoChange }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<PopupItem> {
        vec![
            PopupItem { label: "all tasks".into(), value: "{}".into() },
            PopupItem { label: "high priority".into(), value: "{}".into() },
            PopupItem { label: "done tasks".into(), value: "{}".into() },
        ]
    }

    fn theme() -> Arc<Theme> {
        Arc::new(Theme::new(crate::config::ThemeConfig::default()))
    }

    #[test]
    fn new_shows_all() {
        let popup = SearchablePopup::new(theme(), "Test", items());
        assert_eq!(popup.filtered.len(), 3);
        assert_eq!(popup.selected, 0);
    }

    #[test]
    fn filter_narrows_list() {
        let mut popup = SearchablePopup::new(theme(), "Test", items());
        popup.insert_char('h');
        popup.insert_char('i');
        assert_eq!(popup.filtered.len(), 1);
        assert_eq!(popup.items[popup.filtered[0]].label, "high priority");
    }

    #[test]
    fn backspace_widens_list() {
        let mut popup = SearchablePopup::new(theme(), "Test", items());
        popup.insert_char('h');
        popup.insert_char('i');
        assert_eq!(popup.filtered.len(), 1);
        popup.backspace();
        popup.backspace();
        assert_eq!(popup.filtered.len(), 3);
    }

    #[test]
    fn select_next_prev() {
        let mut popup = SearchablePopup::new(theme(), "Test", items());
        assert_eq!(popup.selected, 0);
        popup.select_next();
        assert_eq!(popup.selected, 1);
        popup.select_next();
        assert_eq!(popup.selected, 2);
        popup.select_next();
        assert_eq!(popup.selected, 2);
        popup.select_prev();
        assert_eq!(popup.selected, 1);
    }

    #[test]
    fn selected_item_returns_correct() {
        let popup = SearchablePopup::new(theme(), "Test", items());
        assert_eq!(popup.selected_item().unwrap().label, "all tasks");
    }

    #[test]
    fn filter_clamps_selection() {
        let mut popup = SearchablePopup::new(theme(), "Test", items());
        popup.select_next();
        popup.select_next();
        popup.insert_char('h');
        popup.insert_char('i');
        assert_eq!(popup.selected, 0);
    }

    #[test]
    fn query_text_returns_typed() {
        let mut popup = SearchablePopup::new(theme(), "Test", items());
        popup.insert_char('x');
        popup.insert_char('y');
        assert_eq!(popup.query_text(), "xy");
    }

    #[test]
    fn filtered_is_empty_when_no_match() {
        let mut popup = SearchablePopup::new(theme(), "Test", items());
        popup.insert_char('z');
        popup.insert_char('z');
        popup.insert_char('z');
        assert!(popup.filtered_is_empty());
    }
}
