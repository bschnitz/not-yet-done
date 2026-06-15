//! `confluence:page` node, action set, and `Content` impl. The recursive
//! read-only branch lives here too (every page lists its own direct
//! children via [`ConfluenceClient::list_child_pages`]; top-level pages of
//! a space come from [`super::super::space::ConfluenceSpaceNode::list`]).
//!
//! CF-5 adds lazy hydration of the full page detail (`body.storage`,
//! version, ancestors, labels) via [`tokio::sync::OnceCell`] so the
//! listing path stays cheap. CF-9 adds the `edit` action which lives in
//! the [`edit`] submodule with conflict-merge logic in [`merge`] and
//! XHTML pretty-print helpers in [`format`]. Page-create / -delete /
//! comment-CRUD join the action set in CF-10..CF-12.

mod add_comment;
mod clone;
mod create;
mod edit;
// `format` is also used by the comment-edit flow (CF-12) — both flavours
// of body XHTML pretty-print through the same xmllint pipeline.
pub(super) mod format;
mod merge;
mod upload;

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::OnceCell;

use not_yet_done_content::{
    ActionContext, ActionDispatch, ActionInput, ActionOutcome, Content, ContentError, EditorPrep,
    HintPlacement, InputSpec, ListParams, ListResult, Metadata, MetadataField, Node, NodeAction,
    NodeSummary, NodeType, PageInfo, PageRequest, Result,
};

use crate::client::{ConfluenceClient, PageDetail, PageMeta};

use super::attachment::{ConfluenceAttachmentNode, attachment_node_type};
use super::comment::{ConfluenceCommentNode, comment_node_type};
use crate::adapter::{DEFAULT_PAGE_PAGE_SIZE, other_err};

pub(super) fn page_node_type() -> NodeType {
    NodeType {
        type_id: "confluence:page".into(),
        mime_type: "application/x-not-yet-done-folder".into(),
        // Confluence `body.storage` is XHTML-flavoured with custom Atlassian
        // tags (`<ac:structured-macro>`, …). `"html"` is the closest stock
        // syntax-highlighter mapping; CF-9 reuses the same `.html` suffix
        // when handing the buffer to `$EDITOR`.
        syntax: Some("html".into()),
        file_extension: ".html".into(),
        display_name: "Confluence Page".into(),
    }
}

/// Static superset of actions exposed for `confluence:page`. Surfaced via
/// both `Node::actions()` and `ContentAdapter::actions_for_type()` so the
/// TUI can populate shortcut hints without instantiating a node.
pub(super) fn page_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new("edit", "edit", InputSpec::Editor)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('e'),
        NodeAction::new("create-child", "create child page", InputSpec::Editor)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('a'),
        // CF-12: `c` opens an empty XHTML buffer for a new comment on
        // this page. The comment's body POSTs straight to
        // `/rest/api/content` with `type=comment, container={page_id}`;
        // editor-flow + reload-after-Done are inherited from the
        // generic CreateContentChild pipeline.
        NodeAction::new("add-comment", "add comment", InputSpec::Editor)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('c'),
        // CF-13: capital `A` opens the FilePicker (multi-select). Each
        // chosen file is POSTed to `/content/{id}/child/attachment` as
        // multipart; aggregate failures bubble up as a ContentError that
        // names every failed path. Capital `A` follows the destructive-
        // upload convention (lowercase `a` is already taken by
        // `create-child`, lowercase `d` by attachment download).
        NodeAction::new(
            "upload-attachment",
            "upload attachment",
            InputSpec::FilePicker { multi: true },
        )
        .with_placement(HintPlacement::ActionBar)
        .with_default_key('A'),
        // CF-14: `y` opens an editor pre-filled with the source page's
        // title (suffixed with " (Clone)") and body, then POSTs a new
        // page under the same parent in the same space. Reuses the
        // create-child template parser (CF-10) so the buffer shape is
        // identical to `a: create-child`. Cross-space cloning isn't
        // exposed at adapter level — the user can move the resulting
        // page via the Confluence UI if they want it elsewhere.
        NodeAction::new("clone", "clone", InputSpec::Editor)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('y'),
        // CF-11: capital `D` (Shift+D) routes through `invoke_action` →
        // [`ActionDispatch::DeleteSelf`], which the TUI stages behind a
        // confirm popup before firing the actual `Node::execute("delete")`.
        // Using capital `D` matches the destructive-action convention in
        // the codebase (lowercase `d` is reserved for non-destructive
        // operations like attachment download).
        NodeAction::new("delete", "delete (Trash)", InputSpec::None)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('D'),
        NodeAction::new("open-in-browser", "open in browser", InputSpec::None)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('o'),
    ]
}

