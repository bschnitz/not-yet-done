use ratatui::style::Color;
use crate::widgets::common::LineStyle;

#[derive(Debug, Clone, Default)]
pub struct TextInputStyle {
    pub prefix_color:      Option<Color>,
    pub title_style:       LineStyle,
    pub input_style:       LineStyle,
    pub error_style:       LineStyle,
    pub placeholder_color: Option<Color>,
}

impl TextInputStyle {
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

    pub fn input_style(mut self, style: LineStyle) -> Self {
        self.input_style = style;
        self
    }

    pub fn error_style(mut self, style: LineStyle) -> Self {
        self.error_style = style;
        self
    }

    pub fn placeholder_color(mut self, color: Color) -> Self {
        self.placeholder_color = Some(color);
        self
    }
}
