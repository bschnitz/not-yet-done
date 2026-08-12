//! Operators — the "what happens to the span" half of vim's grammar.
//!
//! An operator plus a [`Motion`] spans a region; the motion's [`MotionKind`]
//! decides how that region is cut. Because the kind travels with the motion,
//! `dw`, `de` and `dj` all land in the same code path here — adding a motion
//! never means touching an operator.
//!
//! Deleted and yanked text goes through a [`RegisterSink`], so an operator
//! never needs to know about naming or appending.

use crate::buffer::{Buffer, Position};
use crate::motion::{self, Motion, MotionKind};
use crate::register::{Register, RegisterSink};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    /// `d`
    Delete,
    /// `c` — like delete, but the caller switches to insert mode afterwards.
    Change,
    /// `y`
    Yank,
}

/// Run `{count}{op}{motion}` from the buffer's cursor.
///
/// Returns `true` when the caller should enter insert mode (`c`).
pub fn apply(
    buf: &mut Buffer,
    sink: &mut RegisterSink,
    op: Operator,
    motion: Motion,
    count: usize,
    explicit_count: bool,
) -> bool {
    // `cw` on a non-blank behaves like `ce` — vim's one deliberate
    // irregularity, and the one users notice immediately when it is missing.
    let motion = match (op, motion) {
        (Operator::Change, Motion::WordForward)
            if buf
                .char_at(buf.cursor())
                .is_some_and(|c| !c.is_whitespace()) =>
        {
            Motion::WordEnd
        }
        _ => motion,
    };

    let start = buf.cursor();
    let target = motion::resolve(buf, motion, count, explicit_count);

    match motion.kind() {
        MotionKind::Linewise => {
            let (from, to) = ordered_lines(start.line, target.line);
            apply_lines(buf, sink, op, from, to)
        }
        kind => {
            let (from, to) = if start <= target {
                (start, target)
            } else {
                (target, start)
            };
            match kind {
                MotionKind::Inclusive => apply_inclusive(buf, sink, op, from, to),
                _ => apply_exclusive(buf, sink, op, from, to),
            }
        }
    }
}

/// `[from, to]` — the end position is part of the span. That is what an
/// inclusive motion produces, and what a visual selection is.
pub(crate) fn apply_inclusive(
    buf: &mut Buffer,
    sink: &mut RegisterSink,
    op: Operator,
    from: Position,
    to: Position,
) -> bool {
    let end = buf.next_pos(to).unwrap_or(to);
    apply_exclusive(buf, sink, op, from, end)
}

/// `[from, to)`. Returns `true` when the caller should enter insert mode.
fn apply_exclusive(
    buf: &mut Buffer,
    sink: &mut RegisterSink,
    op: Operator,
    from: Position,
    mut to: Position,
) -> bool {
    // A charwise operator never eats the line break at the far end.
    if to.line > from.line && to.col == 0 {
        if let Some(prev) = buf.prev_pos(to) {
            to = prev;
        }
    }
    if from >= to {
        return false;
    }
    apply_range(buf, sink, op, from, to);
    op == Operator::Change
}

/// `dd` / `cc` / `yy` — `count` whole lines from the cursor line down.
pub fn apply_current_lines(
    buf: &mut Buffer,
    sink: &mut RegisterSink,
    op: Operator,
    count: usize,
) -> bool {
    let from = buf.cursor().line;
    let to = (from + count.max(1) - 1).min(buf.len_lines().saturating_sub(1));
    apply_lines(buf, sink, op, from, to)
}

/// `D` / `C` — from the cursor to the end of the line.
pub fn apply_to_line_end(buf: &mut Buffer, sink: &mut RegisterSink, op: Operator) -> bool {
    let start = buf.cursor();
    let end = Position::new(start.line, buf.end_col(start.line));
    if start == end {
        return op == Operator::Change;
    }
    apply_range(buf, sink, op, start, end);
    op == Operator::Change
}

/// `x` — delete `count` characters under and after the cursor, never crossing
/// into the next line.
pub fn delete_chars(buf: &mut Buffer, sink: &mut RegisterSink, count: usize) {
    let start = buf.cursor();
    let mut end = start;
    for _ in 0..count.max(1) {
        match buf.next_pos(end) {
            Some(next) if next.line == start.line => end = next,
            _ => break,
        }
    }
    if start == end {
        return;
    }
    buf.snapshot();
    sink.set_charwise(buf.delete_range(start, end));
    buf.set_cursor(start);
}

