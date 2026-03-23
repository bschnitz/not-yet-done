use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::app::App;

pub struct WelcomeTab<'a> {
    app: &'a App,
}

impl<'a> WelcomeTab<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for WelcomeTab<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let t = &self.app.theme;

        Block::default()
            .style(Style::default().bg(t.bg()))
            .render(area, buf);

        let v_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Fill(1),
                Constraint::Length(14),
                Constraint::Fill(1),
            ])
            .split(area);

        let h_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Fill(1),
                Constraint::Max(60),
                Constraint::Fill(1),
            ])
            .split(v_chunks[1]);

        let card_area = h_chunks[1];

        let card = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.primary_dim()))
            .style(Style::default().bg(t.surface_2()));

        let inner = card.inner(card_area);
        card.render(card_area, buf);

        let lines = vec![
            Line::from(""),
            Line::from(vec![Span::styled(
                "✓ not-yet-done",
                Style::default()
                    .fg(t.primary())
                    .add_modifier(Modifier::BOLD),
            )])
            .alignment(Alignment::Center),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Your terminal task & time tracker",
                Style::default().fg(t.text_high()),
            )])
            .alignment(Alignment::Center),
            Line::from(""),
            Line::from(vec![Span::styled(
                "─────────────────────────────────",
                Style::default().fg(t.text_dim()),
            )])
            .alignment(Alignment::Center),
            Line::from(""),
            Line::from(vec![
                Span::styled("  Tasks      ", Style::default().fg(t.text_med())),
                Span::styled("→", Style::default().fg(t.accent())),
                Span::styled("  manage your to-dos", Style::default().fg(t.text_med())),
            ]),
            Line::from(vec![
                Span::styled("  Trackings  ", Style::default().fg(t.text_med())),
                Span::styled("→", Style::default().fg(t.accent())),
                Span::styled("  record time spent", Style::default().fg(t.text_med())),
            ]),
            Line::from(""),
        ];

        Paragraph::new(lines)
            .style(Style::default().bg(t.surface_2()))
            .render(inner, buf);
    }
}
