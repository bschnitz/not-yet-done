//! Issue-link nodes: the table of tickets linked to an issue.
//!
//! One `jira:link` node is one link of the parent issue, already oriented
//! from that issue's side — the row reads `<relation> <key>`, e.g. `blocks
//! ABC-2`. The node is the *link*, so `delete` unlinks (and never touches
//! either ticket); the ticket on the other end travels in the row's `key`
//! field, which lets a view point an issue-level action at it via
//! `node_id_from: key` (that is what `e e` / the preview do in `jira.yaml`).
//!
//! Sibling of [`super::comment`] and [`super::attachment`]: same composite-id
//! shape (`{issue_key}/link/{link_id}`), same leaf contract. Unlike those two
//! this module also owns its listing and column schema, because nothing else
//! needs them.

use std::sync::Arc;

use async_trait::async_trait;

use not_yet_done_content::*;

use crate::client::{JiraClient, JiraIssueLink};

use super::types::link_node_type;
use super::util::{browse_issue, other_err};

pub(super) fn link_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new("open_in_browser", "open in browser", InputSpec::None),
        NodeAction::new("delete", "unlink", InputSpec::None),
    ]
}

/// The link table's columns. All of them are carried in every row and sorted
/// locally ([`apply_sort`]) — Jira returns an issue's links in its own order
/// and offers no server-side ordering for them.
pub(super) fn link_columns() -> Vec<ColumnSchema> {
    vec![
        ColumnSchema::new("relation", "Relation"),
        ColumnSchema::new("key", "Key"),
        ColumnSchema::new("type", "Type"),
        ColumnSchema::new("status", "Status"),
        ColumnSchema::new("priority", "Priority"),
        ColumnSchema::new("assignee", "Assignee"),
        ColumnSchema::new("summary", "Summary"),
    ]
}

/// Metadata for one link, shared by the list row and the node so both render
/// the same cells. Keys mirror [`link_columns`]; `issue` (the parent key) is
/// extra detail, not a column.
fn link_metadata(issue_key: &str, l: &JiraIssueLink) -> Metadata {
    let field = |key: &str, label: &str, value: &str| MetadataField {
        key: key.into(),
        value: value.to_string(),
        display_label: label.into(),
        editable: false,
        allowed_values: None,
    };
    Metadata {
        fields: vec![
            field("relation", "Relation", &l.relation),
            field("key", "Key", &l.key),
            field("type", "Type", &l.issue_type),
            field("status", "Status", &l.status),
            field("priority", "Priority", &l.priority),
            field("assignee", "Assignee", &l.assignee),
            field("summary", "Summary", &l.summary),
            field("link_type", "Link Type", &l.type_name),
            field("issue", "Issue", issue_key),
        ],
    }
}

/// List an issue's links — the single fetch source behind the adapter's
/// [`ContentAdapter::childs`] declaration for `jira:link`. Reconstructs from
/// adapter state (`client`) plus the issue key (= the node's `id()`).
/// Rows arrive in Jira's own order; a requested sort is applied locally.
pub(super) async fn list_links(
    client: &Arc<JiraClient>,
    key: &str,
    params: ListParams,
) -> Result<ListResult> {
    let links = client.get_issue_links(key).await.map_err(other_err)?;

    let mut items: Vec<NodeSummary> = links
        .iter()
        .map(|l| NodeSummary {
            id: format!("{key}/link/{}", l.id),
            label: l.summary.clone(),
            node_type: link_node_type(),
            metadata: link_metadata(key, l),
            has_children: None,
        })
        .collect();

    let applied_sort = apply_sort(&mut items, &params.sort, &link_columns());

    Ok(ListResult {
        items,
        applied_sort,
        page: None,
        batch_download_available: false,
        downloaded: vec![],
    })
}

pub(super) struct JiraLinkNode {
    client: Arc<JiraClient>,
    /// The issue the link hangs off — the side the `relation` is phrased from.
    issue_key: String,
    /// Composite ID: `{issue_key}/link/{link_id}` for use in `get_by_id`.
    composite_id: String,
    link: JiraIssueLink,
    cached_metadata: Metadata,
}

impl JiraLinkNode {
    pub(super) fn new(client: Arc<JiraClient>, link: JiraIssueLink, issue_key: String) -> Self {
        let cached_metadata = link_metadata(&issue_key, &link);
        let composite_id = format!("{}/link/{}", issue_key, link.id);
        Self {
            client,
            issue_key,
            composite_id,
            link,
            cached_metadata,
        }
    }

    /// `<relation> <KEY>` — how the link reads from the parent issue, used in
    /// the delete prompt and the outcome message.
    fn phrase(&self) -> String {
        if self.link.relation.is_empty() {
            self.link.key.clone()
        } else {
            format!("{} {}", self.link.relation, self.link.key)
        }
    }
}

#[async_trait]
impl Node for JiraLinkNode {
    fn id(&self) -> &str {
        &self.composite_id
    }

    fn label(&self) -> &str {
        &self.link.summary
    }

    fn node_type(&self) -> &NodeType {
        static LINK_TYPE: std::sync::LazyLock<NodeType> = std::sync::LazyLock::new(link_node_type);
        &LINK_TYPE
    }

    fn metadata(&self) -> &Metadata {
        &self.cached_metadata
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        Err(ContentError::NotFound(format!("No child: {id}")))
    }

    fn content(&self) -> Option<&dyn Content> {
        None
    }

