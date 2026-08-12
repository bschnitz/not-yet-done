//! Read-only viewer session for the notification log. Commit is a no-op —
//! closing the editor just drops the buffer. The session exists because the
//! bottom notification bar only shows the last few messages (one, when
//! `notifications.max_messages: 1`): the log holds everything that scrolled
//! past, so it has to be readable somewhere with scrolling, search and copy.

use async_trait::async_trait;

use super::{CommitOutcome, EditSession, SessionScope};

pub struct NotificationViewSession {
    template: String,
    scope: SessionScope,
}

impl NotificationViewSession {
    pub fn new(content: String, scope: SessionScope) -> Self {
        Self {
            template: content,
            scope,
        }
    }
}

#[async_trait]
impl EditSession for NotificationViewSession {
    fn template(&self) -> &str {
        &self.template
    }

    fn suffix(&self) -> &str {
        ".log"
    }

    fn scope(&self) -> SessionScope {
        self.scope
    }

    fn label(&self) -> &str {
        "notifications"
    }

    async fn commit(&mut self, _text: &str) -> CommitOutcome {
        CommitOutcome::Cancelled { message: None }
    }
}
