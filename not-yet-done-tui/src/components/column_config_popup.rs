//! Column configuration popup — select and reorder table columns.

use std::sync::Arc;

use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::Frame;

use tuirealm::command::{Cmd, CmdResult};
use tuirealm::props::{Attribute, AttrValue, QueryResult};
use tuirealm::component::Component;
use tuirealm::state::{State, StateValue};

use crate::config::{KeyBindingConfig, CommonAction};
use crate::ui::popup_utils::{render_popup_frame, render_hints_bar, hints_height};
use crate::ui::theme::Theme;

/// One configurable column, already resolved to display data. The popup
/// is source-agnostic: native tabs map their static `ColumnMeta` registry
/// into this, content tabs build it from the active level's `ColumnDef`s
/// — so both share one component without the popup knowing about either.
#[derive(Debug, Clone)]
pub struct ColumnEntry {
    pub id: String,
    /// Abbreviated header shown in the table (highlighted as the
    /// display-name prefix when it matches).
    pub header: String,
    /// Full display name shown in the popup list.
    pub display_name: String,
    /// Already-resolved column color (theme lookup happens at the call
    /// site, which knows its own color vocabulary).
    pub color: Color,
    /// Whether this column can be hidden.
    pub hideable: bool,
}

pub struct ColumnConfigPopup {
    theme: Arc<Theme>,
    order: Vec<String>,
    selected: Vec<bool>,
    cursor: usize,
    open: bool,
    /// All available columns for lookup.
    all_columns: Vec<ColumnEntry>,
    /// Pre-built hint labels from config.
    hints: Vec<(String, &'static str)>,
}

impl ColumnConfigPopup {
    pub fn new(
        theme: Arc<Theme>,
        current_config: &[String],
        all_columns: Vec<ColumnEntry>,
        kb: &KeyBindingConfig,
    ) -> Self {
        let mut order: Vec<String> = current_config.to_vec();
        for entry in &all_columns {
            if !order.iter().any(|id| *id == entry.id) {
                order.push(entry.id.clone());
            }
        }
        let selected: Vec<bool> = order.iter()
            .map(|id| current_config.contains(id))
            .collect();

        let hints = vec![
            ("Spc".to_string(), "toggle"),
            ("C-d/f".to_string(), "reorder"),
            (kb.common.label(&CommonAction::ListPrev), "up"),
            (kb.common.label(&CommonAction::ListNext), "down"),
            (kb.common.label(&CommonAction::FormClose), "close"),
        ];

        Self { theme, order, selected, cursor: 0, open: true, all_columns, hints }
    }

    fn entry(&self, id: &str) -> Option<&ColumnEntry> {
        self.all_columns.iter().find(|c| c.id == id)
    }

    pub fn result(&self) -> Vec<String> {
        self.order.iter()
            .enumerate()
            .filter(|(i, _)| self.selected[*i])
            .map(|(_, id)| id.clone())
            .collect()
    }

    pub fn is_open(&self) -> bool { self.open }
    pub fn close(&mut self) { self.open = false; }

    fn move_up(&mut self) {
        if self.cursor > 0 { self.cursor -= 1; }
    }

    fn move_down(&mut self) {
        if self.cursor + 1 < self.order.len() { self.cursor += 1; }
    }

    fn toggle(&mut self) {
        let id = &self.order[self.cursor];
        if let Some(entry) = self.entry(id) {
            if !entry.hideable { return; }
        }
        self.selected[self.cursor] = !self.selected[self.cursor];
    }

    fn order_up(&mut self) {
        if self.cursor == 0 { return; }
        self.order.swap(self.cursor, self.cursor - 1);
        self.selected.swap(self.cursor, self.cursor - 1);
        self.cursor -= 1;
    }

    fn order_down(&mut self) {
        if self.cursor + 1 >= self.order.len() { return; }
        self.order.swap(self.cursor, self.cursor + 1);
        self.selected.swap(self.cursor, self.cursor + 1);
        self.cursor += 1;
    }

    pub fn handle_key(&mut self, key: &str, kb: &KeyBindingConfig) -> bool {
        if kb.common.bindings.get(&CommonAction::FormClose)
            .map_or(false, |b| b.matches(key))
        {
            self.close();
            return true;
        }
        if key == "enter" { self.close(); return true; }

        if kb.common.bindings.get(&CommonAction::ListPrev)
            .map_or(false, |b| b.matches(key))
        {
            self.move_up();
            return true;
        }
        if kb.common.bindings.get(&CommonAction::ListNext)
            .map_or(false, |b| b.matches(key))
        {
            self.move_down();
            return true;
        }

        match key {
            " " | "ctrl+ " => { self.toggle(); true }
            "ctrl+d" => { self.order_up(); true }
            "ctrl+f" => { self.order_down(); true }
            _ => false,
        }
    }

