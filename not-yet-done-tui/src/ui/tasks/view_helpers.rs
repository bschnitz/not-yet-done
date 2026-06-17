use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

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
