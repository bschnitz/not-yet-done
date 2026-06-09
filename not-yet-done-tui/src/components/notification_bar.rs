//! NotificationBar component: persistent message area below the table.
//!
//! Notifications accumulate until dismissed. They are word-wrapped and
//! limited to a configurable maximum number of lines.

use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::Frame;

use tuirealm::command::{Cmd, CmdResult};
use tuirealm::props::{Attribute, AttrValue, QueryResult};
use tuirealm::component::Component;
use tuirealm::state::{State, StateValue};

use std::sync::Arc;
use crate::ui::theme::Theme;

pub struct NotificationBarComponent {
    theme: Arc<Theme>,
    messages: Vec<String>,
    max_lines: u16,
}

impl NotificationBarComponent {
    pub fn new(theme: Arc<Theme>) -> Self {
        Self { theme, messages: Vec::new(), max_lines: 5 }
    }

    pub fn set_max_lines(&mut self, max: u16) {
        self.max_lines = max;
    }

    pub fn push(&mut self, msg: String) {
        self.messages.push(msg);
    }

    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// Remove the first message equal to `msg`, leaving the rest intact.
    /// Used to retract a transient status line (e.g. "Opening editor…")
    /// once the work it announced finishes, without disturbing unrelated
    /// notifications the user hasn't dismissed yet.
    pub fn remove(&mut self, msg: &str) {
        if let Some(pos) = self.messages.iter().position(|m| m == msg) {
            self.messages.remove(pos);
        }
    }

    #[allow(dead_code)]
    pub fn is_visible(&self) -> bool {
        !self.messages.is_empty()
    }

    #[allow(dead_code)]
    pub fn set_message(&mut self, msg: Option<String>) {
        match msg {
            Some(m) => {
                self.messages.clear();
                self.messages.push(m);
            }
            None => self.messages.clear(),
        }
    }

    /// Calculate the number of rows needed to display all messages.
    pub fn required_height(&self, available_width: u16) -> u16 {
        if self.messages.is_empty() { return 0; }
        let w = available_width.saturating_sub(2) as usize; // 1 char padding each side
        if w == 0 { return 0; }

        let mut total_lines: u16 = 0;
        for msg in &self.messages {
            let lines = wrap_lines(msg, w);
            total_lines += lines.len() as u16;
        }
        total_lines.min(self.max_lines).max(1)
    }
}

impl Component for NotificationBarComponent {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        if self.messages.is_empty() || area.height == 0 { return; }
        let t = &self.theme;
        let bg = t.surface();
        let fg = t.text_high();
        let style = Style::default().fg(fg).bg(bg);
        let dim_style = Style::default().fg(t.text_dim()).bg(bg);
        let buf = frame.buffer_mut();

        // Fill background.
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                    cell.set_char(' ');
                    cell.set_style(Style::default().bg(bg));
                }
            }
        }

        let w = area.width.saturating_sub(2) as usize;
        let mut y = area.top();

        for msg in &self.messages {
            if y >= area.bottom() { break; }
            let lines = wrap_lines(msg, w);
            for (li, line) in lines.iter().enumerate() {
                if y >= area.bottom() { break; }
                let mut x = area.left() + 1;
                // Show bullet on first line of each message.
                if li == 0 {
                    let bullet_style = Style::default().fg(t.accent()).bg(bg);
                    if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                        cell.set_char('●');
                        cell.set_style(bullet_style);
                    }
                    x += 1;
                    if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                        cell.set_char(' ');
                        cell.set_style(style);
                    }
                    x += 1;
                } else {
                    x += 2; // indent continuation lines
                }
                for ch in line.chars() {
                    if x >= area.right().saturating_sub(1) { break; }
                    if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                        cell.set_char(ch);
                        cell.set_style(style);
                    }
                    x += 1;
                }
                y += 1;
            }
        }

        // Dismiss hint on the last row, right-aligned.
        let hint = "[z] dismiss";
        let hint_x = area.right().saturating_sub(hint.len() as u16 + 1);
        let hint_y = area.bottom().saturating_sub(1);
        let mut hx = hint_x;
        for ch in hint.chars() {
            if hx >= area.right() { break; }
            if let Some(cell) = buf.cell_mut(Position::new(hx, hint_y)) {
                cell.set_char(ch);
                cell.set_style(dim_style);
            }
            hx += 1;
        }
    }

    fn query(&self, _attr: Attribute) -> Option<QueryResult<'_>> { None }
    fn attr(&mut self, _attr: Attribute, _value: AttrValue) {}
    fn state(&self) -> State {
        if self.messages.is_empty() {
            State::None
        } else {
            State::Single(StateValue::String(self.messages.join("\n")))
        }
    }
    fn perform(&mut self, _cmd: Cmd) -> CmdResult { CmdResult::NoChange }
}

fn wrap_lines(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 { return vec![text.to_string()]; }
    let mut lines = Vec::new();
    for raw_line in text.lines() {
        if raw_line.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut current = String::new();
        for word in raw_line.split_whitespace() {
            if current.is_empty() {
                current = word.to_string();
            } else if current.len() + 1 + word.len() > max_width {
                lines.push(current);
                current = word.to_string();
            } else {
                current.push(' ');
                current.push_str(word);
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}
