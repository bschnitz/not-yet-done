use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    widgets::Clear,
    Frame,
};

use not_yet_done_ratatui::FilePicker;
use tuirealm::component::Component;

use crate::app::App;
use crate::tabs::Tab;

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    // Each view owns its own action bar; ask the active view for its height.
    let Tab::Content(active_idx) = app.active_tab;
    let action_bar_height = app
        .content_view(active_idx)
        .map(|cv| cv.action_bar_height(area.width))
        .unwrap_or(0);

    let notification_height = app.notification_bar.required_height(area.width);
    let status_bar_height = app.status_bar.required_height(area.width);

    let tab_bar_height = app.tab_bar.required_height(area.width);
    let mut constraints = vec![Constraint::Length(tab_bar_height)];
    if action_bar_height > 0 {
        constraints.push(Constraint::Length(action_bar_height));
    }
    constraints.push(Constraint::Fill(1));
    if notification_height > 0 {
        constraints.push(Constraint::Length(notification_height));
    }
    constraints.push(Constraint::Length(status_bar_height));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let mut idx = 0;
    app.tab_bar.view(frame, chunks[idx]);
    idx += 1;

    if action_bar_height > 0 {
        let bar_area = chunks[idx];
        if let Some(cv) = app.content_view_mut(active_idx) {
            cv.render_action_bar(frame, bar_area);
        }
        idx += 1;
    }

    let content_area = chunks[idx];
    idx += 1;

    {
        let content_idx = active_idx;
        // Inline error bar above the content view, mirroring the
        // tasks render path so adapter-action errors
        // (`set_query_error(Some(_))`) are visible on Content tabs too.
        let error_height = app.query_error_bar.required_height(content_area.width);
        let mut constraints: Vec<Constraint> = Vec::new();
        if error_height > 0 {
            constraints.push(Constraint::Length(error_height));
        }
        constraints.push(Constraint::Fill(1));
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(content_area);
        let mut row_idx = 0;
        if error_height > 0 {
            app.query_error_bar.view(frame, rows[row_idx]);
            row_idx += 1;
        }
        if let Some(cv) = app.content_view_mut(content_idx) {
            cv.view(frame, rows[row_idx]);
        } else {
            // Broken content slot: show the configuration error
            // panel in the content area (Phase 3 placeholder; final
            // theming lives in `ui::content_error`).
            crate::ui::content_error::render(frame, rows[row_idx], app, content_idx);
        }
    }

    if notification_height > 0 {
        app.notification_bar.view(frame, chunks[idx]);
        idx += 1;
    }

    let status_area = chunks[idx];
    app.status_bar.view(frame, status_area);

    // Overlay: content action popup (transitions, etc.).
    if let Some(ref mut state) = app.content_action_popup {
        state.popup.view(frame, area);
    }

    // Overlay: content file-picker popup (e.g. Taiga attachment upload).
    if let Some(ref mut state) = app.content_file_picker_popup {
        let theme = std::sync::Arc::clone(&app.shared_theme);
        render_file_picker_overlay(
            frame,
            area,
            &mut state.picker,
            &state.action_id,
            &theme,
        );
    }

    // Overlay: generic content form popup (`InputSpec::Form` actions).
    if let Some(ref state) = app.content_form_popup {
        state.popup.render(frame, area, &app.shared_theme);
    }

    // Overlay: column config popup.
    if let Some(popup) = &mut app.column_config_popup {
        popup.view(frame, area);
    }

    // Overlay: adapter credentials popup.
    if let Some(popup) = &mut app.adapter_creds_popup {
        popup.view(frame, area);
    }

    // Overlay: query-variable input popup.
    if let Some(popup) = &mut app.query_var_popup {
        popup.view(frame, area);
    }

    // Overlay: link popup (gl).
    if let Some(state) = &mut app.link_popup {
        state.popup.view(frame, area);
    }

    // Overlay: :config picker popup.
    if let Some(popup) = &mut app.config_picker_popup {
        popup.view(frame, area);
    }

    // Overlay: :script fuzzy menu.
    if app.script_menu.is_open() {
        app.script_menu.render(frame, area);
    }

    // Overlay: :tag tag-management menu.
    if app.tag_menu.is_open() {
        app.tag_menu.render(frame, area);
    }

    // Overlay: tab-set switch popup (ctrl+x).
    if app.tab_set_popup.is_open() {
        app.tab_set_popup.render(frame, area);
    }

    // Overlay: modal message.
    if let Some(ref msg) = app.modal_message {
        render_modal(frame, area, &app.theme, msg);
    }
}