pub(super) struct ConfluencePageNode {
    client: Arc<ConfluenceClient>,
    /// Adapter base URL, retained for child-page `web_url` resolution and for
    /// propagation into recursively-constructed children.
    base_url: String,
    page: PageMeta,
    /// Pre-resolved URL the open-in-browser action passes to `xdg-open`.
    /// Empty when Confluence omitted the `webui` link — in that case the
    /// action errors out instead of spawning a stub URL.
    web_url: String,
    cached_metadata: Metadata,
    /// Lazily populated full page detail (`body.storage`, version, ancestors,
    /// labels) — fetched once on first `read()` / `detail()` call and
    /// reused for subsequent preview-toggles and edit operations.
    detail: OnceCell<PageDetail>,
}

impl ConfluencePageNode {
    pub(super) fn new(client: Arc<ConfluenceClient>, base_url: &str, page: PageMeta) -> Self {
        let web_url = if page.webui.is_empty() {
            String::new()
        } else {
            format!("{}{}", base_url.trim_end_matches('/'), page.webui)
        };
        let cached_metadata = Metadata {
            fields: vec![
                MetadataField {
                    key: "id".into(),
                    value: page.id.clone(),
                    display_label: "ID".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "title".into(),
                    value: page.title.clone(),
                    display_label: "Title".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "type".into(),
                    value: page.page_type.clone(),
                    display_label: "Type".into(),
                    editable: false,
                    allowed_values: None,
                },
            ],
        };
        Self {
            client,
            base_url: base_url.to_string(),
            page,
            web_url,
            cached_metadata,
            detail: OnceCell::new(),
        }
    }

    /// Lazily fetch the full page detail. The first call hits
    /// `GET /content/{id}?expand=body.storage,version,ancestors,metadata.labels`;
    /// every subsequent caller (preview re-toggle, future edit prepare,
    /// breadcrumb render) reuses the cached value.
    pub(super) async fn detail(&self) -> Result<&PageDetail> {
        self.detail
            .get_or_try_init(|| async {
                self.client
                    .get_page(&self.page.id)
                    .await
                    .map_err(other_err)
            })
            .await
    }

    /// Inner page-listing path — split out of `Node::list` so the
    /// node-type-routing in `list()` stays readable. Always queries
    /// `/content/{id}/child/page` for the direct children of this page.
    async fn list_pages(&self, params: ListParams) -> Result<ListResult> {
        let page_req = params.page.unwrap_or(PageRequest {
            offset: 0,
            limit: DEFAULT_PAGE_PAGE_SIZE,
        });
        let list = self
            .client
            .list_child_pages(&self.page.id, page_req.offset, page_req.limit)
            .await
            .map_err(other_err)?;
        let items = list
            .pages
            .into_iter()
            .map(|p| NodeSummary {
                id: p.id.clone(),
                label: p.title.clone(),
                node_type: page_node_type(),
                metadata: Metadata {
                    fields: vec![
                        MetadataField {
                            key: "id".into(),
                            value: p.id,
                            display_label: "ID".into(),
                            editable: false,
                            allowed_values: None,
                        },
                        MetadataField {
                            key: "title".into(),
                            value: p.title,
                            display_label: "Title".into(),
                            editable: false,
                            allowed_values: None,
                        },
                        MetadataField {
                            key: "type".into(),
                            value: p.page_type,
                            display_label: "Type".into(),
                            editable: false,
                            allowed_values: None,
                        },
                    ],
                },
                // `children.page.size > 0` from `?expand=children.page.size`
                // on the list call. `Some(false)` lets the tree renderer
                // pick the leaf glyph (`📄` via view-YAML `leaf_glyph`)
                // instead of the expand marker — otherwise the static
                // `recursive: true` ChildDef would force `▶` on every page
                // row. Confluence Server silently drops `childTypes.page`
                // against `/child/page`, so `children.page.size` is the
                // only listing-side hook that reports per-row counts.
                has_children: p.has_children,
            })
            .collect();
        let page_info = PageInfo {
            offset: list.start,
            limit: list.limit,
            total: None,
            has_next: list.has_next,
            has_prev: list.start > 0,
        };
        Ok(ListResult {
            items,
            applied_sort: Vec::new(),
            page: Some(page_info),
            batch_download_available: false,
            downloaded: Vec::new(),
        })
    }

