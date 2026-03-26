use chrono::{DateTime, Local, Utc};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
};

use not_yet_done_core::entity::task::{Model as Task, TaskStatus};
use not_yet_done_forest::TableRow;

use crate::app::App;
use crate::tabs::{build_table_rows, LoadState, TasksView};
use crate::ui::tasks::forest::{find_task_in_forest, LocalUuid};

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

        match ts.active_view {
            TasksView::List => render_list(inner, buf, self.app),
            TasksView::Tree => render_tree(inner, buf, self.app),
        }
    }
}

// ---------------------------------------------------------------------------
// List view (unchanged)
// ---------------------------------------------------------------------------

fn render_list(inner: Rect, buf: &mut Buffer, app: &App) {
    let t = &app.theme;
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

    render_list_header(header_area, buf, t);

    let visible_rows = rows_area.height as usize;
    let scroll = ts.scroll_offset;

    for (i, task) in ts.task_rows.iter().skip(scroll).take(visible_rows).enumerate() {
        let row_area = Rect { y: rows_area.y + i as u16, height: 1, ..rows_area };
        render_list_row(row_area, buf, task, (scroll + i) == ts.selected_row, t);
    }

    render_scroll_indicator(inner, ts.task_rows.len(), visible_rows, ts.selected_row, buf, t);
}

// ---------------------------------------------------------------------------
// Tree view
// ---------------------------------------------------------------------------

fn render_tree(inner: Rect, buf: &mut Buffer, app: &App) {
    let t = &app.theme;
    let ts = &app.tasks_state;

    let Some(forest) = &ts.forest else {
        render_centered_msg(inner, buf, "󰄰  No tasks found.", app);
        return;
    };

    // Build table rows for tree view
    let table_rows = build_table_rows(forest, &ts.tree_filter, inner.width as usize);
    
    if table_rows.is_empty() {
        render_centered_msg(inner, buf, "󰄰  No tasks match the current filter.", app);
        return;
    }

    // First row is the header, the rest are data.
    let Some((header, data_rows)) = table_rows.split_first() else {
        return;
    };

    // Draw header
    let header_area = Rect { y: inner.y, height: 1, ..inner };
    render_table_header(header_area, buf, header, t);

    // Draw data rows
    let rows_area = Rect {
        y: inner.y + 1,
        height: inner.height.saturating_sub(1),
        ..inner
    };
    let visible_rows = rows_area.height as usize;
    let scroll = ts.scroll_offset;
    let selected = ts.selected_row;

    // Korrekte Reihenfolge: erst skip, dann take
    // Wir brauchen keine explizite Typannotation mehr
    for (i, row) in data_rows.iter().skip(scroll).take(visible_rows).enumerate() {
        let row_area = Rect { y: rows_area.y + i as u16, height: 1, ..rows_area };
        let is_sel = (scroll + i) == selected;

        let (status, deleted) = find_task_in_forest(forest, row.id.0)
            .map(|item| (item.status().clone(), item.deleted()))
            .unwrap_or((TaskStatus::Todo, false));

        render_tree_table_row(row_area, buf, row, is_sel, &status, deleted, t);
    }

    render_scroll_indicator(inner, data_rows.len(), visible_rows, selected, buf, t);
}

// ---------------------------------------------------------------------------
// Tree table header
// ---------------------------------------------------------------------------

fn render_table_header(
    area: Rect,
    buf: &mut Buffer,
    row: &TableRow<LocalUuid>,
    t: &crate::ui::theme::Theme,
) {
    let spans: Vec<Span> = row
        .cells
        .iter()
        .enumerate()
        .flat_map(|(i, cell)| {
            let mut v = vec![];
            if i > 0 {
                v.push(Span::raw(" "));
            }
            v.push(Span::styled(
                cell.clone(),
                Style::default().fg(t.text_dim()).add_modifier(Modifier::BOLD),
            ));
            v
        })
        .collect();
    Paragraph::new(Line::from(spans))
        .style(Style::default().bg(t.surface()))
        .render(area, buf);
}

// ---------------------------------------------------------------------------
// Tree table data row
// ---------------------------------------------------------------------------

