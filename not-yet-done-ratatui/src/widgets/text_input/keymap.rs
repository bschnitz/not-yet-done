use crate::widgets::common::KeyBinding;
use crossterm::event::KeyCode;

#[derive(Debug, Clone)]
pub struct TextInputKeymap {
    pub move_left: KeyBinding,
    pub move_right: KeyBinding,
    pub delete_back: KeyBinding,
    pub delete_fwd: KeyBinding,
    pub clear: KeyBinding,
}

impl Default for TextInputKeymap {
    fn default() -> Self {
        Self {
            move_left: KeyBinding::new(KeyCode::Left),
            move_right: KeyBinding::new(KeyCode::Right),
            delete_back: KeyBinding::new(KeyCode::Backspace),
            delete_fwd: KeyBinding::new(KeyCode::Delete),
            clear: KeyBinding::ctrl(KeyCode::Char('u')),
        }
    }
}
