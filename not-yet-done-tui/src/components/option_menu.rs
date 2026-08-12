//! Generic option-menu popup — a host-side, adapter-agnostic menu that
//! toggles values on the selected content node (e.g. tags) and, optionally,
//! creates / renames / deletes the underlying options.
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
//! Beyond toggling, the menu hosts three optional sub-flows — gated by the
//! capabilities the embedder enables ([`OptionMenuCaps`], which mirror the
//! configured create/rename/delete action ids):
//!
//! - **Create** (`Ctrl+N`) and **Rename** (`Ctrl+E`) open an inline text
//!   prompt; on submit the menu emits [`OptionMenuMessage::Submit`] with the
//!   typed text (and, for rename, the focused option's id).
//! - **Delete** (`Ctrl+D`) opens an inline `(y/n)` confirmation; on `y` the
//!   menu emits [`OptionMenuMessage::Delete`].
//!
//! The confirmation lives *inside* the popup (rather than going through the
//! global confirm plumbing) so the modal popup keeps owning the keyboard.
//!
//! Keybindings are shared with the tag menu ([`TagMenuAction`]): the menu shape
//! (Toggle / Create / Edit / Delete / Next / Prev / Close) is identical, so a
//! second near-identical action enum would be pure duplication.

use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use tuirealm::component::Component;

use crate::components::searchable_popup::{PopupItem, SearchablePopup};
use crate::config::keybindings::{KeyBindingSection, KeyIconMap, PopupAction, TagMenuAction};
use crate::ui::popup_utils::render_popup_frame;
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

/// Which option-store mutations the embedder wired up (mirrors the configured
/// `create`/`rename`/`delete` action ids being present). A disabled verb's key
/// is inert and its hint is hidden.
#[derive(Debug, Clone, Copy, Default)]
pub struct OptionMenuCaps {
    pub create: bool,
    pub rename: bool,
    pub delete: bool,
}

/// A text-prompt sub-flow (create or rename).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionMenuVerb {
    Create,
    Rename,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionMenuMessage {
    Unhandled,
    Handled,
    Closed,
    /// Enter on a selected option — the embedder dispatches the configured
    /// toggle action with `value`. The popup has already flipped the option's
    /// `★` marker and stays open for further toggles.
    Toggle {
        value: String,
        label: String,
    },
    /// A text prompt was submitted — the embedder dispatches the create or
    /// rename action with `text` (and, for rename, the focused option's id in
    /// `value`). The popup stays open; the embedder refreshes its options.
    Submit {
        verb: OptionMenuVerb,
        value: Option<String>,
        text: String,
    },
    /// A delete was confirmed — the embedder dispatches the delete action with
    /// the focused option's `value`. The popup stays open.
    Delete {
        value: String,
        label: String,
    },
}

/// Inline text prompt state for a create / rename flow.
struct PromptState {
    verb: OptionMenuVerb,
    /// Focused option's stable id (rename) or `None` (create).
    value: Option<String>,
    title: String,
    buffer: String,
}

pub struct OptionMenuComponent {
    theme: Arc<Theme>,
    title: String,
    popup: Option<SearchablePopup>,
    popup_kb: Option<KeyBindingSection<PopupAction>>,
    key_icons: Option<KeyIconMap>,
    caps: OptionMenuCaps,
    /// Active create/rename text prompt, if any (overlays the menu).
    prompt: Option<PromptState>,
    /// Active delete confirmation `(value, label)`, if any.
    confirm_delete: Option<(String, String)>,
}

