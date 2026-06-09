//! Edit session for restructuring a task subtree via the tree editor.
//!
//! Owns the subtree snapshot that gets diffed on each save. After every
//! successful apply, the snapshot is refreshed from the DB so subsequent
//! saves see new IDs and don't re-create rows that already exist.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use not_yet_done_core::entity::task::Model as Task;
use not_yet_done_core::repository::TrackingRepository;
use not_yet_done_core::service::TaskService;

use crate::app::is_in_subtree;
use crate::tree_edit;

use super::{CommitOutcome, EditSession, FollowUp, SessionScope};

pub struct RestructureSession {
    task_service: Arc<dyn TaskService>,
    tracking_repo: Arc<dyn TrackingRepository>,
    allow_parallel: bool,
    original_tasks: Vec<Task>,
    root_id: Uuid,
    template: String,
}

impl RestructureSession {
    pub fn new(
        task_service: Arc<dyn TaskService>,
        tracking_repo: Arc<dyn TrackingRepository>,
        allow_parallel: bool,
        original_tasks: Vec<Task>,
        root_id: Uuid,
        template: String,
    ) -> Self {
        Self {
            task_service,
            tracking_repo,
            allow_parallel,
            original_tasks,
            root_id,
            template,
        }
    }

    async fn fetch_tracked_ids(&self) -> HashSet<Uuid> {
        self.tracking_repo
            .find_all_active()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|t| t.task_id)
            .collect()
    }

    async fn refresh_originals(&mut self) {
        if let Ok(all_tasks) = self.task_service.list_tasks(None).await {
            self.original_tasks = all_tasks
                .iter()
                .filter(|t| is_in_subtree(t, self.root_id, &all_tasks))
                .cloned()
                .collect();
        }
    }

    async fn run_apply(&mut self, content: &str) -> Result<String, String> {
        let tracked_ids = self.fetch_tracked_ids().await;
        tree_edit::apply_changes(
            content,
            &self.original_tasks,
            self.root_id,
            &self.task_service,
            &self.tracking_repo,
            &tracked_ids,
            self.allow_parallel,
        )
        .await
    }
}

#[async_trait]
impl EditSession for RestructureSession {
    fn template(&self) -> &str {
        &self.template
    }

    fn suffix(&self) -> &str {
        ".md"
    }

    fn scope(&self) -> SessionScope {
        SessionScope::Tasks
    }

    fn label(&self) -> &str {
        "edit node"
    }

    async fn commit(&mut self, text: &str) -> CommitOutcome {
        match self.run_apply(text).await {
            Ok(msg) => {
                self.refresh_originals().await;
                CommitOutcome::FollowUp(FollowUp::ReloadTasks {
                    focus_id: Some(self.root_id),
                    tracking_changed: false,
                    message: msg,
                })
            }
            Err(e) => CommitOutcome::Reopen {
                content: render_with_error(text, &e),
            },
        }
    }

    async fn live_apply(&mut self, text: &str) -> Option<FollowUp> {
        match self.run_apply(text).await {
            Ok(msg) => {
                self.refresh_originals().await;
                Some(FollowUp::ReloadTasks {
                    focus_id: Some(self.root_id),
                    tracking_changed: false,
                    message: msg,
                })
            }
            Err(e) => Some(FollowUp::SetQueryError(e)),
        }
    }
}

/// Inline error banner. The tree-edit parser skips lines starting with
/// `#`, so the banner is parser-invisible — and stripped before re-rendering
/// so reopens don't stack.
const ERROR_BANNER_START: &str = "# ─── ERRORS ───";
const ERROR_BANNER_END:   &str = "# ──────────────";

fn strip_error_banner(text: &str) -> &str {
    if let Some(rest) = text.strip_prefix(ERROR_BANNER_START) {
        let after_start = rest.strip_prefix('\n').unwrap_or(rest);
        let needle = format!("\n{ERROR_BANNER_END}");
        if let Some(pos) = after_start.find(&needle) {
            let after_end = &after_start[pos + needle.len()..];
            return after_end.strip_prefix('\n').unwrap_or(after_end);
        }
        return after_start;
    }
    text
}

fn render_with_error(text: &str, error: &str) -> String {
    let stripped = strip_error_banner(text);
    let mut out = String::new();
    out.push_str(ERROR_BANNER_START);
    out.push('\n');
    for line in error.lines() {
        out.push_str(&format!("# • {line}\n"));
    }
    out.push_str(ERROR_BANNER_END);
    out.push('\n');
    out.push_str(stripped);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_with_error_prepends_banner() {
        let original = "- [ ] task\n";
        let out = render_with_error(original, "Line 2: missing ']'");
        assert!(out.starts_with(ERROR_BANNER_START));
        assert!(out.contains("# • Line 2: missing ']'"));
        assert!(out.ends_with("- [ ] task\n"));
    }

    #[test]
    fn render_with_error_does_not_stack() {
        let text = "- [ ] task\n";
        let once = render_with_error(text, "first");
        let twice = render_with_error(&once, "first");
        assert_eq!(once, twice);
    }
}
