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
    ActionContext, ActionDispatch, ActionInput, ActionOutcome, ContentError, FormFieldSpec,
    InputSpec, ListResult, MarkedNode, Metadata, MetadataField, Node, NodeAction, NodeSummary,
    NodeType, Result,
};

use super::server::channel_summary;
use super::types::{category_type, channel_type};
use super::{form_field, other_err};
use crate::client::StoatClient;
use crate::gateway::StoatState;
use crate::gateway::protocol::Category;

const CAT_MARKER: &str = "/cat/";

/// Actions a category exposes: create a channel directly inside it,
/// rename the category, or accept a previously-cut channel (`paste-move`,
/// the move target — see [`move_marked_channel`]). Kept in lockstep with
/// [`StoatCategoryNode::actions`].
pub(super) fn category_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new(
            "create_channel",
            "new channel",
            InputSpec::Form {
                fields: vec![FormFieldSpec::text("name", "Channel name")],
            },
        ),
        NodeAction::new(
            "rename",
            "rename",
            InputSpec::Form {
                fields: vec![FormFieldSpec::text("name", "Category name")],
            },
        ),
        // Paste target only — the cut itself happens on a channel row.
        NodeAction::new("paste-move", "paste channel", InputSpec::None),
    ]
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

/// Return `existing` with the category `category_id` retitled to
/// `title`. Other categories (and every channel assignment) pass through
/// untouched — Stoat's server PATCH takes the whole list, so a rename is
/// "the same list with one title changed".
pub(super) fn categories_with_renamed(
    existing: &[Category],
    category_id: &str,
    title: &str,
) -> Vec<Category> {
    existing
        .iter()
        .cloned()
        .map(|mut cat| {
            if cat.id == category_id {
                cat.title = title.to_string();
            }
            cat
        })
        .collect()
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

/// Return `existing` with `channel_id` detached from every category and,
/// when `target` is `Some(cat_id)`, re-attached to that category. `None`
/// leaves it uncategorized. Stoat's server PATCH replaces the whole list,
/// so a move is "remove it from wherever it was, then place it" — one
/// rule covers category→category, category→uncategorized and the reverse.
/// Removing first makes the re-insert duplicate-free even when the source
/// and target category coincide (a no-op move).
pub(super) fn categories_with_channel_moved(
    existing: &[Category],
    channel_id: &str,
    target: Option<&str>,
) -> Vec<Category> {
    existing
        .iter()
        .cloned()
        .map(|mut cat| {
            cat.channels.retain(|c| c != channel_id);
            if target == Some(cat.id.as_str()) {
                cat.channels.push(channel_id.to_string());
            }
            cat
        })
        .collect()
}

/// Execute a cut/paste channel move: relocate the marked channel to
/// `target_category` (`None` = the server's uncategorized branch) within
/// `server_id`. Shared by the server / category / channel `paste-move`
/// handlers, which differ only in how they resolve the destination.
///
/// Validates that the marked node is a channel **belonging to this
/// server** — categories are server-scoped, so a cross-server paste is
/// rejected rather than silently dropping the channel. On success the
/// `ServerUpdate` echo refreshes the tree, so we return
/// [`ActionDispatch::Reload`] (which also clears the frontend's cut mark).
pub(super) async fn move_marked_channel(
    client: &StoatClient,
    state: &RwLock<StoatState>,
    server_id: &str,
    target_category: Option<&str>,
    marked: &MarkedNode,
) -> ActionDispatch {
    if marked.node_type.type_id != channel_type().type_id {
        return ActionDispatch::Error("Only channels can be cut and pasted".into());
    }
    let channel_id = marked.node_id.as_str();
    let categories = {
        let st = state.read().await;
        match st
            .channels
            .get(channel_id)
            .and_then(|c| c.server.as_deref())
        {
            Some(srv) if srv == server_id => {}
            Some(_) => {
                return ActionDispatch::Error("Cannot move a channel to a different server".into());
            }
            None => return ActionDispatch::Error("The cut channel no longer exists".into()),
        }
        st.servers
            .get(server_id)
            .map(|s| s.categories.clone())
            .unwrap_or_default()
    };
    let updated = categories_with_channel_moved(&categories, channel_id, target_category);
    match client.update_server_categories(server_id, &updated).await {
        Ok(()) => ActionDispatch::Reload,
        Err(e) => ActionDispatch::Error(format!("Move failed: {e}")),
    }
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
            // Rename: edit the title in the server's full category list and
            // PATCH it back (no per-category endpoint). The `ServerUpdate`
            // echo refreshes the tree.
            "rename" => {
                let name = form_field(&input, "name")?;
                let existing = {
                    let state = self.state.read().await;
                    state
                        .servers
                        .get(&self.server_id)
                        .map(|s| s.categories.clone())
                        .unwrap_or_default()
                };
                let updated = categories_with_renamed(&existing, &self.category_id, &name);
                self.client
                    .update_server_categories(&self.server_id, &updated)
                    .await
                    .map_err(other_err)?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("Renamed category to {name}")),
                })
            }
            other => Err(ContentError::NotSupported(format!(
                "execute: unknown action {other}"
            ))),
        }
    }

    async fn invoke_action(&self, name: &str, ctx: &ActionContext) -> Result<ActionDispatch> {
        Ok(match name {
            // Paste a previously-cut channel INTO this category.
            "paste-move" => match &ctx.marked {
                Some(marked) => {
                    move_marked_channel(
                        &self.client,
                        &self.state,
                        &self.server_id,
                        Some(&self.category_id),
                        marked,
                    )
                    .await
                }
                None => ActionDispatch::Error("Nothing cut to paste".into()),
            },
            // `mark-move` is frontend-owned (it records the cut); the
            // adapter only acknowledges it. Anything else is a no-op.
            _ => ActionDispatch::Noop,
        })
    }
}

