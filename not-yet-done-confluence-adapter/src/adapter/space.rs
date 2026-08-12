//! `confluence:space` node + `open-in-browser` action.
//!
//! Read-only leaf for CF-3 — pages are added in CF-4 by extending
//! [`ConfluenceSpaceNode::children_types`] to include `confluence:page`.
//! The single action is `open-in-browser`, which constructs the full web
//! URL by joining the adapter's base URL with the space's `webui` link
//! and spawns `xdg-open` detached (same pattern as the Jira attachment
//! viewer).
//!
//! The space's REST endpoints reach the user behind a path like
//! `/spaces/<KEY>` (relative `webui` from the API). Stashing the full
//! resolved URL on the node keeps `Node::execute` synchronous-ish — no
//! second REST call needed to spawn the viewer.

use std::sync::Arc;

use async_trait::async_trait;

use not_yet_done_content::{
    ActionInput, ActionOutcome, ContentError, EditorPrep, InputSpec, ListParams, ListResult,
    Metadata, MetadataField, Node, NodeAction, NodeSummary, NodeType, PageInfo, PageRequest,
    Result,
};

use crate::client::{ConfluenceClient, PageMeta, SpaceMeta};

use super::create_template::{ParsedCreate, parse_template, render_template, render_with_error};
use super::page::{ConfluencePageNode, page_node_type};
use super::{DEFAULT_PAGE_PAGE_SIZE, other_err};

pub(super) fn space_node_type() -> NodeType {
    NodeType {
        type_id: "confluence:space".into(),
        mime_type: "application/x-not-yet-done-folder".into(),
        syntax: None,
        file_extension: String::new(),
        display_name: "Confluence Space".into(),
    }
}

/// Static superset of actions exposed for `confluence:space`. Surfaced via
/// both `Node::actions()` and `ContentAdapter::actions_for_type()` so the
/// TUI can populate shortcut hints without instantiating a node.
pub(super) fn space_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new("create-page", "create top-level page", InputSpec::Editor),
        // open-in-browser is fire-and-forget (no input, no popup) → never
        // "active", so it stays in the status bar (default placement).
        NodeAction::new("open-in-browser", "open in browser", InputSpec::None),
    ]
}

