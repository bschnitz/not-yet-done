use ratatui::style::Color;

/// "#RRGGBB" → Color::Rgb
pub fn hex_color(hex: &str) -> Color {
    let hex = hex.trim_start_matches('#');
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
    Color::Rgb(r, g, b)
}

/// Generates common builder methods for widget style structs.
///
/// Requirements for the struct:
///   - `pub prefix_color: Option<Color>`
///   - `pub styles: [Option<Style>; N]`
///
/// Generated methods:
///   - `prefix_color(Color) -> Self`
///   - `set_style(StyleType, Style) -> Self`
///   - `style(StyleType) -> Option<&Style>` — `None` if not explicitly configured
///   - `resolved_style(StyleType) -> Style`  — falls back to `Style::default()`
macro_rules! impl_widget_style_base {
    ($T:ty, $ST:ty) => {
        impl $T {
            pub fn prefix_color(mut self, color: ::ratatui::style::Color) -> Self {
                self.prefix_color = Some(color);
                self
            }

            pub fn set_style(mut self, style_type: $ST, style: ::ratatui::style::Style) -> Self {
                self.styles[style_type as usize] = Some(style);
                self
            }

            /// Returns `Some(&style)` if this slot was explicitly configured, `None` otherwise.
            /// Useful for the form layer to decide whether to apply a fallback.
            pub fn style(&self, style_type: $ST) -> Option<&::ratatui::style::Style> {
                self.styles[style_type as usize].as_ref()
            }

            /// Returns the configured style or `Style::default()` as fallback.
            /// Use this inside widget render code.
            pub fn resolved_style(&self, style_type: $ST) -> ::ratatui::style::Style {
                self.styles[style_type as usize].unwrap_or_default()
            }
        }
    };
}

pub(crate) use impl_widget_style_base;
