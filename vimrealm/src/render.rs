//! Drawing.
//!
//! The buffer holds *logical* lines; the screen shows *display* rows. Soft wrap
//! is the mapping between the two and lives only here — which is why `j`/`k`
//! stay logical, exactly as in vim with `nowrap`-independent line motions.
//!
//! # Two kinds of cursor
//!
//! Normal and visual mode operate on the character *under* the cursor, so the
//! cursor is a block covering exactly that character — painted into the text as
//! a styled cell. Insert mode inserts *between* characters, and command mode
//! does not point into the buffer at all; both therefore hand the job to the
//! terminal's own cursor via [`Frame::set_cursor_position`], the way every other
//! text input in this workspace does. That also gets the blinking bar the user's
//! terminal is configured for, which no painted cell can imitate.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::buffer::Position;
use crate::editor::VimEditor;
use crate::mode::Mode;
use crate::style::VimStyleType as S;

/// One display row: a byte range `[start, end)` of logical line `line`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Row {
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

pub(crate) fn render(editor: &mut VimEditor, frame: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let inner = match editor.title.is_empty() {
        true => area,
        false => {
            let block = Block::bordered()
                .title(editor.title.clone())
                .style(editor.style.resolved(S::Text));
            let inner = block.inner(area);
            frame.render_widget(block, area);
            inner
        }
    };
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let status_rows = u16::from(editor.show_status).min(inner.height.saturating_sub(1));
    let text_area = Rect {
        height: inner.height - status_rows,
        ..inner
    };
    let gutter = gutter_width(editor);
    let text_width = usize::from(text_area.width).saturating_sub(gutter);
    if text_width == 0 {
        return;
    }

    let rows = layout_rows(editor, text_width);
    let cursor_row = cursor_row(&rows, editor.buffer().cursor());
    editor.scroll = clamp_scroll(
        editor.scroll,
        cursor_row,
        rows.len(),
        text_area.height.into(),
    );

    let lines = visible_lines(editor, &rows, gutter, text_area.height.into());
    frame.render_widget(
        Paragraph::new(lines).style(editor.style.resolved(S::Text)),
        text_area,
    );

    // The terminal cursor: in the buffer while inserting, in the command line
    // while typing a command. In normal and visual mode it stays hidden,
    // because there the painted block *is* the cursor.
    let mut cursor = match editor.mode() {
        Mode::Insert => buffer_cursor_pos(editor, &rows, gutter, text_area),
        _ => None,
    };

    if status_rows > 0 {
        let area = Rect {
            y: inner.y + text_area.height,
            height: status_rows,
            ..inner
        };
        let status = status_line(editor);
        if let Some(col) = status.cursor_col {
            // The line is right-aligned, so its content starts that many
            // columns short of the right edge.
            let start = area.x + area.width.saturating_sub(status.width);
            cursor = Some((
                start
                    .saturating_add(col)
                    .min(area.right().saturating_sub(1)),
                area.y,
            ));
        }
        frame.render_widget(status.line, area);
    }

    if let Some(pos) = cursor.filter(|_| editor.focused) {
        frame.set_cursor_position(pos);
    }
}

/// Where the terminal cursor belongs on screen for the buffer's cursor, or
/// `None` when it scrolled out of the viewport.
fn buffer_cursor_pos(
    editor: &VimEditor,
    rows: &[Row],
    gutter: usize,
    text_area: Rect,
) -> Option<(u16, u16)> {
    let cursor = editor.buffer().cursor();
    let idx = cursor_row(rows, cursor);
    let row = rows.get(idx)?;
    let y = idx.checked_sub(editor.scroll)?;
    if y >= usize::from(text_area.height) {
        return None;
    }
    // The cursor may sit one past the last character (insert mode at line end),
    // which is a column of the row like any other.
    let line = editor.buffer().line(row.line);
    let col = width_of(&line[row.start..cursor.col.min(line.len())]);
    let x = text_area.x + u16::try_from(gutter + col).unwrap_or(u16::MAX);
    Some((
        x.min(text_area.right().saturating_sub(1)),
        text_area.y + u16::try_from(y).unwrap_or(u16::MAX),
    ))
}

/// Width reserved for the line-number gutter, including its trailing space.
fn gutter_width(editor: &VimEditor) -> usize {
    if !editor.line_numbers {
        return 0;
    }
    editor.buffer().len_lines().to_string().len() + 1
}

