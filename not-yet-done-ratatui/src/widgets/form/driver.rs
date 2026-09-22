use std::collections::HashMap;

use ratatui::style::Color;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use tuirealm::command::{Cmd, CmdResult, Direction};
use tuirealm::component::Component;
use tuirealm::props::{AttrValue, Attribute, PropPayload, PropValue};
use tuirealm::state::{State, StateValue};
use unicode_width::UnicodeWidthChar;

use crate::widgets::common::{PREFIX_LEN, SelectionMarker, SelectionMode, truncate_to_width};
use crate::widgets::multi_choice::{ATTR_SELECTED as MC_ATTR_SELECTED, MultiChoice};
use crate::widgets::select_list::{ATTR_SELECTED as SL_ATTR_SELECTED, SelectList};
use crate::widgets::text_input::{CMD_END, CMD_HOME, TextInput};
use crate::widgets::toggle::Toggle;

use super::options::{FormOptions, SelectStyle};
use super::spec::{FieldCondition, FormFieldKind, FormFieldSpec};
use super::style::FormStyle;

/// Maximum number of option rows shown when a dropdown select is focused/open.
const MAX_VISIBLE: u16 = 6;

/// The `▍ ` gutter drawn left of an inline select's options.
const GUTTER: u16 = 2;

/// How many rows a field message may take when it is too long for the footer.
/// Three is enough for a sentence a script is likely to write and still leaves
/// the form readable behind it; beyond that the last row is cut with an
/// ellipsis, because a message nobody reads to the end is not worth hiding the
/// whole form for.
const ERROR_EXPAND_ROWS: u16 = 3;

/// Outcome of feeding a normalized key string to [`Form::handle_key`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormEvent {
    /// The user confirmed the form; carries the collected `key -> value` map.
    Submitted(HashMap<String, String>),
    /// The user cancelled the form (e.g. pressed `esc`).
    Cancelled,
    /// The key was handled internally; nothing to report to the caller.
    Consumed,
}

/// A message the *caller* puts on the form, as opposed to the form's own
/// validation error: why the last submit was rejected by whatever received it
/// ("wrong passphrase"), or that it is still under way ("Submitting…").
///
/// Unlike a validation error this survives focus changes — the form never
/// clears it, only [`Form::set_notice`] does. A validation error outranks it
/// while it stands, because that one is about the field the user is looking at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormNotice {
    /// Neutral progress note, rendered in the footer style.
    Info(String),
    /// Something went wrong, rendered in the error style.
    Alert(String),
}

/// The concrete widget backing a field, plus any per-kind metadata the driver
/// needs (select options, date/time granularity).
enum FieldWidget {
    Text(TextInput),
    /// Compact dropdown select (expands only while focused).
    Select {
        widget: MultiChoice,
        options: Vec<String>,
    },
    /// Inline radio-list select (all options always shown, with a `▍` gutter).
    /// The backing [`SelectList`] renders no title, so the driver hand-draws the
    /// label using [`FieldEntry::label`].
    SelectInline {
        widget: SelectList,
        options: Vec<String>,
    },
    Toggle(Toggle),
    DateTime {
        widget: TextInput,
        with_time: bool,
    },
}

struct FieldEntry {
    key: String,
    label: String,
    required: bool,
    widget: FieldWidget,
    /// When set, the field participates (render/focus/collect/validate) only
    /// while this condition holds against another field's current value.
    condition: Option<FieldCondition>,
}

/// A spec-driven, keyboard-navigated form built from `not-yet-done-ratatui`
/// widgets.
///
/// The form owns retained-mode widgets and is driven with the application's
/// normalized `&str` key vocabulary via [`handle_key`](Self::handle_key):
/// `tab`/`shift+tab` (and `up`/`down` outside selects) move between fields,
/// `enter` submits, `esc` cancels; everything else is dispatched to the focused
/// field's widget as a [`Cmd`].
///
/// Layout (column count, per-field column) and select rendering come from a
/// [`FormOptions`]; colours from a [`FormStyle`].
pub struct Form {
    title: String,
    fields: Vec<FieldEntry>,
    focused: usize,
    /// Per-field messages, keyed by field key: the form's own required-check and
    /// whatever the caller attached to a field. Each one marks its field and is
    /// shown in the footer while that field is focused; it survives a focus
    /// change and is dropped when the field's value changes.
    field_errors: HashMap<String, String>,
    /// Caller-owned message (see [`FormNotice`]); outranked by a field error.
    notice: Option<FormNotice>,
    /// Value of every field (visible or not) as the form was built, against
    /// which `esc` decides whether anything is worth a discard prompt.
    initial_values: HashMap<String, String>,
    /// Whether the `Discard your input?` prompt is currently up.
    discard_pending: bool,
    title_style: Style,
    footer_style: Style,
    error_style: Style,
    #[cfg(feature = "natural-date")]
    preview_color: Color,
    // --- layout ---
    columns: usize,
    /// Per-field column index (`0`/`1`), parallel to `fields`.
    column_of: Vec<usize>,
    // --- hand-drawn inline-select chrome ---
    accent: Color,
    label_idle: Color,
    field_bg: Option<Color>,
    // --- panel chrome (event_form look), used when `field_bar` is set ---
    /// When true the form renders as a centred, borderless, content-sized panel
    /// (heading + columns + submit bar + compact help) instead of the classic
    /// bordered full-area block. Driven by [`FormOptions::field_bar`].
    field_bar: bool,
    /// Fill behind the panel; `None` → transparent.
    panel_bg: Option<Color>,
    /// Style of the panel's submit bar.
    submit_style: Style,
    /// Verb on the panel's submit bar ("Save" unless the caller says otherwise).
    submit_label: String,
}

