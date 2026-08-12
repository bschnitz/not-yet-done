//! Motions — the "where does the cursor go" half of vim's grammar.
//!
//! A motion resolves against a [`Buffer`] to a target [`Position`] plus a
//! [`MotionKind`]. The kind is what makes `dw` and `de` differ: an *exclusive*
//! motion deletes up to but not including the target, an *inclusive* one takes
//! the target character too, and a *linewise* one takes whole lines. Keeping
//! that on the motion (not the operator) is why one `apply_operator` handles
//! every combination.
//!
//! How far right a motion may land depends on the caller's mode, not on the
//! motion: normal mode parks the cursor *on* a character, insert mode *between*
//! characters and so one column further. That is [`Bound`], and only the two
//! motions that aim at the line end read it.

use crate::buffer::{Buffer, CharClass, Position};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    /// `w` — start of the next word.
    WordForward,
    /// `b` — start of the previous word.
    WordBackward,
    /// `e` — end of the current/next word.
    WordEnd,
    /// `0`
    LineStart,
    /// `^`
    LineFirstNonBlank,
    /// `$`
    LineEnd,
    /// `gg` (or `{count}gg`)
    FileStart,
    /// `G`
    FileEnd,
}

/// How an operator consumes the span between the cursor and a motion target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionKind {
    /// Target character not included (`w`, `0`, `h`).
    Exclusive,
    /// Target character included (`e`, `$`).
    Inclusive,
    /// Whole lines (`j`, `k`, `G`, `gg`).
    Linewise,
}

/// The rightmost column a motion may land on — vim's two cursor regimes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bound {
    /// Normal and visual mode, and every operator: the cursor sits *on* a
    /// character, so the last one on the line is as far right as it goes.
    OnCharacter,
    /// Insert mode: the cursor sits *between* characters and may stand one past
    /// the last one — the spot the next typed character takes.
    PastEnd,
}

impl Bound {
    fn max_col(self, buf: &Buffer, line: usize) -> usize {
        match self {
            Bound::OnCharacter => buf.last_col(line),
            Bound::PastEnd => buf.end_col(line),
        }
    }
}

impl Motion {
    pub fn kind(self) -> MotionKind {
        match self {
            Motion::Up | Motion::Down | Motion::FileStart | Motion::FileEnd => MotionKind::Linewise,
            Motion::WordEnd | Motion::LineEnd => MotionKind::Inclusive,
            _ => MotionKind::Exclusive,
        }
    }
}

/// Resolve `motion` from the buffer's cursor, applied `count` times, bounded
/// the way normal mode bounds a cursor.
///
/// `count` is a repetition for most motions but an absolute line number for
/// `gg`/`G` — the same overload vim uses (`5G` = line 5).
pub fn resolve(buf: &Buffer, motion: Motion, count: usize, explicit_count: bool) -> Position {
    resolve_bounded(buf, motion, count, explicit_count, Bound::OnCharacter)
}

/// [`resolve`] with the caller's choice of how far right the target may sit —
/// insert mode passes [`Bound::PastEnd`], everything else the default.
pub fn resolve_bounded(
    buf: &Buffer,
    motion: Motion,
    count: usize,
    explicit_count: bool,
    bound: Bound,
) -> Position {
    let start = buf.cursor();
    let count = count.max(1);
    match motion {
        Motion::Left => {
            let mut col = start.col;
            for _ in 0..count {
                match buf.prev_pos(Position::new(start.line, col)) {
                    // Stop at the line start: `h` never wraps in vim.
                    Some(p) if p.line == start.line => col = p.col,
                    _ => break,
                }
            }
            Position::new(start.line, col)
        }
        Motion::Right => {
            let mut col = start.col;
            let max = bound.max_col(buf, start.line);
            for _ in 0..count {
                match buf.next_pos(Position::new(start.line, col)) {
                    Some(p) if p.line == start.line && p.col <= max => col = p.col,
                    _ => break,
                }
            }
            Position::new(start.line, col)
        }
        Motion::Up => Position::new(start.line.saturating_sub(count), start.col),
        Motion::Down => Position::new(
            (start.line + count).min(buf.len_lines().saturating_sub(1)),
            start.col,
        ),
        Motion::LineStart => Position::new(start.line, 0),
        Motion::LineFirstNonBlank => Position::new(start.line, buf.first_non_blank(start.line)),
        Motion::LineEnd => {
            let line = (start.line + count - 1).min(buf.len_lines().saturating_sub(1));
            Position::new(line, bound.max_col(buf, line))
        }
        Motion::FileStart => {
            let line = if explicit_count {
                (count - 1).min(buf.len_lines().saturating_sub(1))
            } else {
                0
            };
            Position::new(line, buf.first_non_blank(line))
        }
        Motion::FileEnd => {
            let line = if explicit_count {
                (count - 1).min(buf.len_lines().saturating_sub(1))
            } else {
                buf.len_lines().saturating_sub(1)
            };
            Position::new(line, buf.first_non_blank(line))
        }
        Motion::WordForward => repeat(buf, start, count, word_forward),
        Motion::WordBackward => repeat(buf, start, count, word_backward),
        Motion::WordEnd => repeat(buf, start, count, word_end),
    }
}

