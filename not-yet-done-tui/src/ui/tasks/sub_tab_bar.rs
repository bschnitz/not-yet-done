// Sub-tab bar rendered inside the Tasks tab.
//
// Layout:
//   VIEW   List [l]  Tree [t]  │  FORM   Filter [f]  Add [a]  Delete [d]  [esc] close
//
// When the Tree view is active, an inline fuzzy filter input is appended:
//   … Tree [t]  │  󰈲  <filter input>  [esc] clear
//
// The active view / form item is highlighted in primary colour.

use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Modifier, Style},
    widgets::Widget,
};

use crate::app::App;
use crate::config::TasksAction;
use crate::tabs::{TasksForm, TasksView};

pub struct TasksSubTabBar<'a> {
    app: &'a App,
}

impl<'a> TasksSubTabBar<'a> {
    pub fn new(app: &'a App) -> Self { Self { app } }
}

impl Widget for TasksSubTabBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let t  = &self.app.theme;
        let kb = &self.app.keybindings.tasks;
        let ts = &self.app.tasks_state;

        // Fill background
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut(Position::new(x, area.top())) {
                cell.set_char(' ');
                cell.set_bg(t.surface());
            }
        }

        let mut x = area.left() + 1;

        // Helper: write styled text into the buffer at the current x position.
        // Returns the new x after writing.
        let write = |buf: &mut Buffer, x: &mut u16, text: &str, fg, bg, bold: bool| {
            let mut style = Style::default().fg(fg).bg(bg);
            if bold { style = style.add_modifier(Modifier::BOLD); }
            for ch in text.chars() {
                if *x >= area.right() { return; }
                if let Some(cell) = buf.cell_mut(Position::new(*x, area.top())) {
                    cell.set_char(ch);
                    cell.set_style(style);
                }
                *x += 1;
            }
        };

        // ── VIEW section ─────────────────────────────────────────────────
        write(buf, &mut x, "VIEW ", t.text_dim(), t.surface(), false);

        // List tab
        let list_active = ts.active_view == TasksView::List;
        let list_fg = if list_active { t.on_primary() } else { t.text_med() };
        let list_bg = if list_active { t.primary()    } else { t.surface()  };
        write(buf, &mut x,
            &format!(" List {} ", kb.label(&TasksAction::ViewList)),
            list_fg, list_bg, list_active);

        write(buf, &mut x, " ", t.text_dim(), t.surface(), false);

        // Tree tab
        let tree_active = ts.active_view == TasksView::Tree;
        let tree_fg = if tree_active { t.on_primary() } else { t.text_med() };
        let tree_bg = if tree_active { t.primary()    } else { t.surface()  };
        write(buf, &mut x,
            &format!(" Tree {} ", kb.label(&TasksAction::ViewTree)),
            tree_fg, tree_bg, tree_active);

        // ── Tree filter input (only when Tree view is active) ─────────────
        if ts.active_view == TasksView::Tree {
            write(buf, &mut x, "  ", t.text_dim(), t.surface(), false);

            // Icon + label
            let filter_icon = "󰈲 ";
            let filter_fg = if ts.tree_filter_focused { t.accent() } else { t.text_dim() };
            write(buf, &mut x, filter_icon, filter_fg, t.surface(), false);

            // Input field: render up to the available remaining width
            // Reserve space for the trailing hint "  [esc]" (≈ 8 chars)
            let hint = "  [esc] ";
            let reserved = hint.len() as u16 + 2;
            let input_max_w = area.right().saturating_sub(x).saturating_sub(reserved);

            if input_max_w > 0 {
                let input_bg = if ts.tree_filter_focused { t.surface_2() } else { t.surface() };
                let input_fg = if ts.tree_filter_focused { t.text_high() } else { t.text_med() };

                // Which portion of the string to display (slide window around cursor)
                let text = &ts.tree_filter;
                let cursor = ts.tree_filter_cursor;
                let max_w = input_max_w as usize;

                // Compute a visible window so the cursor stays in view
                let chars: Vec<char> = text.chars().collect();
                let start = if cursor >= max_w { cursor + 1 - max_w } else { 0 };
                let visible: String = chars
                    .iter()
                    .skip(start)
                    .take(max_w)
                    .collect();

                // Pad / truncate to exactly input_max_w display columns
                let pad = max_w.saturating_sub(visible.chars().count());
                let display = format!("{}{}", visible, " ".repeat(pad));

                // Render character by character, drawing the cursor block
                let cursor_in_view = cursor.saturating_sub(start);
                for (i, ch) in display.chars().enumerate() {
                    if x >= area.right() { break; }
                    let is_cursor = ts.tree_filter_focused && i == cursor_in_view;
                    let (cfg, cbg) = if is_cursor {
                        (t.surface(), t.accent())
                    } else {
                        (input_fg, input_bg)
                    };
                    if let Some(cell) = buf.cell_mut(Position::new(x, area.top())) {
                        cell.set_char(ch);
                        cell.set_style(Style::default().fg(cfg).bg(cbg));
                    }
                    x += 1;
                }

                // Clear hint
                if !ts.tree_filter.is_empty() {
                    write(buf, &mut x, " [esc]clear ", t.text_dim(), t.surface(), false);
                }
            }
        } else {
            // ── FORM section (only when List view or no Tree) ────────────
            write(buf, &mut x, "  │  ", t.text_dim(), t.surface(), false);

            let form_section_fg = if ts.form_visible() { t.accent() } else { t.text_dim() };
            write(buf, &mut x, "FORM ", form_section_fg, t.surface(), false);

            let form_tabs: &[(&str, TasksAction, Option<TasksForm>)] = &[
                ("Filter", TasksAction::FormFilter, Some(TasksForm::Filter)),
                ("Add",    TasksAction::FormAdd,    Some(TasksForm::Add)),
                ("Delete", TasksAction::FormDelete, Some(TasksForm::Delete)),
            ];

            for (label, action, form_variant) in form_tabs {
                let is_active = ts.active_form == *form_variant;
                let fg = if is_active { t.on_primary() } else { t.text_med() };
                let bg = if is_active { t.primary()    } else { t.surface()  };
                write(buf, &mut x,
                    &format!(" {} {} ", label, kb.label(action)),
                    fg, bg, is_active);
                write(buf, &mut x, " ", t.text_dim(), t.surface(), false);
            }

            if ts.form_visible() {
                write(buf, &mut x,
                    &format!("{} close", kb.label(&TasksAction::FormClose)),
                    t.text_dim(), t.surface(), false);
            }
        }

        // ── Fill remainder ────────────────────────────────────────────────
        while x < area.right() {
            if let Some(cell) = buf.cell_mut(Position::new(x, area.top())) {
                cell.set_char(' ');
                cell.set_bg(t.surface());
            }
            x += 1;
        }
    }
}
