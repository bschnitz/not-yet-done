//! Text objects — the third thing an operator can take, after a motion and a
//! doubled operator.
//!
//! A motion says "from here to there"; a text object says "this word", "this
//! quoted string", "this block", wherever the cursor happens to sit inside it.
//! Both resolve to a byte range, which is why [`resolve`] hands back the same
//! `[from, to)` an operator already knows how to cut.
//!
//! Two deliberate simplifications, both because this widget edits messages
//! rather than source files: word objects stay on their line, and a quoted
//! string is found by pairing up the quote characters on the line rather than
//! by parsing escapes.

use crate::buffer::{Buffer, CharClass, Position, classify};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextObject {
    /// `w` — a run of word characters, of punctuation, or of blanks.
    Word,
    /// `W` — everything up to the next blank.
    BigWord,
    /// A quoted string, named by its quote character.
    Quote(char),
    /// A bracketed block, named by its *opening* character.
    Block(char),
}

impl TextObject {
    /// The object a key selects after `i` or `a`. Both halves of a bracket pair
    /// name the same block, as do vim's `b` and `B` shorthands.
    pub fn from_char(c: char) -> Option<Self> {
        match c {
            'w' => Some(Self::Word),
            'W' => Some(Self::BigWord),
            '"' | '\'' | '`' => Some(Self::Quote(c)),
            '(' | ')' | 'b' => Some(Self::Block('(')),
            '[' | ']' => Some(Self::Block('[')),
            '{' | '}' | 'B' => Some(Self::Block('{')),
            '<' | '>' => Some(Self::Block('<')),
            _ => None,
        }
    }
}

/// Resolve `obj` around the cursor to a byte range `[from, to)`.
///
/// `inner` is the `i` variant (`iw`), `false` the `a` variant (`aw`). `None`
/// means there is nothing there — an unclosed bracket, or an empty pair, where
/// vim beeps and the operator must not fire.
pub fn resolve(
    buf: &Buffer,
    obj: TextObject,
    inner: bool,
    count: usize,
) -> Option<(Position, Position)> {
    let cursor = buf.cursor();
    match obj {
        TextObject::Word | TextObject::BigWord => {
            let big = obj == TextObject::BigWord;
            let (start, end) = word_span(buf.line(cursor.line), cursor.col, big, inner, count)?;
            Some((
                Position::new(cursor.line, start),
                Position::new(cursor.line, end),
            ))
        }
        TextObject::Quote(q) => quote_span(buf, cursor, q, inner),
        TextObject::Block(open) => block_span(buf, cursor, open, inner),
    }
}

/// One maximal run of characters of the same class on a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Run {
    start: usize,
    end: usize,
    blank: bool,
}

/// Split a line into runs. `big` collapses words and punctuation into one
/// class, which is the only difference between `w` and `W`.
fn runs(line: &str, big: bool) -> Vec<Run> {
    let key = |c: char| match (big, classify(c)) {
        (_, CharClass::Blank) => CharClass::Blank,
        (true, _) => CharClass::Word,
        (_, class) => class,
    };
    let mut runs: Vec<Run> = Vec::new();
    let mut prev: Option<CharClass> = None;
    for (i, c) in line.char_indices() {
        let class = key(c);
        match runs.last_mut() {
            Some(run) if prev == Some(class) => run.end = i + c.len_utf8(),
            _ => runs.push(Run {
                start: i,
                end: i + c.len_utf8(),
                blank: class == CharClass::Blank,
            }),
        }
        prev = Some(class);
    }
    runs
}

fn word_span(
    line: &str,
    col: usize,
    big: bool,
    inner: bool,
    count: usize,
) -> Option<(usize, usize)> {
    let runs = runs(line, big);
    let i = runs.iter().position(|r| col >= r.start && col < r.end)?;
    let count = count.max(1);
    if inner {
        // `iw` is "this run", and a count simply takes the runs that follow —
        // which is why `2iw` covers a word and the blank after it.
        let last = (i + count - 1).min(runs.len() - 1);
        return Some((runs[i].start, runs[last].end));
    }
    Some(around_word_span(&runs, i, count))
}

