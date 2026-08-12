//! Finding text: the arithmetic behind `/`, `?`, `n` and `N`.
//!
//! Vim searches with regular expressions. This searches for a literal
//! substring — it keeps the crate free of a regex dependency, and in the
//! messages and ticket bodies this widget edits a substring is what people
//! type anyway. Case is significant, as in vim without `ignorecase`.
//!
//! Searching always wraps around the buffer (vim's `wrapscan`) and always
//! starts *past* the cursor, which is what makes `n` advance instead of finding
//! the match it is standing on again.

use crate::buffer::{Buffer, Position};

/// Where a match starts, and whether reaching it meant wrapping around the end
/// of the buffer — vim says so in the status line, so the caller needs to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub pos: Position,
    pub wrapped: bool,
}

/// Find `pattern` from `from`, excluding `from` itself. `None` means the
/// pattern is nowhere in the buffer.
pub fn find(buf: &Buffer, pattern: &str, from: Position, forward: bool) -> Option<Hit> {
    if pattern.is_empty() {
        return None;
    }
    match forward {
        true => find_forward(buf, pattern, from),
        false => find_backward(buf, pattern, from),
    }
}

fn find_forward(buf: &Buffer, pattern: &str, from: Position) -> Option<Hit> {
    let start = after(buf.line(from.line), from.col);
    if let Some(col) = match_from(buf.line(from.line), start, pattern) {
        return Some(hit(from.line, col, false));
    }
    for line in from.line + 1..buf.len_lines() {
        if let Some(col) = buf.line(line).find(pattern) {
            return Some(hit(line, col, false));
        }
    }
    // From the top again, up to and including the cursor line — so the only
    // match in the buffer is found even when the cursor sits on it.
    for line in 0..=from.line {
        if let Some(col) = buf.line(line).find(pattern) {
            return Some(hit(line, col, true));
        }
    }
    None
}

fn find_backward(buf: &Buffer, pattern: &str, from: Position) -> Option<Hit> {
    // A match counts when it *starts* before the cursor, even if it reaches
    // past it.
    if let Some(col) = buf.line(from.line)[..from.col].rfind(pattern) {
        return Some(hit(from.line, col, false));
    }
    for line in (0..from.line).rev() {
        if let Some(col) = buf.line(line).rfind(pattern) {
            return Some(hit(line, col, false));
        }
    }
    for line in (from.line..buf.len_lines()).rev() {
        if let Some(col) = buf.line(line).rfind(pattern) {
            return Some(hit(line, col, true));
        }
    }
    None
}

fn hit(line: usize, col: usize, wrapped: bool) -> Hit {
    Hit {
        pos: Position::new(line, col),
        wrapped,
    }
}

/// First match at or after byte `start`, in whole-line coordinates.
fn match_from(line: &str, start: usize, pattern: &str) -> Option<usize> {
    line.get(start..)?.find(pattern).map(|i| i + start)
}

/// The byte index just after the character at `col`.
fn after(line: &str, col: usize) -> usize {
    line[col..]
        .chars()
        .next()
        .map_or(col, |c| col + c.len_utf8())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(text: &str) -> Buffer {
        let mut buf = Buffer::default();
        buf.set_text(text);
        buf
    }

    #[test]
    fn a_forward_search_skips_the_match_it_starts_on() {
        let buf = buffer("foo foo foo");
        let first = find(&buf, "foo", Position::new(0, 0), true).unwrap();
        assert_eq!(first.pos, Position::new(0, 4));
        let second = find(&buf, "foo", first.pos, true).unwrap();
        assert_eq!(second.pos, Position::new(0, 8));
        assert!(!second.wrapped);
    }

    #[test]
    fn a_forward_search_continues_on_the_next_line() {
        let buf = buffer("one\ntwo\nthree");
        let hit = find(&buf, "ee", Position::new(0, 0), true).unwrap();
        assert_eq!(hit.pos, Position::new(2, 3));
    }

    #[test]
    fn a_forward_search_wraps_and_says_so() {
        let buf = buffer("target\nother");
        let hit = find(&buf, "target", Position::new(1, 0), true).unwrap();
        assert_eq!(hit.pos, Position::new(0, 0));
        assert!(hit.wrapped);
    }

    #[test]
    fn the_only_match_is_found_even_from_on_top_of_it() {
        let buf = buffer("lonely");
        let hit = find(&buf, "lonely", Position::new(0, 0), true).unwrap();
        assert_eq!(hit.pos, Position::new(0, 0));
        assert!(hit.wrapped, "it took a wrap to get back to it");
    }

    #[test]
    fn a_backward_search_takes_the_closest_match_before_the_cursor() {
        let buf = buffer("foo foo foo");
        let hit = find(&buf, "foo", Position::new(0, 8), false).unwrap();
        assert_eq!(hit.pos, Position::new(0, 4));
        assert!(!hit.wrapped);
    }

    #[test]
    fn a_backward_search_wraps_to_the_last_match() {
        let buf = buffer("a\nb\ntarget");
        let hit = find(&buf, "target", Position::new(0, 0), false).unwrap();
        assert_eq!(hit.pos, Position::new(2, 0));
        assert!(hit.wrapped);
    }

    #[test]
    fn a_pattern_that_is_nowhere_reports_nothing() {
        let buf = buffer("one\ntwo");
        assert_eq!(find(&buf, "three", Position::new(0, 0), true), None);
        assert_eq!(find(&buf, "three", Position::new(0, 0), false), None);
        assert_eq!(find(&buf, "", Position::new(0, 0), true), None);
    }

    #[test]
    fn match_columns_are_byte_offsets_past_multibyte_text() {
        let buf = buffer("ärger macht ärger");
        let hit = find(&buf, "ärger", Position::new(0, 0), true).unwrap();
        assert_eq!(hit.pos.col, 13, "each umlaut is two bytes");
    }
}