impl Form {
    /// Builds a form from field specs, an optional prefill map (`key -> value`),
    /// a resolved [`FormStyle`] and layout [`FormOptions`]. The first field
    /// receives focus.
    pub fn new(
        title: impl Into<String>,
        specs: Vec<FormFieldSpec>,
        prefill: &HashMap<String, String>,
        style: &FormStyle,
        options: &FormOptions,
    ) -> Self {
        let mut fields = Vec::with_capacity(specs.len());

        for spec in specs {
            let value = prefill
                .get(&spec.key)
                .cloned()
                .or_else(|| spec.default.clone());
            let condition = spec.visible_when.clone();

            let widget = match spec.kind {
                FormFieldKind::Text => {
                    let mut ti = TextInput::default()
                        .with_title(spec.label.clone())
                        .with_masked(spec.masked)
                        .with_inactive_style(style.text_inactive.clone())
                        .with_active_style(style.text_active.clone());
                    if let Some(v) = &value {
                        ti.attr(Attribute::Value, AttrValue::String(v.clone()));
                    }
                    FieldWidget::Text(ti)
                }
                FormFieldKind::DateTime { with_time } => {
                    let placeholder = if with_time {
                        "e.g. tomorrow 9am"
                    } else {
                        "e.g. next monday"
                    };
                    let mut ti = TextInput::default()
                        .with_title(spec.label.clone())
                        .with_placeholder(placeholder)
                        .with_inactive_style(style.text_inactive.clone())
                        .with_active_style(style.text_active.clone());
                    if let Some(v) = &value {
                        ti.attr(Attribute::Value, AttrValue::String(v.clone()));
                    }
                    FieldWidget::DateTime {
                        widget: ti,
                        with_time,
                    }
                }
                FormFieldKind::Select { options: opts } => {
                    let selected = value
                        .as_ref()
                        .and_then(|v| opts.iter().position(|o| o == v))
                        // No default: a *required* select starts on its first
                        // option. The selection follows the cursor (see
                        // `select_cmd`), and the cursor starts at 0 — leaving
                        // the list unselected would make the first option the
                        // one value the user cannot pick without stepping away
                        // and back, while an untouched required field blocks
                        // submission. An optional select keeps the empty
                        // `(none)` state: there, choosing nothing is a choice.
                        .or_else(|| (spec.required && !opts.is_empty()).then_some(0));
                    match options.select_style {
                        SelectStyle::Dropdown => {
                            let mut mc = MultiChoice::default()
                                .with_title(spec.label.clone())
                                .with_choices(opts.clone())
                                .with_mode(SelectionMode::Single)
                                .with_marker(SelectionMarker::Radio)
                                .with_max_height(MAX_VISIBLE)
                                .with_placeholder("(none)")
                                .with_inactive_style(style.select_inactive.clone())
                                .with_active_style(style.select_active.clone());
                            if let Some(i) = selected {
                                // Align both the selection *and* the cursor to the
                                // default, so the highlight starts on it and the
                                // first Move steps away from it, not from index 0.
                                for _ in 0..i {
                                    mc.perform(Cmd::Move(Direction::Down));
                                }
                                mc.attr(
                                    Attribute::Custom(MC_ATTR_SELECTED),
                                    AttrValue::Payload(PropPayload::Vec(vec![PropValue::Usize(i)])),
                                );
                            }
                            FieldWidget::Select {
                                widget: mc,
                                options: opts,
                            }
                        }
                        SelectStyle::Inline => {
                            let mut sl = SelectList::default()
                                .with_items(opts.clone())
                                .with_marker(SelectionMarker::Radio)
                                .with_mode(SelectionMode::Single)
                                .with_inactive_style(style.select_inline_inactive.clone())
                                .with_active_style(style.select_inline_active.clone());
                            if let Some(i) = selected {
                                for _ in 0..i {
                                    sl.perform(Cmd::Move(Direction::Down));
                                }
                                sl.attr(
                                    Attribute::Custom(SL_ATTR_SELECTED),
                                    AttrValue::Payload(PropPayload::Vec(vec![PropValue::Usize(i)])),
                                );
                            }
                            FieldWidget::SelectInline {
                                widget: sl,
                                options: opts,
                            }
                        }
                    }
                }
                FormFieldKind::Toggle => {
                    let on = matches!(value.as_deref(), Some("true") | Some("1") | Some("yes"));
                    let t = Toggle::default()
                        .with_title(spec.label.clone())
                        .with_value(on)
                        .with_inactive_style(style.toggle_inactive.clone())
                        .with_active_style(style.toggle_active.clone());
                    FieldWidget::Toggle(t)
                }
            };

            fields.push(FieldEntry {
                key: spec.key,
                label: spec.label,
                required: spec.required,
                widget,
                condition,
            });
        }

        let columns = options.column_count() as usize;
        let column_of = resolve_columns(&fields, columns, &options.column_of);

        let mut form = Self {
            title: title.into(),
            fields,
            focused: 0,
            field_errors: HashMap::new(),
            notice: None,
            initial_values: HashMap::new(),
            discard_pending: false,
            title_style: style.title,
            footer_style: style.footer,
            error_style: style.error,
            #[cfg(feature = "natural-date")]
            preview_color: style.preview,
            columns,
            column_of,
            accent: style.accent,
            label_idle: style.label_idle,
            field_bg: style.field_bg,
            field_bar: options.field_bar,
            panel_bg: style.panel_bg,
            submit_style: style.submit,
            submit_label: options
                .submit_label
                .clone()
                .unwrap_or_else(|| "Save".to_string()),
        };
        // Focus the first *visible* field (a leading field may be gated off from
        // the start by its default/prefill values).
        form.focused = form.first_visible().unwrap_or(0);
        form.set_focus(form.focused, true);
        form.initial_values = form.all_values();
        form
    }

    /// Puts a caller-owned message on the form, or clears it with `None`.
    /// See [`FormNotice`] for how it relates to the form's own validation error.
    pub fn set_notice(&mut self, notice: Option<FormNotice>) {
        self.notice = notice;
    }

    /// Moves focus to the field with `key`, if it exists and is currently
    /// visible. Returns whether it did. Lets a caller open a form on the field
    /// the user actually has to fill rather than always on the first one.
    pub fn focus_key(&mut self, key: &str) -> bool {
        let Some(idx) = self.fields.iter().position(|e| e.key == key) else {
            return false;
        };
        if !self.is_visible(idx) {
            return false;
        }
        self.focus_to(idx);
        true
    }

    /// Attaches per-field messages, replacing every message the form currently
    /// carries — including its own required-check. A round answers with the
    /// full set of errors, so merging would keep messages the script has already
    /// dropped. Keys naming no field are ignored: what to do with those is the
    /// caller's decision (the round protocol promotes them to the general
    /// message rather than swallowing them).
    pub fn set_field_errors(&mut self, errors: HashMap<String, String>) {
        self.field_errors = errors
            .into_iter()
            .filter(|(k, _)| self.fields.iter().any(|e| &e.key == k))
            .collect();
    }

    /// Drops the message on one field. Called automatically when that field's
    /// value changes, because the message was about the value that is now gone.
    pub fn clear_field_error(&mut self, key: &str) {
        self.field_errors.remove(key);
    }

