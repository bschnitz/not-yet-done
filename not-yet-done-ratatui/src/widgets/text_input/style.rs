use crate::widgets::common::impl_widget_style_base;
use ratatui::style::{Color, Style};

/// Identifies the visual part of a `TextInput` to be styled.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextInputStyleType {
    /// Title line
    Title = 0,
    /// Input line
    Input = 1,
    /// Error line
    Error = 2,
}

/// Styling configuration for the `TextInput` widget.
///
/// Every slot is `Option<Style>`: `None` means "not configured" and allows an
/// outer form to inject a fallback.  Inside widget render code use
/// `resolved_style()` which falls back to `Style::default()`.
#[derive(Debug, Clone)]
pub struct TextInputStyle {
    /// Colour of the prefix bar (`▍ `).
    pub prefix_color: Option<Color>,
    /// Per-slot styles — indexed by `TextInputStyleType as usize`.
    pub styles: [Option<Style>; 3],
    /// Foreground colour used for placeholder text when the field is empty.
    pub placeholder_color: Option<Color>,
}

impl Default for TextInputStyle {
    fn default() -> Self {
        Self {
            prefix_color: None,
            styles: [None; 3],
            placeholder_color: None,
        }
    }
}

impl TextInputStyle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn placeholder_color(mut self, color: Color) -> Self {
        self.placeholder_color = Some(color);
        self
    }
}

// Generates: prefix_color(), set_style(), style(), resolved_style()
impl_widget_style_base!(TextInputStyle, TextInputStyleType);
