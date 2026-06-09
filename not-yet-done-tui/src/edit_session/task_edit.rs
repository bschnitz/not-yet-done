//! Edit session for adding/editing local tasks.
//!
//! Owns the parsing (via `editor_templates`), persistence (via
//! `TaskService`), notes file I/O, and tracking start/stop logic. Mode
//! transitions from `Create` to `Edit` after the first successful save so
//! subsequent live-saves update the new task instead of creating duplicates.

use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use not_yet_done_core::entity::task::Model as Task;
use not_yet_done_core::repository::TrackingRepository;
use not_yet_done_core::service::TaskService;

use crate::editor_templates::{self, FieldError, ParseResult};
use crate::notes;

use super::{CommitOutcome, EditSession, FollowUp, SessionScope};

enum Mode {
    Create,
    Edit { task: Task, was_tracked: bool },
}

pub struct TaskEditSession {
    task_service: Arc<dyn TaskService>,
    tracking_repo: Arc<dyn TrackingRepository>,
    allow_parallel: bool,
    task_rows: Vec<Task>,
    template: String,
    mode: Mode,
}

impl TaskEditSession {
    pub fn create(
        task_service: Arc<dyn TaskService>,
        tracking_repo: Arc<dyn TrackingRepository>,
        allow_parallel: bool,
        task_rows: Vec<Task>,
        parent_id: Option<Uuid>,
    ) -> Self {
        let template = editor_templates::new_task(parent_id);
        Self {
            task_service,
            tracking_repo,
            allow_parallel,
            task_rows,
            template,
            mode: Mode::Create,
        }
    }

    pub fn edit(
        task_service: Arc<dyn TaskService>,
        tracking_repo: Arc<dyn TrackingRepository>,
        allow_parallel: bool,
        task_rows: Vec<Task>,
        task: Task,
        was_tracked: bool,
    ) -> Self {
        let notes_str = notes::read_notes(&task, &task_rows);
        let template = editor_templates::edit_task_with_notes(&task, was_tracked, &notes_str);
        Self {
            task_service,
            tracking_repo,
            allow_parallel,
            task_rows,
            template,
            mode: Mode::Edit { task, was_tracked },
        }
    }

    async fn apply_tracking(&self, task_id: Uuid, wants_tracked: bool) {
        let now = chrono::Utc::now();
        if wants_tracked {
            if !self.allow_parallel {
                if let Ok(active) = self.tracking_repo.find_all_active().await {
                    for t in active {
                        let _ = self.tracking_repo.stop(t.id, now).await;
                    }
                }
            }
            let _ = self.tracking_repo.insert(task_id, now, None).await;
        } else if let Ok(Some(t)) = self.tracking_repo.find_active_for_task(task_id).await {
            let _ = self.tracking_repo.stop(t.id, now).await;
        }
    }

    async fn commit_create(&mut self, text: &str) -> CommitOutcome {
        let result = editor_templates::parse_new_task(text, &self.template);
        match result {
            ParseResult::Aborted => CommitOutcome::Cancelled {
                message: Some("Add cancelled".into()),
            },
            ParseResult::Errors { errors, original_content } => CommitOutcome::Reopen {
                content: editor_templates::render_with_errors(&original_content, &errors),
            },
            ParseResult::Ok(parsed) => {
                let notes_text = editor_templates::parse_notes(text);
                let wants_tracking = parsed.tracking;
                let svc_result = self
                    .task_service
                    .add_task(
                        parsed.description,
                        None,
                        parsed.parent_id,
                        None,
                        parsed.status,
                        parsed.priority,
                    )
                    .await;
                match svc_result {
                    Ok(created) => {
                        notes::write_notes(&created, &self.task_rows, &notes_text);
                        if wants_tracking {
                            self.apply_tracking(created.id, true).await;
                        }
                        // Flip to Edit so subsequent live-saves update.
                        let new_template =
                            editor_templates::edit_task(&created, wants_tracking);
                        self.task_rows.push(created.clone());
                        self.template = new_template;
                        let focus_id = created.id;
                        self.mode = Mode::Edit {
                            task: created,
                            was_tracked: wants_tracking,
                        };
                        CommitOutcome::FollowUp(FollowUp::ReloadTasks {
                            focus_id: Some(focus_id),
                            tracking_changed: wants_tracking,
                            message: "Task created".into(),
                        })
                    }
                    Err(e) => {
                        let errors = vec![FieldError {
                            field: "description",
                            message: format!("Service error: {e}"),
                        }];
                        CommitOutcome::Reopen {
                            content: editor_templates::render_with_errors(text, &errors),
                        }
                    }
                }
            }
        }
    }