/// Break every logical line into display rows.
pub(crate) fn layout_rows(editor: &VimEditor, width: usize) -> Vec<Row> {
    let mut rows = Vec::new();
    for (line, text) in editor.buffer().lines().iter().enumerate() {
        for (start, end) in wrap(text, width) {
            rows.push(Row { line, start, end });
        }
    }
    rows
}

/// Byte ranges of `text` that each fit into `width` display columns. Always at
/// least one range, so an empty line still occupies a row.
pub(crate) fn wrap(text: &str, width: usize) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut used = 0;
    for (idx, c) in text.char_indices() {
        let w = char_width(c);
        if used + w > width && idx > start {
            ranges.push((start, idx));
            start = idx;
            used = 0;
        }
        used += w;
    }
    ranges.push((start, text.len()));
    ranges
}

/// Display width of one character; a tab counts as one column, because the
/// buffer stores it as a single byte and the cursor must stay in step.
fn char_width(c: char) -> usize {
    match c {
        '\t' => 1,
        _ => unicode_width::UnicodeWidthChar::width(c)
            .unwrap_or(0)
            .max(1),
    }
}

/// Which display row the cursor sits on.
pub(crate) fn cursor_row(rows: &[Row], cursor: Position) -> usize {
    let mut last = 0;
    for (i, row) in rows.iter().enumerate() {
        if row.line != cursor.line {
            continue;
        }
        last = i;
        if cursor.col < row.end {
            return i;
        }
    }
    // Past the end of the last row of that line — insert mode may sit there.
    last
}

/// Keep the cursor row inside the viewport, moving as little as possible.
pub(crate) fn clamp_scroll(scroll: usize, cursor_row: usize, rows: usize, height: usize) -> usize {
    let height = height.max(1);
    let max_scroll = rows.saturating_sub(height);
    let mut scroll = scroll.min(max_scroll);
    if cursor_row < scroll {
        scroll = cursor_row;
    } else if cursor_row >= scroll + height {
        scroll = cursor_row + 1 - height;
    }
    scroll
}

fn visible_lines<'a>(
    editor: &'a VimEditor,
    rows: &[Row],
    gutter: usize,
    height: usize,
) -> Vec<Line<'a>> {
    let cursor = editor.buffer().cursor();
    let cursor_row = cursor_row(rows, cursor);
    rows.iter()
        .enumerate()
        .skip(editor.scroll)
        .take(height)
        .map(|(idx, row)| {
            let mut spans = Vec::new();
            if gutter > 0 {
                spans.push(Span::styled(
                    gutter_label(row, rows, idx, gutter),
                    editor.style.resolved(S::Gutter),
                ));
            }
            let text = &editor.buffer().line(row.line)[row.start..row.end];
            let on_cursor_row = idx == cursor_row && editor.focused && paints_block(editor.mode());
            let last_row = idx + 1 == rows.len() || rows[idx + 1].line != row.line;
            spans.extend(row_spans(
                editor,
                text,
                row,
                on_cursor_row.then_some(cursor.col),
                last_row,
            ));
            Line::from(spans)
        })
        .collect()
}

/// Whether this mode's cursor is the painted block. Only the modes that act on
/// the character under the cursor draw one; see the module docs.
fn paints_block(mode: Mode) -> bool {
    matches!(mode, Mode::Normal | Mode::Visual | Mode::VisualLine)
}

/// The line number, but only on a logical line's first display row — a wrapped
/// continuation gets blanks, so the number always means "line".
fn gutter_label(row: &Row, rows: &[Row], idx: usize, width: usize) -> String {
    let is_continuation = idx > 0 && rows[idx - 1].line == row.line;
    if is_continuation {
        " ".repeat(width)
    } else {
        format!("{:>w$} ", row.line + 1, w = width - 1)
    }
}

/// Split the row's text into styled runs: the selection, with the block cursor
/// on top of it. Both are decided per character, because either can start or
/// end anywhere inside a wrapped row.
///
/// `cursor` is the cursor's byte column when it sits on this row, `last_row`
/// whether the row is the final one of its logical line.
fn row_spans<'a>(
    editor: &'a VimEditor,
    text: &'a str,
    row: &Row,
    cursor: Option<usize>,
    last_row: bool,
) -> Vec<Span<'a>> {
    let selection = selected_range(editor, row, last_row);
    let slot_at = |at: usize| -> Option<S> {
        if cursor == Some(at) {
            return Some(S::Cursor);
        }
        selection
            .filter(|(from, to)| at >= *from && at < *to)
            .map(|_| S::Selection)
    };

    let mut spans = Vec::new();
    let mut run: Option<(usize, Option<S>)> = None;
    for (idx, _) in text.char_indices() {
        let slot = slot_at(row.start + idx);
        match run {
            Some((start, previous)) if previous != slot => {
                spans.push(styled(editor, &text[start..idx], previous));
                run = Some((idx, slot));
            }
            Some(_) => {}
            None => run = Some((idx, slot)),
        }
    }
    if let Some((start, slot)) = run {
        spans.push(styled(editor, &text[start..], slot));
    }
    // One cell past the last character: nothing to style, so the cursor becomes
    // a highlighted blank and a selection shows the line break it swallowed.
    if let Some(slot) = slot_at(row.start + text.len()) {
        spans.push(styled(editor, " ", Some(slot)));
    }
    spans
}