    /// Inner comment-listing path — split out of `Node::list` so the
    /// node-type-routing in `list()` stays readable. Always queries
    /// `/content/{id}/child/comment?expand=body.storage,version` for the
    /// comments hanging off this page. Body XHTML rides in on the list
    /// response so the comment node never needs a per-row detail GET.
    async fn list_comments(&self, params: ListParams) -> Result<ListResult> {
        let page_req = params.page.unwrap_or(PageRequest {
            offset: 0,
            limit: DEFAULT_PAGE_PAGE_SIZE,
        });
        let list = self
            .client
            .list_comments(&self.page.id, page_req.offset, page_req.limit)
            .await
            .map_err(other_err)?;
        let items = list
            .comments
            .into_iter()
            .map(|c| {
                let composite_id = format!("{}/comment/{}", self.page.id, c.id);
                NodeSummary {
                    id: composite_id,
                    label: c.title.clone(),
                    node_type: comment_node_type(),
                    metadata: Metadata {
                        fields: vec![
                            MetadataField {
                                key: "author".into(),
                                value: c.author,
                                display_label: "Author".into(),
                                editable: false,
                                allowed_values: None,
                            },
                            MetadataField {
                                key: "created".into(),
                                value: c.created,
                                display_label: "Created".into(),
                                editable: false,
                                allowed_values: None,
                            },
                            MetadataField {
                                key: "body".into(),
                                value: c.body_storage,
                                display_label: "Body".into(),
                                editable: false,
                                allowed_values: None,
                            },
                        ],
                    },
                    has_children: None,
                }
            })
            .collect();
        let page_info = PageInfo {
            offset: list.start,
            limit: list.limit,
            total: None,
            has_next: list.has_next,
            has_prev: list.start > 0,
        };
        Ok(ListResult {
            items,
            applied_sort: Vec::new(),
            page: Some(page_info),
            batch_download_available: false,
            downloaded: Vec::new(),
        })
    }

