use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use crate::widgets::common::keymap::KeyBinding;
use super::keymap::TextInputKeymap;

#[derive(Debug, Clone)]
pub enum TextInputEvent {
    Changed(String),
    Ignored,
}

#[derive(Debug, Default, Clone)]
pub struct TextInputState {
    pub value: String,
    pub error: Option<String>,
    pub(crate) cursor: usize,
}

impl TextInputState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_error(&mut self, msg: impl Into<String>) {
        self.error = Some(msg.into());
    }

    pub fn clear_error(&mut self) {
        self.error = None;
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn handle_event(
        &mut self,
        event: &Event,
        keymap: &TextInputKeymap,
    ) -> TextInputEvent {
        let Event::Key(KeyEvent { code, modifiers, .. }) = event else {
            return TextInputEvent::Ignored;
        };

        let pressed = KeyBinding {
            code: *code,
            modifiers: *modifiers,
        };

        if pressed == keymap.move_left {
            self.move_cursor_left();
            return TextInputEvent::Ignored;
        }
        if pressed == keymap.move_right {
            self.move_cursor_right();
            return TextInputEvent::Ignored;
        }
        if pressed == keymap.delete_back {
            self.pop_char();
            return TextInputEvent::Changed(self.value.clone());
        }
        if pressed == keymap.delete_fwd {
            self.delete_forward();
            return TextInputEvent::Changed(self.value.clone());
        }
        if pressed == keymap.clear {
            self.clear();
            return TextInputEvent::Changed(self.value.clone());
        }

        if let KeyCode::Char(c) = code {
            if modifiers.is_empty() || *modifiers == KeyModifiers::SHIFT {
                self.push_char(*c);
                return TextInputEvent::Changed(self.value.clone());
            }
        }

        TextInputEvent::Ignored
    }

    // --- interne Cursor/Editing-Methoden ---

    pub(crate) fn push_char(&mut self, c: char) {
        self.value.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub(crate) fn pop_char(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let mut pos = self.cursor - 1;
        while !self.value.is_char_boundary(pos) {
            pos -= 1;
        }
        self.value.remove(pos);
        self.cursor = pos;
    }

    pub(crate) fn delete_forward(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        self.value.remove(self.cursor);
    }

    pub(crate) fn move_cursor_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let mut pos = self.cursor - 1;
        while !self.value.is_char_boundary(pos) {
            pos -= 1;
        }
        self.cursor = pos;
    }

    pub(crate) fn move_cursor_right(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        let mut pos = self.cursor + 1;
        while pos <= self.value.len() && !self.value.is_char_boundary(pos) {
            pos += 1;
        }
        self.cursor = pos;
    }

    pub(crate) fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }
}
