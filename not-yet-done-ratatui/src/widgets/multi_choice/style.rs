use ratatui::style::Color;
use crate::widgets::common::LineStyle;

/// Styling-Konfiguration für das MultiChoice-Widget.
#[derive(Debug, Clone, Default)]
pub struct MultiChoiceStyle {
    /// Farbe des Prefix-Balkens (`▍ `)
    pub prefix_color:     Option<Color>,
    /// Stil der Titelzeile
    pub title_style:      LineStyle,
    /// Stil einer *nicht* ausgewählten Choice (collapsed + expanded)
    pub item_style:       LineStyle,
    /// Stil einer *ausgewählten* Choice (collapsed + expanded)
    pub item_selected_style: LineStyle,
}

impl MultiChoiceStyle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn prefix_color(mut self, color: Color) -> Self {
        self.prefix_color = Some(color);
        self
    }

    pub fn title_style(mut self, style: LineStyle) -> Self {
        self.title_style = style;
        self
    }

    pub fn item_style(mut self, style: LineStyle) -> Self {
        self.item_style = style;
        self
    }

    pub fn item_selected_style(mut self, style: LineStyle) -> Self {
        self.item_selected_style = style;
        self
    }
}
