//! 3-way merge for page edits when the server returns 409 (the page was
//! modified upstream between the editor opening and our PUT going out).
//!
//! Approach lifted from `jira-adapter::adapter::issue::merge`: diffy
//! merges the three buffers (ancestor / ours / theirs) line-by-line.
//! Disjoint edits collapse into a single auto-merged buffer that we
//! retry the PUT against; overlapping edits come back as `Reopen` with
//! git-style `<<<<<<< ours` / `>>>>>>> theirs` markers above the
//! conflicting region and a banner explaining what to do.

use not_yet_done_content::{ActionOutcome, Result};

use crate::client::UpdatePageError;

use super::ConfluencePageNode;
use super::format::format_xhtml;
use super::super::conflict_banner::{CONFLICT_BANNER_END, CONFLICT_BANNER_START, strip_banner};
use super::super::other_err;

/// Prepend the conflict banner to a buffer that already contains diffy's
/// `<<<<<<< ours` / `>>>>>>> theirs` markers.
pub(super) fn render_conflict_banner(text: &str) -> String {
    let mut out = String::new();
    out.push_str(CONFLICT_BANNER_START);
    out.push('\n');
    out.push_str("    Page was modified upstream while you were editing.\n");
    out.push_str("    Disjoint changes were merged automatically. Each remaining\n");
    out.push_str("    `<<<<<<< ours` … `>>>>>>> theirs` block marks an overlapping\n");
    out.push_str("    change — keep one side and remove the markers, then save.\n");
    out.push_str(CONFLICT_BANNER_END);
    out.push('\n');
    out.push_str(strip_banner(text));
    out
}

impl ConfluencePageNode {
    /// Re-fetch upstream, run a 3-way merge against the user's buffer,
    /// and either retry the PUT (disjoint changes) or return a
    /// `Reopen` with conflict markers (overlapping changes).
    pub(super) async fn handle_conflict(
        &self,
        user_text: &str,
        original_text: &str,
    ) -> Result<ActionOutcome> {
        // Fresh fetch bypasses the OnceCell — we *want* the latest
        // upstream state for the diff. The OnceCell is for the lazy
        // preview path; the edit flow always pulls fresh.
        let fresh = match self.client.get_page(self.page.id.as_str()).await {
            Ok(d) => d,
            Err(e) => {
                // Can't re-fetch — surface a banner, return the user's
                // text as-is so they can decide what to do next.
                let mut content = String::new();
                content.push_str(CONFLICT_BANNER_START);
                content.push('\n');
                content.push_str("    Page was modified upstream and the fresh state could not be re-fetched.\n");
                content.push_str(&format!("    Error: {e}\n"));
                content.push_str("    Save again to overwrite, or Esc to cancel.\n");
                content.push_str(CONFLICT_BANNER_END);
                content.push('\n');
                content.push_str(strip_banner(user_text));
                return Ok(ActionOutcome::Reopen {
                    content,
                    new_version: None,
                });
            }
        };

        let fresh_formatted = format_xhtml(&fresh.body_storage).await;
        let ancestor = strip_banner(original_text);
        let ours = strip_banner(user_text);

        let mut opts = diffy::MergeOptions::new();
        opts.set_conflict_style(diffy::ConflictStyle::Merge);
        match opts.merge(ancestor, ours, &fresh_formatted) {
            Ok(clean) => {
                // Disjoint edits — write the merged buffer through. If
                // *that* PUT also conflicts (another concurrent change)
                // we bail out with the merged text + a banner; the user
                // can retry manually.
                match self
                    .client
                    .update_page(
                        self.page.id.as_str(),
                        fresh.version + 1,
                        &fresh.title,
                        &clean,
                    )
                    .await
                {
                    Ok(updated) => Ok(ActionOutcome::Done {
                        message: Some(format!(
                            "{} updated (auto-merged, version {})",
                            self.page.id, updated.version
                        )),
                    }),
                    Err(UpdatePageError::Conflict(_)) => Ok(ActionOutcome::Reopen {
                        content: render_conflict_banner(&clean),
                        new_version: Some((fresh.version + 1).to_string()),
                    }),
                    Err(UpdatePageError::Other(msg)) => Err(other_err(msg)),
                }
            }
            Err(marked) => Ok(ActionOutcome::Reopen {
                content: render_conflict_banner(&marked),
                new_version: Some(fresh.version.to_string()),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_banner_removes_full_block() {
        let with_banner = format!(
            "{CONFLICT_BANNER_START}\n    a\n    b\n{CONFLICT_BANNER_END}\n<p>body</p>"
        );
        assert_eq!(strip_banner(&with_banner), "<p>body</p>");
    }

    #[test]
    fn strip_banner_leaves_text_alone_without_marker() {
        assert_eq!(strip_banner("<p>body</p>"), "<p>body</p>");
    }

    #[test]
    fn strip_banner_idempotent_after_render() {
        let rendered = render_conflict_banner("<p>x</p>");
        let stripped = strip_banner(&rendered);
        assert_eq!(stripped, "<p>x</p>");
    }
}
