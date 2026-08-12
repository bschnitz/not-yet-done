//! Generic searchable list popup — reusable overlay with fuzzy search.
//!
//! Used for saved filter selection, script picker, etc.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use tuirealm::command::{Cmd, CmdResult};
use tuirealm::component::Component;
use tuirealm::props::{AttrValue, Attribute, QueryResult};
use tuirealm::state::{State, StateValue};

use crate::config::keybindings::{KeyBindingSection, KeyIconMap, PopupAction};
use crate::ui::popup_utils::{hints_height, render_hints_bar, render_popup_frame};
use crate::ui::theme::Theme;
use std::sync::Arc;

/// A single item in the searchable list.
#[derive(Default)]
pub struct PopupItem {
    pub label: String,
    /// Opaque payload returned when the item is selected.
    pub value: String,
    /// Renders a `★` marker in front of the label (e.g. the default
    /// saved query). Purely visual — not part of the filter text and
    /// not returned on selection.
    pub marked: bool,
    /// Dim text rendered after the label (e.g. a shortcut key). Like
    /// `marked`, display-only.
    pub suffix: Option<String>,
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
    pub fn with_popup_kb(mut self, kb: KeyBindingSection<PopupAction>, icons: KeyIconMap) -> Self {
        self.popup_kb = Some(kb);
        self.key_icons = Some(icons);
        self
    }

    pub fn insert_char(&mut self, c: char) {
        let byte_pos = self
            .query
            .char_indices()
            .nth(self.cursor)
            .map(|(i, _)| i)
            .unwrap_or(self.query.len());
        self.query.insert(byte_pos, c);
        self.cursor += 1;
        self.apply_filter();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 || self.query.is_empty() {
            return;
        }
        let byte_pos = self
            .query
            .char_indices()
            .nth(self.cursor - 1)
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.query.remove(byte_pos);
        self.cursor -= 1;
        self.apply_filter();
    }

    pub fn cursor_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub fn cursor_right(&mut self) {
        let max = self.query.chars().count();
        if self.cursor < max {
            self.cursor += 1;
        }
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

    /// Flip the `★` marker on the currently selected item. Used by the
    /// option menu for live multi-toggle: the embedder dispatches the
    /// toggle action async and reflects the new state in the open popup
    /// immediately, without rebuilding it.
    pub fn toggle_selected_marked(&mut self) {
        if let Some(&idx) = self.filtered.get(self.selected) {
            if let Some(item) = self.items.get_mut(idx) {
                item.marked = !item.marked;
            }
        }
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
            if kb
                .get(&PopupAction::Backspace)
                .is_some_and(|b| b.matches(key))
            {
                self.backspace();
                return PopupKeyOutcome::Handled;
            }
            if kb
                .get(&PopupAction::CursorLeft)
                .is_some_and(|b| b.matches(key))
            {
                self.cursor_left();
                return PopupKeyOutcome::Handled;
            }
            if kb
                .get(&PopupAction::CursorRight)
                .is_some_and(|b| b.matches(key))
            {
                self.cursor_right();
                return PopupKeyOutcome::Handled;
            }
        } else {
            // Legacy fallback — used by embedders that haven't called
            // with_popup_kb (and by all unit tests that exercise the
            // popup without a kb).
            match key {
                "down" => {
                    self.select_next();
                    return PopupKeyOutcome::Handled;
                }
                "up" => {
                    self.select_prev();
                    return PopupKeyOutcome::Handled;
                }
                "backspace" => {
                    self.backspace();
                    return PopupKeyOutcome::Handled;
                }
                "left" => {
                    self.cursor_left();
                    return PopupKeyOutcome::Handled;
                }
                "right" => {
                    self.cursor_right();
                    return PopupKeyOutcome::Handled;
                }
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
        self.filtered = self
            .items
            .iter()
            .enumerate()
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
        let t: &Theme = &self.theme;

        // Intrinsic hints render in front of embedder-supplied ones.
        let intrinsic = self.intrinsic_hints();
        let all_hints: Vec<(&str, &str)> = intrinsic
            .iter()
            .chain(self.hints.iter())
            .map(|(k, d)| (k.as_str(), d.as_str()))
            .collect();

        let popup_w = (area.width * 50 / 100)
            .max(30)
            .min(area.width.saturating_sub(4));
        let hints_h = if all_hints.is_empty() {
            0u16
        } else {
            hints_height(&all_hints, popup_w.saturating_sub(2))
        };
        let max_items = self.filtered.len() as u16;
        let popup_h = (max_items + 3 + hints_h).min(area.height * 60 / 100).max(5);

        // Shared popup chrome — same frame + hint bar as the column
        // config popup, so all pickers look alike.
        let inner = render_popup_frame(frame, area, t, &self.title, popup_w, popup_h);
        if inner.height == 0 || inner.width == 0 {
            return;
        }

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
                if px >= inner.right() {
                    break;
                }
                if let Some(cell) = buf.cell_mut(Position::new(px, input_y)) {
                    cell.set_char(ch);
                    cell.set_style(Style::default().fg(t.accent()).bg(input_bg));
                }
                px += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1) as u16;
            }

            let text_start_x = px;
            let max_w = inner.right().saturating_sub(text_start_x) as usize;
            let chars: Vec<char> = self.query.chars().collect();
            let view_start = if self.cursor >= max_w {
                self.cursor + 1 - max_w
            } else {
                0
            };

            for (screen_idx, char_idx) in (view_start..chars.len()).enumerate() {
                if screen_idx >= max_w {
                    break;
                }
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
                if cx < inner.right() {
                    Some(Position::new(cx, input_y))
                } else {
                    None
                }
            } else {
                None
            };

            // Items list. The cursor row gets a `surface_2` background
            // (matching the column config popup) instead of a colored
            // selection bar.
            let list_y = inner.y + 1;
            let list_h = inner.height.saturating_sub(1 + hints_h);
            let any_marked = self.items.iter().any(|i| i.marked);

            for (i, &item_idx) in self.filtered.iter().enumerate() {
                if i as u16 >= list_h {
                    break;
                }
                let item = &self.items[item_idx];
                let is_selected = i == self.selected;
                let bg = if is_selected { t.surface_2() } else { t.bg() };
                let row_y = list_y + i as u16;

                for cx in inner.left()..inner.right() {
                    if let Some(cell) = buf.cell_mut(Position::new(cx, row_y)) {
                        cell.set_char(' ');
                        cell.set_style(Style::default().bg(bg));
                    }
                }

                let mut spans: Vec<Span> = vec![Span::styled(" ", Style::default().bg(bg))];
                if any_marked {
                    let (glyph, style) = if item.marked {
                        ("★ ", Style::default().fg(t.accent()).bg(bg))
                    } else {
                        ("  ", Style::default().bg(bg))
                    };
                    spans.push(Span::styled(glyph, style));
                }
                spans.push(Span::styled(
                    item.label.clone(),
                    Style::default().fg(t.text_high()).bg(bg),
                ));
                if let Some(suffix) = &item.suffix {
                    spans.push(Span::styled(
                        format!(" {suffix}"),
                        Style::default().fg(t.text_dim()).bg(bg),
                    ));
                }
                let row_area = Rect {
                    x: inner.x,
                    y: row_y,
                    width: inner.width,
                    height: 1,
                };
                Line::from(spans).render(row_area, buf);
            }
        }

