//! The form as it stands between two rounds, and how the next answer folds
//! into it.

use std::collections::BTreeMap;

use serde_json::Value;

use super::{RoundForm, ScriptFormError};
use crate::FormFieldSpec;

/// Everything a front-end needs to show the form, and everything the next
/// request needs to be built from.
///
/// It is deliberately a plain value with no behaviour beyond [`advance`] and
/// [`clear_field_error`](Self::clear_field_error): the TUI, the CLI and the
/// tests all fold rounds the same way, and a form that behaves differently
/// depending on who is showing it would be a bug nobody could reproduce.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormState {
    /// Dialog title, carried over from whichever round last sent one.
    pub header: Option<String>,
    /// The fields on screen, in order.
    pub fields: Vec<FormFieldSpec>,
    /// The current value of each field that has one, keyed by
    /// [`FormFieldSpec::key`].
    pub values: BTreeMap<String, String>,
    /// Messages tied to a field. More than one per field is kept rather than
    /// merged — which of two complaints matters is the front-end's call.
    pub field_errors: BTreeMap<String, Vec<String>>,
    /// Messages about the submission as a whole.
    pub general_errors: Vec<String>,
    /// The script's own memory, handed back to it next round.
    pub script_state: Value,
}

impl FormState {
    /// The messages on one field, joined for a single-line notice.
    pub fn field_error_text(&self, key: &str) -> Option<String> {
        self.field_errors.get(key).map(|m| m.join("; "))
    }

    /// Drop the messages on one field.
    ///
    /// A field error describes a *value*, so the moment that value is edited
    /// it describes nothing. Leaving it up would have the form contradict
    /// what is on screen until the next round comes back.
    pub fn clear_field_error(&mut self, key: &str) {
        self.field_errors.remove(key);
    }
}

