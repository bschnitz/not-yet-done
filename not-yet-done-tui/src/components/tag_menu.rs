//! Tag-management menu — modeled after `query_menu.rs` but trimmed.
//!
//! No Apply, no Shortcut. The user can:
//!   - select an existing entry → Toggle assignment to the currently
//!     selected task (Enter): assigns if not yet assigned, unassigns
//!     if already assigned
//!   - select an existing entry → Edit (Ctrl+E) opens the YAML form
//!   - type a name with no match → CreateNew (Enter) opens the form
//!     for a new tag and auto-assigns the result to the selected task
//!   - prefix the typed name with `+` → force CreateNew even if the
//!     name fuzzy-matches an existing entry (`+` is stripped)
//!   - delete the selected entry (Ctrl+D)
//!   - close (Esc)

use std::sync::Arc;

use ratatui::layout::Rect;
use ratatui::Frame;
use tuirealm::component::Component;

use crate::components::searchable_popup::{PopupItem, SearchablePopup};
use crate::config::keybindings::{
    KeyBindingSection, KeyIconMap, PopupAction, TagMenuAction,
};
use crate::ui::theme::Theme;

/// One row in the menu. `label` is what the user sees; `id` is the
/// stable tag-identifier (`global-tag:<uuid>` or `project-tag:<uuid>`)
/// the embedder uses for edit/delete dispatch.
#[derive(Debug, Clone)]
pub struct TagMenuEntry {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagMenuMessage {
    Unhandled,
    Handled,
    Closed,
    /// Enter on a selected entry — embedder toggles assignment of the
    /// tag to the currently selected task.
    ToggleAssign { id: String, label: String },
    /// Ctrl+E on a selected entry — open the YAML form for editing.
    EditExisting { id: String, label: String },
    Delete { id: String, label: String },
    CreateNew { name: String },
}

pub struct TagMenuComponent {
    theme: Arc<Theme>,
    title: String,
    popup: Option<SearchablePopup>,
    popup_kb: Option<KeyBindingSection<PopupAction>>,
    key_icons: Option<KeyIconMap>,
}

impl TagMenuComponent {
    pub fn new(theme: Arc<Theme>, title: impl Into<String>) -> Self {
        Self {
            theme,
            title: title.into(),
            popup: None,
            popup_kb: None,
            key_icons: None,
        }
    }

    pub fn with_popup_kb(
        mut self,
        kb: KeyBindingSection<PopupAction>,
        icons: KeyIconMap,
    ) -> Self {
        self.popup_kb = Some(kb);
        self.key_icons = Some(icons);
        self
    }

    pub fn is_open(&self) -> bool { self.popup.is_some() }

    pub fn close(&mut self) { self.popup = None; }

    pub fn open(
        &mut self,
        entries: &[TagMenuEntry],
        kb: &KeyBindingSection<TagMenuAction>,
    ) {
        let items: Vec<PopupItem> = entries.iter().map(|e| PopupItem {
            label: e.label.clone(),
            value: e.id.clone(),
        }).collect();
        let mut popup = SearchablePopup::new(
            Arc::clone(&self.theme),
            self.title.clone(),
            items,
        );
        if let (Some(pkb), Some(icons)) = (self.popup_kb.clone(), self.key_icons.clone()) {
            popup = popup.with_popup_kb(pkb, icons);
        }
        popup = popup.with_hints(vec![
            (kb.label(&TagMenuAction::Toggle), "toggle / new".into()),
            (kb.label(&TagMenuAction::Edit), "edit".into()),
            (kb.label(&TagMenuAction::Delete), "delete".into()),
            (kb.label(&TagMenuAction::Close), "close".into()),
        ]);
        self.popup = Some(popup);
    }

