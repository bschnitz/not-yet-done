//! `stoat:server` — a chat server (guild). Its children are split across
//! two branches at the same tree level:
//!
//! - `stoat:category` — the server's channel categories, in order.
//! - `stoat:channel` — the **uncategorized** channels (those in
//!   `server.channels` that no category claims), so nothing is hidden
//!   when a server only partially categorises its channels.
//!
//! Pure `StoatState` read: the server's `channels[]` list fixes the
//! order, and each id is resolved against the channel map. Voice channels
//! are listed but marked childless (`has_children: Some(false)`) — LiveKit
//! audio is out of scope, so there's nothing to drill into. The channel
//! *contents* (messages) are a REST pull handled one level down by
//! [`StoatChannelNode`](super::channel).

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use not_yet_done_content::{
    ContentError, ListParams, ListResult, Metadata, MetadataField, Node, NodeSummary, NodeType,
    Result,
};

use super::category::category_composite_id;
use super::types::{category_type, channel_type, server_type};
use crate::gateway::StoatState;
use crate::gateway::protocol::Channel;

/// Build the `NodeSummary` for a channel row. Shared by the server's
/// uncategorized branch and [`StoatCategoryNode`](super::category) so a
/// channel renders identically wherever it sits in the tree.
pub(super) fn channel_summary(c: &Channel) -> NodeSummary {
    let is_voice = c.channel_type == "VoiceChannel";
    let label = c.name.clone().unwrap_or_else(|| c.id.clone());
    NodeSummary {
        id: c.id.clone(),
        label: label.clone(),
        node_type: channel_type().clone(),
        metadata: Metadata {
            fields: vec![
                MetadataField {
                    key: "name".into(),
                    value: label,
                    display_label: "Name".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "type".into(),
                    value: c.channel_type.clone(),
                    display_label: "Type".into(),
                    editable: false,
                    allowed_values: None,
                },
            ],
        },
        // Voice channels have no readable content; text channels are
        // expandable (drillable to messages) when they've seen activity.
        has_children: Some(if is_voice {
            false
        } else {
            c.last_message_id.is_some()
        }),
    }
}

pub(super) struct StoatServerNode {
    state: Arc<RwLock<StoatState>>,
    server_id: String,
    name: String,
    metadata: Metadata,
}

impl StoatServerNode {
    pub(super) fn new(state: Arc<RwLock<StoatState>>, server_id: String, name: String) -> Self {
        let metadata = Metadata {
            fields: vec![
                MetadataField {
                    key: "name".into(),
                    value: name.clone(),
                    display_label: "Name".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "id".into(),
                    value: server_id.clone(),
                    display_label: "ID".into(),
                    editable: false,
                    allowed_values: None,
                },
            ],
        };
        Self {
            state,
            server_id,
            name,
            metadata,
        }
    }
}

#[async_trait]
impl Node for StoatServerNode {
    fn id(&self) -> &str {
        &self.server_id
    }

    fn label(&self) -> &str {
        &self.name
    }

