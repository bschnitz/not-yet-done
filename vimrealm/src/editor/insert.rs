//! Insert mode: everything printable goes into the buffer, everything else is
//! a key.

use tuirealm::event::{Key, KeyEvent, KeyModifiers};

use super::VimEditor;
use crate::keymap::InsertCommand;
use crate::mode::Mode;
use crate::motion;
use crate::state::VimEvent;

impl VimEditor {
    pub(super) fn on_insert(&mut self, key: KeyEvent) -> Option<VimEvent> {
        if let Some(cmd) = self.keymap.insert(key) {
            return match cmd {
                InsertCommand::Escape => {
                    self.leave_insert();
                    None
                }
                InsertCommand::Newline => {
                    let pos = self.buffer.split_line(self.buffer.cursor());
                    self.buffer.set_cursor_insert(pos);
                    Some(VimEvent::Changed)
                }
                InsertCommand::Backspace => self.backspace(),
                InsertCommand::Delete => self.delete_forward(),
                InsertCommand::Motion(m) => {
                    // Insert-mode bounds: the arrow may step onto the column
                    // past the last character, where the next one would land.
                    let target =
                        motion::resolve_bounded(&self.buffer, m, 1, false, motion::Bound::PastEnd);
                    self.buffer.set_cursor_insert(target);
                    None
                }
            };
        }
        // Anything printable is inserted literally. Control combinations are
        // left for the host, so `Ctrl+…` shortcuts keep working while typing.
        let c = match key.code {
            Key::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                c
            }
            Key::Tab => '\t',
            _ => return None,
        };
        let pos = self.buffer.insert_char(self.buffer.cursor(), c);
        self.buffer.set_cursor_insert(pos);
        Some(VimEvent::Changed)
    }

    /// Back to normal mode, cursor one step left the way vim does it.
    pub(super) fn leave_insert(&mut self) {
        self.mode = Mode::Normal;
        let cursor = self.buffer.cursor();
        let back = self
            .buffer
            .prev_pos(cursor)
            .filter(|p| p.line == cursor.line)
            .unwrap_or(cursor);
        self.buffer.set_cursor(back);
    }

    fn backspace(&mut self) -> Option<VimEvent> {
        let cursor = self.buffer.cursor();
        let Some(from) = self.buffer.prev_pos(cursor) else {
            return None;
        };
        self.buffer.delete_range(from, cursor);
        self.buffer.set_cursor_insert(from);
        Some(VimEvent::Changed)
    }

    fn delete_forward(&mut self) -> Option<VimEvent> {
        let cursor = self.buffer.cursor();
        let Some(to) = self.buffer.next_pos(cursor) else {
            return None;
        };
        self.buffer.delete_range(cursor, to);
        self.buffer.set_cursor_insert(cursor);
        Some(VimEvent::Changed)
    }
}
