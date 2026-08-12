//! Shared popup rendering utilities used by grouping and column config popups.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders, Clear};

use crate::ui::theme::Theme;

/// Render a centered popup frame and return the inner area.
pub fn render_popup_frame(
    frame: &mut Frame,
    area: Rect,
    t: &Theme,
    title: &str,
    popup_w: u16,
    popup_h: u16,
) -> Rect {
    let w = popup_w.min(area.width.saturating_sub(4));
    let h = popup_h.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let popup_area = Rect::new(x, y, w, h);

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.primary()))
        .title(Span::styled(
            format!(" {} ", title),
            Style::default().fg(t.accent()).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(t.bg()));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);
    inner
}

/// Calculate the number of rows needed for a wrapping hints bar.
pub fn hints_height(hints: &[(&str, &str)], inner_w: u16) -> u16 {
    let w = inner_w as usize;
    let mut lines = 1usize;
    let mut line_used = 1usize;
    for (key, desc) in hints {
        let hw = key.chars().count() + 1 + desc.chars().count() + 2;
        if line_used + hw > w && line_used > 1 {
            lines += 1;
            line_used = 1;
        }
        line_used += hw;
    }
    lines as u16
}

/// Render a wrapping hints bar at the bottom of the given inner area.
pub fn render_hints_bar(
    frame: &mut Frame,
    inner: Rect,
    t: &Theme,
    hints: &[(&str, &str)],
    hints_h: u16,
) {
    let buf = frame.buffer_mut();
    let hints_start_y = inner.bottom().saturating_sub(hints_h);
    let hints_bg = t.surface();

    for row in 0..hints_h {
        let hy = hints_start_y + row;
        for cx in inner.left()..inner.right() {
            if let Some(cell) = buf.cell_mut(Position::new(cx, hy)) {
                cell.set_char(' ');
                cell.set_style(Style::default().bg(hints_bg));
            }
        }
    }

    let mut hx = inner.left() + 1;
    let mut hy = hints_start_y;
    for (key, desc) in hints {
        let hint_w = (key.chars().count() + 1 + desc.chars().count() + 2) as u16;
        if hx + hint_w > inner.right() && hx > inner.left() + 1 && hy + 1 < inner.bottom() {
            hy += 1;
            hx = inner.left() + 1;
        }

        let key_style = Style::default().fg(t.accent()).bg(hints_bg);
        for ch in key.chars() {
            if hx >= inner.right() || hy >= inner.bottom() {
                break;
            }
            if let Some(cell) = buf.cell_mut(Position::new(hx, hy)) {
                cell.set_char(ch);
                cell.set_style(key_style);
            }
            hx += 1;
        }
        let desc_style = Style::default().fg(t.text_med()).bg(hints_bg);
        let desc_text = format!(" {}  ", desc);
        for ch in desc_text.chars() {
            if hx >= inner.right() || hy >= inner.bottom() {
                break;
            }
            if let Some(cell) = buf.cell_mut(Position::new(hx, hy)) {
                cell.set_char(ch);
                cell.set_style(desc_style);
            }
            hx += 1;
        }
    }
}