    /// Inner attachment-listing path — split out of `Node::list` so the
    /// node-type-routing in `list()` stays readable. Always queries
    /// `/content/{id}/child/attachment` for the attachments hanging off
    /// this page.
    async fn list_attachments(&self, params: ListParams) -> Result<ListResult> {
        let page_req = params.page.unwrap_or(PageRequest {
            offset: 0,
            limit: DEFAULT_PAGE_PAGE_SIZE,
        });
        let list = self
            .client
            .list_attachments(&self.page.id, page_req.offset, page_req.limit)
            .await
            .map_err(other_err)?;
        let items = list
            .attachments
            .into_iter()
            .map(|a| {
                let composite_id = format!("{}/attachment/{}", self.page.id, a.id);
                NodeSummary {
                    id: composite_id,
                    label: a.title.clone(),
                    node_type: attachment_node_type(),
                    metadata: Metadata {
                        fields: vec![
                            MetadataField {
                                key: "filename".into(),
                                value: a.title,
                                display_label: "Filename".into(),
                                editable: false,
                                allowed_values: None,
                            },
                            MetadataField {
                                key: "author".into(),
                                value: a.author,
                                display_label: "Author".into(),
                                editable: false,
                                allowed_values: None,
                            },
                            MetadataField {
                                key: "size".into(),
                                value: a.file_size.to_string(),
                                display_label: "Size".into(),
                                editable: false,
                                allowed_values: None,
                            },
                            MetadataField {
                                key: "mime_type".into(),
                                value: a.media_type,
                                display_label: "Type".into(),
                                editable: false,
                                allowed_values: None,
                            },
                            MetadataField {
                                key: "created".into(),
                                value: a.created,
                                display_label: "Created".into(),
                                editable: false,
                                allowed_values: None,
                            },
                        ],
                    },
                    has_children: None,
                }
            })
            .collect();
        let page_info = PageInfo {
            offset: list.start,
            limit: list.limit,
            total: None,
            has_next: list.has_next,
            has_prev: list.start > 0,
        };
        Ok(ListResult {
            items,
            applied_sort: Vec::new(),
            page: Some(page_info),
            batch_download_available: false,
            downloaded: Vec::new(),
        })
    }

    /// CF-11: soft-delete the page (move to Trash) via
    /// [`ConfluenceClient::delete_page`]. Confirmation happens TUI-side
    /// before this fires — by the time `execute_delete` runs the user
    /// has already accepted. Returns [`ActionOutcome::Done`] so the
    /// editor's `FollowUp::ReloadContentPane` refreshes the listing and
    /// the gone row drops out.
    async fn execute_delete(&self) -> Result<ActionOutcome> {
        self.client
            .delete_page(&self.page.id, false)
            .await
            .map_err(other_err)?;
        Ok(ActionOutcome::Done {
            message: Some(format!(
                "Moved page {} (id {}) to Trash",
                self.page.title, self.page.id
            )),
        })
    }

    /// Resolve the full web URL, hydrating the page detail when the node
    /// was built without a `webui` link. The TUI re-resolves every action
    /// target via `ContentAdapter::get_by_id`, which synthesizes a stub
    /// `PageMeta` with an empty `webui` (the listing's link is discarded on
    /// that round-trip). So `web_url` is reliably empty for action targets;
    /// fall back to a `GET /content/{id}` detail fetch (which carries
    /// `_links.webui`) to recover the link instead of erroring out.
    async fn resolve_web_url(&self) -> Result<String> {
        if !self.web_url.is_empty() {
            return Ok(self.web_url.clone());
        }
        let webui = &self.detail().await?.webui;
        if webui.is_empty() {
            return Ok(String::new());
        }
        Ok(format!("{}{}", self.base_url.trim_end_matches('/'), webui))
    }

    async fn open_via_xdg(&self) -> Result<ActionOutcome> {
        let web_url = self.resolve_web_url().await?;
        if web_url.is_empty() {
            return Err(other_err(format!(
                "Page {} has no webui link — cannot open in browser",
                self.page.id
            )));
        }
        std::process::Command::new("xdg-open")
            .arg(&web_url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| other_err(format!("spawn xdg-open: {e}")))?;
        Ok(ActionOutcome::Done {
            message: Some(format!("opened page {} in browser", self.page.id)),
        })
    }
}

#[async_trait]
impl Node for ConfluencePageNode {
    fn id(&self) -> &str {
        &self.page.id
    }

    fn label(&self) -> &str {
        &self.page.title
    }

    fn node_type(&self) -> &NodeType {
        static T: std::sync::LazyLock<NodeType> = std::sync::LazyLock::new(page_node_type);
        &T
    }

    fn metadata(&self) -> &Metadata {
        &self.cached_metadata
    }

    fn actions(&self) -> Vec<NodeAction> {
        page_actions()
    }

    fn content(&self) -> Option<&dyn Content> {
        Some(self)
    }

