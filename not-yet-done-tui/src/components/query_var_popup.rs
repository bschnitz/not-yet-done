//! Query-variable popup — generic input form rendered when a saved
//! query carries `${var}` placeholders the adapter has reported via
//! `ContentAdapter::query_variables`.
//!
//! Mirrors [`AdapterCredsPopup`] in shape (TextFieldWidget stack, Tab
//! navigation, Enter to submit, Esc to cancel). Differences:
//! - No `submitting` state: the popup closes immediately on submit and
//!   the App runs the load synchronously like `:query apply` does today.
//! - Required vs. optional: a variable is *required* iff its
//!   `QueryVariable::default` is `None`. Required fields with empty
//!   content block submit.

use std::collections::HashMap;
use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use ratatui_form_widgets::{TextFieldStyle, TextFieldWidget};

use not_yet_done_content::QueryVariable;

use crate::ui::popup_utils::{hints_height, render_hints_bar, render_popup_frame};
use crate::ui::theme::Theme;

pub enum QueryVarKeyOutcome {
    /// Key consumed; popup stays open.
    Consumed,
    /// User submitted the form. App should run the load.
    Submit { values: HashMap<String, String> },
    /// User cancelled (Esc). App closes the popup.
    Cancel,
    /// Key not consumed.
    Pass,
}

/// Context the App needs to dispatch a submit back to the right pane.
#[derive(Clone, Debug)]
pub struct QueryVarPopupTarget {
    pub tab_idx: usize,
    pub pane_id: crate::views::content_view::PaneId,
    pub raw_query: String,
    pub saved_name: Option<String>,
}

pub struct QueryVarPopup {
    theme: Arc<Theme>,
    title: String,
    target: QueryVarPopupTarget,
    vars: Vec<QueryVariable>,
    /// Live values, indexed by `vars`.
    values: Vec<String>,
    /// Cursor position (in chars) per field.
    cursor_pos: Vec<usize>,
    focused: usize,
    error: Option<String>,
    open: bool,
}

impl QueryVarPopup {
    pub fn new(
        theme: Arc<Theme>,
        title: String,
        target: QueryVarPopupTarget,
        vars: Vec<QueryVariable>,
        prefilled: HashMap<String, String>,
    ) -> Self {
        let values: Vec<String> = vars
            .iter()
            .map(|v| {
                prefilled
                    .get(&v.name)
                    .cloned()
                    .or_else(|| v.default.clone())
                    .unwrap_or_default()
            })
            .collect();
        let cursor_pos: Vec<usize> = values.iter().map(|s| s.chars().count()).collect();
        let focused = values.iter().position(|v| v.is_empty()).unwrap_or(0);
        Self {
            theme,
            title,
            target,
            vars,
            values,
            cursor_pos,
            focused,
            error: None,
            open: true,
        }
    }

