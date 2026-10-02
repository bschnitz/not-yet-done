//! Edit session for a stored editor template of a create action.
//!
//! A template is a buffer in the action's own format (a task's front matter
//! and body, …), so the session writes it through without validation; the
//! adapter judges it when the template is used. It goes through the
//! adapter's [`TemplateStore`] rather than a file path, so a store that is
//! not a directory works the same.

use async_trait::async_trait;
use not_yet_done_content::TemplateStore;

use super::{CommitOutcome, EditSession, SessionScope};

pub struct TemplateEditSession {
    store: Box<dyn TemplateStore>,
    name: String,
    template: String,
    label: String,
    suffix: String,
}

impl TemplateEditSession {
    /// A session over the template `name`, opened on `template` — the
    /// stored body, or the adapter's plain buffer for a new one.
    pub fn new(
        store: Box<dyn TemplateStore>,
        name: String,
        template: String,
        suffix: String,
        is_new: bool,
    ) -> Self {
        let label = if is_new {
            format!("new template: {name}")
        } else {
            format!("edit template: {name}")
        };
        Self {
            store,
            name,
            template,
            label,
            suffix,
        }
    }
}

#[async_trait]
impl EditSession for TemplateEditSession {
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
        &self.label
    }

    async fn commit(&mut self, text: &str) -> CommitOutcome {
        match self.store.save(&self.name, text).await {
            Ok(()) => CommitOutcome::Done {
                message: Some(format!("Saved template '{}'", self.name)),
            },
            Err(e) => CommitOutcome::Reopen {
                content: format!("# save failed: {e}\n# (this comment will not be saved)\n{text}"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_content::FsQueryStore;

    #[tokio::test]
    async fn commit_saves_through_the_store() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("templates").join("add");
        let store = FsQueryStore::new(dir.clone(), ".md");
        let mut session = TemplateEditSession::new(
            Box::new(store),
            "todo".to_string(),
            "tags:\n".to_string(),
            ".md".to_string(),
            true,
        );
        assert_eq!(session.label(), "new template: todo");
        match session.commit("tags: todo\n").await {
            CommitOutcome::Done { message } => assert!(message.unwrap().contains("todo")),
            _ => panic!("expected Done"),
        }
        let body = std::fs::read_to_string(dir.join("todo.md")).unwrap();
        assert_eq!(body, "tags: todo\n");
    }
}
