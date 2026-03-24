use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Widget,
};

use crate::app::App;
use crate::config::{GlobalAction, TasksAction};
use crate::tabs::Tab;

pub struct StatusBar<'a> {
    app: &'a App,
}

impl<'a> StatusBar<'a> {
    pub fn new(app: &'a App) -> Self { Self { app } }
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let t    = &self.app.theme;
        let gkb  = &self.app.keybindings.global;
        let tkb  = &self.app.keybindings.tasks;
        let ts   = &self.app.tasks_state;

        let mut spans = vec![
            Span::styled(" ", Style::default().bg(t.surface())),
            Span::styled(gkb.label(&GlobalAction::Quit), Style::default().fg(t.accent()).bg(t.surface())),
            Span::styled(" quit  ",                      Style::default().fg(t.text_med()).bg(t.surface())),
        ];

        if self.app.active_tab == Tab::Tasks && ts.form_visible() {
            // Form is open — show close hint, suppress tab-switch hints
            spans.push(Span::styled(tkb.label(&TasksAction::FormClose), Style::default().fg(t.accent()).bg(t.surface())));
            spans.push(Span::styled(" close form  ",                     Style::default().fg(t.text_med()).bg(t.surface())));
            spans.push(Span::styled("(tab switch locked while form is open)", Style::default().fg(t.text_dim()).bg(t.surface())));
        } else {
            // Normal — show tab cycling hints
            spans.push(Span::styled(gkb.label(&GlobalAction::TabNext), Style::default().fg(t.accent()).bg(t.surface())));
            spans.push(Span::styled("/",                                Style::default().fg(t.text_dim()).bg(t.surface())));
            spans.push(Span::styled(gkb.label(&GlobalAction::TabPrev), Style::default().fg(t.accent()).bg(t.surface())));
            spans.push(Span::styled(" cycle tabs",                      Style::default().fg(t.text_med()).bg(t.surface())));
        }

        Line::from(spans).render(area, buf);
    }
}
