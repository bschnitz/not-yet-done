use ratatui::style::Color;

/// Material Design-inspired dark theme
pub struct Theme;

impl Theme {
    // Surface colours
    pub const BG:          Color = Color::Rgb(18,  18,  18);   // #121212 — Material surface
    pub const SURFACE:     Color = Color::Rgb(30,  30,  30);   // #1e1e1e — elevated surface
    pub const SURFACE_2:   Color = Color::Rgb(40,  40,  40);   // #282828 — card surface

    // Primary — deep teal (Material Teal 300 / 200)
    pub const PRIMARY:     Color = Color::Rgb(77,  182, 172);  // #4db6ac — teal 300
    pub const PRIMARY_DIM: Color = Color::Rgb(38,  166, 154);  // #26a69a — teal 400
    pub const ON_PRIMARY:  Color = Color::Rgb(0,   37,  34);   // #002522 — text on primary

    // Secondary — amber accent
    pub const ACCENT:      Color = Color::Rgb(255, 202, 40);   // #ffca28 — amber 400
    pub const ACCENT_DIM:  Color = Color::Rgb(255, 179, 0);    // #ffb300 — amber 600

    // Text
    pub const TEXT_HIGH:   Color = Color::Rgb(230, 230, 230);  // primary text
    pub const TEXT_MED:    Color = Color::Rgb(158, 158, 158);  // secondary text (Grey 500)
    pub const TEXT_DIM:    Color = Color::Rgb(97,  97,  97);   // disabled text (Grey 600)

    // Status colours
    pub const SUCCESS:     Color = Color::Rgb(102, 187, 106);  // green 400
    pub const ERROR:       Color = Color::Rgb(239, 83,  80);   // red 400
    pub const WARNING:     Color = Color::Rgb(255, 167, 38);   // orange 400
}
