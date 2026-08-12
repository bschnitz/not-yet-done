use super::style::{TextInputStyle, TextInputStyleType};
use crate::widgets::common::{PREFIX_LEN, render_prefixed_line, truncate_to_width};
use ratatui::{Frame, buffer::Buffer, layout::Rect, style::Style};
use unicode_width::UnicodeWidthChar;

/// All data required to render a single frame of a [`super::TextInput`].
///
/// Constructed in the component's `view` implementation and passed to
/// [`render`].  Keeping render logic as a free function makes it independently
/// testable without a full tuirealm component context.
pub(super) struct TextInputViewData<'a> {
    pub title: &'a str,
    pub value: &'a str,
    pub placeholder: &'a str,
    pub error: Option<&'a str>,
    /// Byte offset of the cursor within `value`.
    pub cursor_byte_offset: usize,
    pub focused: bool,
    /// Render one bullet per character instead of the value itself.
    pub masked: bool,
    /// Already-selected style — caller picks active vs inactive based on `focused`.
    pub style: &'a TextInputStyle,
}

/// The character standing in for every masked one.
const MASK_CHAR: char = '•';

/// The string actually drawn for a value: the value itself, or one
/// [`MASK_CHAR`] per character. Masking counts *characters*, not display
/// width — a wide or combining character in a password would otherwise leak
/// its width, and the bullets must line up with the cursor arithmetic below.
fn display_value(value: &str, masked: bool) -> String {
    if masked {
        MASK_CHAR.to_string().repeat(value.chars().count())
    } else {
        value.to_string()
    }
}

/// Renders the text input widget into `frame` at `area`.
///
/// Layout (three rows):
/// - Row 0: title
/// - Row 1: value, or placeholder text when the value is empty
/// - Row 2: error message (only when `data.error` is `Some` and `area.height > 2`)
///
/// When `data.focused` is `true` and there is already input in the field, the terminal cursor is
/// placed on row 1 at the current cursor position.
pub(super) fn render(frame: &mut Frame, area: Rect, data: &TextInputViewData<'_>) {
    let total_width = area.width;
    let text_width = total_width.saturating_sub(PREFIX_LEN) as usize;

    {
        let buf = frame.buffer_mut();

        // Row 0: title
        let title_style = data.style.resolved_style(TextInputStyleType::Title);
        render_prefixed_line(
            buf,
            area.x,
            area.y,
            total_width,
            data.title,
            text_width,
            &data.style.prefix_color,
            &title_style,
            false,
        );

        // Row 1: value or placeholder
        let shown = display_value(data.value, data.masked);
        let input_text = if data.value.is_empty() {
            data.placeholder
        } else {
            shown.as_str()
        };
        let input_style = data.style.resolved_style(TextInputStyleType::Input);
        let active_style = if data.value.is_empty() {
            match data.style.placeholder_color {
                Some(c) => input_style.fg(c),
                None => input_style,
            }
        } else {
            input_style
        };
        render_prefixed_line(
            buf,
            area.x,
            area.y + 1,
            total_width,
            input_text,
            text_width,
            &data.style.prefix_color,
            &active_style,
            false,
        );

        // Row 2: error (only when area is tall enough)
        if area.height > 2 {
            let error_text = match data.error {
                Some(e) => format!("  ⚠ {}", e),
                None => String::new(),
            };
            let error_style = data.style.resolved_style(TextInputStyleType::Error);
            render_error_line(
                buf,
                area.x,
                area.y + 2,
                total_width,
                &error_text,
                &error_style,
            );
        }
    }

    // Place terminal cursor on the input row when focused.
    if data.focused && !data.value.is_empty() {
        // Masked: every character became exactly one single-width bullet, so
        // the column is the character count — using the real widths would put
        // the cursor where the *unmasked* text would have ended.
        let col: u16 = if data.masked {
            data.value[..data.cursor_byte_offset].chars().count() as u16
        } else {
            data.value[..data.cursor_byte_offset]
                .chars()
                .map(|c| c.width().unwrap_or(1) as u16)
                .sum()
        };
        let cursor_x = (area.x + PREFIX_LEN + col).min(area.x + area.width.saturating_sub(1));
        frame.set_cursor_position((cursor_x, area.y + 1));
    }
}

#[cfg(test)]
mod tests {
    use super::display_value;

    #[test]
    fn unmasked_value_is_shown_as_is() {
        assert_eq!(display_value("hunter2", false), "hunter2");
    }

    /// One bullet per *character*: a wide or non-ASCII character must not
    /// betray its width, and the count is what the cursor arithmetic assumes.
    #[test]
    fn masking_is_one_bullet_per_character() {
        assert_eq!(display_value("abc", true), "•••");
        assert_eq!(display_value("äöü", true), "•••");
        assert_eq!(display_value("日本", true), "••");
        assert_eq!(display_value("", true), "");
    }
}

/// Renders a plain line without a prefix — used for the error row.
fn render_error_line(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    total_width: u16,
    text: &str,
    line_style: &Style,
) {
    if let Some(bg) = line_style.bg {
        for dx in 0..total_width {
            if let Some(cell) = buf.cell_mut((x + dx, y)) {
                cell.set_bg(bg);
            }
        }
    }
    let mut px = x;
    for ch in truncate_to_width(text, total_width as usize).chars() {
        if let Some(cell) = buf.cell_mut((px, y)) {
            let mut s = Style::default();
            if let Some(fg) = line_style.fg {
                s = s.fg(fg);
            }
            if let Some(bg) = line_style.bg {
                s = s.bg(bg);
            }
            cell.set_char(ch);
            cell.set_style(s);
        }
        px += ch.width().unwrap_or(1) as u16;
    }
}
