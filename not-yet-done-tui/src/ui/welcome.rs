use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::ui::theme::Theme;

pub struct WelcomeTab;

impl Widget for WelcomeTab {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Outer background
        Block::default()
            .style(Style::default().bg(Theme::BG))
            .render(area, buf);

        // Centre a card vertically
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

        // Card background
        let card = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Theme::PRIMARY_DIM))
            .style(Style::default().bg(Theme::SURFACE_2));

        let inner = card.inner(card_area);
        card.render(card_area, buf);

        // Content inside the card
        let lines = vec![
            Line::from(""),
            Line::from(vec![
                Span::styled(
                    "✓ not-yet-done",
                    Style::default()
                        .fg(Theme::PRIMARY)
                        .add_modifier(Modifier::BOLD),
                ),
            ])
            .alignment(Alignment::Center),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Your terminal task & time tracker",
                Style::default().fg(Theme::TEXT_HIGH),
            )])
            .alignment(Alignment::Center),
            Line::from(""),
            Line::from(vec![Span::styled(
                "─────────────────────────────────",
                Style::default().fg(Theme::TEXT_DIM),
            )])
            .alignment(Alignment::Center),
            Line::from(""),
            Line::from(vec![
                Span::styled("  Tasks      ", Style::default().fg(Theme::TEXT_MED)),
                Span::styled("→", Style::default().fg(Theme::ACCENT)),
                Span::styled("  manage your to-dos", Style::default().fg(Theme::TEXT_MED)),
            ])
            .alignment(Alignment::Left),
            Line::from(vec![
                Span::styled("  Trackings  ", Style::default().fg(Theme::TEXT_MED)),
                Span::styled("→", Style::default().fg(Theme::ACCENT)),
                Span::styled("  record time spent", Style::default().fg(Theme::TEXT_MED)),
            ])
            .alignment(Alignment::Left),
            Line::from(""),
        ];

        Paragraph::new(lines)
            .style(Style::default().bg(Theme::SURFACE_2))
            .render(inner, buf);
    }
}
