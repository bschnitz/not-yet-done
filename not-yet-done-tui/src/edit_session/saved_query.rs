//! Edit session for adapter-owned saved-query bodies.
//!
//! The body format is adapter-specific — JQL string for Jira, YAML for
//! Taiga, … — and is parsed by the adapter at apply time. The session
//! therefore writes the buffer through without validation; mistakes
//! surface later via `:query apply`. The session emits
//! [`FollowUp::ReloadContentSavedQueries`] so the content view picks up
//! the new body on next apply.
//!
//! Only adapters whose [`not_yet_done_content::SavedQueryStore::path`]
//! returns `Some` are supported (filesystem-backed). Adapters with an
//! opaque store (DB-backed, remote) need a separate text-buffer flow.

use std::path::PathBuf;

use async_trait::async_trait;

use super::{CommitOutcome, EditSession, FollowUp, SessionScope};

pub struct SavedQueryEditSession {
    path: PathBuf,
    view_index: usize,
    name: String,
    template: String,
    label: String,
    /// Editor file suffix, taken from the store the body lives in — the
    /// adapter's query language, or `.md` for an extended document. The
    /// buffer is written to `path` either way; this only decides what the
    /// editor highlights.
    suffix: String,
}

impl SavedQueryEditSession {
    /// Construct a session for an existing saved query. Reads the file
    /// at `path` into the template buffer. Returns `Err` only on read
    /// failure — callers surface the I/O error to the user.
    pub fn open(
        path: PathBuf,
        view_index: usize,
        name: String,
        suffix: String,
    ) -> std::io::Result<Self> {
        let template = std::fs::read_to_string(&path)?;
        let label = format!("edit query: {name}");
        Ok(Self {
            path,
            view_index,
            name,
            template,
            label,
            suffix,
        })
    }

    /// Construct a session for a brand-new saved query. No I/O happens
    /// up front; the file is created on first commit.
    pub fn new(path: PathBuf, view_index: usize, name: String, suffix: String) -> Self {
        let label = format!("new query: {name}");
        Self {
            path,
            view_index,
            name,
            template: String::new(),
            label,
            suffix,
        }
    }
}

#[async_trait]
impl EditSession for SavedQueryEditSession {
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
        if let Some(parent) = self.path.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                return CommitOutcome::Reopen {
                    content: format!(
                        "# mkdir failed: {e}\n# (this comment will not be saved)\n{text}"
                    ),
                };
            }
        }
        if let Err(e) = tokio::fs::write(&self.path, text).await {
            return CommitOutcome::Reopen {
                content: format!("# write failed: {e}\n# (this comment will not be saved)\n{text}"),
            };
        }
        CommitOutcome::FollowUp(FollowUp::ReloadContentSavedQueries {
            view_index: self.view_index,
            message: format!("Saved query '{}'", self.name),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn commit_writes_body_and_emits_followup() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("Foo.yaml");
        let mut session =
            SavedQueryEditSession::new(path.clone(), 7, "Foo".to_string(), ".yaml".to_string());
        let outcome = session.commit("status: open\n").await;
        match outcome {
            CommitOutcome::FollowUp(FollowUp::ReloadContentSavedQueries {
                view_index,
                message,
            }) => {
                assert_eq!(view_index, 7);
                assert!(message.contains("Foo"));
            }
            _ => panic!("expected ReloadContentSavedQueries"),
        }
        let body = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(body, "status: open\n");
    }

    #[tokio::test]
    async fn open_reads_existing_body_into_template() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("Bar.yaml");
        tokio::fs::write(&path, "type: task\n").await.unwrap();
        let session =
            SavedQueryEditSession::open(path, 3, "Bar".to_string(), ".yaml".to_string()).unwrap();
        assert_eq!(session.template(), "type: task\n");
        assert_eq!(session.label(), "edit query: Bar");
        assert_eq!(session.suffix(), ".yaml");
    }
}
