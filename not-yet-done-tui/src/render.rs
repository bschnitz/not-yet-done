use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    widgets::Clear,
};

use not_yet_done_ratatui::FilePicker;
use tuirealm::component::Component;

use crate::app::App;
use crate::tabs::Tab;

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    // Before any height is measured: globally routed load banners are derived
    // from the tabs' current status, and the bar they live on must already
    // hold them when `required_height` asks how tall it is.
    app.sync_load_banners();

    // Fullscreen hides the chrome bars (tab bar, the view's action/shortcut
    // bar and the bottom status bar) so the content view fills the terminal.
    // Message bars (alerts, notifications, inline query errors) stay visible.
    let fullscreen = app.fullscreen;

    // Each view owns its own action bar; ask the active view for its height.
    let Tab::Content(active_idx) = app.active_tab;
    let action_bar_height = if fullscreen {
        0
    } else {
        app.content_view(active_idx)
            .map(|cv| cv.action_bar_height(area.width))
            .unwrap_or(0)
    };

    // The builtin editor is a pane of the layout, not a floating overlay: it
    // takes its rows from the content view so the surrounding chrome and both
    // message bars stay where they are.
    let editor_height = app
        .builtin_editor
        .as_ref()
        .map(|pane| pane.required_height(area.height))
        .unwrap_or(0);

    let notification_height = app.notification_bar.required_height(area.width);
    let alert_height = app.alert_bar.required_height(area.width);
    let status_bar_height = if fullscreen {
        0
    } else {
        app.status_bar.required_height(area.width)
    };

    let tab_bar_height = if fullscreen {
        0
    } else {
        app.tab_bar.required_height(area.width)
    };
    // The prominent alert bar (when it has messages) sits directly beneath the
    // tab bar and the view's action/key bar, so it reads as a banner attached to
    // the top chrome rather than floating above it. Zero-height and invisible
    // when empty.
    let mut constraints = vec![Constraint::Length(tab_bar_height)];
    if action_bar_height > 0 {
        constraints.push(Constraint::Length(action_bar_height));
    }
    if alert_height > 0 {
        constraints.push(Constraint::Length(alert_height));
    }
    constraints.push(Constraint::Fill(1));
    if editor_height > 0 {
        constraints.push(Constraint::Length(editor_height));
    }
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

    if alert_height > 0 {
        app.alert_bar.view(frame, chunks[idx]);
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

    if editor_height > 0 {
        let editor_area = chunks[idx];
        if let Some(pane) = app.builtin_editor.as_mut() {
            pane.render(frame, editor_area);
        }
        idx += 1;
    }

    if notification_height > 0 {
        app.notification_bar.view(frame, chunks[idx]);
        idx += 1;
    }

    let status_area = chunks[idx];
    app.status_bar.view(frame, status_area);

    // Floating popups are kept at least two blank rows clear of the
    // terminal's top and bottom edge, regardless of where the surrounding
    // chrome (tab/action/status bars) sits. Every overlay below sizes and
    // centres itself within this area.
    let popup_area = popup_bounds(area);

    // Overlay: content action popup (transitions, etc.).
    if let Some(ref mut state) = app.content_action_popup {
        state.popup.view(frame, popup_area);
    }

    // Overlay: content file-picker popup (e.g. Taiga attachment upload).
    if let Some(ref mut state) = app.content_file_picker_popup {
        let theme = std::sync::Arc::clone(&app.shared_theme);
        render_file_picker_overlay(
            frame,
            popup_area,
            &mut state.picker,
            &state.action_id,
            &theme,
        );
    }

    // Overlay: generic content form popup (`InputSpec::Form` actions).
    if let Some(ref mut state) = app.content_form_popup {
        state.popup.render(frame, popup_area);
    }

    // Overlay: column config popup.
    if let Some(popup) = &mut app.column_config_popup {
        popup.view(frame, popup_area);
    }

    // Overlay: sort menu.
    if let Some(popup) = &mut app.sort_menu_popup {
        popup.render(frame, popup_area);
    }

    // Overlay: adapter credentials popup.
    if let Some(popup) = &mut app.adapter_creds_popup {
        popup.view(frame, popup_area);
    }

    // Overlay: query-variable input popup.
    if let Some(popup) = &mut app.query_var_popup {
        popup.view(frame, popup_area);
    }

    // Overlay: link popup (gl).
    if let Some(state) = &mut app.link_popup {
        state.popup.view(frame, popup_area);
    }

    // Overlay: :config picker popup.
    if let Some(popup) = &mut app.config_picker_popup {
        popup.view(frame, popup_area);
    }

    // Overlay: :script fuzzy menu.
    if app.script_menu.is_open() {
        app.script_menu.render(frame, popup_area);
    }

    // Overlay: generic option menu (a `type: option_menu` action).
    if app.option_menu.is_open() {
        app.option_menu.render(frame, popup_area);
    }

    // Overlay: shortcut/action menu (ctrl+y).
    if app.shortcut_menu.is_open() {
        app.shortcut_menu.render(frame, popup_area);
    }

    // Overlay: which-key chord-completion preview (passive; mirrors the
    // half-typed chord).
    if app.which_key.is_open() {
        app.which_key.render(frame, popup_area);
    }

    // Overlay: adapter prompt (e.g. MFA challenge). Drawn near the top so it is
    // never hidden behind a lower popup; it blocks input while shown.
    if let Some(ref mut popup) = app.adapter_prompt_popup {
        popup.render(frame, area, &app.shared_theme);
    }

    // Overlay: modal message.
    if let Some(ref msg) = app.modal_message {
        render_modal(frame, area, &app.theme, msg);
    }

    // Final pass: pin the diff width of VS16 emoji cells (see `force_vs16_widths`).
    force_vs16_widths(frame.buffer_mut());
}

