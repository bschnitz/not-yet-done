//! Trackings tab rendering using the Table widget.

use ratatui::{
    layout::Rect,
    style::Style,
    Frame,
};

use tuirealm::component::Component;

use crate::app::App;
use crate::tabs::trackings_state::TrackingGrouping;
use crate::ui::tasks::view_helpers::render_centered_msg;

pub fn render(frame: &mut Frame, area: Rect, app: &mut App) {
    use ratatui::layout::{Constraint, Direction, Layout};

    // Query error bar (same as tasks tab).
    let error_height = app.query_error_bar.required_height(area.width);
    let mut constraints = Vec::new();
    if error_height > 0 {
        constraints.push(Constraint::Length(error_height));
    }
    constraints.push(Constraint::Fill(1));

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let mut idx = 0;
    if error_height > 0 {
        app.query_error_bar.view(frame, rows[idx]);
        idx += 1;
    }

    let content_area = rows[idx];
    render_table(frame, content_area, app);
}

fn render_table(frame: &mut Frame, area: Rect, app: &mut App) {
    let t = &app.theme;
    let ts = &app.trackings_view.state;

    // Show persistent error instead of the table.
    if let crate::tabs::LoadState::Error(ref err) = ts.load_state {
        render_error_view(frame, area, t, err);
        return;
    }

    if ts.filtered_indices.is_empty() {
        let msg = if ts.rows.is_empty() {
            "󰄰  No trackings found."
        } else {
            "󰄰  No trackings match the filter."
        };
        render_centered_msg(area, frame.buffer_mut(), msg, app);
        return;
    }

    // Render the persistent table component.
    app.trackings_view.table.view(frame, area);

    // Show grouping popup overlay if open.
    if let Some(cursor) = app.trackings_view.group_popup {
        render_grouping_popup(frame, area, &app.theme, cursor, app.trackings_view.state.grouping);
    }
}

fn render_grouping_popup(
    frame: &mut ratatui::Frame,
    area: Rect,
    t: &crate::ui::theme::Theme,
    cursor: usize,
    current: TrackingGrouping,
) {
    use ratatui::layout::Position;
    use ratatui::style::Modifier;
    use crate::ui::popup_utils::{render_popup_frame, render_hints_bar, hints_height};

    let options = TrackingGrouping::ALL;
    let hints: &[(&str, &str)] = &[("↑↓", "nav"), ("Spc", "select"), ("Esc", "close")];

    let popup_w = 28u16;
    let hh = hints_height(hints, popup_w.saturating_sub(2));
    let popup_h = options.len() as u16 + 2 + hh;

    let inner = render_popup_frame(frame, area, t, "Group by", popup_w, popup_h);
    if inner.height == 0 || inner.width == 0 { return; }

    let buf = frame.buffer_mut();
    let items_height = inner.height.saturating_sub(hh) as usize;

    for (i, variant) in options.iter().enumerate() {
        if i >= items_height { break; }
        let row_y = inner.y + i as u16;
        let is_cursor = i == cursor;
        let is_active = *variant == current;
        let bg = if is_cursor { t.surface_2() } else { t.bg() };

        for cx in inner.left()..inner.right() {
            if let Some(cell) = buf.cell_mut(Position::new(cx, row_y)) {
                cell.set_char(' ');
                cell.set_style(Style::default().bg(bg));
            }
        }

        let marker = if is_active { "● " } else { "  " };
        let mut cx = inner.left() + 1;
        for ch in marker.chars() {
            if cx >= inner.right() { break; }
            if let Some(cell) = buf.cell_mut(Position::new(cx, row_y)) {
                cell.set_char(ch);
                cell.set_style(Style::default().fg(t.accent()).bg(bg));
            }
            cx += 1;
        }

        let text = variant.label();
        let shortcut_pos = cx; // position of the first character
        let label_style = Style::default().fg(t.text_high()).bg(bg);
        let hl_style = Style::default().fg(t.text_high()).bg(bg)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
        for (ci, ch) in text.chars().enumerate() {
            if cx >= inner.right() { break; }
            let style = if ci == 0 { hl_style } else { label_style };
            if let Some(cell) = buf.cell_mut(Position::new(cx, row_y)) {
                cell.set_char(ch);
                cell.set_style(style);
            }
            cx += 1;
        }
        let _ = shortcut_pos; // suppress unused warning
    }

    render_hints_bar(frame, inner, t, hints, hh);
}

fn render_error_view(
    frame: &mut ratatui::Frame,
    area: Rect,
    t: &crate::ui::theme::Theme,
    err: &str,
) {
    use ratatui::layout::Position;

    let buf = frame.buffer_mut();
    // Fill background.
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                cell.set_char(' ');
                cell.set_style(Style::default().bg(t.bg()));
            }
        }
    }

    // Show error text, word-wrapped.
    let prefix = "Error: ";
    let full = format!("{prefix}{err}");
    let w = area.width as usize;
    let mut y = area.top() + 1;
    let mut remaining = full.as_str();
    while !remaining.is_empty() && y < area.bottom() {
        let line: String = remaining.chars().take(w.saturating_sub(2)).collect();
        let taken = line.len();
        let mut x = area.left() + 1;
        for ch in line.chars() {
            if x >= area.right() { break; }
            if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                cell.set_char(ch);
                cell.set_style(Style::default().fg(t.error()).bg(t.bg()));
            }
            x += 1;
        }
        remaining = &remaining[taken..];
        y += 1;
    }

    // Hint at bottom.
    let hint = "Press y to copy error to clipboard";
    if y + 1 < area.bottom() {
        y += 1;
        let mut x = area.left() + 1;
        for ch in hint.chars() {
            if x >= area.right() { break; }
            if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                cell.set_char(ch);
                cell.set_style(Style::default().fg(t.text_dim()).bg(t.bg()));
            }
            x += 1;
        }
    }
}
