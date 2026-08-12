//! `StoatAdapter` — the `ContentAdapter` implementation.
//!
//! Unlike the pull-only adapters, Stoat holds a live WebSocket: the
//! first `root()` spawns a background bootstrap that logs in, discovers
//! the WS URL, and starts the [`StoatGateway`]. The adapter owns a
//! single [`AdapterStatus`] watch channel that reflects **both** the
//! login phase (forwarded from the auth orchestrator) and the socket
//! phase (published by the gateway), so the TUI banner tracks reality
//! end to end.

mod anonymize;
mod attachment;
mod auth_bridge;
mod category;
mod channel;
pub mod config;
mod factory;
mod members;
mod mentions;
mod message;
mod root;
mod server;
mod types;

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Mutex, RwLock, broadcast, watch};

use not_yet_done_content::{
    ActionInput, AdapterCapabilities, AdapterStatus, Child, ContentAdapter, ContentError,
    Invalidation, MetadataField, Node, NodeType, Result,
};

use crate::gateway::{StoatGateway, StoatState};
use attachment::StoatAttachmentNode;
use auth_bridge::AuthBridge;
use category::StoatCategoryNode;
use channel::StoatChannelNode;
use members::MemberCache;
use message::StoatMessageNode;
use root::StoatRoot;
use server::StoatServerNode;

pub use factory::StoatAdapterFactory;

/// Map any `Display` error (the REST/client layer returns `String`s) onto
/// the generic `ContentError::Other`.
pub(in crate::adapter) fn other_err(e: impl std::fmt::Display) -> ContentError {
    ContentError::Other(e.to_string().into())
}

/// Pull a required, trimmed text field out of a `Form` action's input.
/// Shared by the server/category create actions, which both take a single
/// `name` field. An empty (or missing) value is rejected with a clear
/// message rather than creating a nameless channel/category.
pub(in crate::adapter) fn form_field(input: &ActionInput, key: &str) -> Result<String> {
    match input {
        ActionInput::Form(values) => {
            let value = values.get(key).map(|s| s.trim()).unwrap_or("");
            if value.is_empty() {
                return Err(ContentError::Other(
                    format!("`{key}` must not be empty").into(),
                ));
            }
            Ok(value.to_string())
        }
        _ => Err(ContentError::NotSupported("expected form input".into())),
    }
}

/// Build the `unread` metadata field carried on channel/category/message
/// summaries. Value is `"true"` when unread, empty otherwise — the view's
/// styling layer paints the unread highlight + leading marker when it's
/// non-empty (mirrors the tasks adapter's `tracking` marker field).
pub(in crate::adapter) fn unread_field(unread: bool) -> MetadataField {
    MetadataField {
        key: "unread".into(),
        value: if unread { "true".into() } else { String::new() },
        display_label: "Unread".into(),
        editable: false,
        allowed_values: None,
    }
}

pub struct StoatAdapter {
    auth: Arc<AuthBridge>,
    name: String,
    instance_id: String,
    state: Arc<RwLock<StoatState>>,
    /// Single source of truth for the TUI banner. Driven by the auth
    /// orchestrator (login phase) and the gateway (socket phase).
    status_tx: watch::Sender<AdapterStatus>,
    /// Keeps the channel open even if no external subscriber exists yet.
    _status_keepalive: watch::Receiver<AdapterStatus>,
    /// Out-of-band content-change events from the gateway, fanned out to
    /// every view's invalidation watcher (see [`Invalidation`]). Held as
    /// the sender; each [`subscribe_invalidations`](ContentAdapter::subscribe_invalidations)
    /// call returns a fresh `broadcast` receiver.
    inv_tx: broadcast::Sender<Invalidation>,
    /// The running gateway, started lazily once on first `root()`.
    gateway: Arc<Mutex<Option<StoatGateway>>>,
    /// Per-server member lists, fetched on demand to back mention
    /// autocomplete (see [`members`]).
    members: Arc<MemberCache>,
}

