//! Edit session for the YAML task query filter.
//!
//! Thin descriptor: holds the saved-query name and is_new flag, hands the
//! buffer back via `FollowUp` on each live save and on close. The App applies
//! the filter and persists it.

use async_trait::async_trait;

use super::{CommitOutcome, EditSession, FollowUp, SessionScope};

pub struct TaskQueryFilterSession {
    name: String,
    is_new: bool,
    template: String,
}

impl TaskQueryFilterSession {
    pub fn new(name: String, is_new: bool, template: String) -> Self {
        Self { name, is_new, template }
    }
}

#[async_trait]
impl EditSession for TaskQueryFilterSession {
    fn template(&self) -> &str {
        &self.template
    }

    fn suffix(&self) -> &str {
        ".yaml"
    }

    fn scope(&self) -> SessionScope {
        SessionScope::Tasks
    }

    fn label(&self) -> &str {
        "edit query"
    }

    async fn commit(&mut self, text: &str) -> CommitOutcome {
        CommitOutcome::FollowUp(FollowUp::CloseTaskFilter {
            content: text.to_string(),
            name: self.name.clone(),
            is_new: self.is_new,
        })
    }

    async fn live_apply(&mut self, text: &str) -> Option<FollowUp> {
        Some(FollowUp::ApplyTaskFilter { content: text.to_string() })
    }
}