fn repeat(
    buf: &Buffer,
    start: Position,
    count: usize,
    step: fn(&Buffer, Position) -> Position,
) -> Position {
    let mut pos = start;
    for _ in 0..count {
        let next = step(buf, pos);
        if next == pos {
            break;
        }
        pos = next;
    }
    pos
}

/// `w`: leave the current run of same-class characters, then skip blanks.
fn word_forward(buf: &Buffer, start: Position) -> Position {
    let mut pos = start;
    let class = buf.class_at(pos);
    if class != CharClass::Blank {
        while buf.class_at(pos) == class {
            match buf.next_pos(pos) {
                Some(next) => pos = next,
                None => return pos,
            }
        }
    }
    while buf.class_at(pos) == CharClass::Blank {
        match buf.next_pos(pos) {
            Some(next) => pos = next,
            None => return pos,
        }
    }
    pos
}

/// `b`: step back, skip blanks, then walk to the start of that run.
fn word_backward(buf: &Buffer, start: Position) -> Position {
    let Some(mut pos) = buf.prev_pos(start) else {
        return start;
    };
    while buf.class_at(pos) == CharClass::Blank {
        match buf.prev_pos(pos) {
            Some(prev) => pos = prev,
            None => return pos,
        }
    }
    let class = buf.class_at(pos);
    while let Some(prev) = buf.prev_pos(pos) {
        if buf.class_at(prev) != class {
            break;
        }
        pos = prev;
    }
    pos
}

