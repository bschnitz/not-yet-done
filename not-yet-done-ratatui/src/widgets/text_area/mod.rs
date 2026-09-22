mod component;
pub mod keymap;
mod render;
pub mod state;

pub use component::{CMD_DELETE_FWD, CMD_END, CMD_HOME, CMD_KILL_TO_START, CMD_NEWLINE};
pub use keymap::TextAreaKeymap;
pub use state::TextAreaEvent;

use crate::widgets::text_input::TextInputStyle;

/// How many text rows a text area shows when nobody says otherwise.
pub const DEFAULT_ROWS: u16 = 4;

/// A multi-line text input implementing tuirealm's [`MockComponent`] and
/// [`tuirealm::component::AppComponent<TextAreaEvent, NoUserEvent>`].
///
/// It is the [`TextInput`](crate::widgets::text_input::TextInput) of several
/// lines, and deliberately nothing more: a fixed-height box the text scrolls
/// through, with the same `▍ ` chrome and the same
/// [`TextInputStyle`] slots, so a form that themes its text fields themes this
/// one too. There are no Vim bindings and no modes — somebody who wants to
/// *edit* rather than *fill in* is better served by opening an editor, which is
/// a different flow.
///
/// ```rust
/// use not_yet_done_ratatui::widgets::text_area::TextArea;
///
/// let area = TextArea::default()
///     .with_title("Description")
///     .with_placeholder("What happened?")
///     .with_rows(6)
///     .with_value("first line\nsecond line");
/// assert_eq!(area.value(), "first line\nsecond line");
/// ```
pub struct TextArea {
    // --- framework state ---
    pub(crate) focused: bool,

    // --- editing state ---
    /// The text, split on newlines. Never empty: an empty text is one empty
    /// line, so there is always a line the cursor can sit on.
    pub(crate) lines: Vec<String>,
    /// Index into `lines`.
    pub(crate) row: usize,
    /// Byte offset of the cursor within `lines[row]`.
    pub(crate) col: usize,
    /// Character index the cursor aims for while moving vertically. Without it
    /// a trip down through a short line and back up would leave the cursor at
    /// that line's end instead of where it started.
    pub(crate) desired: usize,
    /// First visible row.
    pub(crate) scroll: usize,

    // --- configuration (set once at construction) ---
    pub(crate) title: String,
    pub(crate) placeholder: String,
    /// Text rows the widget asks for; what it actually gets is decided by the
    /// area it is rendered into.
    pub(crate) rows: u16,
    pub(crate) inactive_style: TextInputStyle,
    pub(crate) active_style: TextInputStyle,
    pub(crate) keymap: TextAreaKeymap,
}

impl Default for TextArea {
    fn default() -> Self {
        Self {
            focused: false,
            lines: vec![String::new()],
            row: 0,
            col: 0,
            desired: 0,
            scroll: 0,
            title: String::new(),
            placeholder: String::new(),
            rows: DEFAULT_ROWS,
            inactive_style: TextInputStyle::default(),
            active_style: TextInputStyle::default(),
            keymap: TextAreaKeymap::default(),
        }
    }
}

impl TextArea {
    /// Sets the title displayed above the text box.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Sets the placeholder shown on the first row while the text is empty.
    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Sets how many text rows the widget asks for (at least one).
    pub fn with_rows(mut self, rows: u16) -> Self {
        self.rows = rows.max(1);
        self
    }

    /// Sets the initial text; the cursor lands at its end.
    pub fn with_value(mut self, value: impl AsRef<str>) -> Self {
        self.set_value(value.as_ref());
        self
    }

    /// Style applied when this component does not have focus.
    pub fn with_inactive_style(mut self, style: TextInputStyle) -> Self {
        self.inactive_style = style;
        self
    }

    /// Style applied when this component has focus.
    pub fn with_active_style(mut self, style: TextInputStyle) -> Self {
        self.active_style = style;
        self
    }

    /// Overrides the default key bindings.
    pub fn with_keymap(mut self, keymap: TextAreaKeymap) -> Self {
        self.keymap = keymap;
        self
    }

    /// How many text rows the widget asks for.
    pub fn rows(&self) -> u16 {
        self.rows
    }

    /// The whole text, newline-joined.
    pub fn value(&self) -> String {
        self.lines.join("\n")
    }

    /// Replaces the whole text and puts the cursor at its end.
    pub fn set_value(&mut self, value: &str) {
        self.lines = if value.is_empty() {
            vec![String::new()]
        } else {
            value.split('\n').map(str::to_string).collect()
        };
        self.row = self.lines.len() - 1;
        self.col = self.lines[self.row].len();
        self.sync_desired();
        self.scroll = 0;
    }

    /// Cursor position as `(row, byte offset in that row)`.
    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    // --- internal editing helpers (used by component.rs) ------------------

    pub(crate) fn insert_char(&mut self, c: char) {
        let col = self.col;
        self.lines[self.row].insert(col, c);
        self.col += c.len_utf8();
        self.sync_desired();
    }

    /// Splits the current line at the cursor — what `enter` does here, since
    /// submitting a multi-line field is `ctrl+enter`'s job.
    pub(crate) fn insert_newline(&mut self) {
        let rest = self.lines[self.row].split_off(self.col);
        self.lines.insert(self.row + 1, rest);
        self.row += 1;
        self.col = 0;
        self.sync_desired();
    }

