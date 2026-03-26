use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use not_yet_done_core::entity::task::TaskStatus;
use not_yet_done_forest::TableRow;

use crate::app::App;
use crate::tabs::build_rendered_table;
use crate::ui::tasks::forest::{find_task_in_forest, LocalUuid};
use crate::ui::tasks::view_helpers::{
    render_centered_msg, render_scroll_indicator, spans_with_highlights, status_display,
};

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn render(inner: Rect, buf: &mut Buffer, app: &App) {
    let t = &app.theme;
    let ts = &app.tasks_state;

    let Some(forest) = &ts.forest else {
        render_centered_msg(inner, buf, "󰄰  No tasks loaded.", app);
        return;
    };

    let table = build_rendered_table(forest, &ts.tree_filter, inner.width as usize);

    if table.rows.is_empty() {
        render_centered_msg(inner, buf, "󰄰  No tasks match the current filter.", app);
        return;
    }

    // Header
    if let Some(header) = &table.header {
        let header_area = Rect { y: inner.y, height: 1, ..inner };
        render_table_header(header_area, buf, header, t);
    }

    // Data rows
    let rows_area = Rect {
        y: inner.y + 1,
        height: inner.height.saturating_sub(1),
        ..inner
    };
    let visible_rows = rows_area.height as usize;
    let scroll = ts.scroll_offset;
    let selected = ts.selected_row;

    for (i, row) in table.rows.iter().skip(scroll).take(visible_rows).enumerate() {
        let row_area = Rect { y: rows_area.y + i as u16, height: 1, ..rows_area };
        let is_sel = (scroll + i) == selected;
        let (status, deleted) = find_task_in_forest(forest, row.id.0)
            .map(|item| (item.status().clone(), item.deleted()))
            .unwrap_or((TaskStatus::Todo, false));
        let highlights = table.highlights.get(&row.id).map(|v| v.as_slice()).unwrap_or(&[]);
        render_tree_table_row(row_area, buf, row, highlights, is_sel, &status, deleted, t);
    }

    render_scroll_indicator(inner, table.rows.len(), visible_rows, selected, buf, t);
}

// ---------------------------------------------------------------------------
// Table header
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
// Data row
// ---------------------------------------------------------------------------

fn render_tree_table_row(
    area: Rect,
    buf: &mut Buffer,
    row: &TableRow<LocalUuid>,
    highlights: &[std::ops::Range<usize>],
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
            highlights,
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
