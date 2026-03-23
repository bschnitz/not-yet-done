use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Widget},
};

use crate::app::App;

pub struct PlaceholderTab<'a> {
    app:   &'a App,
    label: &'static str,
    icon:  &'static str,
}

impl<'a> PlaceholderTab<'a> {
    pub fn new(app: &'a App, label: &'static str, icon: &'static str) -> Self {
        Self { app, label, icon }
    }
}

impl Widget for PlaceholderTab<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let t = &self.app.theme;

        Block::default()
            .style(Style::default().bg(t.bg()))
            .render(area, buf);

        let v_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Fill(1),
                Constraint::Length(3),
                Constraint::Fill(1),
            ])
            .split(area);

        let lines = vec![
            Line::from(vec![Span::styled(
                format!("{}  {}", self.icon, self.label),
                Style::default()
                    .fg(t.text_dim())
                    .add_modifier(Modifier::ITALIC),
            )])
            .alignment(Alignment::Center),
            Line::from(vec![Span::styled(
                "coming soon",
                Style::default().fg(t.text_dim()),
            )])
            .alignment(Alignment::Center),
        ];

        Paragraph::new(lines).render(v_chunks[1], buf);
    }
}
