//! Generic option-menu popup — a host-side, adapter-agnostic menu that
//! toggles values on the selected content node (e.g. tags).
//!
//! Driven by a `type: option_menu` action (see
//! [`crate::config::view_config::OptionMenuConfig`]). The options come from
//! the adapter via `list_values(source)`; the currently-selected values come
//! from a hidden node metadata field (the `marker`). On Enter the menu emits a
//! [`OptionMenuMessage::Toggle`] carrying the focused option's stable value;
//! the embedder dispatches the configured adapter action with that value in
//! `ActionContext.value`. Unlike the legacy tag menu the popup *stays open* so
//! several options can be toggled in one session, with the `★` marker flipping
//! live as each toggle is dispatched.
//!
//! Keybindings are shared with the tag menu ([`TagMenuAction`]): the menu shape
//! (Toggle / Next / Prev / Close — plus Edit / Delete reserved for a later
//! create/rename/delete step) is identical, so a second near-identical action
//! enum would be pure duplication.

use std::sync::Arc;

use ratatui::layout::Rect;
use ratatui::Frame;
use tuirealm::component::Component;

use crate::components::searchable_popup::{PopupItem, SearchablePopup};
use crate::config::keybindings::{KeyBindingSection, KeyIconMap, PopupAction, TagMenuAction};
use crate::ui::theme::Theme;

/// One selectable option. `value` is the adapter's stable id (handed back on
/// toggle and dispatched via `ActionContext.value`); `label` is shown to the
/// user; `assigned` pre-marks the option as currently selected on the node.
#[derive(Debug, Clone)]
pub struct OptionMenuEntry {
    pub value: String,
    pub label: String,
    pub assigned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionMenuMessage {
    Unhandled,
    Handled,
    Closed,
    /// Enter on a selected option — the embedder dispatches the configured
    /// toggle action with `value`. The popup has already flipped the option's
    /// `★` marker and stays open for further toggles.
    Toggle { value: String, label: String },
}

pub struct OptionMenuComponent {
    theme: Arc<Theme>,
    title: String,
    popup: Option<SearchablePopup>,
    popup_kb: Option<KeyBindingSection<PopupAction>>,
    key_icons: Option<KeyIconMap>,
}

impl OptionMenuComponent {
    pub fn new(theme: Arc<Theme>) -> Self {
        Self {
            theme,
            title: "Options".to_string(),
            popup: None,
            popup_kb: None,
            key_icons: None,
        }
    }

    pub fn with_popup_kb(mut self, kb: KeyBindingSection<PopupAction>, icons: KeyIconMap) -> Self {
        self.popup_kb = Some(kb);
        self.key_icons = Some(icons);
        self
    }

    pub fn is_open(&self) -> bool {
        self.popup.is_some()
    }

    /// Open the menu with the given options and title. Assigned options are
    /// pre-marked with `★`.
    pub fn open(
        &mut self,
        title: impl Into<String>,
        entries: &[OptionMenuEntry],
        kb: &KeyBindingSection<TagMenuAction>,
    ) {
        self.title = title.into();
        let items: Vec<PopupItem> = entries
            .iter()
            .map(|e| PopupItem {
                label: e.label.clone(),
                value: e.value.clone(),
                marked: e.assigned,
                ..Default::default()
            })
            .collect();
        let mut popup = SearchablePopup::new(Arc::clone(&self.theme), self.title.clone(), items);
        if let (Some(pkb), Some(icons)) = (self.popup_kb.clone(), self.key_icons.clone()) {
            popup = popup.with_popup_kb(pkb, icons);
        }
        popup = popup.with_hints(vec![
            (kb.label(&TagMenuAction::Toggle), "toggle".into()),
            (kb.label(&TagMenuAction::Close), "close".into()),
        ]);
        self.popup = Some(popup);
    }

