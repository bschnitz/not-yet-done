use ratatui::style::{Color, Modifier, Style};

use crate::widgets::multi_choice::{MultiChoiceStyle, MultiChoiceStyleType};
use crate::widgets::select_list::{SelectListStyle, SelectListStyleType};
use crate::widgets::text_input::{TextInputStyle, TextInputStyleType};
use crate::widgets::toggle::{ToggleStyle, ToggleStyleType};

/// A small colour palette from which a full [`FormStyle`] is derived.
///
/// Consumers usually build one of these from their theme and call
/// [`FormStyle::from_palette`], rather than assembling every widget style by
/// hand.
#[derive(Debug, Clone, Copy)]
pub struct FormPalette {
    /// Focused labels, titles and the prefix bar.
    pub accent: Color,
    /// Unfocused labels.
    pub label_idle: Color,
    /// Focused input text.
    pub text: Color,
    /// Unfocused input text and select options.
    pub text_idle: Color,
    /// Placeholder text.
    pub placeholder: Color,
    /// Selected option / checked toggle marker.
    pub selected: Color,
    /// Footer and date-time preview text.
    pub hint: Color,
    /// Error text.
    pub error: Color,
    /// Fill behind the *focused* field (title + input bar). `None` → no bar,
    /// preserving the classic borderless look. Only takes effect when the
    /// form's [`FormOptions::field_bar`](super::FormOptions) is enabled.
    pub field_bg: Option<Color>,
    /// Fill behind *unfocused* fields. Usually `None` (only the focused field
    /// gets a bar), but a subtle idle fill can be set for an always-boxed look.
    pub field_bg_idle: Option<Color>,
    /// Fill behind the whole floating panel (the `event_form` look). Only used
    /// when the form's [`FormOptions::field_bar`](super::FormOptions) is enabled
    /// — that switches the driver from the classic bordered full-area block to a
    /// centred, borderless, content-sized panel. `None` → transparent panel.
    pub panel_bg: Option<Color>,
}

impl Default for FormPalette {
    fn default() -> Self {
        Self {
            accent: Color::Cyan,
            label_idle: Color::Gray,
            text: Color::White,
            text_idle: Color::Gray,
            placeholder: Color::DarkGray,
            selected: Color::Green,
            hint: Color::DarkGray,
            error: Color::Red,
            field_bg: None,
            field_bg_idle: None,
            panel_bg: None,
        }
    }
}

/// Fully-resolved styles for every widget a [`super::Form`] can host, plus the
/// form's own chrome (title, footer, error, preview).
///
/// Because the backing widgets are retained-mode and take their styles at
/// construction, the whole style is handed to [`super::Form::new`] up front.
#[derive(Debug, Clone)]
pub struct FormStyle {
    pub text_active: TextInputStyle,
    pub text_inactive: TextInputStyle,
    pub select_active: MultiChoiceStyle,
    pub select_inactive: MultiChoiceStyle,
    /// Inline (radio-list) select styles, used when the form's
    /// [`SelectStyle::Inline`](super::SelectStyle) is chosen.
    pub select_inline_active: SelectListStyle,
    pub select_inline_inactive: SelectListStyle,
    pub toggle_active: ToggleStyle,
    pub toggle_inactive: ToggleStyle,
    /// Style for the outer block title.
    pub title: Style,
    /// Style for the footer hint line.
    pub footer: Style,
    /// Style for the footer error line.
    pub error: Style,
    /// Colour of the date-time preview line.
    pub preview: Color,
    /// Resolved fill behind the focused field, if any. Exposed so the driver
    /// can paint the inline-select gutter stripe to match the text bars.
    pub field_bg: Option<Color>,
    /// Focused-label / prefix accent, exposed so the driver can hand-draw the
    /// inline-select label + `▍` gutter (the `SelectList` widget draws no title).
    pub accent: Color,
    /// Unfocused-label colour, for the same hand-drawn inline-select label.
    pub label_idle: Color,
    /// Colour picked options / checked toggles use, exposed for the panel's
    /// submit-bar accent.
    pub selected: Color,
    /// Fill behind the whole floating panel (panel/`event_form` chrome). `None`
    /// → transparent. See [`FormPalette::panel_bg`].
    pub panel_bg: Option<Color>,
    /// Style of the panel's "Save" submit bar (dark text on the accent fill).
    pub submit: Style,
}

/// Adds a background to `style` when `bg` is set, else returns it unchanged.
fn with_bg(style: Style, bg: Option<Color>) -> Style {
    match bg {
        Some(c) => style.bg(c),
        None => style,
    }
}

