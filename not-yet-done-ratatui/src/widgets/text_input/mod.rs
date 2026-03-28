pub mod keymap;
pub mod state;
pub mod style;

pub use keymap::TextInputKeymap;
pub use state::{TextInputEvent, TextInputState};
pub use style::{TextInputStyle, TextInputStyleType};

use crate::widgets::common::{render_prefixed_line, truncate_to_width, PREFIX_LEN};
use unicode_width::UnicodeWidthChar;

use ratatui::{buffer::Buffer, layout::Rect, style::Style, widgets::Widget};

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

        // Row 0: title
        let title_style = self.style.resolved_style(TextInputStyleType::Title);
        render_prefixed_line(
            buf,
            area.x,
            area.y,
            total_width,
            self.title,
            text_width,
            &self.style.prefix_color,
            &title_style,
            false,
        );

        // Row 1: input (or placeholder when empty)
        let input_text = if state.value.is_empty() {
            self.placeholder
        } else {
            &state.value
        };

        let input_style = self.style.resolved_style(TextInputStyleType::Input);
        let effective_input_style = if state.value.is_empty() {
            if let Some(ph_color) = self.style.placeholder_color {
                input_style.fg(ph_color)
            } else {
                input_style
            }
        } else {
            input_style
        };

        render_prefixed_line(
            buf,
            area.x,
            area.y + 1,
            total_width,
            input_text,
            text_width,
            &self.style.prefix_color,
            &effective_input_style,
            false,
        );

        // Row 2: error
        if area.height > 2 {
            let error_text = match &state.error {
                Some(e) => format!("  ⚠ {}", e),
                None => String::new(),
            };
            let error_style = self.style.resolved_style(TextInputStyleType::Error);
            render_plain_line(buf, area.x, area.y + 2, total_width, &error_text, &error_style);
        }
    }

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

/// Renders a plain line without a prefix — used for the error row of `TextInput`.
fn render_plain_line(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    total_width: u16,
    text: &str,
    line_style: &Style,
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
        px += ch.width().unwrap_or(1) as u16;
    }
}
