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
    ActionContext, ActionDispatch, ActionInput, ActionOutcome, ContentError, EditorPrep,
    FormFieldSpec, InputSpec, ListParams, ListResult, Metadata, MetadataField, Node, NodeAction,
    NodeSummary, NodeType, Result,
};

use super::category::move_marked_channel;
use super::members::{MemberCache, channel_user_map};
use super::mentions;
use super::message::{StoatMessageNode, composite_id};
use super::types::{channel_type, message_type};
use super::{form_field, other_err};
use crate::client::StoatClient;
use crate::gateway::StoatState;

/// How many messages to pull for the latest page.
const DEFAULT_MESSAGE_LIMIT: u32 = 50;

/// Files per posted message. Revolt's default instance limit is 5
/// attachments per message, so a larger selection is split across several
/// messages instead of being rejected wholesale.
const MAX_ATTACHMENTS_PER_MESSAGE: usize = 5;

/// Actions a channel exposes:
/// - `send_message` — the `create`-style action the message-list view
///   triggers (parent = channel, child = message): opens an empty editor
///   and posts the buffer as a new message.
/// - `attach` — upload local file(s) and post them into this channel. The
///   TUI's file picker supplies the paths; see [`StoatChannelNode::attach`].
/// - `rename` — retitle the channel itself (a single-field name form),
///   reachable while the cursor sits on the channel row in the tree.
/// - `mark-move` / `paste-move` — the cut/paste move pair. `mark-move`
///   records this channel as the cut; `paste-move` (on a category, the
///   server, or another channel) relocates it. Both are frontend-driven
///   shortcuts dispatched through [`Node::invoke_action`].
pub(super) fn channel_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new("send_message", "send", InputSpec::Editor),
        NodeAction::new("attach", "attach", InputSpec::FilePicker { multi: true }),
        NodeAction::new(
            "rename",
            "rename",
            InputSpec::Form {
                fields: vec![FormFieldSpec::text("name", "Channel name")],
            },
        ),
        // `cut` lives in the action bar (top) so it sits beside the other
        // primary verbs and can light up while a cut is armed. `paste
        // channel` stays a status-bar hint (a paste target only matters
        // once something is cut).
        NodeAction::new("mark-move", "cut", InputSpec::None),
        NodeAction::new("paste-move", "paste channel", InputSpec::None),
    ]
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
    /// display and the `@uu_…` edit slugs.
    async fn user_map(&self) -> Arc<HashMap<String, String>> {
        channel_user_map(&self.state, &self.members, &self.client, &self.channel_id).await
    }

    /// Post local file(s) into this channel.
    ///
    /// Two steps per Revolt's design: every path is uploaded to the autumn
    /// file server (yielding a file id), then the ids are posted as
    /// messages with an empty body. There is deliberately no caption — the
    /// API has no endpoint that adds a file to an existing message, so a
    /// caption would have to be a separate send; `e` (edit) on the posted
    /// message adds text afterwards, in place.
    ///
    /// A single upload failure doesn't abort the batch: whatever uploaded
    /// is still posted and the summary names what didn't make it. Only if
    /// *nothing* uploaded does the action fail outright.
    async fn attach(&self, paths: Vec<std::path::PathBuf>) -> Result<ActionOutcome> {
        if paths.is_empty() {
            // The picker was closed without a selection.
            return Ok(ActionOutcome::NoChanges);
        }

        let mut ids: Vec<String> = Vec::new();
        let mut failures: Vec<String> = Vec::new();
        for path in &paths {
            match self.client.upload_attachment(path).await {
                Ok(id) => ids.push(id),
                Err(e) => failures.push(format!("{}: {e}", path.display())),
            }
        }
        if ids.is_empty() {
            return Err(other_err(format!(
                "no file uploaded ({})",
                failures.join("; ")
            )));
        }

        let uploaded = ids.len();
        let mut last_message: Option<String> = None;
        for chunk in ids.chunks(MAX_ATTACHMENTS_PER_MESSAGE) {
            let message_id = self
                .client
                .send_message_with_attachments(&self.channel_id, "", chunk)
                .await
                .map_err(other_err)?;
            last_message = Some(message_id);
        }

        // Posting reads the channel — ack it exactly like `send_message`
        // does, so our own echo doesn't flag the channel unread.
        if let Some(message_id) = &last_message {
            let _ = self.client.ack(&self.channel_id, message_id).await;
            self.state
                .write()
                .await
                .mark_read(&self.channel_id, message_id);
        }

        let mut message = format!("Attached {uploaded} file(s) to #{}", self.name);
        if !failures.is_empty() {
            message.push_str(&format!(
                " — {} failed ({})",
                failures.len(),
                failures.join("; ")
            ));
        }
        Ok(ActionOutcome::Done {
            message: Some(message),
        })
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
    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        match action_id {
            // Compose starts on an empty buffer followed by the CACHE
            // section, so `@uu_…` mention slugs are available to copy.
            "send_message" => {
                let users = self.user_map().await;
                let table = mentions::user_table(&users);
                Ok(EditorPrep {
                    template: mentions::cache_section(&table),
                    version: String::new(),
                    suffix: ".md".into(),
                    file_path: None,
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
                // Drop the CACHE section, then translate `@uu_slug`
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
                // Sending reads the channel: ack it and record the read
                // locally so the channel doesn't flag itself unread off its
                // own `Message` echo. Best-effort — a failed ack is repaired
                // by the WS `ChannelAck` or the next `Ready` resync.
                let _ = self.client.ack(&self.channel_id, &message_id).await;
                self.state
                    .write()
                    .await
                    .mark_read(&self.channel_id, &message_id);
                // Return the created message's composite id so callers can
                // navigate to it (and the `commit_on_save` editor flow can
                // retarget later saves at this message's `edit_message`).
                Ok(ActionOutcome::Navigate {
                    node_id: composite_id(&self.channel_id, &message_id),
                    node_type: message_type().clone(),
                    message: None,
                })
            }
            ("attach", ActionInput::Files(paths)) => self.attach(paths).await,
            // Rename the channel itself. `PATCH /channels/{id}` is a
            // single-field delta; the `ChannelUpdate` echo refreshes the
            // tree without a reload.
            ("rename", input) => {
                let name = form_field(&input, "name")?;
                self.client
                    .rename_channel(&self.channel_id, &name)
                    .await
                    .map_err(other_err)?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("Renamed channel to #{name}")),
                })
            }
            (other, _) => Err(ContentError::NotSupported(format!(
                "execute: unsupported action/input for {other}"
            ))),
        }
    }

    async fn invoke_action(&self, name: &str, ctx: &ActionContext) -> Result<ActionDispatch> {
        Ok(match name {
            // Paste a cut channel beside this one — into whatever container
            // (a category, or the uncategorized branch) currently holds
            // this channel. Resolving the destination needs this channel's
            // server + owning category from the live snapshot.
            "paste-move" => {
                let Some(marked) = &ctx.marked else {
                    return Ok(ActionDispatch::Error("Nothing cut to paste".into()));
                };
                let resolved = {
                    let st = self.state.read().await;
                    st.channels
                        .get(&self.channel_id)
                        .and_then(|c| c.server.clone())
                        .map(|server_id| {
                            let target_cat = st.servers.get(&server_id).and_then(|s| {
                                s.categories
                                    .iter()
                                    .find(|c| c.channels.iter().any(|ch| ch == &self.channel_id))
                                    .map(|c| c.id.clone())
                            });
                            (server_id, target_cat)
                        })
                };
                match resolved {
                    Some((server_id, target_cat)) => {
                        move_marked_channel(
                            &self.client,
                            &self.state,
                            &server_id,
                            target_cat.as_deref(),
                            marked,
                        )
                        .await
                    }
                    None => ActionDispatch::Error("This channel has no server".into()),
                }
            }
            // `mark-move` is frontend-owned (it records the cut); just
            // acknowledge it. Anything else is a no-op.
            _ => ActionDispatch::Noop,
        })
    }
}