impl StoatAdapter {
    pub(in crate::adapter) fn from_parts(
        auth: Arc<AuthBridge>,
        name: String,
        instance_id: String,
    ) -> Self {
        let (status_tx, status_rx) = watch::channel(AdapterStatus::Idle);
        // Capacity is a small backlog: a slow frontend that lags only
        // misses discrete events, and the watcher resyncs with an
        // `Invalidation::All` on `Lagged` — so no update is ever lost,
        // just coarsened.
        let (inv_tx, _) = broadcast::channel(64);

        // Wire the invalidation sink into the shared state before sharing it,
        // so a local read (`mark_read`) repaints the tree itself rather than
        // depending on the server echoing the ack back.
        let mut initial_state = StoatState::default();
        initial_state.set_invalidations(inv_tx.clone());

        // Forward the auth orchestrator's status into our own channel so
        // interactive login (NeedsCreds / Connecting / Failed) still
        // surfaces. We deliberately drop its `Ready` — the gateway is
        // the sole source of `Ready` (a valid token does not yet mean a
        // live socket).
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let mut rx = auth.subscribe_status();
            let tx = status_tx.clone();
            handle.spawn(async move {
                loop {
                    {
                        let s = rx.borrow().clone();
                        if !matches!(s, AdapterStatus::Ready) {
                            let _ = tx.send(s);
                        }
                    }
                    if rx.changed().await.is_err() {
                        break;
                    }
                }
            });
        }

        Self {
            auth,
            name,
            instance_id,
            state: Arc::new(RwLock::new(initial_state)),
            status_tx,
            _status_keepalive: status_rx,
            inv_tx,
            gateway: Arc::new(Mutex::new(None)),
            members: Arc::new(MemberCache::default()),
        }
    }

    /// Kick off login + WS connect in the background (idempotent). Does
    /// not block the caller — progress is reported via the status
    /// channel; the gateway keeps running for the adapter's lifetime.
    fn spawn_gateway_bootstrap(&self) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        // Announce the connect attempt *synchronously*, before the task that
        // performs it is even spawned. Our listings project the in-memory
        // snapshot, which is empty until the socket is up — a frontend must
        // be able to tell "connecting, ask again in a moment" from "idle,
        // nothing is happening" the instant `root()` returns. Left on `Idle`
        // the CLI cannot distinguish the two and prints an empty list as if
        // the account had no servers.
        if matches!(*self.status_tx.borrow(), AdapterStatus::Idle) {
            let _ = self.status_tx.send(AdapterStatus::Connecting {
                retry: 1,
                max_retries: 1,
                timeout_secs: 0,
            });
        }
        let auth = Arc::clone(&self.auth);
        let state = Arc::clone(&self.state);
        let status_tx = self.status_tx.clone();
        let inv_tx = self.inv_tx.clone();
        let slot = Arc::clone(&self.gateway);
        handle.spawn(async move {
            let mut guard = slot.lock().await;
            if guard.is_some() {
                return;
            }
            let client = match auth.get_client().await {
                Ok(c) => c,
                Err(e) => {
                    let _ = status_tx.send(AdapterStatus::Failed { reason: e });
                    return;
                }
            };
            let ws_url = match client.discover_ws_url().await {
                Ok(u) => u,
                Err(e) => {
                    let _ = status_tx.send(AdapterStatus::Failed {
                        reason: format!("discover ws url: {e}"),
                    });
                    return;
                }
            };
            let gw = StoatGateway::spawn(ws_url, client, state, status_tx, inv_tx);
            *guard = Some(gw);
        });
    }

    /// Assemble an adapter from a ready state + client for in-crate tests,
    /// so the generic `children::list` / `children::child_types` free
    /// functions can be driven against a node without a live gateway.
    #[cfg(test)]
    pub(in crate::adapter) fn for_test(
        state: Arc<RwLock<StoatState>>,
        client: Arc<crate::client::StoatClient>,
    ) -> Self {
        let (status_tx, status_rx) = watch::channel(AdapterStatus::Idle);
        let (inv_tx, _) = broadcast::channel(64);
        Self {
            auth: AuthBridge::for_test("https://chat.example.invalid", client),
            name: "test".into(),
            instance_id: "test".into(),
            state,
            status_tx,
            _status_keepalive: status_rx,
            inv_tx,
            gateway: Arc::new(Mutex::new(None)),
            members: Arc::new(MemberCache::default()),
        }
    }
}

