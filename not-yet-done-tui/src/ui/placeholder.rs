use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Widget},
};

use crate::ui::theme::Theme;

pub struct PlaceholderTab {
    pub label: &'static str,
    pub icon:  &'static str,
}

impl Widget for PlaceholderTab {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Block::default()
            .style(Style::default().bg(Theme::BG))
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
            Line::from(vec![
                Span::styled(
                    format!("{}  {}", self.icon, self.label),
                    Style::default()
                        .fg(Theme::TEXT_DIM)
                        .add_modifier(Modifier::ITALIC),
                ),
            ])
            .alignment(Alignment::Center),
            Line::from(vec![Span::styled(
                "coming soon",
                Style::default().fg(Theme::TEXT_DIM),
            )])
            .alignment(Alignment::Center),
        ];

        Paragraph::new(lines).render(v_chunks[1], buf);
    }
}