    /// The messages currently attached to fields, keyed by field key.
    pub fn field_errors(&self) -> &HashMap<String, String> {
        &self.field_errors
    }

    /// Whether any field's value differs from the one the form was built with.
    /// This is what `esc` asks before throwing the form away; moving the focus
    /// around does not count, because it changes no value.
    pub fn is_dirty(&self) -> bool {
        self.initial_values != self.all_values()
    }

    /// Whether the `Discard your input?` prompt is up. While it is, every key
    /// goes to the prompt and none to the fields.
    pub fn discard_pending(&self) -> bool {
        self.discard_pending
    }

    /// The line shown below the fields: the focused field's message first, then
    /// the caller's notice, else `None` so the chrome falls back to its hints or
    /// submit bar.
    ///
    /// A field message outranks the notice while it stands, because it is about
    /// the field the user is looking at; the notice is about the form as a whole
    /// and stays until the caller replaces it.
    fn status_line(&self) -> Option<(String, Style)> {
        if let Some(e) = self.focused_error() {
            return Some((format!("⚠ {e}"), self.error_style));
        }
        match &self.notice {
            Some(FormNotice::Alert(t)) => Some((format!("⚠ {t}"), self.error_style)),
            Some(FormNotice::Info(t)) => Some((t.clone(), self.footer_style)),
            None => None,
        }
    }

    /// The message on the focused field, if it has one.
    fn focused_error(&self) -> Option<&str> {
        let key = &self.fields.get(self.focused)?.key;
        self.field_errors.get(key).map(String::as_str)
    }

    /// Per field, whether it carries a message — precomputed so the render
    /// loops, which hold the fields mutably, can consult it without borrowing
    /// `self` again.
    fn marked_fields(&self) -> Vec<bool> {
        self.fields
            .iter()
            .map(|e| self.field_errors.contains_key(&e.key))
            .collect()
    }