    pub fn handle_key(
        &mut self,
        key: &str,
        kb: &KeyBindingSection<TagMenuAction>,
    ) -> TagMenuMessage {
        if self.popup.is_none() { return TagMenuMessage::Unhandled; }

        if kb.get(&TagMenuAction::Close).is_some_and(|b| b.matches(key)) {
            self.popup = None;
            return TagMenuMessage::Closed;
        }
        if kb.get(&TagMenuAction::Toggle).is_some_and(|b| b.matches(key)) {
            let popup = self.popup.as_ref().unwrap();
            let typed = popup.query_text().trim().to_string();

            // `+`-prefix forces create even if there's an auto-selected
            // fuzzy match. Strip the prefix and treat the rest as the
            // new tag's name.
            if let Some(rest) = typed.strip_prefix('+') {
                let name = rest.trim().to_string();
                self.popup = None;
                if name.is_empty() { return TagMenuMessage::Closed; }
                return TagMenuMessage::CreateNew { name };
            }

            if let Some(item) = popup.selected_item() {
                let msg = TagMenuMessage::ToggleAssign {
                    id: item.value.clone(),
                    label: item.label.clone(),
                };
                self.popup = None;
                return msg;
            }

            self.popup = None;
            if typed.is_empty() { return TagMenuMessage::Closed; }
            return TagMenuMessage::CreateNew { name: typed };
        }
        if kb.get(&TagMenuAction::Edit).is_some_and(|b| b.matches(key)) {
            let popup = self.popup.as_ref().unwrap();
            if let Some(item) = popup.selected_item() {
                let msg = TagMenuMessage::EditExisting {
                    id: item.value.clone(),
                    label: item.label.clone(),
                };
                self.popup = None;
                return msg;
            }
            return TagMenuMessage::Handled;
        }
        if kb.get(&TagMenuAction::Next).is_some_and(|b| b.matches(key)) {
            self.popup.as_mut().unwrap().select_next();
            return TagMenuMessage::Handled;
        }
        if kb.get(&TagMenuAction::Prev).is_some_and(|b| b.matches(key)) {
            self.popup.as_mut().unwrap().select_prev();
            return TagMenuMessage::Handled;
        }
        if kb.get(&TagMenuAction::Delete).is_some_and(|b| b.matches(key)) {
            let popup = self.popup.as_ref().unwrap();
            if let Some(item) = popup.selected_item() {
                let msg = TagMenuMessage::Delete {
                    id: item.value.clone(),
                    label: item.label.clone(),
                };
                self.popup = None;
                return msg;
            }
            return TagMenuMessage::Handled;
        }

        // Navigation + text input — delegated to the popup's intrinsic
        // PopupAction bindings.
        self.popup.as_mut().unwrap().handle_key(key);
        TagMenuMessage::Handled
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
        m.insert(TagMenuAction::Edit, KeyBinding::new("ctrl+e"));
        m.insert(TagMenuAction::Next, KeyBinding::new("ctrl+j"));
        m.insert(TagMenuAction::Prev, KeyBinding::new("ctrl+k"));
        m.insert(TagMenuAction::Delete, KeyBinding::new("ctrl+d"));
        m.insert(TagMenuAction::Close, KeyBinding::new("esc"));
        KeyBindingSection { bindings: m }
    }

    fn entries() -> Vec<TagMenuEntry> {
        vec![
            TagMenuEntry { id: "global-tag:1".into(), label: "alpha".into() },
            TagMenuEntry { id: "global-tag:2".into(), label: "beta".into() },
        ]
    }

    fn theme() -> Arc<Theme> {
        Arc::new(Theme::new(Default::default()))
    }

    #[test]
    fn unhandled_when_closed() {
        let mut menu = TagMenuComponent::new(theme(), "T");
        let kb = make_kb();
        assert_eq!(menu.handle_key("enter", &kb), TagMenuMessage::Unhandled);
    }

    #[test]
    fn enter_emits_toggle_assign() {
        let mut menu = TagMenuComponent::new(theme(), "T");
        let kb = make_kb();
        menu.open(&entries(), &kb);
        let msg = menu.handle_key("enter", &kb);
        assert!(matches!(msg, TagMenuMessage::ToggleAssign { ref id, .. } if id == "global-tag:1"));
        assert!(!menu.is_open());
    }

    #[test]
    fn ctrl_e_emits_edit_existing() {
        let mut menu = TagMenuComponent::new(theme(), "T");
        let kb = make_kb();
        menu.open(&entries(), &kb);
        let msg = menu.handle_key("ctrl+e", &kb);
        assert!(matches!(msg, TagMenuMessage::EditExisting { ref id, .. } if id == "global-tag:1"));
        assert!(!menu.is_open());
    }

    #[test]
    fn typed_unknown_name_creates_new() {
        let mut menu = TagMenuComponent::new(theme(), "T");
        let kb = make_kb();
        menu.open(&entries(), &kb);
        for c in "xyz".chars() {
            menu.handle_key(&c.to_string(), &kb);
        }
        let msg = menu.handle_key("enter", &kb);
        assert_eq!(msg, TagMenuMessage::CreateNew { name: "xyz".into() });
    }

    #[test]
    fn plus_prefix_forces_create_even_if_match() {
        // "alpha" matches an existing entry, but "+alpha" forces create.
        let mut menu = TagMenuComponent::new(theme(), "T");
        let kb = make_kb();
        menu.open(&entries(), &kb);
        for c in "+alpha".chars() {
            menu.handle_key(&c.to_string(), &kb);
        }
        let msg = menu.handle_key("enter", &kb);
        assert_eq!(msg, TagMenuMessage::CreateNew { name: "alpha".into() });
    }

    #[test]
    fn ctrl_d_deletes_selected() {
        let mut menu = TagMenuComponent::new(theme(), "T");
        let kb = make_kb();
        menu.open(&entries(), &kb);
        let msg = menu.handle_key("ctrl+d", &kb);
        assert!(matches!(msg, TagMenuMessage::Delete { ref id, .. } if id == "global-tag:1"));
    }

    #[test]
    fn esc_closes_without_action() {
        let mut menu = TagMenuComponent::new(theme(), "T");
        let kb = make_kb();
        menu.open(&entries(), &kb);
        assert_eq!(menu.handle_key("esc", &kb), TagMenuMessage::Closed);
    }
}
