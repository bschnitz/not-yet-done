//! Edit session for a content view's query (e.g. JQL).
//!
//! Thin descriptor: holds the view index, optional save-name, is_new flag,
//! and the editor template. App applies the live query and persists/saves
//! on close via `FollowUp` dispatch.

use async_trait::async_trait;
use not_yet_done_content::QueryKind;

use super::{CommitOutcome, EditSession, FollowUp, SessionScope};

pub struct ContentQueryFilterSession {
    view_index: usize,
    save_name: Option<String>,
    is_new: bool,
    template: String,
    /// Editor file suffix for syntax highlighting — the adapter's query
    /// language (`.yaml` for the FilterExpr DSL, `.jql`/`.cql` for Jira /
    /// Confluence), or `.md` for an extended document, whose container is
    /// the framework's format rather than the adapter's. See
    /// `ContentAdapter::query_body_suffix`.
    suffix: String,
    /// Which store this body belongs to. Carried through the session
    /// because the App cannot recover it from the text on commit: an
    /// extended document is a Markdown file, and a `yaml`-language
    /// adapter's own query would look just like the spec fence inside one.
    kind: QueryKind,
}

impl ContentQueryFilterSession {
    pub fn new(
        view_index: usize,
        save_name: Option<String>,
        is_new: bool,
        template: String,
        suffix: String,
        kind: QueryKind,
    ) -> Self {
        Self {
            view_index,
            save_name,
            is_new,
            template,
            suffix,
            kind,
        }
    }
}

#[async_trait]
impl EditSession for ContentQueryFilterSession {
    fn template(&self) -> &str {
        &self.template
    }

    fn suffix(&self) -> &str {
        &self.suffix
    }

    fn scope(&self) -> SessionScope {
        SessionScope::Content
    }

    fn label(&self) -> &str {
        "edit query"
    }

    async fn commit(&mut self, text: &str) -> CommitOutcome {
        CommitOutcome::FollowUp(FollowUp::CloseContentQuery {
            view_index: self.view_index,
            content: text.to_string(),
            save_name: self.save_name.clone(),
            is_new: self.is_new,
            kind: self.kind,
        })
    }

    async fn live_apply(&mut self, text: &str) -> Option<FollowUp> {
        Some(FollowUp::ApplyContentFilter {
            view_index: self.view_index,
            content: text.to_string(),
            save_name: self.save_name.clone(),
            kind: self.kind,
        })
    }
}
