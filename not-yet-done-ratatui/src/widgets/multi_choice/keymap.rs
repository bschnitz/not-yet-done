use crate::widgets::common::KeyBinding;
use crossterm::event::KeyCode;

#[derive(Debug, Clone)]
pub struct MultiChoiceKeymap {
    pub move_down: KeyBinding,
    pub move_up: KeyBinding,
    pub toggle: KeyBinding,
}

impl Default for MultiChoiceKeymap {
    fn default() -> Self {
        Self {
            move_down: KeyBinding::ctrl(KeyCode::Char('j')),
            move_up: KeyBinding::ctrl(KeyCode::Char('k')),
            toggle: KeyBinding::new(KeyCode::Char(' ')),
        }
    }
}
