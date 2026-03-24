use chrono::{DateTime, Local, Utc};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
};

use not_yet_done_core::entity::task::{Model as Task, TaskStatus};

use crate::app::App;
use crate::tabs::{LoadState, TasksView};

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

        // Count line in title area  
        let task_count = ts.task_rows.len();
        let count_str = format!(" {} task{} ", task_count, if task_count == 1 { "" } else { "s" });

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(t.primary_dim()))
            .title(Span::styled(title, Style::default().fg(t.primary()).add_modifier(Modifier::BOLD)))
            .title_bottom(Span::styled(&count_str, Style::default().fg(t.text_dim())))
            .style(Style::default().bg(t.bg()));

        let inner = block.inner(area);
        block.render(area, buf);

        // ── Status overlays ───────────────────────────────────────────────
        match &ts.load_state {
            LoadState::Loading => {
                let msg = Line::from(Span::styled(
                    "  Loading tasks…",
                    Style::default().fg(t.text_dim()).add_modifier(Modifier::ITALIC),
                ));
                Paragraph::new(msg).render(inner, buf);
                return;
            }
            LoadState::Error(e) => {
                let msg = Line::from(Span::styled(
                    format!(" 󰅚  Error: {}", e),
                    Style::default().fg(t.error()),
                ));
                Paragraph::new(msg).render(inner, buf);
                return;
            }
            LoadState::Idle | LoadState::Loaded => {}
        }

        if ts.task_rows.is_empty() {
            let v = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Fill(1), Constraint::Length(1), Constraint::Fill(1)])
                .split(inner);
            Paragraph::new(Line::from(Span::styled(
                "󰄰  No tasks match the current filter.",
                Style::default().fg(t.text_dim()).add_modifier(Modifier::ITALIC),
            )))
            .alignment(Alignment::Center)
            .render(v[1], buf);
            return;
        }

        // ── Header row ────────────────────────────────────────────────────
        let header_height: u16 = 1;
        let rows_area = Rect {
            y: inner.y + header_height,
            height: inner.height.saturating_sub(header_height),
            ..inner
        };
        let header_area = Rect { height: header_height, ..inner };

        render_header(header_area, buf, t);

        // ── Task rows ─────────────────────────────────────────────────────
        let visible_rows = rows_area.height as usize;
        let scroll = ts.scroll_offset;
        let tasks = &ts.task_rows;

        for (i, task) in tasks.iter().skip(scroll).take(visible_rows).enumerate() {
            let row_y = rows_area.y + i as u16;
            let row_area = Rect { y: row_y, height: 1, ..rows_area };
            let is_selected = (scroll + i) == ts.selected_row;
            render_task_row(row_area, buf, task, is_selected, t);
        }

        // ── Scroll indicator ──────────────────────────────────────────────
        if tasks.len() > visible_rows {
            let total = tasks.len();
            let indicator = format!(" {}/{} ", ts.selected_row + 1, total);
            // Draw at bottom-right of inner area
            let x = inner.right().saturating_sub(indicator.len() as u16 + 1);
            let y = inner.bottom().saturating_sub(1);
            if y < inner.bottom() && x >= inner.left() {
                let ind_area = Rect { x, y, width: indicator.len() as u16, height: 1 };
                Paragraph::new(Span::styled(
                    &indicator,
                    Style::default().fg(t.text_dim()),
                ))
                .render(ind_area, buf);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

fn render_header(area: Rect, buf: &mut Buffer, t: &crate::ui::theme::Theme) {
    // Columns: status icon (3) | description (fill) | priority (4) | created_at (22)
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
// Task row
// ---------------------------------------------------------------------------

fn render_task_row(
    area: Rect,
    buf: &mut Buffer,
    task: &Task,
    selected: bool,
    t: &crate::ui::theme::Theme,
) {
    let bg = if selected { t.surface_2() } else { t.bg() };
    let fg_desc = if selected { t.text_high() } else { t.text_med() };

    // Fill background
    for x in area.left()..area.right() {
        if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(x, area.y)) {
            cell.set_bg(bg);
        }
    }

    let (status_icon, status_color) = status_display(&task.status, task.deleted, t);
    let priority_str = format!("{:>3} ", task.priority);
    let date_str = format_local_date(task.created_at);

    // Selection indicator
    let sel_icon = if selected { "▶ " } else { "  " };
    let sel_fg   = if selected { t.primary() } else { t.bg() };

    let desc_width = description_width(area.width) as usize;
    let desc_truncated = truncate_str(&task.description, desc_width);

    let line = Line::from(vec![
        Span::styled(sel_icon,     Style::default().fg(sel_fg).bg(bg)),
        Span::styled(status_icon,  Style::default().fg(status_color).bg(bg)),
        Span::styled(" ",          Style::default().bg(bg)),
        Span::styled(
            format!("{:<width$}", desc_truncated, width = desc_width),
            Style::default().fg(fg_desc).bg(bg),
        ),
        Span::styled(
            format!("{:>4} ", priority_str.trim()),
            Style::default().fg(t.text_dim()).bg(bg),
        ),
        Span::styled(date_str,    Style::default().fg(t.text_dim()).bg(bg)),
    ]);

    Paragraph::new(line).render(area, buf);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn status_display(
    status: &TaskStatus,
    deleted: bool,
    t: &crate::ui::theme::Theme,
) -> (&'static str, ratatui::style::Color) {
    if deleted { return ("󰆴 ", t.error()); }
    match status {
        TaskStatus::Todo       => ("󰄰 ", t.text_dim()),
        TaskStatus::InProgress => ("󰑐 ", t.accent()),
        TaskStatus::Done       => ("󰄵 ", t.success()),
        TaskStatus::Cancelled  => ("󰜺 ", t.text_dim()),
    }
}

/// Calculate description column width given total row width.
fn description_width(total_width: u16) -> u16 {
    // sel(2) + icon(2) + space(1) + desc + pri(5) + date(12)
    let fixed: u16 = 2 + 2 + 1 + 5 + 12;
    total_width.saturating_sub(fixed).max(10)
}

fn truncate_str(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = chars[..max_chars.saturating_sub(1)].iter().collect();
        format!("{}…", truncated)
    }
}

fn format_local_date(dt: DateTime<Utc>) -> String {
    let local: DateTime<Local> = dt.with_timezone(&Local);
    local.format("%Y-%m-%d").to_string()
}