/// List a category's channels (in declared order) from a state snapshot.
/// Shared by the category node's legacy `list` and the adapter's `childs`
/// fetcher. A category whose server/id has vanished (reconnect race) lists
/// empty rather than erroring.
pub(super) fn list_category_channels(
    state: &StoatState,
    server_id: &str,
    category_id: &str,
) -> ListResult {
    let items: Vec<NodeSummary> = state
        .servers
        .get(server_id)
        .and_then(|s| s.categories.iter().find(|c| c.id == category_id))
        .map(|cat| cat.channels.as_slice())
        .unwrap_or(&[])
        .iter()
        .filter_map(|cid| state.channels.get(cid))
        .map(|c| channel_summary(c, state.is_channel_unread(&c.id)))
        .collect();

    ListResult {
        items,
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
    use not_yet_done_content::{ListParams, Node, children};

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
    fn categories_with_renamed_changes_only_target_title() {
        let existing = vec![cat("c1", &["A"]), cat("c2", &["B"])];
        let out = categories_with_renamed(&existing, "c2", "Fresh Title");
        assert_eq!(out[0].title, "title-c1"); // untouched
        assert_eq!(out[1].title, "Fresh Title");
        assert_eq!(out[1].channels, vec!["B"]); // channels preserved
    }

    #[test]
    fn channel_moved_detaches_then_reattaches() {
        let existing = vec![cat("c1", &["A", "X"]), cat("c2", &["B"])];
        // Move X from c1 into c2.
        let into_c2 = categories_with_channel_moved(&existing, "X", Some("c2"));
        assert_eq!(into_c2[0].channels, vec!["A"]); // detached from c1
        assert_eq!(into_c2[1].channels, vec!["B", "X"]); // attached to c2
        // Move X out to uncategorized (no target): gone from every list.
        let uncategorized = categories_with_channel_moved(&into_c2, "X", None);
        assert_eq!(uncategorized[0].channels, vec!["A"]);
        assert_eq!(uncategorized[1].channels, vec!["B"]);
        // A no-op move (target == current category) keeps it once, no dup.
        let same = categories_with_channel_moved(&existing, "A", Some("c1"));
        assert_eq!(same[0].channels, vec!["X", "A"]);
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
        assert_eq!(
            split_category_composite("C1/msg/01ARZ3NDEKTSV4RRFFQ69G5FAV"),
            None
        );
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
        let state = Arc::new(RwLock::new(st));
        let node = StoatCategoryNode::new(
            test_client(),
            Arc::clone(&state),
            "S1".into(),
            "cat1".into(),
            "General".into(),
        );
        let adapter = crate::adapter::StoatAdapter::for_test(state, test_client());
        let node: &dyn Node = &node;
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
        // Category order (C2 before C1), not server order.
        let ids: Vec<&str> = res.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["C2", "C1"]);
    }
}
