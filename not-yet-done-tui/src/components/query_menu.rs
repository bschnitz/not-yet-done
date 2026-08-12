//! Query menu popup component — shared by ContentView, TasksView, TrackingsView.
//!
//! Wraps a `SearchablePopup` with a uniform set of actions:
//! Apply (Enter), Edit (ctrl+e), Delete (ctrl+d), EditShortcut (ctrl+s), Close.
//! When the user types a name with no match and presses Enter, a
//! `CreateNew` message is emitted so the embedder can open the editor for a
//! brand-new entry. Prefixing the typed name with `+` forces that even when
//! something is fuzzy-selected, and `++` asks for an extended document
//! instead of an adapter-native query body.

use std::sync::Arc;

use not_yet_done_content::QueryKind;

use ratatui::Frame;
use ratatui::layout::Rect;
use tuirealm::component::Component;

use crate::components::searchable_popup::{PopupItem, SearchablePopup};
use crate::config::keybindings::{KeyBindingSection, KeyIconMap, PopupAction, QueryMenuAction};
use crate::ui::theme::Theme;

/// One entry in the menu. The embedder is responsible for any merging
/// (e.g. ContentView merges YAML defaults + DB entries).
#[derive(Debug, Clone, Default)]
pub struct QueryMenuEntry {
    pub name: String,
    pub query: String,
    pub shortcut: Option<String>,
    /// Marks this entry as the default query (★ in the list). The
    /// default query is applied automatically on app start.
    pub is_default: bool,
}

/// Output of `handle_key` — describes what the embedder should do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryMenuMessage {
    /// Key was not recognised — pass through to other handlers.
    Unhandled,
    /// Key was consumed but produced no externally visible action.
    Handled,
    /// The popup was closed (Esc / Close binding / Enter on empty input).
    Closed,
    /// Apply the selected entry.
    Apply { name: String, query: String },
    /// Edit the selected entry's query in the editor.
    EditExisting { name: String, query: String },
    /// Delete the selected entry from the persistent store.
    Delete { name: String },
    /// Prompt for a new shortcut for the selected entry.
    EditShortcut { name: String, query: String },
    /// Remove the shortcut bound to the selected entry (query kept).
    ClearShortcut { name: String },
    /// Toggle the selected entry as the default query (embedder decides
    /// set-vs-clear and persists it).
    SetDefault { name: String },
    /// User typed a name (optionally `+name` / `++name`) — create a
    /// brand-new entry under that name. `kind` says which store the body
    /// belongs in; embedders that repurpose this menu for files ignore it.
    CreateNew { name: String, kind: QueryKind },
}

pub struct QueryMenuComponent {
    theme: Arc<Theme>,
    title: String,
    popup: Option<SearchablePopup>,
    /// Popup-intrinsic bindings forwarded to [`SearchablePopup::with_popup_kb`]
    /// so navigation (next/prev/backspace/cursor) is uniform across pickers
    /// and the hint bar auto-shows it.
    popup_kb: Option<KeyBindingSection<PopupAction>>,
    key_icons: Option<KeyIconMap>,
    /// Whether the current popup session supports marking a default
    /// query. Set per `open*` call — saved-query menus do, repurposed
    /// menus (e.g. the Postgres script picker) don't.
    set_default_enabled: bool,
}

impl QueryMenuComponent {
    pub fn new(theme: Arc<Theme>, title: impl Into<String>) -> Self {
        Self {
            theme,
            title: title.into(),
            popup: None,
            popup_kb: None,
            key_icons: None,
            set_default_enabled: false,
        }
    }

    /// Attach the shared popup keybindings + icon map. Without this, the
    /// embedded [`SearchablePopup`] keeps its legacy behaviour (no
    /// intrinsic Next/Prev hints, embedder must dispatch every key).
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

    /// Open the menu for saved queries — supports marking a default
    /// query (★) via [`QueryMenuAction::SetDefault`].
    pub fn open(&mut self, entries: &[QueryMenuEntry], kb: &KeyBindingSection<QueryMenuAction>) {
        self.open_inner(entries, kb, true);
    }

    /// Open the menu for non-query entries (e.g. the Postgres script
    /// picker reusing this component) — no default-query semantics.
    pub fn open_without_default(
        &mut self,
        entries: &[QueryMenuEntry],
        kb: &KeyBindingSection<QueryMenuAction>,
    ) {
        self.open_inner(entries, kb, false);
    }

