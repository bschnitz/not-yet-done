//! In-memory mock adapter for testing.
//!
//! Gated behind `#[cfg(test)]` or `feature = "mock"`.
//!
//! # Example
//! ```ignore
//! use not_yet_done_content::mock::*;
//!
//! let adapter = MockAdapterBuilder::new("test")
//!     .node(MockNodeData::new("root", "Root")
//!         .child_type(issue_type())
//!         .child(MockNodeData::new("ISSUE-1", "Fix bug")
//!             .node_type(issue_type())
//!             .meta("status", "Open")
//!             .content("Description here")))
//!     .build();
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::*;

// ---------------------------------------------------------------------------
// Builder types
// ---------------------------------------------------------------------------

/// Data for a single mock node.
#[derive(Clone, Debug)]
pub struct MockNodeData {
    pub id: String,
    pub label: String,
    pub node_type: NodeType,
    pub metadata: Metadata,
    pub child_types: Vec<NodeType>,
    pub children: Vec<MockNodeData>,
    pub content_text: Option<String>,
}

impl MockNodeData {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            node_type: default_node_type(),
            metadata: Metadata::default(),
            child_types: Vec::new(),
            children: Vec::new(),
            content_text: None,
        }
    }

    pub fn node_type(mut self, nt: NodeType) -> Self {
        self.node_type = nt;
        self
    }

    pub fn child_type(mut self, nt: NodeType) -> Self {
        self.child_types.push(nt);
        self
    }

    pub fn child(mut self, child: MockNodeData) -> Self {
        self.children.push(child);
        self
    }

    pub fn meta(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        let key = key.into();
        let value = value.into();
        self.metadata.fields.push(MetadataField {
            display_label: key.clone(),
            key,
            value,
            editable: true,
            allowed_values: None,
        });
        self
    }

    pub fn content(mut self, text: impl Into<String>) -> Self {
        self.content_text = Some(text.into());
        self
    }
}

/// Builder for MockAdapter.
pub struct MockAdapterBuilder {
    adapter_type: String,
    instance_id: Option<String>,
    root: Option<MockNodeData>,
    capabilities: AdapterCapabilities,
    /// Per-node_type action lists for `actions_for_type`. Tests use
    /// this to stub the shortcut-hint resolution without needing
    /// real `Node` impls.
    actions_by_type: HashMap<String, Vec<NodeAction>>,
}

impl MockAdapterBuilder {
    pub fn new(adapter_type: impl Into<String>) -> Self {
        Self {
            adapter_type: adapter_type.into(),
            instance_id: None,
            root: None,
            capabilities: AdapterCapabilities::default(),
            actions_by_type: HashMap::new(),
        }
    }

    /// Register the action list returned by `actions_for_type` for the
    /// given node-type id. Used by tests that exercise the shortcut-
    /// hint resolution path.
    pub fn actions_for(
        mut self,
        node_type_id: impl Into<String>,
        actions: Vec<NodeAction>,
    ) -> Self {
        self.actions_by_type.insert(node_type_id.into(), actions);
        self
    }

    /// Override the instance id (default = `adapter_type`).
    pub fn instance_id(mut self, id: impl Into<String>) -> Self {
        self.instance_id = Some(id.into());
        self
    }

    /// Set the root node (and its entire subtree).
    pub fn node(mut self, root: MockNodeData) -> Self {
        self.root = Some(root);
        self
    }

    pub fn capabilities(mut self, caps: AdapterCapabilities) -> Self {
        self.capabilities = caps;
        self
    }

    pub fn build(self) -> MockAdapter {
        let root = self.root.unwrap_or_else(|| MockNodeData::new("root", "Root"));
        // Flatten the tree into a lookup map
        let mut nodes = HashMap::new();
        flatten_tree(&root, &mut nodes);

        let instance_id = self.instance_id.unwrap_or_else(|| self.adapter_type.clone());
        MockAdapter {
            adapter_type: self.adapter_type,
            instance_id,
            root_data: Arc::new(root),
            nodes: Arc::new(nodes),
            capabilities: self.capabilities,
            actions_by_type: Arc::new(self.actions_by_type),
        }
    }
}

fn flatten_tree(data: &MockNodeData, map: &mut HashMap<String, MockNodeData>) {
    map.insert(data.id.clone(), data.clone());
    for child in &data.children {
        flatten_tree(child, map);
    }
}

// ---------------------------------------------------------------------------
// MockAdapter
// ---------------------------------------------------------------------------

pub struct MockAdapter {
    adapter_type: String,
    instance_id: String,
    root_data: Arc<MockNodeData>,
    nodes: Arc<HashMap<String, MockNodeData>>,
    capabilities: AdapterCapabilities,
    actions_by_type: Arc<HashMap<String, Vec<NodeAction>>>,
}

