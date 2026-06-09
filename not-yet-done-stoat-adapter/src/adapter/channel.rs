//! `stoat:channel` — a text channel (or DM/group), whose children are
//! its messages.
//!
//! Structure (id/name) comes from `StoatState`; the message list is a
//! REST pull (`GET …/messages`). Phase 1 fetches the most-recent page
//! only — backfill of older messages is left to a later cursor-pagination
//! pass. Voice channels never reach this listing path (the server node
//! marks them childless), so `list()` here always means "text messages".

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use not_yet_done_content::{
    ActionInput, ActionOutcome, ContentError, EditorPrep, InputSpec, ListParams, ListResult,
    Metadata, MetadataField, Node, NodeAction, NodeSummary, NodeType, Result,
};

use super::members::{MemberCache, channel_user_map};
use super::mentions;
use super::message::{StoatMessageNode, composite_id};
use super::other_err;
use super::types::{channel_type, message_type};
use crate::client::StoatClient;
use crate::gateway::StoatState;

/// How many messages to pull for the latest page.
const DEFAULT_MESSAGE_LIMIT: u32 = 50;

/// Actions a channel exposes. `send_message` is the `create`-style action
/// the message-list view triggers (parent = channel, child = message): it
/// opens an empty editor and posts the buffer as a new message.
pub(super) fn channel_actions() -> Vec<NodeAction> {
    vec![NodeAction::new("send_message", "send", InputSpec::Editor)]
}

pub(super) struct StoatChannelNode {
    client: Arc<StoatClient>,
    channel_id: String,
    name: String,
    metadata: Metadata,
    /// Live tree state — used to resolve this channel's server (and, for
    /// DMs, its recipients) when building the mention completion set.
    state: Arc<RwLock<StoatState>>,
    /// Shared per-server member cache backing mention autocomplete.
    members: Arc<MemberCache>,
}

impl StoatChannelNode {
    pub(super) fn new(
        client: Arc<StoatClient>,
        channel_id: String,
        name: String,
        state: Arc<RwLock<StoatState>>,
        members: Arc<MemberCache>,
    ) -> Self {
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
                    value: channel_id.clone(),
                    display_label: "ID".into(),
                    editable: false,
                    allowed_values: None,
                },
            ],
        };
        Self {
            client,
            channel_id,
            name,
            metadata,
            state,
            members,
        }
    }

    /// The `id → username` map of users mentionable in this channel
    /// (server members, or DM recipients). Backs both message-body
    /// display and the `@uu-…` edit slugs.
    async fn user_map(&self) -> Arc<HashMap<String, String>> {
        channel_user_map(&self.state, &self.members, &self.client, &self.channel_id).await
    }
}

#[async_trait]
impl Node for StoatChannelNode {
    fn id(&self) -> &str {
        &self.channel_id
    }

    fn label(&self) -> &str {
        &self.name
    }

    fn node_type(&self) -> &NodeType {
        channel_type()
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn children_types(&self) -> Vec<NodeType> {
        vec![message_type().clone()]
    }

    fn actions(&self) -> Vec<NodeAction> {
        channel_actions()
    }

    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        match action_id {
            // Compose starts on an empty buffer followed by the CACHE
            // section, so `@uu-…` mention slugs are available to copy.
            "send_message" => {
                let users = self.user_map().await;
                let table = mentions::user_table(&users);
                Ok(EditorPrep {
                    template: mentions::cache_section(&table),
                    version: String::new(),
                    suffix: ".md".into(),
                })
            }
            other => Err(ContentError::NotSupported(format!(
                "prepare: unknown action {other}"
            ))),
        }
    }

    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        match (action_id, input) {
            ("send_message", ActionInput::Edited { text, .. }) => {
                // Drop the CACHE section, then translate `@uu-slug`
                // mentions back to the wire `<@ID>` form.
                let body = mentions::strip_cache_section(&text);
                let users = self.user_map().await;
                let table = mentions::user_table(&users);
                let content = mentions::parse_slugs(body.trim(), &table)
                    .map_err(|slug| other_err(format!("unknown mention slug @{slug}")))?;
                let content = content.trim().to_string();
                if content.is_empty() {
                    return Ok(ActionOutcome::NoChanges);
                }
                let message_id = self
                    .client
                    .send_message(&self.channel_id, &content)
                    .await
                    .map_err(other_err)?;
                // Return the created message's composite id so callers can
                // navigate to it (and the `commit_on_save` editor flow can
                // retarget later saves at this message's `edit_message`).
                Ok(ActionOutcome::Navigate {
                    node_id: composite_id(&self.channel_id, &message_id),
                    node_type: message_type().clone(),
                })
            }
            (other, _) => Err(ContentError::NotSupported(format!(
                "execute: unsupported action/input for {other}"
            ))),
        }
    }

    async fn list(&self, params: ListParams) -> Result<ListResult> {
        if params.node_type.type_id != "stoat:message" {
            return Err(ContentError::NotSupported(format!(
                "StoatChannelNode only lists stoat:message, got {}",
                params.node_type.type_id
            )));
        }
        let limit = params
            .page
            .map(|p| p.limit)
            .filter(|&l| l > 0)
            .unwrap_or(DEFAULT_MESSAGE_LIMIT);

        let views = self
            .client
            .list_messages(&self.channel_id, limit, None)
            .await
            .map_err(super::other_err)?;

        // Resolve the channel's mentionable users once for the whole
        // page; each message node renders `<@ID>` → `@username` against it.
        let users = self.user_map().await;

        let items = views
            .into_iter()
            .map(|v| {
                let id = composite_id(&v.channel_id, &v.id);
                let node = StoatMessageNode::new(Arc::clone(&self.client), v, Arc::clone(&users));
                NodeSummary {
                    id,
                    label: node.label().to_string(),
                    node_type: message_type().clone(),
                    metadata: node.metadata().clone(),
                    // Messages are leaves.
                    has_children: Some(false),
                }
            })
            .collect();

        Ok(ListResult {
            items,
            applied_sort: Vec::new(),
            // Phase 1 returns a single (latest) page; no offset pagination.
            page: None,
            batch_download_available: false,
            downloaded: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_node_metadata_and_children_type() {
        // No HTTP performed — we only inspect the synchronous surface.
        let client = crate::client::StoatClient::from_session(
            "https://chat.example.invalid",
            crate::client::StoatSession {
                token: "synthetic".into(),
                user_id: "U0".into(),
                session_id: "S0".into(),
                session_name: "test".into(),
            },
        )
        .expect("client");
        let node = StoatChannelNode::new(
            client,
            "C1".into(),
            "general".into(),
            Arc::new(RwLock::new(StoatState::default())),
            Arc::new(MemberCache::default()),
        );
        assert_eq!(node.id(), "C1");
        assert_eq!(node.label(), "general");
        assert_eq!(node.metadata().fields[0].value, "general");
        assert_eq!(node.children_types()[0].type_id, "stoat:message");
    }
}
