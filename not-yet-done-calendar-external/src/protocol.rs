//! The line protocol between nyd and an external calendar backend — see
//! ADR 0014.
//!
//! One JSON object per line, in both directions; **stdout is the protocol,
//! stderr is the log** (it lands in `<log dir>/calendar-<name>.log`, as an
//! auth plugin's does). Every line is one externally tagged variant:
//! `{"list_events":{"id":3,"range":{…}}}`.
//!
//! The conversation:
//!
//! 1. nyd writes [`ToBackend::Start`] — the connection, its opaque `config:`
//!    block and the secrets nyd resolved for it — and the backend answers
//!    [`FromBackend::Hello`]. A backend that cannot speak the protocol
//!    version answers [`FromBackend::Fatal`].
//! 2. nyd sends calls, each with an `id` of its own; the backend answers each
//!    with one [`FromBackend::Reply`] carrying that `id`. Calls may overlap:
//!    replies are matched by `id`, not by order.
//! 3. Between replies, unasked, the backend may report load progress, ask the
//!    person for attention, or put a form in front of them; nyd answers a form
//!    with [`ToBackend::Input`] or [`ToBackend::Dismissed`].
//! 4. [`ToBackend::Cancel`], or stdin closing, ends it: the backend cleans up
//!    (closes its browser, say) and exits.
//!
//! The types are both readable and writable, so a backend written in Rust
//! reads the line nyd wrote instead of transcribing its shape — the drift
//! between two spellings of one protocol is what this module exists to
//! prevent.
//!
//! Secrets travel in `start` and nowhere else; `attention`, `form` and
//! `progress` reach the status line and the log.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use not_yet_done_calendar_core::{CalendarError, EventDraft, EventPatch, LoadProgress, TimeRange};
use not_yet_done_content::auth::ScriptFormField;

/// The protocol version nyd announces in `start`.
pub const PROTOCOL: u32 = 1;

/// What nyd writes to the backend.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ToBackend {
    /// Once, first.
    Start {
        protocol: u32,
        /// The connection's `id` from the calendar config.
        connection: String,
        /// The connection's `config:` block, verbatim; nyd does not read it.
        #[serde(default)]
        config: serde_json::Value,
        /// The configured `secrets:`, resolved, by name.
        #[serde(default)]
        secrets: BTreeMap<String, String>,
    },
    /// Every event overlapping the range. Replies with a list of events.
    ListEvents { id: u64, range: TimeRange },
    /// The calendars a new event may be created in. Replies with a list of
    /// calendars.
    ListCalendars { id: u64 },
    /// Create an event in the calendar named by `calendar_id`, or in the
    /// default one. Replies with the event as created.
    CreateEvent {
        id: u64,
        #[serde(default)]
        calendar_id: Option<String>,
        draft: EventDraft,
    },
    /// Change an event as the patch says. Replies with `null`.
    UpdateEvent {
        id: u64,
        uid: String,
        patch: EventPatch,
    },
    /// Delete an event. Replies with `null`.
    DeleteEvent { id: u64, uid: String },
    /// Show the backend's window if it is hidden, hide it if it is shown.
    /// Replies with whether it is shown now.
    ToggleWindow { id: u64 },
    /// The answers to the form `form`.
    Input {
        form: u64,
        values: BTreeMap<String, String>,
    },
    /// Nobody answered the form `form`: dismissed, or no one there to ask.
    Dismissed { form: u64 },
    /// Clean up and exit.
    Cancel {},
}

/// What the backend writes to nyd.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum FromBackend {
    /// The answer to `start`.
    Hello {
        protocol: u32,
        /// Whether an empty event list means the calendar really is empty in
        /// the range (see `CalendarBackend::empty_is_authoritative`). Default
        /// `false`: nyd then keeps the events it knows.
        #[serde(default)]
        empty_is_authoritative: bool,
    },
    /// The answer to the call `id`: `error` if it failed, else `ok`.
    Reply {
        id: u64,
        #[serde(default)]
        ok: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ReplyError>,
    },
    /// How far a load has got; the terminal one sets `done`.
    Progress(LoadProgress),
    /// A sentence telling the person what to do somewhere nyd cannot reach
    /// ("Approve the sign-in on your phone"), and `null` once that is over.
    Attention(Option<String>),
    /// Ask the person this; nyd answers with `input` or `dismissed`.
    Form {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        header: Option<String>,
        /// Why the form is shown again ("that code was wrong").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        fields: Vec<ScriptFormField>,
    },
    /// The backend cannot go on; shown to the user. The process is restarted
    /// on the next call.
    Fatal(String),
}