#[async_trait]
impl ContentAdapter for StoatAdapter {
    fn adapter_type(&self) -> &str {
        "stoat"
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Realism anonymizer: server/category/channel names become
    /// `<adjective>_<noun>` placeholders, message bodies become neutral text,
    /// authors stay person names. The safe StandardAnonymizer is the fallback.
    /// See [`anonymize`](self::anonymize).
    fn anonymizer(&self) -> std::sync::Arc<dyn not_yet_done_content::Anonymizer> {
        std::sync::Arc::new(anonymize::StoatAnonymizer::default())
    }

    async fn root(&self) -> Result<Box<dyn Node>> {
        // Start the socket in the background; return the (empty in
        // Phase 0) root immediately so the view renders while the
        // gateway connects.
        self.spawn_gateway_bootstrap();
        Ok(Box::new(StoatRoot {
            connection_name: self.name.clone(),
        }))
    }

    /// Resolve an opaque tree id to a node. Servers and channels are
    /// looked up in the live `StoatState`; messages arrive as composite
    /// `<channel>/msg/<ulid>` ids (the only place that encoding is
    /// decoded) and are re-fetched over REST for the preview path.
    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
        // Category composites `<server>/cat/<catid>` resolve from the live
        // snapshot (title looked up for the label; channels listed lazily).
        if let Some((server_id, category_id)) = category::split_category_composite(id) {
            let title = {
                let state = self.state.read().await;
                state
                    .servers
                    .get(server_id)
                    .and_then(|s| s.categories.iter().find(|c| c.id == category_id))
                    .map(|c| c.title.clone())
                    .unwrap_or_else(|| category_id.to_string())
            };
            let client = self.auth.get_client().await.map_err(other_err)?;
            return Ok(Box::new(StoatCategoryNode::new(
                client,
                Arc::clone(&self.state),
                server_id.to_string(),
                category_id.to_string(),
                title,
            )));
        }

        // Attachment composites `<channel>/msg/<msg>/file/<fileid>` must be
        // tested BEFORE the message split — the `/msg/` marker matches them
        // too and would mis-read the trailing `/file/…` as part of the
        // message id. Revolt has no per-file endpoint, so the parent message
        // is re-fetched and the file picked out of its attachment list.
        if let Some((channel_id, message_id, file_id)) = attachment::split_attachment_composite(id)
        {
            let client = self.auth.get_client().await.map_err(other_err)?;
            let view = client
                .fetch_message(channel_id, message_id, None)
                .await
                .map_err(other_err)?;
            let found = view
                .attachments
                .into_iter()
                .find(|a| a.id == file_id)
                .ok_or_else(|| ContentError::NotFound(id.to_string()))?;
            return Ok(Box::new(StoatAttachmentNode::new(
                client,
                channel_id.to_string(),
                message_id.to_string(),
                found,
            )));
        }

        if let Some((channel_id, message_id)) = message::split_composite(id) {
            let client = self.auth.get_client().await.map_err(other_err)?;
            // Single-fetch has no `users[]` array; the preview only reads
            // the body, so author resolution is deferred (falls back to
            // the raw id in metadata).
            let view = client
                .fetch_message(channel_id, message_id, None)
                .await
                .map_err(other_err)?;
            // Server-scoped user map so `<@ID>` mentions render as
            // `@username` and the edit path can build `@uu_…` slugs.
            let users =
                members::channel_user_map(&self.state, &self.members, &client, channel_id).await;
            return Ok(Box::new(StoatMessageNode::new(
                client,
                view,
                users,
                Arc::clone(&self.state),
            )));
        }

        if id == "root" {
            return Ok(Box::new(StoatRoot {
                connection_name: self.name.clone(),
            }));
        }

        // Classify against the live snapshot, then drop the guard before
        // any `await` (the client fetch doesn't touch state).
        enum Kind {
            Server(String),
            Channel(String),
            Unknown,
        }
        let kind = {
            let state = self.state.read().await;
            if let Some(s) = state.servers.get(id) {
                Kind::Server(s.name.clone())
            } else if let Some(c) = state.channels.get(id) {
                Kind::Channel(c.name.clone().unwrap_or_else(|| c.id.clone()))
            } else {
                Kind::Unknown
            }
        };
        match kind {
            Kind::Server(name) => {
                let client = self.auth.get_client().await.map_err(other_err)?;
                Ok(Box::new(StoatServerNode::new(
                    client,
                    Arc::clone(&self.state),
                    id.to_string(),
                    name,
                )))
            }
            Kind::Channel(name) => {
                let client = self.auth.get_client().await.map_err(other_err)?;
                Ok(Box::new(StoatChannelNode::new(
                    client,
                    id.to_string(),
                    name,
                    Arc::clone(&self.state),
                    Arc::clone(&self.members),
                )))
            }
            Kind::Unknown => Err(ContentError::NotFound(id.to_string())),
        }
    }

