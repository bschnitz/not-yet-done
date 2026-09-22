//! The round protocol behind script-driven forms (`# mode: form`).
//!
//! A view script normally runs once: it is handed the selected node (or the
//! filtered set, or the table) and answers with commands. A *form* script is
//! asked in rounds instead, so it can put a dialog on screen, look at what
//! came back, and either complain about it or act on it.
//!
//! Each round is a fresh process. The runtime writes a request — the ordinary
//! `scope:` payload plus [`build_request`]'s two extra keys — and reads back
//! exactly one of three answers ([`parse_round`]):
//!
//! - `form` — show this, then run me again. This is both the opening move
//!   ("here are my fields") and every repetition after a failed check ("here
//!   they are again, with these errors"): asking and rejecting are one act.
//! - `result` — we are done; inside sits the ordinary script answer.
//! - `error` — give up, here is why.
//!
//! Because the process is new every round it remembers nothing, so the request
//! carries *every* value collected so far, and a script that needs to remember
//! something of its own (a list it fetched) hands it back as opaque `state`.
//!
//! This module is the protocol only — the shapes, the checks and how one
//! round's answer folds into the form on screen ([`advance`]). Running the
//! process and rendering the form live a layer up; see
//! `docs/plan-script-forms.md`.

use std::collections::BTreeMap;

use serde_json::{Map, Value};
use thiserror::Error;

mod state;
mod wire;

pub use state::{FormState, advance};
pub use wire::{RoundForm, ScriptRound, parse_round};

/// Something the script did that the protocol cannot make sense of — a
/// malformed answer, a field type nobody knows, a parameter that means
/// nothing for the type it was written on.
///
/// Every message names the script, because a form script is configured by
/// hand and *which* one is wrong is half the answer.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[error("form script `{script}`: {message}")]
pub struct ScriptFormError {
    /// The script as it is configured, for the message.
    pub script: String,
    /// What is wrong with what it said.
    pub message: String,
}

impl ScriptFormError {
    pub fn new(script: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            script: script.into(),
            message: message.into(),
        }
    }
}

/// After this many rounds the runtime writes a line to the log.
///
/// Not a limit: every round needs a submit and `Esc` is always there, so a
/// form conversation cannot spin unattended and cutting it off would only
/// punish the rare script that legitimately walks a long wizard. It is a
/// diagnosis aid — a script bouncing between two forms looks exactly like a
/// slow one until something says so.
pub const CHATTY_ROUND_COUNT: usize = 50;

/// The keys [`build_request`] adds to the scope payload. A script's own
/// payload may not use them.
const RESERVED_KEYS: [&str; 2] = ["input", "state"];

/// Build one round's request: the ordinary scope payload, plus what the
/// script cannot know by itself.
///
/// - `input` — every value collected so far, keyed by field name. `{}` in the
///   first round.
/// - `state` — whatever the script returned last round, passed through
///   untouched. Omitted when there is none, so a first round looks the same
///   whether or not the script uses it.
///
/// Refuses a payload that is not a JSON object, or one that already carries a
/// reserved key: silently overwriting it would make the script see something
/// other than what its own scope produced.
pub fn build_request(
    script: &str,
    payload: Value,
    input: &BTreeMap<String, String>,
    state: &Value,
) -> Result<Value, ScriptFormError> {
    let Value::Object(mut obj) = payload else {
        return Err(ScriptFormError::new(
            script,
            "the scope payload is not a JSON object, so there is nowhere to \
             put `input` and `state`",
        ));
    };
    for key in RESERVED_KEYS {
        if obj.contains_key(key) {
            return Err(ScriptFormError::new(
                script,
                format!(
                    "the scope payload already carries `{key}`, which the form protocol reserves"
                ),
            ));
        }
    }

    let mut values = Map::with_capacity(input.len());
    for (k, v) in input {
        values.insert(k.clone(), Value::String(v.clone()));
    }
    obj.insert("input".into(), Value::Object(values));
    if !state.is_null() {
        obj.insert("state".into(), state.clone());
    }
    Ok(Value::Object(obj))
}

#[cfg(test)]
mod request_tests {
    use super::*;
    use serde_json::json;

    fn input(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn the_first_round_carries_an_empty_input_and_no_state() {
        let req = build_request(
            "f.form.py",
            json!({"node": {"id": "n1"}}),
            &input(&[]),
            &Value::Null,
        )
        .expect("payload is an object");
        assert_eq!(req["node"]["id"], json!("n1"));
        assert_eq!(req["input"], json!({}));
        assert!(
            req.get("state").is_none(),
            "no state until the script sends one"
        );
    }

    #[test]
    fn a_later_round_carries_every_value_and_the_scripts_own_state() {
        let req = build_request(
            "f.form.py",
            json!({"node": {"id": "n1"}}),
            &input(&[("title", "Rework"), ("hours", "1.5")]),
            &json!({"sprints": ["S-41"]}),
        )
        .expect("payload is an object");
        assert_eq!(req["input"], json!({"title": "Rework", "hours": "1.5"}));
        assert_eq!(req["state"], json!({"sprints": ["S-41"]}));
    }

    #[test]
    fn a_payload_that_already_uses_a_reserved_key_is_refused() {
        let err = build_request("f.form.py", json!({"input": 1}), &input(&[]), &Value::Null)
            .expect_err("`input` is ours");
        assert!(err.message.contains("`input`"), "{err}");
    }

    #[test]
    fn a_payload_that_is_not_an_object_is_refused() {
        let err = build_request("f.form.py", json!([1, 2]), &input(&[]), &Value::Null)
            .expect_err("an array has no keys");
        assert!(err.message.contains("not a JSON object"), "{err}");
    }
}
