//! In-memory source of truth for the chat tree.
//!
//! Rebuilt wholesale from the WS `Ready` event on every (re)connect —
//! deliberately **not** persisted to SQLite (chat state is volatile and
//! the authoritative copy lives on the server). Wrapped in an
//! `Arc<RwLock<…>>` so the gateway task writes it while `Node::list`
//! reads it synchronously (no network await for the tree structure).

use std::collections::HashMap;

use not_yet_done_content::Invalidation;
use tokio::sync::broadcast;

use super::protocol::{Channel, ChannelPatch, Server, ServerPatch, User};

#[derive(Default, Debug)]
pub struct StoatState {
    /// True once a `Ready` has been applied on the current connection.
    pub connected: bool,
    pub servers: HashMap<String, Server>,
    pub channels: HashMap<String, Channel>,
    pub users: HashMap<String, User>,
    /// Read-state: `channel_id → last-read message id`. Seeded from
    /// `GET /api/sync/unreads` after each `Ready`, updated by `ChannelAck`
    /// WS events and local acks (send / mark-read). A channel is **unread**
    /// when its `last_message_id` is newer than its entry here (ULID
    /// lexicographic compare); a channel with messages but **no** entry has
    /// never been read and counts as unread. Deliberately kept across a
    /// reconnect (the background unread refetch overwrites it) so the tree
    /// doesn't flash every channel as unread mid-resync.
    pub reads: HashMap<String, String>,
    /// Invalidation sink, set once by the adapter at construction (the gateway
    /// shares the same `Arc<RwLock<StoatState>>`). When a read marker actually
    /// advances in [`Self::mark_read`] we push [`Invalidation::All`] here so
    /// every bound view repaints — the channel/category unread markers in the
    /// tree clear immediately on a local ack, without waiting for the server's
    /// `ChannelAck` echo (which it may not send back to the originating
    /// client). `None` in unit tests built from `StoatState::default()`.
    inv_tx: Option<broadcast::Sender<Invalidation>>,
}

impl StoatState {
    /// Replace the whole snapshot from a fresh `Ready`. Called on first
    /// connect and again after every reconnect (the server resends the
    /// full state), so we clear before inserting rather than merging.
    pub fn apply_ready(&mut self, users: Vec<User>, servers: Vec<Server>, channels: Vec<Channel>) {
        self.users = users.into_iter().map(|u| (u.id.clone(), u)).collect();
        self.servers = servers.into_iter().map(|s| (s.id.clone(), s)).collect();
        self.channels = channels.into_iter().map(|c| (c.id.clone(), c)).collect();
    }

    /// Insert (or replace) a channel — handles a `ChannelCreate`. If the
    /// channel belongs to a server we know and isn't already in its
    /// `channels` list, append it so it renders even when the matching
    /// `ServerUpdate.data.channels` doesn't arrive (idempotent; a later
    /// `ServerUpdate` replaces the list with the authoritative order).
    pub fn insert_channel(&mut self, channel: Channel) {
        if let Some(server_id) = &channel.server {
            if let Some(server) = self.servers.get_mut(server_id) {
                if !server.channels.contains(&channel.id) {
                    server.channels.push(channel.id.clone());
                }
            }
        }
        self.channels.insert(channel.id.clone(), channel);
    }

    /// Apply a partial `ChannelUpdate`. No-op if the channel is unknown.
    /// `clear` is intentionally not threaded in: none of the fields we
    /// render (name, last_message_id) are clearable in practice.
    pub fn patch_channel(&mut self, id: &str, patch: ChannelPatch) {
        if let Some(channel) = self.channels.get_mut(id) {
            if let Some(name) = patch.name {
                channel.name = Some(name);
            }
            if let Some(last) = patch.last_message_id {
                channel.last_message_id = Some(last);
            }
        }
    }

    /// Remove a channel — handles a `ChannelDelete`. Also unlinks it from
    /// its server's channel list and every category, so a stale id never
    /// lingers (the server doesn't always re-emit the channel list on
    /// delete — verified against Stoat 0.13.7).
    pub fn remove_channel(&mut self, id: &str) {
        self.channels.remove(id);
        for server in self.servers.values_mut() {
            server.channels.retain(|c| c != id);
            for cat in &mut server.categories {
                cat.channels.retain(|c| c != id);
            }
        }
    }

