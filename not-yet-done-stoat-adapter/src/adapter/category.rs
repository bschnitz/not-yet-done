//! `stoat:category` — a channel category within a server. Its children
//! are the channels the category lists, in order.
//!
//! Categories are identified by a **composite id** `<server>/cat/<catid>`
//! (the only place that encoding is decoded is [`split_category_composite`]).
//! The category id alone is not enough: it is server-scoped, not always a
//! ULID, and resolving its channels needs the owning server's snapshot —
//! the composite carries the server id so `get_by_id` finds it
//! deterministically. This mirrors the `<channel>/msg/<ulid>` message
//! encoding.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use not_yet_done_content::{
    ContentError, ListParams, ListResult, Metadata, MetadataField, Node, NodeSummary, NodeType,
    Result,
};

use super::server::channel_summary;
use super::types::{category_type, channel_type};
use crate::gateway::StoatState;

const CAT_MARKER: &str = "/cat/";

/// Build the composite id for a category row: `<server>/cat/<catid>`.
pub(super) fn category_composite_id(server_id: &str, category_id: &str) -> String {
    format!("{server_id}{CAT_MARKER}{category_id}")
}

/// Decode a `<server>/cat/<catid>` composite back into its parts. Returns
/// `None` for any id that is not a category composite (channels, servers,
/// and `<channel>/msg/<ulid>` message ids all fall through).
pub(super) fn split_category_composite(id: &str) -> Option<(&str, &str)> {
    let (server, cat) = id.split_once(CAT_MARKER)?;
    if server.is_empty() || cat.is_empty() {
        return None;
    }
    Some((server, cat))
}

pub(super) struct StoatCategoryNode {
    state: Arc<RwLock<StoatState>>,
    /// The composite `<server>/cat/<catid>` — also what `id()` returns so
    /// tree paths stay consistent with the summary the server emitted.
    composite_id: String,
    server_id: String,
    category_id: String,
    title: String,
    metadata: Metadata,
}

impl StoatCategoryNode {
    pub(super) fn new(
        state: Arc<RwLock<StoatState>>,
        server_id: String,
        category_id: String,
        title: String,
    ) -> Self {
        let composite_id = category_composite_id(&server_id, &category_id);
        let metadata = Metadata {
            fields: vec![
                MetadataField {
                    key: "name".into(),
                    value: title.clone(),
                    display_label: "Name".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "id".into(),
                    value: category_id.clone(),
                    display_label: "ID".into(),
                    editable: false,
                    allowed_values: None,
                },
            ],
        };
        Self {
            state,
            composite_id,
            server_id,
            category_id,
            title,
            metadata,
        }
    }
}

#[async_trait]
impl Node for StoatCategoryNode {
    fn id(&self) -> &str {
        &self.composite_id
    }

    fn label(&self) -> &str {
        &self.title
    }

    fn node_type(&self) -> &NodeType {
        category_type()
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn children_types(&self) -> Vec<NodeType> {
        vec![channel_type().clone()]
    }

    async fn list(&self, params: ListParams) -> Result<ListResult> {
        if params.node_type.type_id != "stoat:channel" {
            return Err(ContentError::NotSupported(format!(
                "StoatCategoryNode only lists stoat:channel, got {}",
                params.node_type.type_id
            )));
        }
        let state = self.state.read().await;
        // Resolve the category against the live snapshot, then its channel
        // ids in declared order. A category whose server/id has vanished
        // (reconnect race) lists empty rather than erroring.
        let items: Vec<NodeSummary> = state
            .servers
            .get(&self.server_id)
            .and_then(|s| s.categories.iter().find(|c| c.id == self.category_id))
            .map(|cat| cat.channels.as_slice())
            .unwrap_or(&[])
            .iter()
            .filter_map(|cid| state.channels.get(cid))
            .map(channel_summary)
            .collect();

        Ok(ListResult {
            items,
            applied_sort: Vec::new(),
            page: None,
            batch_download_available: false,
            downloaded: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::{Category, Channel, Server};

    fn channel(id: &str, last: Option<&str>) -> Channel {
        Channel {
            id: id.into(),
            channel_type: "TextChannel".into(),
            server: Some("S1".into()),
            name: Some(format!("chan-{id}")),
            last_message_id: last.map(Into::into),
            recipients: None,
        }
    }

    #[test]
    fn composite_round_trips() {
        let id = category_composite_id("S1", "cat1");
        assert_eq!(id, "S1/cat/cat1");
        assert_eq!(split_category_composite(&id), Some(("S1", "cat1")));
    }

    #[test]
    fn split_rejects_non_category_ids() {
        // Plain channel/server ids and message composites are not categories.
        assert_eq!(split_category_composite("01ARZ3NDEKTSV4RRFFQ69G5FAV"), None);
        assert_eq!(split_category_composite("C1/msg/01ARZ3NDEKTSV4RRFFQ69G5FAV"), None);
        assert_eq!(split_category_composite("/cat/x"), None);
        assert_eq!(split_category_composite("S1/cat/"), None);
    }

    #[tokio::test]
    async fn lists_category_channels_in_order() {
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
                    channels: vec!["C2".into(), "C1".into()],
                }],
                owner: None,
            }],
            vec![
                channel("C1", Some("01ARZ3NDEKTSV4RRFFQ69G5FAV")),
                channel("C2", None),
            ],
        );
        let node = StoatCategoryNode::new(
            Arc::new(RwLock::new(st)),
            "S1".into(),
            "cat1".into(),
            "General".into(),
        );
        let res = node
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
        // Category order (C2 before C1), not server order.
        let ids: Vec<&str> = res.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["C2", "C1"]);
    }
}
