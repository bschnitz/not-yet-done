//! Read-only viewer session for the most recently captured error. Commit
//! is a no-op — closing the editor just dismisses the buffer. The session
//! exists so the user can scroll, search and copy long error messages
//! that would not fit into the inline error bar.

use async_trait::async_trait;

use super::{CommitOutcome, EditSession, SessionScope};

pub struct ErrorViewSession {
    template: String,
    scope: SessionScope,
}

impl ErrorViewSession {
    pub fn new(content: String, scope: SessionScope) -> Self {
        Self { template: content, scope }
    }
}

#[async_trait]
impl EditSession for ErrorViewSession {
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
        "error"
    }

    async fn commit(&mut self, _text: &str) -> CommitOutcome {
        CommitOutcome::Cancelled { message: None }
    }
}
