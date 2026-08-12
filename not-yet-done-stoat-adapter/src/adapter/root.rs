//! Root node of the Stoat tree.
//!
//! Lists the two top-level kinds from [`StoatState`] (populated by the
//! gateway's `Ready`): `stoat:server` for chat servers and `stoat:channel`
//! for direct-message / group channels. A view picks which one it shows
//! via its top-level `node_type` (the bundled view browses servers; point
//! a view at `stoat:channel` to browse DMs). The structure is a pure,
//! synchronous state read — no network await — so the tree renders the
//! instant `Ready` lands and a manual `r`-reload picks up changes until
//! the Phase 2 live layer pushes them.

use async_trait::async_trait;

use not_yet_done_content::{ListResult, Metadata, MetadataField, Node, NodeSummary, NodeType};

use super::types::{channel_type, root_type, server_type};
use crate::gateway::StoatState;

pub(super) struct StoatRoot {
    pub(super) connection_name: String,
}

/// One metadata field shorthand (everything on these nodes is read-only).
fn field(key: &str, value: String, label: &str) -> MetadataField {
    MetadataField {
        key: key.into(),
        value,
        display_label: label.into(),
        editable: false,
        allowed_values: None,
    }
}

/// List the chat servers from a state snapshot. Shared by the root node's
/// legacy `list` and the adapter's `childs` fetcher, so both project a
/// `stoat:server` row identically.
pub(super) fn list_servers_from(state: &StoatState) -> ListResult {
    let items = state
        .servers
        .values()
        .map(|s| NodeSummary {
            id: s.id.clone(),
            label: s.name.clone(),
            node_type: server_type().clone(),
            metadata: Metadata {
                fields: vec![
                    field("name", s.name.clone(), "Name"),
                    field("id", s.id.clone(), "ID"),
                    super::unread_field(state.is_server_unread(&s.id)),
                ],
            },
            has_children: Some(!s.channels.is_empty()),
        })
        .collect();
    ListResult {
        items,
        applied_sort: Vec::new(),
        page: None,
        batch_download_available: false,
        downloaded: Vec::new(),
    }
}

/// List the direct-message / group channels from a state snapshot. Shared
/// by the root node's legacy `list` and the adapter's `childs` fetcher.
pub(super) fn list_dms_from(state: &StoatState) -> ListResult {
    let items = state
        .dm_channels()
        .map(|c| {
            // Group DMs carry a `name`; 1:1 DMs don't, so fall back to
            // the channel id (recipient-name resolution is a later
            // nicety, not Phase 1).
            let label = c.name.clone().unwrap_or_else(|| c.id.clone());
            NodeSummary {
                id: c.id.clone(),
                label: label.clone(),
                node_type: channel_type().clone(),
                metadata: Metadata {
                    fields: vec![
                        field("name", label, "Name"),
                        field("type", c.channel_type.clone(), "Type"),
                    ],
                },
                has_children: Some(c.last_message_id.is_some()),
            }
        })
        .collect();
    ListResult {
        items,
        applied_sort: Vec::new(),
        page: None,
        batch_download_available: false,
        downloaded: Vec::new(),
    }
}

#[async_trait]
impl Node for StoatRoot {
    fn id(&self) -> &str {
        "root"
    }

    fn label(&self) -> &str {
        &self.connection_name
    }

    fn node_type(&self) -> &NodeType {
        root_type()
    }

    fn metadata(&self) -> &Metadata {
        static EMPTY: Metadata = Metadata { fields: Vec::new() };
        &EMPTY
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::StoatAdapter;
    use crate::client::StoatClient;
    use crate::gateway::protocol::{Channel, Server};
    use not_yet_done_content::{ListParams, Node, children};
    use std::sync::Arc;
    use tokio::sync::RwLock;

    /// A synthetic client — no HTTP is performed in these list tests.
    fn test_client() -> Arc<StoatClient> {
        StoatClient::from_session(
            "https://chat.example.invalid",
            crate::client::StoatSession {
                token: "synthetic".into(),
                user_id: "U0".into(),
                session_id: "S0".into(),
                session_name: "test".into(),
            },
        )
        .expect("client")
    }

    fn dm(id: &str, last: Option<&str>) -> Channel {
        Channel {
            id: id.into(),
            channel_type: "DirectMessage".into(),
            server: None,
            name: None,
            last_message_id: last.map(Into::into),
            recipients: Some(vec!["U1".into(), "U2".into()]),
        }
    }

    /// Build a root node and an adapter that share the same seeded state,
    /// so the generic `children::list` free function lists against the node.
    fn seeded_root() -> (StoatRoot, StoatAdapter) {
        let mut st = StoatState::default();
        st.apply_ready(
            vec![],
            vec![Server {
                id: "S1".into(),
                name: "Guild".into(),
                channels: vec!["C1".into()],
                categories: vec![],
                owner: None,
            }],
            vec![
                Channel {
                    id: "C1".into(),
                    channel_type: "TextChannel".into(),
                    server: Some("S1".into()),
                    name: Some("general".into()),
                    last_message_id: None,
                    recipients: None,
                },
                dm("D1", Some("01ARZ3NDEKTSV4RRFFQ69G5FAV")),
            ],
        );
        let state = Arc::new(RwLock::new(st));
        let node = StoatRoot {
            connection_name: "chat".into(),
        };
        let adapter = StoatAdapter::for_test(state, test_client());
        (node, adapter)
    }

    #[tokio::test]
    async fn lists_servers() {
        let (root, adapter) = seeded_root();
        let node: &dyn Node = &root;
        let res = children::list(
            &adapter,
            node,
            ListParams {
                node_type: server_type().clone(),
                query: None,
                sort: Vec::new(),
                page: None,
                download: false,
                group_by: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(res.items.len(), 1);
        assert_eq!(res.items[0].id, "S1");
        assert_eq!(res.items[0].has_children, Some(true));
    }

    #[tokio::test]
    async fn lists_dms_only_not_server_channels() {
        let (root, adapter) = seeded_root();
        let node: &dyn Node = &root;
        let res = children::list(
            &adapter,
            node,
            ListParams {
                node_type: channel_type().clone(),
                query: None,
                sort: Vec::new(),
                page: None,
                download: false,
                group_by: None,
            },
        )
        .await
        .unwrap();
        let ids: Vec<&str> = res.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["D1"]);
        assert_eq!(res.items[0].has_children, Some(true));
    }
}
