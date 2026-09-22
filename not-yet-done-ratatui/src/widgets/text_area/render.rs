use crate::widgets::common::{PREFIX_LEN, render_prefixed_line};
use crate::widgets::text_input::{TextInputStyle, TextInputStyleType};
use ratatui::{Frame, layout::Rect, style::Style};
use unicode_width::UnicodeWidthChar;

/// All data required to render a single frame of a [`super::TextArea`].
///
/// Built in the component's `view` and handed to [`render`] as a plain struct,
/// so the layout can be exercised without a tuirealm context.
pub(super) struct TextAreaViewData<'a> {
    pub title: &'a str,
    pub lines: &'a [String],
    pub placeholder: &'a str,
    /// Cursor row (index into `lines`) and byte offset within that line.
    pub row: usize,
    pub col: usize,
    /// First visible row.
    pub scroll: usize,
    pub focused: bool,
    pub style: &'a TextInputStyle,
}

/// Drawn in the right-hand column when there is text above or below the box.
const MORE_ABOVE: char = '↑';
const MORE_BELOW: char = '↓';

/// Renders the text area into `frame` at `area`.
///
/// Layout: the title on row 0, then one row per visible line of text. When the
/// text is taller than the box, the rightmost column carries an arrow saying
/// which way the rest of it lies.
pub(super) fn render(frame: &mut Frame, area: Rect, data: &TextAreaViewData<'_>) {
    if area.height == 0 || area.width <= PREFIX_LEN {
        return;
    }
    let visible = area.height.saturating_sub(1) as usize;
    // The arrow column is only taken away while there is something to point
    // at, so a box that fits its text uses its full width.
    let scrollable = data.lines.len() > visible;
    let text_width = (area.width - PREFIX_LEN) as usize - usize::from(scrollable);

    let cursor_col = display_width(&data.lines[data.row][..data.col]);
    let h_offset = h_offset(cursor_col, text_width);
    let empty = data.lines.len() == 1 && data.lines[0].is_empty();

    {
        let buf = frame.buffer_mut();

        let title_style = data.style.resolved_style(TextInputStyleType::Title);
        render_prefixed_line(
            buf,
            area.x,
            area.y,
            area.width,
            data.title,
            text_width,
            &data.style.prefix_color,
            &title_style,
            false,
        );

        let input_style = data.style.resolved_style(TextInputStyleType::Input);
        let placeholder_style = match data.style.placeholder_color {
            Some(c) => input_style.fg(c),
            None => input_style,
        };

        for i in 0..visible {
            let idx = data.scroll + i;
            let (text, style) = match (empty, i, data.lines.get(idx)) {
                // An empty box says what it is for instead of standing blank.
                (true, 0, _) => (data.placeholder.to_string(), placeholder_style),
                (_, _, Some(line)) => (slice_from_column(line, h_offset, text_width), input_style),
                // Past the last line: still part of the box, so it keeps the
                // gutter and the background.
                (_, _, None) => (String::new(), input_style),
            };
            render_prefixed_line(
                buf,
                area.x,
                area.y + 1 + i as u16,
                area.width,
                &text,
                text_width,
                &data.style.prefix_color,
                &style,
                false,
            );
        }

        if scrollable {
            let x = area.x + area.width - 1;
            if data.scroll > 0 {
                set_marker(buf, x, area.y + 1, MORE_ABOVE, &input_style);
            }
            if data.scroll + visible < data.lines.len() {
                set_marker(buf, x, area.y + visible as u16, MORE_BELOW, &input_style);
            }
        }
    }

    if data.focused {
        let screen_row = data.row.saturating_sub(data.scroll).min(visible - 1) as u16;
        let col = cursor_col.saturating_sub(h_offset).min(text_width) as u16;
        frame.set_cursor_position((area.x + PREFIX_LEN + col, area.y + 1 + screen_row));
    }
}

fn set_marker(buf: &mut ratatui::buffer::Buffer, x: u16, y: u16, ch: char, style: &Style) {
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_char(ch);
        if let Some(fg) = style.fg {
            cell.set_fg(fg);
        }
        if let Some(bg) = style.bg {
            cell.set_bg(bg);
        }
    }
}

fn display_width(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(1)).sum()
}

/// How many display columns the box is scrolled to the right.
///
/// Zero while the cursor is still inside the first screenful — which is the
/// normal case — and otherwise just enough to keep the cursor on the last
/// column. Every row is drawn at the same offset: a line that scrolled
/// independently of its neighbours would make the text unreadable.
fn h_offset(cursor_col: usize, text_width: usize) -> usize {
    if text_width == 0 {
        return 0;
    }
    (cursor_col + 1).saturating_sub(text_width)
}

/// The part of `s` starting `from` display columns in, cut to `width` columns.
///
/// A wide character straddling the left edge is dropped rather than halved —
/// half a character is not a character.
fn slice_from_column(s: &str, from: usize, width: usize) -> String {
    let mut col = 0usize;
    let mut start = s.len();
    for (i, c) in s.char_indices() {
        if col >= from {
            start = i;
            break;
        }
        col += c.width().unwrap_or(1);
    }
    cut_to_width(&s[start..], width)
}

/// Cuts `s` down to `width` display columns without an ellipsis: the box has a
/// right edge, and a text that runs past it is not an abbreviation of itself —
/// the cursor and the horizontal offset already say there is more.
fn cut_to_width(s: &str, width: usize) -> String {
    let mut col = 0usize;
    let mut end = 0usize;
    for (i, c) in s.char_indices() {
        let w = c.width().unwrap_or(1);
        if col + w > width {
            break;
        }
        col += w;
        end = i + c.len_utf8();
    }
    s[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::{h_offset, slice_from_column};

    /// Nothing scrolls while the cursor is inside the box; beyond it the
    /// offset is exactly enough to keep the cursor on the last column.
    #[test]
    fn the_box_scrolls_only_once_the_cursor_leaves_it() {
        assert_eq!(h_offset(0, 10), 0);
        assert_eq!(h_offset(9, 10), 0);
        assert_eq!(h_offset(10, 10), 1);
        assert_eq!(h_offset(25, 10), 16);
    }

    #[test]
    fn a_slice_starts_at_a_column_and_stops_at_a_width() {
        assert_eq!(slice_from_column("abcdef", 0, 3), "abc");
        assert_eq!(slice_from_column("abcdef", 2, 3), "cde");
        assert_eq!(slice_from_column("abcdef", 9, 3), "");
    }

    /// Columns are display columns: a double-width character counts twice, and
    /// one that straddles the left edge is dropped rather than shown as half.
    #[test]
    fn a_slice_counts_display_columns() {
        assert_eq!(slice_from_column("日本語", 2, 2), "本");
        assert_eq!(slice_from_column("日本語", 1, 2), "本");
    }
}
