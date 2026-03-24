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

/// All colors needed to render a `MultipleChoiceWidget`.
///
/// # Example (in your theme adapter)
/// ```rust
/// MultipleChoiceStyle {
///     label_focused:    theme.primary(),
///     label_idle:       theme.text_dim(),
///     checked_fg:       theme.primary(),
///     unchecked_fg:     theme.text_dim(),
///     cursor_text_fg:   theme.on_primary(),
///     cursor_bg:        theme.primary(),
///     item_idle_fg:     theme.text_med(),
///     item_idle_bg:     theme.surface_2(),
///     hint_fg:          theme.text_dim(),
/// }
/// ```
#[derive(Debug, Clone, Copy)]
pub struct MultipleChoiceStyle {
    pub label_focused: ratatui::style::Color,
    pub label_idle: ratatui::style::Color,
    /// Color of the ●/○ indicator when the option is checked.
    pub checked_fg: ratatui::style::Color,
    /// Color of the ●/○ indicator when the option is unchecked.
    pub unchecked_fg: ratatui::style::Color,
    /// Text color of the option under the keyboard cursor.
    pub cursor_text_fg: ratatui::style::Color,
    /// Background of the option under the keyboard cursor.
    pub cursor_bg: ratatui::style::Color,
    /// Text color of options not under the cursor.
    pub item_idle_fg: ratatui::style::Color,
    /// Background of options not under the cursor.
    pub item_idle_bg: ratatui::style::Color,
    /// Color for the navigation hint text.
    pub hint_fg: ratatui::style::Color,
}

// ---------------------------------------------------------------------------
// ChoiceOption
// ---------------------------------------------------------------------------

/// A single selectable option passed to [`MultipleChoiceWidget`].
pub struct ChoiceOption<'a> {
    /// Display label shown to the user.
    pub label: &'a str,
    /// Whether this option is currently selected.
    pub selected: bool,
}

impl<'a> ChoiceOption<'a> {
    pub fn new(label: &'a str, selected: bool) -> Self {
        Self { label, selected }
    }
}

// ---------------------------------------------------------------------------
// Widget
// ---------------------------------------------------------------------------

/// A labelled horizontal multi-select widget with keyboard cursor navigation.
///
/// Occupies exactly **2 rows**: label row + options row.
/// Returns the next available `y` via [`MultipleChoiceWidget::render_and_next_y`].
///
/// # Layout
/// ```text
///   Status              ← → navigate  space toggle
///   ● todo  ○ in_progress  ● done  ○ cancelled
///       ↑ cursor highlight when focused
/// ```
///
/// The widget is fully generic over the option set — pass any `&[ChoiceOption]`.
pub struct MultipleChoiceWidget<'a> {
    /// Label displayed above the options row.
    pub label: &'a str,
    /// The selectable options (in display order).
    pub options: &'a [ChoiceOption<'a>],
    /// Whether this field currently has keyboard focus.
    pub focused: bool,
    /// Index of the option under the keyboard cursor (only relevant when focused).
    pub cursor: usize,
    /// Navigation hint appended to the label row.  Pass `""` to hide.
    pub nav_hint: &'a str,
    /// Icon for a selected option.
    pub checked_icon: &'a str,
    /// Icon for an unselected option.
    pub unchecked_icon: &'a str,
    pub style: MultipleChoiceStyle,
}

impl<'a> MultipleChoiceWidget<'a> {
    /// Construct with sensible defaults.
    pub fn new(
        label: &'a str,
        options: &'a [ChoiceOption<'a>],
        focused: bool,
        cursor: usize,
        style: MultipleChoiceStyle,
    ) -> Self {
        Self {
            label,
            options,
            focused,
            cursor,
            nav_hint: "",
            checked_icon: "● ",
            unchecked_icon: "○ ",
            style,
        }
    }

    /// Render into `area` and return the next `y` position after this widget.
    pub fn render_and_next_y(self, area: Rect, buf: &mut Buffer) -> u16 {
        let next_y = area.y + 2;
        Widget::render(self, area, buf);
        next_y
    }
}

impl Widget for MultipleChoiceWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 2 || area.width == 0 {
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

        let mut label_spans = vec![
            Span::styled(prefix, Style::default().fg(label_fg)),
            Span::styled(
                self.label,
                Style::default().fg(label_fg).add_modifier(label_modifier),
            ),
        ];
        if !self.nav_hint.is_empty() {
            label_spans.push(Span::styled(
                format!("  {}", self.nav_hint),
                Style::default().fg(s.hint_fg),
            ));
        }

        Paragraph::new(Line::from(label_spans)).render(
            Rect { x, y, width, height: 1 },
            buf,
        );

        // ── Options row ───────────────────────────────────────────────────
        let opts_y = y + 1;
        if opts_y >= area.bottom() {
            return;
        }

        let mut rx = x + 2u16;

        for (idx, opt) in self.options.iter().enumerate() {
            let is_cursor = self.focused && self.cursor == idx;

            let icon = if opt.selected { self.checked_icon } else { self.unchecked_icon };
            let icon_fg = if opt.selected { s.checked_fg } else { s.unchecked_fg };

            let (text_fg, text_bg, bold) = if is_cursor {
                (s.cursor_text_fg, s.cursor_bg, true)
            } else {
                (s.item_idle_fg, s.item_idle_bg, false)
            };

            let icon_style = Style::default().fg(icon_fg).bg(s.item_idle_bg);
            let label_style = Style::default()
                .fg(text_fg)
                .bg(text_bg)
                .add_modifier(if bold { Modifier::BOLD } else { Modifier::empty() });

            let label_str = format!("{} ", opt.label);

            // Write icon chars
            for (ci, ch) in icon.chars().enumerate() {
                let cx = rx + ci as u16;
                if cx >= x + width {
                    return;
                }
                if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(cx, opts_y)) {
                    cell.set_char(ch);
                    cell.set_style(icon_style);
                }
            }

            // Write label chars (leading space included in highlight)
            let icon_len = icon.chars().count() as u16;

            // Leading space — highlighted with cursor background
            let cx = rx + icon_len;
            if cx >= x + width {
                return;
            }
            if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(cx, opts_y)) {
                cell.set_char(' ');
                cell.set_style(label_style);
            }

            for (li, ch) in label_str.chars().enumerate() {
                let cx = rx + icon_len + 1 + li as u16;
                if cx >= x + width {
                    return;
                }
                if let Some(cell) = buf.cell_mut(ratatui::layout::Position::new(cx, opts_y)) {
                    cell.set_char(ch);
                    cell.set_style(label_style);
                }
            }

            rx += icon_len + 1 + label_str.chars().count() as u16 + 2;
        }
    }
}
