//! Query-variable popup — generic input form rendered when a saved
//! query carries `${var}` placeholders the adapter has reported via
//! `ContentAdapter::query_variables`.
//!
//! Like [`AdapterCredsPopup`](super::adapter_creds_popup::AdapterCredsPopup)
//! it is a thin shell around the shared form driver; what it adds is the
//! target the submitted values belong to. A variable is *required* iff its
//! [`QueryVariable::default`] is `None` — with a default there is always
//! something sensible to run with, so an empty field is a choice, not a gap.

use std::collections::HashMap;
use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::Rect;

use not_yet_done_content::{FormFieldSpec, QueryVariable};
use not_yet_done_ratatui::FormOptions;

use crate::components::content_form_popup::{ContentFormEvent, ContentFormPopup};
use crate::ui::theme::Theme;

pub enum QueryVarKeyOutcome {
    /// Key consumed; popup stays open.
    Consumed,
    /// User submitted the form. App should run the load.
    Submit { values: HashMap<String, String> },
    /// User cancelled (Esc). App closes the popup.
    Cancel,
}

/// Context the App needs to dispatch a submit back to the right pane.
#[derive(Clone, Debug)]
pub struct QueryVarPopupTarget {
    pub tab_idx: usize,
    pub pane_id: crate::views::content_view::PaneId,
    pub raw_query: String,
    pub saved_name: Option<String>,
    /// Which language `raw_query` is written in. It decides where the
    /// variables come from — the adapter reads a native query, an extended
    /// document declares them across its branches — and rides on to the pane
    /// so the load knows how to execute what the user just filled in.
    pub kind: not_yet_done_content::QueryKind,
}

pub struct QueryVarPopup {
    form: ContentFormPopup,
    target: QueryVarPopupTarget,
    /// Empty variable lists never reach a popup; kept to answer that in
    /// `handle_key` rather than rendering an empty panel.
    empty: bool,
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
        let empty = vars.is_empty();
        let specs: Vec<FormFieldSpec> = vars.iter().map(to_form_field).collect();
        let prefill: HashMap<String, String> = vars
            .iter()
            .filter_map(|v| {
                prefilled
                    .get(&v.name)
                    .cloned()
                    .or_else(|| v.default.clone())
                    .map(|val| (v.name.clone(), val))
            })
            .collect();

        let mut form = ContentFormPopup::new(
            title,
            specs,
            &prefill,
            &theme,
            &FormOptions {
                field_bar: true,
                submit_label: Some("Apply".to_string()),
                ..FormOptions::default()
            },
        );
        // Open on the first variable without a value — the one the query is
        // actually waiting for.
        if let Some(v) = vars.iter().find(|v| {
            prefilled
                .get(&v.name)
                .or(v.default.as_ref())
                .is_none_or(|s| s.trim().is_empty())
        }) {
            form.focus_field(&v.name);
        }

        Self {
            form,
            target,
            empty,
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
        if self.empty {
            return QueryVarKeyOutcome::Cancel;
        }
        match self.form.handle_key(key) {
            ContentFormEvent::Cancelled => QueryVarKeyOutcome::Cancel,
            ContentFormEvent::Submitted(values) => QueryVarKeyOutcome::Submit { values },
            ContentFormEvent::Consumed => QueryVarKeyOutcome::Consumed,
        }
    }

    pub fn view(&mut self, frame: &mut Frame, area: Rect) {
        if !self.open {
            return;
        }
        self.form.render(frame, area);
    }
}

/// One query variable as a form field. Without a default there is nothing to
/// fall back on, so the field is required and says so in its label.
fn to_form_field(v: &QueryVariable) -> FormFieldSpec {
    match &v.default {
        Some(_) => FormFieldSpec::text(v.name.clone(), v.name.clone()).optional(),
        None => FormFieldSpec::text(v.name.clone(), format!("{} (required)", v.name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> QueryVarPopupTarget {
        QueryVarPopupTarget {
            tab_idx: 0,
            pane_id: crate::views::content_view::PaneId::default(),
            raw_query: String::new(),
            saved_name: None,
            kind: not_yet_done_content::QueryKind::Saved,
        }
    }

    fn popup(vars: Vec<QueryVariable>, prefilled: HashMap<String, String>) -> QueryVarPopup {
        let theme = Arc::new(Theme::new(crate::config::ThemeConfig::default()));
        QueryVarPopup::new(theme, "Query".into(), target(), vars, prefilled)
    }

    fn var(name: &str, default: Option<&str>) -> QueryVariable {
        QueryVariable {
            name: name.into(),
            default: default.map(Into::into),
        }
    }

    fn type_str(p: &mut QueryVarPopup, text: &str) {
        for c in text.chars() {
            p.handle_key(&c.to_string());
        }
    }

    /// A variable with a default starts on it and may be submitted untouched.
    #[test]
    fn a_default_is_prefilled_and_submits_as_is() {
        let mut p = popup(vec![var("project", Some("nyd"))], HashMap::new());
        match p.handle_key("enter") {
            QueryVarKeyOutcome::Submit { values } => assert_eq!(values["project"], "nyd"),
            _ => panic!("a variable with a default must not block the submit"),
        }
    }

    /// Without a default the query has nothing to run with, so an empty field
    /// blocks — and the typed value comes back under the variable's name.
    #[test]
    fn a_variable_without_a_default_must_be_filled() {
        let mut p = popup(vec![var("sprint", None)], HashMap::new());
        assert!(matches!(
            p.handle_key("enter"),
            QueryVarKeyOutcome::Consumed
        ));
        type_str(&mut p, "42");
        match p.handle_key("enter") {
            QueryVarKeyOutcome::Submit { values } => assert_eq!(values["sprint"], "42"),
            _ => panic!("a filled required variable must submit"),
        }
    }

    /// A previously entered value wins over the static default.
    #[test]
    fn a_prefilled_value_beats_the_default() {
        let mut prefilled = HashMap::new();
        prefilled.insert("project".to_string(), "other".to_string());
        let mut p = popup(vec![var("project", Some("nyd"))], prefilled);
        match p.handle_key("enter") {
            QueryVarKeyOutcome::Submit { values } => assert_eq!(values["project"], "other"),
            _ => panic!("prefilled values must submit"),
        }
    }
}
