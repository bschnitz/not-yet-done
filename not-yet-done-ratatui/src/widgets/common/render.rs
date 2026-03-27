use ratatui::{buffer::Buffer, style::{Color, Style}};
use super::style::LineStyle;

pub const PREFIX: &str = "▍ ";
pub const PREFIX_CURSOR: &str = "▶ ";
pub const PREFIX_LEN: u16 = 2;

/// Rendert eine Zeile mit Prefix (`▍ ` oder `▶ `) und anschließendem Text.
///
/// - `highlight_cursor`: wenn `true`, wird `▶ ` statt `▍ ` als Prefix verwendet.
/// - `prefix_color`: optionale Farbe für das Prefix-Zeichen.
/// - `line_style`: Farben für Text und Hintergrund der gesamten Zeile.
pub fn render_prefixed_line(
    buf: &mut Buffer,
    x: u16, y: u16,
    total_width: u16,
    text: &str,
    text_width: usize,
    prefix_color: &Option<Color>,
    line_style: &LineStyle,
    highlight_cursor: bool,
) {
    // Hintergrund über gesamte Breite
    if let Some(bg) = line_style.bg {
        for dx in 0..total_width {
            if let Some(cell) = buf.cell_mut((x + dx, y)) {
                cell.set_bg(bg);
            }
        }
    }

    // Prefix
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

    // Text
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
        px += 1;
    }
}

/// Kürzt `s` auf maximal `max_chars` Unicode-Zeichen (gibt einen Byte-Slice zurück).
pub fn truncate_to_width(s: &str, max_chars: usize) -> &str {
    let mut count = 0;
    let mut byte_pos = 0;
    for ch in s.chars() {
        if count >= max_chars {
            break;
        }
        count += 1;
        byte_pos += ch.len_utf8();
    }
    &s[..byte_pos]
}