    fn children_types(&self) -> Vec<NodeType> {
        // CF-6 added `confluence:attachment` as a second branch off every
        // page; CF-7 adds `confluence:comment` as a third. The page-tree
        // (`confluence:page`) stays the primary recursive branch — the
        // View-YAML's secondary ChildDefs pull attachments and comments
        // via the same `list()` entry point, routed by `node_type`.
        vec![
            page_node_type(),
            attachment_node_type(),
            comment_node_type(),
        ]
    }

    async fn list(&self, params: ListParams) -> Result<ListResult> {
        match params.node_type.type_id.as_str() {
            "confluence:page" => self.list_pages(params).await,
            "confluence:attachment" => self.list_attachments(params).await,
            "confluence:comment" => self.list_comments(params).await,
            other => Err(ContentError::NotSupported(format!(
                "ConfluencePageNode does not list {other}",
            ))),
        }
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        // Attachment child ids come in as `<page_id>/attachment/<att_id>`
        // (from this node's `list()` for `confluence:attachment`). Strip
        // the prefix and synthesize a minimal `AttachmentMeta` — the only
        // action the attachment node carries (download) needs the
        // `_links.download` URL which a synthesized lookup can't
        // recover, so this lookup-path is only useful for hierarchy
        // traversal, not for executing the download action.
        if let Some(att_id) = id.split('/').nth(2)
            && id.starts_with(&format!("{}/attachment/", self.page.id))
        {
            return Ok(Box::new(ConfluenceAttachmentNode::new(
                Arc::clone(&self.client),
                crate::client::AttachmentMeta {
                    id: att_id.to_string(),
                    title: att_id.to_string(),
                    attachment_type: "attachment".into(),
                    file_size: 0,
                    media_type: String::new(),
                    author: String::new(),
                    created: String::new(),
                    download_path: String::new(),
                },
                &self.page.id,
            )));
        }
        // Comment child ids come in as `<page_id>/comment/<comment_id>`.
        // The synthesized lookup carries an empty body — useful for
        // hierarchy round-trips (e.g. link-target reconstruction); the
        // listing path is the authoritative source for body XHTML.
        if let Some(comment_id) = id.split('/').nth(2)
            && id.starts_with(&format!("{}/comment/", self.page.id))
        {
            return Ok(Box::new(ConfluenceCommentNode::new(
                Arc::clone(&self.client),
                crate::client::CommentMeta {
                    id: comment_id.to_string(),
                    title: comment_id.to_string(),
                    body_storage: String::new(),
                    author: String::new(),
                    created: String::new(),
                    version_number: 0,
                },
                &self.page.id,
            )));
        }
        // CF-4 first cut: no dedicated `/content/{id}` round-trip — the
        // page child is synthesized from the id alone. The recursive
        // `list()` call on the new node refreshes everything from the
        // network anyway.
        Ok(Box::new(ConfluencePageNode::new(
            Arc::clone(&self.client),
            &self.base_url,
            PageMeta {
                id: id.to_string(),
                title: id.to_string(),
                page_type: "page".into(),
                webui: String::new(),
                has_children: None,
            },
        )))
    }

    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        match action_id {
            "edit" => self.prepare_edit().await,
            "create-child" => self.prepare_create_child().await,
            "add-comment" => self.prepare_add_comment().await,
            "clone" => self.prepare_clone().await,
            other => Err(ContentError::NotSupported(format!(
                "ConfluencePageNode prepare: unknown action {other}"
            ))),
        }
    }

    /// CF-11: route the `delete` shortcut through the TUI's
    /// confirm-popup pipeline. Every other action either has its own
    /// dispatch path (editor for `edit` / `create-child`, direct execute
    /// for `open-in-browser`) or no shortcut wired to it, so they all
    /// fall through to [`ActionDispatch::Noop`].
    async fn invoke_action(
        &self,
        name: &str,
        _ctx: &ActionContext,
    ) -> Result<ActionDispatch> {
        match name {
            "delete" => Ok(ActionDispatch::DeleteSelf),
            _ => Ok(ActionDispatch::Noop),
        }
    }

    async fn execute(
        &mut self,
        action_id: &str,
        input: ActionInput,
    ) -> Result<ActionOutcome> {
        match (action_id, input) {
            ("open-in-browser", ActionInput::None) => self.open_via_xdg().await,
            ("edit", ActionInput::Edited { text, original, version }) => {
                self.execute_edit(&text, &original, &version).await
            }
            ("create-child", ActionInput::Edited { text, .. }) => {
                self.execute_create_child(&text).await
            }
            // CF-12 add-comment: POST /content with `type=comment, container={id}`.
            // The text is the comment body as raw XHTML; banner-stripping
            // happens inside `execute_add_comment` so reopen-on-error
            // doesn't stack banners.
            ("add-comment", ActionInput::Edited { text, .. }) => {
                self.execute_add_comment(&text).await
            }
            // CF-13 upload-attachment: the FilePicker hands back N paths;
            // execute_upload_attachment loops one POST per file and
            // aggregates failures into a single ContentError so the user
            // sees exactly what didn't make it.
            ("upload-attachment", ActionInput::Files(paths)) => {
                self.execute_upload_attachment(paths).await
            }
            // CF-14 clone: reuses the create-child template parser. The
            // POST runs against the same space + parent the source page
            // lives under; the user can edit title/body in the buffer
            // before saving.
            ("clone", ActionInput::Edited { text, .. }) => {
                self.execute_clone(&text).await
            }
            // CF-11 page-delete (Trash). Fired by the TUI after the user
            // confirms the popup staged from `invoke_action`. Returns the
            // page id in the notification because the row is gone from
            // the listing by the time the user sees the message.
            ("delete", ActionInput::None) => self.execute_delete().await,
            (id, _) => Err(ContentError::NotSupported(format!(
                "ConfluencePageNode action `{id}` not supported"
            ))),
        }
    }
}

