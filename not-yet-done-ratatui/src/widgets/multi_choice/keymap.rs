use tuirealm::event::Key;

use crate::widgets::common::Keys;

/// Keyboard bindings for [`MultiChoice`].
///
/// Each field is a [`Keys`] which can hold one or more key combinations.
#[derive(Debug, Clone)]
pub struct MultiChoiceKeymap {
    pub move_up: Keys,
    pub move_down: Keys,
    pub toggle: Keys,
    /// Closes the dropdown without moving focus away.
    pub close: Keys,
    pub select_all: Keys,
    pub select_none: Keys,
    pub filter_cursor_left: Keys,
    pub filter_cursor_right: Keys,
    pub filter_delete: Keys,
    pub filter_clear: Keys,
    /// Move the item at the cursor one position up (requires ordering mode).
    pub order_up: Keys,
    /// Move the item at the cursor one position down (requires ordering mode).
    pub order_down: Keys,
}

impl Default for MultiChoiceKeymap {
    fn default() -> Self {
        Self {
            move_up:             Keys::plain(Key::Up),
            move_down:           Keys::plain(Key::Down),
            toggle:              Keys::plain(Key::Char(' ')),
            close:               Keys::plain(Key::Esc),
            select_all:          Keys::ctrl(Key::Char('a')),
            select_none:         Keys::ctrl(Key::Char('n')),
            filter_cursor_left:  Keys::plain(Key::Left),
            filter_cursor_right: Keys::plain(Key::Right),
            filter_delete:       Keys::plain(Key::Backspace),
            filter_clear:        Keys::ctrl(Key::Char('u')),
            order_up:            Keys::one(Key::Up, tuirealm::event::KeyModifiers::CONTROL),
            order_down:          Keys::one(Key::Down, tuirealm::event::KeyModifiers::CONTROL),
        }
    }
}
