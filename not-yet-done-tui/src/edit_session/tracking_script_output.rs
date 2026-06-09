//! Read-only viewer session for captured script output. Commit is a
//! no-op — closing the editor just dismisses the buffer. Reused by the
//! Trackings and content-view script-menu paths; `scope` is set by the
//! embedder so the action bar slot lights up under the right tab.

use async_trait::async_trait;

use super::{CommitOutcome, EditSession, SessionScope};

pub struct ScriptOutputSession {
    template: String,
    scope: SessionScope,
}

impl ScriptOutputSession {
    pub fn new(content: String) -> Self {
        Self { template: content, scope: SessionScope::Trackings }
    }

    /// Override the action-bar scope (defaults to Trackings for legacy
    /// callers). Content-tab script invocations call this with
    /// `SessionScope::Content`.
    pub fn with_scope(mut self, scope: SessionScope) -> Self {
        self.scope = scope;
        self
    }
}

#[async_trait]
impl EditSession for ScriptOutputSession {
    fn template(&self) -> &str {
        &self.template
    }

    fn suffix(&self) -> &str {
        ".txt"
    }

    fn scope(&self) -> SessionScope {
        self.scope
    }

    fn label(&self) -> &str {
        "run"
    }

    async fn commit(&mut self, _text: &str) -> CommitOutcome {
        CommitOutcome::Cancelled { message: None }
    }
}
