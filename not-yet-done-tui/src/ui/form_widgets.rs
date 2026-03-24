// not-yet-done-tui/src/ui/form_widgets.rs
//
// Theme adapter — translates `Theme` into the style types expected by
// `ratatui-form-widgets`.  Keep all widget-crate imports here so the rest
// of the TUI crate never needs to know about `TextFieldStyle` etc.

use ratatui_form_widgets::{MultipleChoiceStyle, TextFieldStyle, ToggleFieldStyle};

use crate::ui::theme::Theme;

impl Theme {
    /// Style for a [`TextFieldWidget`].
    pub fn text_field_style(&self) -> TextFieldStyle {
        TextFieldStyle {
            label_focused:  self.primary(),
            label_idle:     self.text_dim(),
            input_focused:  self.text_high(),
            input_idle:     self.text_med(),
            cursor_fg:      self.bg(),
            cursor_bg:      self.primary(),
            error_fg:       self.error(),
            placeholder_fg: self.text_dim(),
            input_bg:       self.surface(),
        }
    }

    /// Style for a [`ToggleFieldWidget`].
    pub fn toggle_field_style(&self) -> ToggleFieldStyle {
        ToggleFieldStyle {
            label_focused: self.primary(),
            label_idle:    self.text_dim(),
            checked_fg:    self.success(),
            unchecked_fg:  self.text_dim(),
            hint_fg:       self.text_dim(),
        }
    }

    /// Style for a [`MultipleChoiceWidget`].
    pub fn multiple_choice_style(&self) -> MultipleChoiceStyle {
        MultipleChoiceStyle {
            label_focused:  self.primary(),
            label_idle:     self.text_dim(),
            checked_fg:     self.primary(),
            unchecked_fg:   self.text_dim(),
            cursor_text_fg: self.on_primary(),
            cursor_bg:      self.primary(),
            item_idle_fg:   self.text_med(),
            item_idle_bg:   self.surface_2(),
            hint_fg:        self.text_dim(),
        }
    }
}