        // Hints bar with auto-wrap — shared with the column config popup.
        if hints_h > 0 {
            render_hints_bar(frame, inner, t, &all_hints, hints_h);
        }

        if let Some(pos) = cursor_pos {
            frame.set_cursor_position(pos);
        }
    }

    fn query(&self, _attr: Attribute) -> Option<QueryResult<'_>> {
        None
    }
    fn attr(&mut self, _attr: Attribute, _value: AttrValue) {}
    fn state(&self) -> State {
        State::Single(StateValue::String(self.query.clone()))
    }
    fn perform(&mut self, _cmd: Cmd) -> CmdResult {
        CmdResult::NoChange
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<PopupItem> {
        vec![
            PopupItem {
                label: "all tasks".into(),
                value: "{}".into(),
                ..Default::default()
            },
            PopupItem {
                label: "high priority".into(),
                value: "{}".into(),
                ..Default::default()
            },
            PopupItem {
                label: "done tasks".into(),
                value: "{}".into(),
                ..Default::default()
            },
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

    #[test]
    fn renders_marker_and_suffix() {
        use ratatui::{Terminal, backend::TestBackend};

        let items = vec![
            PopupItem {
                label: "plain".into(),
                value: "v".into(),
                ..Default::default()
            },
            PopupItem {
                label: "starred".into(),
                value: "v".into(),
                marked: true,
                suffix: Some("[1]".into()),
            },
        ];
        let mut popup = SearchablePopup::new(theme(), "Test", items);
        let mut terminal = Terminal::new(TestBackend::new(60, 14)).unwrap();
        terminal.draw(|f| popup.view(f, f.area())).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("★ starred"), "marker missing: {text}");
        assert!(text.contains("[1]"), "suffix missing: {text}");
        // Unmarked rows are indented to align with marked ones, no star.
        assert!(text.contains("  plain"), "indent missing: {text}");
        assert!(!text.contains("★ plain"));
    }
}
