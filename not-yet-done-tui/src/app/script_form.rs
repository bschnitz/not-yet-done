//! The TUI's half of a `# mode: form` script: the dialog on screen and the
//! round loop behind it.
//!
//! Everything about *the protocol* — what a round may answer, how one answer
//! folds into the form, where the request and the answer file live — belongs
//! to `not-yet-done-content` and `not-yet-done-scripts`. What is here is only
//! what a frontend has to decide for itself:
//!
//!   - a round's [`FormState`](not_yet_done_content::script_form::FormState)
//!     becomes a [`ContentFormPopup`], the same widget every other form in
//!     this application is drawn with, so a script's dialog is not a dialect;
//!   - the conversation ends in the ordinary script answer, handed to the
//!     same `commands` path a `# mode: commands` script ends in;
//!   - `Esc` belongs to the popup, which asks before discarding a filled-in
//!     form — the script is told nothing, because there is nothing to tell:
//!     a round it never ran cannot have side effects to undo.
//!
//! Each round is run synchronously, like every other script path in the TUI
//! ([`App::run_script_background`](super::App) and friends): a form script's
//! process is short by construction — it answers a question and exits — and a
//! round that ran off-thread would have to freeze the dialog it came from
//! anyway, to keep the values the next request is built from from changing
//! underneath it.

use std::collections::BTreeMap;

use not_yet_done_content::FieldError;
use not_yet_done_content::script_form::FormState;
use not_yet_done_scripts::form_run::{FormSession, FormStep};

use crate::app::script::ScriptContext;
use crate::app::{App, EditorRequest};
use crate::components::content_form_popup::{ContentFormEvent, ContentFormPopup};

/// A form-script conversation with its current round on screen.
pub struct ScriptFormPopupState {
    /// The round's form, built fresh each time: a script may send different
    /// fields every round, and the values it wants carried over are already
    /// in the state it answered with.
    pub popup: ContentFormPopup,
    /// The conversation, holding the script's own state between rounds.
    session: FormSession,
    /// Where the script was started from — it decides what the payload of
    /// every round looks like and which pane an answer reloads.
    ctx: ScriptContext,
}

impl App {
    /// Open a form script's first round. The entry point [`App::run_script`]
    /// forks into when the header says `# mode: form`.
    ///
    /// Returns an [`EditorRequest`] only for symmetry with the other run
    /// paths — a form conversation never yields the terminal, so it is always
    /// [`EditorRequest::None`].
    pub(super) fn start_form_script(
        &mut self,
        ctx: &ScriptContext,
        script_path: &str,
    ) -> EditorRequest {
        let payload = match serde_json::from_str(&ctx.build_json()) {
            Ok(v) => v,
            Err(e) => {
                self.notify_error(format!("Could not build the script payload: {e}"));
                return EditorRequest::None;
            }
        };
        let session =
            FormSession::new(script_path, payload).with_env(self.child_env_for_script(ctx));
        // A conversation already on screen is replaced rather than queued:
        // two form scripts at once would be two dialogs over one terminal,
        // and the one the user just asked for is the one they meant.
        self.script_form_popup = None;
        self.run_form_round(session, ctx.clone(), BTreeMap::new());
        EditorRequest::None
    }

    /// Feed a key to the open form-script dialog. `true` when there was one
    /// and it took the key.
    pub(super) fn handle_script_form_key(&mut self, key: &str) -> bool {
        let Some(state) = self.script_form_popup.as_mut() else {
            return false;
        };
        match state.popup.handle_key(key) {
            ContentFormEvent::Submitted(values) => {
                let state = self.script_form_popup.take().expect("just borrowed");
                let submitted = values.into_iter().collect();
                self.run_form_round(state.session, state.ctx, submitted);
            }
            ContentFormEvent::Cancelled => {
                self.script_form_popup = None;
            }
            ContentFormEvent::Consumed => {}
        }
        true
    }

    /// Run one round and put what came back where it belongs: a new form on
    /// screen, the script's result through the ordinary answer path, or the
    /// script's own complaint in the notification bar.
    fn run_form_round(
        &mut self,
        mut session: FormSession,
        ctx: ScriptContext,
        submitted: BTreeMap<String, String>,
    ) {
        let round = match session.next(&submitted) {
            Ok(round) => round,
            Err(e) => {
                // The session is dropped with the error, taking its request
                // file — and the row inside it — along.
                self.notify_error(e.to_string());
                return;
            }
        };
        for notice in round.notices {
            self.notify(notice);
        }
        match round.step {
            FormStep::Ask(state) => {
                let popup = self.build_script_form_popup(&state);
                self.script_form_popup = Some(ScriptFormPopupState {
                    popup,
                    session,
                    ctx,
                });
            }
            FormStep::Done(answer) => {
                drop(session);
                self.apply_script_answer(answer);
                // Only a finished conversation reloads: a script that gave up
                // (or was cancelled) says by doing so that it changed nothing,
                // and a reload in the middle of a wizard would pull the row
                // the next round is about out from under it.
                self.reload_after_script(&ctx);
            }
            FormStep::Failed(message) => {
                drop(session);
                self.notify_error(message);
            }
        }
    }

