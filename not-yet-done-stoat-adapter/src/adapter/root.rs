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

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use not_yet_done_content::{
    ContentError, ListParams, ListResult, Metadata, MetadataField, Node, NodeSummary, NodeType,
    Result,
};

use super::types::{channel_type, root_type, server_type};
use crate::gateway::StoatState;

pub(super) struct StoatRoot {
    pub(super) connection_name: String,
    pub(super) state: Arc<RwLock<StoatState>>,
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

impl StoatRoot {
    async fn list_servers(&self) -> ListResult {
        let state = self.state.read().await;
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

    async fn list_dms(&self) -> ListResult {
        let state = self.state.read().await;
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

    fn children_types(&self) -> Vec<NodeType> {
        vec![server_type().clone(), channel_type().clone()]
    }

    async fn list(&self, params: ListParams) -> Result<ListResult> {
        match params.node_type.type_id.as_str() {
            "stoat:server" => Ok(self.list_servers().await),
            "stoat:channel" => Ok(self.list_dms().await),
            other => Err(ContentError::NotSupported(format!(
                "StoatRoot only lists stoat:server / stoat:channel, got {other}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::{Channel, Server};

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

    fn seeded_root() -> StoatRoot {
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
        StoatRoot {
            connection_name: "chat".into(),
            state: Arc::new(RwLock::new(st)),
        }
    }

    #[tokio::test]
    async fn lists_servers() {
        let root = seeded_root();
        let res = root
            .list(ListParams {
                node_type: server_type().clone(),
                query: None,
                sort: Vec::new(),
                page: None,
                download: false,
                group_by: None,
            })
            .await
            .unwrap();
        assert_eq!(res.items.len(), 1);
        assert_eq!(res.items[0].id, "S1");
        assert_eq!(res.items[0].has_children, Some(true));
    }

    #[tokio::test]
    async fn lists_dms_only_not_server_channels() {
        let root = seeded_root();
        let res = root
            .list(ListParams {
                node_type: channel_type().clone(),
                query: None,
                sort: Vec::new(),
                page: None,
                download: false,
                group_by: None,
            })
            .await
            .unwrap();
        let ids: Vec<&str> = res.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["D1"]);
        assert_eq!(res.items[0].has_children, Some(true));
    }
}
