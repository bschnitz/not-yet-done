use chrono::{DateTime, Local, Utc};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use not_yet_done_core::entity::task::TaskStatus;

use crate::app::App;

// ---------------------------------------------------------------------------
// Centered empty-state message
// ---------------------------------------------------------------------------

pub fn render_centered_msg(inner: Rect, buf: &mut ratatui::buffer::Buffer, msg: &str, app: &App) {
    let t = &app.theme;
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Fill(1), Constraint::Length(1), Constraint::Fill(1)])
        .split(inner);
    Paragraph::new(Line::from(Span::styled(
        msg,
        Style::default().fg(t.text_dim()).add_modifier(Modifier::ITALIC),
    )))
    .alignment(Alignment::Center)
    .render(v[1], buf);
}

// ---------------------------------------------------------------------------
// Scroll indicator (bottom-right corner)
// ---------------------------------------------------------------------------

pub fn render_scroll_indicator(
    inner: Rect,
    total: usize,
    visible_rows: usize,
    selected: usize,
    buf: &mut ratatui::buffer::Buffer,
    t: &crate::ui::theme::Theme,
) {
    if total <= visible_rows {
        return;
    }
    let indicator = format!(" {}/{} ", selected + 1, total);
    let x = inner.right().saturating_sub(indicator.len() as u16 + 1);
    let y = inner.bottom().saturating_sub(1);
    if y < inner.bottom() && x >= inner.left() {
        Paragraph::new(Span::styled(&indicator, Style::default().fg(t.text_dim())))
            .render(Rect { x, y, width: indicator.len() as u16, height: 1 }, buf);
    }
}

// ---------------------------------------------------------------------------
// Status icon + colour
// ---------------------------------------------------------------------------

pub fn status_display(
    status: &TaskStatus,
    deleted: bool,
    t: &crate::ui::theme::Theme,
) -> (&'static str, ratatui::style::Color) {
    if deleted {
        return ("󰆴 ", t.error());
    }
    match status {
        TaskStatus::Todo => ("󰄰 ", t.text_dim()),
        TaskStatus::InProgress => ("󰑐 ", t.accent()),
        TaskStatus::Done => ("󰄵 ", t.success()),
        TaskStatus::Cancelled => ("󰜺 ", t.text_dim()),
    }
}

// ---------------------------------------------------------------------------
// Layout helpers for the list view
// ---------------------------------------------------------------------------

pub fn description_width(total_width: u16) -> u16 {
    let fixed: u16 = 2 + 2 + 1 + 5 + 12;
    total_width.saturating_sub(fixed).max(10)
}

pub fn truncate_str(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = chars[..max_chars.saturating_sub(1)].iter().collect();
        format!("{}…", truncated)
    }
}

pub fn format_local_date(dt: DateTime<Utc>) -> String {
    let local: DateTime<Local> = dt.with_timezone(&Local);
    local.format("%Y-%m-%d").to_string()
}

// ---------------------------------------------------------------------------
// Highlight spans
// ---------------------------------------------------------------------------

/// Split `s` into styled [`Span`]s based on char-index `ranges`.
///
/// Characters inside a range get `hl_style`; all others get `normal_style`.
pub fn spans_with_highlights<'a>(
    s: &'a str,
    ranges: &[std::ops::Range<usize>],
    normal_style: Style,
    hl_style: Style,
) -> Vec<Span<'a>> {
    if ranges.is_empty() {
        return vec![Span::styled(s, normal_style)];
    }
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut spans = Vec::new();
    let mut cursor = 0usize;
    for range in ranges {
        let start = range.start.min(len);
        let end = range.end.min(len);
        if start > cursor {
            let text: String = chars[cursor..start].iter().collect();
            spans.push(Span::styled(text, normal_style));
        }
        if end > start {
            let text: String = chars[start..end].iter().collect();
            spans.push(Span::styled(text, hl_style));
        }
        cursor = end;
    }
    if cursor < len {
        let text: String = chars[cursor..].iter().collect();
        spans.push(Span::styled(text, normal_style));
    }
    spans
}
