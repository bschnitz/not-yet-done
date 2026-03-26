use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use crate::app::App;
use crate::ui::tasks::view_helpers::{
    description_width, format_local_date, render_centered_msg, render_scroll_indicator,
    status_display, truncate_str,
};
use not_yet_done_core::entity::task::Model as Task;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn render(inner: Rect, buf: &mut Buffer, app: &App) {
    let ts = &app.tasks_state;

    if ts.task_rows.is_empty() {
        render_centered_msg(inner, buf, "󰄰  No tasks found.", app);
        return;
    }

    let header_height: u16 = 1;
    let rows_area = Rect {
        y: inner.y + header_height,
        height: inner.height.saturating_sub(header_height),
        ..inner
    };
    let header_area = Rect { height: header_height, ..inner };

    render_list_header(header_area, buf, &app.theme);

    let visible_rows = rows_area.height as usize;
    let scroll = ts.scroll_offset;

    for (i, task) in ts.task_rows.iter().skip(scroll).take(visible_rows).enumerate() {
        let row_area = Rect { y: rows_area.y + i as u16, height: 1, ..rows_area };
        render_list_row(row_area, buf, task, (scroll + i) == ts.selected_row, &app.theme);
    }

    render_scroll_indicator(inner, ts.task_rows.len(), visible_rows, ts.selected_row, buf, &app.theme);
}

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

fn render_list_header(area: Rect, buf: &mut Buffer, t: &crate::ui::theme::Theme) {
    let line = Line::from(vec![
        Span::styled("St  ", Style::default().fg(t.text_dim()).add_modifier(Modifier::BOLD)),
        Span::styled(
            format!("{:<width$}", "Description", width = description_width(area.width) as usize),
            Style::default().fg(t.text_dim()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" Pri  ", Style::default().fg(t.text_dim()).add_modifier(Modifier::BOLD)),
        Span::styled("Created", Style::default().fg(t.text_dim()).add_modifier(Modifier::BOLD)),
    ]);
    Paragraph::new(line)
        .style(Style::default().bg(t.surface()))
        .render(area, buf);
}

// ---------------------------------------------------------------------------
// Data row
// ---------------------------------------------------------------------------

fn render_list_row(
    area: Rect,
    buf: &mut Buffer,
    task: &Task,
    selected: bool,
    t: &crate::ui::theme::Theme,
) {
    let bg = if selected { t.surface_2() } else { t.bg() };
    let fg_desc = if selected { t.text_high() } else { t.text_med() };

    for x in area.left()..area.right() {
        if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(x, area.y)) {
            cell.set_bg(bg);
        }
    }

    let (status_icon, status_color) = status_display(&task.status, task.deleted, t);
    let priority_str = format!("{:>3} ", task.priority);
    let date_str = format_local_date(task.created_at);
    let sel_icon = if selected { "▶ " } else { "  " };
    let sel_fg = if selected { t.primary() } else { t.bg() };
    let desc_width = description_width(area.width) as usize;
    let desc_truncated = truncate_str(&task.description, desc_width);

    let line = Line::from(vec![
        Span::styled(sel_icon, Style::default().fg(sel_fg).bg(bg)),
        Span::styled(status_icon, Style::default().fg(status_color).bg(bg)),
        Span::styled(" ", Style::default().bg(bg)),
        Span::styled(
            format!("{:<width$}", desc_truncated, width = desc_width),
            Style::default().fg(fg_desc).bg(bg),
        ),
        Span::styled(
            format!("{:>4} ", priority_str.trim()),
            Style::default().fg(t.text_dim()).bg(bg),
        ),
        Span::styled(date_str, Style::default().fg(t.text_dim()).bg(bg)),
    ]);
    Paragraph::new(line).render(area, buf);
}