impl FormStyle {
    /// Derives a full set of widget and chrome styles from a compact palette.
    pub fn from_palette(p: &FormPalette) -> Self {
        let bold = Modifier::BOLD;

        // --- text ---
        // The focused field's title + input carry the optional filled bar; the
        // idle style carries the (usually absent) idle fill.
        let text_active = TextInputStyle::new()
            .prefix_color(p.accent)
            .set_style(
                TextInputStyleType::Title,
                with_bg(Style::default().fg(p.accent).add_modifier(bold), p.field_bg),
            )
            .set_style(
                TextInputStyleType::Input,
                with_bg(Style::default().fg(p.text), p.field_bg),
            )
            .set_style(TextInputStyleType::Error, Style::default().fg(p.error))
            .placeholder_color(p.placeholder);
        let text_inactive = TextInputStyle::new()
            // Match the inline-select gutter: an idle field's `▍` is label_idle,
            // not the widget's default colour (keeps all idle prefixes uniform).
            .prefix_color(p.label_idle)
            .set_style(
                TextInputStyleType::Title,
                with_bg(Style::default().fg(p.label_idle), p.field_bg_idle),
            )
            .set_style(
                TextInputStyleType::Input,
                with_bg(Style::default().fg(p.text_idle), p.field_bg_idle),
            )
            .set_style(TextInputStyleType::Error, Style::default().fg(p.error))
            .placeholder_color(p.placeholder);

        // --- select ---
        let select_active = MultiChoiceStyle::new()
            .prefix_color(p.accent)
            .set_style(
                MultiChoiceStyleType::Title,
                Style::default().fg(p.accent).add_modifier(bold),
            )
            .set_style(
                MultiChoiceStyleType::Normal,
                Style::default().fg(p.text_idle),
            )
            .set_style(
                MultiChoiceStyleType::Active,
                Style::default().fg(p.accent).add_modifier(bold),
            )
            .set_style(
                MultiChoiceStyleType::Selected,
                Style::default().fg(p.selected),
            )
            .set_style(
                MultiChoiceStyleType::SelectedActive,
                Style::default().fg(p.selected).add_modifier(bold),
            )
            .set_style(MultiChoiceStyleType::Footer, Style::default().fg(p.hint));
        let select_inactive = MultiChoiceStyle::new()
            .set_style(
                MultiChoiceStyleType::Title,
                Style::default().fg(p.label_idle),
            )
            .set_style(
                MultiChoiceStyleType::Normal,
                Style::default().fg(p.text_idle),
            )
            .set_style(
                MultiChoiceStyleType::SelectedActive,
                Style::default().fg(p.selected),
            );

        // --- inline select (radio list; the driver hand-draws the label + gutter) ---
        let select_inline_active = SelectListStyle::default()
            .set_style(
                SelectListStyleType::Item,
                with_bg(Style::default().fg(p.text_idle), p.field_bg),
            )
            .set_style(
                SelectListStyleType::ItemSelected,
                with_bg(
                    Style::default().fg(p.selected).add_modifier(bold),
                    p.field_bg,
                ),
            )
            .set_style(
                SelectListStyleType::ItemCursor,
                with_bg(Style::default().fg(p.accent).add_modifier(bold), p.field_bg),
            )
            .set_style(
                SelectListStyleType::ItemCursorSelected,
                with_bg(
                    Style::default().fg(p.selected).add_modifier(bold),
                    p.field_bg,
                ),
            );
        let select_inline_inactive = SelectListStyle::default()
            .set_style(SelectListStyleType::Item, Style::default().fg(p.text_idle))
            .set_style(
                SelectListStyleType::ItemSelected,
                Style::default().fg(p.selected),
            )
            .set_style(
                SelectListStyleType::ItemCursor,
                Style::default().fg(p.text_idle),
            )
            .set_style(
                SelectListStyleType::ItemCursorSelected,
                Style::default().fg(p.selected),
            );

        // --- toggle ---
        let toggle_active = ToggleStyle::new()
            .prefix_color(p.accent)
            .set_style(
                ToggleStyleType::Title,
                with_bg(Style::default().fg(p.accent).add_modifier(bold), p.field_bg),
            )
            .set_style(
                ToggleStyleType::Value,
                with_bg(Style::default().fg(p.text), p.field_bg),
            );
        let toggle_inactive = ToggleStyle::new()
            .prefix_color(p.label_idle)
            .set_style(
                ToggleStyleType::Title,
                with_bg(Style::default().fg(p.label_idle), p.field_bg_idle),
            )
            .set_style(
                ToggleStyleType::Value,
                with_bg(Style::default().fg(p.text_idle), p.field_bg_idle),
            );

        Self {
            text_active,
            text_inactive,
            select_active,
            select_inactive,
            select_inline_active,
            select_inline_inactive,
            toggle_active,
            toggle_inactive,
            title: Style::default().fg(p.accent).add_modifier(bold),
            footer: Style::default().fg(p.hint),
            error: Style::default().fg(p.error),
            preview: p.hint,
            field_bg: p.field_bg,
            accent: p.accent,
            label_idle: p.label_idle,
            selected: p.selected,
            panel_bg: p.panel_bg,
            // Dark text on the accent fill — a clear, legible button. When no
            // panel fill is configured, fall back to black text.
            submit: Style::default()
                .fg(p.panel_bg.unwrap_or(Color::Black))
                .bg(p.selected)
                .add_modifier(bold),
        }
    }
}

impl Default for FormStyle {
    fn default() -> Self {
        Self::from_palette(&FormPalette::default())
    }
}
