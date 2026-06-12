//! Tab-set switch popup — a small hotkey menu for choosing the active
//! tab constellation.
//!
//! Opened by [`GlobalAction::TabSetPopup`] (default `ctrl+x`). Unlike the
//! fuzzy [`SearchablePopup`], this is a fixed list driven by single-key
//! hotkeys: each constellation may declare a `shortcut`, and pressing it
//! switches immediately. Arrow keys + Enter/Space cover sets without a
//! shortcut and aid discovery; Esc cancels. The currently active set is
//! marked (`●`), and the shortcut letter is underlined in the label.
//!
//! Renders on the shared `popup_utils` chrome — including the standard
//! keybinding legend at the bottom — like every other menu popup.
//!
//! [`SearchablePopup`]: crate::components::searchable_popup::SearchablePopup
//! [`GlobalAction::TabSetPopup`]: crate::config::keybindings::GlobalAction::TabSetPopup

use std::sync::Arc;

use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::Frame;

use crate::ui::popup_utils::{hints_height, render_hints_bar, render_popup_frame};
use crate::ui::theme::Theme;

/// Keybinding legend shown at the bottom of the popup — part of the
/// standard popup chrome (`popup_utils`), like the other menu popups.
const HINTS: &[(&str, &str)] = &[("↑↓", "nav"), ("↵/Spc", "select"), ("Esc", "close")];

/// One selectable constellation in the popup.
#[derive(Debug, Clone)]
pub struct TabSetEntry {
    /// Constellation key (under `tabs.sets`) — the value handed back on
    /// selection / written to `active`.
    pub name: String,
    /// Display text shown in the popup (the set's `label`, or its key
    /// when no label is configured).
    pub label: String,
    /// Optional leading glyph.
    pub icon: Option<String>,
    /// Optional single-key hotkey.
    pub shortcut: Option<String>,
    /// Whether this is the constellation currently shown.
    pub active: bool,
}

/// Outcome of a key press while the popup is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabSetPopupMessage {
    /// Popup is not open — key not consumed here.
    Unhandled,
    /// Key consumed, popup stays open (navigation).
    Handled,
    /// Popup closed without a switch (Esc).
    Closed,
    /// Switch to the named constellation and close.
    Switch(String),
}

pub struct TabSetPopup {
    theme: Arc<Theme>,
    /// Frame title. Defaults to "Tab set"; [`Self::with_title`] re-labels
    /// the chrome so other fixed hotkey menus (e.g. the content group-by
    /// menu) can reuse this component instead of duplicating it.
    title: String,
    entries: Vec<TabSetEntry>,
    selected: usize,
    open: bool,
}

impl TabSetPopup {
    pub fn new(theme: Arc<Theme>) -> Self {
        Self {
            theme,
            title: "Tab set".to_string(),
            entries: Vec::new(),
            selected: 0,
            open: false,
        }
    }

