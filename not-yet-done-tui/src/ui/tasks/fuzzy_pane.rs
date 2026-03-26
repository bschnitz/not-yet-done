// Wird angezeigt, wenn `tasks_state.fuzzy_active == true`.
// Ersetzt in diesem Modus die Form-Leiste vollständig.

use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Modifier, Style},
    widgets::Widget,
};

use crate::app::App;
use crate::config::TasksAction;

pub struct FuzzyPane<'a> {
    app: &'a App,
}

impl<'a> FuzzyPane<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for FuzzyPane<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }

        let t = &self.app.theme;
        let ts = &self.app.tasks_state;
        let kb = &self.app.keybindings.tasks;

        let y = area.top();

        // Background
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                cell.set_char(' ');
                cell.set_bg(t.surface_2());
            }
        }

        let mut x = area.left() + 1;

        let write = |buf: &mut Buffer, x: &mut u16, text: &str, fg, bg, bold: bool| {
            let mut style = Style::default().fg(fg).bg(bg);
            if bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            for ch in text.chars() {
                if *x >= area.right() {
                    return;
                }
                if let Some(cell) = buf.cell_mut(Position::new(*x, y)) {
                    cell.set_char(ch);
                    cell.set_style(style);
                }
                *x += 1;
            }
        };

        // Label
        write(buf, &mut x, "󰈲 FuzzyFilter  ", t.accent(), t.surface_2(), true);

        // Exit hint (reserviert am rechten Rand)
        let exit_hint = format!("  {} exit ", kb.label(&TasksAction::FuzzyFilterExit));
        let reserved = exit_hint.len() as u16 + 1;
        let input_max_w = area.right().saturating_sub(x).saturating_sub(reserved);

        if input_max_w > 0 {
            let text = &ts.fuzzy_query;
            let cursor = ts.fuzzy_cursor;
            let max_w = input_max_w as usize;

            let chars: Vec<char> = text.chars().collect();
            let start = if cursor >= max_w { cursor + 1 - max_w } else { 0 };
            let visible: String = chars.iter().skip(start).take(max_w).collect();
            let pad = max_w.saturating_sub(visible.chars().count());
            let display = format!("{}{}", visible, " ".repeat(pad));

            let cursor_in_view = cursor.saturating_sub(start);
            for (i, ch) in display.chars().enumerate() {
                if x >= area.right() {
                    break;
                }
                let is_cursor = i == cursor_in_view;
                let (cfg, cbg) = if is_cursor {
                    (t.surface_2(), t.accent())
                } else {
                    (t.text_high(), t.surface_2())
                };
                if let Some(cell) = buf.cell_mut(Position::new(x, y)) {
                    cell.set_char(ch);
                    cell.set_style(Style::default().fg(cfg).bg(cbg));
                }
                x += 1;
            }
        }

        // Exit hint am rechten Rand
        let hint_x = area.right().saturating_sub(exit_hint.len() as u16 + 1);
        let mut hx = hint_x;
        write(buf, &mut hx, &exit_hint, t.text_dim(), t.surface_2(), false);
    }
}
