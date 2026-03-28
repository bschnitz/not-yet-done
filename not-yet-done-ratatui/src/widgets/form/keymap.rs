use crate::widgets::common::KeyBinding;
use crossterm::event::{KeyCode, KeyModifiers};

#[derive(Debug, Clone)]
pub struct FormKeymap {
    /// Move focus to the next field.
    pub focus_next: KeyBinding,
    /// Move focus to the previous field.
    pub focus_prev: KeyBinding,
    /// Confirm / submit the form (or toggle a MultiChoice open/closed).
    pub confirm: KeyBinding,
}

impl Default for FormKeymap {
    fn default() -> Self {
        Self {
            focus_next: KeyBinding::new(KeyCode::Tab),
            focus_prev: KeyBinding {
                code: KeyCode::BackTab,
                modifiers: KeyModifiers::SHIFT,
            },
            confirm: KeyBinding::new(KeyCode::Enter),
        }
    }
}
