//! The WebSocket gateway — the **only** place that speaks WS.
//!
//! A single background tokio task owns the socket lifecycle:
//! `connect → Authenticate → Ready → event loop`, with a periodic
//! heartbeat ping and automatic reconnect (capped exponential backoff).
//! It mirrors the connection state into the shared [`AdapterStatus`]
//! watch channel so the TUI banner reflects reality, and rebuilds
//! [`StoatState`] from each `Ready`.
//!
//! Phase 0 scope: hold the socket and collect `Ready`. Live events are
//! received but ignored (they hit [`protocol::ServerMessage::Other`]);
//! Phase 2 turns them into out-of-band invalidations.

pub mod protocol;
pub mod state;

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::{RwLock, broadcast, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use not_yet_done_content::{AdapterStatus, Invalidation};

pub use state::StoatState;

use protocol::{ClientMessage, ServerMessage};

/// Heartbeat period. Revolt's default client pings well inside the
/// server's idle timeout; 20s is comfortably below it.
const HEARTBEAT: Duration = Duration::from_secs(20);
/// Reconnect backoff ceiling.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Handle to the running gateway task. Aborting on drop ties the
/// socket's lifetime to the adapter — when the adapter is dropped the
/// background task and its connection go away.
pub struct StoatGateway {
    handle: JoinHandle<()>,
}

impl StoatGateway {
    /// Spawn the gateway. Requires a Tokio runtime (the adapter is built
    /// and used inside one).
    pub fn spawn(
        ws_url: String,
        token: String,
        state: Arc<RwLock<StoatState>>,
        status_tx: watch::Sender<AdapterStatus>,
        inv_tx: broadcast::Sender<Invalidation>,
    ) -> Self {
        let handle = tokio::spawn(run(ws_url, token, state, status_tx, inv_tx));
        Self { handle }
    }
}

impl Drop for StoatGateway {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Reconnect loop. Never returns under normal operation — the adapter
/// drop aborts the task.
async fn run(
    ws_url: String,
    token: String,
    state: Arc<RwLock<StoatState>>,
    status_tx: watch::Sender<AdapterStatus>,
    inv_tx: broadcast::Sender<Invalidation>,
) {
    let mut backoff = Duration::from_secs(1);
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        let _ = status_tx.send(AdapterStatus::Connecting {
            retry: attempt,
            max_retries: attempt,
            timeout_secs: 0,
        });

        match connect_once(&ws_url, &token, &state, &status_tx, &inv_tx).await {
            // Clean close → reset backoff and reconnect promptly.
            Ok(()) => backoff = Duration::from_secs(1),
            Err(e) => {
                let _ = status_tx.send(AdapterStatus::Failed {
                    reason: format!("gateway: {e}"),
                });
            }
        }

        if let Ok(mut st) = state.try_write() {
            st.connected = false;
        } else {
            state.write().await.connected = false;
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// One full connection: connect, authenticate, then pump events until
/// the socket closes or errors. Returns `Ok` on a clean close, `Err`
/// with a human-readable reason otherwise.
async fn connect_once(
    ws_url: &str,
    token: &str,
    state: &Arc<RwLock<StoatState>>,
    status_tx: &watch::Sender<AdapterStatus>,
    inv_tx: &broadcast::Sender<Invalidation>,
) -> Result<(), String> {
    let (ws_stream, _resp) = tokio_tungstenite::connect_async(ws_url)
        .await
        .map_err(|e| format!("connect {ws_url}: {e}"))?;
    let (mut write, mut read) = ws_stream.split();

    let auth = serde_json::to_string(&ClientMessage::Authenticate {
        token: token.to_string(),
    })
    .map_err(|e| format!("encode Authenticate: {e}"))?;
    write
        .send(Message::Text(auth.into()))
        .await
        .map_err(|e| format!("send Authenticate: {e}"))?;

    // First ping fires after one full period, never before Authenticate.
    let mut hb = tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT, HEARTBEAT);
    let mut ping_seq: i64 = 0;

    loop {
        tokio::select! {
            _ = hb.tick() => {
                ping_seq += 1;
                let ping = serde_json::to_string(&ClientMessage::Ping { data: ping_seq })
                    .map_err(|e| format!("encode Ping: {e}"))?;
                if write.send(Message::Text(ping.into())).await.is_err() {
                    return Ok(()); // socket gone — let the outer loop reconnect
                }
            }
            incoming = read.next() => {
                match incoming {
                    Some(Ok(Message::Text(txt))) => {
                        handle_text(&txt, state, status_tx, inv_tx).await;
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        // Answer transport-level pings to stay alive.
                        let _ = write.send(Message::Pong(payload)).await;
                    }
                    Some(Ok(Message::Pong(_))) | Some(Ok(Message::Binary(_)))
                    | Some(Ok(Message::Frame(_))) => {}
                    Some(Ok(Message::Close(_))) | None => return Ok(()),
                    Some(Err(e)) => return Err(e.to_string()),
                }
            }
        }
    }
}

/// Decode and apply one text frame. Unparseable frames and unmodelled
/// events are ignored so a single odd message never kills the socket.
///
/// Live-layer: a fresh `Ready` pushes [`Invalidation::All`] (so the
/// initially-empty tree populates and a reconnect resyncs every view
/// without a manual `r`); message events push [`Invalidation::Node`] for
/// just their channel, so only a view currently showing that channel's
/// messages reloads. Structural events (`ChannelCreate`/`ChannelUpdate`/
/// `ChannelDelete`/`ServerUpdate`) mutate [`StoatState`] and push
/// [`Invalidation::All`] — the tree shape changed, so every bound view
/// reloads its current level off the updated snapshot.
async fn handle_text(
    txt: &str,
    state: &Arc<RwLock<StoatState>>,
    status_tx: &watch::Sender<AdapterStatus>,
    inv_tx: &broadcast::Sender<Invalidation>,
) {
    let msg: ServerMessage = match serde_json::from_str(txt) {
        Ok(m) => m,
        Err(_) => return,
    };

    // Message-level events only invalidate one channel's list — handle
    // them uniformly before the structural match below.
    if let Some(channel) = msg.affected_channel() {
        let _ = inv_tx.send(Invalidation::Node {
            id: channel.to_string(),
        });
        return;
    }

    match msg {
        ServerMessage::Authenticated => {
            // Ready follows; nothing to do yet.
        }
        ServerMessage::Ready {
            users,
            servers,
            channels,
        } => {
            {
                let mut st = state.write().await;
                st.apply_ready(users, servers, channels);
                st.connected = true;
            }
            let _ = status_tx.send(AdapterStatus::Ready);
            // The tree structure just (re)appeared — tell every bound
            // view to reload its current level.
            let _ = inv_tx.send(Invalidation::All);
        }
        ServerMessage::Pong { .. } => {}
        ServerMessage::Error { .. } => {
            // Protocol error (e.g. InvalidSession). Leave the socket to
            // close; the outer loop reconnects. Re-auth on a rejected
            // token is auth-bridge territory (Phase 2 hardening).
        }
        // Structural events: mutate the shared snapshot, then reload the
        // whole tree. (Server join/leave isn't modelled — those still go
        // through the reconnect `Ready` resnapshot.)
        ServerMessage::ChannelCreate(channel) => {
            state.write().await.insert_channel(channel);
            let _ = inv_tx.send(Invalidation::All);
        }
        ServerMessage::ChannelUpdate { id, data, .. } => {
            state.write().await.patch_channel(&id, data);
            let _ = inv_tx.send(Invalidation::All);
        }
        ServerMessage::ChannelDelete { id } => {
            state.write().await.remove_channel(&id);
            let _ = inv_tx.send(Invalidation::All);
        }
        ServerMessage::ServerUpdate { id, data, .. } => {
            state.write().await.patch_server(&id, data);
            let _ = inv_tx.send(Invalidation::All);
        }
        // Message events handled above via `affected_channel`.
        ServerMessage::Message { .. }
        | ServerMessage::MessageUpdate { .. }
        | ServerMessage::MessageDelete { .. }
        | ServerMessage::MessageReact { .. }
        | ServerMessage::MessageUnreact { .. } => {}
        ServerMessage::Other => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_content::Invalidation;

    /// Build the three sinks `handle_text` writes to. The `watch`
    /// keepalive receiver is returned so the channel stays open.
    fn sinks() -> (
        Arc<RwLock<StoatState>>,
        watch::Sender<AdapterStatus>,
        watch::Receiver<AdapterStatus>,
        broadcast::Sender<Invalidation>,
        broadcast::Receiver<Invalidation>,
    ) {
        let state = Arc::new(RwLock::new(StoatState::default()));
        let (status_tx, status_rx) = watch::channel(AdapterStatus::Idle);
        let (inv_tx, inv_rx) = broadcast::channel(8);
        (state, status_tx, status_rx, inv_tx, inv_rx)
    }

    #[tokio::test]
    async fn message_event_pushes_node_invalidation() {
        // Invented ids — no real instance data.
        let (state, status_tx, _sr, inv_tx, mut inv_rx) = sinks();
        handle_text(
            r#"{"type":"Message","_id":"M0001","channel":"C0009","author":"U0001","content":"hi"}"#,
            &state,
            &status_tx,
            &inv_tx,
        )
        .await;
        assert_eq!(
            inv_rx.try_recv().unwrap(),
            Invalidation::Node { id: "C0009".into() }
        );
    }

    #[tokio::test]
    async fn ready_populates_state_and_pushes_all() {
        let (state, status_tx, mut st_rx, inv_tx, mut inv_rx) = sinks();
        handle_text(
            r#"{"type":"Ready","users":[],
                "servers":[{"_id":"S0001","name":"guild","channels":[]}],
                "channels":[]}"#,
            &state,
            &status_tx,
            &inv_tx,
        )
        .await;
        assert!(state.read().await.servers.contains_key("S0001"));
        assert!(matches!(*st_rx.borrow_and_update(), AdapterStatus::Ready));
        assert_eq!(inv_rx.try_recv().unwrap(), Invalidation::All);
    }

    /// Seed a server `S0001` with one channel so structural mutations
    /// have something to act on. Invented ids — no real instance data.
    async fn seed_one_server(state: &Arc<RwLock<StoatState>>) {
        let mut st = state.write().await;
        st.apply_ready(
            vec![],
            vec![protocol::Server {
                id: "S0001".into(),
                name: "guild".into(),
                channels: vec!["C0001".into()],
                categories: vec![],
                owner: None,
            }],
            vec![protocol::Channel {
                id: "C0001".into(),
                channel_type: "TextChannel".into(),
                server: Some("S0001".into()),
                name: Some("general".into()),
                last_message_id: None,
                recipients: None,
            }],
        );
    }

    #[tokio::test]
    async fn channel_create_updates_state_and_pushes_all() {
        let (state, status_tx, _sr, inv_tx, mut inv_rx) = sinks();
        seed_one_server(&state).await;
        handle_text(
            r#"{"type":"ChannelCreate","channel_type":"TextChannel",
                "_id":"C0002","server":"S0001","name":"new"}"#,
            &state,
            &status_tx,
            &inv_tx,
        )
        .await;
        let st = state.read().await;
        assert!(st.channels.contains_key("C0002"));
        assert_eq!(st.servers["S0001"].channels, vec!["C0001", "C0002"]);
        assert_eq!(inv_rx.try_recv().unwrap(), Invalidation::All);
    }

    #[tokio::test]
    async fn channel_delete_unlinks_and_pushes_all() {
        let (state, status_tx, _sr, inv_tx, mut inv_rx) = sinks();
        seed_one_server(&state).await;
        handle_text(
            r#"{"type":"ChannelDelete","id":"C0001"}"#,
            &state,
            &status_tx,
            &inv_tx,
        )
        .await;
        let st = state.read().await;
        assert!(!st.channels.contains_key("C0001"));
        assert!(st.servers["S0001"].channels.is_empty());
        assert_eq!(inv_rx.try_recv().unwrap(), Invalidation::All);
    }

    #[tokio::test]
    async fn server_update_replaces_categories_and_pushes_all() {
        let (state, status_tx, _sr, inv_tx, mut inv_rx) = sinks();
        seed_one_server(&state).await;
        handle_text(
            r#"{"type":"ServerUpdate","id":"S0001","data":{"categories":[
                {"id":"cat1","title":"General","channels":["C0001"]}]},"clear":[]}"#,
            &state,
            &status_tx,
            &inv_tx,
        )
        .await;
        let st = state.read().await;
        assert_eq!(st.servers["S0001"].categories.len(), 1);
        assert_eq!(st.servers["S0001"].categories[0].id, "cat1");
        assert_eq!(inv_rx.try_recv().unwrap(), Invalidation::All);
    }

    #[tokio::test]
    async fn non_content_event_pushes_nothing() {
        let (state, status_tx, _sr, inv_tx, mut inv_rx) = sinks();
        handle_text(r#"{"type":"Pong","data":7}"#, &state, &status_tx, &inv_tx).await;
        assert!(inv_rx.try_recv().is_err());
    }
}
