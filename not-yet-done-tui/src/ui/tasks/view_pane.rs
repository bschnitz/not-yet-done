use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
};

use crate::app::App;
use crate::tabs::TasksView;

pub struct TasksViewPane<'a> {
    app: &'a App,
}

impl<'a> TasksViewPane<'a> {
    pub fn new(app: &'a App) -> Self { Self { app } }
}

impl Widget for TasksViewPane<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let t  = &self.app.theme;
        let ts = &self.app.tasks_state;

        let title = match ts.active_view {
            TasksView::List => " 󰝖  Tasks — List ",
            TasksView::Tree => " 󰙅  Tasks — Tree ",
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(t.primary_dim()))
            .title(Span::styled(title, Style::default().fg(t.primary()).add_modifier(Modifier::BOLD)))
            .style(Style::default().bg(t.bg()));

        let inner = block.inner(area);
        block.render(area, buf);

        // Placeholder content
        let v = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Fill(1), Constraint::Length(3), Constraint::Fill(1)])
            .split(inner);

        let mode_label = match ts.active_view {
            TasksView::List => "List view",
            TasksView::Tree => "Tree view",
        };

        let lines = vec![
            Line::from(vec![
                Span::styled("󰄰  ", Style::default().fg(t.text_dim())),
                Span::styled(
                    format!("{} — not yet implemented", mode_label),
                    Style::default().fg(t.text_dim()).add_modifier(Modifier::ITALIC),
                ),
            ])
            .alignment(Alignment::Center),
        ];

        Paragraph::new(lines).render(v[1], buf);
    }
}
