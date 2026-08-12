//! `confluence:comment` node — CRUD leaf (CF-7 read + CF-12 edit/delete).
//!
//! Lists carry author / created / body (raw XHTML) / version metadata.
//! Body is exposed via the [`Content`] trait so the preview pane renders
//! the full comment text on toggle (in-memory — bodies arrive on the
//! list response with `expand=body.storage,version`).
//!
//! CF-12 adds two actions:
//! - `edit` (`e`, `InputSpec::Editor`) — opens the body XHTML in
//!   `$EDITOR`, PUT-writes a new revision via
//!   [`ConfluenceClient::update_comment`]. On 409 (server moved
//!   underneath us) the editor re-opens with a small banner. No 3-way
//!   merge — comments are small enough that hand-re-editing is cheaper
//!   than wiring up `diffy` for them.
//! - `delete` (`D`, `InputSpec::None`) — routes through the generic
//!   `ConfirmDeleteContentNode` path (CF-11) via
//!   [`ActionDispatch::DeleteSelf`]; the actual DELETE fires from
//!   `execute("delete", ActionInput::None)`.
//!
//! Composite id follows the attachment pattern:
//! `<page_id>/comment/<comment_id>`. When a comment is reached via
//! `get_child` (e.g. from a stored link) the constructor stubs out
//! body/version/author with empties; [`detail`] refetches the real
//! payload from `/rest/api/content/{comment_id}` on first need.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::OnceCell;

use not_yet_done_content::*;

use crate::client::{CommentMeta, ConfluenceClient, UpdatePageError};

use super::conflict_banner::{CONFLICT_BANNER_END, CONFLICT_BANNER_START, strip_banner};
use super::other_err;
use super::page::format::format_xhtml;

pub(super) fn comment_node_type() -> NodeType {
    NodeType {
        type_id: "confluence:comment".into(),
        // `body.storage` is XHTML (same as page body) — `"html"` is
        // the closest stock syntax-highlighter mapping.
        mime_type: "text/html".into(),
        syntax: Some("html".into()),
        file_extension: ".html".into(),
        display_name: "Confluence Comment".into(),
    }
}

/// Static action set for `confluence:comment`. Both actions follow the
/// destructive-vs-edit convention: `e` opens an editor, `D` (Shift+D)
/// routes through the TUI's confirm-popup path. The function is also
/// the source of truth for the adapter's `actions_for_type`, so shortcut
/// hints are stable without instantiating a comment node first.
pub(super) fn comment_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new("edit", "edit", InputSpec::Editor),
        // Capital `D` matches CF-11's destructive convention — lowercase
        // `d` stays reserved for non-destructive operations (e.g. the
        // attachment download key).
        NodeAction::new("delete", "delete", InputSpec::None),
    ]
}

pub(super) struct ConfluenceCommentNode {
    client: Arc<ConfluenceClient>,
    comment: CommentMeta,
    /// Composite id for `get_by_id` round-trips: `{page_id}/comment/{comment_id}`.
    composite_id: String,
    /// Page id the comment hangs off — kept for the eventual edit-flow
    /// (when we need to refresh the parent page-listing).
    #[allow(dead_code)]
    page_id: String,
    cached_metadata: Metadata,
    /// Lazy-fetched full comment record. Populated on first call to
    /// [`detail`] — listings pre-populate `comment` with body+version, so
    /// the OnceCell only ever fires when the node was synthesized via
    /// `get_child` (link navigation) and an action needs fresh data.
    detail: OnceCell<CommentMeta>,
}

impl ConfluenceCommentNode {
    pub(super) fn new(client: Arc<ConfluenceClient>, comment: CommentMeta, page_id: &str) -> Self {
        let composite_id = format!("{}/comment/{}", page_id, comment.id);
        let cached_metadata = Metadata {
            fields: vec![
                MetadataField {
                    key: "author".into(),
                    value: comment.author.clone(),
                    display_label: "Author".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "created".into(),
                    value: comment.created.clone(),
                    display_label: "Created".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "body".into(),
                    value: comment.body_storage.clone(),
                    display_label: "Body".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "page".into(),
                    value: page_id.to_string(),
                    display_label: "Page".into(),
                    editable: false,
                    allowed_values: None,
                },
            ],
        };
        Self {
            client,
            comment,
            composite_id,
            page_id: page_id.to_string(),
            cached_metadata,
            detail: OnceCell::new(),
        }
    }