fn styled<'a>(editor: &VimEditor, text: &'a str, slot: Option<S>) -> Span<'a> {
    match slot {
        Some(slot) => Span::styled(text, editor.style.resolved(slot)),
        None => Span::raw(text),
    }
}

/// The part of `row` the selection covers, as a byte range in its logical line.
/// `end` may be one past the line, which is the cell standing in for the line
/// break — vim highlights that too when the selection continues below.
fn selected_range(editor: &VimEditor, row: &Row, last_row: bool) -> Option<(usize, usize)> {
    let (from, to) = editor.selection()?;
    if row.line < from.line || row.line > to.line {
        return None;
    }
    let line = editor.buffer().line(row.line);
    let charwise = editor.mode() == Mode::Visual;
    let start = match charwise && row.line == from.line {
        true => from.col,
        false => 0,
    };
    let end = match charwise && row.line == to.line {
        true => after(line, to.col),
        false => line.len() + usize::from(last_row),
    };
    let start = start.max(row.start);
    let end = end.min(row.end + usize::from(last_row));
    (start < end).then_some((start, end))
}

/// The byte index just after the character at `col`.
fn after(line: &str, col: usize) -> usize {
    line[col..]
        .chars()
        .next()
        .map_or(col, |c| col + c.len_utf8())
}

/// The status line plus what the caller needs to place a cursor in it: the
/// rendered width, because the line is right-aligned, and the column the
/// command line's cursor sits at.
struct Status {
    line: Line<'static>,
    width: u16,
    cursor_col: Option<u16>,
}

fn status_line(editor: &VimEditor) -> Status {
    let command = editor.command_line();
    let left = match (&command, editor.message()) {
        (Some(cmd), _) => Span::styled(cmd.clone(), editor.style.resolved(S::CommandLine)),
        (None, Some(msg)) => Span::styled(msg.to_string(), editor.style.resolved(S::CommandLine)),
        (None, None) => Span::styled(mode_label(editor.mode()), editor.style.resolved(S::Mode)),
    };
    let cursor = editor.buffer().cursor();
    let right = format!(
        "{}{}:{} ",
        match editor.pending_label() {
            p if p.is_empty() => String::new(),
            p => format!("{p}  "),
        },
        cursor.line + 1,
        // Columns are reported 1-based in characters, the way vim does — byte
        // offsets would be surprising in a message written in German or French.
        editor.buffer().line(cursor.line)[..cursor.col]
            .chars()
            .count()
            + 1,
    );
    // Typing appends to the command line, so its cursor is always just past
    // what has been typed — including the prompt character.
    let cursor_col = command.map(|cmd| u16::try_from(width_of(&cmd)).unwrap_or(u16::MAX));
    let width =
        u16::try_from(width_of(left.content.as_ref()) + 1 + width_of(&right)).unwrap_or(u16::MAX);
    Status {
        line: Line::from(vec![
            left,
            Span::raw(" "),
            Span::styled(right, editor.style.resolved(S::Status)),
        ])
        .right_aligned(),
        width,
        cursor_col,
    }
}

/// Vim shows nothing in normal mode and `-- INSERT --` while inserting.
fn mode_label(mode: Mode) -> String {
    match mode {
        Mode::Normal => String::new(),
        other => format!("-- {} --", other.label()),
    }
}

