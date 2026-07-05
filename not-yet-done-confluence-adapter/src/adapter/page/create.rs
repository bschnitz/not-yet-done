//! `create-child` action for `confluence:page`: open a tiny editor
//! buffer (`title:` + empty `<p></p>` body), POST a new page with the
//! parent set to this page, and surface the new page id via
//! [`ActionOutcome::Done`] (the TUI's `ReloadContentPane` follow-up
//! refreshes the listing so the new row appears).
//!
//! Space-key resolution — Confluence requires `space.key` on the POST
//! even when `ancestors` is set, but the page node only carries
//! `PageMeta` (id + title + type + webui). [`detail()`] hydrates the
//! full record (CF-5) which includes `space.key`, so the prepare call
//! triggers the same lazy fetch the preview path uses.
//!
//! Failure-mode shape — parse errors reopen the editor with a banner
//! above the buffer (so the user can fix the title line and save
//! again). REST errors go through the same banner path with the server
//! message in the body — same UX as the conflict-merge banner in CF-9.

use not_yet_done_content::{ActionOutcome, EditorPrep, Result};

use crate::adapter::create_template::{
    ParsedCreate, parse_template, render_template, render_with_error,
};

use super::ConfluencePageNode;
use super::super::other_err;

impl ConfluencePageNode {
    /// Open a fresh create-child buffer. No version stash — POST has no
    /// optimistic-lock token.
    pub(super) async fn prepare_create_child(&self) -> Result<EditorPrep> {
        // Touch detail() so the subsequent execute call already has
        // `space_key` cached — saves the second roundtrip on save.
        let _ = self.detail().await?;
        Ok(EditorPrep {
            template: render_template(),
            version: String::new(),
            suffix: ".html".into(),
        })
    }

    /// Commit a create-child buffer. POST runs against the cached
    /// `space.key` from `detail()`; parent id is the current page.
    pub(super) async fn execute_create_child(&self, text: &str) -> Result<ActionOutcome> {
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
                "page {} has no space.key — cannot resolve create-child target",
                self.page.id
            )));
        }

        match self
            .client
            .create_page(
                &detail.space_key,
                Some(self.page.id.as_str()),
                &parsed.title,
                &parsed.body,
            )
            .await
        {
            Ok(created) => Ok(ActionOutcome::Done {
                message: Some(format!(
                    "Created child page {} (id {})",
                    parsed.title, created.id
                )),
            }),
            Err(msg) => Ok(ActionOutcome::Reopen {
                content: render_with_error(text, &format!("Create failed: {msg}")),
                new_version: None,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::ConfluencePageNode;
    use crate::adapter::create_template::{
        CREATE_ERROR_BANNER_START, parse_template, render_with_error,
    };
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
    async fn execute_create_child_reopens_on_missing_title_prefix() {
        // Pre-seed an error banner on the buffer so we can check that
        // parse_template strips it before re-parsing. The banner is
        // re-rendered around the cleaned buffer when parse fails again.
        let node = bare_page_node();
        let bad = "no title prefix here\n\n<p>x</p>\n";
        let outcome = node.execute_create_child(bad).await.expect("returns outcome");
        match outcome {
            ActionOutcome::Reopen { content, new_version } => {
                assert!(content.starts_with(CREATE_ERROR_BANNER_START));
                assert!(new_version.is_none());
            }
            other => panic!("expected Reopen, got {:?}", debug_variant(&other)),
        }
    }

    #[test]
    fn render_with_error_preserves_user_buffer_after_strip() {
        let buf = "title: Mine\n\n<p>body</p>\n";
        let with_banner = render_with_error(buf, "boom");
        // Re-parsing the banner-wrapped buffer must still succeed —
        // strip_error_banner + parse_template is the round-trip path.
        let parsed = parse_template(&with_banner).expect("strips banner cleanly");
        assert_eq!(parsed.title, "Mine");
    }

    /// Stringify enum variant name without depending on Debug — used in
    /// test failure messages only.
    fn debug_variant(outcome: &ActionOutcome) -> &'static str {
        match outcome {
            ActionOutcome::Done { .. } => "Done",
            ActionOutcome::Reopen { .. } => "Reopen",
            ActionOutcome::NoChanges => "NoChanges",
            ActionOutcome::Navigate { .. } => "Navigate",
            ActionOutcome::OpenExternal { .. } => "OpenExternal",
            ActionOutcome::OpenEditor { .. } => "OpenEditor",
        }
    }

    #[tokio::test]
    async fn prepare_create_child_propagates_detail_fetch_errors() {
        // Synthetic client → unreachable host → detail() errors. The
        // create flow must surface the error rather than silently
        // returning an empty template.
        let node = bare_page_node();
        let result: Result<EditorPrep, _> = node.prepare_create_child().await;
        assert!(result.is_err(), "prepare must error on unreachable host");
    }
}