    /// Apply a partial `ServerUpdate`. Channel-list and category changes
    /// arrive as full-list replacements (add/remove/rename/reorder all map
    /// to one new list). No-op if the server is unknown.
    pub fn patch_server(&mut self, id: &str, patch: ServerPatch) {
        if let Some(server) = self.servers.get_mut(id) {
            if let Some(name) = patch.name {
                server.name = name;
            }
            if let Some(channels) = patch.channels {
                server.channels = channels;
            }
            if let Some(categories) = patch.categories {
                server.categories = categories;
            }
        }
    }

    /// Replace read-state from a fresh `GET /api/sync/unreads`. Each entry
    /// is `(channel_id, last_read_id)`; entries with no `last_id` (a tracked
    /// channel that has never been read) are dropped, so the channel falls
    /// back to the "no entry = unread" rule.
    pub fn apply_unreads(&mut self, entries: Vec<(String, Option<String>)>) {
        self.reads = entries
            .into_iter()
            .filter_map(|(channel, last)| last.map(|l| (channel, l)))
            .collect();
    }

    /// Set the invalidation sink (see the `inv_tx` field). Called once by the
    /// adapter right after construction, before the state is shared.
    pub fn set_invalidations(&mut self, tx: broadcast::Sender<Invalidation>) {
        self.inv_tx = Some(tx);
    }

    /// Record a local read up to `message_id` (ack-on-send, mark-read, or a
    /// `ChannelAck` echo). Monotonic: never moves the marker backwards, so
    /// an out-of-order ack for an older message can't re-flag a channel.
    ///
    /// When the marker actually advances, push [`Invalidation::All`] so the
    /// tree's channel/category unread markers repaint at once — this is the
    /// single choke point for "a read happened", so every caller (mark-read
    /// hook, ack-on-send, `ChannelAck` echo) repaints without each having to
    /// remember to. No-op emit when the marker doesn't move (duplicate/older
    /// ack → nothing changed → no repaint) or when no sink is wired (tests).
    pub fn mark_read(&mut self, channel_id: &str, message_id: &str) {
        let entry = self.reads.entry(channel_id.to_string()).or_default();
        if message_id > entry.as_str() {
            *entry = message_id.to_string();
            if let Some(tx) = &self.inv_tx {
                let _ = tx.send(Invalidation::All);
            }
        }
    }

    /// Whether `channel_id` has unread messages: its `last_message_id` is
    /// strictly newer than the last-read marker (ULID lexicographic), or it
    /// has messages but no read marker at all. Channels without messages
    /// (idle text, voice) are never unread.
    pub fn is_channel_unread(&self, channel_id: &str) -> bool {
        let Some(channel) = self.channels.get(channel_id) else {
            return false;
        };
        let Some(last_msg) = channel.last_message_id.as_deref() else {
            return false;
        };
        match self.reads.get(channel_id) {
            Some(last_read) => last_msg > last_read.as_str(),
            None => true,
        }
    }

    /// Whether a category has any unread channel (the OR over its members).
    /// `category_id` is the plain (non-composite) id within `server_id`.
    pub fn is_category_unread(&self, server_id: &str, category_id: &str) -> bool {
        self.servers
            .get(server_id)
            .and_then(|s| s.categories.iter().find(|c| c.id == category_id))
            .map(|cat| cat.channels.iter().any(|ch| self.is_channel_unread(ch)))
            .unwrap_or(false)
    }

    /// Whether a server has any unread channel (the OR over all of its
    /// channels, categorised or not). Drives the server-level unread marker,
    /// mirroring [`is_category_unread`](Self::is_category_unread) one level up.
    pub fn is_server_unread(&self, server_id: &str) -> bool {
        self.servers
            .get(server_id)
            .map(|s| s.channels.iter().any(|ch| self.is_channel_unread(ch)))
            .unwrap_or(false)
    }

