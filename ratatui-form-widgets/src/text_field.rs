use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

// ---------------------------------------------------------------------------
// Style
// ---------------------------------------------------------------------------

/// All colors needed to render a `TextFieldWidget`.
/// Construct this from your theme — widget itself has no theme dependency.
///
/// # Example (in your theme adapter)
/// ```rust
/// TextFieldStyle {
///     label_focused:   theme.primary(),
///     label_idle:      theme.text_dim(),
///     input_focused:   theme.text_high(),
///     input_idle:      theme.text_med(),
///     cursor_fg:       theme.bg(),
///     cursor_bg:       theme.primary(),
///     error_fg:        theme.error(),
///     placeholder_fg:  theme.text_dim(),
///     input_bg:        theme.surface(),
///     focused_bg:      theme.focused_bg(),
/// }
/// ```
#[derive(Debug, Clone, Copy)]
pub struct TextFieldStyle {
    pub label_focused: ratatui::style::Color,
    pub label_idle: ratatui::style::Color,
    pub input_focused: ratatui::style::Color,
    pub input_idle: ratatui::style::Color,
    /// Foreground of cursor block.
    pub cursor_fg: ratatui::style::Color,
    /// Background of cursor block.
    pub cursor_bg: ratatui::style::Color,
    pub error_fg: ratatui::style::Color,
    pub placeholder_fg: ratatui::style::Color,
    /// Background of input row — gives a field an HTML-input-like appearance.
    pub input_bg: ratatui::style::Color,
    /// Background for focused field (header + input).
    pub focused_bg: ratatui::style::Color,
}

// ---------------------------------------------------------------------------
// Widget
// ---------------------------------------------------------------------------

/// A single labelled text-input field with optional inline error and cursor.
///
/// Occupies exactly **2 rows**: label row + input row.
/// Returns to next available `y` via [`TextFieldWidget::render_and_next_y`].
///
/// Use [`Widget::render`] if you just need to paint it into a fixed `Rect`.
///
/// # Layout
/// ```text
/// ▍ Label text          ⚠ optional error
///   current input value█
/// ```
pub struct TextFieldWidget<'a> {
    /// Visible label above input.
    pub label: &'a str,
    /// Current text value.
    pub value: &'a str,
    /// Greyed-out hint shown when `value` is empty.
    pub placeholder: &'a str,
    /// Optional inline error shown next to label.
    pub error: Option<&'a str>,
    /// Whether this field currently has keyboard focus.
    pub focused: bool,
    /// Cursor position in Unicode scalar values (chars).
    /// `None` = no cursor rendered (e.g. field is idle).
    pub cursor_pos: Option<usize>,
    pub style: TextFieldStyle,
}

impl<'a> TextFieldWidget<'a> {
    /// Render into `area` and return to next `y` position after this widget.
    ///
    /// This is the preferred entry point when stacking multiple fields
    /// vertically, because it tells the caller where the next field starts.
    pub fn render_and_next_y(self, area: Rect, buf: &mut Buffer) -> u16 {
        let next_y = area.y + 2;
        Widget::render(self, area, buf);
        next_y
    }
}

impl Widget for TextFieldWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 2 {
            return;
        }

        let x = area.x;
        let y = area.y;
        let width = area.width;
        let s = &self.style;

        // ── Label row ─────────────────────────────────────────────────────
        let label_fg = s.label_focused;
        let prefix = " ▍ ";
        let label_modifier = Modifier::BOLD;

        let label_line = Line::from(vec![
            Span::styled(prefix, Style::default().fg(label_fg)),
            Span::styled(
                self.label,
                Style::default().fg(label_fg).add_modifier(label_modifier),
            ),
            match self.error {
                Some(err) => Span::styled(
                    format!("  ⚠ {}", err),
                    Style::default()
                        .fg(s.error_fg)
                        .add_modifier(Modifier::ITALIC),
                ),
                None => Span::raw(""),
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

        // ── Input row ─────────────────────────────────────────────────────
        let input_y = y + 1;
        if input_y >= area.bottom() {
            return;
        }

        // Determine background color for focused state
        let bg_color = if self.focused {
            s.focused_bg
        } else {
            s.input_bg
        };

        // Write ▍ prefix on input row (with leading space)
        let prefix_str = " ▍ ";
        let label_fg = s.label_focused;
        let prefix_style = Style::default().fg(label_fg);
        let prefix_chars: Vec<char> = prefix_str.chars().collect();

        for (pi, ch) in prefix_chars.iter().enumerate() {
            let cx = x + pi as u16;
            if cx >= x + width {
                return;
            }
            if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(cx, input_y)) {
                cell.set_char(*ch);
                cell.set_style(prefix_style);
            }
        }

        // Fill rest of input row with background — creates a
        // "HTML input field" look regardless of whether there is content.
        // Prefix takes 3 chars (space + ▍ + space)
        let input_inner_x = x + 3;
        let input_inner_width = width.saturating_sub(4); // 3 for prefix + 1 for space
        for col in 0..input_inner_width {
            let cx = input_inner_x + col;
            if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(cx, input_y)) {
                cell.set_char(' ');
                cell.set_bg(bg_color);
            }
        }

        // Determine background color for focused state
        let bg_color = if self.focused {
            s.focused_bg
        } else {
            s.input_bg
        };

        if self.value.is_empty() {
            // Show placeholder on top of filled background
            let ph_line = Line::from(vec![Span::styled(
                self.placeholder,
                Style::default()
                    .fg(s.placeholder_fg)
                    .add_modifier(Modifier::ITALIC)
                    .bg(bg_color),
            )]);
            Paragraph::new(ph_line).render(
                Rect {
                    x: input_inner_x,
                    y: input_y,
                    width: input_inner_width,
                    height: 1,
                },
                buf,
            );
        } else {
            render_text_with_cursor(
                buf,
                input_inner_x,
                input_y,
                input_inner_width,
                self.value,
                self.cursor_pos,
                s,
                self.focused,
                bg_color,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Cursor rendering helper (private)
// ---------------------------------------------------------------------------

fn render_text_with_cursor(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    value: &str,
    cursor_pos: Option<usize>,
    s: &TextFieldStyle,
    focused: bool,
    bg_color: ratatui::style::Color,
) {
    let fg = if focused {
        s.input_focused
    } else {
        s.input_idle
    };
    let chars: Vec<char> = value.chars().collect();
    let max_w = width as usize;

    // Scroll view so that cursor stays visible.
    let view_start = match cursor_pos {
        Some(pos) if pos >= max_w => pos + 1 - max_w,
        _ => 0,
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
            Style::default().fg(s.cursor_fg).bg(s.cursor_bg)
        } else {
            Style::default().fg(fg).bg(bg_color)
        };

        if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(cx, y)) {
            cell.set_char(ch);
            cell.set_style(style);
        }
    }

    // Cursor at end-of-string
    if let Some(pos) = cursor_pos {
        if pos == chars.len() {
            let screen_pos = pos.saturating_sub(view_start);
            if screen_pos < max_w {
                let cx = x + screen_pos as u16;
                if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(cx, y)) {
                    cell.set_char(' ');
                    cell.set_bg(s.cursor_bg);
                }
            }
        }
    }
}
