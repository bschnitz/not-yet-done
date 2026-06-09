//! Tab-set switch popup — a small hotkey menu for choosing the active
//! tab constellation.
//!
//! Opened by [`GlobalAction::TabSetPopup`] (default `ctrl+x`). Unlike the
//! fuzzy [`SearchablePopup`], this is a fixed list driven by single-key
//! hotkeys: each constellation may declare a `shortcut`, and pressing it
//! switches immediately. Arrow keys + Enter cover sets without a shortcut
//! and aid discovery; Esc cancels. The currently active set is marked.
//!
//! [`SearchablePopup`]: crate::components::searchable_popup::SearchablePopup
//! [`GlobalAction::TabSetPopup`]: crate::config::keybindings::GlobalAction::TabSetPopup

use std::sync::Arc;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Widget};
use ratatui::Frame;

use crate::ui::theme::Theme;

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
    entries: Vec<TabSetEntry>,
    selected: usize,
    open: bool,
}

impl TabSetPopup {
    pub fn new(theme: Arc<Theme>) -> Self {
        Self {
            theme,
            entries: Vec::new(),
            selected: 0,
            open: false,
        }
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
    /// `shortcut` switches to it; arrows move the cursor; Enter switches
    /// to the highlighted entry.
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
            "enter" => match self.entries.get(self.selected) {
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

    /// Build the visible row text for an entry: `‣ icon name (key)`.
    fn row_label(entry: &TabSetEntry) -> String {
        let mut out = String::new();
        out.push_str(if entry.active { " ‣ " } else { "   " });
        if let Some(icon) = &entry.icon {
            out.push_str(icon);
            out.push(' ');
        }
        out.push_str(&entry.label);
        if let Some(sc) = &entry.shortcut {
            out.push_str(&format!("  ({sc})"));
        }
        out
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        if !self.open {
            return;
        }
        let t = &self.theme;

        let label_w = self
            .entries
            .iter()
            .map(|e| Self::row_label(e).chars().count())
            .max()
            .unwrap_or(0);
        let popup_w = ((label_w as u16) + 4)
            .max(24)
            .min(area.width.saturating_sub(4));
        let popup_h = (self.entries.len() as u16 + 2)
            .max(3)
            .min(area.height.saturating_sub(2));
        let x = area.width.saturating_sub(popup_w) / 2;
        let y = area.height.saturating_sub(popup_h) / 2;
        let popup_area = Rect::new(x, y, popup_w, popup_h);

        frame.render_widget(Clear, popup_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.primary()))
            .title(" Tab set ")
            .title_style(Style::default().fg(t.accent()).add_modifier(Modifier::BOLD))
            .style(Style::default().bg(t.bg()));
        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let buf = frame.buffer_mut();
        for (i, entry) in self.entries.iter().enumerate() {
            if i as u16 >= inner.height {
                break;
            }
            let is_selected = i == self.selected;
            let style = if is_selected {
                Style::default().fg(t.bg()).bg(t.primary())
            } else if entry.active {
                Style::default().fg(t.accent()).bg(t.bg())
            } else {
                Style::default().fg(t.text_high()).bg(t.bg())
            };
            let row_area = Rect {
                x: inner.x,
                y: inner.y + i as u16,
                width: inner.width,
                height: 1,
            };
            let padded = format!(
                "{:width$}",
                Self::row_label(entry),
                width = inner.width as usize
            );
            Line::from(Span::styled(padded, style)).render(row_area, buf);
        }
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