    /// Writes the focused field's message out in full under `footer` when it is
    /// too long for that one row, wrapped over at most [`ERROR_EXPAND_ROWS`]
    /// rows and overlapping whatever sits below — the border, the help line,
    /// the pane behind the popup.
    ///
    /// Overlapping is the point: the alternative is a message the user can only
    /// read half of, and a form that grows a row whenever a script complains
    /// would move every field under the user's hands. The expansion is tied to
    /// the focus, so it disappears the moment the user steps to another field.
    fn render_error_expansion(&self, frame: &mut Frame, footer: Rect) {
        let Some(text) = self.focused_error() else {
            return;
        };
        let text = format!("⚠ {text}");
        let width = footer.width as usize;
        if width == 0 || display_width(&text) <= width {
            return;
        }

        let rows = wrap_to_width(&text, width, ERROR_EXPAND_ROWS as usize);
        let bounds = frame.area();
        for (i, row) in rows.iter().enumerate() {
            let y = footer.y + i as u16;
            if y >= bounds.y + bounds.height {
                break;
            }
            let rect = Rect {
                x: footer.x,
                y,
                width: footer.width.min(bounds.x + bounds.width - footer.x),
                height: 1,
            };
            frame.render_widget(Clear, rect);
            if let Some(bg) = self.panel_bg {
                frame.render_widget(Block::default().style(Style::default().bg(bg)), rect);
            }
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(row.clone(), self.error_style))),
                rect,
            );
        }
    }

    // --- conditional visibility ------------------------------------------

    /// Current string value of the field with `key`, if present (used to
    /// evaluate another field's visibility condition).
    fn current_value(&self, key: &str) -> Option<String> {
        self.fields
            .iter()
            .find(|e| e.key == key)
            .map(|e| field_value(&e.widget))
    }

    /// Whether field `idx` is currently shown: `true` when it has no condition,
    /// else the condition evaluated against the controller's live value.
    fn is_visible(&self, idx: usize) -> bool {
        let Some(entry) = self.fields.get(idx) else {
            return false;
        };
        match &entry.condition {
            None => true,
            Some(cond) => {
                let val = self.current_value(&cond.field).unwrap_or_default();
                let matched = cond.equals_any.iter().any(|v| *v == val);
                matched ^ cond.negate
            }
        }
    }

    /// Precomputes the visibility of every field once, so the mutable render
    /// loops can consult it without re-borrowing `self`.
    fn visibility(&self) -> Vec<bool> {
        (0..self.fields.len()).map(|i| self.is_visible(i)).collect()
    }

    /// Index of the first visible field, if any.
    fn first_visible(&self) -> Option<usize> {
        (0..self.fields.len()).find(|&i| self.is_visible(i))
    }

    /// The next visible field from `from` stepping by `dir` (±1), wrapping.
    /// `None` when no field other than the start is visible.
    fn step_visible(&self, from: usize, dir: isize) -> Option<usize> {
        let n = self.fields.len();
        if n == 0 {
            return None;
        }
        let mut i = from as isize;
        for _ in 0..n {
            i = (i + dir).rem_euclid(n as isize);
            if self.is_visible(i as usize) {
                return Some(i as usize);
            }
        }
        None
    }

    /// After a value change may have hidden the focused field, move focus to a
    /// visible one.
    fn ensure_focus_visible(&mut self) {
        if !self.is_visible(self.focused) {
            if let Some(idx) = self.first_visible() {
                self.focus_to(idx);
            }
        }
    }

    // --- keyboard driving -------------------------------------------------

    /// Feeds one normalized key string and returns the resulting [`FormEvent`].
    pub fn handle_key(&mut self, key: &str) -> FormEvent {
        // While the discard prompt is up it owns every key: the form underneath
        // must not take another character on the way out.
        if self.discard_pending {
            return match key {
                "y" | "Y" => FormEvent::Cancelled,
                // `n` and `esc` both mean "back to the form", so `esc esc` can
                // never throw work away — the second one answers the prompt.
                _ => {
                    self.discard_pending = false;
                    FormEvent::Consumed
                }
            };
        }

        match key {
            // Cancelling outright is only free while nothing has been entered.
            "esc" => {
                if self.is_dirty() {
                    self.discard_pending = true;
                    return FormEvent::Consumed;
                }
                return FormEvent::Cancelled;
            }
            // `ctrl+enter` is the panel look's submit; plain `enter` stays a
            // submit too (classic forms, and a harmless convenience).
            "enter" | "ctrl+enter" => return self.submit(),
            // Field navigation: `tab`/`shift+tab` (classic) and `ctrl+j`/`ctrl+k`
            // (event_form look) all move between fields.
            "tab" | "ctrl+j" => {
                self.focus_next();
                return FormEvent::Consumed;
            }
            "shift+tab" | "ctrl+k" => {
                self.focus_prev();
                return FormEvent::Consumed;
            }
            _ => {}
        }

        // up/down navigate fields, unless the focused field is a select (there
        // they move the option cursor).
        if key == "up" || key == "down" {
            if !self.focused_is_select() {
                if key == "up" {
                    self.focus_prev();
                } else {
                    self.focus_next();
                }
                return FormEvent::Consumed;
            }
        }

        // A field's message is about the value it was raised against. Compare
        // the value across the dispatch rather than guessing from the key:
        // `left` and `home` change nothing, a paste changes everything, and
        // only the widget knows which is which.
        let before = self.focused_value();
        self.dispatch(key);
        if self.focused_value() != before {
            if let Some(key) = self.fields.get(self.focused).map(|e| e.key.clone()) {
                self.clear_field_error(&key);
            }
        }
        // The change may have flipped another field's visibility condition; if
        // it hid the focused field, move focus to one still on screen.
        self.ensure_focus_visible();
        FormEvent::Consumed
    }

    /// The focused field's current value (empty when there is no field).
    fn focused_value(&self) -> String {
        self.fields
            .get(self.focused)
            .map(|e| field_value(&e.widget))
            .unwrap_or_default()
    }

    fn dispatch(&mut self, key: &str) {
        let Some(entry) = self.fields.get_mut(self.focused) else {
            return;
        };
        match &mut entry.widget {
            FieldWidget::Text(w) | FieldWidget::DateTime { widget: w, .. } => {
                if let Some(cmd) = text_cmd(key) {
                    w.perform(cmd);
                }
            }
            FieldWidget::Select { widget, .. } => {
                if let Some(cmd) = select_cmd(key) {
                    // Single-choice: the selection follows the cursor, so moving
                    // it also picks the newly-highlighted option.
                    if let CmdResult::Changed(State::Single(StateValue::Usize(i))) =
                        widget.perform(cmd)
                    {
                        widget.attr(
                            Attribute::Custom(MC_ATTR_SELECTED),
                            AttrValue::Payload(PropPayload::Vec(vec![PropValue::Usize(i)])),
                        );
                    }
                }
            }
            FieldWidget::SelectInline { widget, .. } => {
                if let Some(cmd) = select_inline_cmd(key) {
                    if let CmdResult::Changed(State::Single(StateValue::Usize(i))) =
                        widget.perform(cmd)
                    {
                        widget.attr(
                            Attribute::Custom(SL_ATTR_SELECTED),
                            AttrValue::Payload(PropPayload::Vec(vec![PropValue::Usize(i)])),
                        );
                    }
                }
            }
            FieldWidget::Toggle(w) => {
                if key == " " || key == "space" {
                    w.perform(Cmd::Toggle);
                }
            }
        }
    }

    fn submit(&mut self) -> FormEvent {
        // Only visible fields are validated: a required field gated off by a
        // condition must not block submission.
        let invalid = self.fields.iter().enumerate().position(|(i, e)| {
            self.is_visible(i) && e.required && field_value(&e.widget).trim().is_empty()
        });
        if let Some(idx) = invalid {
            self.focus_to(idx);
            if let Some(key) = self.fields.get(idx).map(|e| e.key.clone()) {
                self.field_errors
                    .insert(key, "This field is required".to_string());
            }
            return FormEvent::Consumed;
        }
        FormEvent::Submitted(self.values())
    }

    // --- focus ------------------------------------------------------------

    fn focused_is_select(&self) -> bool {
        matches!(
            self.fields.get(self.focused).map(|e| &e.widget),
            Some(FieldWidget::Select { .. }) | Some(FieldWidget::SelectInline { .. })
        )
    }

    fn set_focus(&mut self, idx: usize, focus: bool) {
        if let Some(entry) = self.fields.get_mut(idx) {
            let flag = AttrValue::Flag(focus);
            match &mut entry.widget {
                FieldWidget::Text(w) | FieldWidget::DateTime { widget: w, .. } => {
                    w.attr(Attribute::Focus, flag)
                }
                FieldWidget::Select { widget, .. } => widget.attr(Attribute::Focus, flag),
                FieldWidget::SelectInline { widget, .. } => widget.attr(Attribute::Focus, flag),
                FieldWidget::Toggle(w) => w.attr(Attribute::Focus, flag),
            }
        }
    }

    fn focus_to(&mut self, idx: usize) {
        if self.fields.is_empty() {
            return;
        }
        self.set_focus(self.focused, false);
        self.focused = idx % self.fields.len();
        self.set_focus(self.focused, true);
    }

    fn focus_next(&mut self) {
        if let Some(idx) = self.step_visible(self.focused, 1) {
            self.focus_to(idx);
        }
    }

    fn focus_prev(&mut self) {
        if let Some(idx) = self.step_visible(self.focused, -1) {
            self.focus_to(idx);
        }
    }

    // --- values -----------------------------------------------------------

    /// Collects the current `key -> value` map. Selects return the option label
    /// (empty when nothing is picked); toggles return `"true"` / `"false"`.
    /// Fields hidden by an unmet [`FieldCondition`] are omitted, so the caller
    /// never receives a stale value for a field the user could not see.
    pub fn values(&self) -> HashMap<String, String> {
        self.fields
            .iter()
            .enumerate()
            .filter(|(i, _)| self.is_visible(*i))
            .map(|(_, e)| (e.key.clone(), field_value(&e.widget)))
            .collect()
    }

    /// Every field's value, hidden ones included. Only the dirty check uses
    /// this: a value typed into a field that a later toggle hid is still the
    /// user's work, and `esc` must not drop it without asking.
    fn all_values(&self) -> HashMap<String, String> {
        self.fields
            .iter()
            .map(|e| (e.key.clone(), field_value(&e.widget)))
            .collect()
    }

    // --- rendering --------------------------------------------------------

    /// Renders the form into `area`. With `field_bar` set this is a centred,
    /// borderless, content-sized panel (the `event_form` look); otherwise the
    /// classic bordered full-area block. The focused text/date field places the
    /// terminal cursor itself via the backing widget.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        if area.width < 4 || area.height < 3 {
            return;
        }
        if self.field_bar {
            self.render_panel(frame, area);
        } else {
            self.render_classic(frame, area);
        }
        if self.discard_pending {
            render_discard_prompt(
                frame,
                area,
                self.error_style,
                self.footer_style,
                self.panel_bg,
            );
        }
    }

    /// The classic look: a bordered block filling `area`, single footer row.
    fn render_classic(&mut self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.title_style)
            .title(Line::from(Span::styled(
                self.title.clone(),
                self.title_style,
            )));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Footer occupies the last inner row; fields fill the rest.
        let footer_y = inner.y + inner.height - 1;
        let fields_bottom = footer_y; // exclusive bound for field rows

        let focused_idx = self.focused;
        #[cfg(feature = "natural-date")]
        let preview_color = self.preview_color;
        let accent = self.accent;
        let label_idle = self.label_idle;
        let field_bg = self.field_bg;

        // Column geometry: one or two side-by-side strips, each with its own
        // y-cursor. Field/focus order stays the spec order.
        let cols = self.columns.max(1);
        let gap = if cols >= 2 { 3u16 } else { 0 };
        let col_w = if cols >= 2 {
            inner.width.saturating_sub(gap) / 2
        } else {
            inner.width
        };
        let col_x = [inner.x, inner.x + col_w + gap];
        let mut col_y = [inner.y; 2];

        let visible = self.visibility();
        let marked = self.marked_fields();
        let error_fg = self.error_style.fg;
        for (i, entry) in self.fields.iter_mut().enumerate() {
            if !visible[i] {
                continue;
            }
            let c = self.column_of.get(i).copied().unwrap_or(0).min(cols - 1);
            let focused = i == focused_idx;
            let h = field_height(&entry.widget, focused);
            let y = col_y[c.min(1)];
            if y >= fields_bottom {
                continue;
            }
            let avail = fields_bottom - y;
            let rect = Rect {
                x: col_x[c.min(1)],
                y,
                width: col_w,
                height: h.min(avail),
            };
            render_field(
                frame,
                rect,
                entry,
                focused,
                accent,
                label_idle,
                field_bg,
                #[cfg(feature = "natural-date")]
                preview_color,
            );
            // A field the caller (or the required-check) objected to wears the
            // error colour on its label and gutter. Restyling the cells the
            // widget just drew keeps the marking one rule for every field kind
            // and leaves the height alone — the text itself is in the footer,
            // so nothing on screen moves when a message arrives.
            if marked[i] {
                if let Some(c) = error_fg {
                    mark_error_field(frame, rect, c);
                }
            }
            col_y[c.min(1)] = y + rect.height + 1; // one-row gap between fields
        }

        // Footer: a validation error or the caller's notice takes precedence
        // over the key hint.
        let (text, fstyle) = self.status_line().unwrap_or_else(|| {
            (
                "tab move · ↑↓ navigate/pick · space toggle · enter save · esc cancel".to_string(),
                self.footer_style,
            )
        });
        let footer_rect = Rect {
            x: inner.x,
            y: footer_y,
            width: inner.width,
            height: 1,
        };
        // One row, so a message wider than the form is cut with an ellipsis
        // rather than silently halved. A *field* message says more than that:
        // it is written out below (see `render_error_expansion`).
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                truncate_to_width(&text, footer_rect.width as usize),
                fstyle,
            ))),
            footer_rect,
        );
        self.render_error_expansion(frame, footer_rect);
    }

    /// The `event_form` look: a centred, borderless, content-sized panel filled
    /// with `panel_bg`, a `✦ {title}` heading, the field columns, a highlighted
    /// submit bar and a compact key-hint line. Mirrors
    /// `examples/event_form.rs::render`.
    fn render_panel(&mut self, frame: &mut Frame, area: Rect) {
        let cols = self.columns.max(1);
        let focused_idx = self.focused;

        // Content height = the taller column (each field plus a one-row gap).
        // Hidden fields take no space.
        let visible = self.visibility();
        let mut col_heights = [0u16, 0u16];
        for (i, entry) in self.fields.iter().enumerate() {
            if !visible[i] {
                continue;
            }
            let c = self
                .column_of
                .get(i)
                .copied()
                .unwrap_or(0)
                .min(cols - 1)
                .min(1);
            col_heights[c] += field_height(&entry.widget, i == focused_idx) + 1;
        }
        let content_h = col_heights[0].max(col_heights[1]);

        // Panel geometry: pad(1) + heading(1) + gap(1) + content + submit(1) +
        // gap(1) + help(1) → content + 6. `content_h` already carries a
        // trailing gap row per field, which is the single blank line between
        // the last field and the submit bar; adding more padding on top of it
        // just left a hole in a two-field dialog.
        let want_w = if cols >= 2 { 78 } else { 52 };
        let panel_w = want_w.min(area.width);
        let panel_h = (content_h + 6).min(area.height);
        let px = area.x + area.width.saturating_sub(panel_w) / 2;
        let py = area.y + area.height.saturating_sub(panel_h) / 2;
        let panel = Rect::new(px, py, panel_w, panel_h);

        // Floating panel: clear whatever is behind, then fill (no border).
        frame.render_widget(Clear, panel);
        if let Some(bg) = self.panel_bg {
            frame.render_widget(Block::default().style(Style::default().bg(bg)), panel);
        }

        let inner = Rect::new(
            panel.x + 2,
            panel.y + 1,
            panel.width.saturating_sub(4),
            panel.height.saturating_sub(2),
        );

        // Heading: `✦ {title}`.
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("\u{2726} ", Style::default().fg(self.accent)),
                Span::styled(self.title.clone(), self.title_style),
            ])),
            Rect { height: 1, ..inner },
        );

        // Bottom rows: help on the last inner row, submit two rows above it.
        let help_y = panel.y + panel.height.saturating_sub(1);
        let submit_y = panel.y + panel.height.saturating_sub(3);

        // Columns between the heading and the submit bar.
        let content_top = inner.y + 2;
        let content_bottom = submit_y; // exclusive
        let gap = if cols >= 2 { 3u16 } else { 0 };
        let col_w = if cols >= 2 {
            inner.width.saturating_sub(gap) / 2
        } else {
            inner.width
        };
        let col_x = [inner.x, inner.x + col_w + gap];
        let mut col_y = [content_top; 2];

        let accent = self.accent;
        let label_idle = self.label_idle;
        let field_bg = self.field_bg;
        #[cfg(feature = "natural-date")]
        let preview_color = self.preview_color;
        let marked = self.marked_fields();
        let error_fg = self.error_style.fg;

        for (i, entry) in self.fields.iter_mut().enumerate() {
            if !visible[i] {
                continue;
            }
            let c = self
                .column_of
                .get(i)
                .copied()
                .unwrap_or(0)
                .min(cols - 1)
                .min(1);
            let focused = i == focused_idx;
            let h = field_height(&entry.widget, focused);
            let y = col_y[c];
            if y >= content_bottom {
                continue;
            }
            let avail = content_bottom - y;
            let rect = Rect {
                x: col_x[c],
                y,
                width: col_w,
                height: h.min(avail),
            };
            render_field(
                frame,
                rect,
                entry,
                focused,
                accent,
                label_idle,
                field_bg,
                #[cfg(feature = "natural-date")]
                preview_color,
            );
            // A field the caller (or the required-check) objected to wears the
            // error colour on its label and gutter. Restyling the cells the
            // widget just drew keeps the marking one rule for every field kind
            // and leaves the height alone — the text itself is in the footer,
            // so nothing on screen moves when a message arrives.
            if marked[i] {
                if let Some(c) = error_fg {
                    mark_error_field(frame, rect, c);
                }
            }
            col_y[c] = y + rect.height + 1;
        }

        // Submit bar (panel-wide) — or, in its place, whatever the form or its
        // caller has to say about the last attempt.
        match self.status_line() {
            Some((text, style)) => frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    truncate_to_width(&format!("  {text}  "), inner.width as usize),
                    style,
                ))),
                Rect::new(inner.x, submit_y, inner.width, 1),
            ),
            None => frame.render_widget(
                Paragraph::new(format!(
                    "  [ Ctrl+\u{21b5} \u{2192} {} ]  ",
                    self.submit_label
                ))
                .style(self.submit_style),
                Rect::new(inner.x, submit_y, inner.width, 1),
            ),
        }
        self.render_error_expansion(frame, Rect::new(inner.x, submit_y, inner.width, 1));

        // Compact help line.
        let key = Style::default().fg(accent).add_modifier(Modifier::BOLD);
        let dim = self.footer_style;
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" C-j", key),
                Span::styled(" next  ", dim),
                Span::styled("C-k", key),
                Span::styled(" prev  ", dim),
                Span::styled("\u{2191}\u{2193}/C-l/h", key),
                Span::styled(" pick  ", dim),
                Span::styled("Spc", key),
                Span::styled(" toggle  ", dim),
                Span::styled("C-\u{21b5}", key),
                Span::styled(" save  ", dim),
                Span::styled("Esc", key),
                Span::styled(" quit", dim),
            ])),
            Rect::new(inner.x, help_y, inner.width, 1),
        );
    }
}

