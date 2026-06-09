use tuirealm::event::{Key, KeyModifiers};

use crate::widgets::common::Keys;

/// Keyboard bindings for [`FilePicker`](super::FilePicker).
///
/// Every field is a [`Keys`], so each action can be triggered by any
/// number of alternative key combinations — combine them with
/// [`Keys::or`], [`Keys::or_plain`], [`Keys::or_ctrl`] when building a
/// custom keymap.
///
/// Defaults:
/// - `focus_next`: `Ctrl+L`
/// - `focus_prev`: `Ctrl+H`
/// - `submit`:     `Ctrl+O`
/// - `cancel`:     `Esc`
/// - `paste`:      `Ctrl+V`
/// - `toggle`:     `Enter` or `Ctrl+Enter` — select/deselect the entry
///   under the cursor in the Files or Selected pane
/// - `remove_selected`: `Ctrl+D` — in Files (when cursor is on a
///   currently-selected file) or in Selected, remove that path from the
///   selection
/// - `filter_clear`: `,` — wipe the embedded filter query of the
///   Files / Selected SelectList while it has focus
/// - `browse_down`/`browse_up`: `Ctrl+J`/`Ctrl+K` — move the dir-picker
///   cursor while the Directory pane is focused
/// - `browse_navigate`: `Enter` or `Ctrl+Enter` — descend into the
///   highlighted entry while the Directory pane is focused (also jumps
///   to parent when the input ends with `..`)
/// - `tab_complete`: `Tab` — extend the Directory input to the longest
///   common prefix of the currently matching subdirectories
#[derive(Debug, Clone)]
pub struct FilePickerKeymap {
    pub focus_next: Keys,
    pub focus_prev: Keys,
    pub submit: Keys,
    pub cancel: Keys,
    pub paste: Keys,
    pub toggle: Keys,
    pub remove_selected: Keys,
    pub filter_clear: Keys,
    pub browse_down: Keys,
    pub browse_up: Keys,
    pub browse_navigate: Keys,
    pub tab_complete: Keys,
}

impl Default for FilePickerKeymap {
    fn default() -> Self {
        Self {
            focus_next: Keys::ctrl(Key::Char('l')),
            focus_prev: Keys::ctrl(Key::Char('h')),
            submit: Keys::ctrl(Key::Char('o')),
            cancel: Keys::plain(Key::Esc),
            paste: Keys::ctrl(Key::Char('v')),
            toggle: Keys::plain(Key::Enter)
                .or(Key::Enter, KeyModifiers::CONTROL),
            remove_selected: Keys::ctrl(Key::Char('d')),
            filter_clear: Keys::plain(Key::Char(',')),
            browse_down: Keys::ctrl(Key::Char('j')),
            browse_up: Keys::ctrl(Key::Char('k')),
            browse_navigate: Keys::plain(Key::Enter)
                .or(Key::Enter, KeyModifiers::CONTROL),
            tab_complete: Keys::plain(Key::Tab),
        }
    }
}
