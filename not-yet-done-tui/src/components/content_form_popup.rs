//! TUI adapter for the generic [`not_yet_done_ratatui::Form`] driver.
//!
//! The reusable form state machine + rendering lives in `not-yet-done-ratatui`;
//! this module only bridges it to the two things the widget crate deliberately
//! doesn't know about:
//!
//! * the protocol spec type [`not_yet_done_content::FormFieldSpec`] (converted
//!   to the crate's own [`FormFieldSpec`](RForm)), and
//! * the app [`Theme`] (translated to a [`FormPalette`]/[`FormStyle`]).
//!
//! Because the driver's backing widgets are retained-mode and take their styles
//! at construction, the [`Theme`] is required in [`ContentFormPopup::new`] and
//! [`render`](ContentFormPopup::render) takes `&mut self` (the focused text
//! widget places the terminal cursor itself).

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use ratatui::Frame;
use ratatui::layout::Rect;

use not_yet_done_ratatui::{
    FieldCondition as RFieldCondition, Form, FormEvent, FormFieldKind as RFormKind,
    FormFieldSpec as RFormSpec, FormNotice, FormOptions, FormPalette, FormStyle,
};

use not_yet_done_content::{FieldError, FormFieldKind, FormFieldSpec};

use crate::ui::theme::Theme;

/// Outcome of feeding a key to the popup (thin re-shape of [`FormEvent`]).
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
    form: Form,
}

impl ContentFormPopup {
    /// Build the popup from the action's field specs. `prefill` (from
    /// [`not_yet_done_content::Node::form_prep`]) overrides each field's
    /// static [`FormFieldSpec::default`]. The [`Theme`] is baked into the
    /// backing widgets' styles at construction.
    pub fn new(
        title: impl Into<String>,
        fields: Vec<FormFieldSpec>,
        prefill: &HashMap<String, String>,
        theme: &Theme,
        options: &FormOptions,
    ) -> Self {
        Self::with_palette(title, fields, prefill, palette(theme), options)
    }

    /// [`Self::new`] for a form that opens on top of another panel: the
    /// panel is filled with [`Theme::form_overlay_bg`] so it stands apart
    /// from the panel underneath.
    pub fn overlay(
        title: impl Into<String>,
        fields: Vec<FormFieldSpec>,
        prefill: &HashMap<String, String>,
        theme: &Theme,
        options: &FormOptions,
    ) -> Self {
        let palette = FormPalette {
            panel_bg: theme.form_overlay_bg(),
            ..palette(theme)
        };
        Self::with_palette(title, fields, prefill, palette, options)
    }

    fn with_palette(
        title: impl Into<String>,
        fields: Vec<FormFieldSpec>,
        prefill: &HashMap<String, String>,
        palette: FormPalette,
        options: &FormOptions,
    ) -> Self {
        let specs = fields.into_iter().map(to_field_spec).collect();
        let style = FormStyle::from_palette(&palette);
        Self {
            form: Form::new(title, specs, prefill, &style, options),
        }
    }

    /// Feed a normalized key string (see [`Form::handle_key`]).
    pub fn handle_key(&mut self, key: &str) -> ContentFormEvent {
        match self.form.handle_key(key) {
            FormEvent::Submitted(v) => ContentFormEvent::Submitted(v),
            FormEvent::Cancelled => ContentFormEvent::Cancelled,
            FormEvent::Consumed => ContentFormEvent::Consumed,
        }
    }

    /// Collected values, keyed by field key.
    pub fn values(&self) -> HashMap<String, String> {
        self.form.values()
    }

    /// Put a caller-owned line under the fields (an adapter's rejection, a
    /// progress note), or clear it with `None`.
    pub fn set_notice(&mut self, notice: Option<FormNotice>) {
        self.form.set_notice(notice);
    }

