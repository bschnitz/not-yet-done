//! `add-comment` action for `confluence:page`: open a tiny editor with
//! an empty XHTML body, POST a new comment whose `container` points at
//! this page, and surface the new comment id via [`ActionOutcome::Done`]
//! (the editor's `FollowUp::ReloadContentPane` refreshes the listing so
//! the new comment row appears under the page's `comments` branch).
//!
//! Buffer shape — body-only XHTML. Comments have no user-editable title
//! (Confluence auto-generates `Re: <page title>` server-side), so the
//! buffer is just the storage-format body. The template is a single
//! empty paragraph so the user lands inside writeable XHTML without
//! having to figure out the format from scratch.
//!
//! Parse failures route to a banner-decorated reopen (same banner
//! markers as the conflict-merge flow via [`super::super::conflict_banner`])
//! so repeated submits don't stack banners.

use not_yet_done_content::{ActionOutcome, EditorPrep, Result};

use super::super::conflict_banner::{CONFLICT_BANNER_END, CONFLICT_BANNER_START, strip_banner};
use super::ConfluencePageNode;

/// Initial editor template — single empty paragraph. Confluence rejects
/// an empty body string on POST, so the template seeds something the
/// user can build on without immediately tripping the validator.
const COMMENT_TEMPLATE: &str = "<p></p>\n";

impl ConfluencePageNode {
    /// Open a fresh add-comment buffer. No version stash — POST has no
    /// optimistic-lock token; the buffer is body-only XHTML.
    pub(super) async fn prepare_add_comment(&self) -> Result<EditorPrep> {
        Ok(EditorPrep {
            template: COMMENT_TEMPLATE.to_string(),
            version: String::new(),
            suffix: ".html".into(),
            file_path: None,
        })
    }

    /// Commit an add-comment buffer. Empty-body short-circuits to a
    /// reopen with banner (Confluence's POST would 400 anyway, but the
    /// banner is friendlier than a propagated REST error). REST errors
    /// also surface through the banner path so the user can fix their
    /// buffer and retry without losing it.
    pub(super) async fn execute_add_comment(&self, text: &str) -> Result<ActionOutcome> {
        let body = strip_banner(text);
        if body.trim().is_empty() {
            return Ok(ActionOutcome::Reopen {
                content: render_comment_create_banner(text, "Comment body must not be empty."),
                new_version: None,
            });
        }
        match self.client.create_comment(&self.page.id, body).await {
            Ok(comment) => Ok(ActionOutcome::Done {
                message: Some(format!(
                    "Comment posted on page {} (id {})",
                    self.page.id, comment.id
                )),
            }),
            Err(msg) => Ok(ActionOutcome::Reopen {
                content: render_comment_create_banner(text, &format!("Create failed: {msg}")),
                new_version: None,
            }),
        }
    }
}

/// Stable wording shared between the empty-body short-circuit and the
/// REST-error path. The banner markers come from the shared module so
/// the strip helper handles both flows.
fn render_comment_create_banner(text: &str, message: &str) -> String {
    let mut out = String::new();
    out.push_str(CONFLICT_BANNER_START);
    out.push('\n');
    for line in message.lines() {
        out.push_str("    ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(CONFLICT_BANNER_END);
    out.push('\n');
    out.push_str(strip_banner(text));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{ConfluenceClient, PageMeta};
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
    async fn add_comment_template_is_a_single_empty_paragraph() {
        let node = bare_page_node();
        let prep = node.prepare_add_comment().await.expect("prep");
        assert_eq!(prep.template, "<p></p>\n");
        assert!(prep.version.is_empty());
        assert_eq!(prep.suffix, ".html");
    }

    #[tokio::test]
    async fn add_comment_reopens_on_empty_body() {
        // Pure whitespace must be treated as empty — the banner path
        // explains why before the user re-saves.
        let node = bare_page_node();
        let outcome = node
            .execute_add_comment("   \n\n")
            .await
            .expect("returns outcome");
        match outcome {
            ActionOutcome::Reopen { content, .. } => {
                assert!(content.starts_with(CONFLICT_BANNER_START));
                assert!(content.contains("must not be empty"));
            }
            ActionOutcome::Done { .. } => panic!("expected Reopen for empty body, got Done"),
            ActionOutcome::NoChanges => panic!("expected Reopen for empty body, got NoChanges"),
            ActionOutcome::Navigate { .. } => {
                panic!("expected Reopen for empty body, got Navigate")
            }
            ActionOutcome::OpenExternal { .. } | ActionOutcome::OpenEditor { .. } => {
                panic!("expected Reopen for empty body, got a menu/external outcome")
            }
        }
    }

    #[test]
    fn create_banner_round_trips_through_strip() {
        let with = render_comment_create_banner("<p>hi</p>\n", "oops");
        assert_eq!(strip_banner(&with), "<p>hi</p>\n");
    }
}