#[async_trait]
impl Content for ConfluencePageNode {
    fn node_type(&self) -> &NodeType {
        static T: std::sync::LazyLock<NodeType> = std::sync::LazyLock::new(page_node_type);
        &T
    }

    /// Page version stash for CF-9 conflict detection. CF-5 keeps this
    /// `None` — the preview path doesn't need it, and the `&str` lifetime
    /// would require carrying a pre-formatted string on the detail struct
    /// just to satisfy the trait. CF-9 will route the version through a
    /// dedicated `EditorPrep.version` field instead.
    fn version(&self) -> Option<&str> {
        None
    }

    async fn read(&self) -> Result<Vec<u8>> {
        Ok(self.detail().await?.body_storage.as_bytes().to_vec())
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

    fn sample_page() -> PageMeta {
        serde_json::from_str(
            r#"{
                "id": "12345",
                "type": "page",
                "title": "Sample",
                "_links": { "webui": "/spaces/DEMO/pages/12345/Sample" }
            }"#,
        )
        .expect("parses")
    }

    #[test]
    fn metadata_carries_id_title_type() {
        let node = ConfluencePageNode::new(
            synthetic_client(),
            "https://wiki.example.invalid/confluence",
            sample_page(),
        );
        assert_eq!(node.id(), "12345");
        assert_eq!(node.label(), "Sample");
        let meta = node.metadata();
        assert_eq!(meta.fields.len(), 3);
        assert_eq!(meta.fields[0].key, "id");
        assert_eq!(meta.fields[1].value, "Sample");
        assert_eq!(meta.fields[2].value, "page");
    }

    #[test]
    fn web_url_joins_base_and_webui() {
        let node = ConfluencePageNode::new(
            synthetic_client(),
            "https://wiki.example.invalid/confluence",
            sample_page(),
        );
        assert_eq!(
            node.web_url,
            "https://wiki.example.invalid/confluence/spaces/DEMO/pages/12345/Sample"
        );
    }

    #[test]
    fn web_url_is_empty_when_links_missing() {
        let page = PageMeta {
            id: "1".into(),
            title: "t".into(),
            page_type: "page".into(),
            webui: String::new(),
            has_children: None,
        };
        let node = ConfluencePageNode::new(
            synthetic_client(),
            "https://wiki.example.invalid",
            page,
        );
        assert!(node.web_url.is_empty());
    }

