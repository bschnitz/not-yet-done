//! Column configuration popup — select and reorder table columns.

use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use tuirealm::command::{Cmd, CmdResult};
use tuirealm::component::Component;
use tuirealm::props::{AttrValue, Attribute, QueryResult};
use tuirealm::state::{State, StateValue};

use crate::config::{CommonAction, KeyBindingConfig};
use crate::ui::panel_chrome::{PanelChrome, cursor_bg, pad_row};
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
        let selected: Vec<bool> = order.iter().map(|id| current_config.contains(id)).collect();

        let hints = vec![
            ("Spc".to_string(), "toggle"),
            ("C-d/f".to_string(), "reorder"),
            (kb.common.label(&CommonAction::ListPrev), "up"),
            (kb.common.label(&CommonAction::ListNext), "down"),
            (kb.common.label(&CommonAction::FormClose), "close"),
        ];

        Self {
            theme,
            order,
            selected,
            cursor: 0,
            open: true,
            all_columns,
            hints,
        }
    }

    fn entry(&self, id: &str) -> Option<&ColumnEntry> {
        self.all_columns.iter().find(|c| c.id == id)
    }

    pub fn result(&self) -> Vec<String> {
        self.order
            .iter()
            .enumerate()
            .filter(|(i, _)| self.selected[*i])
            .map(|(_, id)| id.clone())
            .collect()
    }

    pub fn is_open(&self) -> bool {
        self.open
    }
    pub fn close(&mut self) {
        self.open = false;
    }

    fn move_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    fn move_down(&mut self) {
        if self.cursor + 1 < self.order.len() {
            self.cursor += 1;
        }
    }

    fn toggle(&mut self) {
        let id = &self.order[self.cursor];
        if let Some(entry) = self.entry(id) {
            if !entry.hideable {
                return;
            }
        }
        self.selected[self.cursor] = !self.selected[self.cursor];
    }

    fn order_up(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.order.swap(self.cursor, self.cursor - 1);
        self.selected.swap(self.cursor, self.cursor - 1);
        self.cursor -= 1;
    }

    fn order_down(&mut self) {
        if self.cursor + 1 >= self.order.len() {
            return;
        }
        self.order.swap(self.cursor, self.cursor + 1);
        self.selected.swap(self.cursor, self.cursor + 1);
        self.cursor += 1;
    }

    pub fn handle_key(&mut self, key: &str, kb: &KeyBindingConfig) -> bool {
        if kb
            .common
            .bindings
            .get(&CommonAction::FormClose)
            .map_or(false, |b| b.matches(key))
        {
            self.close();
            return true;
        }
        if key == "enter" {
            self.close();
            return true;
        }

        if kb
            .common
            .bindings
            .get(&CommonAction::ListPrev)
            .map_or(false, |b| b.matches(key))
        {
            self.move_up();
            return true;
        }
        if kb
            .common
            .bindings
            .get(&CommonAction::ListNext)
            .map_or(false, |b| b.matches(key))
        {
            self.move_down();
            return true;
        }

        match key {
            " " | "ctrl+ " => {
                self.toggle();
                true
            }
            "ctrl+d" => {
                self.order_up();
                true
            }
            "ctrl+f" => {
                self.order_down();
                true
            }
            _ => false,
        }
    }

    fn hints_as_refs(&self) -> Vec<(&str, &str)> {
        self.hints.iter().map(|(k, d)| (k.as_str(), *d)).collect()
    }
}

impl Component for ColumnConfigPopup {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let t = Arc::clone(&self.theme);

        let hint_refs = self.hints_as_refs();
        let heading = Line::from(vec![Span::styled(
            "\u{2726} Column Config",
            Style::default()
                .fg(t.form_accent())
                .add_modifier(Modifier::BOLD),
        )]);
        let digits = if self.order.is_empty() {
            1
        } else {
            (self.order.len() as f64).log10() as usize + 1
        };
        // Row width: number + `[x] ` marker + the widest display name.
        let name_w = self
            .order
            .iter()
            .map(|id| {
                self.entry(id)
                    .map(|e| e.display_name.chars().count())
                    .unwrap_or_else(|| id.chars().count())
            })
            .max()
            .unwrap_or(0);
        let body = PanelChrome::new(heading)
            .hints(hint_refs)
            .body(digits + 2 + 4 + name_w, self.order.len() as u16)
            .render(frame, area, &t);
        let Some(body) = body else {
            return;
        };

        let sel_bg = cursor_bg(&t);
        let lines: Vec<Line> = self
            .order
            .iter()
            .enumerate()
            .take(body.height as usize)
            .map(|(i, col_id)| {
                let is_sel = self.selected[i];
                let entry = self.entry(col_id);
                let is_fixed = entry.is_some_and(|e| !e.hideable);
                let row_style = if i == self.cursor {
                    Style::default().bg(sel_bg)
                } else {
                    Style::default()
                };

                let display_name = entry
                    .map(|e| e.display_name.as_str())
                    .unwrap_or(col_id.as_str());
                let header = entry.map(|e| e.header.as_str()).unwrap_or("");
                let col_color = entry.map(|e| e.color).unwrap_or_else(|| t.form_hint());

                let mut spans = vec![
                    Span::styled(
                        format!("{:>w$}. ", i + 1, w = digits),
                        row_style.fg(t.form_hint()),
                    ),
                    Span::styled(
                        if is_fixed || is_sel { "[x] " } else { "[ ] " },
                        row_style.fg(t.form_text()),
                    ),
                ];
                // The header prefix keeps the column's own colour, the rest
                // of the display name reads as plain text.
                if !header.is_empty() && display_name.starts_with(header) {
                    spans.push(Span::styled(
                        header.to_string(),
                        row_style.fg(col_color).add_modifier(Modifier::BOLD),
                    ));
                    spans.push(Span::styled(
                        display_name[header.len()..].to_string(),
                        row_style.fg(t.form_text()),
                    ));
                } else {
                    spans.push(Span::styled(
                        display_name.to_string(),
                        row_style.fg(col_color).add_modifier(Modifier::BOLD),
                    ));
                }
                pad_row(spans, body.width, row_style)
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), body);
    }

    fn query(&self, _attr: Attribute) -> Option<QueryResult<'_>> {
        None
    }
    fn attr(&mut self, _attr: Attribute, _value: AttrValue) {}
    fn state(&self) -> State {
        State::Single(StateValue::Usize(self.cursor))
    }
    fn perform(&mut self, _cmd: Cmd) -> CmdResult {
        CmdResult::NoChange
    }
}
