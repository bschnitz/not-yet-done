//! Generic searchable list popup — reusable overlay with fuzzy search.
//!
//! Used for saved query selection, the script picker, the `gl` link popup,
//! the config picker and the option menu. Chrome is the shared floating
//! panel ([`crate::ui::panel_chrome`]) and the list itself is a
//! [`LeaderList`], so a popup looks and behaves like the shortcut menu:
//! label on the left, an optional dim suffix flush right, a fuzzy filter
//! prompt on the first row, a status line with the page counter — and
//! scrolling once there are more entries than rows.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use tuirealm::command::{Cmd, CmdResult, Direction};
use tuirealm::component::Component;
use tuirealm::props::{AttrValue, Attribute, QueryResult};
use tuirealm::state::{State, StateValue};

use not_yet_done_ratatui::LeaderList;

use crate::components::key_recorder::{
    KeyRecorder, RECORDING_HINTS, RecorderStep, recording_heading,
};
use crate::config::keybindings::{KeyBindingSection, KeyIconMap, PopupAction};
use crate::ui::panel_chrome::{PanelChrome, panel_leader_style};
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
    items: Vec<PopupItem>,
    /// Renders the items and owns the live filter, cursor and scroll state.
    list: LeaderList,
    hints: Vec<(String, String)>, // (key_label, description) — embedder-supplied
    /// Popup-intrinsic key bindings (Next/Prev/Backspace/Cursor). When set,
    /// the navigation hints render automatically in the hint bar in front
    /// of the embedder-supplied hints, and `handle_key` is routable.
    popup_kb: Option<KeyBindingSection<PopupAction>>,
    /// Icon map used when rendering intrinsic hints; if `None`, raw key
    /// strings are shown.
    key_icons: Option<KeyIconMap>,
    /// Live shortcut recording, if the embedder started one (the query and
    /// script menus bind a chord to the selected entry without closing).
    /// While `Some`, the heading and hints turn into the recorder's, and the
    /// embedder feeds every key through [`Self::feed_recorder`].
    recorder: Option<KeyRecorder>,
    /// What the running recording binds, shown dim in the recorder heading.
    record_subject: String,
}

impl SearchablePopup {
    pub fn new(theme: Arc<Theme>, title: impl Into<String>, items: Vec<PopupItem>) -> Self {
        let mut list = LeaderList::default()
            .with_entries(Self::entries(&items))
            .with_affixes(" ", " ", " ")
            .with_selectable(true)
            .with_status_line(true)
            .with_search(true)
            .with_style(panel_leader_style(&theme));
        list.attr(Attribute::Focus, AttrValue::Flag(true));
        Self {
            theme,
            title: title.into(),
            items,
            list,
            hints: Vec::new(),
            popup_kb: None,
            key_icons: None,
            recorder: None,
            record_subject: String::new(),
        }
    }