/// `p` (`after == true`) / `P`, repeated `count` times.
pub fn paste(buf: &mut Buffer, reg: &Register, after: bool, count: usize) {
    if reg.text.is_empty() && !reg.linewise {
        return;
    }
    buf.snapshot();
    let cursor = buf.cursor();
    if reg.linewise {
        let mut lines = Vec::new();
        for _ in 0..count.max(1) {
            lines.extend(reg.text.split('\n').map(str::to_string));
        }
        let at = buf.insert_lines(cursor.line, lines, after);
        buf.set_cursor(Position::new(at, buf.first_non_blank(at)));
        return;
    }
    // Charwise `p` pastes *after* the cursor character, `P` before it.
    let at = if after {
        buf.next_pos(cursor)
            .filter(|p| p.line == cursor.line)
            .unwrap_or(Position::new(cursor.line, buf.end_col(cursor.line)))
    } else {
        cursor
    };
    let mut end = at;
    for _ in 0..count.max(1) {
        end = buf.insert_str(end, &reg.text);
    }
    // Vim parks on the last pasted character, not past it.
    buf.set_cursor(buf.prev_pos(end).unwrap_or(end));
}

fn ordered_lines(a: usize, b: usize) -> (usize, usize) {
    if a <= b { (a, b) } else { (b, a) }
}

/// Linewise variant shared by `d{linewise motion}`, `dd` and friends.
pub(crate) fn apply_lines(
    buf: &mut Buffer,
    sink: &mut RegisterSink,
    op: Operator,
    from: usize,
    to: usize,
) -> bool {
    if op == Operator::Yank {
        sink.set_linewise(&buf.lines()[from..=to.min(buf.len_lines() - 1)]);
        buf.set_cursor(Position::new(from, buf.first_non_blank(from)));
        return false;
    }
    buf.snapshot();
    if op == Operator::Change {
        // `cc` empties the lines but keeps one to type on. Emptying the first
        // line in place — rather than deleting it and inserting a fresh one —
        // is what keeps a single-line buffer at one line.
        sink.set_linewise(&buf.lines()[from..=to]);
        if to > from {
            buf.delete_lines(from + 1, to);
        }
        buf.delete_range(
            Position::new(from, 0),
            Position::new(from, buf.end_col(from)),
        );
        buf.set_cursor_insert(Position::new(from, 0));
        return true;
    }
    let removed = buf.delete_lines(from, to);
    sink.set_linewise(&removed);
    let line = from.min(buf.len_lines().saturating_sub(1));
    buf.set_cursor(Position::new(line, buf.first_non_blank(line)));
    false
}

/// Charwise variant. `from < to`, both already clamped by the caller.
pub(crate) fn apply_range(
    buf: &mut Buffer,
    sink: &mut RegisterSink,
    op: Operator,
    from: Position,
    to: Position,
) {
    if op == Operator::Yank {
        let text = yank_range(buf, from, to);
        sink.set_charwise(text);
        buf.set_cursor(from);
        return;
    }
    buf.snapshot();
    sink.set_charwise(buf.delete_range(from, to));
    if op == Operator::Change {
        buf.set_cursor_insert(from);
    } else {
        buf.set_cursor(from);
    }
}

