//! Edit session for the YAML tracking query filter.
//!
//! Mirror of `TaskQueryFilterSession` for the trackings tab.

use async_trait::async_trait;

use super::{CommitOutcome, EditSession, FollowUp, SessionScope};

pub struct TrackingQueryFilterSession {
    name: String,
    is_new: bool,
    template: String,
}

impl TrackingQueryFilterSession {
    pub fn new(name: String, is_new: bool, template: String) -> Self {
        Self { name, is_new, template }
    }
}

#[async_trait]
impl EditSession for TrackingQueryFilterSession {
    fn template(&self) -> &str {
        &self.template
    }

    fn suffix(&self) -> &str {
        ".yaml"
    }

    fn scope(&self) -> SessionScope {
        SessionScope::Trackings
    }

    fn label(&self) -> &str {
        "edit query"
    }

    async fn commit(&mut self, text: &str) -> CommitOutcome {
        CommitOutcome::FollowUp(FollowUp::CloseTrackingFilter {
            content: text.to_string(),
            name: self.name.clone(),
            is_new: self.is_new,
        })
    }

    async fn live_apply(&mut self, text: &str) -> Option<FollowUp> {
        Some(FollowUp::ApplyTrackingFilter { content: text.to_string() })
    }
}
