use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
};

use crate::app::App;
use crate::tabs::{LoadState, TasksView};
use crate::ui::tasks::{list_view, tree_view};

pub struct TasksViewPane<'a> {
    app: &'a App,
}

impl<'a> TasksViewPane<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for TasksViewPane<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let t = &self.app.theme;
        let ts = &self.app.tasks_state;

        let title = match ts.active_view {
            TasksView::List => " 󰝖  Tasks — List ",
            TasksView::Tree => " 󰙅  Tasks — Tree ",
        };

        let task_count = ts.task_rows.len();
        let count_str = format!(
            " {} task{} ",
            task_count,
            if task_count == 1 { "" } else { "s" }
        );

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(t.primary_dim()))
            .title(Span::styled(
                title,
                Style::default().fg(t.primary()).add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Span::styled(&count_str, Style::default().fg(t.text_dim())))
            .style(Style::default().bg(t.bg()));

        let inner = block.inner(area);
        block.render(area, buf);

        // ── Loading / error states ──────────────────────────────────────
        match &ts.load_state {
            LoadState::Loading => {
                Paragraph::new(Line::from(Span::styled(
                    "  Loading tasks…",
                    Style::default().fg(t.text_dim()).add_modifier(Modifier::ITALIC),
                )))
                .render(inner, buf);
                return;
            }
            LoadState::Error(e) => {
                Paragraph::new(Line::from(Span::styled(
                    format!(" 󰅚  Error: {}", e),
                    Style::default().fg(t.error()),
                )))
                .render(inner, buf);
                return;
            }
            LoadState::Idle | LoadState::Loaded => {}
        }

        // ── Dispatch to the active view ─────────────────────────────────
        match ts.active_view {
            TasksView::List => list_view::render(inner, buf, self.app),
            TasksView::Tree => tree_view::render(inner, buf, self.app),
        }
    }
}