    /// Resolve the comment's full record. If the constructor received a
    /// list-time payload (body+version both populated), the OnceCell is
    /// seeded with `self.comment` verbatim — no round-trip. Otherwise we
    /// fetch via `GET /rest/api/content/{id}` and cache the result.
    async fn detail(&self) -> Result<&CommentMeta> {
        self.detail
            .get_or_try_init(|| async {
                if !self.comment.body_storage.is_empty() || self.comment.version_number > 0 {
                    Ok::<_, ContentError>(self.comment.clone())
                } else {
                    self.client
                        .get_comment(&self.comment.id)
                        .await
                        .map_err(other_err)
                }
            })
            .await
    }

    /// Render the initial edit-buffer: pretty-print the body XHTML and
    /// stash the current version number for optimistic-lock enforcement
    /// on commit.
    async fn prepare_edit(&self) -> Result<EditorPrep> {
        let detail = self.detail().await?;
        let template = format_xhtml(&detail.body_storage).await;
        Ok(EditorPrep {
            template,
            version: detail.version_number.to_string(),
            suffix: ".html".into(),
            file_path: None,
        })
    }

    /// Commit an edited buffer. Parses the stashed version, short-
    /// circuits no-op edits, and PUTs the new revision. On 409 we
    /// re-open the editor with a banner so the user can manually retry
    /// (no 3-way merge for comments — they're small enough that the
    /// occasional rewrite is cheaper than wiring up `diffy`).
    async fn execute_edit(
        &self,
        text: &str,
        original: &str,
        version: &str,
    ) -> Result<ActionOutcome> {
        let version_num: i64 = version
            .parse()
            .map_err(|e| other_err(format!("invalid comment version stash {version:?}: {e}")))?;

        let user = strip_banner(text);
        let ancestor = strip_banner(original);
        if user == ancestor {
            return Ok(ActionOutcome::NoChanges);
        }

        let detail = self.detail().await?;
        match self
            .client
            .update_comment(&self.comment.id, version_num + 1, &detail.title, user)
            .await
        {
            Ok(new_version) => Ok(ActionOutcome::Done {
                message: Some(format!(
                    "Comment {} updated (version {new_version})",
                    self.comment.id
                )),
            }),
            Err(UpdatePageError::Conflict(_)) => Ok(ActionOutcome::Reopen {
                content: render_comment_conflict_banner(user),
                new_version: None,
            }),
            Err(UpdatePageError::Other(msg)) => Err(ContentError::Other(msg.into())),
        }
    }

    /// CF-12: hard-delete the comment via the client. Confirmation
    /// already happened TUI-side (generic `ConfirmDeleteContentNode`
    /// from CF-11) by the time this runs.
    async fn execute_delete(&self) -> Result<ActionOutcome> {
        self.client
            .delete_comment(&self.comment.id)
            .await
            .map_err(other_err)?;
        Ok(ActionOutcome::Done {
            message: Some(format!("Comment {} deleted", self.comment.id)),
        })
    }
}

/// Prepend a small banner above the user's buffer when a 409 came back
/// from the PUT. Reuses the shared banner markers so the strip helper
/// in [`super::conflict_banner`] removes them cleanly on next save.
fn render_comment_conflict_banner(text: &str) -> String {
    let mut out = String::new();
    out.push_str(CONFLICT_BANNER_START);
    out.push('\n');
    out.push_str("    Comment was modified upstream while you were editing.\n");
    out.push_str("    Re-edit and save to overwrite, or Esc to cancel.\n");
    out.push_str(CONFLICT_BANNER_END);
    out.push('\n');
    out.push_str(text);
    out
}

#[async_trait]
impl Node for ConfluenceCommentNode {
    fn id(&self) -> &str {
        &self.composite_id
    }