// --- free helpers ---------------------------------------------------------

/// Resolves the per-field column index. An explicit assignment (from config)
/// wins; otherwise for two columns the fields are balanced by running height,
/// and for one column everything lands in column 0.
fn resolve_columns(fields: &[FieldEntry], cols: usize, explicit: &[usize]) -> Vec<usize> {
    if cols <= 1 {
        return vec![0; fields.len()];
    }
    if explicit.len() == fields.len() {
        return explicit.iter().map(|c| (*c).min(cols - 1)).collect();
    }
    let mut heights = [0u16, 0u16];
    let mut out = Vec::with_capacity(fields.len());
    for e in fields {
        let c = if heights[0] <= heights[1] { 0 } else { 1 };
        heights[c] += field_height(&e.widget, false) + 1;
        out.push(c);
    }
    out
}

fn text_cmd(key: &str) -> Option<Cmd> {
    match key {
        "backspace" => Some(Cmd::Delete),
        "delete" => Some(Cmd::Custom("delete_fwd")),
        "left" => Some(Cmd::Move(Direction::Left)),
        "right" => Some(Cmd::Move(Direction::Right)),
        "home" => Some(Cmd::Custom(CMD_HOME)),
        "end" => Some(Cmd::Custom(CMD_END)),
        _ => {
            let mut chars = key.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(Cmd::Type(c)),
                _ => None,
            }
        }
    }
}

