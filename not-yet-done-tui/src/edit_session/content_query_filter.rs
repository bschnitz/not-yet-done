//! Edit session for a content view's query (e.g. JQL).
//!
//! Thin descriptor: holds the view index, optional save-name, is_new flag,
//! and the editor template. App applies the live query and persists/saves
//! on close via `FollowUp` dispatch.

use async_trait::async_trait;

use super::{CommitOutcome, EditSession, FollowUp, SessionScope};

pub struct ContentQueryFilterSession {
    view_index: usize,
    save_name: Option<String>,
    is_new: bool,
    template: String,
}

impl ContentQueryFilterSession {
    pub fn new(
        view_index: usize,
        save_name: Option<String>,
        is_new: bool,
        template: String,
    ) -> Self {
        Self { view_index, save_name, is_new, template }
    }
}

#[async_trait]
impl EditSession for ContentQueryFilterSession {
    fn template(&self) -> &str {
        &self.template
    }

    fn suffix(&self) -> &str {
        ".jql"
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
        })
    }

    async fn live_apply(&mut self, text: &str) -> Option<FollowUp> {
        Some(FollowUp::ApplyContentFilter {
            view_index: self.view_index,
            content: text.to_string(),
            save_name: self.save_name.clone(),
        })
    }
}