/// Fold one `form` answer into the next state of the form.
///
/// - `prev` is `None` in the first round, and the script must then send
///   `fields` — there is no form to keep.
/// - `submitted` is what the front-end collected from the round just gone.
/// - The returned warnings are for the log: things worth telling a script
///   author about that are not worth aborting over.
///
/// Values are `submitted`, then a field's `default` for anything the user was
/// never asked, then the script's `prefill` on top — the script wins when it
/// says so. What deliberately does **not** happen is a fallback to the
/// previous round's values for a key `submitted` leaves out: the front-end
/// omits exactly the fields a `visible_when` hid, and resurrecting the value
/// it dropped would submit something the user cannot see.
pub fn advance(
    script: &str,
    prev: Option<&FormState>,
    submitted: &BTreeMap<String, String>,
    form: RoundForm,
) -> Result<(FormState, Vec<String>), ScriptFormError> {
    let mut warnings = Vec::new();

    let fields = match form.fields {
        Some(fields) => fields,
        None => match prev {
            Some(prev) => prev.fields.clone(),
            None => {
                return Err(ScriptFormError::new(
                    script,
                    "asked for a form without fields in the first round, so \
                     there is no form to keep",
                ));
            }
        },
    };
    let header = form.header.or_else(|| prev.and_then(|p| p.header.clone()));

    let mut values = BTreeMap::new();
    for field in &fields {
        if let Some(value) = submitted
            .get(&field.key)
            .cloned()
            .or_else(|| field.default.clone())
        {
            values.insert(field.key.clone(), value);
        }
    }
    for (key, value) in form.prefills {
        if !fields.iter().any(|f| f.key == key) {
            warnings.push(format!("prefill for unknown field `{key}` was dropped"));
            continue;
        }
        values.insert(key, value);
    }

    let mut field_errors: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut general_errors = Vec::new();
    for error in form.errors {
        match error.field {
            Some(key) if fields.iter().any(|f| f.key == key) => {
                field_errors.entry(key).or_default().push(error.message);
            }
            Some(key) => {
                // A typo in a script would otherwise hide the complaint on a
                // field that is not there: the user sees a form that looks
                // fine and a submit that keeps failing.
                warnings.push(format!(
                    "error for unknown field `{key}` was shown as a general message"
                ));
                general_errors.push(error.message);
            }
            None => general_errors.push(error.message),
        }
    }

    Ok((
        FormState {
            header,
            fields,
            values,
            field_errors,
            general_errors,
            script_state: form.state,
        },
        warnings,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FieldError, FormFieldSpec};

    fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn round(fields: Option<Vec<FormFieldSpec>>) -> RoundForm {
        RoundForm {
            fields,
            ..RoundForm::default()
        }
    }

    fn first(fields: Vec<FormFieldSpec>) -> FormState {
        advance("f.form.py", None, &values(&[]), round(Some(fields)))
            .expect("the first round sends fields")
            .0
    }

    #[test]
    fn the_first_round_seeds_the_values_from_the_defaults() {
        let state = first(vec![
            FormFieldSpec::text("title", "Title").with_default("Rework"),
            FormFieldSpec::text("note", "Note").optional(),
        ]);
        assert_eq!(state.values, values(&[("title", "Rework")]));
        assert!(
            !state.values.contains_key("note"),
            "a field with no default has no value yet"
        );
    }

    #[test]
    fn a_first_round_without_fields_is_refused() {
        let err = advance("f.form.py", None, &values(&[]), round(None))
            .expect_err("there is no form to keep");
        assert!(err.message.contains("first round"), "{err}");
    }

    #[test]
    fn a_later_round_without_fields_keeps_the_form_and_the_values() {
        let prev = first(vec![FormFieldSpec::text("title", "Title")]);
        let (state, warnings) = advance(
            "f.form.py",
            Some(&prev),
            &values(&[("title", "Rework")]),
            RoundForm {
                errors: vec![FieldError::field("title", "too short")],
                ..RoundForm::default()
            },
        )
        .expect("a repeat round");
        assert_eq!(state.fields, prev.fields, "the form on screen stays");
        assert_eq!(state.values, values(&[("title", "Rework")]));
        assert_eq!(
            state.field_error_text("title").as_deref(),
            Some("too short")
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn a_new_field_list_keeps_what_survives_and_starts_the_rest_at_its_default() {
        let prev = first(vec![FormFieldSpec::text("title", "Title")]);
        let (state, _) = advance(
            "f.form.py",
            Some(&prev),
            &values(&[("title", "Rework")]),
            round(Some(vec![
                FormFieldSpec::text("title", "Title"),
                FormFieldSpec::select("sprint", "Sprint", vec!["S-41".into()]).with_default("S-41"),
            ])),
        )
        .expect("a second round with a wider form");
        assert_eq!(
            state.values,
            values(&[("title", "Rework"), ("sprint", "S-41")]),
            "what the user typed survives, the new field starts at its default"
        );
    }

    #[test]
    fn a_field_that_disappears_takes_its_value_with_it() {
        let prev = first(vec![
            FormFieldSpec::text("title", "Title"),
            FormFieldSpec::text("why", "Why"),
        ]);
        let (state, _) = advance(
            "f.form.py",
            Some(&prev),
            &values(&[("title", "Rework"), ("why", "because")]),
            round(Some(vec![FormFieldSpec::text("title", "Title")])),
        )
        .expect("a narrower form");
        assert_eq!(state.values, values(&[("title", "Rework")]));
    }

    #[test]
    fn a_hidden_field_does_not_come_back_from_the_round_before() {
        // The front-end drops the values of fields a `visible_when` hid, so
        // they are simply absent from `submitted`. Falling back to the last
        // round would submit a value the user can no longer see.
        let prev = first(vec![
            FormFieldSpec::toggle("billable", "Billable"),
            FormFieldSpec::text("rate", "Rate")
                .with_default("0")
                .visible_when("billable", "true"),
        ]);
        let (state, _) = advance(
            "f.form.py",
            Some(&prev),
            &values(&[("billable", "false")]),
            round(None),
        )
        .expect("a repeat round");
        assert_eq!(
            state.values.get("rate").map(String::as_str),
            Some("0"),
            "back to its default, not to whatever was typed while it showed"
        );
    }

    #[test]
    fn a_prefill_outranks_both_the_default_and_what_was_typed() {
        let prev = first(vec![
            FormFieldSpec::text("title", "Title").with_default("d"),
        ]);
        let (state, _) = advance(
            "f.form.py",
            Some(&prev),
            &values(&[("title", "typed")]),
            RoundForm {
                prefills: vec![("title".into(), "the script insists".into())],
                ..RoundForm::default()
            },
        )
        .expect("a repeat round");
        assert_eq!(state.values["title"], "the script insists");
    }

    #[test]
    fn an_error_naming_an_unknown_field_becomes_general_and_is_reported() {
        let prev = first(vec![FormFieldSpec::text("title", "Title")]);
        let (state, warnings) = advance(
            "f.form.py",
            Some(&prev),
            &values(&[]),
            RoundForm {
                errors: vec![
                    FieldError::field("titel", "typo in the script"),
                    FieldError::general("the tracker said no"),
                ],
                ..RoundForm::default()
            },
        )
        .expect("a repeat round");
        assert!(state.field_errors.is_empty());
        assert_eq!(
            state.general_errors,
            vec!["typo in the script", "the tracker said no"]
        );
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("`titel`"), "{:?}", warnings);
    }

    #[test]
    fn a_prefill_for_an_unknown_field_is_dropped_and_reported() {
        let prev = first(vec![FormFieldSpec::text("title", "Title")]);
        let (state, warnings) = advance(
            "f.form.py",
            Some(&prev),
            &values(&[]),
            RoundForm {
                prefills: vec![("titel".into(), "x".into())],
                ..RoundForm::default()
            },
        )
        .expect("a repeat round");
        assert!(!state.values.contains_key("titel"));
        assert!(warnings[0].contains("`titel`"), "{:?}", warnings);
    }

    #[test]
    fn two_complaints_about_one_field_are_both_kept() {
        let prev = first(vec![FormFieldSpec::text("title", "Title")]);
        let (state, _) = advance(
            "f.form.py",
            Some(&prev),
            &values(&[]),
            RoundForm {
                errors: vec![
                    FieldError::field("title", "too short"),
                    FieldError::field("title", "and it is taken"),
                ],
                ..RoundForm::default()
            },
        )
        .expect("a repeat round");
        assert_eq!(
            state.field_error_text("title").as_deref(),
            Some("too short; and it is taken")
        );
    }

    #[test]
    fn editing_a_field_drops_the_message_about_its_old_value() {
        let mut state = first(vec![FormFieldSpec::text("title", "Title")]);
        state
            .field_errors
            .insert("title".into(), vec!["too short".into()]);
        state.clear_field_error("title");
        assert!(state.field_error_text("title").is_none());
    }

    #[test]
    fn the_header_survives_a_round_that_does_not_send_one() {
        let (prev, _) = advance(
            "f.form.py",
            None,
            &values(&[]),
            RoundForm {
                header: Some("New work item".into()),
                fields: Some(vec![FormFieldSpec::text("title", "Title")]),
                ..RoundForm::default()
            },
        )
        .expect("the first round");
        let (state, _) =
            advance("f.form.py", Some(&prev), &values(&[]), round(None)).expect("a repeat round");
        assert_eq!(state.header.as_deref(), Some("New work item"));
    }
}
