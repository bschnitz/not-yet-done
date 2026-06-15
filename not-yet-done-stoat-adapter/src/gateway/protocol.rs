//! WebSocket wire protocol (Revolt/Stoat 0.11.5).
//!
//! Only the messages Phase 0 needs are modelled in full; everything else
//! is swallowed by the [`ServerMessage::Other`] catch-all so an unknown
//! event never tears the connection down. Object ids are `_id` on the
//! wire. Structs intentionally do **not** `deny_unknown_fields` — the
//! server sends many fields we don't care about yet.

use serde::{Deserialize, Serialize};

/// A chat server (a.k.a. guild). `channels` is the ordered list of
/// channel ids that belong to it; `categories` groups (a subset of)
/// those ids under titles. Channels present in `channels` but in no
/// category are "uncategorized" and render directly under the server.
#[derive(Deserialize, Clone, Debug)]
pub struct Server {
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default)]
    pub categories: Vec<Category>,
    #[serde(default)]
    pub owner: Option<String>,
}

/// A channel category within a server: an ordered group of channel ids
/// under a title. Unlike servers/channels the id field is plain `id`
/// (not `_id`) on the wire, and is not always a ULID — keep it an opaque
/// string. Verified against Stoat 0.13.7 (`Server.categories`).
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Category {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub channels: Vec<String>,
}

/// A channel of any kind. `channel_type` is one of `TextChannel`,
/// `VoiceChannel`, `Group`, `DirectMessage`, `SavedMessages`. We keep
/// the raw type string rather than an enum so a new channel kind never
/// fails deserialisation.
#[derive(Deserialize, Clone, Debug)]
pub struct Channel {
    #[serde(rename = "_id")]
    pub id: String,
    pub channel_type: String,
    #[serde(default)]
    pub server: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub last_message_id: Option<String>,
    #[serde(default)]
    pub recipients: Option<Vec<String>>,
}

/// Partial channel fields carried in a `ChannelUpdate.data`. Only the
/// fields we render are modelled; everything else is ignored. Verified
/// against Stoat 0.13.7 (a rename arrives as `data: { name }`).
#[derive(Deserialize, Clone, Debug, Default)]
pub struct ChannelPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub last_message_id: Option<String>,
}

/// Partial server fields carried in a `ServerUpdate.data`. Channel-list
/// and category changes (add/remove/rename/reorder/reassign) all arrive
/// here as a **full replacement** of the respective list — there is no
/// separate Category* event on the wire. Verified against Stoat 0.13.7.
#[derive(Deserialize, Clone, Debug, Default)]
pub struct ServerPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub channels: Option<Vec<String>>,
    #[serde(default)]
    pub categories: Option<Vec<Category>>,
}

/// A user as carried in `Ready` (enough to resolve message authors).
#[derive(Deserialize, Clone, Debug)]
pub struct User {
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub discriminator: Option<String>,
}

/// Messages we send to the server.
#[derive(Serialize, Debug)]
#[serde(tag = "type")]
pub enum ClientMessage {
    /// First frame after connect — exchanges the session token for an
    /// authenticated socket.
    Authenticate { token: String },
    /// Keep-alive. The server echoes a `Pong` with the same `data`.
    Ping { data: i64 },
}

/// Messages the server sends to us. Internally tagged on `type`; the
/// `Other` catch-all absorbs every event we don't model (an unknown
/// `type`, or a known one whose payload shape differs — a mismatched
/// variant fails to deserialise and the whole frame is then dropped, so
/// an odd event can never tear the socket down).
///
/// Message-level events carry the affected channel and reload one list
/// ([`affected_channel`](Self::affected_channel)); structural events
/// (`Channel*`/`ServerUpdate`) mutate [`StoatState`](super::StoatState)
/// and reload the whole tree. Server join/leave (`ServerCreate`/
/// `ServerDelete`) is intentionally not modelled yet — those still rely
/// on the reconnect `Ready` resnapshot.
#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub enum ServerMessage {
    /// Authentication succeeded; `Ready` follows.
    Authenticated,
    /// One-shot bootstrap snapshot — the only source of the server list.
    Ready {
        #[serde(default)]
        users: Vec<User>,
        #[serde(default)]
        servers: Vec<Server>,
        #[serde(default)]
        channels: Vec<Channel>,
    },
    /// Keep-alive echo.
    Pong {
        #[serde(default)]
        data: i64,
    },
    /// Protocol-level error (e.g. `InvalidSession`, `AlreadyAuthenticated`).
    Error {
        #[serde(default)]
        error: String,
    },
    /// A new message was posted. The full message object is inline; we read
    /// its `channel` (which view level is now stale) and its `_id` — the new
    /// `last_message_id`, so unread highlighting updates live without a REST
    /// round-trip.
    Message {
        #[serde(rename = "_id", default)]
        id: String,
        #[serde(alias = "channel_id")]
        channel: String,
    },
    /// An existing message was edited.
    MessageUpdate {
        #[serde(alias = "channel_id")]
        channel: String,
    },
    /// A message (or batch) was deleted.
    MessageDelete {
        #[serde(alias = "channel_id")]
        channel: String,
    },
    /// A reaction was added. Bonfire names the field `channel_id` here;
    /// the alias keeps one shape for every message event.
    MessageReact {
        #[serde(alias = "channel_id")]
        channel: String,
    },
    /// A reaction was removed.
    MessageUnreact {
        #[serde(alias = "channel_id")]
        channel: String,
    },
    /// A channel was created. The full channel object is inline (same
    /// shape as in `Ready`), so it deserialises straight into [`Channel`].
    ChannelCreate(Channel),
    /// A channel was edited (e.g. renamed). `data` carries only the
    /// changed fields; `clear` names fields reset to default (none we
    /// render are clearable, so it's parsed but not applied).
    ChannelUpdate {
        id: String,
        #[serde(default)]
        data: ChannelPatch,
        #[serde(default)]
        clear: Vec<String>,
    },
    /// A channel was deleted.
    ChannelDelete { id: String },
    /// The user read a channel (from this or another of their clients).
    /// `id` is the channel, `message_id` the message read up to. Updates
    /// the read marker so unread highlighting clears in lockstep across
    /// clients. Verified against Stoat 0.13.7.
    ChannelAck {
        id: String,
        #[serde(default)]
        message_id: String,
    },
    /// A server property changed. Carries category and channel-list
    /// changes (full-list replacement) plus server rename — see
    /// [`ServerPatch`].
    ServerUpdate {
        id: String,
        #[serde(default)]
        data: ServerPatch,
        #[serde(default)]
        clear: Vec<String>,
    },
    #[serde(other)]
    Other,
}