    /// Build one round's dialog: the script's fields, the values the protocol
    /// carried over, and its complaints where they belong.
    fn build_script_form_popup(&mut self, state: &FormState) -> ContentFormPopup {
        let title = state
            .header
            .clone()
            .unwrap_or_else(|| "Script form".to_string());
        let prefill = state
            .values
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let options = Self::build_form_options(None, &state.fields);
        let mut popup =
            ContentFormPopup::new(title, state.fields.clone(), &prefill, &self.theme, &options);
        popup.show_errors(&field_errors(state));
        popup
    }
}

/// A round's complaints in the shape the popup shows them in — the protocol
/// keeps every message on a field, the dialog shows one line per field.
///
/// **In form order.** The popup opens on the first field it is handed, and
/// only that field's message is on screen; which one the reader is shown
/// first therefore decides where they start. [`FormState::field_errors`] is a
/// `BTreeMap`, so taking the keys as they come would open on whichever
/// complained field sorts first — the alphabet, which nobody wrote down and
/// nobody can see. The field nearest the top of the form is the one an eye
/// starts at, so that is the one to land on.
fn field_errors(state: &FormState) -> Vec<FieldError> {
    let mut errors: Vec<FieldError> = state
        .fields
        .iter()
        .filter_map(|field| {
            state
                .field_error_text(&field.key)
                .map(|text| FieldError::field(&field.key, text))
        })
        .collect();
    // A message about a field this round no longer shows has nowhere to land,
    // but it is still the script's word about the submission, so it is kept
    // behind the ones that do — never ahead of them, or the form would open
    // on a field that is not there.
    errors.extend(
        state
            .field_errors
            .keys()
            .filter(|key| !state.fields.iter().any(|f| &f.key == *key))
            .filter_map(|key| {
                state
                    .field_error_text(key)
                    .map(|text| FieldError::field(key, text))
            }),
    );
    errors.extend(
        state
            .general_errors
            .iter()
            .map(|m| FieldError::general(m.clone())),
    );
    errors
}

#[cfg(test)]
mod tests {
    use not_yet_done_content::FormFieldSpec;

    use super::*;

    fn state_with(field_errors: &[(&str, &[&str])], general: &[&str]) -> FormState {
        FormState {
            field_errors: field_errors
                .iter()
                .map(|(k, ms)| {
                    (
                        (*k).to_string(),
                        ms.iter().map(|m| (*m).to_string()).collect(),
                    )
                })
                .collect(),
            general_errors: general.iter().map(|m| (*m).to_string()).collect(),
            ..FormState::default()
        }
    }

    #[test]
    fn a_fields_messages_are_joined_into_one() {
        // The protocol keeps every complaint about a value; the dialog has one
        // line per field, so they travel joined rather than dropped.
        let errors = field_errors(&state_with(&[("title", &["too long", "taken"])], &[]));
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].field.as_deref(), Some("title"));
        assert_eq!(errors[0].message, "too long; taken");
    }

    #[test]
    fn general_messages_keep_no_field() {
        let errors = field_errors(&state_with(&[], &["the board is read-only"]));
        assert_eq!(errors.len(), 1);
        assert!(errors[0].field.is_none());
        assert_eq!(errors[0].message, "the board is read-only");
    }

    #[test]
    fn field_messages_come_before_the_general_ones() {
        // The first entry decides which field the reopened form lands on, so a
        // general message must not be able to take that place.
        let errors = field_errors(&state_with(&[("a", &["no"])], &["also no"]));
        assert_eq!(errors[0].field.as_deref(), Some("a"));
        assert!(errors[1].field.is_none());
    }

    #[test]
    fn a_form_without_complaints_produces_none() {
        assert!(field_errors(&FormState::default()).is_empty());
    }

    /// The reopened form lands on the first message it is handed, so the
    /// order is the form's, not the alphabet's. A script that complains about
    /// the title and the approver must reopen on the title, which is what the
    /// reader sees first — even though `approver` sorts before it in the
    /// `BTreeMap` the messages were kept in.
    #[test]
    fn messages_follow_the_order_of_the_fields() {
        let mut state = state_with(
            &[
                ("approver", &["who signed off?"]),
                ("title", &["too short"]),
            ],
            &[],
        );
        state.fields = vec![
            FormFieldSpec::text("title", "Title"),
            FormFieldSpec::text("approver", "Approved by"),
        ];
        let errors = field_errors(&state);
        assert_eq!(errors[0].field.as_deref(), Some("title"));
        assert_eq!(errors[1].field.as_deref(), Some("approver"));
    }

    /// A message about a field the round has dropped cannot be shown beside
    /// it, but it is still what the script said — it stays, behind the ones
    /// that have a field to stand next to.
    #[test]
    fn a_message_about_a_vanished_field_comes_last() {
        let mut state = state_with(&[("gone", &["stale"]), ("title", &["too short"])], &[]);
        state.fields = vec![FormFieldSpec::text("title", "Title")];
        let errors = field_errors(&state);
        assert_eq!(errors[0].field.as_deref(), Some("title"));
        assert_eq!(errors[1].field.as_deref(), Some("gone"));
    }
}