    /// Backspace. At the start of a line it joins this line onto the previous
    /// one — the newline is a character like any other.
    pub(crate) fn delete_back(&mut self) {
        if self.col > 0 {
            let mut pos = self.col - 1;
            while !self.lines[self.row].is_char_boundary(pos) {
                pos -= 1;
            }
            self.lines[self.row].remove(pos);
            self.col = pos;
        } else if self.row > 0 {
            let line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].len();
            self.lines[self.row].push_str(&line);
        }
        self.sync_desired();
    }

    pub(crate) fn delete_forward(&mut self) {
        if self.col < self.lines[self.row].len() {
            self.lines[self.row].remove(self.col);
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&next);
        }
        self.sync_desired();
    }

    /// Deletes from the cursor back to the start of the line (`ctrl+u`).
    /// Unlike a single-line input's clear this leaves the rest of the text
    /// alone: wiping a whole description with one keystroke is not a mercy.
    pub(crate) fn kill_to_start(&mut self) {
        self.lines[self.row].replace_range(..self.col, "");
        self.col = 0;
        self.sync_desired();
    }

    pub(crate) fn move_left(&mut self) {
        if self.col > 0 {
            let mut pos = self.col - 1;
            while !self.lines[self.row].is_char_boundary(pos) {
                pos -= 1;
            }
            self.col = pos;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].len();
        }
        self.sync_desired();
    }

    pub(crate) fn move_right(&mut self) {
        if self.col < self.lines[self.row].len() {
            let mut pos = self.col + 1;
            while pos < self.lines[self.row].len() && !self.lines[self.row].is_char_boundary(pos) {
                pos += 1;
            }
            self.col = pos;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
        self.sync_desired();
    }

    pub(crate) fn move_up(&mut self) {
        if self.row == 0 {
            return;
        }
        self.row -= 1;
        self.col = self.offset_of_desired();
    }

    pub(crate) fn move_down(&mut self) {
        if self.row + 1 >= self.lines.len() {
            return;
        }
        self.row += 1;
        self.col = self.offset_of_desired();
    }

    pub(crate) fn move_home(&mut self) {
        self.col = 0;
        self.sync_desired();
    }

    pub(crate) fn move_end(&mut self) {
        self.col = self.lines[self.row].len();
        self.sync_desired();
    }

    /// Scrolls so the cursor row is among the `visible` rows on screen.
    pub(crate) fn ensure_visible(&mut self, visible: usize) {
        let visible = visible.max(1);
        if self.row < self.scroll {
            self.scroll = self.row;
        } else if self.row >= self.scroll + visible {
            self.scroll = self.row + 1 - visible;
        }
    }

    /// Records the column a vertical move should aim for.
    fn sync_desired(&mut self) {
        self.desired = self.lines[self.row][..self.col].chars().count();
    }

    /// The byte offset in the current line closest to [`Self::desired`].
    fn offset_of_desired(&self) -> usize {
        let line = &self.lines[self.row];
        line.char_indices()
            .nth(self.desired)
            .map(|(i, _)| i)
            .unwrap_or(line.len())
    }
}

#[cfg(test)]
mod tests {
    use super::TextArea;

    fn area(value: &str) -> TextArea {
        TextArea::default().with_value(value)
    }

    /// The value is the lines joined back together, so a caller never has to
    /// know the widget keeps them apart.
    #[test]
    fn a_value_survives_the_round_trip() {
        assert_eq!(area("").value(), "");
        assert_eq!(area("one").value(), "one");
        assert_eq!(area("one\n\nthree").value(), "one\n\nthree");
    }

    /// `enter` splits the line, backspace at column zero joins it back — the
    /// newline behaves like any other character.
    #[test]
    fn a_newline_splits_and_a_backspace_joins() {
        let mut a = area("abcd");
        a.move_home();
        a.move_right();
        a.move_right();
        a.insert_newline();
        assert_eq!(a.value(), "ab\ncd");
        assert_eq!(a.cursor(), (1, 0));
        a.delete_back();
        assert_eq!(a.value(), "abcd");
        assert_eq!(a.cursor(), (0, 2));
    }

    /// Going down through a short line and back up returns to the column the
    /// cursor started in, not to where the short line ended.
    #[test]
    fn a_vertical_trip_remembers_the_column() {
        let mut a = area("abcdef\nxy\nabcdef");
        a.row = 0;
        a.col = 5;
        a.desired = 5;
        a.move_down();
        assert_eq!(a.cursor(), (1, 2));
        a.move_down();
        assert_eq!(a.cursor(), (2, 5));
    }

    /// Moving past the end of a line steps to the next one: the text reads as
    /// one stream, whatever the line breaks.
    #[test]
    fn horizontal_movement_crosses_line_ends() {
        let mut a = area("ab\ncd");
        a.row = 0;
        a.col = 2;
        a.move_right();
        assert_eq!(a.cursor(), (1, 0));
        a.move_left();
        assert_eq!(a.cursor(), (0, 2));
    }

    /// `ctrl+u` clears the line the cursor is on, never the whole text.
    #[test]
    fn killing_to_the_start_spares_the_other_lines() {
        let mut a = area("keep\nthrow away");
        a.move_end();
        a.kill_to_start();
        assert_eq!(a.value(), "keep\n");
    }

    /// Wide and multi-byte characters move by character, not by byte.
    #[test]
    fn multi_byte_characters_move_as_one() {
        let mut a = area("äb");
        a.move_home();
        a.move_right();
        assert_eq!(a.cursor(), (0, 2));
        a.delete_back();
        assert_eq!(a.value(), "b");
    }

    /// The window follows the cursor in both directions and never shows fewer
    /// rows than it has.
    #[test]
    fn the_window_follows_the_cursor() {
        let mut a = area("0\n1\n2\n3\n4\n5");
        a.row = 5;
        a.ensure_visible(3);
        assert_eq!(a.scroll, 3);
        a.row = 1;
        a.ensure_visible(3);
        assert_eq!(a.scroll, 1);
    }
}