impl FromBackend {
    /// A successful reply carrying `value`.
    pub fn ok(id: u64, value: impl Serialize) -> Self {
        FromBackend::Reply {
            id,
            ok: serde_json::to_value(value).unwrap_or(serde_json::Value::Null),
            error: None,
        }
    }

    /// A failed reply.
    pub fn error(id: u64, kind: ErrorKind, message: impl Into<String>) -> Self {
        FromBackend::Reply {
            id,
            ok: serde_json::Value::Null,
            error: Some(ReplyError {
                kind,
                message: message.into(),
            }),
        }
    }
}

/// Why a call failed.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReplyError {
    pub kind: ErrorKind,
    /// Shown to the user.
    pub message: String,
}

/// The kinds of [`CalendarError`], plus `unsupported` for a call the backend
/// does not implement.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Auth,
    Network,
    Config,
    NotFound,
    Unsupported,
    #[serde(other)]
    Other,
}

impl ReplyError {
    pub(crate) fn into_calendar_error(self) -> CalendarError {
        let ReplyError { kind, message } = self;
        match kind {
            ErrorKind::Auth => CalendarError::Auth(message),
            ErrorKind::Network => CalendarError::Network(message),
            ErrorKind::Config => CalendarError::Config(message),
            ErrorKind::NotFound => CalendarError::NotFound(message),
            ErrorKind::Unsupported | ErrorKind::Other => CalendarError::Other(message),
        }
    }
}

impl From<&CalendarError> for ReplyError {
    /// For a backend written in Rust: answer with the error it already has.
    fn from(e: &CalendarError) -> Self {
        let (kind, message) = match e {
            CalendarError::Auth(m) => (ErrorKind::Auth, m),
            CalendarError::Network(m) => (ErrorKind::Network, m),
            CalendarError::Config(m) => (ErrorKind::Config, m),
            CalendarError::NotFound(m) => (ErrorKind::NotFound, m),
            CalendarError::Other(m) => (ErrorKind::Other, m),
        };
        ReplyError {
            kind,
            message: message.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_one_tagged_variant() {
        let line = serde_json::to_string(&ToBackend::DeleteEvent {
            id: 3,
            uid: "e1".into(),
        })
        .unwrap();
        assert_eq!(line, r#"{"delete_event":{"id":3,"uid":"e1"}}"#);
        let line = serde_json::to_string(&ToBackend::Cancel {}).unwrap();
        assert_eq!(line, r#"{"cancel":{}}"#);
    }

    #[test]
    fn attention_is_withdrawn_by_null() {
        let said: FromBackend = serde_json::from_str(r#"{"attention":null}"#).unwrap();
        assert_eq!(said, FromBackend::Attention(None));
        let said: FromBackend =
            serde_json::from_str(r#"{"attention":"Tap 42 on your phone"}"#).unwrap();
        assert_eq!(
            said,
            FromBackend::Attention(Some("Tap 42 on your phone".into()))
        );
    }

    #[test]
    fn a_reply_without_ok_is_a_null_success() {
        let said: FromBackend = serde_json::from_str(r#"{"reply":{"id":7}}"#).unwrap();
        assert_eq!(
            said,
            FromBackend::Reply {
                id: 7,
                ok: serde_json::Value::Null,
                error: None
            }
        );
        let said: FromBackend = serde_json::from_str(
            r#"{"reply":{"id":7,"error":{"kind":"not_found","message":"gone"}}}"#,
        )
        .unwrap();
        let FromBackend::Reply { error: Some(e), .. } = said else {
            panic!("an error reply");
        };
        assert!(matches!(e.into_calendar_error(), CalendarError::NotFound(m) if m == "gone"));
    }

    #[test]
    fn progress_and_a_form_read_as_written() {
        let said: FromBackend = serde_json::from_str(r#"{"progress":{"fraction":0.5}}"#).unwrap();
        assert_eq!(said, FromBackend::Progress(LoadProgress::at(0.5)));
        let said: FromBackend = serde_json::from_str(
            r#"{"form":{"id":1,"header":"Second factor","fields":[{"name":"code"}]}}"#,
        )
        .unwrap();
        let FromBackend::Form { id, fields, .. } = said else {
            panic!("a form");
        };
        assert_eq!((id, fields[0].name.as_str()), (1, "code"));
    }

    #[test]
    fn an_unknown_key_is_refused() {
        assert!(serde_json::from_str::<FromBackend>(r#"{"hello":{"protocol":1,"x":1}}"#).is_err());
    }
}