/// A space's top-level page listing body. Extracted as a free fn so both the
/// legacy `ConfluenceSpaceNode::list` and the adapter-level `childs` closure
/// call the identical path. Keys solely on the space key (= `node.id()` for a
/// space node) + the client — no other space state is read.
///
/// CT-12: list the space's top-level pages directly. Pre-CT-12 we listed the
/// homepage's children, which hid orphan top-level pages AND broke tree_find —
/// the `ancestors[]` chain CQL returns starts at the top-level page (Homepage
/// or sibling), not at its children, so a homepage-children listing on level 1
/// never contained any of those ancestors and the walker bailed out with
/// NotInTree. `list_top_pages` returns the pages whose parent is the space
/// itself, which matches both the web UI's tree browser AND the search-hit
/// path shape.
pub(in crate::adapter) async fn list_space_top_pages(
    client: &ConfluenceClient,
    space_key: &str,
    params: ListParams,
) -> Result<ListResult> {
    let page_req = params.page.unwrap_or(PageRequest {
        offset: 0,
        limit: DEFAULT_PAGE_PAGE_SIZE,
    });
    let list = client
        .list_top_pages(space_key, page_req.offset, page_req.limit)
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
            // on the list call; populated for live pages, `None` for
            // synthetic entries the lookup path constructs locally.
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

pub(super) struct ConfluenceSpaceNode {
    client: Arc<ConfluenceClient>,
    /// Adapter base URL — kept on the node so the listing path can hand
    /// it down to each `ConfluencePageNode` it constructs (the page node
    /// uses it for its own `web_url` + for recursive child construction).
    base_url: String,
    space: SpaceMeta,
    /// Pre-resolved URL the open-in-browser action passes to `xdg-open`.
    /// Empty when Confluence omitted the `webui` link — in that case the
    /// action errors out instead of spawning a stub-URL.
    web_url: String,
    cached_metadata: Metadata,
}

impl ConfluenceSpaceNode {
    pub(super) fn new(client: Arc<ConfluenceClient>, base_url: &str, space: SpaceMeta) -> Self {
        let web_url = if space.webui.is_empty() {
            String::new()
        } else {
            format!("{}{}", base_url.trim_end_matches('/'), space.webui)
        };
        let cached_metadata = Metadata {
            fields: vec![
                MetadataField {
                    key: "key".into(),
                    value: space.key.clone(),
                    display_label: "Key".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "name".into(),
                    value: space.name.clone(),
                    display_label: "Name".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "type".into(),
                    value: space.space_type.clone(),
                    display_label: "Type".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "id".into(),
                    value: space.id.to_string(),
                    display_label: "ID".into(),
                    editable: false,
                    allowed_values: None,
                },
            ],
        };
        Self {
            client,
            base_url: base_url.to_string(),
            space,
            web_url,
            cached_metadata,
        }
    }

    /// Open a fresh create-page buffer for a top-level page in this
    /// space. No detail() prefetch needed — `self.space.key` is the
    /// only thing the POST needs.
    async fn prepare_create_page(&self) -> Result<EditorPrep> {
        Ok(EditorPrep {
            template: render_template(),
            version: String::new(),
            suffix: ".html".into(),
            file_path: None,
        })
    }

    /// Commit a create-page buffer against this space. Parent-id is
    /// always `None` here (top-level page); for child-page creation
    /// the page node's `create-child` action handles the request.
    async fn execute_create_page(&self, text: &str) -> Result<ActionOutcome> {
        let parsed: ParsedCreate = match parse_template(text) {
            Ok(p) => p,
            Err(msg) => {
                return Ok(ActionOutcome::Reopen {
                    content: render_with_error(text, &msg),
                    new_version: None,
                });
            }
        };
        match self
            .client
            .create_page(&self.space.key, None, &parsed.title, &parsed.body)
            .await
        {
            Ok(created) => Ok(ActionOutcome::Done {
                message: Some(format!(
                    "Created page {} (id {}) in space {}",
                    parsed.title, created.id, self.space.key
                )),
            }),
            Err(msg) => Ok(ActionOutcome::Reopen {
                content: render_with_error(text, &format!("Create failed: {msg}")),
                new_version: None,
            }),
        }
    }

    /// Spawn `xdg-open` on the resolved web URL. Detached (we don't wait
    /// for the viewer to exit) — same approach as the Jira attachment
    /// node. Returns `ActionOutcome::Done` so the TUI surfaces a
    /// status-bar confirmation.
    fn open_via_xdg(&self) -> Result<ActionOutcome> {
        if self.web_url.is_empty() {
            return Err(other_err(format!(
                "Space {} has no webui link — cannot open in browser",
                self.space.key
            )));
        }
        std::process::Command::new("xdg-open")
            .arg(&self.web_url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| other_err(format!("spawn xdg-open: {e}")))?;
        Ok(ActionOutcome::Done {
            message: Some(format!("opened {} in browser", self.space.key)),
        })
    }
}

#[async_trait]
impl Node for ConfluenceSpaceNode {
    fn id(&self) -> &str {
        &self.space.key
    }

    fn label(&self) -> &str {
        &self.space.name
    }

    fn node_type(&self) -> &NodeType {
        static T: std::sync::LazyLock<NodeType> = std::sync::LazyLock::new(space_node_type);
        &T
    }

    fn metadata(&self) -> &Metadata {
        &self.cached_metadata
    }
    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        // No dedicated `/content/{id}` round-trip in CF-4 — the child is
        // synthesized from the id alone (title = id until CF-5 lands).
        // The recursive `list()` call on the new node refreshes everything
        // from the network anyway.
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
            "create-page" => self.prepare_create_page().await,
            other => Err(ContentError::NotSupported(format!(
                "ConfluenceSpaceNode prepare: unknown action {other}"
            ))),
        }
    }

    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        match (action_id, input) {
            ("open-in-browser", ActionInput::None) => self.open_via_xdg(),
            ("create-page", ActionInput::Edited { text, .. }) => {
                self.execute_create_page(&text).await
            }
            (id, _) => Err(ContentError::NotSupported(format!(
                "ConfluenceSpaceNode action `{id}` not supported"
            ))),
        }
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

    fn sample_space() -> SpaceMeta {
        serde_json::from_str(
            r#"{
                "id": 17,
                "key": "DEMO",
                "name": "Demo Space",
                "type": "global",
                "_links": { "webui": "/spaces/DEMO" }
            }"#,
        )
        .expect("sample parses")
    }

    #[test]
    fn metadata_carries_key_name_type_id() {
        let node = ConfluenceSpaceNode::new(
            synthetic_client(),
            "https://wiki.example.invalid/confluence",
            sample_space(),
        );
        assert_eq!(node.id(), "DEMO");
        assert_eq!(node.label(), "Demo Space");

        let meta = node.metadata();
        assert_eq!(meta.fields.len(), 4);
        assert_eq!(meta.fields[0].key, "key");
        assert_eq!(meta.fields[0].value, "DEMO");
        assert_eq!(meta.fields[1].key, "name");
        assert_eq!(meta.fields[2].key, "type");
        assert_eq!(meta.fields[2].value, "global");
        assert_eq!(meta.fields[3].key, "id");
        assert_eq!(meta.fields[3].value, "17");
    }

    #[test]
    fn space_actions_includes_create_page_and_open_in_browser() {
        let actions = space_actions();
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].id, "create-page");
        assert!(matches!(actions[0].input, InputSpec::Editor));
        assert_eq!(actions[1].id, "open-in-browser");
    }

    #[test]
    fn web_url_joins_base_and_webui() {
        let node = ConfluenceSpaceNode::new(
            synthetic_client(),
            "https://wiki.example.invalid/confluence",
            sample_space(),
        );
        assert_eq!(
            node.web_url,
            "https://wiki.example.invalid/confluence/spaces/DEMO"
        );
    }

    #[test]
    fn web_url_is_empty_when_links_missing() {
        let space: SpaceMeta = serde_json::from_str(
            r#"{
                "id": 18,
                "key": "BARE",
                "name": "Bare",
                "type": "global"
            }"#,
        )
        .expect("parses");
        let node = ConfluenceSpaceNode::new(
            synthetic_client(),
            "https://wiki.example.invalid/confluence",
            space,
        );
        assert!(node.web_url.is_empty());
    }

    #[tokio::test]
    async fn execute_rejects_unknown_action() {
        let mut node = ConfluenceSpaceNode::new(
            synthetic_client(),
            "https://wiki.example.invalid/confluence",
            sample_space(),
        );
        match node.execute("nope", ActionInput::None).await {
            Err(e) => assert!(format!("{e}").contains("nope")),
            Ok(_) => panic!("unknown action must be rejected"),
        }
    }

    #[tokio::test]
    async fn execute_rejects_open_when_webui_missing() {
        let space: SpaceMeta = serde_json::from_str(
            r#"{
                "id": 19,
                "key": "NOWEB",
                "name": "No Webui",
                "type": "global"
            }"#,
        )
        .expect("parses");
        let mut node = ConfluenceSpaceNode::new(
            synthetic_client(),
            "https://wiki.example.invalid/confluence",
            space,
        );
        match node.execute("open-in-browser", ActionInput::None).await {
            Err(e) => assert!(
                format!("{e}").contains("NOWEB"),
                "error mentions space key: {e}"
            ),
            Ok(_) => panic!("missing webui must be rejected"),
        }
    }

    // Pre-CT-12 we had a `pre_seeds_homepage_id_when_present_on_meta`
    // test for the OnceCell-cached homepage id. The new list() path
    // doesn't need the homepage id at all (it lists top-level pages of
    // the space by key), so the cache and its test are gone.
}