/// Work around a ratatui-core `BufferDiff` quirk that corrupts rows containing
/// VS16 emoji (e.g. the ⚠️ tag symbol) on terminals that render such emoji two
/// cells wide.
///
/// When an emoji whose sequence contains U+FE0F is drawn, the diff emits an
/// explicit "clearing" space for its trailing column *only when the previous
/// frame held a non-blank char there*. On a terminal that draws the emoji two
/// cells wide that space is spurious — it lands past the emoji and shifts the
/// rest of the row, so the row looks garbled until a full row redraw (cursor
/// hover) repaints it. The effect is intermittent because it depends on what
/// scrolled into that screen position (e.g. after a tree expand).
///
/// Marking every VS16 cell with [`CellDiffOption::ForcedWidth`] makes the diff
/// treat it like a plain wide character: it emits only the emoji cell and skips
/// the trailing column entirely (no clearing space), matching how the terminal
/// actually renders it. `ratatui-core` is a registry dependency, so this cannot
/// be fixed at the source and must be applied to the frame buffer here.
fn force_vs16_widths(buf: &mut ratatui::buffer::Buffer) {
    use ratatui::buffer::CellDiffOption;
    use std::num::NonZeroU16;
    use unicode_width::UnicodeWidthStr;

    let mut count = 0usize;
    for cell in buf.content.iter_mut() {
        let symbol = cell.symbol();
        if symbol.contains('\u{FE0F}') {
            let width = UnicodeWidthStr::width(symbol);
            if let Some(w) = NonZeroU16::new(width.min(u16::MAX as usize) as u16) {
                if w.get() > 1 {
                    cell.set_diff_option(CellDiffOption::ForcedWidth(w));
                    count += 1;
                }
            }
        }
    }

    // Opt-in diagnostics: confirm in the wild that this path actually fires in
    // the user's terminal. Zero cost when the env var is unset. Enable with
    // `NYD_DEBUG_VS16=1`; a single line is appended to
    // `$TMPDIR/nyd-vs16-debug.log` the first time VS16 cells are pinned (not
    // every frame — that would flood the log, since ⚠️ is on screen constantly).
    if count > 0 {
        static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if std::env::var_os("NYD_DEBUG_VS16").is_some()
            && !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            let path = std::env::temp_dir().join("nyd-vs16-debug.log");
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                use std::io::Write;
                let _ = writeln!(
                    f,
                    "force_vs16_widths engaged: {count} VS16 emoji cell(s) pinned to ForcedWidth on first paint"
                );
            }
        }
    }
}

