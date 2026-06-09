//! TasksFormPane component: filter/add/delete form with borders.
//!
//! Like ViewPane, this needs `&App` for filter state and keybindings,
//! so it provides `render_with_app()`.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Widget};
use ratatui::Frame;

use ratatui_form_widgets::{ChoiceOption, MultipleChoiceWidget, TextFieldWidget, ToggleFieldWidget};

use tuirealm::command::{Cmd, CmdResult};
use tuirealm::props::{Attribute, AttrValue, QueryResult};
use tuirealm::component::Component;
use tuirealm::state::State;

use crate::app::App;
use crate::config::CommonAction;
use crate::tabs::{FilterField, FilterState, StatusFilter, TasksForm};
use std::sync::Arc;
use crate::ui::theme::Theme;

pub struct FormPaneComponent {
    theme: Arc<Theme>,
}

impl FormPaneComponent {
    pub fn new(theme: Arc<Theme>) -> Self {
        Self { theme }
    }

    pub fn render_with_app(&self, frame: &mut Frame, area: Rect, app: &App) {
        let t = &self.theme;
        let ts = &app.tasks_view.state;
        let kb = &app.keybindings.common;

        let Some(form) = ts.active_form else { return };

        let (title, _icon) = match form {
            TasksForm::Filter => (" 󰈲  Search Tasks ", "󰈲"),
            TasksForm::Add => ("   Add Task ", ""),
        };

        let accent = match form {
            TasksForm::Filter => t.primary(),
            TasksForm::Add => t.success(),
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
                format!(" {} close  [ctrl+r] reset ", kb.label(&CommonAction::FormClose)),
                Style::default().fg(t.text_dim()),
            ))
            .style(Style::default().bg(t.form_bg()));

        let inner = block.inner(area);
        let buf = frame.buffer_mut();
        block.render(area, buf);

        match form {
            TasksForm::Filter => render_filter_form(inner, buf, &ts.filter, t),
            TasksForm::Add => render_placeholder(inner, buf, "Add form", "", app),
        }
    }
}

fn render_filter_form(
    area: Rect,
    buf: &mut ratatui::buffer::Buffer,
    filter: &FilterState,
    t: &Theme,
) {
    if area.height < 3 { return; }

    let tf_style = t.text_field_style();
    let mc_style = t.multiple_choice_style();
    let tg_style = t.toggle_field_style();

    let x = area.x;
    let width = area.width;

    macro_rules! text_field {
        ($y:expr, $field:expr, $value:expr, $error:expr, $placeholder:expr) => {{
            if $y + 2 > area.bottom() { return; }
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

    let mut y = area.y + 1;

    y = text_field!(y, FilterField::CreatedAfter, &filter.created_after_raw,
        filter.created_after_err.as_deref(), "e.g. last monday, 2 weeks ago, 2024-01-01");
    y += 1;
    y = text_field!(y, FilterField::CreatedBefore, &filter.created_before_raw,
        filter.created_before_err.as_deref(), "e.g. yesterday, today, 2024-06-30");
    y += 1;
    y = text_field!(y, FilterField::Description, &filter.description_like,
        None, "substring match");
    y += 1;

    if y + 2 > area.bottom() { return; }
    let status_options = status_options(&filter.status);
    y = MultipleChoiceWidget::new(
        FilterField::Status.label(), &status_options,
        filter.focused_field == FilterField::Status,
        filter.status_cursor, mc_style,
    ).render_and_next_y(Rect { x, y, width, height: 2 }, buf);
    y += 1;

    y = text_field!(y, FilterField::Priority, &filter.priority_min_raw,
        filter.priority_err.as_deref(), "integer, e.g. 1");
    y += 1;

    if y + 1 > area.bottom() { return; }
    ToggleFieldWidget::new(
        FilterField::ShowDeleted.label(), filter.show_deleted,
        filter.focused_field == FilterField::ShowDeleted, tg_style,
    ).render(Rect { x, y, width, height: 1 }, buf);

    let placeholder_y = y + 2;
    if placeholder_y + 1 < area.bottom() {
        Paragraph::new(Line::from(vec![Span::styled(
            "  ─── Coming soon: tag & project filters ───",
            Style::default().fg(t.text_dim()).add_modifier(Modifier::ITALIC),
        )])).render(Rect { x, y: placeholder_y, width, height: 1 }, buf);
    }
}

fn status_options(s: &StatusFilter) -> Vec<ChoiceOption<'_>> {
    vec![
        ChoiceOption::new("todo", s.todo),
        ChoiceOption::new("in_progress", s.in_progress),
        ChoiceOption::new("done", s.done),
        ChoiceOption::new("cancelled", s.cancelled),
    ]
}

fn render_placeholder(area: Rect, buf: &mut ratatui::buffer::Buffer, label: &str, icon: &str, app: &App) {
    let t = &app.theme;
    let kb = &app.keybindings.common;

    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Fill(1), Constraint::Length(3), Constraint::Fill(1)])
        .split(area);

    let close_hint = format!("{} to close", kb.label(&CommonAction::FormClose));
    let lines = vec![
        Line::from(vec![
            Span::styled(format!("{}  ", icon), Style::default().fg(t.text_dim())),
            Span::styled(
                format!("{} — not yet implemented", label),
                Style::default().fg(t.text_dim()).add_modifier(Modifier::ITALIC),
            ),
        ]).alignment(Alignment::Center),
        Line::from(""),
        Line::from(vec![Span::styled(close_hint, Style::default().fg(t.text_dim()))])
            .alignment(Alignment::Center),
    ];
    Paragraph::new(lines).render(v[1], buf);
}

impl Component for FormPaneComponent {
    fn view(&mut self, _frame: &mut Frame, _area: Rect) {
        // Use render_with_app() instead.
    }
    fn query(&self, _attr: Attribute) -> Option<QueryResult<'_>> { None }
    fn attr(&mut self, _attr: Attribute, _value: AttrValue) {}
    fn state(&self) -> State { State::None }
    fn perform(&mut self, _cmd: Cmd) -> CmdResult { CmdResult::NoChange }
}