/// Fetch (and project) a channel's most-recent message page from REST.
/// Shared by the channel node's legacy `list` and the adapter's `childs`
/// fetcher; both take exactly the ingredients the adapter can reconstruct
/// from its own `state`/`members` plus a channel id — no concrete node
/// downcast needed. `params.node_type` is assumed to be `stoat:message`
/// (the caller has already matched it).
pub(super) async fn list_channel_messages(
    client: &Arc<StoatClient>,
    state: &Arc<RwLock<StoatState>>,
    members: &Arc<MemberCache>,
    channel_id: &str,
    params: ListParams,
) -> Result<ListResult> {
    let limit = params
        .page
        .map(|p| p.limit)
        .filter(|&l| l > 0)
        .unwrap_or(DEFAULT_MESSAGE_LIMIT);

    let views = client
        .list_messages(channel_id, limit, None)
        .await
        .map_err(super::other_err)?;

    // This is always the NEWEST page (`before = None`), so its last entry is
    // the channel's true newest message — and an empty page means the channel
    // is empty. Heal the snapshot with it: Stoat leaves `last_message_id`
    // pointing at a deleted message, which would otherwise keep the tree
    // showing an expand arrow for an empty channel and flag it unread forever.
    state
        .write()
        .await
        .reconcile_last_message(channel_id, views.last().map(|v| v.id.as_str()));

    // Resolve the channel's mentionable users once for the whole
    // page; each message node renders `<@ID>` → `@username` against it.
    let users = channel_user_map(state, members, client, channel_id).await;

    // The last-read marker for this channel: a message is unread when
    // its id sorts after it (ULID lexicographic), or when there is no
    // marker at all (the channel has never been read).
    let last_read = state.read().await.reads.get(channel_id).cloned();

    let items = views
        .into_iter()
        .map(|v| {
            let id = composite_id(&v.channel_id, &v.id);
            let unread = match &last_read {
                Some(read) => v.id.as_str() > read.as_str(),
                None => true,
            };
            // A message is expandable exactly when it carries files — its
            // children are the `stoat:attachment` rows.
            let has_attachments = !v.attachments.is_empty();
            let node =
                StoatMessageNode::new(Arc::clone(client), v, Arc::clone(&users), Arc::clone(state));
            let mut metadata = node.metadata().clone();
            metadata.fields.push(super::unread_field(unread));
            NodeSummary {
                id,
                label: node.label().to_string(),
                node_type: message_type().clone(),
                metadata,
                has_children: Some(has_attachments),
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

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_content::{Node, children};

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
        let state = Arc::new(RwLock::new(StoatState::default()));
        let node = StoatChannelNode::new(
            Arc::clone(&client),
            "C1".into(),
            "general".into(),
            Arc::clone(&state),
            Arc::new(MemberCache::default()),
        );
        let adapter = crate::adapter::StoatAdapter::for_test(state, client);
        assert_eq!(node.id(), "C1");
        assert_eq!(node.label(), "general");
        assert_eq!(node.metadata().fields[0].value, "general");
        let node_ref: &dyn Node = &node;
        assert_eq!(
            children::child_types(&adapter, node_ref)[0].type_id,
            "stoat:message"
        );
    }

    #[test]
    fn channel_exposes_mark_and_paste_move_actions() {
        // Bar placement is derived TUI-side from InputSpec + id, not declared
        // here; the adapter's job is only to expose the actions. `cut`
        // (mark-move) lights up in the action bar while armed; `paste channel`
        // is a fire-and-forget status-bar hint.
        let actions = channel_actions();
        let cut = actions
            .iter()
            .find(|a| a.id == "mark-move")
            .expect("mark-move");
        assert_eq!(cut.label, "cut");
        assert!(actions.iter().any(|a| a.id == "paste-move"));
    }

    #[test]
    fn channel_offers_a_multi_file_upload() {
        // The input spec is the contract with the frontend: `FilePicker` with
        // `multi: true` is what makes the TUI open its picker in
        // multi-select mode, so a single `A` can post a batch. A regression to
        // `multi: false` (or to `InputSpec::None`) would silently reduce the
        // action to one file per keypress.
        let attach = channel_actions()
            .into_iter()
            .find(|a| a.id == "attach")
            .expect("attach action");
        assert_eq!(attach.label, "attach");
        assert!(matches!(
            attach.input,
            InputSpec::FilePicker { multi: true }
        ));
    }
}
