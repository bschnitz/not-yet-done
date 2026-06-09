//! Edit session for free-form per-task notes (markdown file alongside the
//! task).
//!
//! No parsing; the buffer is the file content. Empty buffer on commit
//! deletes the notes file; non-empty writes it.

use async_trait::async_trait;

use not_yet_done_core::entity::task::Model as Task;

use crate::notes;

use super::{CommitOutcome, EditSession, FollowUp, SessionScope};

pub struct TaskNotesSession {
    task: Task,
    task_rows: Vec<Task>,
    template: String,
}

impl TaskNotesSession {
    pub fn new(task: Task, task_rows: Vec<Task>) -> Self {
        // Read any existing notes file. Don't pre-create an empty file —
        // the App's cancel detection compares the saved buffer against the
        // initial template, so a zero-byte template + empty save would be
        // mistaken for `:q!` and the placeholder file would never be
        // cleaned up.
        let template = notes::read_notes(&task, &task_rows);
        Self { task, task_rows, template }
    }
}

#[async_trait]
impl EditSession for TaskNotesSession {
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
        "notes"
    }

    async fn commit(&mut self, text: &str) -> CommitOutcome {
        if text.trim().is_empty() {
            notes::delete_notes(&self.task, &self.task_rows);
        } else {
            notes::write_notes(&self.task, &self.task_rows, text);
        }
        CommitOutcome::FollowUp(FollowUp::ReloadTasks {
            focus_id: Some(self.task.id),
            tracking_changed: false,
            message: "Notes saved".into(),
        })
    }
}
