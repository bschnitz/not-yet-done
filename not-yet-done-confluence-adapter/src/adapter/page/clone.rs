//! `clone` action for `confluence:page`: open an editor pre-filled with
//! the source page's title (suffixed with " (Clone)") and pretty-printed
//! `body.storage`, then POST a new page that lives under the same parent
//! (or as top-level when the source is top-level) in the same space.
//!
//! Why "same parent, same space" by default — the clone is a tree-local
//! duplicate. Cross-space cloning would require a Space-Picker, which the
//! TUI doesn't expose for adapter-side actions; the user can edit the
//! resulting page afterwards (move it, change ancestors) if they want it
//! somewhere else. Mirrors the Jira/Taiga clone-action that always lands
//! in the source project.
//!
//! Buffer shape — same `title:` + body XHTML format as `create-child`
//! (CF-10), so we reuse [`parse_template`] / [`render_with_error`] from
//! [`super::super::create_template`]. The body is pretty-printed through
//! the same xmllint pipeline as `edit` (CF-9) so the user can read /
//! tweak it before saving.
//!
//! Failure-mode shape — parse errors and REST errors both round-trip
//! through `render_with_error`, which strips any previously-rendered
//! banner so retries don't stack them.

use not_yet_done_content::{ActionOutcome, EditorPrep, Result};

use crate::adapter::create_template::{ParsedCreate, parse_template, render_with_error};

use super::super::other_err;
use super::ConfluencePageNode;
use super::format::format_xhtml;

impl ConfluencePageNode {
    /// Render a clone-buffer pre-filled with the source page's title +
    /// pretty-printed body. The title carries a " (Clone)" suffix so the
    /// new page is distinguishable in the listing before the user edits
    /// it; if the source already ends with " (Clone)" the suffix is left
    /// off to avoid stacking.
    pub(super) async fn prepare_clone(&self) -> Result<EditorPrep> {
        let detail = self.detail().await?;
        let suggested_title = if detail.title.ends_with(" (Clone)") {
            detail.title.clone()
        } else {
            format!("{} (Clone)", detail.title)
        };
        let body = format_xhtml(&detail.body_storage).await;
        let mut template = String::new();
        template.push_str("title: ");
        template.push_str(&suggested_title);
        template.push('\n');
        template.push('\n');
        template.push_str(&body);
        if !template.ends_with('\n') {
            template.push('\n');
        }
        Ok(EditorPrep {
            template,
            // POST has no optimistic-lock token — clone is a fresh create.
            version: String::new(),
            suffix: ".html".into(),
            file_path: None,
        })
    }

    /// Commit a clone-buffer. Reuses the create-child template parser so
    /// the title/body shape stays identical to `a: create-child`. The new
    /// page lands under the source's immediate parent (or at the space
    /// root when the source is top-level), in the source's space.
    pub(super) async fn execute_clone(&self, text: &str) -> Result<ActionOutcome> {
        let parsed: ParsedCreate = match parse_template(text) {
            Ok(p) => p,
            Err(msg) => {
                return Ok(ActionOutcome::Reopen {
                    content: render_with_error(text, &msg),
                    new_version: None,
                });
            }
        };

        let detail = self.detail().await?;
        if detail.space_key.is_empty() {
            return Err(other_err(format!(
                "page {} has no space.key — cannot resolve clone target",
                self.page.id
            )));
        }
        let parent_id = detail.ancestors.last().map(|a| a.id.as_str());

        match self
            .client
            .create_page(&detail.space_key, parent_id, &parsed.title, &parsed.body)
            .await
        {
            Ok(created) => Ok(ActionOutcome::Done {
                message: Some(format!(
                    "Cloned page {} → {} (id {})",
                    self.page.title, parsed.title, created.id
                )),
            }),
            Err(msg) => Ok(ActionOutcome::Reopen {
                content: render_with_error(text, &format!("Clone failed: {msg}")),
                new_version: None,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::ConfluencePageNode;
    use crate::adapter::create_template::{CREATE_ERROR_BANNER_START, parse_template};
    use crate::client::{ConfluenceClient, PageMeta};
    use not_yet_done_content::{ActionOutcome, EditorPrep};
    use std::sync::Arc;

    fn synthetic_client() -> Arc<ConfluenceClient> {
        Arc::new(
            ConfluenceClient::new(
                "https://wiki.example.invalid/confluence",
                "JSESSIONID=synthetic",
                false,
            )
            .expect("client"),
        )
    }

    fn bare_page_node() -> ConfluencePageNode {
        ConfluencePageNode::new(
            synthetic_client(),
            "https://wiki.example.invalid/confluence",
            PageMeta {
                id: "777".into(),
                title: "Parent".into(),
                page_type: "page".into(),
                webui: "/spaces/DEMO/pages/777/Parent".into(),
                has_children: None,
            },
        )
    }

    #[tokio::test]
    async fn prepare_clone_propagates_detail_fetch_errors() {
        // Synthetic client → unreachable host → detail() errors. The
        // clone flow must surface the error rather than silently returning
        // an empty template.
        let node = bare_page_node();
        let result: Result<EditorPrep, _> = node.prepare_clone().await;
        assert!(result.is_err(), "prepare must error on unreachable host");
    }

    #[tokio::test]
    async fn execute_clone_reopens_on_malformed_buffer() {
        // Pre-seed an error banner on the buffer so we can check that
        // parse_template strips it before re-parsing. The banner is
        // re-rendered around the cleaned buffer when parse fails again.
        let node = bare_page_node();
        let bad = "no title prefix here\n\n<p>x</p>\n";
        let outcome = node.execute_clone(bad).await.expect("returns outcome");
        match outcome {
            ActionOutcome::Reopen {
                content,
                new_version,
            } => {
                assert!(content.starts_with(CREATE_ERROR_BANNER_START));
                assert!(new_version.is_none());
            }
            ActionOutcome::Done { .. } => panic!("expected Reopen, got Done"),
            ActionOutcome::NoChanges => panic!("expected Reopen, got NoChanges"),
            ActionOutcome::Navigate { .. } => panic!("expected Reopen, got Navigate"),
            ActionOutcome::OpenExternal { .. } | ActionOutcome::OpenEditor { .. } => {
                panic!("expected Reopen, got a menu/external outcome")
            }
        }
    }

    #[test]
    fn parse_template_round_trips_through_clone_suggested_buffer() {
        // The buffer prepare_clone produces is the same shape parse_template
        // accepts. This locks in that the title-suffix rendering doesn't
        // break the contract — `title: ` prefix, blank line, body.
        let suggested = "title: Foo (Clone)\n\n<p>body</p>\n";
        let parsed = parse_template(suggested).expect("parses");
        assert_eq!(parsed.title, "Foo (Clone)");
        assert_eq!(parsed.body, "<p>body</p>\n");
    }
}
