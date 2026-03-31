use crate::widgets::common::impl_widget_style_base;
use ratatui::style::{Color, Style};

/// Identifies the visual part of a `MultiChoice` entry to be styled.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiChoiceStyleType {
    /// Title line
    Title = 0,
    /// Item: not selected, cursor is elsewhere
    Normal = 1,
    /// Item: not selected, cursor is on this row (keyboard focus)
    Active = 2,
    /// Item: selected, cursor is elsewhere
    Selected = 3,
    /// Item: selected and cursor is on this row
    SelectedActive = 4,
    /// Empty closing line rendered below the expanded list
    LastLine = 5,
}

/// Styling configuration for the `MultiChoice` widget.
///
/// Every slot is `Option<Style>`: `None` means "not configured" and allows an
/// outer form to inject a fallback.  Inside widget render code use
/// `resolved_style()` which falls back to `Style::default()`.
#[derive(Debug, Clone)]
pub struct MultiChoiceStyle {
    /// Colour of the prefix bar (`▍ `).
    pub prefix_color: Option<Color>,
    /// Per-slot styles — indexed by `MultiChoiceStyleType as usize`.
    pub styles: [Option<Style>; 6],
}

impl Default for MultiChoiceStyle {
    fn default() -> Self {
        Self {
            prefix_color: None,
            styles: [None; 6],
        }
    }
}

impl MultiChoiceStyle {
    pub fn new() -> Self {
        Self::default()
    }
}

// Generates: prefix_color(), set_style(), style(), resolved_style()
impl_widget_style_base!(MultiChoiceStyle, MultiChoiceStyleType);
