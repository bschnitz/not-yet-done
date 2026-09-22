use tuirealm::event::{Key, KeyEvent, KeyModifiers};

/// Keyboard bindings for [`TextArea`](super::TextArea).
///
/// The single-line input's `submit` has no counterpart here: `enter` inserts a
/// newline, and confirming whatever the text area sits in is the surrounding
/// form's business (`ctrl+enter`).
///
/// ```rust
/// use tuirealm::event::{Key, KeyEvent, KeyModifiers};
/// use not_yet_done_ratatui::widgets::text_area::TextAreaKeymap;
///
/// let keymap = TextAreaKeymap {
///     kill_to_start: KeyEvent { code: Key::Char('w'), modifiers: KeyModifiers::CONTROL },
///     ..TextAreaKeymap::default()
/// };
/// ```
#[derive(Debug, Clone)]
pub struct TextAreaKeymap {
    pub move_left: KeyEvent,
    pub move_right: KeyEvent,
    pub move_up: KeyEvent,
    pub move_down: KeyEvent,
    pub move_home: KeyEvent,
    pub move_end: KeyEvent,
    pub delete_back: KeyEvent,
    pub delete_fwd: KeyEvent,
    /// Deletes from the cursor back to the start of the line.
    pub kill_to_start: KeyEvent,
    pub newline: KeyEvent,
}

impl Default for TextAreaKeymap {
    fn default() -> Self {
        let plain = |code| KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
        };
        Self {
            move_left: plain(Key::Left),
            move_right: plain(Key::Right),
            move_up: plain(Key::Up),
            move_down: plain(Key::Down),
            move_home: plain(Key::Home),
            move_end: plain(Key::End),
            delete_back: plain(Key::Backspace),
            delete_fwd: plain(Key::Delete),
            kill_to_start: KeyEvent {
                code: Key::Char('u'),
                modifiers: KeyModifiers::CONTROL,
            },
            newline: plain(Key::Enter),
        }
    }
}
