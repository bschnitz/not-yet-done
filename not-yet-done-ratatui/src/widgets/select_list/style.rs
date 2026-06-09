use crate::widgets::common::style::impl_widget_style_base;
use ratatui::style::Color;

/// Visual slots for [`SelectList`].
#[derive(Debug, Clone, Copy)]
pub enum SelectListStyleType {
    /// Normal item text.
    Item = 0,
    /// Item text when selected.
    ItemSelected = 1,
    /// Item text when cursor is on it.
    ItemCursor = 2,
    /// Item text when both selected and cursor.
    ItemCursorSelected = 3,
    /// Group header text.
    GroupHeader = 4,
    /// Description / secondary text.
    Description = 5,
    /// Filter input text.
    FilterInput = 6,
    /// Filter input cursor.
    FilterCursor = 7,
    /// Footer / status text.
    Footer = 8,
}

/// Styling configuration for [`SelectList`].
#[derive(Debug, Clone)]
pub struct SelectListStyle {
    pub prefix_color: Option<Color>,
    pub styles: [Option<ratatui::style::Style>; 9],
    /// Foreground colour used for the filter row's placeholder
    /// (`type to filter…`) when the filter is empty. The bg comes from
    /// the [`SelectListStyleType::FilterInput`] slot so the row stays
    /// visually flush.
    pub placeholder_color: Option<Color>,
}

impl Default for SelectListStyle {
    fn default() -> Self {
        Self {
            prefix_color: None,
            styles: [None; 9],
            placeholder_color: None,
        }
    }
}

impl SelectListStyle {
    /// Sets the foreground colour used for the filter placeholder text.
    pub fn placeholder_color(mut self, color: Color) -> Self {
        self.placeholder_color = Some(color);
        self
    }
}

impl_widget_style_base!(SelectListStyle, SelectListStyleType);