    fn node_type(&self) -> &NodeType {
        server_type()
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn children_types(&self) -> Vec<NodeType> {
        vec![category_type().clone(), channel_type().clone()]
    }

    async fn list(&self, params: ListParams) -> Result<ListResult> {
        let state = self.state.read().await;
        let Some(server) = state.servers.get(&self.server_id) else {
            return Ok(empty_result());
        };

        let items: Vec<NodeSummary> = match params.node_type.type_id.as_str() {
            // Categories, in server order. An empty category still shows
            // (faithful to server state) but is marked childless.
            "stoat:category" => server
                .categories
                .iter()
                .map(|cat| NodeSummary {
                    id: category_composite_id(&self.server_id, &cat.id),
                    label: cat.title.clone(),
                    node_type: category_type().clone(),
                    metadata: Metadata {
                        fields: vec![MetadataField {
                            key: "name".into(),
                            value: cat.title.clone(),
                            display_label: "Name".into(),
                            editable: false,
                            allowed_values: None,
                        }],
                    },
                    has_children: Some(!cat.channels.is_empty()),
                })
                .collect(),
            // Uncategorized channels only — anything a category claims is
            // listed one level down by the category node instead.
            "stoat:channel" => {
                let categorized: HashSet<&str> = server
                    .categories
                    .iter()
                    .flat_map(|cat| cat.channels.iter().map(String::as_str))
                    .collect();
                server
                    .channels
                    .iter()
                    .filter(|cid| !categorized.contains(cid.as_str()))
                    .filter_map(|cid| state.channels.get(cid))
                    .map(channel_summary)
                    .collect()
            }
            other => {
                return Err(ContentError::NotSupported(format!(
                    "StoatServerNode lists stoat:category or stoat:channel, got {other}"
                )));
            }
        };

        Ok(ListResult {
            items,
            applied_sort: Vec::new(),
            page: None,
            batch_download_available: false,
            downloaded: Vec::new(),
        })
    }
}

fn empty_result() -> ListResult {
    ListResult {
        items: Vec::new(),
        applied_sort: Vec::new(),
        page: None,
        batch_download_available: false,
        downloaded: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::{Category, Channel, Server};

    fn channel(id: &str, kind: &str, last: Option<&str>) -> Channel {
        Channel {
            id: id.into(),
            channel_type: kind.into(),
            server: Some("S1".into()),
            name: Some(format!("chan-{id}")),
            last_message_id: last.map(Into::into),
            recipients: None,
        }
    }

    fn list_params(ty: &NodeType) -> ListParams {
        ListParams {
            node_type: ty.clone(),
            query: None,
            sort: Vec::new(),
            page: None,
            download: false,
            group_by: None,
        }
    }

    #[tokio::test]
    async fn lists_channels_in_server_order_with_voice_childless() {
        // No categories → every channel is uncategorized and lists here.
        let mut st = StoatState::default();
        st.apply_ready(
            vec![],
            vec![Server {
                id: "S1".into(),
                name: "Guild".into(),
                channels: vec!["C1".into(), "C2".into(), "V1".into()],
                categories: vec![],
                owner: None,
            }],
            vec![
                channel("C1", "TextChannel", Some("01ARZ3NDEKTSV4RRFFQ69G5FAV")),
                channel("C2", "TextChannel", None),
                channel("V1", "VoiceChannel", None),
            ],
        );
        let node = StoatServerNode::new(Arc::new(RwLock::new(st)), "S1".into(), "Guild".into());
        let res = node.list(list_params(channel_type())).await.unwrap();
        let ids: Vec<&str> = res.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["C1", "C2", "V1"]);
        // Text channel with activity is expandable, idle one is not …
        assert_eq!(res.items[0].has_children, Some(true));
        assert_eq!(res.items[1].has_children, Some(false));
        // … and the voice channel is never expandable.
        assert_eq!(res.items[2].has_children, Some(false));
    }

    #[tokio::test]
    async fn splits_categories_from_uncategorized_channels() {
        // C1 sits in a category; C2 is uncategorized. The server level
        // lists the category (under stoat:category) and only C2 (under
        // stoat:channel).
        let mut st = StoatState::default();
        st.apply_ready(
            vec![],
            vec![Server {
                id: "S1".into(),
                name: "Guild".into(),
                channels: vec!["C1".into(), "C2".into()],
                categories: vec![Category {
                    id: "cat1".into(),
                    title: "General".into(),
                    channels: vec!["C1".into()],
                }],
                owner: None,
            }],
            vec![
                channel("C1", "TextChannel", Some("01ARZ3NDEKTSV4RRFFQ69G5FAV")),
                channel("C2", "TextChannel", None),
            ],
        );
        let node = StoatServerNode::new(Arc::new(RwLock::new(st)), "S1".into(), "Guild".into());

        // Category branch: one category, expandable, composite id.
        let cats = node.list(list_params(category_type())).await.unwrap();
        assert_eq!(cats.items.len(), 1);
        assert_eq!(cats.items[0].label, "General");
        assert_eq!(cats.items[0].id, "S1/cat/cat1");
        assert_eq!(cats.items[0].has_children, Some(true));

        // Channel branch: only the uncategorized C2 surfaces here.
        let chans = node.list(list_params(channel_type())).await.unwrap();
        let ids: Vec<&str> = chans.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["C2"]);
    }
}