    fn hints_as_refs(&self) -> Vec<(&str, &str)> {
        self.hints.iter().map(|(k, d)| (k.as_str(), *d)).collect()
    }
}

impl Component for ColumnConfigPopup {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let t = &self.theme;

        let popup_w = 40u16;
        let hint_refs = self.hints_as_refs();
        let hints_h = hints_height(&hint_refs, popup_w.saturating_sub(2));
        let popup_h = self.order.len() as u16 + 2 + hints_h;

        let inner = render_popup_frame(frame, area, t, "Column Config", popup_w, popup_h);
        if inner.height == 0 || inner.width == 0 { return; }

        {
        let buf = frame.buffer_mut();

        let digits = if self.order.is_empty() { 1 } else { (self.order.len() as f64).log10() as usize + 1 };
        let items_height = inner.height.saturating_sub(hints_h) as usize;

        for (i, col_id) in self.order.iter().enumerate() {
            if i >= items_height { break; }
            let row_y = inner.y + i as u16;
            let is_sel = self.selected[i];
            let is_cursor = i == self.cursor;
            let entry = self.entry(col_id);
            let is_fixed = entry.map_or(false, |e| !e.hideable);

            let bg = if is_cursor { t.surface_2() } else { t.bg() };

            for cx in inner.left()..inner.right() {
                if let Some(cell) = buf.cell_mut(Position::new(cx, row_y)) {
                    cell.set_char(' ');
                    cell.set_style(Style::default().bg(bg));
                }
            }

            let display_name = entry.map(|e| e.display_name.as_str()).unwrap_or(col_id.as_str());
            let header = entry.map(|e| e.header.as_str()).unwrap_or("");
            let col_color = entry.map(|e| e.color).unwrap_or(t.text_med());

            let num = format!("{:>w$}. ", i + 1, w = digits);
            let marker = if is_fixed || is_sel { "[x] " } else { "[ ] " };
            let text_fg = t.text_high();

            let mut cx = inner.left() + 1;

            let num_style = Style::default().fg(t.text_dim()).bg(bg);
            for ch in num.chars() {
                if cx >= inner.right() { break; }
                if let Some(cell) = buf.cell_mut(Position::new(cx, row_y)) {
                    cell.set_char(ch);
                    cell.set_style(num_style);
                }
                cx += 1;
            }

            let marker_style = Style::default().fg(text_fg).bg(bg);
            for ch in marker.chars() {
                if cx >= inner.right() { break; }
                if let Some(cell) = buf.cell_mut(Position::new(cx, row_y)) {
                    cell.set_char(ch);
                    cell.set_style(marker_style);
                }
                cx += 1;
            }

            if !header.is_empty() && display_name.starts_with(header) {
                let hl = Style::default().fg(col_color).bg(bg).add_modifier(Modifier::BOLD);
                let rest = Style::default().fg(text_fg).bg(bg);
                for ch in header.chars() {
                    if cx >= inner.right() { break; }
                    if let Some(cell) = buf.cell_mut(Position::new(cx, row_y)) {
                        cell.set_char(ch);
                        cell.set_style(hl);
                    }
                    cx += 1;
                }
                for ch in display_name[header.len()..].chars() {
                    if cx >= inner.right() { break; }
                    if let Some(cell) = buf.cell_mut(Position::new(cx, row_y)) {
                        cell.set_char(ch);
                        cell.set_style(rest);
                    }
                    cx += 1;
                }
            } else {
                let style = Style::default().fg(col_color).bg(bg).add_modifier(Modifier::BOLD);
                for ch in display_name.chars() {
                    if cx >= inner.right() { break; }
                    if let Some(cell) = buf.cell_mut(Position::new(cx, row_y)) {
                        cell.set_char(ch);
                        cell.set_style(style);
                    }
                    cx += 1;
                }
            }
        }

        } // drop buf borrow

        // Hints bar with auto-wrap.
        render_hints_bar(frame, inner, t, &hint_refs, hints_h);
    }

    fn query(&self, _attr: Attribute) -> Option<QueryResult<'_>> { None }
    fn attr(&mut self, _attr: Attribute, _value: AttrValue) {}
    fn state(&self) -> State { State::Single(StateValue::Usize(self.cursor)) }
    fn perform(&mut self, _cmd: Cmd) -> CmdResult { CmdResult::NoChange }
}
