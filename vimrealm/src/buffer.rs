//! The text buffer: lines, a cursor, and snapshot undo.
//!
//! Storage is a `Vec<String>`, one entry per logical line, never empty (an
//! "empty" buffer is one empty line — the same invariant vim keeps). That is
//! ample for the buffers this widget is aimed at (chat messages, commit
//! messages, ticket bodies) and keeps every edit a plain string splice.
//!
//! Everything above this module goes through [`Buffer`]'s methods and
//! [`Position`] — no caller indexes `lines` directly. Swapping the storage for
//! a rope later therefore stays a change to this file plus [`Buffer`]'s
//! internals, not a rewrite of the motions and operators.
//!
//! Positions are `(line, byte offset within that line)`. Byte offsets are
//! always on a `char` boundary; the helpers here are the only ones that move
//! them, so that invariant is local.

/// A cursor position: line index plus byte offset inside that line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Position {
    pub line: usize,
    pub col: usize,
}

impl Position {
    pub fn new(line: usize, col: usize) -> Self {
        Self { line, col }
    }
}

/// A snapshot of the whole buffer, taken for undo.
#[derive(Debug, Clone)]
struct Snapshot {
    lines: Vec<String>,
    cursor: Position,
}

/// Character class used by word motions — vim's `iskeyword` split, simplified
/// to "word / punctuation / blank".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharClass {
    Blank,
    Word,
    Punct,
}

pub fn classify(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Blank
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

pub struct Buffer {
    lines: Vec<String>,
    cursor: Position,
    /// Snapshots taken *before* each change, newest last.
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// Set by every mutation, cleared by the host on save. Drives `:q`'s
    /// refusal to discard unwritten work.
    dirty: bool,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::from_text("")
    }
}

impl Buffer {
    pub fn from_text(text: &str) -> Self {
        Self {
            lines: split_lines(text),
            cursor: Position::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            dirty: false,
        }
    }

    /// Replace the whole content, resetting cursor and undo history.
    pub fn set_text(&mut self, text: &str) {
        self.lines = split_lines(text);
        self.cursor = Position::default();
        self.undo.clear();
        self.redo.clear();
        self.dirty = false;
    }

    /// The buffer as one string, lines joined with `\n`. A trailing newline is
    /// *not* added — the host decides whether its format wants one.
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn line(&self, idx: usize) -> &str {
        self.lines.get(idx).map(String::as_str).unwrap_or("")
    }

    pub fn len_lines(&self) -> usize {
        self.lines.len()
    }

    pub fn cursor(&self) -> Position {
        self.cursor
    }

    pub fn set_cursor(&mut self, pos: Position) {
        self.cursor = self.clamp(pos);
    }

