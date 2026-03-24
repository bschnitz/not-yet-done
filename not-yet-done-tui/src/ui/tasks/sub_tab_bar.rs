use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Modifier, Style},
    widgets::Widget,
};

use crate::app::App;
use crate::config::TasksAction;
use crate::tabs::{TasksForm, TasksView};

/// A slim sub-tab bar rendered inside the Tasks tab, showing:
///
///   VIEW   List [l]  Tree [t]  │  FORM   Filter [f]  Add [a]  Delete [d]  [esc] close
///
/// The active item is highlighted in primary colour.
/// "FORM" section is dimmed when no form is open.
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

        // Helper: write a styled string into the buffer
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

        // ── VIEW section label ────────────────────────────────────────────
        write(buf, &mut x, "VIEW ", t.text_dim(), t.surface(), false);

        // List
        let list_active = ts.active_view == TasksView::List;
        let list_fg = if list_active { t.on_primary() } else { t.text_med() };
        let list_bg = if list_active { t.primary()    } else { t.surface()  };
        let list_label = format!(" List {} ", kb.label(&TasksAction::ViewList));
        write(buf, &mut x, &list_label, list_fg, list_bg, list_active);

        write(buf, &mut x, " ", t.text_dim(), t.surface(), false);

        // Tree
        let tree_active = ts.active_view == TasksView::Tree;
        let tree_fg = if tree_active { t.on_primary() } else { t.text_med() };
        let tree_bg = if tree_active { t.primary()    } else { t.surface()  };
        let tree_label = format!(" Tree {} ", kb.label(&TasksAction::ViewTree));
        write(buf, &mut x, &tree_label, tree_fg, tree_bg, tree_active);

        // Divider
        write(buf, &mut x, "  │  ", t.text_dim(), t.surface(), false);

        // ── FORM section label ────────────────────────────────────────────
        let form_section_fg = if ts.form_visible() { t.accent() } else { t.text_dim() };
        write(buf, &mut x, "FORM ", form_section_fg, t.surface(), false);

        // Filter / Add / Delete
        let form_tabs: &[(&str, TasksAction, Option<TasksForm>)] = &[
            ("Filter", TasksAction::FormFilter, Some(TasksForm::Filter)),
            ("Add",    TasksAction::FormAdd,    Some(TasksForm::Add)),
            ("Delete", TasksAction::FormDelete, Some(TasksForm::Delete)),
        ];

        for (label, action, form_variant) in form_tabs {
            let is_active = ts.active_form == *form_variant;
            let fg = if is_active { t.on_primary() } else { t.text_med() };
            let bg = if is_active { t.primary()    } else { t.surface()  };
            let text = format!(" {} {} ", label, kb.label(action));
            write(buf, &mut x, &text, fg, bg, is_active);
            write(buf, &mut x, " ", t.text_dim(), t.surface(), false);
        }

        // Close hint — only shown when form is open
        if ts.form_visible() {
            let close_label = format!("{} close", kb.label(&TasksAction::FormClose));
            write(buf, &mut x, &close_label, t.text_dim(), t.surface(), false);
        }
    }
}
