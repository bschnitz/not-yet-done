use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Widget,
};

use crate::app::App;
use crate::config::Action;

pub struct StatusBar<'a> {
    app: &'a App,
}

impl<'a> StatusBar<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let t = &self.app.theme;
        let quit_key = self.app.keybindings.label(&Action::Quit);
        let next_key = self.app.keybindings.label(&Action::TabNext);
        let prev_key = self.app.keybindings.label(&Action::TabPrev);

        let spans = vec![
            Span::styled(" ",              Style::default().bg(t.surface())),
            Span::styled(quit_key,         Style::default().fg(t.accent()).bg(t.surface())),
            Span::styled(" quit  ",        Style::default().fg(t.text_med()).bg(t.surface())),
            Span::styled(next_key,         Style::default().fg(t.accent()).bg(t.surface())),
            Span::styled("/",              Style::default().fg(t.text_dim()).bg(t.surface())),
            Span::styled(prev_key,         Style::default().fg(t.accent()).bg(t.surface())),
            Span::styled(" cycle tabs ",   Style::default().fg(t.text_med()).bg(t.surface())),
        ];

        Line::from(spans).render(area, buf);
    }
}