/// Movement for a single-choice dropdown. There is intentionally no `space`
/// pick: the selection follows the cursor (see [`Form::dispatch`]), so a
/// `Cmd::Toggle` here would only ever *deselect* the highlighted option.
fn select_cmd(key: &str) -> Option<Cmd> {
    match key {
        "up" => Some(Cmd::Move(Direction::Up)),
        "down" => Some(Cmd::Move(Direction::Down)),
        _ => None,
    }
}

/// Movement for an inline [`SelectList`]. Like [`select_cmd`], no `space` pick —
/// the selection tracks the cursor.
fn select_inline_cmd(key: &str) -> Option<Cmd> {
    match key {
        "up" | "ctrl+h" => Some(Cmd::Move(Direction::Up)),
        "down" | "ctrl+l" => Some(Cmd::Move(Direction::Down)),
        _ => None,
    }
}

fn field_value(widget: &FieldWidget) -> String {
    match widget {
        FieldWidget::Text(ti) | FieldWidget::DateTime { widget: ti, .. } => match ti.state() {
            State::Single(StateValue::String(s)) => s,
            _ => String::new(),
        },
        FieldWidget::Select { widget, options } => match widget.state() {
            State::Vec(vals) => vals
                .into_iter()
                .find_map(|v| match v {
                    StateValue::Usize(i) => options.get(i).cloned(),
                    _ => None,
                })
                .unwrap_or_default(),
            _ => String::new(),
        },
        FieldWidget::SelectInline { widget, options } => match widget.state() {
            State::Vec(vals) => vals
                .into_iter()
                .find_map(|v| match v {
                    StateValue::Usize(i) => options.get(i).cloned(),
                    _ => None,
                })
                .unwrap_or_default(),
            _ => String::new(),
        },
        FieldWidget::Toggle(t) => {
            if t.is_on() {
                "true".into()
            } else {
                "false".into()
            }
        }
    }
}

fn field_height(widget: &FieldWidget, focused: bool) -> u16 {
    match widget {
        // Label + input, and only when the field actually carries a message
        // the third row it would be drawn on. Reserving that row
        // unconditionally left an empty line under every text field — and the
        // driver reports validation errors in the footer anyway, so for most
        // forms the row is never used at all.
        FieldWidget::Text(w) => {
            if w.error.is_some() {
                3
            } else {
                2
            }
        }
        FieldWidget::DateTime { .. } => datetime_height(),
        FieldWidget::Toggle(_) => 2,
        FieldWidget::Select { options, .. } => {
            if focused {
                2 + (options.len() as u16).min(MAX_VISIBLE)
            } else {
                2
            }
        }
        // Inline selects always show every option: label row + one row each.
        FieldWidget::SelectInline { options, .. } => 1 + options.len() as u16,
    }
}