    /// Show one round of complaints: each message beside the field it names,
    /// everything unattributed joined into the notice line under the fields.
    ///
    /// The single seam for both sources of rejection — an adapter answering
    /// [`ActionOutcome::ReopenForm`](not_yet_done_content::ActionOutcome::ReopenForm)
    /// and a form script sending `errors` in its next round — so a refused
    /// value looks the same wherever it was refused.
    ///
    /// A general message shares the one footer row with the focused field's
    /// own, which outranks it (see [`Form::set_notice`]); it becomes visible
    /// as soon as the user steps off a field that was complained about.
    pub fn show_errors(&mut self, errors: &[FieldError]) {
        let mut per_field: HashMap<String, String> = HashMap::new();
        let mut general: Vec<&str> = Vec::new();
        for error in errors {
            match &error.field {
                Some(key) => match per_field.entry(key.clone()) {
                    Entry::Occupied(mut e) => {
                        // Which of two complaints about one value matters is
                        // not ours to decide, so both stay.
                        let joined = format!("{}; {}", e.get(), error.message);
                        e.insert(joined);
                    }
                    Entry::Vacant(e) => {
                        e.insert(error.message.clone());
                    }
                },
                None => general.push(&error.message),
            }
        }
        // Land on the first field that was complained about: which input was
        // meant is the whole point of a per-field rejection, and a form that
        // reopens on its first field makes the user look for it.
        let first_marked = errors.iter().find_map(|e| e.field.as_deref());
        self.form.set_field_errors(per_field);
        self.set_notice(if general.is_empty() {
            None
        } else {
            Some(FormNotice::Alert(general.join("; ")))
        });
        if let Some(key) = first_marked {
            self.focus_field(key);
        }
    }

    /// Open on the field with `key` instead of the first one. Silently does
    /// nothing when there is no such (visible) field.
    pub fn focus_field(&mut self, key: &str) {
        self.form.focus_key(key);
    }

    /// Draw the popup as a centered overlay. The focused text/date field places
    /// the terminal cursor itself via the backing widget.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        self.form.render(frame, area);
    }
}

/// Convert a protocol [`FormFieldSpec`] into the widget crate's own spec.
fn to_field_spec(f: FormFieldSpec) -> RFormSpec {
    RFormSpec {
        key: f.key,
        label: f.label,
        required: f.required,
        default: f.default,
        masked: f.masked,
        visible_when: f.visible_when.map(|c| RFieldCondition {
            field: c.field,
            equals_any: c.equals_any,
            negate: c.negate,
        }),
        kind: match f.kind {
            FormFieldKind::Text => RFormKind::Text,
            FormFieldKind::Select { allowed_values } => RFormKind::Select {
                options: allowed_values,
            },
            FormFieldKind::Toggle => RFormKind::Toggle,
            FormFieldKind::DateTime { with_time } => RFormKind::DateTime { with_time },
            FormFieldKind::MultiSelect { allowed_values } => RFormKind::MultiSelect {
                options: allowed_values,
            },
            FormFieldKind::Number { min, max, step } => RFormKind::Number { min, max, step },
            FormFieldKind::Decimal {
                min,
                max,
                precision,
            } => RFormKind::Decimal {
                min,
                max,
                precision,
            },
            FormFieldKind::Duration => RFormKind::Duration,
            FormFieldKind::Multiline { lines } => RFormKind::Multiline { lines },
            FormFieldKind::File { multi } => RFormKind::File { multi },
        },
    }
}