    pub fn target(&self) -> &QueryVarPopupTarget {
        &self.target
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn handle_key(&mut self, key: &str) -> QueryVarKeyOutcome {
        if self.vars.is_empty() {
            return QueryVarKeyOutcome::Cancel;
        }
        match key {
            "esc" => QueryVarKeyOutcome::Cancel,
            "tab" | "down" => {
                self.focused = (self.focused + 1) % self.vars.len();
                QueryVarKeyOutcome::Consumed
            }
            "shift+tab" | "up" => {
                self.focused = if self.focused == 0 {
                    self.vars.len() - 1
                } else {
                    self.focused - 1
                };
                QueryVarKeyOutcome::Consumed
            }
            "enter" => {
                for (v, val) in self.vars.iter().zip(self.values.iter()) {
                    if v.default.is_none() && val.trim().is_empty() {
                        self.error = Some(format!("'{}' is required", v.name));
                        return QueryVarKeyOutcome::Consumed;
                    }
                }
                let mut map = HashMap::with_capacity(self.vars.len());
                for (v, val) in self.vars.iter().zip(self.values.iter()) {
                    map.insert(v.name.clone(), val.clone());
                }
                self.error = None;
                QueryVarKeyOutcome::Submit { values: map }
            }
            "backspace" => {
                let pos = self.cursor_pos[self.focused];
                if pos > 0 {
                    let mut chars: Vec<char> = self.values[self.focused].chars().collect();
                    chars.remove(pos - 1);
                    self.values[self.focused] = chars.into_iter().collect();
                    self.cursor_pos[self.focused] -= 1;
                }
                QueryVarKeyOutcome::Consumed
            }
            "left" => {
                if self.cursor_pos[self.focused] > 0 {
                    self.cursor_pos[self.focused] -= 1;
                }
                QueryVarKeyOutcome::Consumed
            }
            "right" => {
                let len = self.values[self.focused].chars().count();
                if self.cursor_pos[self.focused] < len {
                    self.cursor_pos[self.focused] += 1;
                }
                QueryVarKeyOutcome::Consumed
            }
            "home" => {
                self.cursor_pos[self.focused] = 0;
                QueryVarKeyOutcome::Consumed
            }
            "end" => {
                self.cursor_pos[self.focused] = self.values[self.focused].chars().count();
                QueryVarKeyOutcome::Consumed
            }
            other => {
                if let Some(c) = single_char(other) {
                    let pos = self.cursor_pos[self.focused];
                    let mut chars: Vec<char> = self.values[self.focused].chars().collect();
                    chars.insert(pos, c);
                    self.values[self.focused] = chars.into_iter().collect();
                    self.cursor_pos[self.focused] += 1;
                    QueryVarKeyOutcome::Consumed
                } else {
                    QueryVarKeyOutcome::Pass
                }
            }
        }
    }

    pub fn view(&mut self, frame: &mut Frame, area: Rect) {
        if !self.open {
            return;
        }
        let t = Arc::clone(&self.theme);
        let popup_w: u16 = 56;

        let hints: Vec<(&str, &str)> = vec![
            ("Tab", "next"),
            ("S-Tab", "prev"),
            ("Enter", "apply"),
            ("Esc", "cancel"),
        ];
        let hints_h = hints_height(&hints, popup_w.saturating_sub(2));
        let body_rows = self.vars.len() as u16 * 2 + 2;
        let popup_h = body_rows + hints_h + 2;

        let inner = render_popup_frame(frame, area, &t, &self.title, popup_w, popup_h);
        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let style = TextFieldStyle {
            label_focused: t.primary(),
            label_idle: t.text_dim(),
            input_focused: t.text_high(),
            input_idle: t.text_med(),
            cursor_fg: t.bg(),
            cursor_bg: t.primary(),
            error_fg: t.error(),
            placeholder_fg: t.text_dim(),
            input_bg: t.surface(),
            focused_bg: t.focused_bg(),
        };

        let mut y = inner.y;
        let buf = frame.buffer_mut();
        for (i, var) in self.vars.iter().enumerate() {
            let label = if var.default.is_some() {
                var.name.clone()
            } else {
                format!("{} (required)", var.name)
            };
            let cursor = if i == self.focused {
                Some(self.cursor_pos[i])
            } else {
                None
            };
            let placeholder = var.default.as_deref().unwrap_or("");
            let widget = TextFieldWidget {
                label: &label,
                value: &self.values[i],
                placeholder,
                error: None,
                focused: i == self.focused,
                cursor_pos: cursor,
                style,
            };
            let area_field = Rect {
                x: inner.x,
                y,
                width: inner.width,
                height: 2,
            };
            y = widget.render_and_next_y(area_field, buf);
        }

        if let Some(err) = self.error.as_deref() {
            let max = inner.width.saturating_sub(2) as usize;
            let truncated: String = err.chars().take(max).collect();
            let span = Span::styled(
                truncated,
                Style::default().fg(t.error()).add_modifier(Modifier::ITALIC),
            );
            let row_y = y + 1;
            if row_y < inner.bottom().saturating_sub(hints_h) {
                let mut x = inner.x + 1;
                for ch in span.content.chars() {
                    if x >= inner.right() {
                        break;
                    }
                    if let Some(cell) = buf.cell_mut(Position::new(x, row_y)) {
                        cell.set_char(ch);
                        cell.set_style(span.style);
                    }
                    x += 1;
                }
            }
        }

        render_hints_bar(frame, inner, &t, &hints, hints_h);
    }
}

fn single_char(key: &str) -> Option<char> {
    let mut chars = key.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    if c.is_control() {
        return None;
    }
    Some(c)
}