fn render_modal(frame: &mut Frame, area: ratatui::layout::Rect, theme: &crate::ui::theme::Theme, msg: &str) {
    use ratatui::layout::Position;
    use ratatui::style::Style;
    use ratatui::widgets::Clear;

    let lines: Vec<&str> = msg.lines().collect();
    let max_line_w = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let popup_w = (max_line_w as u16 + 4).min(area.width.saturating_sub(4));
    let popup_h = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));

    let x = (area.width.saturating_sub(popup_w)) / 2;
    let y = (area.height.saturating_sub(popup_h)) / 2;
    let popup_area = ratatui::layout::Rect::new(x, y, popup_w, popup_h);

    frame.render_widget(Clear, popup_area);

    let buf = frame.buffer_mut();
    let bg = theme.surface();
    let border_fg = theme.primary();

    // Fill background.
    for py in popup_area.top()..popup_area.bottom() {
        for px in popup_area.left()..popup_area.right() {
            if let Some(cell) = buf.cell_mut(Position::new(px, py)) {
                cell.set_char(' ');
                cell.set_style(Style::default().bg(bg));
            }
        }
    }

    // Border.
    let border_style = Style::default().fg(border_fg).bg(bg);
    for px in popup_area.left()..popup_area.right() {
        if let Some(cell) = buf.cell_mut(Position::new(px, popup_area.top())) {
            cell.set_char('─');
            cell.set_style(border_style);
        }
        if let Some(cell) = buf.cell_mut(Position::new(px, popup_area.bottom().saturating_sub(1))) {
            cell.set_char('─');
            cell.set_style(border_style);
        }
    }
    for py in popup_area.top()..popup_area.bottom() {
        if let Some(cell) = buf.cell_mut(Position::new(popup_area.left(), py)) {
            cell.set_char('│');
            cell.set_style(border_style);
        }
        if let Some(cell) = buf.cell_mut(Position::new(popup_area.right().saturating_sub(1), py)) {
            cell.set_char('│');
            cell.set_style(border_style);
        }
    }
    // Corners.
    if let Some(cell) = buf.cell_mut(Position::new(popup_area.left(), popup_area.top())) {
        cell.set_char('╭'); cell.set_style(border_style);
    }
    if let Some(cell) = buf.cell_mut(Position::new(popup_area.right().saturating_sub(1), popup_area.top())) {
        cell.set_char('╮'); cell.set_style(border_style);
    }
    if let Some(cell) = buf.cell_mut(Position::new(popup_area.left(), popup_area.bottom().saturating_sub(1))) {
        cell.set_char('╰'); cell.set_style(border_style);
    }
    if let Some(cell) = buf.cell_mut(Position::new(popup_area.right().saturating_sub(1), popup_area.bottom().saturating_sub(1))) {
        cell.set_char('╯'); cell.set_style(border_style);
    }

    // Text.
    let text_style = Style::default().fg(theme.text_high()).bg(bg);
    for (i, line) in lines.iter().enumerate() {
        let ly = popup_area.top() + 1 + i as u16;
        if ly >= popup_area.bottom().saturating_sub(1) { break; }
        let mut lx = popup_area.left() + 2;
        for ch in line.chars() {
            if lx >= popup_area.right().saturating_sub(1) { break; }
            if let Some(cell) = buf.cell_mut(Position::new(lx, ly)) {
                cell.set_char(ch);
                cell.set_style(text_style);
            }
            lx += 1;
        }
    }
}

/// Centered overlay for `FilePicker`. The picker now renders its own
/// title + help bar from its live keymap, so we only need to size +
/// clear the area and forward styling via its `FilePickerStyle`.
fn render_file_picker_overlay(
    frame: &mut Frame,
    area: Rect,
    picker: &mut FilePicker,
    _action_id: &str,
    _theme: &crate::ui::theme::Theme,
) {
    let popup_w = area.width.saturating_sub(8).min(96).max(40);
    let popup_h = area.height.saturating_sub(4).min(32).max(18);
    let x = (area.width.saturating_sub(popup_w)) / 2;
    let y = (area.height.saturating_sub(popup_h)) / 2;
    let popup = Rect::new(x, y, popup_w, popup_h);

    frame.render_widget(Clear, popup);
    picker.view(frame, popup);
}
