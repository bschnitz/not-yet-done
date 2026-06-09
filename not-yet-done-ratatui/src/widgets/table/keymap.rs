use tuirealm::event::Key;

use crate::widgets::common::Keys;

/// Keyboard bindings for the [`Table`] widget.
#[derive(Debug, Clone)]
pub struct TableKeymap {
    pub move_up: Keys,
    pub move_down: Keys,
    pub half_page_up: Keys,
    pub half_page_down: Keys,
    pub page_up: Keys,
    pub page_down: Keys,
    pub move_first: Keys,
    pub move_last: Keys,
    pub confirm: Keys,
    pub cancel: Keys,
}

impl Default for TableKeymap {
    fn default() -> Self {
        Self {
            move_up:  Keys::plain(Key::Up),
            move_down: Keys::plain(Key::Down),
            half_page_up: Keys::ctrl(Key::Char('u')),
            half_page_down: Keys::ctrl(Key::Char('d')),
            page_up: Keys::ctrl(Key::Char('b')),
            page_down: Keys::ctrl(Key::Char('f')),
            move_first: Keys::plain(Key::Home),
            move_last: Keys::plain(Key::End),
            confirm:  Keys::plain(Key::Enter),
            cancel:   Keys::plain(Key::Esc),
        }
    }
}