    #[test]
    fn children_types_has_page_attachment_and_comment_branches() {
        let node = ConfluencePageNode::new(
            synthetic_client(),
            "https://wiki.example.invalid",
            sample_page(),
        );
        let types = node.children_types();
        assert_eq!(types.len(), 3);
        assert_eq!(types[0].type_id, "confluence:page");
        assert_eq!(types[1].type_id, "confluence:attachment");
        assert_eq!(types[2].type_id, "confluence:comment");
    }

    #[tokio::test]
    async fn list_rejects_unknown_node_type() {
        let node = ConfluencePageNode::new(
            synthetic_client(),
            "https://wiki.example.invalid",
            sample_page(),
        );
        let params = ListParams {
            node_type: NodeType {
                type_id: "confluence:space".into(),
                mime_type: String::new(),
                syntax: None,
                file_extension: String::new(),
                display_name: String::new(),
            },
            page: None,
            sort: Vec::new(),
            query: None,
            download: false,
            group_by: None,
        };
        match node.list(params).await {
            Err(e) => assert!(
                format!("{e}").contains("confluence:space"),
                "error mentions rejected type: {e}"
            ),
            Ok(_) => panic!("unknown node type must be rejected"),
        }
    }

    #[tokio::test]
    async fn get_child_routes_attachment_composite_id() {
        let node = ConfluencePageNode::new(
            synthetic_client(),
            "https://wiki.example.invalid",
            sample_page(),
        );
        let child = node
            .get_child("12345/attachment/att999")
            .await
            .expect("synthesized");
        assert_eq!(child.id(), "12345/attachment/att999");
        assert_eq!(child.node_type().type_id, "confluence:attachment");
    }

    #[tokio::test]
    async fn get_child_routes_comment_composite_id() {
        let node = ConfluencePageNode::new(
            synthetic_client(),
            "https://wiki.example.invalid",
            sample_page(),
        );
        let child = node
            .get_child("12345/comment/c1001")
            .await
            .expect("synthesized");
        assert_eq!(child.id(), "12345/comment/c1001");
        assert_eq!(child.node_type().type_id, "confluence:comment");
    }

    #[tokio::test]
    async fn get_child_falls_back_to_page_for_plain_id() {
        let node = ConfluencePageNode::new(
            synthetic_client(),
            "https://wiki.example.invalid",
            sample_page(),
        );
        let child = node.get_child("99").await.expect("synthesized");
        assert_eq!(child.id(), "99");
        assert_eq!(child.node_type().type_id, "confluence:page");
    }

    #[tokio::test]
    async fn execute_rejects_unknown_action() {
        let mut node = ConfluencePageNode::new(
            synthetic_client(),
            "https://wiki.example.invalid",
            sample_page(),
        );
        match node.execute("nope", ActionInput::None).await {
            Err(e) => assert!(format!("{e}").contains("nope")),
            Ok(_) => panic!("unknown action must be rejected"),
        }
    }

    #[tokio::test]
    async fn open_with_empty_webui_hydrates_then_errors_not_spawns() {
        // A node built without a `webui` link (the shape `get_by_id`
        // synthesizes for every action target) no longer rejects up
        // front — it falls back to a `GET /content/{id}` detail fetch to
        // recover the link. Against the unreachable `.invalid` host that
        // fetch fails, so the action surfaces an error instead of
        // spawning a browser on an empty URL. The safety property under
        // test: an empty webui never yields `Ok` (no stub browser spawn).
        let page = PageMeta {
            id: "9".into(),
            title: "Bare".into(),
            page_type: "page".into(),
            webui: String::new(),
            has_children: None,
        };
        let mut node = ConfluencePageNode::new(
            synthetic_client(),
            "https://wiki.example.invalid",
            page,
        );
        match node.execute("open-in-browser", ActionInput::None).await {
            Err(_) => {}
            Ok(_) => panic!("empty webui must never spawn a browser"),
        }
    }