/// `aw`: the word plus the blanks that follow it. When none follow, the blanks
/// *before* it come along instead — vim's rule, and the reason `daw` in the
/// middle of a sentence never leaves a double space behind.
fn around_word_span(runs: &[Run], i: usize, count: usize) -> (usize, usize) {
    let mut start = runs[i].start;
    let mut last = i;
    let mut trailing = false;
    for n in 0..count {
        if n > 0 {
            if last + 1 >= runs.len() {
                break;
            }
            last += 1;
        }
        // Starting on blanks, the word behind them belongs to the object.
        if runs[last].blank && last + 1 < runs.len() {
            last += 1;
        }
        trailing = last + 1 < runs.len() && runs[last + 1].blank;
        if trailing {
            last += 1;
        }
    }
    if !trailing && i > 0 && runs[i - 1].blank {
        start = runs[i - 1].start;
    }
    (start, runs[last].end)
}

/// Quotes are paired left to right across the line: the first with the second,
/// the third with the fourth. Escapes are not parsed — a compose pane does not
/// need it, and guessing wrong would be worse than the simple rule.
fn quote_span(
    buf: &Buffer,
    cursor: Position,
    quote: char,
    inner: bool,
) -> Option<(Position, Position)> {
    let line = buf.line(cursor.line);
    let marks: Vec<usize> = line
        .char_indices()
        .filter(|(_, c)| *c == quote)
        .map(|(i, _)| i)
        .collect();
    let width = quote.len_utf8();
    for pair in marks.chunks(2) {
        let (open, close) = (pair[0], *pair.get(1)?);
        // The pair the cursor is inside, or the next one to its right — vim
        // reaches forward on the line rather than giving up.
        if cursor.col <= close {
            let (from, to) = if inner {
                (open + width, close)
            } else {
                (open, close + width)
            };
            if from == to {
                return None;
            }
            return Some((
                Position::new(cursor.line, from),
                Position::new(cursor.line, to),
            ));
        }
    }
    None
}

fn closing(open: char) -> char {
    match open {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        _ => '>',
    }
}