#[cfg(feature = "natural-date")]
fn datetime_height() -> u16 {
    4
}

#[cfg(not(feature = "natural-date"))]
fn datetime_height() -> u16 {
    3
}

#[allow(clippy::too_many_arguments)]
fn render_field(
    frame: &mut Frame,
    area: Rect,
    entry: &mut FieldEntry,
    focused: bool,
    accent: Color,
    label_idle: Color,
    field_bg: Option<Color>,
    #[cfg(feature = "natural-date")] preview_color: Color,
) {
    let label = entry.label.clone();
    match &mut entry.widget {
        FieldWidget::Text(w) => w.view(frame, area),
        FieldWidget::Toggle(w) => w.view(frame, area),
        FieldWidget::Select { widget, .. } => widget.view(frame, area),
        FieldWidget::SelectInline { widget, options } => {
            render_inline_select(
                frame,
                area,
                &label,
                widget,
                options.len() as u16,
                focused,
                accent,
                label_idle,
                field_bg,
            );
        }
        FieldWidget::DateTime { widget, with_time } => {
            let ti_area = Rect {
                height: area.height.min(3),
                ..area
            };
            let _with_time = *with_time;
            widget.view(frame, ti_area);
            #[cfg(feature = "natural-date")]
            {
                if area.height > 3 {
                    let value = match widget.state() {
                        State::Single(StateValue::String(s)) => s,
                        _ => String::new(),
                    };
                    let text = match super::preview::datetime_preview(
                        &value,
                        _with_time,
                        chrono::Local::now(),
                    ) {
                        Some(s) => format!("↳ {s}"),
                        None => String::new(),
                    };
                    frame.render_widget(
                        Paragraph::new(Line::from(Span::styled(
                            text,
                            Style::default().fg(preview_color),
                        ))),
                        Rect {
                            x: area.x,
                            y: area.y + 3,
                            width: area.width,
                            height: 1,
                        },
                    );
                }
            }
        }
    }
}

/// Hand-draws an inline select's `▍`-prefixed label + a `▍ ` gutter beside the
/// (title-less) [`SelectList`], mirroring the other widgets' chrome. The list is
/// shifted right by [`GUTTER`] so its options align under the label text.
#[allow(clippy::too_many_arguments)]
fn render_inline_select(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    widget: &mut SelectList,
    n_options: u16,
    focused: bool,
    accent: Color,
    label_idle: Color,
    field_bg: Option<Color>,
) {
    let bold = Modifier::BOLD;
    let prefix_fg = if focused { accent } else { label_idle };
    let bar_bg = if focused { field_bg } else { None };

    // Label line: `▍ Title`.
    let mut label_style = Style::default().fg(prefix_fg).add_modifier(bold);
    if let Some(bg) = bar_bg {
        label_style = label_style.bg(bg);
    }
    let mut para = Paragraph::new(Line::from(vec![
        Span::styled("\u{258d} ", Style::default().fg(prefix_fg)),
        Span::styled(label.to_string(), label_style),
    ]));
    if let Some(bg) = bar_bg {
        para = para.style(Style::default().bg(bg));
    }
    frame.render_widget(
        para,
        Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        },
    );

    // Options + gutter stripe under the label.
    let rows = n_options.min(area.height.saturating_sub(1));
    if rows == 0 {
        return;
    }
    widget.view(
        frame,
        Rect {
            x: area.x + GUTTER,
            y: area.y + 1,
            width: area.width.saturating_sub(GUTTER),
            height: rows,
        },
    );
    let mut bar_style = Style::default().fg(prefix_fg);
    if let Some(bg) = bar_bg {
        bar_style = bar_style.bg(bg);
    }
    for iy in 0..rows {
        frame.render_widget(
            Paragraph::new(Span::styled("\u{258d} ", bar_style)),
            Rect {
                x: area.x,
                y: area.y + 1 + iy,
                width: GUTTER,
                height: 1,
            },
        );
    }
}