    /// `delete` opts into the frontend's generic delete plumbing: returning
    /// [`ActionDispatch::DeleteSelf`] makes the TUI show a `(y/n)` prompt and,
    /// on confirm, call `execute("delete", None)` here — which removes the
    /// link and lets the pane reload. The prompt spells out that only the link
    /// goes away, since a row looks like a ticket.
    async fn invoke_action(&self, name: &str, _ctx: &ActionContext) -> Result<ActionDispatch> {
        match name {
            "delete" => Ok(ActionDispatch::DeleteSelf {
                confirm: Some(format!(
                    "Remove link '{}' from {}? The ticket stays. (y/n)",
                    self.phrase(),
                    self.issue_key
                )),
            }),
            _ => Ok(ActionDispatch::Noop),
        }
    }

    async fn execute(&mut self, action_id: &str, input: ActionInput, _args: &ActionArgs) -> Result<ActionOutcome> {
        match (action_id, input) {
            // The linked ticket, not the parent issue — the row *is* the other
            // end of the link.
            ("open_in_browser", ActionInput::None) => {
                browse_issue(&self.client.base_url, &self.link.key)
            }
            ("delete", ActionInput::None) => {
                self.client
                    .delete_issue_link(&self.link.id)
                    .await
                    .map_err(other_err)?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("{}: unlinked ({})", self.issue_key, self.phrase())),
                })
            }
            (id, _) => Err(ContentError::NotSupported(format!(
                "JiraLinkNode action `{id}` not supported"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_client() -> Arc<JiraClient> {
        Arc::new(JiraClient::new("http://localhost:0", None, None, Some("test"), false).unwrap())
    }

    fn sample_link() -> JiraIssueLink {
        JiraIssueLink {
            id: "40001".into(),
            type_name: "Blocks".into(),
            relation: "blocks".into(),
            outward: true,
            key: "ABC-2".into(),
            summary: "Downstream cleanup".into(),
            status: "Open".into(),
            priority: "High".into(),
            issue_type: "Task".into(),
            assignee: "Ada Doe".into(),
        }
    }

    fn field<'a>(meta: &'a Metadata, key: &str) -> &'a str {
        meta.fields
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.value.as_str())
            .unwrap_or_else(|| panic!("field {key} present"))
    }

    #[test]
    fn link_node_metadata_describes_the_other_end() {
        let node = JiraLinkNode::new(test_client(), sample_link(), "ABC-1".into());
        assert_eq!(node.id(), "ABC-1/link/40001");
        assert_eq!(node.label(), "Downstream cleanup");

        let meta = node.metadata();
        assert_eq!(field(meta, "relation"), "blocks");
        assert_eq!(field(meta, "key"), "ABC-2");
        assert_eq!(field(meta, "type"), "Task");
        assert_eq!(field(meta, "status"), "Open");
        assert_eq!(field(meta, "priority"), "High");
        assert_eq!(field(meta, "assignee"), "Ada Doe");
        assert_eq!(field(meta, "summary"), "Downstream cleanup");
        assert_eq!(field(meta, "link_type"), "Blocks");
        // The parent issue, not the linked one.
        assert_eq!(field(meta, "issue"), "ABC-1");
    }

    #[test]
    fn every_declared_column_is_carried_in_the_row() {
        // The `in_rows` promise of `link_columns`, checked against the row
        // builder both share (`link_metadata`).
        let rows = [NodeSummary {
            id: "ABC-1/link/40001".into(),
            label: "Downstream cleanup".into(),
            node_type: link_node_type(),
            metadata: link_metadata("ABC-1", &sample_link()),
            has_children: None,
        }];
        let columns = link_columns();
        let complaints = not_yet_done_content::check_rows(&columns, &rows);
        assert!(complaints.is_empty(), "{complaints:?}");
    }

    #[test]
    fn link_node_no_content() {
        let node = JiraLinkNode::new(test_client(), sample_link(), "ABC-1".into());
        assert!(node.content().is_none());
    }

    #[tokio::test]
    async fn link_node_has_no_children() {
        let adapter = crate::adapter::test_adapter().await;
        let node = JiraLinkNode::new(test_client(), sample_link(), "ABC-1".into());
        assert!(not_yet_done_content::children::child_types(&adapter, &node).is_empty());
    }

    #[test]
    fn link_node_declares_open_and_unlink_actions() {
        let actions = link_actions();
        let ids: Vec<&str> = actions.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["open_in_browser", "delete"]);
        assert!(matches!(actions[0].input, InputSpec::None));
        assert!(matches!(actions[1].input, InputSpec::None));
    }

    #[tokio::test]
    async fn delete_prompt_names_the_relation_and_spares_the_ticket() {
        let node = JiraLinkNode::new(test_client(), sample_link(), "ABC-1".into());
        let dispatch = node
            .invoke_action("delete", &ActionContext::default())
            .await
            .unwrap();
        match dispatch {
            ActionDispatch::DeleteSelf { confirm: Some(p) } => {
                assert!(p.contains("blocks ABC-2"), "{p}");
                assert!(p.contains("ABC-1"), "{p}");
                assert!(p.contains("ticket stays"), "{p}");
            }
            other => panic!("expected DeleteSelf with a prompt, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn delete_prompt_falls_back_to_the_bare_key() {
        // An instance that leaves a link type's phrasings blank still gets a
        // readable prompt.
        let mut link = sample_link();
        link.relation = String::new();
        let node = JiraLinkNode::new(test_client(), link, "ABC-1".into());
        let dispatch = node
            .invoke_action("delete", &ActionContext::default())
            .await
            .unwrap();
        match dispatch {
            ActionDispatch::DeleteSelf { confirm: Some(p) } => {
                assert!(p.contains("'ABC-2'"), "{p}");
            }
            other => panic!("expected DeleteSelf with a prompt, got {other:?}"),
        }
    }
}
