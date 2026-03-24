use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
};

use crate::app::App;
use crate::config::TasksAction;
use crate::tabs::TasksForm;

pub struct TasksFormPane<'a> {
    app: &'a App,
}

impl<'a> TasksFormPane<'a> {
    pub fn new(app: &'a App) -> Self { Self { app } }
}

impl Widget for TasksFormPane<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let t  = &self.app.theme;
        let ts = &self.app.tasks_state;
        let kb = &self.app.keybindings.tasks;

        let Some(form) = ts.active_form else { return; };

        let (title, icon) = match form {
            TasksForm::Filter => (" 󰈲  Filter Tasks ", "󰈲"),
            TasksForm::Add    => ("   Add Task ",     ""),
            TasksForm::Delete => (" 󰆴  Delete Task ",  "󰆴"),
        };

        // Accent colour differs by form type to give quick visual context
        let accent = match form {
            TasksForm::Filter => t.primary(),
            TasksForm::Add    => t.success(),
            TasksForm::Delete => t.error(),
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(accent))
            .title(Span::styled(title, Style::default().fg(accent).add_modifier(Modifier::BOLD)))
            .style(Style::default().bg(t.surface_2()));

        let inner = block.inner(area);
        block.render(area, buf);

        // Placeholder content centred in the pane
        let v = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Fill(1), Constraint::Length(3), Constraint::Fill(1)])
            .split(inner);

        let form_name = match form {
            TasksForm::Filter => "Filter form",
            TasksForm::Add    => "Add form",
            TasksForm::Delete => "Delete form",
        };

        let close_hint = format!("{} to close", kb.label(&TasksAction::FormClose));

        let lines = vec![
            Line::from(vec![
                Span::styled(format!("{}  ", icon), Style::default().fg(t.text_dim())),
                Span::styled(
                    format!("{} — not yet implemented", form_name),
                    Style::default().fg(t.text_dim()).add_modifier(Modifier::ITALIC),
                ),
            ])
            .alignment(Alignment::Center),
            Line::from(""),
            Line::from(vec![Span::styled(
                close_hint,
                Style::default().fg(t.text_dim()),
            )])
            .alignment(Alignment::Center),
        ];

        Paragraph::new(lines).render(v[1], buf);
    }
}
