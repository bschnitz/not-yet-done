use tuirealm::event::{Key, KeyEvent, KeyModifiers};

/// Keyboard bindings for [`TextInput`].
///
/// Each field is a tuirealm [`KeyEvent`] (key code + modifiers).
/// Use struct update syntax to override individual bindings:
///
/// ```rust
/// use tuirealm::event::{Key, KeyEvent, KeyModifiers};
///
/// let keymap = TextInputKeymap {
///     move_left:  KeyEvent { code: Key::Char('b'), modifiers: KeyModifiers::CONTROL },
///     move_right: KeyEvent { code: Key::Char('f'), modifiers: KeyModifiers::CONTROL },
///     ..TextInputKeymap::default()
/// };
/// ```
#[derive(Debug, Clone)]
pub struct TextInputKeymap {
    pub move_left:   KeyEvent,
    pub move_right:  KeyEvent,
    pub delete_back: KeyEvent,
    pub delete_fwd:  KeyEvent,
    pub clear:       KeyEvent,
    pub submit:      KeyEvent,
}

impl Default for TextInputKeymap {
    fn default() -> Self {
        Self {
            move_left:   KeyEvent { code: Key::Left,      modifiers: KeyModifiers::NONE },
            move_right:  KeyEvent { code: Key::Right,     modifiers: KeyModifiers::NONE },
            delete_back: KeyEvent { code: Key::Backspace, modifiers: KeyModifiers::NONE },
            delete_fwd:  KeyEvent { code: Key::Delete,    modifiers: KeyModifiers::NONE },
            clear:       KeyEvent { code: Key::Char('u'), modifiers: KeyModifiers::CONTROL },
            submit:      KeyEvent { code: Key::Enter,     modifiers: KeyModifiers::NONE },
        }
    }
}
