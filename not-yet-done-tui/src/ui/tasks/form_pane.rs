use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
};

use crate::app::App;
use crate::config::TasksAction;
use crate::tabs::{FilterField, FilterState, TasksForm};

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
            TasksForm::Filter => render_filter_form(inner, buf, &ts.filter, &self.app.theme),
            TasksForm::Add => render_placeholder(inner, buf, "Add form", "", &self.app),
            TasksForm::Delete => render_placeholder(inner, buf, "Delete form", "󰆴", &self.app),
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
    // Layout: small top padding, then one row per field + spacing
    if area.height < 3 {
        return;
    }

    let mut y = area.y;
    let x = area.x;
    let width = area.width;

    // ── Date fields ───────────────────────────────────────────────────────
    y = render_text_field(
        Rect {
            x,
            y,
            width,
            height: 2,
        },
        buf,
        filter,
        FilterField::CreatedAfter,
        &filter.created_after_raw,
        filter.created_after_err.as_deref(),
        "e.g. last monday, 2 weeks ago, 2024-01-01",
        t,
    );
    y += 1;

    if y + 2 > area.bottom() {
        return;
    }
    y = render_text_field(
        Rect {
            x,
            y,
            width,
            height: 2,
        },
        buf,
        filter,
        FilterField::CreatedBefore,
        &filter.created_before_raw,
        filter.created_before_err.as_deref(),
        "e.g. yesterday, today, 2024-06-30",
        t,
    );
    y += 1;

    // ── Description ───────────────────────────────────────────────────────
    if y + 2 > area.bottom() {
        return;
    }
    y = render_text_field(
        Rect {
            x,
            y,
            width,
            height: 2,
        },
        buf,
        filter,
        FilterField::Description,
        &filter.description_like,
        None,
        "substring match",
        t,
    );
    y += 1;

    // ── Status ────────────────────────────────────────────────────────────
    if y + 2 > area.bottom() {
        return;
    }
    y = render_status_field(
        Rect {
            x,
            y,
            width,
            height: 2,
        },
        buf,
        filter,
        t,
    );
    y += 1;

    // ── Priority ──────────────────────────────────────────────────────────
    if y + 2 > area.bottom() {
        return;
    }
    y = render_text_field(
        Rect {
            x,
            y,
            width,
            height: 2,
        },
        buf,
        filter,
        FilterField::Priority,
        &filter.priority_min_raw,
        filter.priority_err.as_deref(),
        "integer, e.g. 1",
        t,
    );
    y += 1;

    // ── Show deleted ──────────────────────────────────────────────────────
    if y + 1 > area.bottom() {
        return;
    }
    render_toggle_field(
        Rect {
            x,
            y,
            width,
            height: 1,
        },
        buf,
        filter,
        FilterField::ShowDeleted,
        filter.show_deleted,
        t,
    );

    // ── Tag / Project placeholders ────────────────────────────────────────
    // Reserve visual space for future fields so the layout doesn't shift
    let placeholder_y = y + 2;
    if placeholder_y + 1 < area.bottom() {
        let sep = Line::from(vec![Span::styled(
            "  ─── Coming soon: tag & project filters ───",
            Style::default()
                .fg(t.text_dim())
                .add_modifier(Modifier::ITALIC),
        )]);
        Paragraph::new(sep).render(
            Rect {
                x,
                y: placeholder_y,
                width,
                height: 1,
            },
            buf,
        );
    }
}

// ---------------------------------------------------------------------------
// Field renderers
// ---------------------------------------------------------------------------

/// Renders a labelled text input field.
/// Returns the next available y position.
fn render_text_field(
    area: Rect,
    buf: &mut Buffer,
    filter: &FilterState,
    field: FilterField,
    value: &str,
    error: Option<&str>,
    placeholder: &str,
    t: &crate::ui::theme::Theme,
) -> u16 {
    let focused = filter.focused_field == field;
    let x = area.x;
    let y = area.y;
    let width = area.width;

    // Label line
    let label_fg = if focused { t.primary() } else { t.text_dim() };
    let prefix = if focused { "▶ " } else { "  " };
    let label_line = Line::from(vec![
        Span::styled(prefix, Style::default().fg(label_fg)),
        Span::styled(
            field.label(),
            Style::default().fg(label_fg).add_modifier(if focused {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
        ),
        // Inline error
        if let Some(err) = error {
            Span::styled(
                format!("  ⚠ {}", err),
                Style::default()
                    .fg(t.error())
                    .add_modifier(Modifier::ITALIC),
            )
        } else {
            Span::raw("")
        },
    ]);
    Paragraph::new(label_line).render(
        Rect {
            x,
            y,
            width,
            height: 1,
        },
        buf,
    );

    // Input line
    let input_y = y + 1;
    if input_y < area.bottom() {
        let border_fg = if focused { t.primary() } else { t.text_dim() };
        let (display_text, text_style) = if value.is_empty() {
            (
                format!("  {}", placeholder),
                Style::default()
                    .fg(t.text_dim())
                    .add_modifier(Modifier::ITALIC),
            )
        } else {
            let cursor_pos = if focused {
                Some(filter.cursor_pos)
            } else {
                None
            };
            render_text_with_cursor(
                buf,
                x + 2,
                input_y,
                width.saturating_sub(4),
                value,
                cursor_pos,
                t,
                focused,
            );
            // Already rendered by render_text_with_cursor — use empty placeholder
            return input_y + 1;
        };

        // Underline decoration for the input row
        let underline_char = if focused { '─' } else { '╌' };
        let underline: String = std::iter::repeat(underline_char)
            .take(width.saturating_sub(4) as usize)
            .collect();
        let input_line = Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(display_text.trim_start_matches("  "), text_style),
        ]);
        Paragraph::new(input_line).render(
            Rect {
                x,
                y: input_y,
                width,
                height: 1,
            },
            buf,
        );

        // Draw underline below (skip if no room)
        let _ = (underline_char, underline, border_fg); // suppress unused warnings
    }

    input_y + 1
}

