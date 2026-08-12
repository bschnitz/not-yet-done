//! Edit session for a script file (Python or other executable). Writes
//! the buffer under the configured scripts directory and sets executable
//! bits on Unix. Used by the `:script` fuzzy menu for both the Trackings
//! tab (`SessionScope::Trackings`) and content-view nodes
//! (`SessionScope::Content`); the embedder picks the directory + scope.

use async_trait::async_trait;

use super::{CommitOutcome, EditSession, SessionScope};

pub struct ScriptSession {
    scripts_dir: std::path::PathBuf,
    filename: String,
    template: String,
    suffix: String,
    scope: SessionScope,
    label: String,
}

impl ScriptSession {
    pub fn new(
        scripts_dir: std::path::PathBuf,
        filename: String,
        template: String,
        scope: SessionScope,
        label: impl Into<String>,
    ) -> Self {
        let suffix = filename
            .rfind('.')
            .map(|i| filename[i..].to_string())
            .unwrap_or_else(|| ".py".to_string());
        Self {
            scripts_dir,
            filename,
            template,
            suffix,
            scope,
            label: label.into(),
        }
    }
}

#[async_trait]
impl EditSession for ScriptSession {
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
        &self.label
    }

    async fn commit(&mut self, text: &str) -> CommitOutcome {
        if let Err(e) = std::fs::create_dir_all(&self.scripts_dir) {
            return CommitOutcome::Cancelled {
                message: Some(format!("Failed to create scripts dir: {e}")),
            };
        }
        let path = self.scripts_dir.join(&self.filename);
        match std::fs::write(&path, text) {
            Ok(_) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
                }
                CommitOutcome::Done {
                    message: Some(format!("Script saved: {}", path.display())),
                }
            }
            Err(e) => CommitOutcome::Cancelled {
                message: Some(format!("Failed to save script: {e}")),
            },
        }
    }
}
