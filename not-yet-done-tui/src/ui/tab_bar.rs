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
use crate::ui::theme::Theme;

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
        // Fill background of the entire tab bar row first
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut(Position::new(x, area.top())) {
                cell.set_char(' ');
                cell.set_bg(Theme::SURFACE);
            }
        }

        let mut x = area.left() + 1; // 1-cell left margin

        for tab in Tab::ALL {
            let is_active = *tab == self.app.active_tab;

            let action = match tab {
                Tab::Welcome   => Action::TabWelcome,
                Tab::Tasks     => Action::TabTasks,
                Tab::Trackings => Action::TabTrackings,
            };
            let key_label = self.app.keybindings.label(&action);
            // "  Welcome [1]  "  with side-accent glyphs for active tab
            let tab_text = if is_active {
                format!("▌  {} {}  ▐", tab.title(), key_label)
            } else {
                format!("   {} {}   ", tab.title(), key_label)
            };

            let style = if is_active {
                Style::default()
                    .fg(Theme::ON_PRIMARY)
                    .bg(Theme::PRIMARY)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(Theme::TEXT_MED)
                    .bg(Theme::SURFACE)
            };

            for ch in tab_text.chars() {
                if x >= area.right() {
                    break;
                }
                if let Some(cell) = buf.cell_mut(Position::new(x, area.top())) {
                    cell.set_char(ch);
                    cell.set_style(style);
                }
                x += 1;
            }

            // Single-space gap between tabs
            if x < area.right() {
                if let Some(cell) = buf.cell_mut(Position::new(x, area.top())) {
                    cell.set_char(' ');
                    cell.set_bg(Theme::SURFACE);
                }
                x += 1;
            }
        }

        // Fill remaining cells on the right
        while x < area.right() {
            if let Some(cell) = buf.cell_mut(Position::new(x, area.top())) {
                cell.set_char(' ');
                cell.set_bg(Theme::SURFACE);
            }
            x += 1;
        }
    }
}

/// The thin separator line below the tab bar
pub struct TabSeparator;

impl Widget for TabSeparator {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let line = Line::from(vec![Span::styled(
            "─".repeat(area.width as usize),
            Style::default().fg(Theme::PRIMARY_DIM).bg(Theme::BG),
        )]);
        line.render(area, buf);
    }
}
