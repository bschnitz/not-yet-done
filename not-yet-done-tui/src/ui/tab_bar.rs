use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};

use crate::app::App;
use crate::config::Action;
use crate::tabs::Tab;

pub struct TabBar<'a> {
    app: &'a App,
}

impl<'a> TabBar<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for TabBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let t = &self.app.theme;

        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut(Position::new(x, area.top())) {
                cell.set_char(' ');
                cell.set_bg(t.surface());
            }
        }

        let mut x = area.left() + 1;

        for tab in Tab::ALL {
            let is_active = *tab == self.app.active_tab;

            let action = match tab {
                Tab::Welcome   => Action::TabWelcome,
                Tab::Tasks     => Action::TabTasks,
                Tab::Trackings => Action::TabTrackings,
            };
            let key_label = self.app.keybindings.label(&action);
            let tab_text = if is_active {
                format!("▌  {} {}  ▐", tab.title(), key_label)
            } else {
                format!("   {} {}   ", tab.title(), key_label)
            };

            let style = if is_active {
                Style::default()
                    .fg(t.on_primary())
                    .bg(t.primary())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(t.text_med())
                    .bg(t.surface())
            };

            for ch in tab_text.chars() {
                if x >= area.right() { break; }
                if let Some(cell) = buf.cell_mut(Position::new(x, area.top())) {
                    cell.set_char(ch);
                    cell.set_style(style);
                }
                x += 1;
            }

            if x < area.right() {
                if let Some(cell) = buf.cell_mut(Position::new(x, area.top())) {
                    cell.set_char(' ');
                    cell.set_bg(t.surface());
                }
                x += 1;
            }
        }

        while x < area.right() {
            if let Some(cell) = buf.cell_mut(Position::new(x, area.top())) {
                cell.set_char(' ');
                cell.set_bg(t.surface());
            }
            x += 1;
        }
    }
}

/// The thin separator line below the tab bar.
pub struct TabSeparator<'a> {
    app: &'a App,
}

impl<'a> TabSeparator<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for TabSeparator<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let t = &self.app.theme;
        let line = Line::from(vec![Span::styled(
            "─".repeat(area.width as usize),
            Style::default().fg(t.primary_dim()).bg(t.bg()),
        )]);
        line.render(area, buf);
    }
}
