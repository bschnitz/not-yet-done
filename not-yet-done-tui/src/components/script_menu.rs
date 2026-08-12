//! Script management menu — modeled after `tag_menu.rs`.
//!
//! Lists the script files under a context-specific directory. The user
//! can:
//!   - select an existing entry → Run (Enter) executes the script
//!   - select an existing entry → Edit (Ctrl+E) opens it in the editor
//!   - type a name with no match → CreateNew (Enter) opens the editor
//!     pre-filled with the configured template, using the typed value
//!     as the filename (suffix is added by the embedder when missing)
//!   - prefix the typed name with `+` → force CreateNew even when the
//!     name fuzzy-matches an existing entry (`+` is stripped)
//!   - delete the selected entry (Ctrl+D)
//!   - close (Esc)
//!
//! The component is context-agnostic: the embedder builds entries from
//! the appropriate `<scripts_dir>/*` listing and decides what running /
//! editing / deleting does. See `app/script.rs`.

use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::Rect;
use tuirealm::component::Component;

use crate::components::searchable_popup::{PopupItem, SearchablePopup};
use crate::config::keybindings::{KeyBindingSection, KeyIconMap, PopupAction, ScriptMenuAction};
use crate::ui::theme::Theme;

/// One row in the menu. `path` is the absolute script-file path used
/// for run/edit/delete dispatch; `label` is what the user sees (the
/// bare filename).
#[derive(Debug, Clone)]
pub struct ScriptMenuEntry {
    pub path: String,
    pub label: String,
    /// Key chord bound to this script (shown as a `[chord]` suffix), or
    /// `None` when no shortcut is assigned.
    pub shortcut: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptMenuMessage {
    Unhandled,
    Handled,
    Closed,
    /// Enter on a selected entry — run the script.
    Run {
        path: String,
        label: String,
    },
    /// Ctrl+E on a selected entry — open the script in the editor.
    Edit {
        path: String,
        label: String,
    },
    /// Ctrl+S on a selected entry — prompt for a key chord to bind to it.
    EditShortcut {
        path: String,
        label: String,
    },
    /// Ctrl+D on a selected entry — delete the script file.
    Delete {
        path: String,
        label: String,
    },
    /// Typed name (or `+name`) + Enter with no fuzzy-selected entry —
    /// create a new script file under the context's scripts dir using
    /// `name` as the filename.
    CreateNew {
        name: String,
    },
}

pub struct ScriptMenuComponent {
    theme: Arc<Theme>,
    title: String,
    popup: Option<SearchablePopup>,
    popup_kb: Option<KeyBindingSection<PopupAction>>,
    key_icons: Option<KeyIconMap>,
}

impl ScriptMenuComponent {
    pub fn new(theme: Arc<Theme>, title: impl Into<String>) -> Self {
        Self {
            theme,
            title: title.into(),
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

    pub fn close(&mut self) {
        self.popup = None;
    }

    pub fn open(&mut self, entries: &[ScriptMenuEntry], kb: &KeyBindingSection<ScriptMenuAction>) {
        let items: Vec<PopupItem> = entries
            .iter()
            .map(|e| PopupItem {
                label: e.label.clone(),
                value: e.path.clone(),
                suffix: e.shortcut.as_ref().map(|s| format!("[{s}]")),
                ..Default::default()
            })
            .collect();
        let mut popup = SearchablePopup::new(Arc::clone(&self.theme), self.title.clone(), items);
        if let (Some(pkb), Some(icons)) = (self.popup_kb.clone(), self.key_icons.clone()) {
            popup = popup.with_popup_kb(pkb, icons);
        }
        popup = popup.with_hints(vec![
            (kb.label(&ScriptMenuAction::Run), "run / new".into()),
            (kb.label(&ScriptMenuAction::Edit), "edit".into()),
            (kb.label(&ScriptMenuAction::EditShortcut), "shortcut".into()),
            (kb.label(&ScriptMenuAction::Delete), "delete".into()),
            (kb.label(&ScriptMenuAction::Close), "close".into()),
        ]);
        self.popup = Some(popup);
    }

    pub fn handle_key(
        &mut self,
        key: &str,
        kb: &KeyBindingSection<ScriptMenuAction>,
    ) -> ScriptMenuMessage {
        if self.popup.is_none() {
            return ScriptMenuMessage::Unhandled;
        }

        if kb
            .get(&ScriptMenuAction::Close)
            .is_some_and(|b| b.matches(key))
        {
            self.popup = None;
            return ScriptMenuMessage::Closed;
        }
        if kb
            .get(&ScriptMenuAction::Run)
            .is_some_and(|b| b.matches(key))
        {
            let popup = self.popup.as_ref().unwrap();
            let typed = popup.query_text().trim().to_string();

            // `+`-prefix forces create even when there's an auto-selected
            // fuzzy match. Strip the prefix and treat the rest as the
            // new script's filename.
            if let Some(rest) = typed.strip_prefix('+') {
                let name = rest.trim().to_string();
                self.popup = None;
                if name.is_empty() {
                    return ScriptMenuMessage::Closed;
                }
                return ScriptMenuMessage::CreateNew { name };
            }

            if let Some(item) = popup.selected_item() {
                let msg = ScriptMenuMessage::Run {
                    path: item.value.clone(),
                    label: item.label.clone(),
                };
                self.popup = None;
                return msg;
            }

            self.popup = None;
            if typed.is_empty() {
                return ScriptMenuMessage::Closed;
            }
            return ScriptMenuMessage::CreateNew { name: typed };
        }
        if kb
            .get(&ScriptMenuAction::Edit)
            .is_some_and(|b| b.matches(key))
        {
            let popup = self.popup.as_ref().unwrap();
            if let Some(item) = popup.selected_item() {
                let msg = ScriptMenuMessage::Edit {
                    path: item.value.clone(),
                    label: item.label.clone(),
                };
                self.popup = None;
                return msg;
            }
            return ScriptMenuMessage::Handled;
        }
        if kb
            .get(&ScriptMenuAction::EditShortcut)
            .is_some_and(|b| b.matches(key))
        {
            let popup = self.popup.as_ref().unwrap();
            if let Some(item) = popup.selected_item() {
                let msg = ScriptMenuMessage::EditShortcut {
                    path: item.value.clone(),
                    label: item.label.clone(),
                };
                self.popup = None;
                return msg;
            }
            return ScriptMenuMessage::Handled;
        }
        if kb
            .get(&ScriptMenuAction::Next)
            .is_some_and(|b| b.matches(key))
        {
            self.popup.as_mut().unwrap().select_next();
            return ScriptMenuMessage::Handled;
        }
        if kb
            .get(&ScriptMenuAction::Prev)
            .is_some_and(|b| b.matches(key))
        {
            self.popup.as_mut().unwrap().select_prev();
            return ScriptMenuMessage::Handled;
        }
        if kb
            .get(&ScriptMenuAction::Delete)
            .is_some_and(|b| b.matches(key))
        {
            let popup = self.popup.as_ref().unwrap();
            if let Some(item) = popup.selected_item() {
                let msg = ScriptMenuMessage::Delete {
                    path: item.value.clone(),
                    label: item.label.clone(),
                };
                self.popup = None;
                return msg;
            }
            return ScriptMenuMessage::Handled;
        }

        // Navigation + text input — delegated to the popup's intrinsic
        // PopupAction bindings.
        self.popup.as_mut().unwrap().handle_key(key);
        ScriptMenuMessage::Handled
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

    fn make_kb() -> KeyBindingSection<ScriptMenuAction> {
        let mut m: HashMap<ScriptMenuAction, KeyBinding> = HashMap::new();
        m.insert(ScriptMenuAction::Run, KeyBinding::new("enter"));
        m.insert(ScriptMenuAction::Edit, KeyBinding::new("ctrl+e"));
        m.insert(ScriptMenuAction::EditShortcut, KeyBinding::new("ctrl+s"));
        m.insert(ScriptMenuAction::Next, KeyBinding::new("ctrl+j"));
        m.insert(ScriptMenuAction::Prev, KeyBinding::new("ctrl+k"));
        m.insert(ScriptMenuAction::Delete, KeyBinding::new("ctrl+d"));
        m.insert(ScriptMenuAction::Close, KeyBinding::new("esc"));
        KeyBindingSection { bindings: m }
    }

    fn entries() -> Vec<ScriptMenuEntry> {
        vec![
            ScriptMenuEntry {
                path: "/x/alpha.py".into(),
                label: "alpha.py".into(),
                shortcut: None,
            },
            ScriptMenuEntry {
                path: "/x/beta.py".into(),
                label: "beta.py".into(),
                shortcut: Some("1".into()),
            },
        ]
    }

    fn theme() -> Arc<Theme> {
        Arc::new(Theme::new(Default::default()))
    }

    #[test]
    fn unhandled_when_closed() {
        let mut menu = ScriptMenuComponent::new(theme(), "T");
        let kb = make_kb();
        assert_eq!(menu.handle_key("enter", &kb), ScriptMenuMessage::Unhandled);
    }

    #[test]
    fn enter_runs_selected() {
        let mut menu = ScriptMenuComponent::new(theme(), "T");
        let kb = make_kb();
        menu.open(&entries(), &kb);
        let msg = menu.handle_key("enter", &kb);
        assert!(matches!(msg, ScriptMenuMessage::Run { ref path, .. } if path == "/x/alpha.py"));
        assert!(!menu.is_open());
    }

    #[test]
    fn ctrl_e_emits_edit() {
        let mut menu = ScriptMenuComponent::new(theme(), "T");
        let kb = make_kb();
        menu.open(&entries(), &kb);
        let msg = menu.handle_key("ctrl+e", &kb);
        assert!(matches!(msg, ScriptMenuMessage::Edit { ref path, .. } if path == "/x/alpha.py"));
        assert!(!menu.is_open());
    }

    #[test]
    fn typed_unknown_name_creates_new() {
        let mut menu = ScriptMenuComponent::new(theme(), "T");
        let kb = make_kb();
        menu.open(&entries(), &kb);
        for c in "zzz".chars() {
            menu.handle_key(&c.to_string(), &kb);
        }
        let msg = menu.handle_key("enter", &kb);
        assert_eq!(msg, ScriptMenuMessage::CreateNew { name: "zzz".into() });
    }

    #[test]
    fn plus_prefix_forces_create_even_if_match() {
        let mut menu = ScriptMenuComponent::new(theme(), "T");
        let kb = make_kb();
        menu.open(&entries(), &kb);
        for c in "+alpha".chars() {
            menu.handle_key(&c.to_string(), &kb);
        }
        let msg = menu.handle_key("enter", &kb);
        assert_eq!(
            msg,
            ScriptMenuMessage::CreateNew {
                name: "alpha".into()
            }
        );
    }

    #[test]
    fn ctrl_s_emits_edit_shortcut() {
        let mut menu = ScriptMenuComponent::new(theme(), "T");
        let kb = make_kb();
        menu.open(&entries(), &kb);
        let msg = menu.handle_key("ctrl+s", &kb);
        assert!(
            matches!(msg, ScriptMenuMessage::EditShortcut { ref path, .. } if path == "/x/alpha.py")
        );
        assert!(!menu.is_open());
    }

    #[test]
    fn ctrl_d_deletes_selected() {
        let mut menu = ScriptMenuComponent::new(theme(), "T");
        let kb = make_kb();
        menu.open(&entries(), &kb);
        let msg = menu.handle_key("ctrl+d", &kb);
        assert!(matches!(msg, ScriptMenuMessage::Delete { ref path, .. } if path == "/x/alpha.py"));
    }

    #[test]
    fn esc_closes_without_action() {
        let mut menu = ScriptMenuComponent::new(theme(), "T");
        let kb = make_kb();
        menu.open(&entries(), &kb);
        assert_eq!(menu.handle_key("esc", &kb), ScriptMenuMessage::Closed);
    }
}