impl OptionMenuComponent {
    pub fn new(theme: Arc<Theme>) -> Self {
        Self {
            theme,
            title: "Options".to_string(),
            popup: None,
            popup_kb: None,
            key_icons: None,
            caps: OptionMenuCaps::default(),
            prompt: None,
            confirm_delete: None,
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
    /// pre-marked with `★`. `caps` enables the create/rename/delete bindings.
    pub fn open(
        &mut self,
        title: impl Into<String>,
        entries: &[OptionMenuEntry],
        caps: OptionMenuCaps,
        kb: &KeyBindingSection<TagMenuAction>,
    ) {
        self.title = title.into();
        self.caps = caps;
        self.prompt = None;
        self.confirm_delete = None;
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
        let mut hints = vec![(kb.label(&TagMenuAction::Toggle), "toggle".into())];
        if caps.create {
            hints.push((kb.label(&TagMenuAction::Create), "new".into()));
        }
        if caps.rename {
            hints.push((kb.label(&TagMenuAction::Edit), "rename".into()));
        }
        if caps.delete {
            hints.push((kb.label(&TagMenuAction::Delete), "delete".into()));
        }
        hints.push((kb.label(&TagMenuAction::Close), "close".into()));
        popup = popup.with_hints(hints);
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

        // Sub-flow: delete confirmation owns the keyboard until answered.
        if let Some((value, label)) = self.confirm_delete.clone() {
            let cancel = key == "n"
                || key == "N"
                || kb
                    .get(&TagMenuAction::Close)
                    .is_some_and(|b| b.matches(key));
            if key == "y" || key == "Y" {
                self.confirm_delete = None;
                return OptionMenuMessage::Delete { value, label };
            }
            if cancel {
                self.confirm_delete = None;
            }
            return OptionMenuMessage::Handled;
        }

        // Sub-flow: text prompt owns the keyboard until submitted or cancelled.
        if self.prompt.is_some() {
            return self.handle_prompt_key(key, kb);
        }

        // ── Menu mode ──────────────────────────────────────────────────────
        if kb
            .get(&TagMenuAction::Close)
            .is_some_and(|b| b.matches(key))
        {
            self.popup = None;
            return OptionMenuMessage::Closed;
        }
        if kb
            .get(&TagMenuAction::Toggle)
            .is_some_and(|b| b.matches(key))
        {
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
        if self.caps.create
            && kb
                .get(&TagMenuAction::Create)
                .is_some_and(|b| b.matches(key))
        {
            self.prompt = Some(PromptState {
                verb: OptionMenuVerb::Create,
                value: None,
                title: "New".to_string(),
                buffer: String::new(),
            });
            return OptionMenuMessage::Handled;
        }
        if self.caps.rename && kb.get(&TagMenuAction::Edit).is_some_and(|b| b.matches(key)) {
            if let Some(item) = self.popup.as_ref().unwrap().selected_item() {
                self.prompt = Some(PromptState {
                    verb: OptionMenuVerb::Rename,
                    value: Some(item.value.clone()),
                    title: "Rename".to_string(),
                    buffer: item.label.clone(),
                });
            }
            return OptionMenuMessage::Handled;
        }
        if self.caps.delete
            && kb
                .get(&TagMenuAction::Delete)
                .is_some_and(|b| b.matches(key))
        {
            if let Some(item) = self.popup.as_ref().unwrap().selected_item() {
                self.confirm_delete = Some((item.value.clone(), item.label.clone()));
            }
            return OptionMenuMessage::Handled;
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

    fn handle_prompt_key(
        &mut self,
        key: &str,
        kb: &KeyBindingSection<TagMenuAction>,
    ) -> OptionMenuMessage {
        // Esc cancels back to the menu (the popup stays open).
        if kb
            .get(&TagMenuAction::Close)
            .is_some_and(|b| b.matches(key))
        {
            self.prompt = None;
            return OptionMenuMessage::Handled;
        }
        if key == "enter" {
            let prompt = self.prompt.as_ref().unwrap();
            let text = prompt.buffer.trim().to_string();
            if text.is_empty() {
                // Ignore an empty submit; keep the prompt open.
                return OptionMenuMessage::Handled;
            }
            let verb = prompt.verb;
            let value = prompt.value.clone();
            self.prompt = None;
            return OptionMenuMessage::Submit { verb, value, text };
        }
        if key == "backspace" {
            self.prompt.as_mut().unwrap().buffer.pop();
            return OptionMenuMessage::Handled;
        }
        if key.chars().count() == 1 {
            let c = key.chars().next().unwrap();
            if !c.is_control() {
                self.prompt.as_mut().unwrap().buffer.push(c);
            }
        }
        OptionMenuMessage::Handled
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        if let Some(popup) = &mut self.popup {
            popup.view(frame, area);
        }
        // Sub-flows render a small box on top of the (frozen) menu.
        if let Some(prompt) = &self.prompt {
            let body = format!("{}▏", prompt.buffer);
            self.render_box(frame, area, &prompt.title, &body, self.theme.text_high());
        } else if let Some((_, label)) = &self.confirm_delete {
            let body = format!("Delete '{label}'?  (y/n)");
            self.render_box(frame, area, "Confirm", &body, self.theme.error());
        }
    }

    /// Render a small centred box with a single body line — used for the
    /// create/rename prompt and the delete confirmation.
    fn render_box(
        &self,
        frame: &mut Frame,
        area: Rect,
        title: &str,
        body: &str,
        body_fg: ratatui::style::Color,
    ) {
        let t: &Theme = &self.theme;
        let width = (body.chars().count() as u16 + 6)
            .max(28)
            .min(area.width.saturating_sub(4));
        let inner = render_popup_frame(frame, area, t, title, width, 3);
        if inner.height == 0 || inner.width == 0 {
            return;
        }
        let buf = frame.buffer_mut();
        let y = inner.y;
        let mut x = inner.left() + 1;
        for ch in body.chars() {
            if x >= inner.right() {
                break;
            }
            if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                cell.set_char(ch);
                cell.set_style(
                    Style::default()
                        .fg(body_fg)
                        .bg(t.bg())
                        .add_modifier(Modifier::BOLD),
                );
            }
            x += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1) as u16;
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
        m.insert(TagMenuAction::Create, KeyBinding::new("ctrl+n"));
        m.insert(TagMenuAction::Next, KeyBinding::new("ctrl+j"));
        m.insert(TagMenuAction::Prev, KeyBinding::new("ctrl+k"));
        m.insert(TagMenuAction::Delete, KeyBinding::new("ctrl+d"));
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

    fn all_caps() -> OptionMenuCaps {
        OptionMenuCaps {
            create: true,
            rename: true,
            delete: true,
        }
    }

    #[test]
    fn unhandled_when_closed() {
        let mut menu = OptionMenuComponent::new(theme());
        assert_eq!(
            menu.handle_key("enter", &make_kb()),
            OptionMenuMessage::Unhandled
        );
    }

    #[test]
    fn enter_emits_toggle_and_stays_open() {
        let mut menu = OptionMenuComponent::new(theme());
        let kb = make_kb();
        menu.open("Tags", &entries(), OptionMenuCaps::default(), &kb);
        let msg = menu.handle_key("enter", &kb);
        assert!(
            matches!(msg, OptionMenuMessage::Toggle { ref value, .. } if value == "global-tag:1")
        );
        // Stays open for multi-toggle.
        assert!(menu.is_open());
    }

    #[test]
    fn esc_closes() {
        let mut menu = OptionMenuComponent::new(theme());
        let kb = make_kb();
        menu.open("Tags", &entries(), OptionMenuCaps::default(), &kb);
        assert_eq!(menu.handle_key("esc", &kb), OptionMenuMessage::Closed);
        assert!(!menu.is_open());
    }

    #[test]
    fn toggle_flips_marker_live() {
        let mut menu = OptionMenuComponent::new(theme());
        let kb = make_kb();
        menu.open("Tags", &entries(), OptionMenuCaps::default(), &kb);
        // First option starts assigned (★). Toggling should clear it.
        let popup = menu.popup.as_ref().unwrap();
        assert!(popup.selected_item().unwrap().marked);
        menu.handle_key("enter", &kb);
        let popup = menu.popup.as_ref().unwrap();
        assert!(!popup.selected_item().unwrap().marked);
    }

    #[test]
    fn create_prompt_collects_text_and_submits() {
        let mut menu = OptionMenuComponent::new(theme());
        let kb = make_kb();
        menu.open("Tags", &entries(), all_caps(), &kb);
        assert_eq!(menu.handle_key("ctrl+n", &kb), OptionMenuMessage::Handled);
        // Type "ab", backspace, then "c" → "ac".
        menu.handle_key("a", &kb);
        menu.handle_key("b", &kb);
        menu.handle_key("backspace", &kb);
        menu.handle_key("c", &kb);
        let msg = menu.handle_key("enter", &kb);
        assert_eq!(
            msg,
            OptionMenuMessage::Submit {
                verb: OptionMenuVerb::Create,
                value: None,
                text: "ac".to_string(),
            }
        );
        assert!(menu.is_open());
    }

    #[test]
    fn create_disabled_without_capability() {
        let mut menu = OptionMenuComponent::new(theme());
        let kb = make_kb();
        menu.open("Tags", &entries(), OptionMenuCaps::default(), &kb);
        // No create cap → Ctrl+N falls through to the popup (no prompt opens).
        menu.handle_key("ctrl+n", &kb);
        let msg = menu.handle_key("enter", &kb);
        assert!(matches!(msg, OptionMenuMessage::Toggle { .. }));
    }

    #[test]
    fn rename_prefills_focused_label_and_carries_value() {
        let mut menu = OptionMenuComponent::new(theme());
        let kb = make_kb();
        menu.open("Tags", &entries(), all_caps(), &kb);
        assert_eq!(menu.handle_key("ctrl+e", &kb), OptionMenuMessage::Handled);
        // Pre-filled with "alpha"; append to make "alpha2".
        menu.handle_key("2", &kb);
        let msg = menu.handle_key("enter", &kb);
        assert_eq!(
            msg,
            OptionMenuMessage::Submit {
                verb: OptionMenuVerb::Rename,
                value: Some("global-tag:1".to_string()),
                text: "alpha2".to_string(),
            }
        );
    }

    #[test]
    fn empty_prompt_submit_is_ignored() {
        let mut menu = OptionMenuComponent::new(theme());
        let kb = make_kb();
        menu.open("Tags", &entries(), all_caps(), &kb);
        menu.handle_key("ctrl+n", &kb);
        // Nothing typed → Enter keeps the prompt open, no Submit.
        assert_eq!(menu.handle_key("enter", &kb), OptionMenuMessage::Handled);
        // Esc cancels the prompt; menu stays open.
        assert_eq!(menu.handle_key("esc", &kb), OptionMenuMessage::Handled);
        assert!(menu.is_open());
    }

    #[test]
    fn delete_confirm_y_emits_delete() {
        let mut menu = OptionMenuComponent::new(theme());
        let kb = make_kb();
        menu.open("Tags", &entries(), all_caps(), &kb);
        assert_eq!(menu.handle_key("ctrl+d", &kb), OptionMenuMessage::Handled);
        let msg = menu.handle_key("y", &kb);
        assert_eq!(
            msg,
            OptionMenuMessage::Delete {
                value: "global-tag:1".to_string(),
                label: "alpha".to_string(),
            }
        );
        assert!(menu.is_open());
    }

    #[test]
    fn delete_confirm_n_cancels() {
        let mut menu = OptionMenuComponent::new(theme());
        let kb = make_kb();
        menu.open("Tags", &entries(), all_caps(), &kb);
        menu.handle_key("ctrl+d", &kb);
        assert_eq!(menu.handle_key("n", &kb), OptionMenuMessage::Handled);
        // Confirmation cancelled — a subsequent Enter toggles as usual.
        let msg = menu.handle_key("enter", &kb);
        assert!(matches!(msg, OptionMenuMessage::Toggle { .. }));
    }
}
