use tuirealm::event::{Key, KeyEvent, KeyModifiers};

/// Keyboard bindings for [`MultiChoice`].
///
/// Each field is a tuirealm [`KeyEvent`] (key code + modifiers).
/// Use struct update syntax to override individual bindings:
///
/// ```rust
/// use tuirealm::event::{Key, KeyEvent, KeyModifiers};
///
/// let keymap = MultiChoiceKeymap {
///     move_up: KeyEvent { code: Key::Up, modifiers: KeyModifiers::NONE },
///     ..MultiChoiceKeymap::default()
/// };
/// ```
#[derive(Debug, Clone)]
pub struct MultiChoiceKeymap {
    pub move_up: KeyEvent,
    pub move_down: KeyEvent,
    pub toggle: KeyEvent,
}

impl Default for MultiChoiceKeymap {
    fn default() -> Self {
        Self {
            move_up: KeyEvent { code: Key::Up, modifiers: KeyModifiers::NONE },
            move_down: KeyEvent { code: Key::Down, modifiers: KeyModifiers::NONE },
            toggle: KeyEvent { code: Key::Char(' '), modifiers: KeyModifiers::NONE },
        }
    }
}