/// Walk out to the enclosing bracket pair, counting nesting on the way. Blocks
/// may span lines: that is what `di{` is for.
fn block_span(
    buf: &Buffer,
    cursor: Position,
    open: char,
    inner: bool,
) -> Option<(Position, Position)> {
    let close = closing(open);
    let start = if buf.char_at(cursor) == Some(open) {
        cursor
    } else {
        let mut depth = 0usize;
        let mut pos = cursor;
        loop {
            pos = buf.prev_pos(pos)?;
            match buf.char_at(pos) {
                Some(c) if c == close => depth += 1,
                Some(c) if c == open => match depth {
                    0 => break pos,
                    _ => depth -= 1,
                },
                _ => {}
            }
        }
    };
    let end = {
        let mut depth = 0usize;
        let mut pos = start;
        loop {
            pos = buf.next_pos(pos)?;
            match buf.char_at(pos) {
                Some(c) if c == open => depth += 1,
                Some(c) if c == close => match depth {
                    0 => break pos,
                    _ => depth -= 1,
                },
                _ => {}
            }
        }
    };
    let (from, to) = if inner {
        (buf.next_pos(start)?, end)
    } else {
        (start, buf.next_pos(end)?)
    };
    // An empty pair has no inside; refusing here keeps `ci(` from reporting a
    // change that changed nothing.
    (from < to).then_some((from, to))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(text: &str, line: usize, col: usize) -> Buffer {
        let mut b = Buffer::from_text(text);
        b.set_cursor(Position::new(line, col));
        b
    }

    /// The text `obj` covers, for readable assertions.
    fn taken(text: &str, line: usize, col: usize, obj: TextObject, inner: bool) -> Option<String> {
        taken_n(text, line, col, obj, inner, 1)
    }

    fn taken_n(
        text: &str,
        line: usize,
        col: usize,
        obj: TextObject,
        inner: bool,
        count: usize,
    ) -> Option<String> {
        let b = buf(text, line, col);
        let (from, to) = resolve(&b, obj, inner, count)?;
        if from.line == to.line {
            return Some(b.line(from.line)[from.col..to.col].to_string());
        }
        let mut out = b.line(from.line)[from.col..].to_string();
        for l in from.line + 1..to.line {
            out.push('\n');
            out.push_str(b.line(l));
        }
        out.push('\n');
        out.push_str(&b.line(to.line)[..to.col]);
        Some(out)
    }

    #[test]
    fn iw_takes_the_word_under_the_cursor() {
        assert_eq!(
            taken("foo bar baz", 0, 5, TextObject::Word, true).as_deref(),
            Some("bar")
        );
    }

    #[test]
    fn iw_on_a_blank_run_takes_the_blanks() {
        assert_eq!(
            taken("foo   bar", 0, 4, TextObject::Word, true).as_deref(),
            Some("   ")
        );
    }

    #[test]
    fn iw_treats_punctuation_as_its_own_word() {
        assert_eq!(
            taken("foo.bar", 0, 3, TextObject::Word, true).as_deref(),
            Some(".")
        );
        assert_eq!(
            taken("foo.bar", 0, 3, TextObject::BigWord, true).as_deref(),
            Some("foo.bar"),
            "iW does not stop at punctuation"
        );
    }

    #[test]
    fn aw_takes_the_blanks_after_the_word() {
        assert_eq!(
            taken("foo bar baz", 0, 4, TextObject::Word, false).as_deref(),
            Some("bar ")
        );
    }

    #[test]
    fn aw_falls_back_to_the_blanks_before_the_last_word() {
        assert_eq!(
            taken("foo bar", 0, 5, TextObject::Word, false).as_deref(),
            Some(" bar"),
            "no trailing blank, so the leading one comes along"
        );
    }

    #[test]
    fn aw_starting_on_blanks_pulls_in_the_next_word() {
        assert_eq!(
            taken("foo  bar baz", 0, 3, TextObject::Word, false).as_deref(),
            Some("  bar ")
        );
    }

    #[test]
    fn a_count_extends_the_object() {
        assert_eq!(
            taken_n("one two three four", 0, 0, TextObject::Word, true, 3).as_deref(),
            Some("one two"),
            "2iw is a word and its blank, so 3iw reaches the next word"
        );
        assert_eq!(
            taken_n("one two three", 0, 0, TextObject::Word, false, 2).as_deref(),
            Some("one two ")
        );
    }

    #[test]
    fn a_count_past_the_line_end_stops_there() {
        assert_eq!(
            taken_n("one two", 0, 0, TextObject::Word, true, 99).as_deref(),
            Some("one two")
        );
    }

    #[test]
    fn a_word_object_on_an_empty_line_finds_nothing() {
        assert_eq!(taken("", 0, 0, TextObject::Word, true), None);
    }

    #[test]
    fn quotes_pair_up_across_the_line() {
        let text = "say \"hello\" now";
        assert_eq!(
            taken(text, 0, 6, TextObject::Quote('"'), true).as_deref(),
            Some("hello")
        );
        assert_eq!(
            taken(text, 0, 6, TextObject::Quote('"'), false).as_deref(),
            Some("\"hello\"")
        );
    }

    #[test]
    fn a_quote_object_reaches_forward_from_before_the_pair() {
        assert_eq!(
            taken("say \"hi\"", 0, 0, TextObject::Quote('"'), true).as_deref(),
            Some("hi")
        );
    }

    #[test]
    fn an_unclosed_or_empty_quote_finds_nothing() {
        assert_eq!(taken("say \"hi", 0, 5, TextObject::Quote('"'), true), None);
        assert_eq!(taken("\"\"", 0, 0, TextObject::Quote('"'), true), None);
    }

    #[test]
    fn brackets_nest() {
        let text = "f(a, g(b), c)";
        assert_eq!(
            taken(text, 0, 7, TextObject::Block('('), true).as_deref(),
            Some("b"),
            "the innermost pair wins"
        );
        assert_eq!(
            taken(text, 0, 3, TextObject::Block('('), true).as_deref(),
            Some("a, g(b), c")
        );
        assert_eq!(
            taken(text, 0, 3, TextObject::Block('('), false).as_deref(),
            Some("(a, g(b), c)")
        );
    }

    #[test]
    fn a_block_object_may_span_lines() {
        let text = "{\n  a\n}";
        assert_eq!(
            taken(text, 1, 2, TextObject::Block('{'), true).as_deref(),
            Some("\n  a\n")
        );
    }

    #[test]
    fn the_cursor_on_the_opening_bracket_takes_that_block() {
        assert_eq!(
            taken("[one]", 0, 0, TextObject::Block('['), true).as_deref(),
            Some("one")
        );
    }

    #[test]
    fn an_unbalanced_or_empty_block_finds_nothing() {
        assert_eq!(taken("f(a", 0, 2, TextObject::Block('('), true), None);
        assert_eq!(taken("a)", 0, 0, TextObject::Block('('), true), None);
        assert_eq!(taken("()", 0, 0, TextObject::Block('('), true), None);
    }

    #[test]
    fn both_halves_of_a_pair_name_the_same_object() {
        assert_eq!(TextObject::from_char(')'), Some(TextObject::Block('(')));
        assert_eq!(TextObject::from_char('b'), Some(TextObject::Block('(')));
        assert_eq!(TextObject::from_char('B'), Some(TextObject::Block('{')));
        assert_eq!(TextObject::from_char('z'), None);
    }
}