/// Display width of `s` in terminal columns (wide characters count double).
fn display_width(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// Wraps `s` into at most `max_rows` rows of `width` columns, breaking on
/// spaces and only inside a word when a single word is wider than the row. When
/// the text does not fit in `max_rows`, the last row is cut with an ellipsis, so
/// a run-on message is visibly truncated rather than silently halved.
fn wrap_to_width(s: &str, width: usize, max_rows: usize) -> Vec<String> {
    if width == 0 || max_rows == 0 {
        return Vec::new();
    }
    let mut rows: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_w = 0usize;

    let mut push_row = |row: &mut String, w: &mut usize| {
        rows.push(std::mem::take(row));
        *w = 0;
    };

    for word in s.split(' ') {
        let word_w = display_width(word);
        if current_w > 0 && current_w + 1 + word_w > width {
            push_row(&mut current, &mut current_w);
        }
        if word_w > width {
            // A single word wider than the row: fill the row, then carry the
            // rest into the next one.
            for ch in word.chars() {
                let cw = ch.width().unwrap_or(0);
                if current_w + cw > width {
                    push_row(&mut current, &mut current_w);
                }
                current.push(ch);
                current_w += cw;
            }
            continue;
        }
        if current_w > 0 {
            current.push(' ');
            current_w += 1;
        }
        current.push_str(word);
        current_w += word_w;
    }
    if !current.is_empty() {
        rows.push(current);
    }

    if rows.len() > max_rows {
        rows.truncate(max_rows);
        if let Some(last) = rows.last_mut() {
            *last = truncate_to_width(&format!("{last} "), width);
        }
    }
    rows
}

/// Repaints a field's label row and its `▍` gutter in `color`.
///
/// Restyling the cells the widget has just drawn — rather than re-rendering the
/// field with a second set of styles — keeps the marking to one rule for every
/// field kind, including the hand-drawn inline select, and cannot drift out of
/// step with what a widget actually put on screen.
fn mark_error_field(frame: &mut Frame, area: Rect, color: Color) {
    let buf = frame.buffer_mut();
    for dx in 0..area.width {
        if let Some(cell) = buf.cell_mut((area.x + dx, area.y)) {
            cell.set_fg(color);
        }
    }
    for dy in 1..area.height {
        for dx in 0..PREFIX_LEN.min(area.width) {
            if let Some(cell) = buf.cell_mut((area.x + dx, area.y + dy)) {
                cell.set_fg(color);
            }
        }
    }
}

/// Draws the `Discard your input?` prompt as a small centred box over the form.
///
/// It defaults to keeping the input: anything but `y` returns to the form, so
/// `esc esc` — the reflex for "get me out of here" — can never throw work away.
fn render_discard_prompt(
    frame: &mut Frame,
    area: Rect,
    error_style: Style,
    footer_style: Style,
    panel_bg: Option<Color>,
) {
    const QUESTION: &str = "Discard your input?";
    const HINT: &str = "y discard · any other key keep";

    let want_w = (display_width(HINT).max(display_width(QUESTION)) + 4) as u16;
    let w = want_w.min(area.width);
    let h = 3u16.min(area.height);
    if w < 4 || h < 3 {
        return;
    }
    let rect = Rect::new(
        area.x + area.width.saturating_sub(w) / 2,
        area.y + area.height.saturating_sub(h) / 2,
        w,
        h,
    );

    frame.render_widget(Clear, rect);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(error_style)
        .title(Line::from(Span::styled(QUESTION, error_style)));
    if let Some(bg) = panel_bg {
        block = block.style(Style::default().bg(bg));
    }
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(HINT, footer_style))),
        inner,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::form::{FormFieldSpec, FormOptions, FormStyle};

    fn build(specs: Vec<FormFieldSpec>) -> Form {
        Form::new(
            "T",
            specs,
            &HashMap::new(),
            &FormStyle::default(),
            &FormOptions::default(),
        )
    }

    /// A select's default value gates a dependent field's initial visibility:
    /// present when the condition holds, absent when it does not.
    #[test]
    fn conditional_field_hidden_by_controller_default() {
        let hidden = build(vec![
            FormFieldSpec::select("kind", "Kind", vec!["call".into(), "meeting".into()])
                .with_default("call"),
            FormFieldSpec::text("attendees", "Attendees").visible_when("kind", "meeting"),
        ]);
        assert!(hidden.values().contains_key("kind"));
        assert!(
            !hidden.values().contains_key("attendees"),
            "attendees must be hidden while kind=call"
        );

        let shown = build(vec![
            FormFieldSpec::select("kind", "Kind", vec!["call".into(), "meeting".into()])
                .with_default("meeting"),
            FormFieldSpec::text("attendees", "Attendees").visible_when("kind", "meeting"),
        ]);
        assert!(shown.values().contains_key("attendees"));
    }

    /// Toggling the controller flips a dependent field in and out of the
    /// collected values, and focus navigation skips it while hidden.
    #[test]
    fn toggle_reveals_dependent_field_and_focus_skips_it() {
        let mut form = build(vec![
            FormFieldSpec::toggle("adv", "Advanced"),
            FormFieldSpec::text("detail", "Detail").visible_when("adv", "true"),
        ]);
        // Toggle defaults off → detail hidden, focus on the toggle, and Tab has
        // nowhere else visible to go.
        assert_eq!(form.focused, 0);
        assert!(!form.values().contains_key("detail"));
        form.handle_key("tab");
        assert_eq!(form.focused, 0, "focus must skip the hidden field");

        // Turn the toggle on → detail appears and becomes reachable.
        form.handle_key(" ");
        assert!(form.values().contains_key("detail"));
        form.handle_key("tab");
        assert_eq!(form.focused, 1, "detail is now focusable");
    }

    /// In a single-choice select the selection tracks the cursor: navigating
    /// with `up`/`down` immediately picks the highlighted option, no separate
    /// `space` press. With a default the cursor starts aligned to it.
    #[test]
    fn single_select_selection_follows_cursor() {
        let mut form = build(vec![
            FormFieldSpec::select("k", "K", vec!["a".into(), "b".into(), "c".into()])
                .with_default("b"),
        ]);
        // Cursor starts on the default; navigating re-selects the neighbour.
        assert_eq!(form.values().get("k").map(String::as_str), Some("b"));
        form.handle_key("down");
        assert_eq!(form.values().get("k").map(String::as_str), Some("c"));
        form.handle_key("up");
        assert_eq!(form.values().get("k").map(String::as_str), Some("b"));
        form.handle_key("up");
        assert_eq!(form.values().get("k").map(String::as_str), Some("a"));

        // Same for an inline select.
        let mut inline = Form::new(
            "T",
            vec![FormFieldSpec::select("k", "K", vec!["a".into(), "b".into()]).with_default("a")],
            &HashMap::new(),
            &FormStyle::default(),
            &FormOptions {
                select_style: SelectStyle::Inline,
                ..FormOptions::default()
            },
        );
        assert_eq!(inline.values().get("k").map(String::as_str), Some("a"));
        inline.handle_key("down");
        assert_eq!(inline.values().get("k").map(String::as_str), Some("b"));
    }

    /// Without a default, a *required* select starts on its first option — the
    /// selection follows the cursor, and the cursor starts there. An optional
    /// select stays empty so `(none)` remains reachable.
    #[test]
    fn required_select_without_default_starts_on_first_option() {
        let mut form = build(vec![FormFieldSpec::select(
            "k",
            "K",
            vec!["a".into(), "b".into()],
        )]);
        assert_eq!(form.values().get("k").map(String::as_str), Some("a"));
        assert!(
            matches!(form.handle_key("enter"), FormEvent::Submitted(_)),
            "an untouched required select must not block submit"
        );

        let optional = build(vec![
            FormFieldSpec::select("k", "K", vec!["a".into(), "b".into()]).optional(),
        ]);
        assert_eq!(optional.values().get("k").map(String::as_str), Some(""));

        // Inline look, same rule.
        let inline = Form::new(
            "T",
            vec![FormFieldSpec::select(
                "k",
                "K",
                vec!["a".into(), "b".into()],
            )],
            &HashMap::new(),
            &FormStyle::default(),
            &FormOptions {
                select_style: SelectStyle::Inline,
                ..FormOptions::default()
            },
        );
        assert_eq!(inline.values().get("k").map(String::as_str), Some("a"));
    }

    /// A required field that is hidden by its condition must not block submit;
    /// once revealed and still empty, it does.
    #[test]
    fn hidden_required_field_does_not_block_submit() {
        let mut form = build(vec![
            FormFieldSpec::toggle("adv", "Advanced"),
            FormFieldSpec::text("detail", "Detail").visible_when("adv", "true"),
        ]);
        assert!(
            matches!(form.handle_key("enter"), FormEvent::Submitted(_)),
            "empty required field is hidden → submit succeeds"
        );

        form.handle_key(" "); // reveal detail (required, empty)
        assert!(
            matches!(form.handle_key("enter"), FormEvent::Consumed),
            "revealed empty required field blocks submit"
        );
    }
}