/// `e`: step forward, skip blanks, then walk to the last character of that run.
fn word_end(buf: &Buffer, start: Position) -> Position {
    let Some(mut pos) = buf.next_pos(start) else {
        return start;
    };
    while buf.class_at(pos) == CharClass::Blank {
        match buf.next_pos(pos) {
            Some(next) => pos = next,
            None => return pos,
        }
    }
    let class = buf.class_at(pos);
    while let Some(next) = buf.next_pos(pos) {
        if buf.class_at(next) != class {
            break;
        }
        pos = next;
    }
    pos
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(text: &str, line: usize, col: usize) -> Buffer {
        let mut b = Buffer::from_text(text);
        b.set_cursor(Position::new(line, col));
        b
    }

    #[test]
    fn h_and_l_stay_on_their_line() {
        let b = buf("ab\ncd", 1, 0);
        assert_eq!(resolve(&b, Motion::Left, 1, false), Position::new(1, 0));
        let b = buf("ab", 0, 1);
        assert_eq!(resolve(&b, Motion::Right, 5, true), Position::new(0, 1));
    }

    #[test]
    fn counts_repeat_a_motion() {
        let b = buf("abcdef", 0, 0);
        assert_eq!(resolve(&b, Motion::Right, 3, true), Position::new(0, 3));
    }

    #[test]
    fn w_crosses_words_and_lines() {
        let b = buf("foo bar\nbaz", 0, 0);
        let p = resolve(&b, Motion::WordForward, 1, false);
        assert_eq!(p, Position::new(0, 4));
        let b = buf("foo bar\nbaz", 0, 4);
        assert_eq!(
            resolve(&b, Motion::WordForward, 1, false),
            Position::new(1, 0)
        );
    }

    #[test]
    fn w_treats_punctuation_as_its_own_word() {
        let b = buf("foo.bar", 0, 0);
        assert_eq!(
            resolve(&b, Motion::WordForward, 1, false),
            Position::new(0, 3)
        );
        let b = buf("foo.bar", 0, 3);
        assert_eq!(
            resolve(&b, Motion::WordForward, 1, false),
            Position::new(0, 4)
        );
    }

    #[test]
    fn b_goes_to_the_start_of_the_previous_word() {
        let b = buf("foo bar", 0, 5);
        assert_eq!(
            resolve(&b, Motion::WordBackward, 1, false),
            Position::new(0, 4)
        );
        let b = buf("foo bar", 0, 4);
        assert_eq!(
            resolve(&b, Motion::WordBackward, 1, false),
            Position::new(0, 0)
        );
    }

    #[test]
    fn e_lands_on_the_last_character_of_the_word() {
        let b = buf("foo bar", 0, 0);
        assert_eq!(resolve(&b, Motion::WordEnd, 1, false), Position::new(0, 2));
        assert_eq!(resolve(&b, Motion::WordEnd, 2, true), Position::new(0, 6));
    }

    #[test]
    fn dollar_and_zero_hit_the_line_bounds() {
        let b = buf("  hi", 0, 2);
        assert_eq!(
            resolve(&b, Motion::LineStart, 1, false),
            Position::new(0, 0)
        );
        assert_eq!(
            resolve(&b, Motion::LineFirstNonBlank, 1, false),
            Position::new(0, 2)
        );
        assert_eq!(resolve(&b, Motion::LineEnd, 1, false), Position::new(0, 3));
    }

    #[test]
    fn gg_and_g_take_a_count_as_an_absolute_line() {
        let b = buf("a\nb\nc", 0, 0);
        assert_eq!(resolve(&b, Motion::FileEnd, 1, false), Position::new(2, 0));
        assert_eq!(resolve(&b, Motion::FileEnd, 2, true), Position::new(1, 0));
        assert_eq!(
            resolve(&b, Motion::FileStart, 1, false),
            Position::new(0, 0)
        );
        assert_eq!(resolve(&b, Motion::FileStart, 3, true), Position::new(2, 0));
    }

    #[test]
    fn past_end_lets_the_line_end_motions_reach_one_further() {
        let b = buf("ab", 0, 1);
        // Normal mode parks on the last character, insert mode behind it.
        assert_eq!(resolve(&b, Motion::Right, 1, false), Position::new(0, 1));
        for motion in [Motion::Right, Motion::LineEnd] {
            let p = resolve_bounded(&b, motion, 1, false, Bound::PastEnd);
            assert_eq!(p, Position::new(0, 2), "{motion:?}");
        }
    }

    #[test]
    fn past_end_still_stops_at_the_line_end() {
        // `l` never wraps to the next line, in either mode.
        let b = buf("ab\ncd", 0, 2);
        assert_eq!(
            resolve_bounded(&b, Motion::Right, 3, true, Bound::PastEnd),
            Position::new(0, 2)
        );
    }

    #[test]
    fn a_wide_character_is_one_step_either_way() {
        let b = buf("äb", 0, 0);
        assert_eq!(resolve(&b, Motion::Right, 2, true), Position::new(0, 2));
        assert_eq!(
            resolve_bounded(&b, Motion::Right, 2, true, Bound::PastEnd),
            Position::new(0, 3)
        );
    }

    #[test]
    fn motion_kinds_match_vims_grammar() {
        assert_eq!(Motion::WordForward.kind(), MotionKind::Exclusive);
        assert_eq!(Motion::WordEnd.kind(), MotionKind::Inclusive);
        assert_eq!(Motion::LineEnd.kind(), MotionKind::Inclusive);
        assert_eq!(Motion::Down.kind(), MotionKind::Linewise);
    }
}