#[async_trait]
impl ContentAdapter for MockAdapter {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn adapter_type(&self) -> &str {
        &self.adapter_type
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    async fn root(&self) -> Result<Box<dyn Node>> {
        Ok(Box::new(MockNode {
            data: (*self.root_data).clone(),
            nodes: Arc::clone(&self.nodes),
        }))
    }

    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
        let data = self
            .nodes
            .get(id)
            .ok_or_else(|| ContentError::NotFound(id.to_string()))?;
        Ok(Box::new(MockNode {
            data: data.clone(),
            nodes: Arc::clone(&self.nodes),
        }))
    }

    fn capabilities(&self) -> AdapterCapabilities {
        self.capabilities.clone()
    }

    fn actions_for_type(&self, node_type: &NodeType) -> Vec<NodeAction> {
        self.actions_by_type
            .get(&node_type.type_id)
            .cloned()
            .unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// MockNode
// ---------------------------------------------------------------------------

struct MockNode {
    data: MockNodeData,
    nodes: Arc<HashMap<String, MockNodeData>>,
}

#[async_trait]
impl Node for MockNode {
    fn id(&self) -> &str {
        &self.data.id
    }

    fn label(&self) -> &str {
        &self.data.label
    }

    fn node_type(&self) -> &NodeType {
        &self.data.node_type
    }

    fn metadata(&self) -> &Metadata {
        &self.data.metadata
    }

    fn children_types(&self) -> Vec<NodeType> {
        self.data.child_types.clone()
    }

    async fn list(&self, params: ListParams) -> Result<ListResult> {
        let page = params.page.unwrap_or(PageRequest { offset: 0, limit: u32::MAX });
        let items: Vec<NodeSummary> = self
            .data
            .children
            .iter()
            .filter(|c| c.node_type.type_id == params.node_type.type_id)
            .skip(page.offset as usize)
            .take(page.limit as usize)
            .map(|c| NodeSummary {
                id: c.id.clone(),
                label: c.label.clone(),
                node_type: c.node_type.clone(),
                metadata: c.metadata.clone(),
                has_children: None,
            })
            .collect();

        let total = self
            .data
            .children
            .iter()
            .filter(|c| c.node_type.type_id == params.node_type.type_id)
            .count() as u64;

        let returned = items.len() as u32;
        let has_next = (page.offset as u64) + (returned as u64) < total;
        let has_prev = page.offset > 0;

        Ok(ListResult {
            items,
            applied_sort: Vec::new(),
            page: Some(PageInfo {
                offset: page.offset,
                limit: page.limit,
                total: Some(total),
                has_next,
                has_prev,
            }),
            batch_download_available: false,
            downloaded: vec![],
        })
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        let data = self
            .data
            .children
            .iter()
            .find(|c| c.id == id)
            .ok_or_else(|| ContentError::NotFound(id.to_string()))?;
        Ok(Box::new(MockNode {
            data: data.clone(),
            nodes: Arc::clone(&self.nodes),
        }))
    }

    fn content(&self) -> Option<&dyn Content> {
        if self.data.content_text.is_some() {
            Some(self)
        } else {
            None
        }
    }
}

#[async_trait]
impl Content for MockNode {
    fn node_type(&self) -> &NodeType {
        &self.data.node_type
    }

    fn version(&self) -> Option<&str> {
        None
    }

    async fn read(&self) -> Result<Vec<u8>> {
        match &self.data.content_text {
            Some(text) => Ok(text.as_bytes().to_vec()),
            None => Err(ContentError::NotSupported("No content".into())),
        }
    }
}

// ---------------------------------------------------------------------------
// Convenience node types
// ---------------------------------------------------------------------------

pub fn default_node_type() -> NodeType {
    NodeType {
        type_id: "mock:root".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Root".into(),
    }
}

pub fn issue_type() -> NodeType {
    NodeType {
        type_id: "mock:issue".into(),
        mime_type: "text/plain".into(),
        syntax: None,
        file_extension: ".txt".into(),
        display_name: "Issue".into(),
    }
}

pub fn comment_type() -> NodeType {
    NodeType {
        type_id: "mock:comment".into(),
        mime_type: "text/plain".into(),
        syntax: None,
        file_extension: ".txt".into(),
        display_name: "Comment".into(),
    }
}

// ---------------------------------------------------------------------------
// Tests for the mock itself
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_adapter_root_and_list() {
        let adapter = MockAdapterBuilder::new("test")
            .node(
                MockNodeData::new("root", "Root")
                    .child_type(issue_type())
                    .child(
                        MockNodeData::new("ISS-1", "First issue")
                            .node_type(issue_type())
                            .meta("status", "Open"),
                    )
                    .child(
                        MockNodeData::new("ISS-2", "Second issue")
                            .node_type(issue_type())
                            .meta("status", "Closed"),
                    ),
            )
            .build();

        assert_eq!(adapter.adapter_type(), "test");

        let root = adapter.root().await.unwrap();
        assert_eq!(root.id(), "root");
        assert_eq!(root.children_types().len(), 1);

        let result = root
            .list(ListParams {
                node_type: issue_type(),
                query: None,
                sort: vec![],
                page: Some(PageRequest { offset: 0, limit: 50 }),
                download: false,
            })
            .await
            .unwrap();

        assert_eq!(result.items.len(), 2);
        assert_eq!(result.page.and_then(|p| p.total), Some(2));
        assert_eq!(result.items[0].id, "ISS-1");
        assert_eq!(result.items[0].label, "First issue");
        assert_eq!(result.items[1].id, "ISS-2");
    }

    #[tokio::test]
    async fn mock_adapter_get_by_id() {
        let adapter = MockAdapterBuilder::new("test")
            .node(
                MockNodeData::new("root", "Root")
                    .child_type(issue_type())
                    .child(
                        MockNodeData::new("ISS-1", "Bug fix")
                            .node_type(issue_type())
                            .content("Fix the thing"),
                    ),
            )
            .build();

        let node = adapter.get_by_id("ISS-1").await.unwrap();
        assert_eq!(node.label(), "Bug fix");

        let text = node.content().unwrap().read_text().await.unwrap();
        assert_eq!(text, "Fix the thing");

        assert!(adapter.get_by_id("NOPE").await.is_err());
    }

    #[tokio::test]
    async fn mock_adapter_nested_children() {
        let adapter = MockAdapterBuilder::new("test")
            .node(
                MockNodeData::new("root", "Root")
                    .child_type(issue_type())
                    .child(
                        MockNodeData::new("ISS-1", "Issue 1")
                            .node_type(issue_type())
                            .child_type(comment_type())
                            .child(
                                MockNodeData::new("COM-1", "First comment")
                                    .node_type(comment_type())
                                    .content("Comment body"),
                            ),
                    ),
            )
            .build();

        // Drill into issue via get_by_id
        let issue = adapter.get_by_id("ISS-1").await.unwrap();
        assert_eq!(issue.children_types().len(), 1);
        assert_eq!(issue.children_types()[0].type_id, "mock:comment");

        // List comments
        let comments = issue
            .list(ListParams {
                node_type: comment_type(),
                query: None,
                sort: vec![],
                page: Some(PageRequest { offset: 0, limit: 50 }),
                download: false,
            })
            .await
            .unwrap();

        assert_eq!(comments.items.len(), 1);
        assert_eq!(comments.items[0].id, "COM-1");
    }

    #[tokio::test]
    async fn mock_adapter_pagination() {
        let mut root = MockNodeData::new("root", "Root").child_type(issue_type());
        for i in 0..10 {
            root = root.child(
                MockNodeData::new(format!("ISS-{i}"), format!("Issue {i}"))
                    .node_type(issue_type()),
            );
        }
        let adapter = MockAdapterBuilder::new("test").node(root).build();

        let root = adapter.root().await.unwrap();
        let result = root
            .list(ListParams {
                node_type: issue_type(),
                query: None,
                sort: vec![],
                page: Some(PageRequest { offset: 3, limit: 2 }),
                download: false,
            })
            .await
            .unwrap();

        assert_eq!(result.items.len(), 2);
        assert_eq!(result.items[0].id, "ISS-3");
        assert_eq!(result.items[1].id, "ISS-4");
        let page = result.page.unwrap();
        assert_eq!(page.total, Some(10));
        assert!(page.has_next);
        assert!(page.has_prev);
    }

    #[tokio::test]
    async fn mock_adapter_metadata() {
        let adapter = MockAdapterBuilder::new("test")
            .node(
                MockNodeData::new("root", "Root")
                    .child_type(issue_type())
                    .child(
                        MockNodeData::new("ISS-1", "Bug")
                            .node_type(issue_type())
                            .meta("status", "Open")
                            .meta("priority", "High"),
                    ),
            )
            .build();

        let node = adapter.get_by_id("ISS-1").await.unwrap();
        let meta = node.metadata();
        assert_eq!(meta.fields.len(), 2);
        assert_eq!(meta.fields[0].key, "status");
        assert_eq!(meta.fields[0].value, "Open");
        assert_eq!(meta.fields[1].key, "priority");
        assert_eq!(meta.fields[1].value, "High");
    }
}
