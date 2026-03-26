// Sub-tab bar rendered inside the Tasks tab.
//
// Layout (beide Views):
//   VIEW   List [l]  Tree [t]  │  FORM   Search [s]  Add [a]  Delete [d]  FuzzyFilter [f]  [esc] close
//
// Wenn fuzzy_active:
//   VIEW   List [l]  Tree [t]  │  FuzzyFilter aktiv  [tab] exit
//
// Wichtig: Die Bar ist in BEIDEN Views (List und Tree) sichtbar.
// Der Tree-Filter (inline fuzzy im Header) wurde entfernt – stattdessen gibt
// es den FuzzyFilter-Tab, der view-unabhängig funktioniert.

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
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for TasksSubTabBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let t = &self.app.theme;
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

        let write = |buf: &mut Buffer, x: &mut u16, text: &str, fg, bg, bold: bool| {
            let mut style = Style::default().fg(fg).bg(bg);
            if bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            for ch in text.chars() {
                if *x >= area.right() {
                    return;
                }
                if let Some(cell) = buf.cell_mut(Position::new(*x, area.top())) {
                    cell.set_char(ch);
                    cell.set_style(style);
                }
                *x += 1;
            }
        };

        // ── VIEW section ─────────────────────────────────────────────────
        write(buf, &mut x, "VIEW ", t.text_dim(), t.surface(), false);

        let list_active = ts.active_view == TasksView::List;
        let list_fg = if list_active { t.on_primary() } else { t.text_med() };
        let list_bg = if list_active { t.primary() } else { t.surface() };
        write(
            buf,
            &mut x,
            &format!(" List {} ", kb.label(&TasksAction::ViewList)),
            list_fg,
            list_bg,
            list_active,
        );

        write(buf, &mut x, " ", t.text_dim(), t.surface(), false);

        let tree_active = ts.active_view == TasksView::Tree;
        let tree_fg = if tree_active { t.on_primary() } else { t.text_med() };
        let tree_bg = if tree_active { t.primary() } else { t.surface() };
        write(
            buf,
            &mut x,
            &format!(" Tree {} ", kb.label(&TasksAction::ViewTree)),
            tree_fg,
            tree_bg,
            tree_active,
        );

        // ── Separator ────────────────────────────────────────────────────
        write(buf, &mut x, "  │  ", t.text_dim(), t.surface(), false);

        if ts.fuzzy_active {
            // ── FuzzyFilter aktiv: nur Hinweis ────────────────────────────
            write(buf, &mut x, "󰈲 FuzzyFilter ", t.accent(), t.surface(), true);
            write(
                buf,
                &mut x,
                &format!("{} exit ", kb.label(&TasksAction::FuzzyFilterExit)),
                t.text_dim(),
                t.surface(),
                false,
            );
        } else {
            // ── FORM section ──────────────────────────────────────────────
            let form_section_fg = if ts.form_visible() { t.accent() } else { t.text_dim() };
            write(buf, &mut x, "FORM ", form_section_fg, t.surface(), false);

            // Search (ehemals Filter)
            let search_active = ts.active_form == Some(TasksForm::Filter);
            let s_fg = if search_active { t.on_primary() } else { t.text_med() };
            let s_bg = if search_active { t.primary() } else { t.surface() };
            write(
                buf,
                &mut x,
                &format!(" Search {} ", kb.label(&TasksAction::FormFilter)),
                s_fg,
                s_bg,
                search_active,
            );
            write(buf, &mut x, " ", t.text_dim(), t.surface(), false);

            // Add
            let add_active = ts.active_form == Some(TasksForm::Add);
            let a_fg = if add_active { t.on_primary() } else { t.text_med() };
            let a_bg = if add_active { t.primary() } else { t.surface() };
            write(
                buf,
                &mut x,
                &format!(" Add {} ", kb.label(&TasksAction::FormAdd)),
                a_fg,
                a_bg,
                add_active,
            );
            write(buf, &mut x, " ", t.text_dim(), t.surface(), false);

            // Delete
            let del_active = ts.active_form == Some(TasksForm::Delete);
            let d_fg = if del_active { t.on_primary() } else { t.text_med() };
            let d_bg = if del_active { t.primary() } else { t.surface() };
            write(
                buf,
                &mut x,
                &format!(" Delete {} ", kb.label(&TasksAction::FormDelete)),
                d_fg,
                d_bg,
                del_active,
            );
            write(buf, &mut x, " ", t.text_dim(), t.surface(), false);

            // FuzzyFilter-Tab
            write(
                buf,
                &mut x,
                &format!(" FuzzyFilter {} ", kb.label(&TasksAction::FuzzyFilterOpen)),
                t.text_med(),
                t.surface(),
                false,
            );
            write(buf, &mut x, " ", t.text_dim(), t.surface(), false);

            // Close-Hint (nur wenn Form geöffnet)
            if ts.form_visible() {
                write(
                    buf,
                    &mut x,
                    &format!("{} close", kb.label(&TasksAction::FormClose)),
                    t.text_dim(),
                    t.surface(),
                    false,
                );
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