    /// Fetch an attachment we serve. Attachments live on the autumn file
    /// host discovered at connect; only URLs under that base are ours, so
    /// any other link (or a not-yet-discovered host) declines here and the
    /// frontend falls back to the browser.
    async fn download_asset(&self, url: &str) -> Result<Vec<u8>> {
        let client = self.auth.get_client().await.map_err(other_err)?;
        let is_ours = client
            .autumn_url()
            .is_some_and(|base| url.starts_with(base));
        if !is_ours {
            return Err(ContentError::NotSupported("download_asset".into()));
        }
        client.download_bytes(url).await.map_err(other_err)
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            // Phase 3: messages can be sent (created) and deleted; search
            // is still gateway-snapshot only.
            supports_create: true,
            supports_delete: true,
            ..AdapterCapabilities::default()
        }
    }

    /// Hint resolution (sync, no I/O): a channel offers `send_message`, a
    /// message offers edit/delete/react. Kept in lockstep with each
    /// node's own `actions()` so the action bar and the editor agree.
    fn actions_for_type(&self, node_type: &NodeType) -> Vec<not_yet_done_content::NodeAction> {
        match node_type.type_id.as_str() {
            "stoat:server" => server::server_actions(),
            "stoat:category" => category::category_actions(),
            "stoat:channel" => channel::channel_actions(),
            "stoat:message" => message::message_actions(),
            "stoat:attachment" => attachment::attachment_actions(),
            _ => Vec::new(),
        }
    }

    /// The single source of truth about what lives under a Stoat node,
    /// keyed on the node's `type_id`. Each `Child` fetcher reconstructs its
    /// listing inputs from the adapter's shared `state`/`members` plus
    /// `node.id()` — no concrete-node downcast:
    ///
    /// - root → `stoat:server` + `stoat:channel` (DMs), both pure state reads.
    /// - server (`node.id()` = server id) → `stoat:category` +
    ///   `stoat:channel` (uncategorized), both scoped by the server id.
    /// - category (`node.id()` = `<server>/cat/<catid>` composite) →
    ///   `stoat:channel`; the composite carries BOTH the server id and the
    ///   category id, so `split_category_composite` recovers everything the
    ///   filter needs from the id alone.
    /// - channel (`node.id()` = channel id) → `stoat:message`, a REST pull
    ///   via the shared client/state/members.
    /// - message → leaf (no children).
    ///
    /// No node declares sortable columns (Stoat lists in server/declared
    /// order), so every `columns` list is empty.
    fn childs<'a>(&'a self, node: &'a dyn Node) -> Vec<Child<'a>> {
        match node.node_type().type_id.as_str() {
            "stoat:root" => vec![
                Child {
                    node_type: types::server_type().clone(),
                    columns: Vec::new(),
                    list: Box::new(move |_params| {
                        Box::pin(async move {
                            let state = self.state.read().await;
                            Ok(root::list_servers_from(&state))
                        })
                    }),
                },
                Child {
                    node_type: types::channel_type().clone(),
                    columns: Vec::new(),
                    list: Box::new(move |_params| {
                        Box::pin(async move {
                            let state = self.state.read().await;
                            Ok(root::list_dms_from(&state))
                        })
                    }),
                },
            ],
            "stoat:server" => {
                let server_id = node.id().to_string();
                let server_id_2 = server_id.clone();
                vec![
                    Child {
                        node_type: types::category_type().clone(),
                        columns: Vec::new(),
                        list: Box::new(move |_params| {
                            Box::pin(async move {
                                let state = self.state.read().await;
                                Ok(server::list_server_categories(&state, &server_id))
                            })
                        }),
                    },
                    Child {
                        node_type: types::channel_type().clone(),
                        columns: Vec::new(),
                        list: Box::new(move |_params| {
                            Box::pin(async move {
                                let state = self.state.read().await;
                                Ok(server::list_uncategorized_channels(&state, &server_id_2))
                            })
                        }),
                    },
                ]
            }
            "stoat:category" => {
                // The composite id `<server>/cat/<catid>` carries BOTH parts,
                // so the filter is fully reconstructable from `node.id()`.
                let (server_id, category_id) = match category::split_category_composite(node.id()) {
                    Some((s, c)) => (s.to_string(), c.to_string()),
                    // Malformed id → no children (mirrors the empty-state
                    // fallthrough of the legacy list path).
                    None => return Vec::new(),
                };
                vec![Child {
                    node_type: types::channel_type().clone(),
                    columns: Vec::new(),
                    list: Box::new(move |_params| {
                        Box::pin(async move {
                            let state = self.state.read().await;
                            Ok(category::list_category_channels(
                                &state,
                                &server_id,
                                &category_id,
                            ))
                        })
                    }),
                }]
            }
            "stoat:channel" => {
                let channel_id = node.id().to_string();
                vec![Child {
                    node_type: types::message_type().clone(),
                    columns: Vec::new(),
                    list: Box::new(move |params| {
                        Box::pin(async move {
                            let client = self.auth.get_client().await.map_err(other_err)?;
                            channel::list_channel_messages(
                                &client,
                                &self.state,
                                &self.members,
                                &channel_id,
                                params,
                            )
                            .await
                        })
                    }),
                }]
            }
            // A message's children are its uploaded files. Revolt embeds
            // them in the message document (no per-file endpoint), so the
            // listing re-fetches the message — cheap, and it keeps the
            // attachment rows in step with an edited/deleted message.
            "stoat:message" => {
                let Some((channel_id, message_id)) = message::split_composite(node.id()) else {
                    return Vec::new();
                };
                let (channel_id, message_id) = (channel_id.to_string(), message_id.to_string());
                vec![Child {
                    node_type: types::attachment_type().clone(),
                    columns: Vec::new(),
                    list: Box::new(move |_params| {
                        Box::pin(async move {
                            let client = self.auth.get_client().await.map_err(other_err)?;
                            attachment::list_message_attachments(&client, &channel_id, &message_id)
                                .await
                        })
                    }),
                }]
            }
            // Attachments (and anything unknown) are leaves.
            _ => Vec::new(),
        }
    }

    fn subscribe_status(&self) -> watch::Receiver<AdapterStatus> {
        self.status_tx.subscribe()
    }

    fn subscribe_invalidations(&self) -> broadcast::Receiver<Invalidation> {
        self.inv_tx.subscribe()
    }

    async fn submit_credentials(
        &self,
        fields: std::collections::HashMap<String, String>,
    ) -> Result<()> {
        self.auth
            .submit_credentials(fields)
            .await
            .map_err(|e| ContentError::Other(e.into()))
    }

    async fn cancel_credentials(&self) -> Result<()> {
        self.auth
            .cancel_credentials()
            .await
            .map_err(|e| ContentError::Other(e.into()))
    }

    async fn invalidate_session(&self) -> Result<()> {
        self.auth.invalidate_session().await;
        Ok(())
    }

    async fn invalidate_credentials(&self) -> Result<()> {
        self.auth.invalidate_credentials().await;
        Ok(())
    }
}
