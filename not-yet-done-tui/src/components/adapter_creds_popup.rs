//! Adapter credentials popup — generic login form for any adapter that
//! reports `AdapterStatus::NeedsCreds`.
//!
//! The fields, their navigation and their rendering are the shared form
//! driver's ([`ContentFormPopup`]), so a login looks and behaves like every
//! other form in the app; a masked field is a plain text field the driver
//! bullets out. What stays here is what a login has and a content form does
//! not: the view it belongs to, the in-flight `submitting` state during the
//! adapter's round-trip, and the identity check that decides whether a second
//! `NeedsCreds` is the same question again or a new one.

use std::collections::HashMap;
use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::Rect;

use not_yet_done_content::{AuthField, FormFieldSpec};
use not_yet_done_ratatui::{FormNotice, FormOptions};

use crate::components::content_form_popup::{ContentFormEvent, ContentFormPopup};
use crate::ui::theme::Theme;

pub enum CredsKeyOutcome {
    /// Key consumed; nothing else for the App to do.
    Consumed,
    /// User submitted the form. App should spawn `submit_credentials`
    /// and meanwhile leave the popup open in `submitting` state.
    Submit { values: HashMap<String, String> },
    /// User cancelled (Esc). App closes the popup.
    Cancel,
}

pub struct AdapterCredsPopup {
    form: ContentFormPopup,
    title: String,
    /// View index this popup is bound to, so the App can route the
    /// submitted values to the right adapter.
    view_index: usize,
    /// Kept verbatim for [`shows`](Self::shows) — the driver owns the widgets,
    /// not the question they were built from.
    fields: Vec<AuthField>,
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
        let specs: Vec<FormFieldSpec> = fields.iter().map(to_form_field).collect();
        let prefill: HashMap<String, String> = fields
            .iter()
            .filter_map(|f| f.prefill.clone().map(|v| (f.name.clone(), v)))
            .collect();

        let mut form = ContentFormPopup::new(
            title.clone(),
            specs,
            &prefill,
            &theme,
            &FormOptions {
                // The centred, content-sized panel: a login is two fields, and
                // the classic chrome would blow it up to the full overlay area.
                field_bar: true,
                submit_label: Some("Log in".to_string()),
                ..FormOptions::default()
            },
        );
        // Open on the first field the user still has to fill — a prefilled
        // username in front of an empty password must not cost a Tab.
        if let Some(f) = fields.iter().find(|f| f.prefill.is_none()) {
            form.focus_field(&f.name);
        }

        Self {
            form,
            title,
            view_index,
            fields,
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
        self.form.set_notice(Some(FormNotice::Alert(msg.clone())));
        self.error = Some(msg);
        self.submitting = false;
    }

    /// Whether this popup already shows exactly that form.
    ///
    /// A credential script asks in rounds, so a second `NeedsCreds` may
    /// arrive while a popup is open. Only an identical form may be
    /// swallowed as a repeat — a different question, or the same one with
    /// "that passphrase was rejected" attached, has to replace it.
    pub fn shows(&self, title: &str, fields: &[AuthField], error: Option<&str>) -> bool {
        self.title == title && self.fields == fields && self.error.as_deref() == error
    }

    pub fn handle_key(&mut self, key: &str) -> CredsKeyOutcome {
        if self.submitting {
            // Block input while a login is in flight, except cancel.
            if matches!(key, "esc") {
                return CredsKeyOutcome::Cancel;
            }
            return CredsKeyOutcome::Consumed;
        }
        match self.form.handle_key(key) {
            ContentFormEvent::Cancelled => CredsKeyOutcome::Cancel,
            ContentFormEvent::Submitted(values) => {
                self.submitting = true;
                self.error = None;
                self.form
                    .set_notice(Some(FormNotice::Info("Submitting…".to_string())));
                CredsKeyOutcome::Submit { values }
            }
            ContentFormEvent::Consumed => CredsKeyOutcome::Consumed,
        }
    }

    pub fn view(&mut self, frame: &mut Frame, area: Rect) {
        if !self.open {
            return;
        }
        self.form.render(frame, area);
    }
}

/// One credential field as a form field. A field the config binds is never
/// optional; a script's own form may declare inputs it can do without.
fn to_form_field(f: &AuthField) -> FormFieldSpec {
    let mut spec = FormFieldSpec::text(f.name.clone(), f.label.clone());
    if f.optional {
        spec = spec.optional();
    }
    if f.masked {
        spec = spec.masked();
    }
    spec
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str, optional: bool) -> AuthField {
        AuthField {
            name: name.into(),
            label: format!("{name} label"),
            masked: false,
            optional,
            prefill: None,
        }
    }

    fn popup(fields: Vec<AuthField>) -> AdapterCredsPopup {
        let theme = Arc::new(Theme::new(crate::config::ThemeConfig::default()));
        AdapterCredsPopup::new(theme, "Login".into(), 0, fields)
    }

    fn type_str(p: &mut AdapterCredsPopup, text: &str) {
        for c in text.chars() {
            p.handle_key(&c.to_string());
        }
    }

    /// The credential form is also how a credential script's own form is
    /// rendered, and a script may declare an input it can do without.
    #[test]
    fn an_optional_field_may_stay_empty() {
        let mut p = popup(vec![field("password", false), field("otp", true)]);
        type_str(&mut p, "secret");
        match p.handle_key("enter") {
            CredsKeyOutcome::Submit { values } => {
                assert_eq!(values["password"], "secret");
                assert_eq!(values["otp"], "");
            }
            _ => panic!("an empty optional field must not block the submit"),
        }
    }

    #[test]
    fn a_missing_required_field_blocks_the_submit() {
        let mut p = popup(vec![field("password", false), field("otp", true)]);
        assert!(
            matches!(p.handle_key("enter"), CredsKeyOutcome::Consumed),
            "an empty required field must not submit"
        );
    }

    /// While a login is in flight the form is inert — except for Esc, which
    /// has to reach the App: the adapter is waiting on the answer and holds
    /// the auth lock until it is told the form is gone.
    #[test]
    fn a_login_in_flight_swallows_everything_but_escape() {
        let mut p = popup(vec![field("password", false)]);
        type_str(&mut p, "secret");
        assert!(matches!(
            p.handle_key("enter"),
            CredsKeyOutcome::Submit { .. }
        ));
        assert!(matches!(p.handle_key("x"), CredsKeyOutcome::Consumed));
        assert!(matches!(p.handle_key("esc"), CredsKeyOutcome::Cancel));
    }

    /// A repeat of the very same question may be swallowed; the same question
    /// carrying a rejection is a new one and must replace the popup.
    #[test]
    fn an_error_makes_the_same_question_a_different_one() {
        let fields = vec![field("passphrase", false)];
        let mut p = popup(fields.clone());
        assert!(p.shows("Login", &fields, None));

        p.set_error("wrong passphrase".into());
        assert!(!p.shows("Login", &fields, None));
        assert!(p.shows("Login", &fields, Some("wrong passphrase")));
    }
}
