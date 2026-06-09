use tuirealm::event::Key;

use crate::widgets::common::Keys;

/// Keyboard bindings for [`SelectList`].
///
/// Each field is a [`Keys`] which can hold one or more key combinations.
#[derive(Debug, Clone)]
pub struct SelectListKeymap {
    pub move_up: Keys,
    pub move_down: Keys,
    pub toggle: Keys,
    pub confirm: Keys,
    pub cancel: Keys,
    pub select_all: Keys,
    pub select_none: Keys,
    pub filter_cursor_left: Keys,
    pub filter_cursor_right: Keys,
    pub filter_delete: Keys,
    pub filter_clear: Keys,
}

impl Default for SelectListKeymap {
    fn default() -> Self {
        Self {
            move_up:             Keys::ctrl(Key::Char('k')),
            move_down:           Keys::ctrl(Key::Char('j')),
            toggle:              Keys::plain(Key::Char(' ')),
            confirm:             Keys::plain(Key::Enter),
            cancel:              Keys::plain(Key::Esc),
            select_all:          Keys::ctrl(Key::Char('a')),
            select_none:         Keys::ctrl(Key::Char('n')),
            filter_cursor_left:  Keys::plain(Key::Left),
            filter_cursor_right: Keys::plain(Key::Right),
            filter_delete:       Keys::plain(Key::Backspace),
            filter_clear:        Keys::ctrl(Key::Char('u')),
        }
    }
}