    fn open_inner(
        &mut self,
        entries: &[QueryMenuEntry],
        kb: &KeyBindingSection<QueryMenuAction>,
        set_default_enabled: bool,
    ) {
        self.set_default_enabled = set_default_enabled;
        let items: Vec<PopupItem> = entries
            .iter()
            .map(|e| PopupItem {
                label: e.name.clone(),
                value: e.query.clone(),
                marked: set_default_enabled && e.is_default,
                suffix: e.shortcut.as_ref().map(|s| format!("[{s}]")),
            })
            .collect();
        let mut popup = SearchablePopup::new(Arc::clone(&self.theme), self.title.clone(), items);
        if let (Some(pkb), Some(icons)) = (self.popup_kb.clone(), self.key_icons.clone()) {
            popup = popup.with_popup_kb(pkb, icons);
        }
        // Embedder-specific hints; Next/Prev are rendered automatically
        // by the popup when popup_kb is attached.
        let mut hints = vec![
            (kb.label(&QueryMenuAction::Select), "apply / +new".into()),
            (kb.label(&QueryMenuAction::Edit), "edit".into()),
            (kb.label(&QueryMenuAction::EditShortcut), "shortcut".into()),
            (
                kb.label(&QueryMenuAction::ClearShortcut),
                "clear key".into(),
            ),
            (kb.label(&QueryMenuAction::Delete), "delete".into()),
        ];
        if set_default_enabled {
            hints.push((kb.label(&QueryMenuAction::SetDefault), "default".into()));
        }
        hints.push((kb.label(&QueryMenuAction::Close), "close".into()));
        popup = popup.with_hints(hints);
        self.popup = Some(popup);
    }

