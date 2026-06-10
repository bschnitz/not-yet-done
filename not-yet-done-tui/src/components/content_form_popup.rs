//! Generic, adapter-driven form popup for `InputSpec::Form` actions (M6/E5).
//!
//! The popup is built purely from a `Vec<FormFieldSpec>` declared by the
//! adapter action: it stacks the reusable `ratatui_form_widgets`
//! (text / select / toggle), drives focus + in-field editing, validates
//! required fields, and emits the collected values keyed by
//! [`FormFieldSpec::key`] as a [`ContentFormEvent::Submitted`].
//!
//! Key/value handling is deliberately kept free of any `Frame` dependency
//! so it can be unit-tested headless; only [`ContentFormPopup::render`]
//! touches the terminal.

use std::collections::HashMap;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Widget};
use ratatui::Frame;

use ratatui_form_widgets::{ChoiceOption, MultipleChoiceWidget, TextFieldWidget, ToggleFieldWidget};

use not_yet_done_content::{FormFieldKind, FormFieldSpec};

use crate::ui::theme::Theme;

/// Per-field runtime state, parallel to `ContentFormPopup::fields`.
enum FieldState {
    Text { value: String, cursor: usize },
    /// `selected` indexes into the field's `allowed_values`; `cursor` is the
    /// keyboard highlight.
    Select { selected: Option<usize>, cursor: usize },
    Toggle { on: bool },
}

/// Outcome of feeding a key to the popup.
pub enum ContentFormEvent {
    /// The user submitted; carries the field values keyed by
    /// [`FormFieldSpec::key`].
    Submitted(HashMap<String, String>),
    /// The user cancelled (Esc).
    Cancelled,
    /// Key consumed, popup stays open.
    Consumed,
}

/// A generic multi-field form rendered as a centered overlay.
pub struct ContentFormPopup {
    title: String,
    fields: Vec<FormFieldSpec>,
    states: Vec<FieldState>,
    focused: usize,
    error: Option<String>,
}

impl ContentFormPopup {
    /// Build the popup from the action's field specs. `prefill` (from
    /// [`not_yet_done_content::Node::form_prep`]) overrides each field's
    /// static [`FormFieldSpec::default`].
    pub fn new(title: impl Into<String>, fields: Vec<FormFieldSpec>, prefill: &HashMap<String, String>) -> Self {
        let states = fields
            .iter()
            .map(|f| initial_state(f, prefill.get(&f.key).cloned()))
            .collect();
        Self {
            title: title.into(),
            fields,
            states,
            focused: 0,
            error: None,
        }
    }

    /// Feed a normalized key string (same vocabulary as the other popups:
    /// `tab`/`shift+tab`/`up`/`down`/`left`/`right`/`enter`/`esc`/
    /// `backspace`, space as `" "`, otherwise a single typed char).
    pub fn handle_key(&mut self, key: &str) -> ContentFormEvent {
        match key {
            "esc" => return ContentFormEvent::Cancelled,
            "enter" => return self.try_submit(),
            "tab" | "down" => {
                self.focus_next();
                return ContentFormEvent::Consumed;
            }
            "shift+tab" | "up" => {
                self.focus_prev();
                return ContentFormEvent::Consumed;
            }
            _ => {}
        }
        self.handle_field_key(key);
        ContentFormEvent::Consumed
    }

    /// Collected values, keyed by field key. Exposed for testing and reused
    /// by [`Self::try_submit`].
    pub fn values(&self) -> HashMap<String, String> {
        self.fields
            .iter()
            .zip(&self.states)
            .map(|(f, s)| (f.key.clone(), field_value(f, s)))
            .collect()
    }

    fn focus_next(&mut self) {
        if !self.fields.is_empty() {
            self.focused = (self.focused + 1) % self.fields.len();
        }
    }

    fn focus_prev(&mut self) {
        if !self.fields.is_empty() {
            self.focused = (self.focused + self.fields.len() - 1) % self.fields.len();
        }
    }

    fn handle_field_key(&mut self, key: &str) {
        let Some(state) = self.states.get_mut(self.focused) else {
            return;
        };
        match state {
            FieldState::Text { value, cursor } => handle_text_key(value, cursor, key),
            FieldState::Select { selected, cursor } => {
                let len = match &self.fields[self.focused].kind {
                    FormFieldKind::Select { allowed_values } => allowed_values.len(),
                    _ => 0,
                };
                handle_select_key(selected, cursor, len, key);
            }
            FieldState::Toggle { on } => {
                if key == " " {
                    *on = !*on;
                }
            }
        }
    }

    fn try_submit(&mut self) -> ContentFormEvent {
        for (i, (f, s)) in self.fields.iter().zip(&self.states).enumerate() {
            if f.required && field_value(f, s).trim().is_empty() {
                self.error = Some(format!("`{}` is required", f.label));
                self.focused = i;
                return ContentFormEvent::Consumed;
            }
        }
        ContentFormEvent::Submitted(self.values())
    }

