//! Adapter credentials popup — generic login form for any adapter that
//! reports `AdapterStatus::NeedsCreds`.
//!
//! Renders a stack of `TextFieldWidget`s (one per `AuthField`), supports
//! Tab/Shift+Tab/Up/Down focus navigation, and emits a value map on
//! Enter. The popup stays open in a `submitting`/`error` state while the
//! adapter performs the login round-trip; the App calls `close()` once
//! the adapter status flips to `Ready`, or `set_error()` on failure.

use std::collections::HashMap;
use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use ratatui_form_widgets::{TextFieldStyle, TextFieldWidget};

use not_yet_done_content::AuthField;

use crate::ui::popup_utils::{hints_height, render_hints_bar, render_popup_frame};
use crate::ui::theme::Theme;

pub enum CredsKeyOutcome {
    /// Key consumed; nothing else for the App to do.
    Consumed,
    /// User submitted the form. App should spawn `submit_credentials`
    /// and meanwhile leave the popup open in `submitting` state.
    Submit { values: HashMap<String, String> },
    /// User cancelled (Esc). App closes the popup.
    Cancel,
    /// Key not consumed.
    Pass,
}

pub struct AdapterCredsPopup {
    theme: Arc<Theme>,
    title: String,
    /// View index this popup is bound to, so the App can route the
    /// submitted values to the right adapter.
    view_index: usize,
    fields: Vec<AuthField>,
    /// Live values, indexed by `fields`.
    values: Vec<String>,
    /// Cursor position (in chars) per field.
    cursor_pos: Vec<usize>,
    focused: usize,
    submitting: bool,
    error: Option<String>,
    open: bool,
}

impl AdapterCredsPopup {
    pub fn new(
        theme: Arc<Theme>,
        title: String,
        view_index: usize,
        fields: Vec<AuthField>,
    ) -> Self {
        let values: Vec<String> = fields
            .iter()
            .map(|f| f.prefill.clone().unwrap_or_default())
            .collect();
        let cursor_pos: Vec<usize> = values.iter().map(|v| v.chars().count()).collect();
        // Focus the first empty field; otherwise field 0.
        let focused = values.iter().position(|v| v.is_empty()).unwrap_or(0);
        Self {
            theme,
            title,
            view_index,
            fields,
            values,
            cursor_pos,
            focused,
            submitting: false,
            error: None,
            open: true,
        }
    }

    pub fn view_index(&self) -> usize {
        self.view_index
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn set_error(&mut self, msg: String) {
        self.error = Some(msg);
        self.submitting = false;
    }

    pub fn handle_key(&mut self, key: &str) -> CredsKeyOutcome {
        if self.submitting {
            // Block input while a login is in flight, except cancel.
            if matches!(key, "esc") {
                return CredsKeyOutcome::Cancel;
            }
            return CredsKeyOutcome::Consumed;
        }
        match key {
            "esc" => CredsKeyOutcome::Cancel,
            "tab" | "down" => {
                self.focused = (self.focused + 1) % self.fields.len();
                CredsKeyOutcome::Consumed
            }
            "shift+tab" | "up" => {
                self.focused = if self.focused == 0 {
                    self.fields.len() - 1
                } else {
                    self.focused - 1
                };
                CredsKeyOutcome::Consumed
            }
            "enter" => {
                // Submit only when all required fields have content.
                if self.values.iter().any(|v| v.trim().is_empty()) {
                    self.error = Some("All fields are required".into());
                    return CredsKeyOutcome::Consumed;
                }
                let mut map = HashMap::with_capacity(self.fields.len());
                for (f, v) in self.fields.iter().zip(self.values.iter()) {
                    map.insert(f.name.clone(), v.clone());
                }
                self.submitting = true;
                self.error = None;
                CredsKeyOutcome::Submit { values: map }
            }
            "backspace" => {
                let pos = self.cursor_pos[self.focused];
                if pos > 0 {
                    let mut chars: Vec<char> = self.values[self.focused].chars().collect();
                    chars.remove(pos - 1);
                    self.values[self.focused] = chars.into_iter().collect();
                    self.cursor_pos[self.focused] -= 1;
                }
                CredsKeyOutcome::Consumed
            }
            "left" => {
                if self.cursor_pos[self.focused] > 0 {
                    self.cursor_pos[self.focused] -= 1;
                }
                CredsKeyOutcome::Consumed
            }
            "right" => {
                let len = self.values[self.focused].chars().count();
                if self.cursor_pos[self.focused] < len {
                    self.cursor_pos[self.focused] += 1;
                }
                CredsKeyOutcome::Consumed
            }
            "home" => {
                self.cursor_pos[self.focused] = 0;
                CredsKeyOutcome::Consumed
            }
            "end" => {
                self.cursor_pos[self.focused] = self.values[self.focused].chars().count();
                CredsKeyOutcome::Consumed
            }
            other => {
                // Insert printable single chars (skip modifier-prefixed bindings).
                if let Some(c) = single_char(other) {
                    let pos = self.cursor_pos[self.focused];
                    let mut chars: Vec<char> = self.values[self.focused].chars().collect();
                    chars.insert(pos, c);
                    self.values[self.focused] = chars.into_iter().collect();
                    self.cursor_pos[self.focused] += 1;
                    CredsKeyOutcome::Consumed
                } else {
                    CredsKeyOutcome::Pass
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

        let hints: Vec<(&str, &str)> = if self.submitting {
            vec![("Esc", "cancel")]
        } else {
            vec![
                ("Tab", "next"),
                ("S-Tab", "prev"),
                ("Enter", "submit"),
                ("Esc", "close"),
            ]
        };
        let hints_h = hints_height(&hints, popup_w.saturating_sub(2));
        // 2 rows per field + status row + hints + padding.
        let body_rows = self.fields.len() as u16 * 2 + 2;
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
        for (i, field) in self.fields.iter().enumerate() {
            let display_value: String = if field.masked {
                "•".repeat(self.values[i].chars().count())
            } else {
                self.values[i].clone()
            };
            let cursor = if i == self.focused {
                Some(self.cursor_pos[i])
            } else {
                None
            };
            let widget = TextFieldWidget {
                label: &field.label,
                value: &display_value,
                placeholder: "",
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

        // Status line: error or submitting indicator.
        let status_text = if self.submitting {
            Some(("Submitting…", t.text_med()))
        } else {
            self.error.as_deref().map(|e| (e, t.error()))
        };
        if let Some((text, fg)) = status_text {
            let max = inner.width.saturating_sub(2) as usize;
            let truncated: String = text.chars().take(max).collect();
            let span = Span::styled(
                truncated,
                Style::default().fg(fg).add_modifier(Modifier::ITALIC),
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

/// Map a key string to a single printable char (e.g. "a", "Z", " ", "ä").
/// Returns `None` for modifier-prefixed bindings ("ctrl+s") or named keys.
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
