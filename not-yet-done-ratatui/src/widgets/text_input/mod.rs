pub mod keymap;
pub mod state;
pub mod style;

pub use keymap::{KeyBinding, TextInputKeymap};
pub use state::{TextInputEvent, TextInputState};
pub use style::{hex_color, CursorShape, CursorStyle, LineStyle, TextInputStyle};

use ratatui::crossterm::execute;
use ratatui::crossterm::cursor::{
    SetCursorStyle as CrosstermCursorStyle,
};

pub fn apply_cursor_style(cursor: &CursorStyle) -> std::io::Result<()> {
    let ct_style = match (&cursor.shape, cursor.blinking) {
        (CursorShape::Block,     true)  => CrosstermCursorStyle::BlinkingBlock,
        (CursorShape::Block,     false) => CrosstermCursorStyle::SteadyBlock,
        (CursorShape::Bar,       true)  => CrosstermCursorStyle::BlinkingBar,
        (CursorShape::Bar,       false) => CrosstermCursorStyle::SteadyBar,
        (CursorShape::Underline, true)  => CrosstermCursorStyle::BlinkingUnderScore,
        (CursorShape::Underline, false) => CrosstermCursorStyle::SteadyUnderScore,
    };
    execute!(std::io::stdout(), ct_style)
}

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget,
};

const PREFIX: &str = "▍ ";
const PREFIX_LEN: u16 = 2;

#[derive(Debug, Clone)]
pub struct TextInput<'a> {
    pub title: &'a str,
    pub placeholder: &'a str,
    pub width: Option<u16>,
    pub style: TextInputStyle,
    pub keymap: TextInputKeymap,
}

impl<'a> TextInput<'a> {
    pub fn new(title: &'a str) -> Self {
        Self {
            title,
            placeholder: "",
            width: None,
            style: TextInputStyle::default(),
            keymap: TextInputKeymap::default(),
        }
    }

    pub fn placeholder(mut self, text: &'a str) -> Self {
        self.placeholder = text;
        self
    }

    pub fn width(mut self, w: u16) -> Self {
        self.width = Some(w);
        self
    }

    pub fn style(mut self, s: TextInputStyle) -> Self {
        self.style = s;
        self
    }

    pub fn keymap(mut self, km: TextInputKeymap) -> Self {
        self.keymap = km;
        self
    }

    pub fn render_with_state(self, area: Rect, buf: &mut Buffer, state: &TextInputState) {
        let total_width = self.width.unwrap_or(area.width);
        let text_width = total_width.saturating_sub(PREFIX_LEN) as usize;

        // Zeile 0: Titel
        render_prefixed_line(
            buf,
            area.x, area.y,
            total_width,
            self.title,
            text_width,
            &self.style.prefix_color,
            &self.style.title_style,
        );

        // Zeile 1: Eingabe — Placeholder bekommt eigene Farbe
        let input_text = if state.value.is_empty() {
            self.placeholder
        } else {
            &state.value
        };

        let effective_input_style = if state.value.is_empty() {
            if let Some(ph_color) = self.style.placeholder_color {
                let mut s = self.style.input_style.clone();
                s.fg = Some(ph_color);
                s
            } else {
                self.style.input_style.clone()
            }
        } else {
            self.style.input_style.clone()
        };

        render_prefixed_line(
            buf,
            area.x, area.y + 1,
            total_width,
            input_text,
            text_width,
            &self.style.prefix_color,
            &effective_input_style,
        );

        // Zeile 2: Fehler
        if area.height > 2 {
            let error_text = match &state.error {
                Some(e) => format!("  ⚠ {}", e),
                None => String::new(),
            };
            render_plain_line(
                buf,
                area.x, area.y + 2,
                total_width,
                &error_text,
                &self.style.error_style,
            );
        }
    }

    /// Gibt die Terminal-Cursor-Position zurück — vom Aufrufer per
    /// `frame.set_cursor_position()` zu setzen, wenn das Feld aktiv ist.
    pub fn cursor_position(&self, area: Rect, state: &TextInputState) -> (u16, u16) {
        let chars_before = state.value[..state.cursor].chars().count();
        let total_width = self.width.unwrap_or(area.width);
        let max_x = area.x + total_width - 1;
        let x = (area.x + PREFIX_LEN + chars_before as u16).min(max_x);
        (x, area.y + 1)
    }
}

impl Widget for TextInput<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let state = TextInputState::default();
        self.render_with_state(area, buf, &state);
    }
}

fn render_prefixed_line(
    buf: &mut Buffer,
    x: u16, y: u16,
    total_width: u16,
    text: &str,
    text_width: usize,
    prefix_color: &Option<Color>,
    line_style: &LineStyle,
) {
    // Hintergrund über gesamte Breite
    if let Some(bg) = line_style.bg {
        for dx in 0..total_width {
            if let Some(cell) = buf.cell_mut((x + dx, y)) {
                cell.set_bg(bg);
            }
        }
    }

    // Prefix
    let mut px = x;
    for ch in PREFIX.chars() {
        if let Some(cell) = buf.cell_mut((px, y)) {
            let mut s = Style::default();
            if let Some(fg) = prefix_color {
                s = s.fg(*fg);
            }
            if let Some(bg) = line_style.bg {
                s = s.bg(bg);
            }
            cell.set_char(ch);
            cell.set_style(s);
        }
        px += 1;
    }

    // Text
    for ch in truncate_to_width(text, text_width).chars() {
        if let Some(cell) = buf.cell_mut((px, y)) {
            let mut s = Style::default();
            if let Some(fg) = line_style.fg {
                s = s.fg(fg);
            }
            if let Some(bg) = line_style.bg {
                s = s.bg(bg);
            }
            cell.set_char(ch);
            cell.set_style(s);
        }
        px += 1;
    }
}

fn render_plain_line(
    buf: &mut Buffer,
    x: u16, y: u16,
    total_width: u16,
    text: &str,
    line_style: &LineStyle,
) {
    if let Some(bg) = line_style.bg {
        for dx in 0..total_width {
            if let Some(cell) = buf.cell_mut((x + dx, y)) {
                cell.set_bg(bg);
            }
        }
    }

    let mut px = x;
    for ch in truncate_to_width(text, total_width as usize).chars() {
        if let Some(cell) = buf.cell_mut((px, y)) {
            let mut s = Style::default();
            if let Some(fg) = line_style.fg {
                s = s.fg(fg);
            }
            if let Some(bg) = line_style.bg {
                s = s.bg(bg);
            }
            cell.set_char(ch);
            cell.set_style(s);
        }
        px += 1;
    }
}

fn truncate_to_width(s: &str, max_chars: usize) -> &str {
    let mut count = 0;
    let mut byte_pos = 0;
    for ch in s.chars() {
        if count >= max_chars {
            break;
        }
        count += 1;
        byte_pos += ch.len_utf8();
    }
    &s[..byte_pos]
}