    /// Draw the popup as a centered overlay.
    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let popup = centered_popup(area, &self.fields);
        frame.render_widget(Clear, popup);
        let block = self.outer_block(theme);
        let inner = block.inner(popup);
        let buf = frame.buffer_mut();
        block.render(popup, buf);
        self.render_fields(inner, buf, theme);
    }

    fn outer_block(&self, theme: &Theme) -> Block<'static> {
        let accent = theme.primary();
        let bottom = match &self.error {
            Some(e) => Span::styled(format!(" ⚠ {e} "), Style::default().fg(theme.error())),
            None => Span::styled(
                " tab/↑↓ field  space select/toggle  enter submit  esc cancel ".to_string(),
                Style::default().fg(theme.text_dim()),
            ),
        };
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(accent))
            .title(Span::styled(
                format!(" {} ", self.title),
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ))
            .title_bottom(bottom)
            .style(Style::default().bg(theme.form_bg()))
    }

    fn render_fields(&self, area: Rect, buf: &mut ratatui::buffer::Buffer, theme: &Theme) {
        let tf_style = theme.text_field_style();
        let mc_style = theme.multiple_choice_style();
        let tg_style = theme.toggle_field_style();
        let x = area.x;
        let width = area.width;
        let mut y = area.y + 1;

        for (i, (f, s)) in self.fields.iter().zip(&self.states).enumerate() {
            let focused = i == self.focused;
            match s {
                FieldState::Text { value, cursor } => {
                    if y + 2 > area.bottom() {
                        break;
                    }
                    TextFieldWidget {
                        label: &f.label,
                        value,
                        placeholder: "",
                        error: None,
                        focused,
                        cursor_pos: focused.then_some(*cursor),
                        style: tf_style,
                    }
                    .render_and_next_y(Rect { x, y, width, height: 2 }, buf);
                    y += 3;
                }
                FieldState::Select { selected, cursor } => {
                    if y + 2 > area.bottom() {
                        break;
                    }
                    let opts = select_options(f, *selected);
                    MultipleChoiceWidget::new(&f.label, &opts, focused, *cursor, mc_style)
                        .render_and_next_y(Rect { x, y, width, height: 2 }, buf);
                    y += 3;
                }
                FieldState::Toggle { on } => {
                    if y + 1 > area.bottom() {
                        break;
                    }
                    ToggleFieldWidget::new(&f.label, *on, focused, tg_style)
                        .render(Rect { x, y, width, height: 1 }, buf);
                    y += 2;
                }
            }
        }
    }
}

fn initial_state(field: &FormFieldSpec, prefill: Option<String>) -> FieldState {
    let initial = prefill.or_else(|| field.default.clone());
    match &field.kind {
        FormFieldKind::Text => {
            let value = initial.unwrap_or_default();
            let cursor = value.chars().count();
            FieldState::Text { value, cursor }
        }
        FormFieldKind::Select { allowed_values } => {
            let selected = initial.and_then(|v| allowed_values.iter().position(|a| a == &v));
            FieldState::Select {
                selected,
                cursor: selected.unwrap_or(0),
            }
        }
        FormFieldKind::Toggle => FieldState::Toggle {
            on: matches!(initial.as_deref(), Some("true")),
        },
    }
}

fn field_value(field: &FormFieldSpec, state: &FieldState) -> String {
    match (state, &field.kind) {
        (FieldState::Text { value, .. }, _) => value.clone(),
        (FieldState::Select { selected, .. }, FormFieldKind::Select { allowed_values }) => {
            selected.and_then(|i| allowed_values.get(i).cloned()).unwrap_or_default()
        }
        (FieldState::Toggle { on }, _) => if *on { "true" } else { "false" }.to_string(),
        _ => String::new(),
    }
}

fn handle_text_key(value: &mut String, cursor: &mut usize, key: &str) {
    match key {
        "left" => *cursor = cursor.saturating_sub(1),
        "right" => *cursor = (*cursor + 1).min(value.chars().count()),
        "backspace" => {
            if *cursor > 0 {
                let byte = char_byte_index(value, *cursor - 1);
                value.remove(byte);
                *cursor -= 1;
            }
        }
        _ => {
            // Single printable char (space included) is inserted at cursor.
            if key.chars().count() == 1 {
                let c = key.chars().next().unwrap();
                if !c.is_control() {
                    let byte = char_byte_index(value, *cursor);
                    value.insert(byte, c);
                    *cursor += 1;
                }
            }
        }
    }
}

fn handle_select_key(selected: &mut Option<usize>, cursor: &mut usize, len: usize, key: &str) {
    if len == 0 {
        return;
    }
    match key {
        "left" => *cursor = cursor.saturating_sub(1),
        "right" => *cursor = (*cursor + 1).min(len - 1),
        " " => *selected = Some(*cursor),
        _ => {}
    }
}