/// Build the crate-owned [`FormPalette`] from the app [`Theme`].
fn palette(theme: &Theme) -> FormPalette {
    FormPalette {
        accent: theme.form_accent(),
        label_idle: theme.form_label_idle(),
        text: theme.form_text(),
        text_idle: theme.form_text_idle(),
        placeholder: theme.form_placeholder(),
        selected: theme.form_selected(),
        hint: theme.form_hint(),
        error: theme.form_error(),
        field_bg: theme.form_field_bg(),
        field_bg_idle: theme.form_field_bg_idle(),
        panel_bg: theme.form_panel_bg(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::new(crate::config::ThemeConfig::default())
    }

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
    fn conversion_preserves_kind_and_flags() {
        let mut p = ContentFormPopup::new(
            "t",
            sample_fields(),
            &HashMap::new(),
            &theme(),
            &FormOptions::default(),
        );
        type_str(&mut p, "hi");
        assert_eq!(p.values().get("title").unwrap(), "hi");
    }

    #[test]
    fn prefill_flows_through_to_the_driver() {
        let mut prefill = HashMap::new();
        prefill.insert("title".to_string(), "seed".to_string());
        prefill.insert("status".to_string(), "done".to_string());
        let p = ContentFormPopup::new(
            "t",
            sample_fields(),
            &prefill,
            &theme(),
            &FormOptions::default(),
        );
        let v = p.values();
        assert_eq!(v.get("title").unwrap(), "seed");
        assert_eq!(v.get("status").unwrap(), "done");
    }

    #[test]
    fn datetime_field_keeps_raw_phrase() {
        let fields = vec![FormFieldSpec::datetime("when", "When", true)];
        let mut p = ContentFormPopup::new(
            "t",
            fields,
            &HashMap::new(),
            &theme(),
            &FormOptions::default(),
        );
        type_str(&mut p, "tomorrow 9am");
        assert_eq!(p.values().get("when").unwrap(), "tomorrow 9am");
    }

    #[test]
    fn every_protocol_kind_reaches_its_own_widget() {
        // The conversion used to fall back to a text box for the kinds whose
        // widgets did not exist yet; each of them now has one, and a duration
        // that submits whole minutes proves it is not a text box any more.
        let fields = vec![
            FormFieldSpec::duration("spent", "Spent"),
            FormFieldSpec::number("count", "Count"),
        ];
        let mut p = ContentFormPopup::new(
            "t",
            fields,
            &HashMap::new(),
            &theme(),
            &FormOptions::default(),
        );
        type_str(&mut p, "1h30");
        assert_eq!(p.values().get("spent").unwrap(), "90");
    }

    #[test]
    fn show_errors_puts_each_message_where_it_belongs() {
        use ratatui::{Terminal, backend::TestBackend};

        let mut p = ContentFormPopup::new(
            "t",
            sample_fields(),
            &HashMap::new(),
            &theme(),
            &FormOptions::default(),
        );
        p.show_errors(&[
            FieldError::field("title", "already taken"),
            FieldError::general("the board is read-only"),
        ]);
        let mut terminal = Terminal::new(TestBackend::new(70, 20)).unwrap();
        terminal.draw(|f| p.render(f, f.area())).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(
            text.contains("already taken"),
            "the complained-about field is focused, so its message is the one \
             on the footer row: {text}"
        );

        // The general message waits behind it, and shows as soon as the focus
        // leaves the field that has one of its own.
        p.handle_key("down");
        terminal.draw(|f| p.render(f, f.area())).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(
            text.contains("the board is read-only"),
            "general message missing: {text}"
        );
    }

    #[test]
    fn two_messages_about_one_field_are_both_kept() {
        let mut p = ContentFormPopup::new(
            "t",
            sample_fields(),
            &HashMap::new(),
            &theme(),
            &FormOptions::default(),
        );
        p.show_errors(&[
            FieldError::field("title", "too long"),
            FieldError::field("title", "already taken"),
        ]);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(70, 20)).unwrap();
        terminal.draw(|f| p.render(f, f.area())).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("too long; already taken"), "{text}");
    }

    #[test]
    fn submit_and_cancel_map_to_content_events() {
        let mut p = ContentFormPopup::new(
            "t",
            sample_fields(),
            &HashMap::new(),
            &theme(),
            &FormOptions::default(),
        );
        type_str(&mut p, "x");
        p.handle_key("down"); // focus the status select
        // A select has no `space` pick: its selection follows the cursor, and a
        // required select starts on its first option.
        p.handle_key("down"); // cursor (and selection) to the second option
        let ContentFormEvent::Submitted(values) = p.handle_key("enter") else {
            panic!("a form with every required field filled must submit");
        };
        assert_eq!(values.get("title").unwrap(), "x");
        assert_eq!(values.get("status").unwrap(), "in_progress");

        let mut p = ContentFormPopup::new(
            "t",
            sample_fields(),
            &HashMap::new(),
            &theme(),
            &FormOptions::default(),
        );
        assert!(matches!(p.handle_key("esc"), ContentFormEvent::Cancelled));
    }
}