impl ServerMessage {
    /// The channel whose message list this event invalidates, if any.
    /// `None` for non-message events (`Ready`, `Pong`, structural events,
    /// `Other`) — those are handled separately (or ignored).
    pub fn affected_channel(&self) -> Option<&str> {
        match self {
            ServerMessage::Message { channel, .. }
            | ServerMessage::MessageUpdate { channel }
            | ServerMessage::MessageDelete { channel }
            | ServerMessage::MessageReact { channel }
            | ServerMessage::MessageUnreact { channel } => Some(channel),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_authenticated() {
        let m: ServerMessage = serde_json::from_str(r#"{"type":"Authenticated"}"#).unwrap();
        assert!(matches!(m, ServerMessage::Authenticated));
    }

    #[test]
    fn parses_ready_with_invented_data() {
        // Fully invented ids/names — no real instance data.
        let json = r#"{
            "type": "Ready",
            "users": [{"_id": "U0001", "username": "alice"}],
            "servers": [{"_id": "S0001", "name": "Test Guild", "channels": ["C0001","C0002"],
                "categories": [{"id": "cat1", "title": "General", "channels": ["C0001"]}]}],
            "channels": [
                {"_id": "C0001", "channel_type": "TextChannel", "server": "S0001", "name": "general"},
                {"_id": "C0002", "channel_type": "VoiceChannel", "server": "S0001", "name": "voice"},
                {"_id": "D0001", "channel_type": "DirectMessage", "recipients": ["U0001","U0002"]}
            ]
        }"#;
        let m: ServerMessage = serde_json::from_str(json).unwrap();
        match m {
            ServerMessage::Ready {
                users,
                servers,
                channels,
            } => {
                assert_eq!(users.len(), 1);
                assert_eq!(servers[0].channels.len(), 2);
                assert_eq!(servers[0].categories.len(), 1);
                assert_eq!(servers[0].categories[0].id, "cat1");
                assert_eq!(servers[0].categories[0].channels, vec!["C0001"]);
                assert_eq!(channels.len(), 3);
                assert_eq!(channels[0].channel_type, "TextChannel");
            }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn server_without_categories_defaults_to_empty() {
        // Older servers / DMs omit the field entirely — must not fail.
        let s: Server =
            serde_json::from_str(r#"{"_id":"S1","name":"G","channels":["C1"]}"#).unwrap();
        assert!(s.categories.is_empty());
    }

    #[test]
    fn unknown_event_maps_to_other() {
        // A genuinely unmodelled event type still falls through to Other.
        let m: ServerMessage =
            serde_json::from_str(r#"{"type":"UserUpdate","id":"X","data":{}}"#).unwrap();
        assert!(matches!(m, ServerMessage::Other));
        assert_eq!(m.affected_channel(), None);
    }

    #[test]
    fn message_event_exposes_channel_and_id() {
        // Invented ids — no real instance data. A live `Message` carries
        // the full object inline; we read `channel` and the `_id` (the new
        // last_message_id used for live unread state).
        let json = r#"{"type":"Message","_id":"M0001","channel":"C0001",
                       "author":"U0001","content":"hi"}"#;
        let m: ServerMessage = serde_json::from_str(json).unwrap();
        assert_eq!(m.affected_channel(), Some("C0001"));
        match m {
            ServerMessage::Message { id, channel } => {
                assert_eq!(id, "M0001");
                assert_eq!(channel, "C0001");
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[test]
    fn channel_ack_parses_channel_and_message() {
        // Invented ids — no real instance data.
        let json = r#"{"type":"ChannelAck","id":"C0001","user":"U0001",
                       "message_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV"}"#;
        let m: ServerMessage = serde_json::from_str(json).unwrap();
        // An ack is not a message-list event.
        assert_eq!(m.affected_channel(), None);
        match m {
            ServerMessage::ChannelAck { id, message_id } => {
                assert_eq!(id, "C0001");
                assert_eq!(message_id, "01ARZ3NDEKTSV4RRFFQ69G5FAV");
            }
            other => panic!("expected ChannelAck, got {other:?}"),
        }
    }

    #[test]
    fn message_delete_exposes_channel() {
        let m: ServerMessage =
            serde_json::from_str(r#"{"type":"MessageDelete","id":"M0001","channel":"C0002"}"#)
                .unwrap();
        assert!(matches!(m, ServerMessage::MessageDelete { .. }));
        assert_eq!(m.affected_channel(), Some("C0002"));
    }

    #[test]
    fn react_event_accepts_channel_id_alias() {
        // Bonfire names the field `channel_id` on reaction events.
        let json = r#"{"type":"MessageReact","id":"M0001","channel_id":"C0003",
                       "user_id":"U0001","emoji_id":"01"}"#;
        let m: ServerMessage = serde_json::from_str(json).unwrap();
        assert_eq!(m.affected_channel(), Some("C0003"));
    }

    #[test]
    fn channel_create_parses_inline_channel() {
        // Verified shape: the channel fields sit at top level alongside
        // `type`. Invented ids — no real instance data.
        let json = r#"{"type":"ChannelCreate","channel_type":"TextChannel",
                       "_id":"C0001","server":"S0001","name":"new"}"#;
        let m: ServerMessage = serde_json::from_str(json).unwrap();
        // A structural event invalidates no single channel.
        assert_eq!(m.affected_channel(), None);
        match m {
            ServerMessage::ChannelCreate(c) => {
                assert_eq!(c.id, "C0001");
                assert_eq!(c.server.as_deref(), Some("S0001"));
                assert_eq!(c.name.as_deref(), Some("new"));
            }
            other => panic!("expected ChannelCreate, got {other:?}"),
        }
    }

    #[test]
    fn channel_update_parses_partial_data_and_clear() {
        let json = r#"{"type":"ChannelUpdate","id":"C0001",
                       "data":{"name":"renamed"},"clear":[]}"#;
        let m: ServerMessage = serde_json::from_str(json).unwrap();
        match m {
            ServerMessage::ChannelUpdate { id, data, clear } => {
                assert_eq!(id, "C0001");
                assert_eq!(data.name.as_deref(), Some("renamed"));
                assert!(clear.is_empty());
            }
            other => panic!("expected ChannelUpdate, got {other:?}"),
        }
    }

    #[test]
    fn channel_delete_parses_id() {
        let m: ServerMessage =
            serde_json::from_str(r#"{"type":"ChannelDelete","id":"C0001"}"#).unwrap();
        assert!(matches!(m, ServerMessage::ChannelDelete { id } if id == "C0001"));
    }

    #[test]
    fn server_update_parses_categories_full_list() {
        // Category add/remove/rename all arrive as a full categories list.
        let json = r#"{"type":"ServerUpdate","id":"S0001","data":{"categories":[
                        {"id":"cat1","title":"General","channels":["C0001"]},
                        {"id":"cat2","title":"Voice","channels":[]}]},"clear":[]}"#;
        let m: ServerMessage = serde_json::from_str(json).unwrap();
        match m {
            ServerMessage::ServerUpdate { id, data, .. } => {
                assert_eq!(id, "S0001");
                let cats = data.categories.expect("categories present");
                assert_eq!(cats.len(), 2);
                assert_eq!(cats[0].id, "cat1");
                assert_eq!(cats[0].channels, vec!["C0001"]);
                assert!(data.channels.is_none());
                assert!(data.name.is_none());
            }
            other => panic!("expected ServerUpdate, got {other:?}"),
        }
    }

    #[test]
    fn server_update_parses_channel_list() {
        let json = r#"{"type":"ServerUpdate","id":"S0001",
                       "data":{"channels":["C0001","C0002"]},"clear":[]}"#;
        let m: ServerMessage = serde_json::from_str(json).unwrap();
        match m {
            ServerMessage::ServerUpdate { data, .. } => {
                assert_eq!(data.channels.unwrap(), vec!["C0001", "C0002"]);
            }
            other => panic!("expected ServerUpdate, got {other:?}"),
        }
    }

    #[test]
    fn serializes_authenticate_and_ping() {
        let a = serde_json::to_string(&ClientMessage::Authenticate {
            token: "tok".into(),
        })
        .unwrap();
        assert_eq!(a, r#"{"type":"Authenticate","token":"tok"}"#);
        let p = serde_json::to_string(&ClientMessage::Ping { data: 7 }).unwrap();
        assert_eq!(p, r#"{"type":"Ping","data":7}"#);
    }
}
