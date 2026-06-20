//! StatusBar component: bottom bar showing available keybindings.
//!
//! Read-only display — does not produce messages.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::Frame;

use tuirealm::command::{Cmd, CmdResult};
use tuirealm::props::{Attribute, AttrValue, QueryResult};
use tuirealm::component::Component;
use tuirealm::state::{State, StateValue};

use crate::config::{GlobalAction, KeyBindingConfig};
use std::sync::Arc;
use crate::ui::theme::Theme;

pub struct StatusBarComponent {
    theme: Arc<Theme>,
    hints: Vec<(String, String)>, // [(key_label, description), ...]
    /// Active link-mark label rendered as a leading "📎 marked: <ref>"
    /// pill on the status bar. `None` hides the marker entirely.
    link_marker: Option<String>,
}

impl StatusBarComponent {
    pub fn new(theme: Arc<Theme>, keybindings: &KeyBindingConfig) -> Self {
        let mut comp = Self {
            theme,
            hints: Vec::new(),
            link_marker: None,
        };
        comp.rebuild_hints(keybindings);
        comp
    }

    /// Set or clear the leading link-mark indicator.
    pub fn set_link_marker(&mut self, marker: Option<String>) {
        self.link_marker = marker;
    }

    /// Set custom hints directly (for Content tabs with dynamic keybindings).
    pub fn set_custom_hints(&mut self, hints: Vec<(String, String)>) {
        self.hints = hints;
    }

    /// Calculate how many rows the status bar needs at the given width.
    pub fn required_height(&self, available_width: u16) -> u16 {
        if self.hints.is_empty() { return 1; }
        let w = available_width as usize;
        let mut lines = 1u16;
        let mut line_used = 1usize; // leading space

        for (key_label, desc) in &self.hints {
            let hint_w = key_label.chars().count() + 1 + desc.chars().count() + 2;
            if line_used + hint_w > w && line_used > 1 {
                lines += 1;
                line_used = 1;
            }
            line_used += hint_w;
        }
        lines
    }

    fn rebuild_hints(&mut self, kb: &KeyBindingConfig) {
        let gkb = &kb.global;

        let mut hints = vec![
            (gkb.label(&GlobalAction::Quit), "quit".to_string()),
        ];

        hints.push((
            format!("{}/{}", gkb.label(&GlobalAction::TabNext), gkb.label(&GlobalAction::TabPrev)),
            "cycle tabs".to_string(),
        ));
        hints.push((gkb.label(&GlobalAction::Quit), "quit".to_string()));

        self.hints = hints;
    }
}

impl Component for StatusBarComponent {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        if area.height == 0 { return; }
        let t = &self.theme;
        let bg = t.surface();
        let buf = frame.buffer_mut();
        let right = area.right();
        let left = area.left();

        // Fill background for all rows.
        for row in 0..area.height {
            let y = area.top() + row;
            for x in left..right {
                if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(x, y)) {
                    cell.set_char(' ');
                    cell.set_style(Style::default().bg(bg));
                }
            }
        }

        let mut x = left + 1;
        let mut y = area.top();

        // Leading link-mark indicator pill. Drawn before the regular
        // hints so it's anchored on the first row, in accent fg on the
        // status-bar bg — purely informational, no key binding hint.
        if let Some(marker) = self.link_marker.as_deref() {
            let label = format!("⚓ marked: {marker}");
            let marker_style = Style::default().fg(t.accent()).bg(bg);
            for ch in label.chars() {
                if x >= right || y >= area.bottom() { break; }
                if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(x, y)) {
                    cell.set_char(ch);
                    cell.set_style(marker_style);
                }
                x += 1;
            }
            let sep_style = Style::default().fg(t.text_med()).bg(bg);
            for ch in "  ".chars() {
                if x >= right || y >= area.bottom() { break; }
                if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(x, y)) {
                    cell.set_char(ch);
                    cell.set_style(sep_style);
                }
                x += 1;
            }
        }

        for (key_label, desc) in &self.hints {
            let hint_w = (key_label.chars().count() + 1 + desc.chars().count() + 2) as u16;
            if x + hint_w > right && x > left + 1 && y + 1 < area.bottom() {
                y += 1;
                x = left + 1;
            }

            let key_style = Style::default().fg(t.accent()).bg(bg);
            for ch in key_label.chars() {
                if x >= right || y >= area.bottom() { break; }
                if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(x, y)) {
                    cell.set_char(ch);
                    cell.set_style(key_style);
                }
                x += 1;
            }

            let desc_style = Style::default().fg(t.text_med()).bg(bg);
            let desc_text = format!(" {}  ", desc);
            for ch in desc_text.chars() {
                if x >= right || y >= area.bottom() { break; }
                if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(x, y)) {
                    cell.set_char(ch);
                    cell.set_style(desc_style);
                }
                x += 1;
            }
        }
    }

    fn query(&self, _attr: Attribute) -> Option<QueryResult<'_>> {
        None
    }

    fn attr(&mut self, _attr: Attribute, _value: AttrValue) {}

    fn state(&self) -> State {
        State::Single(StateValue::String("status".to_string()))
    }

    fn perform(&mut self, _cmd: Cmd) -> CmdResult {
        CmdResult::NoChange
    }
}
