use crate::widgets::common::impl_widget_style_base;
use ratatui::style::{Color, Style};

/// Identifies the visual part of a [`super::Toggle`] to be styled.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleStyleType {
    /// Title line
    Title = 0,
    /// Value line (`[x] Yes` / `[ ] No`)
    Value = 1,
}

/// Styling configuration for the [`super::Toggle`] widget.
///
/// Every slot is `Option<Style>`: `None` means "not configured" and lets an
/// outer form inject a fallback. Inside render code use `resolved_style()`,
/// which falls back to `Style::default()`.
#[derive(Debug, Clone)]
pub struct ToggleStyle {
    /// Colour of the prefix bar (`▍ `).
    pub prefix_color: Option<Color>,
    /// Per-slot styles — indexed by `ToggleStyleType as usize`.
    pub styles: [Option<Style>; 2],
}

impl Default for ToggleStyle {
    fn default() -> Self {
        Self {
            prefix_color: None,
            styles: [None; 2],
        }
    }
}

impl ToggleStyle {
    pub fn new() -> Self {
        Self::default()
    }
}

// Generates: prefix_color(), set_style(), style(), resolved_style()
impl_widget_style_base!(ToggleStyle, ToggleStyleType);