    fn label(&self) -> &str {
        // Confluence auto-generates `Re: <page title>` for the title. It's
        // not the most informative label, but it's the only per-row
        // pre-formatted string the wire payload carries — the body itself
        // is XHTML and would need stripping for in-table display.
        &self.comment.title
    }

    fn node_type(&self) -> &NodeType {
        static T: std::sync::LazyLock<NodeType> = std::sync::LazyLock::new(comment_node_type);
        &T
    }

    fn metadata(&self) -> &Metadata {
        &self.cached_metadata
    }
    fn content(&self) -> Option<&dyn Content> {
        Some(self)
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        Err(ContentError::NotFound(format!("No child: {id}")))
    }

    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        match action_id {
            "edit" => self.prepare_edit().await,
            other => Err(ContentError::NotSupported(format!(
                "ConfluenceCommentNode prepare: unknown action {other}"
            ))),
        }
    }

    /// Route the `delete` shortcut through the TUI's generic
    /// confirm-popup pipeline (CF-11). Every other action either has its
    /// own pipeline (editor for `edit`) or no shortcut wired to it, so
    /// they fall through to [`ActionDispatch::Noop`].
    async fn invoke_action(&self, name: &str, _ctx: &ActionContext) -> Result<ActionDispatch> {
        match name {
            "delete" => Ok(ActionDispatch::DeleteSelf { confirm: None }),
            _ => Ok(ActionDispatch::Noop),
        }
    }

    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        match (action_id, input) {
            (
                "edit",
                ActionInput::Edited {
                    text,
                    original,
                    version,
                },
            ) => self.execute_edit(&text, &original, &version).await,
            ("delete", ActionInput::None) => self.execute_delete().await,
            (id, _) => Err(ContentError::NotSupported(format!(
                "ConfluenceCommentNode action `{id}` not supported"
            ))),
        }
    }
}

#[async_trait]
impl Content for ConfluenceCommentNode {
    fn node_type(&self) -> &NodeType {
        static T: std::sync::LazyLock<NodeType> = std::sync::LazyLock::new(comment_node_type);
        &T
    }

    /// No version stash on the preview path — `read()` just hands back
    /// the cached body bytes. The edit path stashes version via
    /// `EditorPrep.version` instead.
    fn version(&self) -> Option<&str> {
        None
    }

    async fn read(&self) -> Result<Vec<u8>> {
        // Body is already in memory — no extra round-trip needed because
        // `list_comments` requests `expand=body.storage,version` on the
        // list endpoint itself.
        Ok(self.comment.body_storage.as_bytes().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn sample_comment() -> CommentMeta {
        CommentMeta {
            id: "c1001".into(),
            title: "Re: Design Doc".into(),
            body_storage: "<p>Looks good to me.</p>".into(),
            author: "Bob Example".into(),
            created: "2026-05-15T14:22:00.000Z".into(),
            version_number: 3,
        }
    }

    #[test]
    fn comment_metadata_carries_all_fields() {
        let node = ConfluenceCommentNode::new(synthetic_client(), sample_comment(), "12345");
        assert_eq!(node.id(), "12345/comment/c1001");
        assert_eq!(node.label(), "Re: Design Doc");

        let meta = node.metadata();
        assert_eq!(meta.fields.len(), 4);
        assert_eq!(meta.fields[0].key, "author");
        assert_eq!(meta.fields[0].value, "Bob Example");
        assert_eq!(meta.fields[1].key, "created");
        assert_eq!(meta.fields[1].value, "2026-05-15T14:22:00.000Z");
        assert_eq!(meta.fields[2].key, "body");
        assert_eq!(meta.fields[2].value, "<p>Looks good to me.</p>");
        assert_eq!(meta.fields[3].key, "page");
        assert_eq!(meta.fields[3].value, "12345");
    }

    #[tokio::test]
    async fn comment_has_no_children() {
        use not_yet_done_content::children;
        let adapter = super::super::test_adapter().await;
        let node = ConfluenceCommentNode::new(synthetic_client(), sample_comment(), "12345");
        assert!(children::child_types(&adapter, &node).is_empty());
    }

    #[test]
    fn comment_actions_exposes_edit_and_delete() {
        let actions = comment_actions();
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].id, "edit");
        assert!(matches!(actions[0].input, InputSpec::Editor));
        assert_eq!(actions[1].id, "delete");
        assert!(matches!(actions[1].input, InputSpec::None));
    }