    pub fn handle_key(
        &mut self,
        key: &str,
        kb: &KeyBindingSection<TagMenuAction>,
    ) -> OptionMenuMessage {
        if self.popup.is_none() {
            return OptionMenuMessage::Unhandled;
        }

        if kb.get(&TagMenuAction::Close).is_some_and(|b| b.matches(key)) {
            self.popup = None;
            return OptionMenuMessage::Closed;
        }
        if kb.get(&TagMenuAction::Toggle).is_some_and(|b| b.matches(key)) {
            let popup = self.popup.as_mut().unwrap();
            let Some(item) = popup.selected_item() else {
                return OptionMenuMessage::Handled;
            };
            let msg = OptionMenuMessage::Toggle {
                value: item.value.clone(),
                label: item.label.clone(),
            };
            // Live marker: flip the `★` now; the embedder dispatches the
            // toggle async and reloads the pane in the background. The menu
            // stays open so the user can keep toggling.
            popup.toggle_selected_marked();
            return msg;
        }
        if kb.get(&TagMenuAction::Next).is_some_and(|b| b.matches(key)) {
            self.popup.as_mut().unwrap().select_next();
            return OptionMenuMessage::Handled;
        }
        if kb.get(&TagMenuAction::Prev).is_some_and(|b| b.matches(key)) {
            self.popup.as_mut().unwrap().select_prev();
            return OptionMenuMessage::Handled;
        }

        // Navigation + text input — delegated to the popup's intrinsic
        // PopupAction bindings (and the typed-char search).
        self.popup.as_mut().unwrap().handle_key(key);
        OptionMenuMessage::Handled
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        if let Some(popup) = &mut self.popup {
            popup.view(frame, area);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::keybindings::{KeyBinding, KeyBindingSection};
    use std::collections::HashMap;

    fn make_kb() -> KeyBindingSection<TagMenuAction> {
        let mut m: HashMap<TagMenuAction, KeyBinding> = HashMap::new();
        m.insert(TagMenuAction::Toggle, KeyBinding::new("enter"));
        m.insert(TagMenuAction::Next, KeyBinding::new("ctrl+j"));
        m.insert(TagMenuAction::Prev, KeyBinding::new("ctrl+k"));
        m.insert(TagMenuAction::Close, KeyBinding::new("esc"));
        KeyBindingSection { bindings: m }
    }

    fn entries() -> Vec<OptionMenuEntry> {
        vec![
            OptionMenuEntry {
                value: "global-tag:1".into(),
                label: "alpha".into(),
                assigned: true,
            },
            OptionMenuEntry {
                value: "global-tag:2".into(),
                label: "beta".into(),
                assigned: false,
            },
        ]
    }

    fn theme() -> Arc<Theme> {
        Arc::new(Theme::new(Default::default()))
    }

    #[test]
    fn unhandled_when_closed() {
        let mut menu = OptionMenuComponent::new(theme());
        assert_eq!(menu.handle_key("enter", &make_kb()), OptionMenuMessage::Unhandled);
    }

    #[test]
    fn enter_emits_toggle_and_stays_open() {
        let mut menu = OptionMenuComponent::new(theme());
        let kb = make_kb();
        menu.open("Tags", &entries(), &kb);
        let msg = menu.handle_key("enter", &kb);
        assert!(matches!(msg, OptionMenuMessage::Toggle { ref value, .. } if value == "global-tag:1"));
        // Stays open for multi-toggle.
        assert!(menu.is_open());
    }

    #[test]
    fn esc_closes() {
        let mut menu = OptionMenuComponent::new(theme());
        let kb = make_kb();
        menu.open("Tags", &entries(), &kb);
        assert_eq!(menu.handle_key("esc", &kb), OptionMenuMessage::Closed);
        assert!(!menu.is_open());
    }

    #[test]
    fn toggle_flips_marker_live() {
        let mut menu = OptionMenuComponent::new(theme());
        let kb = make_kb();
        menu.open("Tags", &entries(), &kb);
        // First option starts assigned (★). Toggling should clear it.
        let popup = menu.popup.as_ref().unwrap();
        assert!(popup.selected_item().unwrap().marked);
        menu.handle_key("enter", &kb);
        let popup = menu.popup.as_ref().unwrap();
        assert!(!popup.selected_item().unwrap().marked);
    }
}
