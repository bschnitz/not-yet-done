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
/// Construct this from your theme — the widget itself has no theme dependency.
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
/// }
/// ```
#[derive(Debug, Clone, Copy)]
pub struct TextFieldStyle {
    pub label_focused: ratatui::style::Color,
    pub label_idle: ratatui::style::Color,
    pub input_focused: ratatui::style::Color,
    pub input_idle: ratatui::style::Color,
    /// Foreground of the cursor block.
    pub cursor_fg: ratatui::style::Color,
    /// Background of the cursor block.
    pub cursor_bg: ratatui::style::Color,
    pub error_fg: ratatui::style::Color,
    pub placeholder_fg: ratatui::style::Color,
    /// Background of the input row — gives the field an HTML-input-like appearance.
    pub input_bg: ratatui::style::Color,
}

// ---------------------------------------------------------------------------
// Widget
// ---------------------------------------------------------------------------

/// A single labelled text-input field with optional inline error and cursor.
///
/// Occupies exactly **2 rows**: label row + input row.
/// Returns the next available `y` via [`TextFieldWidget::render_and_next_y`].
///
/// Use [`Widget::render`] if you just need to paint it into a fixed `Rect`.
///
/// # Layout
/// ```text
/// ▶ Label text          ⚠ optional error
///   current input value█
/// ```
pub struct TextFieldWidget<'a> {
    /// Visible label above the input.
    pub label: &'a str,
    /// Current text value.
    pub value: &'a str,
    /// Greyed-out hint shown when `value` is empty.
    pub placeholder: &'a str,
    /// Optional inline error shown next to the label.
    pub error: Option<&'a str>,
    /// Whether this field currently has keyboard focus.
    pub focused: bool,
    /// Cursor position in Unicode scalar values (chars).
    /// `None` = no cursor rendered (e.g. field is idle).
    pub cursor_pos: Option<usize>,
    pub style: TextFieldStyle,
}

impl<'a> TextFieldWidget<'a> {
    /// Render into `area` and return the next `y` position after this widget.
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
        let label_fg = if self.focused { s.label_focused } else { s.label_idle };
        let prefix = if self.focused { "▶ " } else { "  " };
        let label_modifier = if self.focused {
            Modifier::BOLD
        } else {
            Modifier::empty()
        };

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

        Paragraph::new(label_line).render(Rect { x, y, width, height: 1 }, buf);

        // ── Input row ─────────────────────────────────────────────────────
        let input_y = y + 1;
        if input_y >= area.bottom() {
            return;
        }

        // Fill the entire input row with input_bg first — creates the
        // "HTML input field" look regardless of whether there is content.
        let input_inner_x = x + 1;
        let input_inner_width = width.saturating_sub(2);
        for col in 0..input_inner_width {
            let cx = input_inner_x + col;
            if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(cx, input_y)) {
                cell.set_char(' ');
                cell.set_bg(s.input_bg);
            }
        }

        if self.value.is_empty() {
            // Show placeholder on top of the filled background
            let ph_line = Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    self.placeholder,
                    Style::default()
                        .fg(s.placeholder_fg)
                        .add_modifier(Modifier::ITALIC)
                        .bg(s.input_bg),
                ),
            ]);
            Paragraph::new(ph_line).render(
                Rect { x, y: input_y, width, height: 1 },
                buf,
            );
        } else {
            render_text_with_cursor(
                buf,
                x + 2,
                input_y,
                width.saturating_sub(4),
                self.value,
                self.cursor_pos,
                s,
                self.focused,
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
) {
    let fg = if focused { s.input_focused } else { s.input_idle };
    let chars: Vec<char> = value.chars().collect();
    let max_w = width as usize;

    // Scroll the view so the cursor stays visible.
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
            Style::default().fg(fg).bg(s.input_bg)
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