/// Render a text value with the cursor inserted at cursor_pos.
/// Returns the next y.
fn render_text_with_cursor(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    value: &str,
    cursor_pos: Option<usize>,
    t: &crate::ui::theme::Theme,
    focused: bool,
) {
    let fg = if focused { t.text_high() } else { t.text_med() };
    let chars: Vec<char> = value.chars().collect();
    let max_w = width as usize;

    // Simple scroll: if cursor would be off-screen, shift view
    let view_start = if let Some(pos) = cursor_pos {
        if pos >= max_w {
            pos + 1 - max_w
        } else {
            0
        }
    } else {
        0
    };

    for (screen_idx, char_idx) in (view_start..chars.len()).enumerate() {
        if screen_idx >= max_w {
            break;
        }
        let cx = x + screen_idx as u16;
        if cx >= x + width {
            break;
        }

        let ch = chars[char_idx];
        let is_cursor = cursor_pos.map_or(false, |p| p == char_idx);
        let style = if is_cursor {
            Style::default().fg(t.bg()).bg(t.primary())
        } else {
            Style::default().fg(fg)
        };

        if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(cx, y)) {
            cell.set_char(ch);
            cell.set_style(style);
        }
    }

    // Draw cursor at end-of-string position if needed
    if let Some(pos) = cursor_pos {
        if pos == chars.len() {
            let screen_pos = pos.saturating_sub(view_start);
            if screen_pos < max_w {
                let cx = x + screen_pos as u16;
                if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(cx, y)) {
                    cell.set_char(' ');
                    cell.set_bg(t.primary());
                }
            }
        }
    }
}

/// Render the status multi-select field.
fn render_status_field(
    area: Rect,
    buf: &mut Buffer,
    filter: &FilterState,
    t: &crate::ui::theme::Theme,
) -> u16 {
    let focused = filter.focused_field == FilterField::Status;
    let x = area.x;
    let y = area.y;
    let width = area.width;

    let label_fg = if focused { t.primary() } else { t.text_dim() };
    let prefix = if focused { "▶ " } else { "  " };

    // Label
    let label_line = Line::from(vec![
        Span::styled(prefix, Style::default().fg(label_fg)),
        Span::styled(
            FilterField::Status.label(),
            Style::default().fg(label_fg).add_modifier(if focused {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
        ),
        Span::styled(
            "  ← → navigate  space toggle",
            Style::default().fg(t.text_dim()),
        ),
    ]);
    Paragraph::new(label_line).render(
        Rect {
            x,
            y,
            width,
            height: 1,
        },
        buf,
    );

    // Options row
    let opts_y = y + 1;
    if opts_y < area.bottom() {
        let statuses = [
            ("todo", filter.status.todo, 0),
            ("in_progress", filter.status.in_progress, 1),
            ("done", filter.status.done, 2),
            ("cancelled", filter.status.cancelled, 3),
        ];

        let mut rx = x + 2u16;
        for (label, active, idx) in &statuses {
            let is_cursor = focused && filter.status_cursor == *idx;
            let is_checked = *active;

            let check = if is_checked { "● " } else { "○ " };
            let check_fg = if is_checked {
                t.primary()
            } else {
                t.text_dim()
            };
            let (text_fg, text_bg, bold) = if is_cursor {
                (t.on_primary(), t.primary(), true)
            } else {
                (t.text_med(), t.surface_2(), false)
            };

            let checkbox_style = Style::default().fg(check_fg).bg(text_bg);
            let label_style = Style::default()
                .fg(text_fg)
                .bg(text_bg)
                .add_modifier(if bold {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                });

            let label_str = format!("{} ", label);
            let total_width = (check.len() + label_str.len()) as u16;

            // Write check
            for (ci, ch) in check.chars().enumerate() {
                let cx = rx + ci as u16;
                if cx >= x + width {
                    break;
                }
                if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(cx, opts_y)) {
                    cell.set_char(ch);
                    cell.set_style(checkbox_style);
                }
            }
            // Write label
            for (li, ch) in label_str.chars().enumerate() {
                let cx = rx + check.len() as u16 + li as u16;
                if cx >= x + width {
                    break;
                }
                if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(cx, opts_y)) {
                    cell.set_char(ch);
                    cell.set_style(label_style);
                }
            }

            rx += total_width + 1;
        }
    }

    opts_y + 1
}

/// Render a boolean toggle field (checkbox).
fn render_toggle_field(
    area: Rect,
    buf: &mut Buffer,
    filter: &FilterState,
    field: FilterField,
    value: bool,
    t: &crate::ui::theme::Theme,
) {
    let focused = filter.focused_field == field;
    let label_fg = if focused { t.primary() } else { t.text_dim() };
    let prefix = if focused { "▶ " } else { "  " };
    let check = if value { "󰄵  " } else { "󰄰  " };
    let check_fg = if value { t.success() } else { t.text_dim() };

    let line = Line::from(vec![
        Span::styled(prefix, Style::default().fg(label_fg)),
        Span::styled(check, Style::default().fg(check_fg)),
        Span::styled(
            field.label(),
            Style::default().fg(label_fg).add_modifier(if focused {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
        ),
        Span::styled("  space to toggle", Style::default().fg(t.text_dim())),
    ]);
    Paragraph::new(line).render(area, buf);
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
