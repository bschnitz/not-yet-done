//! In-memory source of truth for the chat tree.
//!
//! Rebuilt wholesale from the WS `Ready` event on every (re)connect —
//! deliberately **not** persisted to SQLite (chat state is volatile and
//! the authoritative copy lives on the server). Wrapped in an
//! `Arc<RwLock<…>>` so the gateway task writes it while `Node::list`
//! reads it synchronously (no network await for the tree structure).

use std::collections::HashMap;

use super::protocol::{Channel, ChannelPatch, Server, ServerPatch, User};

#[derive(Default, Debug)]
pub struct StoatState {
    /// True once a `Ready` has been applied on the current connection.
    pub connected: bool,
    pub servers: HashMap<String, Server>,
    pub channels: HashMap<String, Channel>,
    pub users: HashMap<String, User>,
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
