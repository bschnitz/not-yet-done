//! Generic edit session for an arbitrary filesystem file.
//!
//! Used by the `:config` flow to edit YAML configs in the same external
//! editor used for task notes / Jira ticket bodies. The session does
//! three things on commit:
//!
//! 1. Strip any previous error banner from the buffer.
//! 2. Validate the buffer as syntactically-valid YAML (cheap parse to
//!    `serde_yaml::Value` — catches indentation/quoting bugs but **not**
//!    schema violations). On failure: [`CommitOutcome::Reopen`] with the
//!    error rendered as a YAML-comment banner at the top.
//! 3. Write the stripped buffer to `path` and emit
//!    [`FollowUp::ReloadConfig`] so the App can re-apply the config
//!    in-process. Reload failure (semantic / schema error) is the App's
//!    problem — it reopens the editor by constructing a fresh
//!    `FileEditSession` via [`FileEditSession::with_error`].
//!
//! The session never reads from `path` at commit time — the editor
//! buffer is the source of truth. Reads only happen at construction
//! ([`FileEditSession::open`]/[`FileEditSession::with_error`]).

use std::path::PathBuf;

use async_trait::async_trait;

use super::{CommitOutcome, EditSession, FollowUp, SessionScope};

const ERROR_BANNER_START: &str = "# ─── ERRORS ───";
const ERROR_BANNER_END: &str = "# ─────────────────";

pub struct FileEditSession {
    path: PathBuf,
    template: String,
    label: String,
}

impl FileEditSession {
    /// Construct a session for `path`. Reads the file from disk into the
    /// template buffer. Returns `Err` only on read failure — callers
    /// surface the I/O error to the user.
    pub fn open(path: PathBuf) -> std::io::Result<Self> {
        let template = std::fs::read_to_string(&path)?;
        let label = label_for(&path);
        Ok(Self {
            path,
            template,
            label,
        })
    }

    /// Like [`Self::open`] but prepends an error banner to the buffer.
    /// Used when a previous reload-attempt failed: the editor reopens
    /// with the latest on-disk content **plus** the error visible at
    /// the top.
    pub fn with_error(path: PathBuf, error: String) -> std::io::Result<Self> {
        let raw = std::fs::read_to_string(&path)?;
        let template = render_with_error(strip_error_banner(&raw), &error);
        let label = label_for(&path);
        Ok(Self {
            path,
            template,
            label,
        })
    }
}

fn label_for(path: &std::path::Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("config");
    format!("edit {stem}")
}

#[async_trait]
impl EditSession for FileEditSession {
    fn template(&self) -> &str {
        &self.template
    }

    fn suffix(&self) -> &str {
        ".yaml"
    }

    fn scope(&self) -> SessionScope {
        // Config edits aren't tied to a specific tab — pin to Tasks so
        // the action bar slot stays consistent. The session itself
        // doesn't care.
        SessionScope::Tasks
    }

    fn label(&self) -> &str {
        &self.label
    }

    async fn commit(&mut self, text: &str) -> CommitOutcome {
        let stripped = strip_error_banner(text).to_string();

        if let Err(e) = serde_yaml::from_str::<serde_yaml::Value>(&stripped) {
            return CommitOutcome::Reopen {
                content: render_with_error(&stripped, &format!("YAML parse: {e}")),
            };
        }

        if let Err(e) = tokio::fs::write(&self.path, &stripped).await {
            return CommitOutcome::Reopen {
                content: render_with_error(&stripped, &format!("Write failed: {e}")),
            };
        }

        CommitOutcome::FollowUp(FollowUp::ReloadConfig {
            path: self.path.clone(),
        })
    }
}

/// Strip a previously rendered error-banner block from the start of
/// `text`. Idempotent: returns `text` unchanged when no banner is
/// present.
pub(crate) fn strip_error_banner(text: &str) -> &str {
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

fn render_with_error(stripped: &str, error: &str) -> String {
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
    fn strip_removes_banner_block() {
        let with_banner =
            format!("{ERROR_BANNER_START}\n# • boom\n{ERROR_BANNER_END}\nkey: value\n");
        assert_eq!(strip_error_banner(&with_banner), "key: value\n");
    }

    #[test]
    fn strip_is_idempotent_when_no_banner() {
        assert_eq!(strip_error_banner("key: value\n"), "key: value\n");
    }

    #[test]
    fn render_then_strip_round_trips() {
        let original = "key: value\n";
        let rendered = render_with_error(original, "line 1\nline 2");
        assert!(rendered.starts_with(ERROR_BANNER_START));
        assert!(rendered.contains("# • line 1"));
        assert!(rendered.contains("# • line 2"));
        assert_eq!(strip_error_banner(&rendered), original);
    }

    #[test]
    fn reopen_round_trip_strips_old_banner_first() {
        // Mirrors what `commit()` does: pass strip_error_banner() output
        // into render_with_error so banners stack to exactly one, never two.
        let with_banner = render_with_error("key: value\n", "first error");
        let with_banner_twice = render_with_error(strip_error_banner(&with_banner), "second error");
        assert_eq!(strip_error_banner(&with_banner_twice), "key: value\n");
    }

    #[tokio::test]
    async fn commit_writes_stripped_buffer_to_disk() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        std::fs::write(&path, "old: content\n").unwrap();

        let mut session = FileEditSession::open(path.clone()).unwrap();
        let banner = render_with_error("new: content\n", "ignore me");
        let outcome = session.commit(&banner).await;
        match outcome {
            CommitOutcome::FollowUp(FollowUp::ReloadConfig { path: p }) => {
                assert_eq!(p, path);
            }
            _ => panic!("expected FollowUp::ReloadConfig"),
        }
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert_eq!(on_disk, "new: content\n");
    }

    #[tokio::test]
    async fn commit_with_invalid_yaml_reopens() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut session = FileEditSession::open(tmp.path().to_path_buf()).unwrap();
        let bad = "key: [unclosed\n";
        let outcome = session.commit(bad).await;
        match outcome {
            CommitOutcome::Reopen { content } => {
                assert!(content.contains(ERROR_BANNER_START));
                assert!(content.contains("YAML parse"));
            }
            _ => panic!("expected Reopen"),
        }
    }

    #[test]
    fn label_uses_file_stem() {
        assert_eq!(
            label_for(std::path::Path::new("/x/y/jira.yaml")),
            "edit jira"
        );
        assert_eq!(label_for(std::path::Path::new("/x/tui.yaml")), "edit tui");
    }
}