/// Read `[from, to)` without touching the buffer — `y` must not be a mutation.
fn yank_range(buf: &Buffer, from: Position, to: Position) -> String {
    if from.line == to.line {
        return buf.line(from.line)[from.col..to.col].to_string();
    }
    let mut out = buf.line(from.line)[from.col..].to_string();
    for line in from.line + 1..to.line {
        out.push('\n');
        out.push_str(buf.line(line));
    }
    out.push('\n');
    out.push_str(&buf.line(to.line)[..to.col]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::register::Registers;

    fn buf(text: &str, line: usize, col: usize) -> Buffer {
        let mut b = Buffer::from_text(text);
        b.set_cursor(Position::new(line, col));
        b
    }

    #[test]
    fn dw_deletes_up_to_the_next_word() {
        let mut b = buf("foo bar baz", 0, 0);
        let mut regs = Registers::new();
        apply(
            &mut b,
            &mut RegisterSink::unnamed(&mut regs),
            Operator::Delete,
            Motion::WordForward,
            1,
            false,
        );
        assert_eq!(b.text(), "bar baz");
        assert_eq!(regs.get(None).text, "foo ");
        assert!(!regs.get(None).linewise);
    }

    #[test]
    fn de_takes_the_last_character_too() {
        let mut b = buf("foo bar", 0, 0);
        let mut regs = Registers::new();
        apply(
            &mut b,
            &mut RegisterSink::unnamed(&mut regs),
            Operator::Delete,
            Motion::WordEnd,
            1,
            false,
        );
        assert_eq!(b.text(), " bar");
    }

    #[test]
    fn cw_on_a_word_behaves_like_ce() {
        let mut b = buf("foo bar", 0, 0);
        let mut regs = Registers::new();
        let insert = apply(
            &mut b,
            &mut RegisterSink::unnamed(&mut regs),
            Operator::Change,
            Motion::WordForward,
            1,
            false,
        );
        assert!(insert, "c must hand the caller into insert mode");
        assert_eq!(b.text(), " bar", "the blank after the word survives");
    }

    #[test]
    fn a_count_multiplies_the_motion() {
        let mut b = buf("a b c d", 0, 0);
        let mut regs = Registers::new();
        apply(
            &mut b,
            &mut RegisterSink::unnamed(&mut regs),
            Operator::Delete,
            Motion::WordForward,
            2,
            true,
        );
        assert_eq!(b.text(), "c d");
    }

    #[test]
    fn dj_is_linewise_and_takes_both_lines() {
        let mut b = buf("one\ntwo\nthree", 0, 1);
        let mut regs = Registers::new();
        apply(
            &mut b,
            &mut RegisterSink::unnamed(&mut regs),
            Operator::Delete,
            Motion::Down,
            1,
            false,
        );
        assert_eq!(b.text(), "three");
        assert_eq!(regs.get(None).text, "one\ntwo");
        assert!(regs.get(None).linewise);
    }

    #[test]
    fn dk_deletes_upwards_from_the_cursor() {
        let mut b = buf("one\ntwo\nthree", 2, 0);
        let mut regs = Registers::new();
        apply(
            &mut b,
            &mut RegisterSink::unnamed(&mut regs),
            Operator::Delete,
            Motion::Up,
            1,
            false,
        );
        assert_eq!(b.text(), "one");
    }

    #[test]
    fn a_named_register_keeps_what_an_operator_took() {
        let mut b = buf("foo bar", 0, 0);
        let mut regs = Registers::new();
        apply(
            &mut b,
            &mut RegisterSink::new(&mut regs, Some('a')),
            Operator::Delete,
            Motion::WordForward,
            1,
            false,
        );
        assert_eq!(regs.get(Some('a')).text, "foo ");
        assert_eq!(regs.get(None).text, "foo ", "and the unnamed one as well");
    }

    #[test]
    fn the_black_hole_register_leaves_the_unnamed_one_alone() {
        let mut b = buf("keep\ndrop", 0, 0);
        let mut regs = Registers::new();
        apply_current_lines(
            &mut b,
            &mut RegisterSink::unnamed(&mut regs),
            Operator::Yank,
            1,
        );
        apply_current_lines(
            &mut b,
            &mut RegisterSink::new(&mut regs, Some('_')),
            Operator::Delete,
            1,
        );
        assert_eq!(b.text(), "drop");
        assert_eq!(
            regs.get(None).text,
            "keep",
            "\"_d must not clobber the yank"
        );
    }

    #[test]
    fn dd_keeps_one_empty_line_in_a_single_line_buffer() {
        let mut b = buf("only", 0, 0);
        let mut regs = Registers::new();
        apply_current_lines(
            &mut b,
            &mut RegisterSink::unnamed(&mut regs),
            Operator::Delete,
            1,
        );
        assert_eq!(b.text(), "");
        assert_eq!(b.cursor(), Position::new(0, 0));
    }

    #[test]
    fn dd_with_a_count_takes_several_lines() {
        let mut b = buf("a\nb\nc\nd", 1, 0);
        let mut regs = Registers::new();
        apply_current_lines(
            &mut b,
            &mut RegisterSink::unnamed(&mut regs),
            Operator::Delete,
            2,
        );
        assert_eq!(b.text(), "a\nd");
        assert_eq!(regs.get(None).text, "b\nc");
    }

    #[test]
    fn cc_empties_the_line_but_keeps_it() {
        let mut b = buf("  hello", 0, 3);
        let mut regs = Registers::new();
        let insert = apply_current_lines(
            &mut b,
            &mut RegisterSink::unnamed(&mut regs),
            Operator::Change,
            1,
        );
        assert!(insert);
        assert_eq!(b.text(), "");
        assert_eq!(b.len_lines(), 1);
    }

    #[test]
    fn yank_leaves_the_buffer_alone() {
        let mut b = buf("foo bar", 0, 0);
        let mut regs = Registers::new();
        apply(
            &mut b,
            &mut RegisterSink::unnamed(&mut regs),
            Operator::Yank,
            Motion::WordForward,
            1,
            false,
        );
        assert_eq!(b.text(), "foo bar");
        assert_eq!(regs.get(None).text, "foo ");
        assert!(!b.is_dirty(), "y must not mark the buffer dirty");
    }

    #[test]
    fn yy_is_linewise() {
        let mut b = buf("a\nb", 0, 0);
        let mut regs = Registers::new();
        apply_current_lines(
            &mut b,
            &mut RegisterSink::unnamed(&mut regs),
            Operator::Yank,
            2,
        );
        assert_eq!(regs.get(None).text, "a\nb");
        assert!(regs.get(None).linewise);
        assert_eq!(b.text(), "a\nb");
    }

    #[test]
    fn shift_d_and_shift_c_stop_at_the_line_end() {
        let mut b = buf("hello world", 0, 5);
        let mut regs = Registers::new();
        apply_to_line_end(
            &mut b,
            &mut RegisterSink::unnamed(&mut regs),
            Operator::Delete,
        );
        assert_eq!(b.text(), "hello");
        assert_eq!(regs.get(None).text, " world");
    }

    #[test]
    fn x_deletes_under_the_cursor_and_stays_on_the_line() {
        let mut b = buf("ab\ncd", 0, 1);
        let mut regs = Registers::new();
        delete_chars(&mut b, &mut RegisterSink::unnamed(&mut regs), 5);
        assert_eq!(b.text(), "a\ncd", "x must not swallow the line break");
        assert_eq!(regs.get(None).text, "b");
    }

    #[test]
    fn charwise_paste_lands_after_the_cursor() {
        let mut b = buf("ac", 0, 0);
        paste(&mut b, &Register::charwise("b"), true, 1);
        assert_eq!(b.text(), "abc");
        assert_eq!(b.cursor(), Position::new(0, 1));
    }

    #[test]
    fn capital_p_pastes_before_the_cursor() {
        let mut b = buf("bc", 0, 0);
        paste(&mut b, &Register::charwise("a"), false, 1);
        assert_eq!(b.text(), "abc");
    }

    #[test]
    fn linewise_paste_opens_a_new_line_below() {
        let mut b = buf("one\ntwo", 0, 0);
        paste(&mut b, &Register::linewise(&["mid".to_string()]), true, 1);
        assert_eq!(b.text(), "one\nmid\ntwo");
        assert_eq!(b.cursor(), Position::new(1, 0));
    }

    #[test]
    fn a_count_repeats_the_paste() {
        let mut b = buf("x", 0, 0);
        paste(&mut b, &Register::charwise("ab"), true, 3);
        assert_eq!(b.text(), "xababab");
    }

    #[test]
    fn delete_then_paste_round_trips_a_word() {
        let mut b = buf("foo bar", 0, 4);
        let mut regs = Registers::new();
        apply(
            &mut b,
            &mut RegisterSink::unnamed(&mut regs),
            Operator::Delete,
            Motion::LineEnd,
            1,
            false,
        );
        assert_eq!(b.text(), "foo ");
        b.set_cursor(Position::new(0, 3));
        paste(&mut b, regs.get(None), true, 1);
        assert_eq!(b.text(), "foo bar");
    }

    #[test]
    fn an_operator_is_one_undo_step() {
        let mut b = buf("foo bar", 0, 0);
        let mut regs = Registers::new();
        apply(
            &mut b,
            &mut RegisterSink::unnamed(&mut regs),
            Operator::Delete,
            Motion::WordForward,
            1,
            false,
        );
        assert!(b.undo());
        assert_eq!(b.text(), "foo bar");
    }
}