    pub fn handle_key(
        &mut self,
        key: &str,
        kb: &KeyBindingSection<QueryMenuAction>,
    ) -> QueryMenuMessage {
        if self.popup.is_none() {
            return QueryMenuMessage::Unhandled;
        }

        if kb
            .get(&QueryMenuAction::Close)
            .is_some_and(|b| b.matches(key))
        {
            self.popup = None;
            return QueryMenuMessage::Closed;
        }
        if kb
            .get(&QueryMenuAction::Select)
            .is_some_and(|b| b.matches(key))
        {
            let popup = self.popup.as_ref().unwrap();
            let typed = popup.query_text().trim().to_string();

            // A `+` prefix forces creation even when the typed text
            // fuzzy-matches an entry — without it, a name that is a
            // substring of an existing one could never be created. A
            // second `+` asks for an extended document; the two live in
            // separate stores but share one namespace, so the prefix is
            // the only place the user gets to say which one they mean.
            if let Some(rest) = typed.strip_prefix('+') {
                let (kind, name) = match rest.strip_prefix('+') {
                    Some(rest) => (QueryKind::Extended, rest.trim().to_string()),
                    None => (QueryKind::Saved, rest.trim().to_string()),
                };
                self.popup = None;
                if name.is_empty() {
                    return QueryMenuMessage::Closed;
                }
                return QueryMenuMessage::CreateNew { name, kind };
            }

            if let Some(item) = popup.selected_item() {
                let msg = QueryMenuMessage::Apply {
                    name: item.label.clone(),
                    query: item.value.clone(),
                };
                self.popup = None;
                return msg;
            }
            self.popup = None;
            if typed.is_empty() {
                return QueryMenuMessage::Closed;
            }
            return QueryMenuMessage::CreateNew {
                name: typed,
                kind: QueryKind::Saved,
            };
        }
        if kb
            .get(&QueryMenuAction::Next)
            .is_some_and(|b| b.matches(key))
        {
            self.popup.as_mut().unwrap().select_next();
            return QueryMenuMessage::Handled;
        }
        if kb
            .get(&QueryMenuAction::Prev)
            .is_some_and(|b| b.matches(key))
        {
            self.popup.as_mut().unwrap().select_prev();
            return QueryMenuMessage::Handled;
        }
        if kb
            .get(&QueryMenuAction::Edit)
            .is_some_and(|b| b.matches(key))
        {
            let popup = self.popup.as_ref().unwrap();
            if let Some(item) = popup.selected_item() {
                let msg = QueryMenuMessage::EditExisting {
                    name: item.label.clone(),
                    query: item.value.clone(),
                };
                self.popup = None;
                return msg;
            }
            return QueryMenuMessage::Handled;
        }
        if kb
            .get(&QueryMenuAction::Delete)
            .is_some_and(|b| b.matches(key))
        {
            let popup = self.popup.as_ref().unwrap();
            if let Some(item) = popup.selected_item() {
                let msg = QueryMenuMessage::Delete {
                    name: item.label.clone(),
                };
                self.popup = None;
                return msg;
            }
            return QueryMenuMessage::Handled;
        }
        if kb
            .get(&QueryMenuAction::EditShortcut)
            .is_some_and(|b| b.matches(key))
        {
            let popup = self.popup.as_ref().unwrap();
            if let Some(item) = popup.selected_item() {
                let msg = QueryMenuMessage::EditShortcut {
                    name: item.label.clone(),
                    query: item.value.clone(),
                };
                self.popup = None;
                return msg;
            }
            return QueryMenuMessage::Handled;
        }
        if kb
            .get(&QueryMenuAction::ClearShortcut)
            .is_some_and(|b| b.matches(key))
        {
            let popup = self.popup.as_ref().unwrap();
            if let Some(item) = popup.selected_item() {
                let msg = QueryMenuMessage::ClearShortcut {
                    name: item.label.clone(),
                };
                self.popup = None;
                return msg;
            }
            return QueryMenuMessage::Handled;
        }
        if self.set_default_enabled
            && kb
                .get(&QueryMenuAction::SetDefault)
                .is_some_and(|b| b.matches(key))
        {
            let popup = self.popup.as_ref().unwrap();
            if let Some(item) = popup.selected_item() {
                let msg = QueryMenuMessage::SetDefault {
                    name: item.label.clone(),
                };
                self.popup = None;
                return msg;
            }
            return QueryMenuMessage::Handled;
        }

        // Navigation + text input — delegated to the popup's intrinsic
        // PopupAction bindings.
        self.popup.as_mut().unwrap().handle_key(key);
        QueryMenuMessage::Handled
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

    fn make_kb() -> KeyBindingSection<QueryMenuAction> {
        let mut m: HashMap<QueryMenuAction, KeyBinding> = HashMap::new();
        m.insert(QueryMenuAction::Select, KeyBinding::new("enter"));
        m.insert(QueryMenuAction::Next, KeyBinding::new("ctrl+j"));
        m.insert(QueryMenuAction::Prev, KeyBinding::new("ctrl+k"));
        m.insert(QueryMenuAction::Edit, KeyBinding::new("ctrl+e"));
        m.insert(QueryMenuAction::Delete, KeyBinding::new("ctrl+d"));
        m.insert(QueryMenuAction::EditShortcut, KeyBinding::new("ctrl+s"));
        m.insert(QueryMenuAction::SetDefault, KeyBinding::new("ctrl+t"));
        m.insert(QueryMenuAction::Close, KeyBinding::new("esc"));
        KeyBindingSection { bindings: m }
    }

    fn entries() -> Vec<QueryMenuEntry> {
        vec![
            QueryMenuEntry {
                name: "alpha".into(),
                query: "Q1".into(),
                shortcut: None,
                is_default: false,
            },
            QueryMenuEntry {
                name: "beta".into(),
                query: "Q2".into(),
                shortcut: Some("1".into()),
                is_default: true,
            },
        ]
    }

    fn theme() -> Arc<Theme> {
        Arc::new(Theme::new(Default::default()))
    }

    #[test]
    fn unhandled_when_closed() {
        let mut menu = QueryMenuComponent::new(theme(), "T");
        let kb = make_kb();
        assert_eq!(menu.handle_key("enter", &kb), QueryMenuMessage::Unhandled);
    }

    #[test]
    fn select_applies_first_item() {
        let mut menu = QueryMenuComponent::new(theme(), "T");
        let kb = make_kb();
        menu.open(&entries(), &kb);
        let msg = menu.handle_key("enter", &kb);
        assert!(matches!(msg, QueryMenuMessage::Apply { ref name, .. } if name == "alpha"));
        assert!(!menu.is_open());
    }

    #[test]
    fn typed_name_with_no_match_creates_new() {
        let mut menu = QueryMenuComponent::new(theme(), "T");
        let kb = make_kb();
        menu.open(&entries(), &kb);
        // Type "xyz" — no entry matches, so on Enter we get CreateNew.
        for c in "xyz".chars() {
            menu.handle_key(&c.to_string(), &kb);
        }
        let msg = menu.handle_key("enter", &kb);
        assert_eq!(
            msg,
            QueryMenuMessage::CreateNew {
                name: "xyz".into(),
                kind: QueryKind::Saved,
            }
        );
        assert!(!menu.is_open());
    }

    #[test]
    fn plus_prefix_creates_even_when_an_entry_is_selected() {
        let mut menu = QueryMenuComponent::new(theme(), "T");
        let kb = make_kb();
        menu.open(&entries(), &kb);
        // "alph" fuzzy-matches "alpha"; the prefix says "new one anyway".
        for c in "+alph".chars() {
            menu.handle_key(&c.to_string(), &kb);
        }
        let msg = menu.handle_key("enter", &kb);
        assert_eq!(
            msg,
            QueryMenuMessage::CreateNew {
                name: "alph".into(),
                kind: QueryKind::Saved,
            }
        );
    }

    #[test]
    fn double_plus_prefix_creates_an_extended_query() {
        let mut menu = QueryMenuComponent::new(theme(), "T");
        let kb = make_kb();
        menu.open(&entries(), &kb);
        for c in "++combo".chars() {
            menu.handle_key(&c.to_string(), &kb);
        }
        let msg = menu.handle_key("enter", &kb);
        assert_eq!(
            msg,
            QueryMenuMessage::CreateNew {
                name: "combo".into(),
                kind: QueryKind::Extended,
            }
        );
    }

    #[test]
    fn a_bare_plus_closes_instead_of_creating_a_nameless_entry() {
        let mut menu = QueryMenuComponent::new(theme(), "T");
        let kb = make_kb();
        menu.open(&entries(), &kb);
        menu.handle_key("+", &kb);
        assert_eq!(menu.handle_key("enter", &kb), QueryMenuMessage::Closed);
    }

    #[test]
    fn edit_emits_edit_existing() {
        let mut menu = QueryMenuComponent::new(theme(), "T");
        let kb = make_kb();
        menu.open(&entries(), &kb);
        let msg = menu.handle_key("ctrl+e", &kb);
        assert!(
            matches!(msg, QueryMenuMessage::EditExisting { ref name, ref query, .. } if name == "alpha" && query == "Q1")
        );
        assert!(!menu.is_open());
    }

    #[test]
    fn delete_emits_delete() {
        let mut menu = QueryMenuComponent::new(theme(), "T");
        let kb = make_kb();
        menu.open(&entries(), &kb);
        let msg = menu.handle_key("ctrl+d", &kb);
        assert_eq!(
            msg,
            QueryMenuMessage::Delete {
                name: "alpha".into()
            }
        );
    }

    #[test]
    fn edit_shortcut_emits_edit_shortcut() {
        let mut menu = QueryMenuComponent::new(theme(), "T");
        let kb = make_kb();
        menu.open(&entries(), &kb);
        let msg = menu.handle_key("ctrl+s", &kb);
        assert!(matches!(msg, QueryMenuMessage::EditShortcut { ref name, .. } if name == "alpha"));
    }

    #[test]
    fn set_default_emits_for_selected_entry() {
        let mut menu = QueryMenuComponent::new(theme(), "T");
        let kb = make_kb();
        menu.open(&entries(), &kb);
        let msg = menu.handle_key("ctrl+t", &kb);
        assert_eq!(
            msg,
            QueryMenuMessage::SetDefault {
                name: "alpha".into()
            }
        );
        assert!(!menu.is_open());
    }

    #[test]
    fn set_default_disabled_without_default_support() {
        let mut menu = QueryMenuComponent::new(theme(), "T");
        let kb = make_kb();
        menu.open_without_default(&entries(), &kb);
        // The key falls through to the popup (typed/ignored), never
        // emitting SetDefault.
        let msg = menu.handle_key("ctrl+t", &kb);
        assert_eq!(msg, QueryMenuMessage::Handled);
        assert!(menu.is_open());
    }

    #[test]
    fn esc_closes_without_action() {
        let mut menu = QueryMenuComponent::new(theme(), "T");
        let kb = make_kb();
        menu.open(&entries(), &kb);
        let msg = menu.handle_key("esc", &kb);
        assert_eq!(msg, QueryMenuMessage::Closed);
    }
}