    /// Builder: replace the frame title (for non-tab-set reuses).
    pub fn with_title(mut self, title: &str) -> Self {
        self.title = title.to_string();
        self
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open with the given constellations. Selection starts on the active
    /// set so Enter without further input is a no-op switch.
    pub fn open(&mut self, entries: Vec<TabSetEntry>) {
        self.selected = entries.iter().position(|e| e.active).unwrap_or(0);
        self.entries = entries;
        self.open = true;
    }

    /// Dispatch a key. `esc` closes; a key matching some entry's
    /// `shortcut` switches to it; arrows move the cursor; Enter or Space
    /// switches to the highlighted entry (native popup parity).
    pub fn handle_key(&mut self, key: &str) -> TabSetPopupMessage {
        if !self.open {
            return TabSetPopupMessage::Unhandled;
        }
        match key {
            "esc" => {
                self.open = false;
                TabSetPopupMessage::Closed
            }
            "down" | "ctrl+j" => {
                if self.selected + 1 < self.entries.len() {
                    self.selected += 1;
                }
                TabSetPopupMessage::Handled
            }
            "up" | "ctrl+k" => {
                self.selected = self.selected.saturating_sub(1);
                TabSetPopupMessage::Handled
            }
            "enter" | " " => match self.entries.get(self.selected) {
                Some(e) => {
                    let name = e.name.clone();
                    self.open = false;
                    TabSetPopupMessage::Switch(name)
                }
                None => TabSetPopupMessage::Handled,
            },
            other => {
                // A set hotkey takes precedence — switch and close.
                if let Some(e) = self
                    .entries
                    .iter()
                    .find(|e| e.shortcut.as_deref() == Some(other))
                {
                    let name = e.name.clone();
                    self.open = false;
                    return TabSetPopupMessage::Switch(name);
                }
                // Swallow every other key so it can't leak to the tab bar
                // (e.g. a digit switching tabs behind the popup).
                TabSetPopupMessage::Handled
            }
        }
    }

    /// The visible row text after the active marker: `icon label` plus a
    /// ` (key)` suffix when the shortcut letter does not occur in the
    /// label (it is then not expressible by underlining).
    fn row_text(entry: &TabSetEntry) -> String {
        let mut out = String::new();
        if let Some(icon) = &entry.icon {
            out.push_str(icon);
            out.push(' ');
        }
        out.push_str(&entry.label);
        if let Some(sc) = &entry.shortcut {
            if Self::shortcut_pos(entry).is_none() {
                out.push_str(&format!(" ({sc})"));
            }
        }
        out
    }

    /// Char index (within [`Self::row_text`]) of the shortcut letter to
    /// underline: the first case-insensitive occurrence of a single-char
    /// shortcut in `icon label`. `None` for multi-char or absent letters
    /// (those fall back to a ` (key)` suffix).
    fn shortcut_pos(entry: &TabSetEntry) -> Option<usize> {
        let sc = entry.shortcut.as_deref()?;
        let mut chars = sc.chars();
        let c = chars.next()?;
        if chars.next().is_some() {
            return None;
        }
        let icon_w = entry.icon.as_ref().map(|i| i.chars().count() + 1).unwrap_or(0);
        entry
            .label
            .chars()
            .position(|lc| lc.eq_ignore_ascii_case(&c))
            .map(|p| icon_w + p)
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        if !self.open {
            return;
        }
        let t = Arc::clone(&self.theme);

        // marker (2) + text, 1 cell indent each side inside the frame.
        let text_w = self
            .entries
            .iter()
            .map(|e| Self::row_text(e).chars().count() + 2)
            .max()
            .unwrap_or(0);
        let popup_w = ((text_w as u16) + 4).max(28).min(area.width.saturating_sub(4));
        let hh = hints_height(HINTS, popup_w.saturating_sub(2));
        let popup_h = self.entries.len() as u16 + 2 + hh;

        let inner = render_popup_frame(frame, area, &t, &self.title, popup_w, popup_h);
        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let buf = frame.buffer_mut();
        let items_height = inner.height.saturating_sub(hh) as usize;

        for (i, entry) in self.entries.iter().enumerate() {
            if i >= items_height {
                break;
            }
            let row_y = inner.y + i as u16;
            let is_cursor = i == self.selected;
            let bg = if is_cursor { t.surface_2() } else { t.bg() };

            for cx in inner.left()..inner.right() {
                if let Some(cell) = buf.cell_mut(Position::new(cx, row_y)) {
                    cell.set_char(' ');
                    cell.set_style(Style::default().bg(bg));
                }
            }

            let marker = if entry.active { "● " } else { "  " };
            let mut cx = inner.left() + 1;
            for ch in marker.chars() {
                if cx >= inner.right() {
                    break;
                }
                if let Some(cell) = buf.cell_mut(Position::new(cx, row_y)) {
                    cell.set_char(ch);
                    cell.set_style(Style::default().fg(t.accent()).bg(bg));
                }
                cx += 1;
            }

            let text = Self::row_text(entry);
            let hl_pos = Self::shortcut_pos(entry);
            let label_style = Style::default().fg(t.text_high()).bg(bg);
            let hl_style = Style::default()
                .fg(t.text_high())
                .bg(bg)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
            for (ci, ch) in text.chars().enumerate() {
                if cx >= inner.right() {
                    break;
                }
                let style = if hl_pos == Some(ci) { hl_style } else { label_style };
                if let Some(cell) = buf.cell_mut(Position::new(cx, row_y)) {
                    cell.set_char(ch);
                    cell.set_style(style);
                }
                cx += 1;
            }
        }

        render_hints_bar(frame, inner, &t, HINTS, hh);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Arc<Theme> {
        Arc::new(Theme::new(crate::config::ThemeConfig::default()))
    }

    fn entries() -> Vec<TabSetEntry> {
        vec![
            TabSetEntry {
                name: "work".into(),
                label: "Work".into(),
                icon: Some("W".into()),
                shortcut: Some("w".into()),
                active: true,
            },
            TabSetEntry {
                name: "personal".into(),
                label: "Personal".into(),
                icon: None,
                shortcut: Some("p".into()),
                active: false,
            },
            TabSetEntry {
                name: "no-shortcut".into(),
                label: "no-shortcut".into(),
                icon: None,
                shortcut: None,
                active: false,
            },
        ]
    }

    #[test]
    fn unhandled_when_closed() {
        let mut p = TabSetPopup::new(theme());
        assert_eq!(p.handle_key("w"), TabSetPopupMessage::Unhandled);
    }

    #[test]
    fn open_selects_active_entry() {
        let mut p = TabSetPopup::new(theme());
        p.open(entries());
        assert!(p.is_open());
        assert_eq!(p.selected, 0); // "work" is active
    }

    #[test]
    fn shortcut_switches_and_closes() {
        let mut p = TabSetPopup::new(theme());
        p.open(entries());
        assert_eq!(
            p.handle_key("p"),
            TabSetPopupMessage::Switch("personal".into())
        );
        assert!(!p.is_open());
    }

    #[test]
    fn enter_switches_to_highlighted() {
        let mut p = TabSetPopup::new(theme());
        p.open(entries());
        p.handle_key("down"); // -> personal
        p.handle_key("down"); // -> no-shortcut
        assert_eq!(
            p.handle_key("enter"),
            TabSetPopupMessage::Switch("no-shortcut".into())
        );
    }

    #[test]
    fn space_switches_to_highlighted() {
        let mut p = TabSetPopup::new(theme());
        p.open(entries());
        p.handle_key("down"); // -> personal
        assert_eq!(p.handle_key(" "), TabSetPopupMessage::Switch("personal".into()));
        assert!(!p.is_open());
    }

    #[test]
    fn shortcut_letter_underline_position() {
        let e = entries();
        // "work" with icon "W ": shortcut `w` hits the icon-offset label start.
        assert_eq!(TabSetPopup::shortcut_pos(&e[0]), Some(2));
        // "personal", no icon: `p` underlines index 0.
        assert_eq!(TabSetPopup::shortcut_pos(&e[1]), Some(0));
        // No shortcut → nothing to underline, no suffix either.
        assert_eq!(TabSetPopup::shortcut_pos(&e[2]), None);
        assert_eq!(TabSetPopup::row_text(&e[2]), "no-shortcut");
        // Shortcut letter absent from the label → ` (key)` suffix fallback.
        let odd = TabSetEntry {
            name: "x".into(),
            label: "Daily".into(),
            icon: None,
            shortcut: Some("q".into()),
            active: false,
        };
        assert_eq!(TabSetPopup::shortcut_pos(&odd), None);
        assert_eq!(TabSetPopup::row_text(&odd), "Daily (q)");
    }

    #[test]
    fn esc_closes_without_switch() {
        let mut p = TabSetPopup::new(theme());
        p.open(entries());
        assert_eq!(p.handle_key("esc"), TabSetPopupMessage::Closed);
        assert!(!p.is_open());
    }

    #[test]
    fn unknown_key_is_swallowed() {
        let mut p = TabSetPopup::new(theme());
        p.open(entries());
        assert_eq!(p.handle_key("9"), TabSetPopupMessage::Handled);
        assert!(p.is_open());
    }
}