/// Byte offset of the `n`-th char (or end of string).
fn char_byte_index(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map(|(b, _)| b).unwrap_or(s.len())
}

fn select_options(field: &FormFieldSpec, selected: Option<usize>) -> Vec<ChoiceOption<'_>> {
    match &field.kind {
        FormFieldKind::Select { allowed_values } => allowed_values
            .iter()
            .enumerate()
            .map(|(i, v)| ChoiceOption::new(v, selected == Some(i)))
            .collect(),
        _ => Vec::new(),
    }
}

/// Height a field consumes (text/select = 2 rows + 1 gap, toggle = 1 + 1).
fn field_rows(field: &FormFieldSpec) -> u16 {
    match field.kind {
        FormFieldKind::Toggle => 2,
        _ => 3,
    }
}

fn centered_popup(area: Rect, fields: &[FormFieldSpec]) -> Rect {
    let content_h: u16 = fields.iter().map(field_rows).sum();
    // borders (2) + top padding (1).
    let popup_h = (content_h + 3).min(area.height.saturating_sub(2)).max(5);
    let popup_w = area.width.saturating_sub(8).min(80).max(40);
    let x = area.x + area.width.saturating_sub(popup_w) / 2;
    let y = area.y + area.height.saturating_sub(popup_h) / 2;
    Rect::new(x, y, popup_w, popup_h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_content::FormFieldSpec;

    fn sample_fields() -> Vec<FormFieldSpec> {
        vec![
            FormFieldSpec::text("title", "Title"),
            FormFieldSpec::select(
                "status",
                "Status",
                vec!["todo".into(), "in_progress".into(), "done".into()],
            ),
            FormFieldSpec::toggle("urgent", "Urgent"),
        ]
    }

    fn type_str(popup: &mut ContentFormPopup, s: &str) {
        for c in s.chars() {
            popup.handle_key(&c.to_string());
        }
    }

    #[test]
    fn text_field_collects_typed_value() {
        let mut p = ContentFormPopup::new("t", sample_fields(), &HashMap::new());
        type_str(&mut p, "hi");
        assert_eq!(p.values().get("title").unwrap(), "hi");
    }

    #[test]
    fn prefill_seeds_initial_values() {
        let mut prefill = HashMap::new();
        prefill.insert("title".to_string(), "seed".to_string());
        prefill.insert("status".to_string(), "done".to_string());
        prefill.insert("urgent".to_string(), "true".to_string());
        let p = ContentFormPopup::new("t", sample_fields(), &prefill);
        let v = p.values();
        assert_eq!(v.get("title").unwrap(), "seed");
        assert_eq!(v.get("status").unwrap(), "done");
        assert_eq!(v.get("urgent").unwrap(), "true");
    }

    #[test]
    fn select_space_picks_option_under_cursor() {
        let mut p = ContentFormPopup::new("t", sample_fields(), &HashMap::new());
        p.handle_key("down"); // focus status
        p.handle_key("right"); // cursor → in_progress
        p.handle_key(" "); // select
        assert_eq!(p.values().get("status").unwrap(), "in_progress");
    }

    #[test]
    fn toggle_space_flips_value() {
        let mut p = ContentFormPopup::new("t", sample_fields(), &HashMap::new());
        p.handle_key("down");
        p.handle_key("down"); // focus urgent
        assert_eq!(p.values().get("urgent").unwrap(), "false");
        p.handle_key(" ");
        assert_eq!(p.values().get("urgent").unwrap(), "true");
    }

    #[test]
    fn submit_blocked_while_required_field_empty() {
        let mut p = ContentFormPopup::new("t", sample_fields(), &HashMap::new());
        // status is a required select with no selection, title empty.
        match p.handle_key("enter") {
            ContentFormEvent::Consumed => {}
            _ => panic!("expected submit to be blocked"),
        }
    }

    #[test]
    fn submit_returns_values_when_required_satisfied() {
        let mut p = ContentFormPopup::new("t", sample_fields(), &HashMap::new());
        type_str(&mut p, "x"); // title
        p.handle_key("down");
        p.handle_key(" "); // select first status (todo)
        match p.handle_key("enter") {
            ContentFormEvent::Submitted(v) => {
                assert_eq!(v.get("title").unwrap(), "x");
                assert_eq!(v.get("status").unwrap(), "todo");
                assert_eq!(v.get("urgent").unwrap(), "false");
            }
            _ => panic!("expected submit"),
        }
    }

    #[test]
    fn esc_cancels() {
        let mut p = ContentFormPopup::new("t", sample_fields(), &HashMap::new());
        assert!(matches!(p.handle_key("esc"), ContentFormEvent::Cancelled));
    }
}
