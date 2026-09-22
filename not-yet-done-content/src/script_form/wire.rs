//! What a form script writes, and what it has to mean.
//!
//! The answer is JSON with exactly one of `form`, `result` and `error`, plus
//! an optional `state` the runtime hands back untouched next round. Unknown
//! keys are rejected everywhere: a typo in a hand-written script is a bug to
//! report, not a value to ignore.

use serde::Deserialize;
use serde_json::Value;

use super::ScriptFormError;
use crate::{FieldCondition, FieldError, FormFieldKind, FormFieldSpec};

/// One round's answer, after validation.
#[derive(Clone, Debug, PartialEq)]
pub enum ScriptRound {
    /// Put this on screen and run another round with what comes back.
    Form(RoundForm),
    /// The conversation is over. The payload is the ordinary script answer —
    /// `commands`, captured text, whatever the scope produces — passed on
    /// unread. Success is expressed by *which key was used*, not by a flag
    /// that could contradict the payload.
    Result(Value),
    /// The script gave up; the message is shown to the user.
    Failed(String),
}

/// The form half of a round.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RoundForm {
    /// Dialog title. `None` keeps whatever the form already showed.
    pub header: Option<String>,
    /// The fields to show. `None` means *keep the form that is on screen* and
    /// change only the errors — the common case after a failed check. `Some`
    /// replaces it, which is how a script reveals a field that only makes
    /// sense once an earlier answer is in.
    pub fields: Option<Vec<FormFieldSpec>>,
    /// Values the script insists on, keyed by field name. Unlike a field's
    /// `default` these win over what the user typed: the script only sends
    /// one when it means to overrule.
    pub prefills: Vec<(String, String)>,
    /// What was wrong with the last submission. An entry with a field name
    /// hangs on that field, one without is the general message.
    pub errors: Vec<FieldError>,
    /// Opaque script memory, echoed into the next request. It rides with the
    /// form because `form` is the only answer that has a next round.
    pub state: Value,
}