    /// Channels that are direct messages or group DMs (no `server`).
    pub fn dm_channels(&self) -> impl Iterator<Item = &Channel> {
        self.channels.values().filter(|c| {
            matches!(c.channel_type.as_str(), "DirectMessage" | "Group" | "SavedMessages")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::protocol::{Category, Channel, Server, User};

    fn user(id: &str) -> User {
        User {
            id: id.into(),
            username: format!("user-{id}"),
            discriminator: None,
        }
    }

    fn text_channel(id: &str, server: &str) -> Channel {
        Channel {
            id: id.into(),
            channel_type: "TextChannel".into(),
            server: Some(server.into()),
            name: Some(format!("chan-{id}")),
            last_message_id: None,
            recipients: None,
        }
    }

    fn dm_channel(id: &str) -> Channel {
        Channel {
            id: id.into(),
            channel_type: "DirectMessage".into(),
            server: None,
            name: None,
            last_message_id: None,
            recipients: Some(vec!["U1".into(), "U2".into()]),
        }
    }

    #[test]
    fn apply_ready_replaces_snapshot() {
        let mut st = StoatState::default();
        st.apply_ready(
            vec![user("U1")],
            vec![Server {
                id: "S1".into(),
                name: "Guild".into(),
                channels: vec!["C1".into()],
                categories: vec![],
                owner: None,
            }],
            vec![text_channel("C1", "S1"), dm_channel("D1")],
        );
        assert_eq!(st.users.len(), 1);
        assert_eq!(st.servers.len(), 1);
        assert_eq!(st.channels.len(), 2);

        // A second Ready (reconnect) wholly replaces the previous one.
        st.apply_ready(vec![], vec![], vec![]);
        assert!(st.users.is_empty());
        assert!(st.servers.is_empty());
        assert!(st.channels.is_empty());
    }

    fn server(id: &str, channels: &[&str]) -> Server {
        Server {
            id: id.into(),
            name: format!("guild-{id}"),
            channels: channels.iter().map(|c| c.to_string()).collect(),
            categories: vec![],
            owner: None,
        }
    }

    #[test]
    fn insert_channel_adds_to_map_and_server_list() {
        let mut st = StoatState::default();
        st.apply_ready(vec![], vec![server("S1", &["C1"])], vec![text_channel("C1", "S1")]);
        st.insert_channel(text_channel("C2", "S1"));
        assert!(st.channels.contains_key("C2"));
        assert_eq!(st.servers["S1"].channels, vec!["C1", "C2"]);
        // Idempotent: re-inserting the same channel doesn't duplicate it.
        st.insert_channel(text_channel("C2", "S1"));
        assert_eq!(st.servers["S1"].channels, vec!["C1", "C2"]);
    }

    #[test]
    fn patch_channel_renames_known_channel_only() {
        let mut st = StoatState::default();
        st.apply_ready(vec![], vec![server("S1", &["C1"])], vec![text_channel("C1", "S1")]);
        st.patch_channel(
            "C1",
            ChannelPatch {
                name: Some("renamed".into()),
                last_message_id: None,
            },
        );
        assert_eq!(st.channels["C1"].name.as_deref(), Some("renamed"));
        // Unknown channel → silent no-op (no panic, no insert).
        st.patch_channel("C9", ChannelPatch::default());
        assert!(!st.channels.contains_key("C9"));
    }

    #[test]
    fn remove_channel_unlinks_from_server_and_category() {
        let mut st = StoatState::default();
        let mut s = server("S1", &["C1", "C2"]);
        s.categories = vec![Category {
            id: "cat1".into(),
            title: "General".into(),
            channels: vec!["C1".into()],
        }];
        st.apply_ready(
            vec![],
            vec![s],
            vec![text_channel("C1", "S1"), text_channel("C2", "S1")],
        );
        st.remove_channel("C1");
        assert!(!st.channels.contains_key("C1"));
        assert_eq!(st.servers["S1"].channels, vec!["C2"]);
        assert!(st.servers["S1"].categories[0].channels.is_empty());
    }

    #[test]
    fn patch_server_replaces_categories_and_channels() {
        let mut st = StoatState::default();
        st.apply_ready(vec![], vec![server("S1", &["C1"])], vec![text_channel("C1", "S1")]);
        st.patch_server(
            "S1",
            ServerPatch {
                name: Some("new name".into()),
                channels: Some(vec!["C1".into(), "C2".into()]),
                categories: Some(vec![Category {
                    id: "cat1".into(),
                    title: "Cat".into(),
                    channels: vec!["C1".into()],
                }]),
            },
        );
        let s = &st.servers["S1"];
        assert_eq!(s.name, "new name");
        assert_eq!(s.channels, vec!["C1", "C2"]);
        assert_eq!(s.categories.len(), 1);
        assert_eq!(s.categories[0].channels, vec!["C1"]);
    }

    /// Two ULIDs that sort `older < newer` lexicographically (the same as
    /// chronologically, by ULID design). Invented values — no real data.
    const OLDER_MSG: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    const NEWER_MSG: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAW";

    #[test]
    fn channel_unread_compares_last_message_against_read_marker() {
        let mut st = StoatState::default();
        st.apply_ready(vec![], vec![server("S1", &["C1"])], vec![{
            let mut c = text_channel("C1", "S1");
            c.last_message_id = Some(NEWER_MSG.into());
            c
        }]);

        // No read marker yet → a channel with messages is unread.
        assert!(st.is_channel_unread("C1"));

        // Read up to the latest message → no longer unread.
        st.mark_read("C1", NEWER_MSG);
        assert!(!st.is_channel_unread("C1"));

        // A new message (newer ULID arrives) → unread again.
        st.patch_channel(
            "C1",
            ChannelPatch {
                name: None,
                last_message_id: Some("01ARZ3NDEKTSV4RRFFQ69G5FAX".into()),
            },
        );
        assert!(st.is_channel_unread("C1"));

        // An idle channel (no messages) is never unread.
        st.apply_ready(vec![], vec![server("S2", &["C2"])], vec![text_channel("C2", "S2")]);
        assert!(!st.is_channel_unread("C2"));
    }

    #[test]
    fn mark_read_is_monotonic() {
        let mut st = StoatState::default();
        st.mark_read("C1", NEWER_MSG);
        // An older ack must not move the marker backwards.
        st.mark_read("C1", OLDER_MSG);
        assert_eq!(st.reads.get("C1").map(String::as_str), Some(NEWER_MSG));
    }

    #[test]
    fn mark_read_emits_all_only_when_marker_advances() {
        let (tx, mut rx) = broadcast::channel(8);
        let mut st = StoatState::default();
        st.set_invalidations(tx);

        // First read of a never-read channel advances the marker → emit.
        st.mark_read("C1", OLDER_MSG);
        assert_eq!(rx.try_recv().unwrap(), Invalidation::All);

        // Advancing to a newer message → emit again.
        st.mark_read("C1", NEWER_MSG);
        assert_eq!(rx.try_recv().unwrap(), Invalidation::All);

        // An older (out-of-order) ack doesn't move the marker → no emit.
        st.mark_read("C1", OLDER_MSG);
        assert!(rx.try_recv().is_err(), "no repaint when nothing changed");
    }

    #[test]
    fn apply_unreads_drops_entries_without_last_id() {
        let mut st = StoatState::default();
        st.apply_unreads(vec![
            ("C1".into(), Some(NEWER_MSG.into())),
            ("C2".into(), None),
        ]);
        assert_eq!(st.reads.get("C1").map(String::as_str), Some(NEWER_MSG));
        assert!(!st.reads.contains_key("C2"));
    }

    #[test]
    fn category_unread_is_or_over_member_channels() {
        let mut st = StoatState::default();
        let mut s = server("S1", &["C1", "C2"]);
        s.categories = vec![Category {
            id: "cat1".into(),
            title: "General".into(),
            channels: vec!["C1".into(), "C2".into()],
        }];
        let mut c1 = text_channel("C1", "S1");
        c1.last_message_id = Some(NEWER_MSG.into());
        let c2 = text_channel("C2", "S1"); // no messages
        st.apply_ready(vec![], vec![s], vec![c1, c2]);

        // C1 unread (no read marker) → category unread.
        assert!(st.is_category_unread("S1", "cat1"));
        // Read C1 → category no longer unread (C2 has no messages).
        st.mark_read("C1", NEWER_MSG);
        assert!(!st.is_category_unread("S1", "cat1"));
    }

    #[test]
    fn server_unread_is_or_over_all_channels() {
        let mut st = StoatState::default();
        let s = server("S1", &["C1", "C2"]);
        let mut c1 = text_channel("C1", "S1");
        c1.last_message_id = Some(NEWER_MSG.into());
        let c2 = text_channel("C2", "S1"); // no messages
        st.apply_ready(vec![], vec![s], vec![c1, c2]);

        // C1 unread (no read marker) → server unread.
        assert!(st.is_server_unread("S1"));
        // Read C1 → server no longer unread (C2 has no messages).
        st.mark_read("C1", NEWER_MSG);
        assert!(!st.is_server_unread("S1"));
        // Unknown server → not unread.
        assert!(!st.is_server_unread("S9"));
    }

    #[test]
    fn dm_channels_filters_server_channels_out() {
        let mut st = StoatState::default();
        st.apply_ready(
            vec![],
            vec![],
            vec![text_channel("C1", "S1"), dm_channel("D1")],
        );
        let dms: Vec<&str> = st.dm_channels().map(|c| c.id.as_str()).collect();
        assert_eq!(dms, vec!["D1"]);
    }
}
