//! Read-only viewer session for captured script output. Commit is a
//! no-op — closing the editor just dismisses the buffer. Reused by the
//! Trackings and content-view script-menu paths; `scope` is set by the
//! embedder so the action bar slot lights up under the right tab.

use async_trait::async_trait;

use super::{CommitOutcome, EditSession, SessionScope};

pub struct ScriptOutputSession {
    template: String,
    scope: SessionScope,
    suffix: String,
}

impl ScriptOutputSession {
    pub fn new(content: String) -> Self {
        Self { template: content, scope: SessionScope::Trackings, suffix: ".txt".to_string() }
    }

    /// Override the action-bar scope (defaults to Trackings for legacy
    /// callers). Content-tab script invocations call this with
    /// `SessionScope::Content`.
    pub fn with_scope(mut self, scope: SessionScope) -> Self {
        self.scope = scope;
        self
    }

    /// Override the temp-buffer file extension (defaults to `.txt`). Set from
    /// the script's `# output:` header so a Markdown-emitting script renders as
    /// Markdown in the viewer. Pass a value including the leading dot.
    pub fn with_suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = suffix.into();
        self
    }
}

#[async_trait]
impl EditSession for ScriptOutputSession {
    fn template(&self) -> &str {
        &self.template
    }

    fn suffix(&self) -> &str {
        &self.suffix
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
