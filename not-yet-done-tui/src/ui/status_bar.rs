use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Widget,
};

use crate::app::App;
use crate::config::Action;
use crate::ui::theme::Theme;

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
        let quit_key  = self.app.keybindings.label(&Action::Quit);
        let next_key  = self.app.keybindings.label(&Action::TabNext);
        let prev_key  = self.app.keybindings.label(&Action::TabPrev);

        let spans = vec![
            Span::styled(" ", Style::default().bg(Theme::SURFACE)),
            Span::styled(quit_key,  Style::default().fg(Theme::ACCENT).bg(Theme::SURFACE)),
            Span::styled(" quit  ", Style::default().fg(Theme::TEXT_MED).bg(Theme::SURFACE)),
            Span::styled(next_key,  Style::default().fg(Theme::ACCENT).bg(Theme::SURFACE)),
            Span::styled("/",       Style::default().fg(Theme::TEXT_DIM).bg(Theme::SURFACE)),
            Span::styled(prev_key,  Style::default().fg(Theme::ACCENT).bg(Theme::SURFACE)),
            Span::styled(" cycle tabs ", Style::default().fg(Theme::TEXT_MED).bg(Theme::SURFACE)),
        ];

        Line::from(spans).render(area, buf);

        // Fill remainder
        // (ratatui Line::render fills its area, so background is set via spans)
    }
}
