use ratatui::style::{Color, Style};

/// Enum für die verschiedenen Zustände eines MultiChoice-Eintrags.
#[repr(u8)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiChoiceStyleType {
    /// Titelzeile
    Title = 0,
    /// Nicht ausgewählt, nicht aktiv (collapsed)
    Normal = 1,
    /// Ausgewählt (collapsed)
    Active = 2,
    /// Nicht ausgewählt (expanded)
    Selected = 3,
    /// Ausgewählt (expanded)
    SelectedActive = 4,
    /// Line after MC
    LastLine = 5,
}

impl MultiChoiceStyleType {
    const COUNT: usize = 6;
}

/// Styling-Konfiguration für das MultiChoice-Widget.
#[derive(Debug, Clone)]
pub struct MultiChoiceStyle {
    /// Farbe des Prefix-Balkens (`▍ `)
    pub prefix_color: Option<Color>,
    /// Linienstile je Zustand
    pub styles: [Style; MultiChoiceStyleType::COUNT],
}

impl Default for MultiChoiceStyle {
    fn default() -> Self {
        Self {
            prefix_color: None,
            styles: core::array::from_fn(|_| Style::default()),
        }
    }
}

impl MultiChoiceStyle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn prefix_color(mut self, color: Color) -> Self {
        self.prefix_color = Some(color);
        self
    }

    pub fn set_style(mut self, style_type: MultiChoiceStyleType, style: Style) -> Self {
        self.styles[style_type as usize] = style;
        self
    }

    pub fn style(&self, style_type: MultiChoiceStyleType) -> &Style {
        &self.styles[style_type as usize]
    }
}
