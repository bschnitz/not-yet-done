use tuirealm::event::Key;

use crate::widgets::common::Keys;

/// Keybindings for a selectable [`super::LeaderList`].
#[derive(Debug, Clone)]
pub struct LeaderListKeymap {
    pub move_up: Keys,
    pub move_down: Keys,
    /// Scroll one page towards the top. Only acts when the list is scrollable
    /// (more entries than visible rows).
    pub page_up: Keys,
    /// Scroll one page towards the bottom. Only acts when the list is
    /// scrollable (more entries than visible rows).
    pub page_down: Keys,
    pub confirm: Keys,
    pub cancel: Keys,
}

impl Default for LeaderListKeymap {
    fn default() -> Self {
        Self {
            move_up: Keys::plain(Key::Up).or_ctrl(Key::Char('k')),
            move_down: Keys::plain(Key::Down).or_ctrl(Key::Char('j')),
            page_up: Keys::plain(Key::PageUp).or_ctrl(Key::Char('b')),
            page_down: Keys::plain(Key::PageDown).or_ctrl(Key::Char('f')),
            confirm: Keys::plain(Key::Enter),
            cancel: Keys::plain(Key::Esc),
        }
    }
}