/// The three answer shapes as they arrive. Kept apart from [`ScriptRound`] so
/// that "exactly one of them" is checked here instead of being expressible in
/// the type everyone else sees.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawRound {
    #[serde(default)]
    form: Option<RawForm>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    state: Value,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawForm {
    #[serde(default)]
    header: Option<String>,
    #[serde(default)]
    fields: Option<Vec<RawField>>,
    #[serde(default)]
    errors: Vec<RawError>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawField {
    name: String,
    #[serde(default)]
    label: Option<String>,
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    required: Option<bool>,
    #[serde(default)]
    default: Option<String>,
    #[serde(default)]
    prefill: Option<String>,
    #[serde(default)]
    options: Option<Vec<String>>,
    #[serde(default)]
    masked: bool,
    #[serde(default)]
    min: Option<f64>,
    #[serde(default)]
    max: Option<f64>,
    #[serde(default)]
    step: Option<i64>,
    #[serde(default)]
    precision: Option<u8>,
    #[serde(default)]
    lines: Option<u16>,
    #[serde(default)]
    multi: bool,
    #[serde(default)]
    visible_when: Option<RawCondition>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCondition {
    field: String,
    equals: OneOrMany,
    #[serde(default)]
    negate: bool,
}

/// `equals: "a"` and `equals: ["a", "b"]` both read naturally; making the
/// script pick a list for the single case would be pedantry.
#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawError {
    #[serde(default)]
    name: Option<String>,
    error: String,
}

/// Read one round's answer.
///
/// `script` only names the culprit in the messages.
pub fn parse_round(script: &str, output: &str) -> Result<ScriptRound, ScriptFormError> {
    let bad = |m: String| ScriptFormError::new(script, m);

    if output.trim().is_empty() {
        return Err(bad("wrote nothing".into()));
    }
    let raw: RawRound = serde_json::from_str(output)
        .map_err(|e| bad(format!("this is not a round answer ({e})")))?;

    let set = raw.form.is_some() as u8 + raw.result.is_some() as u8 + raw.error.is_some() as u8;
    if set != 1 {
        return Err(bad(format!(
            "expected exactly one of `form`, `result` or `error`, got {set}"
        )));
    }

    if let Some(error) = raw.error {
        let error = error.trim().to_string();
        if error.is_empty() {
            return Err(bad("reported an empty error".into()));
        }
        return Ok(ScriptRound::Failed(error));
    }
    if let Some(result) = raw.result {
        return Ok(ScriptRound::Result(result));
    }

    let form = raw.form.unwrap_or_default();
    let mut fields = None;
    let mut prefills = Vec::new();
    if let Some(raw_fields) = form.fields {
        if raw_fields.is_empty() {
            // Nothing to fill in means nothing to submit thoughtfully: the
            // form pops up and is gone again, and a script answering `form`
            // every round would loop with no human brake. `result` and
            // `error` are the ways to end a round without asking.
            return Err(bad("asked for a form without fields".into()));
        }
        let mut specs = Vec::with_capacity(raw_fields.len());
        for raw_field in raw_fields {
            let (spec, prefill) = build_field(script, raw_field)?;
            if specs.iter().any(|s: &FormFieldSpec| s.key == spec.key) {
                return Err(bad(format!("duplicate form field `{}`", spec.key)));
            }
            if let Some(prefill) = prefill {
                prefills.push((spec.key.clone(), prefill));
            }
            specs.push(spec);
        }
        fields = Some(specs);
    }

    Ok(ScriptRound::Form(RoundForm {
        header: form.header,
        fields,
        prefills,
        errors: form
            .errors
            .into_iter()
            .map(|e| FieldError {
                field: e.name,
                message: e.error,
            })
            .collect(),
        state: raw.state,
    }))
}

/// Turn one declared field into a spec, or say why it cannot be one.
fn build_field(
    script: &str,
    raw: RawField,
) -> Result<(FormFieldSpec, Option<String>), ScriptFormError> {
    let name = raw.name.trim().to_string();
    let bad = |m: String| ScriptFormError::new(script, m);
    if name.is_empty() {
        return Err(bad("a form field has an empty name".into()));
    }
    let field_error = |m: String| ScriptFormError::new(script, format!("field `{name}`: {m}"));
    let ty = raw
        .kind
        .as_deref()
        .unwrap_or("text")
        .trim()
        .to_ascii_lowercase();

    // Which extra parameters the type understands. Everything else set on it
    // is a misunderstanding worth reporting — a `step` on a select does
    // nothing, and a script author who wrote one expects it to.
    let allowed: &[&str] = match ty.as_str() {
        "text" => &["masked"],
        "select" | "multiselect" => &["options"],
        "toggle" | "date" | "datetime" | "duration" => &[],
        "number" => &["min", "max", "step"],
        "decimal" => &["min", "max", "precision"],
        "multiline" => &["masked", "lines"],
        "file" => &["multi"],
        other => {
            return Err(field_error(format!(
                "unknown type `{other}` (text, select, multiselect, toggle, \
                 date, datetime, number, decimal, duration, multiline, file)"
            )));
        }
    };
    for (param, is_set) in [
        ("options", raw.options.is_some()),
        ("masked", raw.masked),
        ("min", raw.min.is_some()),
        ("max", raw.max.is_some()),
        ("step", raw.step.is_some()),
        ("precision", raw.precision.is_some()),
        ("lines", raw.lines.is_some()),
        ("multi", raw.multi),
    ] {
        if is_set && !allowed.contains(&param) {
            return Err(field_error(format!(
                "`{param}` means nothing for type `{ty}`"
            )));
        }
    }
    if let (Some(min), Some(max)) = (raw.min, raw.max)
        && min > max
    {
        return Err(field_error(format!("min {min} is above max {max}")));
    }

    let options = || -> Result<Vec<String>, ScriptFormError> {
        let options = raw.options.clone().unwrap_or_default();
        if options.is_empty() {
            return Err(field_error(format!(
                "type `{ty}` needs a non-empty `options` list"
            )));
        }
        Ok(options)
    };
    let whole = |v: Option<f64>, which: &str| -> Result<Option<i64>, ScriptFormError> {
        match v {
            None => Ok(None),
            Some(v) if v.fract() == 0.0 && v >= i64::MIN as f64 && v <= i64::MAX as f64 => {
                Ok(Some(v as i64))
            }
            Some(v) => Err(field_error(format!(
                "`{which}` of a whole-number field is {v}"
            ))),
        }
    };

    let kind = match ty.as_str() {
        "text" => FormFieldKind::Text,
        "select" => FormFieldKind::Select {
            allowed_values: options()?,
        },
        "multiselect" => FormFieldKind::MultiSelect {
            allowed_values: options()?,
        },
        "toggle" => FormFieldKind::Toggle,
        "date" => FormFieldKind::DateTime { with_time: false },
        "datetime" => FormFieldKind::DateTime { with_time: true },
        "number" => FormFieldKind::Number {
            min: whole(raw.min, "min")?,
            max: whole(raw.max, "max")?,
            step: raw.step,
        },
        "decimal" => FormFieldKind::Decimal {
            min: raw.min,
            max: raw.max,
            precision: raw.precision,
        },
        "duration" => FormFieldKind::Duration,
        "multiline" => FormFieldKind::Multiline { lines: raw.lines },
        "file" => FormFieldKind::File { multi: raw.multi },
        _ => unreachable!("the type was checked above"),
    };

    // A default the widget cannot show is a script bug that would otherwise
    // surface as an inexplicably empty select.
    if let Some(default) = &raw.default {
        let allowed_values = match &kind {
            FormFieldKind::Select { allowed_values }
            | FormFieldKind::MultiSelect { allowed_values } => Some(allowed_values),
            _ => None,
        };
        if let Some(allowed_values) = allowed_values {
            for chosen in default.lines().filter(|l| !l.is_empty()) {
                if !allowed_values.iter().any(|o| o == chosen) {
                    return Err(field_error(format!(
                        "default `{chosen}` is not one of the options"
                    )));
                }
            }
        }
    }

    let spec = FormFieldSpec {
        label: raw.label.unwrap_or_else(|| crate::auth::title_case(&name)),
        // A toggle is answered by existing; everything else is wanted unless
        // the script says otherwise.
        required: raw
            .required
            .unwrap_or(!matches!(kind, FormFieldKind::Toggle)),
        key: name,
        kind,
        default: raw.default,
        masked: raw.masked,
        visible_when: raw.visible_when.map(|c| FieldCondition {
            field: c.field,
            equals_any: match c.equals {
                OneOrMany::One(v) => vec![v],
                OneOrMany::Many(v) => v,
            },
            negate: c.negate,
        }),
    };
    Ok((spec, raw.prefill))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(output: serde_json::Value) -> Result<ScriptRound, ScriptFormError> {
        parse_round("f.form.py", &output.to_string())
    }

    fn form(output: serde_json::Value) -> RoundForm {
        match parse(output).expect("a valid form round") {
            ScriptRound::Form(f) => f,
            other => panic!("expected a form, got {other:?}"),
        }
    }

    fn refuse(output: serde_json::Value) -> String {
        parse(output).expect_err("should be refused").message
    }

    #[test]
    fn a_result_ends_the_conversation_with_its_payload_untouched() {
        let round = parse(json!({"result": {"commands": [{"open": "x"}]}})).expect("valid");
        assert_eq!(
            round,
            ScriptRound::Result(json!({"commands": [{"open": "x"}]}))
        );
    }

    #[test]
    fn an_error_answer_carries_the_message() {
        assert_eq!(
            parse(json!({"error": "Jira is not reachable"})).expect("valid"),
            ScriptRound::Failed("Jira is not reachable".into())
        );
    }

    #[test]
    fn exactly_one_of_the_three_keys_is_required() {
        assert!(refuse(json!({})).contains("got 0"));
        assert!(refuse(json!({"result": {}, "error": "x"})).contains("got 2"));
    }

    #[test]
    fn an_unknown_key_is_refused_rather_than_ignored() {
        let msg = refuse(json!({"reslut": {}}));
        assert!(msg.contains("not a round answer"), "{msg}");
    }

    #[test]
    fn an_empty_answer_is_refused() {
        let msg = parse_round("f.form.py", "  \n")
            .expect_err("nothing to read")
            .message;
        assert_eq!(msg, "wrote nothing");
    }

    #[test]
    fn an_empty_error_message_is_refused() {
        assert!(refuse(json!({"error": "   "})).contains("empty error"));
    }

    #[test]
    fn fields_become_specs_with_labels_and_required_defaults() {
        let f = form(json!({"form": {
            "header": "New work item",
            "fields": [
                {"name": "title_text"},
                {"name": "billable", "type": "toggle"},
                {"name": "sprint", "type": "select", "options": ["S-41", "S-42"], "required": false}
            ]
        }}));
        let fields = f.fields.expect("fields were sent");
        assert_eq!(f.header.as_deref(), Some("New work item"));
        assert_eq!(fields[0].label, "Title Text", "the name is title-cased");
        assert!(fields[0].required, "a plain field is wanted by default");
        assert!(!fields[1].required, "a toggle is answered by existing");
        assert!(!fields[2].required, "the script said otherwise");
        assert_eq!(
            fields[2].kind,
            FormFieldKind::Select {
                allowed_values: vec!["S-41".into(), "S-42".into()]
            }
        );
    }

    #[test]
    fn every_field_type_maps_to_its_kind() {
        let f = form(json!({"form": {"fields": [
            {"name": "a", "type": "text", "masked": true},
            {"name": "b", "type": "date"},
            {"name": "c", "type": "datetime"},
            {"name": "d", "type": "number", "min": 1, "max": 10, "step": 2},
            {"name": "e", "type": "decimal", "min": 0.5, "precision": 2},
            {"name": "f", "type": "duration"},
            {"name": "g", "type": "multiline", "lines": 4},
            {"name": "h", "type": "multiselect", "options": ["x", "y"]},
            {"name": "i", "type": "file", "multi": true}
        ]}}));
        let fields = f.fields.expect("fields were sent");
        let kinds: Vec<&FormFieldKind> = fields.iter().map(|f| &f.kind).collect();
        assert_eq!(kinds[0], &FormFieldKind::Text);
        assert!(fields[0].masked);
        assert_eq!(kinds[1], &FormFieldKind::DateTime { with_time: false });
        assert_eq!(kinds[2], &FormFieldKind::DateTime { with_time: true });
        assert_eq!(
            kinds[3],
            &FormFieldKind::Number {
                min: Some(1),
                max: Some(10),
                step: Some(2)
            }
        );
        assert_eq!(
            kinds[4],
            &FormFieldKind::Decimal {
                min: Some(0.5),
                max: None,
                precision: Some(2)
            }
        );
        assert_eq!(kinds[5], &FormFieldKind::Duration);
        assert_eq!(kinds[6], &FormFieldKind::Multiline { lines: Some(4) });
        assert_eq!(
            kinds[7],
            &FormFieldKind::MultiSelect {
                allowed_values: vec!["x".into(), "y".into()]
            }
        );
        assert_eq!(kinds[8], &FormFieldKind::File { multi: true });
    }

    #[test]
    fn an_unknown_type_is_loud_rather_than_silently_text() {
        let msg = refuse(json!({"form": {"fields": [{"name": "a", "type": "slider"}]}}));
        assert!(msg.contains("unknown type `slider`"), "{msg}");
    }

    #[test]
    fn a_parameter_the_type_cannot_use_is_refused() {
        let msg = refuse(json!({"form": {"fields": [
            {"name": "a", "type": "select", "options": ["x"], "step": 2}
        ]}}));
        assert!(
            msg.contains("`step` means nothing for type `select`"),
            "{msg}"
        );
        let msg =
            refuse(json!({"form": {"fields": [{"name": "a", "masked": true, "type": "toggle"}]}}));
        assert!(msg.contains("`masked` means nothing"), "{msg}");
    }

    #[test]
    fn a_whole_number_field_refuses_fractional_bounds() {
        let msg =
            refuse(json!({"form": {"fields": [{"name": "a", "type": "number", "max": 2.5}]}}));
        assert!(
            msg.contains("`max` of a whole-number field is 2.5"),
            "{msg}"
        );
    }

    #[test]
    fn bounds_the_wrong_way_round_are_refused() {
        let msg = refuse(json!({"form": {"fields": [
            {"name": "a", "type": "number", "min": 10, "max": 1}
        ]}}));
        assert!(msg.contains("min 10 is above max 1"), "{msg}");
    }

    #[test]
    fn a_choice_field_needs_options_and_a_default_among_them() {
        let msg =
            refuse(json!({"form": {"fields": [{"name": "a", "type": "select", "options": []}]}}));
        assert!(msg.contains("non-empty `options`"), "{msg}");
        let msg = refuse(json!({"form": {"fields": [
            {"name": "a", "type": "select", "options": ["x"], "default": "y"}
        ]}}));
        assert!(
            msg.contains("default `y` is not one of the options"),
            "{msg}"
        );
    }

    #[test]
    fn a_multiselect_default_is_checked_line_by_line() {
        let f = form(json!({"form": {"fields": [
            {"name": "a", "type": "multiselect", "options": ["x", "y"], "default": "x\ny"}
        ]}}));
        assert_eq!(
            f.fields.expect("fields")[0].default.as_deref(),
            Some("x\ny")
        );
        let msg = refuse(json!({"form": {"fields": [
            {"name": "a", "type": "multiselect", "options": ["x"], "default": "x\nz"}
        ]}}));
        assert!(msg.contains("default `z`"), "{msg}");
    }

    #[test]
    fn a_form_without_fields_is_a_form_that_stays() {
        let f = form(json!({"form": {"errors": [{"name": "a", "error": "too late"}]}}));
        assert!(f.fields.is_none(), "no fields means keep the one on screen");
        assert_eq!(
            f.errors,
            vec![FieldError::field("a", "too late")],
            "the errors are the point of such a round"
        );
    }

    #[test]
    fn an_empty_field_list_is_refused_because_nothing_brakes_it() {
        assert!(refuse(json!({"form": {"fields": []}})).contains("without fields"));
    }

    #[test]
    fn nameless_and_duplicate_fields_are_refused() {
        assert!(refuse(json!({"form": {"fields": [{"name": "  "}]}})).contains("empty name"));
        let msg = refuse(json!({"form": {"fields": [{"name": "a"}, {"name": "a"}]}}));
        assert!(msg.contains("duplicate form field `a`"), "{msg}");
    }

    #[test]
    fn an_error_without_a_name_is_the_general_one() {
        let f = form(json!({"form": {"errors": [
            {"error": "the tracker rejected the whole request"},
            {"name": "birthday", "error": "must not be in the future"}
        ]}}));
        assert_eq!(
            f.errors,
            vec![
                FieldError::general("the tracker rejected the whole request"),
                FieldError::field("birthday", "must not be in the future"),
            ]
        );
    }

    #[test]
    fn a_prefill_travels_beside_the_spec_because_it_outranks_the_default() {
        let f = form(json!({"form": {"fields": [
            {"name": "a", "default": "d", "prefill": "p"},
            {"name": "b", "default": "d"}
        ]}}));
        assert_eq!(f.prefills, vec![("a".to_string(), "p".to_string())]);
        assert_eq!(f.fields.expect("fields")[0].default.as_deref(), Some("d"));
    }

    #[test]
    fn visible_when_accepts_one_value_or_a_list() {
        let f = form(json!({"form": {"fields": [
            {"name": "a", "type": "toggle"},
            {"name": "b", "visible_when": {"field": "a", "equals": "true"}},
            {"name": "c", "visible_when": {"field": "a", "equals": ["x", "y"], "negate": true}}
        ]}}));
        let fields = f.fields.expect("fields");
        assert_eq!(
            fields[1].visible_when,
            Some(FieldCondition {
                field: "a".into(),
                equals_any: vec!["true".into()],
                negate: false
            })
        );
        assert_eq!(
            fields[2].visible_when,
            Some(FieldCondition {
                field: "a".into(),
                equals_any: vec!["x".into(), "y".into()],
                negate: true
            })
        );
    }

    #[test]
    fn the_scripts_own_state_rides_along_unread() {
        let f = form(json!({
            "form": {"fields": [{"name": "a"}]},
            "state": {"sprints": ["S-41"]}
        }));
        assert_eq!(f.state, json!({"sprints": ["S-41"]}));
    }
}
