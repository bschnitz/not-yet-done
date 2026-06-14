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
    ActionInput, ActionOutcome, ContentError, FormFieldSpec, InputSpec, ListParams, ListResult,
    Metadata, MetadataField, Node, NodeAction, NodeSummary, NodeType, Result,
};

use super::server::channel_summary;
use super::types::{category_type, channel_type};
use super::{form_field, other_err};
use crate::client::StoatClient;
use crate::gateway::StoatState;
use crate::gateway::protocol::Category;

const CAT_MARKER: &str = "/cat/";

/// Actions a category exposes: create a channel directly inside it. Kept
/// in lockstep with [`StoatCategoryNode::actions`].
pub(super) fn category_actions() -> Vec<NodeAction> {
    vec![NodeAction::new(
        "create_channel",
        "new channel",
        InputSpec::Form {
            fields: vec![FormFieldSpec::text("name", "Channel name")],
        },
    )]
}

/// Append a fresh, empty category to a server's full category list.
/// Stoat edits categories as a whole-list replacement, so creating one
/// means sending the existing list plus the new entry.
pub(super) fn categories_with_new(existing: &[Category], id: &str, title: &str) -> Vec<Category> {
    let mut out = existing.to_vec();
    out.push(Category {
        id: id.to_string(),
        title: title.to_string(),
        channels: Vec::new(),
    });
    out
}

/// Return `existing` with `channel_id` added to the category `category_id`
/// (idempotent — already-present ids aren't duplicated). Other categories
/// pass through untouched. Used to drop a freshly-created channel into a
/// category via the full-list PATCH.
pub(super) fn categories_with_channel(
    existing: &[Category],
    category_id: &str,
    channel_id: &str,
) -> Vec<Category> {
    existing
        .iter()
        .cloned()
        .map(|mut cat| {
            if cat.id == category_id && !cat.channels.iter().any(|c| c == channel_id) {
                cat.channels.push(channel_id.to_string());
            }
            cat
        })
        .collect()
}

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
    client: Arc<StoatClient>,
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
        client: Arc<StoatClient>,
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
            client,
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

    fn actions(&self) -> Vec<NodeAction> {
        category_actions()
    }

    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        match action_id {
            // Two-step, because Stoat has no "create in category": make
            // the channel (lands uncategorized), then PATCH the server's
            // full category list with this channel added to us. Both
            // gateway events (ChannelCreate + ServerUpdate) refresh the
            // tree.
            "create_channel" => {
                let name = form_field(&input, "name")?;
                let channel_id = self
                    .client
                    .create_channel(&self.server_id, &name)
                    .await
                    .map_err(other_err)?;
                let existing = {
                    let state = self.state.read().await;
                    state
                        .servers
                        .get(&self.server_id)
                        .map(|s| s.categories.clone())
                        .unwrap_or_default()
                };
                let updated = categories_with_channel(&existing, &self.category_id, &channel_id);
                self.client
                    .update_server_categories(&self.server_id, &updated)
                    .await
                    .map_err(other_err)?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("Created channel #{name}")),
                })
            }
            other => Err(ContentError::NotSupported(format!(
                "execute: unknown action {other}"
            ))),
        }
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

    /// Synthetic client — the list test performs no HTTP.
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

    fn cat(id: &str, channels: &[&str]) -> Category {
        Category {
            id: id.into(),
            title: format!("title-{id}"),
            channels: channels.iter().map(|c| c.to_string()).collect(),
        }
    }

    #[test]
    fn categories_with_new_appends_empty_category() {
        let existing = vec![cat("c1", &["X"])];
        let out = categories_with_new(&existing, "c2", "New");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "c1");
        assert_eq!(out[1].id, "c2");
        assert_eq!(out[1].title, "New");
        assert!(out[1].channels.is_empty());
    }

    #[test]
    fn categories_with_channel_adds_to_target_only_and_is_idempotent() {
        let existing = vec![cat("c1", &["A"]), cat("c2", &["B"])];
        let out = categories_with_channel(&existing, "c2", "Z");
        assert_eq!(out[0].channels, vec!["A"]); // untouched
        assert_eq!(out[1].channels, vec!["B", "Z"]);
        // Re-applying the same channel is a no-op (no duplicate).
        let again = categories_with_channel(&out, "c2", "Z");
        assert_eq!(again[1].channels, vec!["B", "Z"]);
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
            test_client(),
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
