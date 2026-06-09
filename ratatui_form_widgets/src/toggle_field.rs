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

/// All colors needed to render a `ToggleFieldWidget`.
///
/// # Example (in your theme adapter)
/// ```rust
/// ToggleFieldStyle {
///     label_focused:  theme.primary(),
///     label_idle:     theme.text_dim(),
///     checked_fg:     theme.success(),
///     unchecked_fg:   theme.text_dim(),
///     hint_fg:        theme.text_dim(),
///     focused_bg:      theme.focused_bg(),
/// }
/// ```
#[derive(Debug, Clone, Copy)]
pub struct ToggleFieldStyle {
    pub label_focused: ratatui::style::Color,
    pub label_idle: ratatui::style::Color,
    pub checked_fg: ratatui::style::Color,
    pub unchecked_fg: ratatui::style::Color,
    /// Color for secondary hint text (e.g. "space to toggle").
    pub hint_fg: ratatui::style::Color,
    /// Background for focused field (header + toggle).
    pub focused_bg: ratatui::style::Color,
}

// ---------------------------------------------------------------------------
// Widget
// ---------------------------------------------------------------------------

/// A single boolean toggle field with label and keyboard hint.
///
/// Occupies exactly **1 row**.
///
/// # Layout
/// ```text
///   󰄵  Label text  space to toggle
/// ▍ 󰄰  Label text  space to toggle   ← when focused
/// ```
pub struct ToggleFieldWidget<'a> {
    pub label: &'a str,
    pub value: bool,
    pub focused: bool,
    /// Icon rendered when to toggle is `true`. Defaults to `"󰄵 "`.
    pub checked_icon: &'a str,
    /// Icon rendered when to toggle is `false`. Defaults to `"󰄰 "`.
    pub unchecked_icon: &'a str,
    /// Optional hint shown after to label. Pass `""` to hide.
    pub hint: &'a str,
    pub style: ToggleFieldStyle,
}

impl<'a> ToggleFieldWidget<'a> {
    /// Construct with sensible Nerd Font defaults.
    pub fn new(label: &'a str, value: bool, focused: bool, style: ToggleFieldStyle) -> Self {
        Self {
            label,
            value,
            focused,
            checked_icon: "󰄵  ",
            unchecked_icon: "☐  ",
            hint: "",
            style,
        }
    }
}

impl Widget for ToggleFieldWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let s = &self.style;
        let label_fg = s.label_focused;
        let prefix = " ▍ ";
        let icon = if self.value {
            self.checked_icon
        } else {
            self.unchecked_icon
        };
        let icon_fg = if self.value {
            s.checked_fg
        } else {
            s.unchecked_fg
        };
        let label_modifier = Modifier::BOLD;

        let mut spans = vec![
            Span::styled(prefix, Style::default().fg(label_fg)),
            Span::styled(icon, Style::default().fg(icon_fg)),
            Span::styled(
                self.label,
                Style::default().fg(label_fg).add_modifier(label_modifier),
            ),
        ];

        if !self.hint.is_empty() {
            spans.push(Span::styled(
                format!("  {}", self.hint),
                Style::default().fg(s.hint_fg),
            ));
        }

        Paragraph::new(Line::from(spans)).render(area, buf);
    }
}