    /// Move the cursor without the normal-mode "must sit on a character"
    /// clamp — insert mode may sit one past the last character.
    pub fn set_cursor_insert(&mut self, pos: Position) {
        let line = pos.line.min(self.lines.len().saturating_sub(1));
        let col = pos.col.min(self.line(line).len());
        self.cursor = Position::new(line, self.snap_boundary(line, col));
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    // -----------------------------------------------------------------
    // Position arithmetic
    // -----------------------------------------------------------------

    /// Clamp to a *normal-mode* position: on an existing line, on a character
    /// (never past the last one, unless the line is empty).
    pub fn clamp(&self, pos: Position) -> Position {
        let line = pos.line.min(self.lines.len().saturating_sub(1));
        let max = self.last_col(line);
        Position::new(line, self.snap_boundary(line, pos.col.min(max)))
    }

    /// Byte offset of the last character on `line` — where normal mode parks
    /// the cursor at `$`. `0` for an empty line.
    pub fn last_col(&self, line: usize) -> usize {
        let text = self.line(line);
        match text.char_indices().next_back() {
            Some((i, _)) => i,
            None => 0,
        }
    }

    /// Byte offset one past the last character — where insert mode may sit.
    pub fn end_col(&self, line: usize) -> usize {
        self.line(line).len()
    }

    /// First non-blank byte offset on `line` (`^`), or the last column when
    /// the line is all blanks.
    pub fn first_non_blank(&self, line: usize) -> usize {
        let text = self.line(line);
        text.char_indices()
            .find(|(_, c)| !c.is_whitespace())
            .map(|(i, _)| i)
            .unwrap_or_else(|| self.last_col(line))
    }

    /// Round `col` down to the nearest `char` boundary of `line`.
    fn snap_boundary(&self, line: usize, col: usize) -> usize {
        let text = self.line(line);
        let mut col = col.min(text.len());
        while col > 0 && !text.is_char_boundary(col) {
            col -= 1;
        }
        col
    }

    /// The character at `pos`, if any.
    pub fn char_at(&self, pos: Position) -> Option<char> {
        self.line(pos.line)[pos.col..].chars().next()
    }

    /// Next position, treating the buffer as one stream: past the last
    /// character of a line comes the line end (`col == len`), then the next
    /// line's first character. `None` at the very end of the buffer.
    pub fn next_pos(&self, pos: Position) -> Option<Position> {
        let text = self.line(pos.line);
        if pos.col < text.len() {
            let step = text[pos.col..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
            return Some(Position::new(pos.line, pos.col + step));
        }
        if pos.line + 1 < self.lines.len() {
            return Some(Position::new(pos.line + 1, 0));
        }
        None
    }

    /// Previous position; mirror of [`Self::next_pos`].
    pub fn prev_pos(&self, pos: Position) -> Option<Position> {
        if pos.col > 0 {
            let text = self.line(pos.line);
            let prev = text[..pos.col]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
            return Some(Position::new(pos.line, prev));
        }
        if pos.line > 0 {
            let prev_line = pos.line - 1;
            return Some(Position::new(prev_line, self.end_col(prev_line)));
        }
        None
    }

    /// Class of the character at `pos`; a line end counts as blank (that is
    /// what makes `w` step over line breaks the way vim does).
    pub fn class_at(&self, pos: Position) -> CharClass {
        self.char_at(pos).map(classify).unwrap_or(CharClass::Blank)
    }

    // -----------------------------------------------------------------
    // Undo
    // -----------------------------------------------------------------

    /// Remember the current state as one undo step. Call once per user-visible
    /// change — an insert-mode session takes a single snapshot on entry, so
    /// `u` undoes the whole typed run like vim does.
    pub fn snapshot(&mut self) {
        self.undo.push(Snapshot {
            lines: self.lines.clone(),
            cursor: self.cursor,
        });
        self.redo.clear();
    }

    /// Restore the newest snapshot. Returns `false` when there is nothing to
    /// undo (so the caller can report "Already at oldest change").
    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop() else {
            return false;
        };
        self.redo.push(Snapshot {
            lines: std::mem::replace(&mut self.lines, prev.lines),
            cursor: self.cursor,
        });
        self.cursor = self.clamp(prev.cursor);
        self.dirty = true;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(Snapshot {
            lines: std::mem::replace(&mut self.lines, next.lines),
            cursor: self.cursor,
        });
        self.cursor = self.clamp(next.cursor);
        self.dirty = true;
        true
    }

    // -----------------------------------------------------------------
    // Mutations
    // -----------------------------------------------------------------

    pub fn insert_char(&mut self, pos: Position, c: char) -> Position {
        self.dirty = true;
        if c == '\n' {
            return self.split_line(pos);
        }
        let line = &mut self.lines[pos.line];
        let col = pos.col.min(line.len());
        line.insert(col, c);
        Position::new(pos.line, col + c.len_utf8())
    }

    pub fn insert_str(&mut self, pos: Position, s: &str) -> Position {
        let mut at = pos;
        for c in s.chars() {
            at = self.insert_char(at, c);
        }
        at
    }

    /// Split `line` at `pos`, moving the tail onto a new line below.
    pub fn split_line(&mut self, pos: Position) -> Position {
        self.dirty = true;
        let col = pos.col.min(self.lines[pos.line].len());
        let tail = self.lines[pos.line].split_off(col);
        self.lines.insert(pos.line + 1, tail);
        Position::new(pos.line + 1, 0)
    }

    /// Insert `text` as whole lines below (`after == true`) or above `line`.
    /// Returns the first inserted line's index.
    pub fn insert_lines(&mut self, line: usize, text: Vec<String>, after: bool) -> usize {
        self.dirty = true;
        let at = if after {
            (line + 1).min(self.lines.len())
        } else {
            line
        };
        for (i, l) in text.into_iter().enumerate() {
            self.lines.insert(at + i, l);
        }
        at
    }

    /// Delete `[from, to)` and return the removed text. `from` must not be
    /// after `to`; both are clamped into the buffer.
    pub fn delete_range(&mut self, from: Position, to: Position) -> String {
        self.dirty = true;
        let (from, to) = (self.clamp_stream(from), self.clamp_stream(to));
        if from >= to {
            return String::new();
        }
        if from.line == to.line {
            let removed = self.lines[from.line][from.col..to.col].to_string();
            self.lines[from.line].replace_range(from.col..to.col, "");
            return removed;
        }
        let mut removed = self.lines[from.line][from.col..].to_string();
        let tail = self.lines[to.line][to.col..].to_string();
        for line in &self.lines[from.line + 1..to.line] {
            removed.push('\n');
            removed.push_str(line);
        }
        removed.push('\n');
        removed.push_str(&self.lines[to.line][..to.col]);
        self.lines[from.line].replace_range(from.col.., &tail);
        self.lines.drain(from.line + 1..=to.line);
        removed
    }

    /// Delete whole lines `[from, to]` inclusive, returning them. The buffer
    /// keeps at least one (empty) line.
    pub fn delete_lines(&mut self, from: usize, to: usize) -> Vec<String> {
        self.dirty = true;
        let from = from.min(self.lines.len().saturating_sub(1));
        let to = to.min(self.lines.len().saturating_sub(1));
        let removed: Vec<String> = self.lines.drain(from..=to).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        removed
    }

    /// Clamp to a position usable as a *range endpoint*: unlike
    /// [`Self::clamp`], the end of a line is allowed (a range may cover the
    /// last character).
    fn clamp_stream(&self, pos: Position) -> Position {
        let line = pos.line.min(self.lines.len().saturating_sub(1));
        Position::new(line, self.snap_boundary(line, pos.col))
    }
}

/// Split into lines the way an editor must: a trailing `\n` yields a final
/// empty line, and an empty input is one empty line (never zero).
fn split_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|l| l.trim_end_matches('\r').to_string())
        .collect();
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_is_one_empty_line() {
        let b = Buffer::from_text("");
        assert_eq!(b.len_lines(), 1);
        assert_eq!(b.text(), "");
    }

    #[test]
    fn trailing_newline_yields_a_final_empty_line() {
        let b = Buffer::from_text("a\n");
        assert_eq!(b.lines(), ["a", ""]);
        assert_eq!(b.text(), "a\n");
    }

    #[test]
    fn crlf_input_loses_the_carriage_return() {
        let b = Buffer::from_text("a\r\nb");
        assert_eq!(b.lines(), ["a", "b"]);
    }

    #[test]
    fn clamp_parks_on_the_last_character_not_past_it() {
        let b = Buffer::from_text("abc");
        assert_eq!(b.clamp(Position::new(0, 99)), Position::new(0, 2));
        assert_eq!(b.last_col(0), 2);
        assert_eq!(b.end_col(0), 3);
    }

    #[test]
    fn positions_step_over_multibyte_characters_whole() {
        let b = Buffer::from_text("äöü");
        let mut pos = Position::new(0, 0);
        let mut seen = Vec::new();
        while let Some(next) = b.next_pos(pos) {
            seen.push(pos.col);
            pos = next;
        }
        seen.push(pos.col);
        assert_eq!(seen, vec![0, 2, 4, 6]);
        assert_eq!(b.prev_pos(Position::new(0, 4)), Some(Position::new(0, 2)));
    }

    #[test]
    fn next_pos_walks_from_line_end_into_the_next_line() {
        let b = Buffer::from_text("ab\ncd");
        assert_eq!(b.next_pos(Position::new(0, 2)), Some(Position::new(1, 0)));
        assert_eq!(b.next_pos(Position::new(1, 2)), None);
        assert_eq!(b.prev_pos(Position::new(1, 0)), Some(Position::new(0, 2)));
    }

    #[test]
    fn delete_range_across_lines_joins_the_remainder() {
        let mut b = Buffer::from_text("hello\nworld");
        let removed = b.delete_range(Position::new(0, 2), Position::new(1, 3));
        assert_eq!(removed, "llo\nwor");
        assert_eq!(b.text(), "held");
    }

    #[test]
    fn delete_lines_keeps_one_empty_line() {
        let mut b = Buffer::from_text("a\nb");
        let removed = b.delete_lines(0, 1);
        assert_eq!(removed, vec!["a", "b"]);
        assert_eq!(b.lines(), [""]);
    }

    #[test]
    fn undo_and_redo_restore_content_and_cursor() {
        let mut b = Buffer::from_text("abc");
        b.set_cursor(Position::new(0, 1));
        b.snapshot();
        b.delete_range(Position::new(0, 0), Position::new(0, 3));
        assert_eq!(b.text(), "");

        assert!(b.undo());
        assert_eq!(b.text(), "abc");
        assert_eq!(b.cursor(), Position::new(0, 1));

        assert!(b.redo());
        assert_eq!(b.text(), "");
        assert!(b.undo(), "the restored snapshot is undoable again");
    }

    #[test]
    fn undo_on_a_pristine_buffer_reports_nothing_to_do() {
        let mut b = Buffer::from_text("abc");
        assert!(!b.undo());
    }

    #[test]
    fn a_new_change_drops_the_redo_branch() {
        let mut b = Buffer::from_text("abc");
        b.snapshot();
        b.delete_range(Position::new(0, 0), Position::new(0, 1));
        b.undo();
        b.snapshot();
        b.insert_char(Position::new(0, 0), 'x');
        assert!(!b.redo(), "redo branch must be dropped by the new edit");
    }

    #[test]
    fn dirty_tracks_mutations_and_clears_on_save() {
        let mut b = Buffer::from_text("a");
        assert!(!b.is_dirty());
        b.insert_char(Position::new(0, 0), 'x');
        assert!(b.is_dirty());
        b.mark_clean();
        assert!(!b.is_dirty());
    }
}
