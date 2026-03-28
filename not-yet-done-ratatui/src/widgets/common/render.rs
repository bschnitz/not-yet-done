use ratatui::{
    buffer::Buffer,
    style::{Color, Style},
};
use unicode_width::UnicodeWidthChar;

pub const PREFIX: &str = "▍ ";
pub const PREFIX_CURSOR: &str = "▍▶";
pub const PREFIX_LEN: u16 = 2;

/// Renders a line with a prefix (`▍ ` or `▍▶`) followed by text.
///
/// - `highlight_cursor`: if `true`, uses `▍▶` instead of `▍ ` as the prefix.
/// - `prefix_color`: optional foreground colour for the prefix character.
/// - `line_style`: colours for the text and background of the whole line.
pub fn render_prefixed_line(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    total_width: u16,
    text: &str,
    text_width: usize,
    prefix_color: &Option<Color>,
    line_style: &Style,
    highlight_cursor: bool,
) {
    // Fill background across the full width.
    if let Some(bg) = line_style.bg {
        for dx in 0..total_width {
            if let Some(cell) = buf.cell_mut((x + dx, y)) {
                cell.set_bg(bg);
            }
        }
    }

    // Prefix.
    let prefix = if highlight_cursor { PREFIX_CURSOR } else { PREFIX };
    let mut px = x;
    for ch in prefix.chars() {
        if let Some(cell) = buf.cell_mut((px, y)) {
            let mut s = Style::default();
            if let Some(fg) = prefix_color {
                s = s.fg(*fg);
            }
            if let Some(bg) = line_style.bg {
                s = s.bg(bg);
            }
            cell.set_char(ch);
            cell.set_style(s);
        }
        px += 1;
    }

    // Text (already truncated to `text_width` display columns).
    for ch in truncate_to_width(text, text_width).chars() {
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

    // Padding: fill the rest of the line with spaces.
    while px < x + total_width {
        if let Some(cell) = buf.cell_mut((px, y)) {
            let mut s = Style::default();
            if let Some(fg) = line_style.fg {
                s = s.fg(fg);
            }
            if let Some(bg) = line_style.bg {
                s = s.bg(bg);
            }
            cell.set_char(' ');
            cell.set_style(s);
        }
        px += 1;
    }
}

/// Truncates `s` so that it fits within `max_display_width` terminal columns.
///
/// Measures display width via `unicode-width` (CJK wide characters count as 2).
/// If the string is truncated, a `…` (U+2026) is appended to signal the cut.
/// Returns the original string unchanged (as a `String`) when it already fits.
pub fn truncate_to_width(s: &str, max_display_width: usize) -> String {
    if max_display_width == 0 {
        return String::new();
    }

    // Measure the full display width of the string.
    let total_width: usize = s.chars().map(|c| c.width().unwrap_or(0)).sum();
    if total_width <= max_display_width {
        return s.to_string();
    }

    // Reserve one column for the ellipsis '…' (display width 1).
    let ellipsis = '…';
    let ellipsis_w = ellipsis.width().unwrap_or(1);
    let target = max_display_width.saturating_sub(ellipsis_w);

    let mut width = 0usize;
    let mut end = 0usize;
    for (byte_idx, ch) in s.char_indices() {
        let w = ch.width().unwrap_or(0);
        if width + w > target {
            break;
        }
        width += w;
        end = byte_idx + ch.len_utf8();
    }

    format!("{}…", &s[..end])
}