/// The region floating popups may occupy: the full terminal area shrunk by
/// two rows top and bottom, so a centred popup always leaves at least two
/// blank lines between itself and the terminal's top/bottom edge — measured
/// from the edge, not from the surrounding chrome. Degrades gracefully on
/// tiny terminals — never shrinks below a usable body.
fn popup_bounds(area: Rect) -> Rect {
    // Only inset while there is height to spare for the two margin rows on
    // each side plus a minimal popup body; otherwise fall back to full area.
    if area.height <= 4 {
        return area;
    }
    Rect {
        x: area.x,
        y: area.y.saturating_add(2),
        width: area.width,
        height: area.height.saturating_sub(4),
    }
}

fn render_modal(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    theme: &crate::ui::theme::Theme,
    msg: &str,
) {
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
        cell.set_char('╭');
        cell.set_style(border_style);
    }
    if let Some(cell) = buf.cell_mut(Position::new(
        popup_area.right().saturating_sub(1),
        popup_area.top(),
    )) {
        cell.set_char('╮');
        cell.set_style(border_style);
    }
    if let Some(cell) = buf.cell_mut(Position::new(
        popup_area.left(),
        popup_area.bottom().saturating_sub(1),
    )) {
        cell.set_char('╰');
        cell.set_style(border_style);
    }
    if let Some(cell) = buf.cell_mut(Position::new(
        popup_area.right().saturating_sub(1),
        popup_area.bottom().saturating_sub(1),
    )) {
        cell.set_char('╯');
        cell.set_style(border_style);
    }

    // Text.
    let text_style = Style::default().fg(theme.text_high()).bg(bg);
    for (i, line) in lines.iter().enumerate() {
        let ly = popup_area.top() + 1 + i as u16;
        if ly >= popup_area.bottom().saturating_sub(1) {
            break;
        }
        let mut lx = popup_area.left() + 2;
        for ch in line.chars() {
            if lx >= popup_area.right().saturating_sub(1) {
                break;
            }
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

#[cfg(test)]
mod tests {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Style};

    /// Without pinning, the diff of a ⚠️ redraw over a row that previously had a
    /// non-blank char in the emoji's trailing column emits a spurious clearing
    /// space (col 1) that garbles the row on 2-cell-wide terminals.
    #[test]
    fn vs16_trailing_clobber_present_without_fix() {
        let rect = Rect::new(0, 0, 4, 1);
        let mut prev = Buffer::empty(rect);
        prev.set_string(0, 0, "ZZab", Style::default());
        let mut next = Buffer::empty(rect);
        next.set_string(0, 0, "\u{26A0}\u{FE0F}", Style::new().fg(Color::Yellow));
        next.set_string(2, 0, "ab", Style::default());

        let cols: Vec<u16> = prev.diff(&next).iter().map(|(x, _, _)| *x).collect();
        assert!(
            cols.contains(&1),
            "baseline: expected the clobber space at col 1; got {cols:?}"
        );
    }

    /// After `force_vs16_widths`, the emoji cell is pinned to `ForcedWidth`, so
    /// the diff emits only the emoji (col 0) and never the clobbering space.
    #[test]
    fn vs16_trailing_clobber_gone_after_fix() {
        let rect = Rect::new(0, 0, 4, 1);
        let mut prev = Buffer::empty(rect);
        prev.set_string(0, 0, "ZZab", Style::default());
        let mut next = Buffer::empty(rect);
        next.set_string(0, 0, "\u{26A0}\u{FE0F}", Style::new().fg(Color::Yellow));
        next.set_string(2, 0, "ab", Style::default());

        super::force_vs16_widths(&mut next);

        let cols: Vec<u16> = prev.diff(&next).iter().map(|(x, _, _)| *x).collect();
        assert!(
            !cols.contains(&1),
            "col 1 clobber space must be gone; got {cols:?}"
        );
        assert!(
            cols.contains(&0),
            "emoji at col 0 must still be emitted; got {cols:?}"
        );
    }

    /// A plain (non-VS16) wide character must be left untouched — the workaround
    /// targets only VS16 presentation sequences.
    #[test]
    fn plain_wide_char_untouched() {
        let rect = Rect::new(0, 0, 4, 1);
        let mut buf = Buffer::empty(rect);
        buf.set_string(0, 0, "你a", Style::default());
        super::force_vs16_widths(&mut buf);
        assert_eq!(
            buf.content[0].diff_option,
            ratatui::buffer::CellDiffOption::None,
            "non-VS16 wide char must keep its default diff option"
        );
    }
}
