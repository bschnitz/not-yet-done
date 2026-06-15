//! `edit` action for `confluence:page`: open the title + storage-format
//! body in `$EDITOR`, write the new revision via `PUT /content/{id}`, and
//! dispatch to the conflict-merge path on 409.
//!
//! Buffer shape — `title:` line, blank line, then the XHTML body (the
//! shared titled-buffer format from [`create_template`]). The title rides
//! *in the buffer* so a rename lands in the same edit session.
//!
//! Why the title is in the buffer at all — the TUI re-resolves every
//! action target through `ContentAdapter::get_by_id`, which synthesizes a
//! `PageMeta` stub whose `title` is just the page **id** (the listing's
//! real title is discarded on that round-trip). The previous body-only
//! buffer PUT `self.page.title` unchanged, so an edit silently *renamed
//! the page to its own id*. Sourcing the title from the hydrated
//! `detail()` and round-tripping it through the buffer fixes that and
//! makes the title editable in one move.
//!
//! Version handling — the stash is the `version.number` we saw at
//! prepare time, converted to a string so it round-trips through
//! `ActionInput::Edited::version`. The PUT sends `stashed + 1`; on 409
//! the merge path re-fetches and stages a fresh version (so the next
//! retry uses the merged buffer + the latest version).

use not_yet_done_content::{ActionOutcome, ContentError, EditorPrep, Result};

use crate::adapter::create_template::{parse_template, render_filled, render_with_error};
use crate::client::UpdatePageError;

use super::ConfluencePageNode;
use super::format::format_xhtml;
use super::super::conflict_banner::strip_banner;
use super::super::other_err;

impl ConfluencePageNode {
    /// Render the initial editor buffer: lazy-hydrate the page detail,
    /// put the current title on the first line, pretty-print the
    /// `body.storage` value below it, and stash the current version
    /// number for conflict detection on commit.
    pub(super) async fn prepare_edit(&self) -> Result<EditorPrep> {
        let detail = self.detail().await?;
        let body = format_xhtml(&detail.body_storage).await;
        Ok(EditorPrep {
            template: render_filled(&detail.title, &body),
            version: detail.version.to_string(),
            suffix: ".html".into(),
        })
    }

    /// Commit the edited buffer. Parses the title + body, short-circuits
    /// no-op edits, sends the PUT, and routes 409 to the 3-way merge
    /// path. A missing/empty title reopens the editor with a banner.
    pub(super) async fn execute_edit(
        &self,
        text: &str,
        original: &str,
        version: &str,
    ) -> Result<ActionOutcome> {
        let version_num: i64 = version
            .parse()
            .map_err(|e| other_err(format!("invalid page version stash {version:?}: {e}")))?;

        // Strip the conflict banner first (a buffer reopened after a 409
        // carries one above the `title:` line), then parse title + body.
        let (title, body) = match parse_template(strip_banner(text)) {
            Ok(p) => (p.title, p.body),
            Err(msg) => {
                return Ok(ActionOutcome::Reopen {
                    content: render_with_error(text, &msg),
                    new_version: None,
                });
            }
        };
        // The ancestor is the prepare-time template; parse it the same way
        // so the no-op check and the 3-way merge compare body-to-body.
        let (orig_title, orig_body) = match parse_template(strip_banner(original)) {
            Ok(p) => (p.title, p.body),
            // A malformed ancestor can't happen via our own prepare, but
            // be defensive: treat the whole stripped original as the body.
            Err(_) => (title.clone(), strip_banner(original).to_string()),
        };

        if title == orig_title && body == orig_body {
            return Ok(ActionOutcome::NoChanges);
        }

        match self
            .client
            .update_page(self.page.id.as_str(), version_num + 1, &title, &body)
            .await
        {
            Ok(updated) => Ok(ActionOutcome::Done {
                message: Some(format!(
                    "{} updated (version {})",
                    self.page.id, updated.version
                )),
            }),
            Err(UpdatePageError::Conflict(_)) => {
                self.handle_conflict(&title, &orig_title, &body, &orig_body)
                    .await
            }
            Err(UpdatePageError::Other(msg)) => Err(ContentError::Other(msg.into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ConfluencePageNode;
    use crate::adapter::create_template::render_filled;
    use crate::client::{ConfluenceClient, PageMeta};
    use not_yet_done_content::ActionOutcome;
    use std::sync::Arc;

    fn node() -> ConfluencePageNode {
        let client = Arc::new(
            ConfluenceClient::new(
                "https://wiki.example.invalid/confluence",
                "JSESSIONID=synthetic",
                false,
            )
            .expect("client"),
        );
        ConfluencePageNode::new(
            client,
            "https://wiki.example.invalid/confluence",
            PageMeta {
                // The stub `get_by_id` builds: title == id. The fix is that
                // execute_edit must NOT use this title for the PUT.
                id: "42".into(),
                title: "42".into(),
                page_type: "page".into(),
                webui: String::new(),
                has_children: None,
            },
        )
    }

    #[tokio::test]
    async fn no_changes_when_title_and_body_unchanged() {
        let buf = render_filled("Real Title", "<p>body</p>\n");
        let outcome = node()
            .execute_edit(&buf, &buf, "7")
            .await
            .expect("returns outcome");
        assert!(matches!(outcome, ActionOutcome::NoChanges));
    }

    #[tokio::test]
    async fn empty_title_reopens_with_banner_no_network() {
        // An empty title must reopen the editor (a banner), never PUT —
        // so this resolves without touching the unreachable host.
        let original = render_filled("Real Title", "<p>body</p>\n");
        let edited = "title:  \n\n<p>body</p>\n";
        let outcome = node()
            .execute_edit(edited, &original, "7")
            .await
            .expect("returns outcome");
        match outcome {
            ActionOutcome::Reopen { content, .. } => {
                assert!(content.contains("empty"), "banner explains the error");
            }
            _ => panic!("empty title must reopen, not commit"),
        }
    }

    #[tokio::test]
    async fn title_only_rename_is_not_a_no_op() {
        // Body identical, title changed → must attempt the PUT (which
        // fails against the synthetic host) rather than short-circuit as
        // NoChanges. Guards the data-loss regression: the rename has to
        // travel to the server.
        let original = render_filled("Old Name", "<p>body</p>\n");
        let edited = render_filled("New Name", "<p>body</p>\n");
        let outcome = node().execute_edit(&edited, &original, "7").await;
        assert!(
            !matches!(outcome, Ok(ActionOutcome::NoChanges)),
            "a pure rename must not be treated as no-op"
        );
    }
}