/// Display width of a string — summed per character, so it counts a tab the
/// same single column [`wrap`] does and the cursor stays in step with the text.
fn width_of(text: &str) -> usize {
    text.chars().map(char_width).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_line_still_takes_a_row() {
        assert_eq!(wrap("", 10), vec![(0, 0)]);
    }

    #[test]
    fn a_short_line_is_one_row() {
        assert_eq!(wrap("abc", 10), vec![(0, 3)]);
        assert_eq!(width_of("abc"), 3);
    }

    #[test]
    fn a_long_line_wraps_at_the_width() {
        assert_eq!(wrap("abcdef", 2), vec![(0, 2), (2, 4), (4, 6)]);
    }

    #[test]
    fn wrapping_counts_display_columns_not_bytes() {
        // Each umlaut is two bytes but one column, so four of them fit a width
        // of four and stay in one row.
        assert_eq!(wrap("äöüä", 4), vec![(0, 8)]);
        assert_eq!(wrap("äöüä", 2), vec![(0, 4), (4, 8)]);
    }

    #[test]
    fn a_wide_character_takes_two_columns() {
        assert_eq!(wrap("漢字", 2), vec![(0, 3), (3, 6)]);
    }

    #[test]
    fn a_width_of_one_never_loops_forever() {
        assert_eq!(
            wrap("漢", 1),
            vec![(0, 3)],
            "an oversized char still gets a row"
        );
    }

    #[test]
    fn the_cursor_row_follows_the_wrap() {
        let rows = vec![
            Row {
                line: 0,
                start: 0,
                end: 2,
            },
            Row {
                line: 0,
                start: 2,
                end: 4,
            },
            Row {
                line: 1,
                start: 0,
                end: 0,
            },
        ];
        assert_eq!(cursor_row(&rows, Position::new(0, 1)), 0);
        assert_eq!(cursor_row(&rows, Position::new(0, 3)), 1);
        assert_eq!(
            cursor_row(&rows, Position::new(0, 4)),
            1,
            "past the end stays on the last row"
        );
        assert_eq!(cursor_row(&rows, Position::new(1, 0)), 2);
    }

    #[test]
    fn the_selection_and_the_cursor_become_their_own_spans() {
        let mut editor = VimEditor::default().with_text("abcdef");
        for c in ['v', 'l'] {
            editor.on_key(tuirealm::event::Key::Char(c).into());
        }
        let row = Row {
            line: 0,
            start: 0,
            end: 6,
        };
        assert_eq!(selected_range(&editor, &row, true), Some((0, 2)));
        let spans = row_spans(&editor, "abcdef", &row, Some(1), true);
        let text: Vec<_> = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(
            text,
            vec!["a", "b", "cdef"],
            "selection, cursor, then plain text"
        );
    }

    #[test]
    fn a_selection_reaching_below_highlights_the_line_break_cell() {
        let mut editor = VimEditor::default().with_text("ab\ncd");
        editor.on_key(tuirealm::event::Key::Char('V').into());
        let row = Row {
            line: 0,
            start: 0,
            end: 2,
        };
        assert_eq!(
            selected_range(&editor, &row, true),
            Some((0, 3)),
            "one cell past the text"
        );
        let spans = row_spans(&editor, "ab", &row, None, true);
        assert_eq!(spans.last().unwrap().content.as_ref(), " ");
    }

    #[test]
    fn only_normal_and_visual_mode_paint_a_block() {
        assert!(paints_block(Mode::Normal));
        assert!(paints_block(Mode::Visual));
        assert!(paints_block(Mode::VisualLine));
        assert!(
            !paints_block(Mode::Insert),
            "insert uses the terminal cursor"
        );
        assert!(
            !paints_block(Mode::Command),
            "the cursor is in the command line"
        );
    }

    #[test]
    fn inserting_leaves_the_text_unstyled() {
        let mut editor = VimEditor::default().with_text("abc");
        editor.on_key(tuirealm::event::Key::Char('i').into());
        let rows = layout_rows(&editor, 10);
        let lines = visible_lines(&editor, &rows, 0, 5);
        assert_eq!(
            lines[0].spans.len(),
            1,
            "no cursor span cut into the row: {:?}",
            lines[0]
        );
    }

    #[test]
    fn the_terminal_cursor_follows_the_character_it_inserts_before() {
        let mut editor = VimEditor::default().with_text("äbc\nxy");
        for c in ['j', 'l', 'i'] {
            editor.on_key(tuirealm::event::Key::Char(c).into());
        }
        let area = Rect {
            x: 4,
            y: 2,
            width: 20,
            height: 5,
        };
        let rows = layout_rows(&editor, 20);
        // Second line, second column: the umlaut in the line above is two bytes
        // but one column, and it is not this line's problem anyway.
        assert_eq!(buffer_cursor_pos(&editor, &rows, 0, area), Some((5, 3)));
        // Appending puts it one past the last character, where no cell exists.
        editor.on_key(tuirealm::event::Key::Esc.into());
        editor.on_key(tuirealm::event::Key::Char('A').into());
        assert_eq!(buffer_cursor_pos(&editor, &rows, 0, area), Some((6, 3)));
    }

    #[test]
    fn a_gutter_shifts_the_cursor_and_a_wide_character_widens_it() {
        let mut editor = VimEditor::default().with_text("漢字");
        for c in ['l', 'i'] {
            editor.on_key(tuirealm::event::Key::Char(c).into());
        }
        let area = Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 5,
        };
        let rows = layout_rows(&editor, 20);
        assert_eq!(
            buffer_cursor_pos(&editor, &rows, 3, area),
            Some((5, 0)),
            "3 gutter columns + the two the first character occupies"
        );
    }

    #[test]
    fn a_cursor_scrolled_out_of_the_viewport_is_not_placed() {
        let mut editor = VimEditor::default().with_text("a\nb\nc\nd");
        editor.on_key(tuirealm::event::Key::Char('i').into());
        let rows = layout_rows(&editor, 10);
        let area = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 2,
        };
        editor.scroll = 2;
        assert_eq!(buffer_cursor_pos(&editor, &rows, 0, area), None);
    }

    #[test]
    fn the_command_line_reports_the_column_past_what_was_typed() {
        let mut editor = VimEditor::default().with_text("abc");
        assert_eq!(
            status_line(&editor).cursor_col,
            None,
            "no command line open"
        );
        for c in [':', 'w', 'q'] {
            editor.on_key(tuirealm::event::Key::Char(c).into());
        }
        let status = status_line(&editor);
        assert_eq!(status.cursor_col, Some(3), "`:wq` is three columns wide");
        assert!(
            status.width > 3,
            "the line:col indicator is part of the width"
        );
    }

    /// Draw into a test terminal and report what the two cursors did: whether
    /// the terminal cursor is on and where, and whether the cell at `cell` was
    /// painted as a block.
    fn drawn(editor: &mut VimEditor, cell: (u16, u16)) -> (bool, (u16, u16), bool) {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(12, 4)).unwrap();
        terminal
            .draw(|frame| render(editor, frame, frame.area()))
            .unwrap();
        let backend = terminal.backend();
        let block = backend
            .buffer()
            .cell(cell)
            .map(|c| c.modifier.contains(ratatui::style::Modifier::REVERSED))
            .unwrap_or(false);
        let pos = backend.cursor_position();
        (backend.cursor_visible(), (pos.x, pos.y), block)
    }

    #[test]
    fn normal_mode_paints_the_block_and_hides_the_terminal_cursor() {
        let mut editor = VimEditor::default().with_text("abc").with_title("");
        editor.on_key(tuirealm::event::Key::Char('l').into());
        let (visible, _, block) = drawn(&mut editor, (1, 0));
        assert!(block, "the character under the cursor is reversed");
        assert!(!visible, "two cursors would show at once");
    }

    #[test]
    fn insert_mode_shows_the_terminal_cursor_instead_of_a_block() {
        let mut editor = VimEditor::default().with_text("abc").with_title("");
        for c in ['l', 'i'] {
            editor.on_key(tuirealm::event::Key::Char(c).into());
        }
        let (visible, pos, block) = drawn(&mut editor, (1, 0));
        assert!(visible, "insert mode uses the terminal's own cursor");
        assert_eq!(pos, (1, 0), "before the character it would insert at");
        assert!(!block, "and paints nothing over the text");
    }

    #[test]
    fn the_command_line_cursor_sits_in_the_status_row() {
        let mut editor = VimEditor::default().with_text("abc").with_title("");
        for c in [':', 'w'] {
            editor.on_key(tuirealm::event::Key::Char(c).into());
        }
        let (visible, pos, block) = drawn(&mut editor, (0, 0));
        assert!(visible);
        assert_eq!(pos.1, 3, "the status row of a four-row area");
        assert!(
            !block,
            "the buffer keeps no cursor while a command is typed"
        );
    }

    #[test]
    fn scrolling_only_moves_when_the_cursor_leaves_the_viewport() {
        assert_eq!(clamp_scroll(0, 3, 100, 10), 0, "still visible");
        assert_eq!(clamp_scroll(0, 12, 100, 10), 3, "scroll down just enough");
        assert_eq!(clamp_scroll(5, 2, 100, 10), 2, "scroll up to the cursor");
        assert_eq!(clamp_scroll(50, 0, 3, 10), 0, "never scroll past the end");
    }
}
