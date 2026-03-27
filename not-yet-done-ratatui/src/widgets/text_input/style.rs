use ratatui::style::Color;

#[derive(Debug, Clone, Default)]
pub struct LineStyle {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
}

impl LineStyle {
    pub fn fg(mut self, color: Color) -> Self {
        self.fg = Some(color);
        self
    }

    pub fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }
}

#[derive(Debug, Clone)]
pub enum CursorShape {
    Block,      // █
    Bar,        // |
    Underline,  // _
}

#[derive(Debug, Clone)]
pub struct CursorStyle {
    pub shape: CursorShape,
    pub blinking: bool,
}

impl Default for CursorStyle {
    fn default() -> Self {
        Self {
            shape: CursorShape::Bar,
            blinking: true,
        }
    }
}

impl CursorStyle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn shape(mut self, shape: CursorShape) -> Self {
        self.shape = shape;
        self
    }

    pub fn blinking(mut self, blinking: bool) -> Self {
        self.blinking = blinking;
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct TextInputStyle {
    pub prefix_color:      Option<Color>,
    pub title_style:       LineStyle,
    pub input_style:       LineStyle,
    pub error_style:       LineStyle,
    pub placeholder_color: Option<Color>,
    pub cursor:            Option<CursorStyle>,  // None = kein Cursor
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

    pub fn placeholder_color(mut self, color: Color) -> Self {   // ← neu
        self.placeholder_color = Some(color);
        self
    }

    pub fn cursor(mut self, style: CursorStyle) -> Self {
        self.cursor = Some(style);
        self
    }

    pub fn no_cursor(mut self) -> Self {
        self.cursor = None;
        self
    }
}

/// "#RRGGBB" → Color::Rgb
pub fn hex_color(hex: &str) -> Color {
    let hex = hex.trim_start_matches('#');
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
    Color::Rgb(r, g, b)
}