fn render_tree_table_row(
    area: Rect,
    buf: &mut Buffer,
    row: &TableRow<LocalUuid>,
    selected: bool,
    status: &TaskStatus,
    deleted: bool,
    t: &crate::ui::theme::Theme,
) {
    let bg = if selected { t.surface_2() } else { t.bg() };
    let fg = if selected { t.text_high() } else { t.text_med() };
    let hl_fg = t.accent();

    for x in area.left()..area.right() {
        if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(x, area.y)) {
            cell.set_bg(bg);
        }
    }

    let (status_icon, status_color) = status_display(status, deleted, t);
    let sel_icon = if selected { "▶ " } else { "  " };
    let sel_fg = if selected { t.primary() } else { t.bg() };

    let mut spans: Vec<Span> = vec![
        Span::styled(sel_icon, Style::default().fg(sel_fg).bg(bg)),
        Span::styled(status_icon, Style::default().fg(status_color).bg(bg)),
        Span::styled(" ", Style::default().bg(bg)),
    ];

    // Tree column (index 0) — with highlight ranges.
    if let Some(tree_cell) = row.cells.first() {

        spans.extend(spans_with_highlights(
            tree_cell,
            &row.highlight_ranges,
            Style::default().fg(fg).bg(bg),
            Style::default().fg(hl_fg).bg(bg).add_modifier(Modifier::BOLD),
        ));
    }

    // Remaining columns — plain, dimmer text.
    for cell in row.cells.iter().skip(1) {
        spans.push(Span::styled(" ", Style::default().bg(bg)));
        spans.push(Span::styled(
            cell.clone(),
            Style::default().fg(t.text_dim()).bg(bg),
        ));
    }

    Paragraph::new(Line::from(spans)).render(area, buf);
}

// ---------------------------------------------------------------------------
// spans_with_highlights
// ---------------------------------------------------------------------------

fn spans_with_highlights<'a>(
    s: &'a str,
    ranges: &[std::ops::Range<usize>],
    normal_style: Style,
    hl_style: Style,
) -> Vec<Span<'a>> {
    if ranges.is_empty() {
        return vec![Span::styled(s, normal_style)];
    }

    // ranges are already char indices
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut spans = Vec::new();
    let mut cursor = 0usize;

    for range in ranges {
        let start = range.start.min(len);
        let end = range.end.min(len);
        if start > cursor {
            let text: String = chars[cursor..start].iter().collect();
            spans.push(Span::styled(text, normal_style));
        }
        if end > start {
            let text: String = chars[start..end].iter().collect();
            spans.push(Span::styled(text, hl_style));
        }
        cursor = end;
    }

    if cursor < len {
        let text: String = chars[cursor..].iter().collect();
        spans.push(Span::styled(text, normal_style));
    }

    spans
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn render_centered_msg(inner: Rect, buf: &mut Buffer, msg: &str, app: &App) {
    let t = &app.theme;
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Fill(1), Constraint::Length(1), Constraint::Fill(1)])
        .split(inner);
    Paragraph::new(Line::from(Span::styled(
        msg,
        Style::default().fg(t.text_dim()).add_modifier(Modifier::ITALIC),
    )))
    .alignment(Alignment::Center)
    .render(v[1], buf);
}

fn render_scroll_indicator(
    inner: Rect,
    total: usize,
    visible_rows: usize,
    selected: usize,
    buf: &mut Buffer,
    t: &crate::ui::theme::Theme,
) {
    if total <= visible_rows {
        return;
    }
    let indicator = format!(" {}/{} ", selected + 1, total);
    let x = inner.right().saturating_sub(indicator.len() as u16 + 1);
    let y = inner.bottom().saturating_sub(1);
    if y < inner.bottom() && x >= inner.left() {
        Paragraph::new(Span::styled(&indicator, Style::default().fg(t.text_dim())))
            .render(Rect { x, y, width: indicator.len() as u16, height: 1 }, buf);
    }
}

// ---------------------------------------------------------------------------
// List header & row (unchanged)
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

fn description_width(total_width: u16) -> u16 {
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
