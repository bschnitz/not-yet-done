//! ViewPane: renders the task view with borders, loading/error states,
//! and scroll indicator.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use ratatui::Frame;

use tuirealm::command::{Cmd, CmdResult};
use tuirealm::props::{Attribute, AttrValue, QueryResult};
use tuirealm::component::Component;
use tuirealm::state::State;

use crate::app::App;
use crate::tabs::{LoadState, Tab, TasksSubView};
use crate::ui::tasks::view_helpers::render_centered_msg;
use crate::ui::theme::Theme;
use std::sync::Arc;

pub struct ViewPaneComponent {
    #[allow(dead_code)]
    theme: Arc<Theme>,
}

impl ViewPaneComponent {
    pub fn new(theme: Arc<Theme>) -> Self {
        Self { theme }
    }
}

/// Free function for rendering the view pane. Avoids borrow conflicts
/// from calling `app.view_pane.render_with_app(app)`.
pub fn render_view(frame: &mut Frame, area: Rect, app: &mut App) {
    // Extract data we need before any mutable borrows.
    if app.active_tab == Tab::Trackings { return; }
    let title = match app.tasks_view.sub_view() {
        TasksSubView::List => " 󰝖  Tasks — List ",
        TasksSubView::Tree => " 󰙅  Tasks — Tree ",
    };
    let load_state = app.tasks_view.state.load_state.clone();
    let task_rows_empty = app.tasks_view.state.task_rows.is_empty();
    let row_count = app.task_table().row_count();
    let table_empty = app.task_table().is_empty();
    let selected = app.task_table().selected_row();
    let count_str = format!(
        " {} task{} ",
        row_count,
        if row_count == 1 { "" } else { "s" }
    );

    let t = &app.theme;
    let block = Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(Style::default().fg(t.primary_dim()))
        .title(Span::styled(
            title,
            Style::default().fg(t.primary()).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(&count_str, Style::default().fg(t.text_dim())))
        .style(Style::default().bg(t.bg()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    match &load_state {
        LoadState::Loading => {
            let buf = frame.buffer_mut();
            Paragraph::new(Line::from(Span::styled(
                "  Loading tasks…",
                Style::default().fg(t.text_dim()).add_modifier(Modifier::ITALIC),
            )))
            .render(inner, buf);
            return;
        }
        LoadState::Error(e) => {
            let buf = frame.buffer_mut();
            Paragraph::new(Line::from(Span::styled(
                format!(" 󰅚  Error: {}", e),
                Style::default().fg(t.error()),
            )))
            .render(inner, buf);
            return;
        }
        LoadState::Idle | LoadState::Loaded => {}
    }

    if table_empty {
        let msg = if task_rows_empty {
            "󰄰  No tasks found."
        } else {
            "󰄰  No tasks match the current filter."
        };
        render_centered_msg(inner, frame.buffer_mut(), msg, app);
    } else {
        // Go through TasksView so its sub-view's view() runs, which
        // paints the sort-mode overlay on top of the table.
        app.tasks_view.view(frame, inner);
    }

    let t = &app.theme;
    render_scroll_indicator(area, row_count, inner.height.saturating_sub(1) as usize, selected, frame.buffer_mut(), t);
}

fn render_scroll_indicator(
    border_area: Rect,
    total: usize,
    visible: usize,
    selected: usize,
    buf: &mut ratatui::buffer::Buffer,
    t: &Theme,
) {
    if total <= visible || visible == 0 { return; }
    let text = format!(" {}/{} ", selected + 1, total);
    let w = text.len() as u16;
    let x = border_area.right().saturating_sub(w + 1);
    let y = border_area.bottom().saturating_sub(1);
    if x >= border_area.left() {
        Paragraph::new(Span::styled(text, Style::default().fg(t.text_dim())))
            .render(Rect { x, y, width: w, height: 1 }, buf);
    }
}

impl Component for ViewPaneComponent {
    fn view(&mut self, _frame: &mut Frame, _area: Rect) {}
    fn query(&self, _attr: Attribute) -> Option<QueryResult<'_>> { None }
    fn attr(&mut self, _attr: Attribute, _value: AttrValue) {}
    fn state(&self) -> State { State::None }
    fn perform(&mut self, _cmd: Cmd) -> CmdResult { CmdResult::NoChange }
}
