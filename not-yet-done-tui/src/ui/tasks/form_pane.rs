use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
};
use ratatui_form_widgets::{ChoiceOption, MultipleChoiceWidget, TextFieldWidget, ToggleFieldWidget};

use crate::app::App;
use crate::config::TasksAction;
use crate::tabs::{FilterField, FilterState, StatusFilter, TasksForm};

pub struct TasksFormPane<'a> {
    app: &'a App,
}

impl<'a> TasksFormPane<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for TasksFormPane<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let t = &self.app.theme;
        let ts = &self.app.tasks_state;
        let kb = &self.app.keybindings.tasks;

        let Some(form) = ts.active_form else {
            return;
        };

        let (title, _icon) = match form {
            TasksForm::Filter => (" 󰈲  Filter Tasks ", "󰈲"),
            TasksForm::Add => ("   Add Task ", ""),
            TasksForm::Delete => (" 󰆴  Delete Task ", "󰆴"),
        };

        let accent = match form {
            TasksForm::Filter => t.primary(),
            TasksForm::Add => t.success(),
            TasksForm::Delete => t.error(),
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(accent))
            .title(Span::styled(
                title,
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Span::styled(
                format!(
                    " {} close  [ctrl+r] reset ",
                    kb.label(&TasksAction::FormClose)
                ),
                Style::default().fg(t.text_dim()),
            ))
            .style(Style::default().bg(t.surface_2()));

        let inner = block.inner(area);
        block.render(area, buf);

        match form {
            TasksForm::Filter => render_filter_form(inner, buf, &ts.filter, t),
            TasksForm::Add => render_placeholder(inner, buf, "Add form", "", self.app),
            TasksForm::Delete => render_placeholder(inner, buf, "Delete form", "󰆴", self.app),
        }
    }
}

// ---------------------------------------------------------------------------
// Filter form
// ---------------------------------------------------------------------------

fn render_filter_form(
    area: Rect,
    buf: &mut Buffer,
    filter: &FilterState,
    t: &crate::ui::theme::Theme,
) {
    if area.height < 3 {
        return;
    }

    let tf_style = t.text_field_style();
    let mc_style = t.multiple_choice_style();
    let tg_style = t.toggle_field_style();

    let x = area.x;
    let width = area.width;

    macro_rules! text_field {
        ($y:expr, $field:expr, $value:expr, $error:expr, $placeholder:expr) => {{
            if $y + 2 > area.bottom() {
                return;
            }
            let focused = filter.focused_field == $field;
            let cursor = if focused { Some(filter.cursor_pos) } else { None };
            TextFieldWidget {
                label: $field.label(),
                value: $value,
                placeholder: $placeholder,
                error: $error,
                focused,
                cursor_pos: cursor,
                style: tf_style,
            }
            .render_and_next_y(Rect { x, y: $y, width, height: 2 }, buf)
        }};
    }

    let mut y = area.y;

    // ── Date fields ───────────────────────────────────────────────────────
    y = text_field!(
        y,
        FilterField::CreatedAfter,
        &filter.created_after_raw,
        filter.created_after_err.as_deref(),
        "e.g. last monday, 2 weeks ago, 2024-01-01"
    );
    y += 1;

    y = text_field!(
        y,
        FilterField::CreatedBefore,
        &filter.created_before_raw,
        filter.created_before_err.as_deref(),
        "e.g. yesterday, today, 2024-06-30"
    );
    y += 1;

    // ── Description ───────────────────────────────────────────────────────
    y = text_field!(
        y,
        FilterField::Description,
        &filter.description_like,
        None,
        "substring match"
    );
    y += 1;

    // ── Status ────────────────────────────────────────────────────────────
    if y + 2 > area.bottom() {
        return;
    }
    let status_options = status_options(&filter.status);
    y = MultipleChoiceWidget::new(
        FilterField::Status.label(),
        &status_options,
        filter.focused_field == FilterField::Status,
        filter.status_cursor,
        mc_style,
    )
    .render_and_next_y(Rect { x, y, width, height: 2 }, buf);
    y += 1;

    // ── Priority ──────────────────────────────────────────────────────────
    y = text_field!(
        y,
        FilterField::Priority,
        &filter.priority_min_raw,
        filter.priority_err.as_deref(),
        "integer, e.g. 1"
    );
    y += 1;

    // ── Show deleted ──────────────────────────────────────────────────────
    if y + 1 > area.bottom() {
        return;
    }
    ToggleFieldWidget::new(
        FilterField::ShowDeleted.label(),
        filter.show_deleted,
        filter.focused_field == FilterField::ShowDeleted,
        tg_style,
    )
    .render(Rect { x, y, width, height: 1 }, buf);

    // ── Coming-soon placeholder ───────────────────────────────────────────
    let placeholder_y = y + 2;
    if placeholder_y + 1 < area.bottom() {
        let sep = Line::from(vec![Span::styled(
            "  ─── Coming soon: tag & project filters ───",
            Style::default()
                .fg(t.text_dim())
                .add_modifier(Modifier::ITALIC),
        )]);
        Paragraph::new(sep).render(
            Rect { x, y: placeholder_y, width, height: 1 },
            buf,
        );
    }
}

/// Builds `ChoiceOption` slice for the status multi-select.
fn status_options(s: &StatusFilter) -> Vec<ChoiceOption<'_>> {
    vec![
        ChoiceOption::new("todo", s.todo),
        ChoiceOption::new("in_progress", s.in_progress),
        ChoiceOption::new("done", s.done),
        ChoiceOption::new("cancelled", s.cancelled),
    ]
}

// ---------------------------------------------------------------------------
// Placeholder for non-filter forms
// ---------------------------------------------------------------------------

fn render_placeholder(area: Rect, buf: &mut Buffer, label: &str, icon: &str, app: &App) {
    let t = &app.theme;
    let kb = &app.keybindings.tasks;

    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(3),
            Constraint::Fill(1),
        ])
        .split(area);

    let close_hint = format!("{} to close", kb.label(&TasksAction::FormClose));

    let lines = vec![
        Line::from(vec![
            Span::styled(format!("{}  ", icon), Style::default().fg(t.text_dim())),
            Span::styled(
                format!("{} — not yet implemented", label),
                Style::default()
                    .fg(t.text_dim())
                    .add_modifier(Modifier::ITALIC),
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