    #[test]
    fn node_type_advertises_html_syntax() {
        let t = comment_node_type();
        assert_eq!(t.type_id, "confluence:comment");
        assert_eq!(t.syntax.as_deref(), Some("html"));
        assert_eq!(t.file_extension, ".html");
    }

    #[test]
    fn content_is_self_and_version_none() {
        let node = ConfluenceCommentNode::new(synthetic_client(), sample_comment(), "12345");
        let content = node.content().expect("content present");
        assert!(content.version().is_none());
        assert_eq!(content.node_type().type_id, "confluence:comment");
    }

    #[tokio::test]
    async fn read_returns_body_storage_bytes() {
        let node = ConfluenceCommentNode::new(synthetic_client(), sample_comment(), "12345");
        let bytes = node.read().await.expect("read");
        assert_eq!(bytes, b"<p>Looks good to me.</p>");
    }

    #[tokio::test]
    async fn execute_rejects_unknown_action() {
        let mut node = ConfluenceCommentNode::new(synthetic_client(), sample_comment(), "12345");
        match node.execute("nope", ActionInput::None).await {
            Err(e) => assert!(format!("{e}").contains("nope")),
            Ok(_) => panic!("unknown action must be rejected"),
        }
    }

    /// Delete routes through the TUI's generic confirm pipeline — the
    /// dispatcher tests in the TUI cover the ViewRequest leg.
    #[tokio::test]
    async fn invoke_action_routes_delete_to_delete_self() {
        let node = ConfluenceCommentNode::new(synthetic_client(), sample_comment(), "12345");
        let ctx = ActionContext::default();
        match node.invoke_action("delete", &ctx).await {
            Ok(ActionDispatch::DeleteSelf { .. }) => {}
            other => panic!("expected DeleteSelf, got {other:?}"),
        }
        match node.invoke_action("edit", &ctx).await {
            Ok(ActionDispatch::Noop) => {}
            other => panic!("expected Noop for edit (editor path), got {other:?}"),
        }
    }

    /// When the node was constructed from a list payload (body + version
    /// both populated) `detail()` must seed the OnceCell with the cached
    /// data instead of issuing a round-trip — otherwise listings turn
    /// into N+1 GETs on first edit-prepare.
    #[tokio::test]
    async fn detail_uses_cache_when_payload_is_populated() {
        let node = ConfluenceCommentNode::new(synthetic_client(), sample_comment(), "12345");
        let detail = node.detail().await.expect("detail");
        assert_eq!(detail.id, "c1001");
        assert_eq!(detail.body_storage, "<p>Looks good to me.</p>");
        assert_eq!(detail.version_number, 3);
    }

    /// A `get_child`-synthesized comment has empty body + zero version —
    /// `detail()` must hit the network on first call. The synthetic
    /// client points at an invalid host, so the fetch errors out
    /// (locking in that the GET was actually attempted).
    #[tokio::test]
    async fn detail_fetches_when_payload_is_empty() {
        let stub = CommentMeta {
            id: "c1001".into(),
            title: "c1001".into(),
            body_storage: String::new(),
            author: String::new(),
            created: String::new(),
            version_number: 0,
        };
        let node = ConfluenceCommentNode::new(synthetic_client(), stub, "12345");
        let result = node.detail().await;
        assert!(
            result.is_err(),
            "detail() must error on unreachable host when body+version are empty"
        );
    }

    /// Banner round-trip: render → strip yields the input verbatim. The
    /// comment-edit code passes `strip_banner(text) == strip_banner(original)`
    /// through the NoChanges short-circuit, so this property is load-bearing.
    #[test]
    fn comment_conflict_banner_strips_clean() {
        let with_banner = render_comment_conflict_banner("<p>edited</p>");
        assert_eq!(strip_banner(&with_banner), "<p>edited</p>");
    }
}
