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

use crate::adapter::create_template::render_filled;
use crate::client::UpdatePageError;

use super::ConfluencePageNode;
use super::format::format_xhtml;
use super::super::conflict_banner::{CONFLICT_BANNER_END, CONFLICT_BANNER_START, strip_banner};
use super::super::other_err;

/// Prepend the conflict banner to a body buffer that already contains
/// diffy's `<<<<<<< ours` / `>>>>>>> theirs` markers, re-attaching the
/// `title:` line above the body so the reopened buffer round-trips
/// through `parse_template` on the next save.
pub(super) fn render_conflict_banner(title: &str, body: &str) -> String {
    let mut out = String::new();
    out.push_str(CONFLICT_BANNER_START);
    out.push('\n');
    out.push_str("    Page was modified upstream while you were editing.\n");
    out.push_str("    Disjoint changes were merged automatically. Each remaining\n");
    out.push_str("    `<<<<<<< ours` … `>>>>>>> theirs` block marks an overlapping\n");
    out.push_str("    change — keep one side and remove the markers, then save.\n");
    out.push_str(CONFLICT_BANNER_END);
    out.push('\n');
    out.push_str(&render_filled(title, strip_banner(body)));
    out
}

impl ConfluencePageNode {
    /// Re-fetch upstream, run a 3-way merge against the user's buffer,
    /// and either retry the PUT (disjoint changes) or return a
    /// `Reopen` with conflict markers (overlapping changes).
    ///
    /// Operates on **body** text only — the title is metadata that rides
    /// above the merge region. The auto-merge PUT keeps the user's title
    /// when they renamed (`user_title != orig_title`); otherwise it adopts
    /// the fresh upstream title so a concurrent rename isn't clobbered.
    pub(super) async fn handle_conflict(
        &self,
        user_title: &str,
        orig_title: &str,
        user_body: &str,
        orig_body: &str,
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
                content.push_str(&render_filled(user_title, strip_banner(user_body)));
                return Ok(ActionOutcome::Reopen {
                    content,
                    new_version: None,
                });
            }
        };

        // A title the user changed wins; otherwise take the fresh upstream
        // title so a concurrent rename survives the merge.
        let merged_title = if user_title == orig_title {
            fresh.title.clone()
        } else {
            user_title.to_string()
        };

        let fresh_formatted = format_xhtml(&fresh.body_storage).await;
        let ancestor = strip_banner(orig_body);
        let ours = strip_banner(user_body);

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
                        &merged_title,
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
                        content: render_conflict_banner(&merged_title, &clean),
                        new_version: Some((fresh.version + 1).to_string()),
                    }),
                    Err(UpdatePageError::Other(msg)) => Err(other_err(msg)),
                }
            }
            Err(marked) => Ok(ActionOutcome::Reopen {
                content: render_conflict_banner(&merged_title, &marked),
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
    fn conflict_banner_round_trips_through_strip_and_parse() {
        // render_conflict_banner re-attaches the `title:` line above the
        // body; after stripping the banner the buffer parses back into
        // exactly the title + body it was built from.
        use crate::adapter::create_template::parse_template;
        let rendered = render_conflict_banner("My Page", "<p>x</p>");
        let parsed = parse_template(strip_banner(&rendered)).expect("parses");
        assert_eq!(parsed.title, "My Page");
        assert_eq!(parsed.body, "<p>x</p>");
    }
}
