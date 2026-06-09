//! `edit` action for `confluence:page`: open the storage-format body in
//! `$EDITOR`, write the new revision via `PUT /content/{id}`, and
//! dispatch to the conflict-merge path on 409.
//!
//! Buffer shape — body-only. The page's title isn't touched; rename has
//! to land as a separate action so the edit buffer stays a single
//! continuous XHTML fragment (otherwise the user's editor wastes
//! syntax-highlighter cycles parsing a YAML-ish header). The title from
//! the cached `PageMeta` rides through the PUT unchanged.
//!
//! Version handling — the stash is the `version.number` we saw at
//! prepare time, converted to a string so it round-trips through
//! `ActionInput::Edited::version`. The PUT sends `stashed + 1`; on 409
//! the merge path re-fetches and stages a fresh version (so the next
//! retry uses the merged buffer + the latest version).

use not_yet_done_content::{ActionOutcome, ContentError, EditorPrep, Result};

use crate::client::UpdatePageError;

use super::ConfluencePageNode;
use super::format::format_xhtml;
use super::super::conflict_banner::strip_banner;
use super::super::other_err;

impl ConfluencePageNode {
    /// Render the initial editor buffer: lazy-hydrate the page detail,
    /// pretty-print the `body.storage` value, and stash the current
    /// version number for conflict detection on commit.
    pub(super) async fn prepare_edit(&self) -> Result<EditorPrep> {
        let detail = self.detail().await?;
        let template = format_xhtml(&detail.body_storage).await;
        Ok(EditorPrep {
            template,
            version: detail.version.to_string(),
            suffix: ".html".into(),
        })
    }

    /// Commit the edited buffer. Parses the stashed version, short-
    /// circuits no-op edits, sends the PUT, and routes 409 to the
    /// 3-way merge path.
    pub(super) async fn execute_edit(
        &self,
        text: &str,
        original: &str,
        version: &str,
    ) -> Result<ActionOutcome> {
        let version_num: i64 = version
            .parse()
            .map_err(|e| other_err(format!("invalid page version stash {version:?}: {e}")))?;

        let user = strip_banner(text);
        let ancestor = strip_banner(original);

        if user == ancestor {
            return Ok(ActionOutcome::NoChanges);
        }

        // PageMeta's title is what we saw at list-time. The conf-edit
        // reference script passes the title through unchanged on PUT —
        // same approach here. If somebody renamed the page upstream the
        // 409 path will pull the fresh title before merging.
        match self
            .client
            .update_page(
                self.page.id.as_str(),
                version_num + 1,
                &self.page.title,
                user,
            )
            .await
        {
            Ok(updated) => Ok(ActionOutcome::Done {
                message: Some(format!(
                    "{} updated (version {})",
                    self.page.id, updated.version
                )),
            }),
            Err(UpdatePageError::Conflict(_)) => self.handle_conflict(user, ancestor).await,
            Err(UpdatePageError::Other(msg)) => Err(ContentError::Other(msg.into())),
        }
    }
}
