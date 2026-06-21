//! Small helpers for reading [`InputSpec::Form`] field maps in the local
//! adapters' `execute` paths.
//!
//! Both the Trackings adapter (`split`/`move`) and the Projects adapter
//! (`create`/`edit`/`delete`) receive their action input as a
//! `HashMap<String, String>` keyed by [`FormFieldSpec::key`] — text/select
//! fields carry their string value (possibly empty for an optional field),
//! toggles carry `"true"`/`"false"`. These four functions centralise the
//! trim/empty/flag conventions so each adapter doesn't re-derive them.
//!
//! [`InputSpec::Form`]: not_yet_done_content::InputSpec::Form
//! [`FormFieldSpec::key`]: not_yet_done_content::FormFieldSpec::key

use std::collections::HashMap;

use not_yet_done_content::{ContentError, Result};

/// Read a Form field, treating absent or whitespace-only values as `None`.
pub(crate) fn form_opt(values: &HashMap<String, String>, key: &str) -> Option<String> {
    values
        .get(key)
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string())
}

/// Read a required Form field; error if absent or empty.
pub(crate) fn form_required<'a>(values: &'a HashMap<String, String>, key: &str) -> Result<&'a str> {
    match values.get(key).map(|v| v.trim()) {
        Some(v) if !v.is_empty() => Ok(v),
        _ => Err(invalid_input(format!("field '{key}' is required"))),
    }
}

/// A Toggle delivers `"true"`/`"false"`; treat anything other than `"true"` as
/// off (an absent toggle is off).
pub(crate) fn form_flag(values: &HashMap<String, String>, key: &str) -> bool {
    values.get(key).map(|v| v == "true").unwrap_or(false)
}

/// Wrap a user-facing parse/validation message as a [`ContentError`].
pub(crate) fn invalid_input(msg: String) -> ContentError {
    ContentError::Other(msg.into())
}