    async fn commit_edit(&mut self, text: &str) -> CommitOutcome {
        let (task_id, original_desc, original_parent_id, was_tracked) = match &self.mode {
            Mode::Edit { task, was_tracked } => (
                task.id,
                task.description.clone(),
                task.parent_id,
                *was_tracked,
            ),
            Mode::Create => unreachable!("commit_edit called in Create mode"),
        };
        let task_for_parse = match &self.mode {
            Mode::Edit { task, .. } => task.clone(),
            Mode::Create => unreachable!(),
        };

        let result = editor_templates::parse_edit_task(text, &self.template, &task_for_parse);
        match result {
            ParseResult::Aborted => CommitOutcome::Cancelled {
                message: Some("Edit cancelled".into()),
            },
            ParseResult::Errors { errors, original_content } => CommitOutcome::Reopen {
                content: editor_templates::render_with_errors(&original_content, &errors),
            },
            ParseResult::Ok(parsed) => {
                let notes_text = editor_templates::parse_notes(text);
                if let Some(ref new_desc) = parsed.description {
                    notes::rename_notes(&task_for_parse, &original_desc, new_desc, &self.task_rows);
                }
                let parent_changed =
                    parsed.parent_id.is_some() && parsed.parent_id != Some(original_parent_id);
                let tracking_changed = parsed.tracking.map_or(false, |t| t != was_tracked);

                let svc_result = self
                    .task_service
                    .update_task(
                        task_id,
                        parsed.description,
                        parsed.status,
                        parsed.priority,
                        parsed.parent_id,
                        None,
                    )
                    .await;
                match svc_result {
                    Ok(updated) => {
                        if parent_changed {
                            let new_rows: Vec<Task> = self
                                .task_rows
                                .iter()
                                .map(|t| if t.id == updated.id { updated.clone() } else { t.clone() })
                                .collect();
                            notes::move_notes(&updated, &self.task_rows, &new_rows);
                            notes::write_notes(&updated, &new_rows, &notes_text);
                            self.task_rows = new_rows;
                        } else {
                            notes::write_notes(&updated, &self.task_rows, &notes_text);
                        }
                        if tracking_changed {
                            if let Some(want) = parsed.tracking {
                                self.apply_tracking(updated.id, want).await;
                            }
                        }
                        let new_was_tracked = parsed.tracking.unwrap_or(was_tracked);
                        // Refresh mode + template so subsequent saves see current state.
                        self.template =
                            editor_templates::edit_task(&updated, new_was_tracked);
                        let focus_id = updated.id;
                        self.mode = Mode::Edit {
                            task: updated,
                            was_tracked: new_was_tracked,
                        };
                        CommitOutcome::FollowUp(FollowUp::ReloadTasks {
                            focus_id: Some(focus_id),
                            tracking_changed,
                            message: "Task updated".into(),
                        })
                    }
                    Err(e) => {
                        let errors = vec![FieldError {
                            field: "description",
                            message: format!("Service error: {e}"),
                        }];
                        CommitOutcome::Reopen {
                            content: editor_templates::render_with_errors(text, &errors),
                        }
                    }
                }
            }
        }
    }
}

#[async_trait]
impl EditSession for TaskEditSession {
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
        match &self.mode {
            Mode::Create => "add",
            Mode::Edit { .. } => "edit",
        }
    }

    async fn commit(&mut self, text: &str) -> CommitOutcome {
        match &self.mode {
            Mode::Create => self.commit_create(text).await,
            Mode::Edit { .. } => self.commit_edit(text).await,
        }
    }

    async fn live_apply(&mut self, text: &str) -> Option<FollowUp> {
        // Drive the same commit path; propagate ReloadTasks so the App
        // notifies + reloads after each `:w`. Errors stay deferred to close.
        let outcome = match &self.mode {
            Mode::Create => self.commit_create(text).await,
            Mode::Edit { .. } => self.commit_edit(text).await,
        };
        match outcome {
            CommitOutcome::FollowUp(fu) => Some(fu),
            _ => None,
        }
    }
}
