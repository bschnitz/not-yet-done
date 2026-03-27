use ratatui::style::{Color, Style};

/// Enum für die verschiedenen Zustände eines TextInput-Elements.
#[repr(u8)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextInputStyleType {
    /// Titelzeile
    Title = 0,
    /// Eingabezeile
    Input = 1,
    /// Fehlerzeile
    Error = 2,
}

impl TextInputStyleType {
    const COUNT: usize = 3;
}

/// Styling-Konfiguration für das TextInput-Widget.
#[derive(Debug, Clone)]
pub struct TextInputStyle {
    /// Farbe des Prefix-Balkens (`▍ `)
    pub prefix_color: Option<Color>,
    /// Farben je Zustand
    pub styles: [Style; TextInputStyleType::COUNT],
    /// Placeholder-Farbe
    pub placeholder_color: Option<Color>,
}

impl Default for TextInputStyle {
    fn default() -> Self {
        Self {
            prefix_color: None,
            styles: core::array::from_fn(|_| Style::default()),
            placeholder_color: None,
        }
    }
}

impl TextInputStyle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn prefix_color(mut self, color: Color) -> Self {
        self.prefix_color = Some(color);
        self
    }

    pub fn set_style(mut self, style_type: TextInputStyleType, style: Style) -> Self {
        self.styles[style_type as usize] = style;
        self
    }

    pub fn style(&self, style_type: TextInputStyleType) -> &Style {
        &self.styles[style_type as usize]
    }

    pub fn placeholder_color(mut self, color: Color) -> Self {
        self.placeholder_color = Some(color);
        self
    }
}