    #[test]
    fn page_actions_includes_full_crud_set_plus_open_in_browser() {
        let actions = page_actions();
        assert_eq!(actions.len(), 7);
        assert_eq!(actions[0].id, "edit");
        assert_eq!(actions[0].default_key, Some('e'));
        assert!(matches!(actions[0].input, InputSpec::Editor));
        assert_eq!(actions[1].id, "create-child");
        assert_eq!(actions[1].default_key, Some('a'));
        assert!(matches!(actions[1].input, InputSpec::Editor));
        assert_eq!(actions[2].id, "add-comment");
        assert_eq!(actions[2].default_key, Some('c'));
        assert!(matches!(actions[2].input, InputSpec::Editor));
        assert_eq!(actions[3].id, "upload-attachment");
        assert_eq!(actions[3].default_key, Some('A'));
        assert!(matches!(
            actions[3].input,
            InputSpec::FilePicker { multi: true }
        ));
        assert_eq!(actions[4].id, "clone");
        assert_eq!(actions[4].default_key, Some('y'));
        assert!(matches!(actions[4].input, InputSpec::Editor));
        assert_eq!(actions[5].id, "delete");
        assert_eq!(actions[5].default_key, Some('D'));
        assert!(matches!(actions[5].input, InputSpec::None));
        assert_eq!(actions[6].id, "open-in-browser");
        assert_eq!(actions[6].default_key, Some('o'));
    }

    /// CF-11: `delete` is wired through the TUI's
    /// confirm-popup path via [`ActionDispatch::DeleteSelf`]. Every
    /// other action stays on its own pipeline (editor/picker/direct
    /// execute) → `Noop` here. The dispatcher tests in the TUI cover
    /// the routing-to-ViewRequest leg.
    #[tokio::test]
    async fn invoke_action_routes_delete_to_delete_self() {
        let node = ConfluencePageNode::new(
            synthetic_client(),
            "https://wiki.example.invalid",
            sample_page(),
        );
        let ctx = not_yet_done_content::ActionContext::default();
        match node.invoke_action("delete", &ctx).await {
            Ok(ActionDispatch::DeleteSelf) => {}
            other => panic!("expected DeleteSelf for delete, got {other:?}"),
        }
        match node.invoke_action("edit", &ctx).await {
            Ok(ActionDispatch::Noop) => {}
            other => panic!("expected Noop for edit (editor-path), got {other:?}"),
        }
    }

    #[test]
    fn node_type_advertises_html_syntax() {
        let t = page_node_type();
        assert_eq!(t.type_id, "confluence:page");
        assert_eq!(t.syntax.as_deref(), Some("html"));
        assert_eq!(t.file_extension, ".html");
    }

    #[test]
    fn content_is_self() {
        let node = ConfluencePageNode::new(
            synthetic_client(),
            "https://wiki.example.invalid",
            sample_page(),
        );
        // Node::content() exposes the page node as Content so the preview
        // pipeline can call read_text() on it.
        assert!(node.content().is_some());
    }

    #[test]
    fn content_version_is_none_until_cf9() {
        let node = ConfluencePageNode::new(
            synthetic_client(),
            "https://wiki.example.invalid",
            sample_page(),
        );
        let content = node.content().expect("content present");
        assert!(content.version().is_none());
        assert_eq!(content.node_type().type_id, "confluence:page");
    }

    #[tokio::test]
    async fn detail_returns_error_when_no_server() {
        // The synthetic client points at an invalid host, so the lazy
        // fetch must surface a transport error rather than silently
        // succeeding. This locks in that `detail()` actually attempts the
        // GET on first call (vs. swallowing it).
        let node = ConfluencePageNode::new(
            synthetic_client(),
            "https://wiki.example.invalid",
            sample_page(),
        );
        let result = node.detail().await;
        assert!(
            result.is_err(),
            "detail() must error on unreachable host, got Ok"
        );
    }
}