    /// The `(left, right)` pairs handed to the [`LeaderList`]: the label
    /// (with the `★` marker in front when any item carries one, so marked and
    /// unmarked rows stay aligned) and the dim suffix.
    fn entries(items: &[PopupItem]) -> Vec<(String, String)> {
        let any_marked = items.iter().any(|i| i.marked);
        items
            .iter()
            .map(|item| {
                let label = if !any_marked {
                    item.label.clone()
                } else if item.marked {
                    format!("\u{2605} {}", item.label)
                } else {
                    format!("  {}", item.label)
                };
                (label, item.suffix.clone().unwrap_or_default())
            })
            .collect()
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

    /// Append a character to the filter. The filter is append-only (the
    /// [`LeaderList`] prompt has no text cursor), so this always types at the
    /// end.
    pub fn insert_char(&mut self, c: char) {
        self.list.push_search(c);
    }

    pub fn backspace(&mut self) {
        self.list.backspace_search();
    }

    pub fn select_next(&mut self) {
        self.list.perform(Cmd::Move(Direction::Down));
    }

    pub fn select_prev(&mut self) {
        self.list.perform(Cmd::Move(Direction::Up));
    }

    pub fn selected_item(&self) -> Option<&PopupItem> {
        self.items.get(self.list.selected_index()?)
    }

    /// Flip the `★` marker on the currently selected item. Used by the
    /// option menu for live multi-toggle: the embedder dispatches the
    /// toggle action async and reflects the new state in the open popup
    /// immediately, without rebuilding it.
    pub fn toggle_selected_marked(&mut self) {
        let Some(idx) = self.list.selected_index() else {
            return;
        };
        if let Some(item) = self.items.get_mut(idx) {
            item.marked = !item.marked;
        }
        // The marker lives in the rendered label, so the entries have to be
        // rebuilt — carry the live filter and cursor across it.
        let query = self.list.search_query().to_string();
        let cursor = self.list.selected();
        self.list.set_entries(Self::entries(&self.items));
        self.list.set_search_query(query);
        self.list.attr(Attribute::Value, AttrValue::Length(cursor));
    }

    /// The current search query text.
    pub fn query_text(&self) -> &str {
        self.list.search_query()
    }

    /// Whether the filtered list is empty (no matches for current query).
    pub fn filtered_is_empty(&self) -> bool {
        self.list.selected_index().is_none()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Start recording a key chord for `subject` (the entry being bound).
    /// The popup stays open and keeps showing its list; only the heading and
    /// the hint line switch to the recorder's. The chord replaces whatever the
    /// subject is bound to, so the recording always runs in overwrite mode —
    /// the DB-stored shortcuts hold exactly one binding.
    pub fn start_recording(&mut self, subject: impl Into<String>) {
        self.recorder = Some(KeyRecorder::new(true));
        self.record_subject = subject.into();
    }

    pub fn is_recording(&self) -> bool {
        self.recorder.is_some()
    }

    /// Feed a key into a running recording. Returns `None` when nothing is
    /// being recorded, so embedders can call this first in their key handler
    /// and fall through to normal dispatch. A finished recording (saved or
    /// cancelled) clears itself.
    pub fn feed_recorder(&mut self, key: &str) -> Option<RecorderStep> {
        let step = self.recorder.as_mut()?.feed(key);
        if !matches!(step, RecorderStep::Recording) {
            self.recorder = None;
            self.record_subject.clear();
        }
        Some(step)
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
            // The filter prompt has no text cursor; the keys stay bound so
            // they are swallowed here instead of leaking to the embedder.
            if kb
                .get(&PopupAction::CursorLeft)
                .is_some_and(|b| b.matches(key))
                || kb
                    .get(&PopupAction::CursorRight)
                    .is_some_and(|b| b.matches(key))
            {
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
                "left" | "right" => return PopupKeyOutcome::Handled,
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
        if !self.query_text().is_empty() {
            hints.push((
                kb.hint_label(&PopupAction::Backspace, icons),
                "erase".to_string(),
            ));
        }
        hints
    }
}

impl Component for SearchablePopup {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let t = Arc::clone(&self.theme);

        // While recording, the recorder owns the heading and the hint line —
        // the list stays visible underneath, exactly as in the shortcut menu.
        let intrinsic = self.intrinsic_hints();
        let hints: Vec<(&str, &str)> = if self.recorder.is_some() {
            RECORDING_HINTS.to_vec()
        } else {
            intrinsic
                .iter()
                .chain(self.hints.iter())
                .map(|(k, d)| (k.as_str(), d.as_str()))
                .collect()
        };

        let heading = match self.recorder.as_ref() {
            Some(rec) => recording_heading(rec, &self.record_subject, &t),
            None => Line::from(vec![Span::styled(
                format!("\u{2726} {}", self.title),
                Style::default()
                    .fg(t.form_accent())
                    .add_modifier(Modifier::BOLD),
            )]),
        };

        // Body rows: the visible entries plus the filter prompt and the
        // status line. Long lists are capped at 60% of the available height
        // and scroll from there.
        let rows = self.list.visible_indices().len() as u16 + 2;
        let cap = (area.height * 3 / 5).max(5);
        let body = PanelChrome::new(heading)
            .hints(hints)
            .body(self.list.min_width() as usize, rows.min(cap))
            .render(frame, area, &t);

        if let Some(body) = body {
            self.list.view(frame, body);
            crate::mouse::push_list_rows(&self.list);
        }
    }

    fn query(&self, _attr: Attribute) -> Option<QueryResult<'_>> {
        None
    }
    fn attr(&mut self, _attr: Attribute, _value: AttrValue) {}
    fn state(&self) -> State {
        State::Single(StateValue::String(self.query_text().to_string()))
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
        assert_eq!(popup.list.visible_indices().len(), 3);
        assert_eq!(popup.list.selected(), 0);
    }

    #[test]
    fn filter_narrows_list() {
        let mut popup = SearchablePopup::new(theme(), "Test", items());
        popup.insert_char('h');
        popup.insert_char('i');
        assert_eq!(popup.selected_item().unwrap().label, "high priority");
    }

    #[test]
    fn backspace_widens_list() {
        let mut popup = SearchablePopup::new(theme(), "Test", items());
        popup.insert_char('h');
        popup.insert_char('i');
        assert_eq!(popup.list.visible_indices().len(), 1);
        popup.backspace();
        popup.backspace();
        assert_eq!(popup.list.visible_indices().len(), 3);
    }

    #[test]
    fn select_next_prev() {
        let mut popup = SearchablePopup::new(theme(), "Test", items());
        assert_eq!(popup.list.selected(), 0);
        popup.select_next();
        assert_eq!(popup.list.selected(), 1);
        popup.select_next();
        assert_eq!(popup.list.selected(), 2);
        popup.select_next();
        assert_eq!(popup.list.selected(), 2);
        popup.select_prev();
        assert_eq!(popup.list.selected(), 1);
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
        assert_eq!(popup.list.selected(), 0);
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
    fn toggle_marked_keeps_filter_and_cursor() {
        let mut popup = SearchablePopup::new(theme(), "Test", items());
        popup.insert_char('t');
        popup.select_next();
        let before = popup.selected_item().unwrap().label.clone();
        popup.toggle_selected_marked();
        assert_eq!(popup.query_text(), "t");
        assert_eq!(popup.selected_item().unwrap().label, before);
        assert!(popup.selected_item().unwrap().marked);
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
