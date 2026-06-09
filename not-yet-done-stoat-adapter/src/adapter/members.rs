//! Per-server member cache feeding mention autocomplete.
//!
//! Completions are **server-scoped**: a message in a server channel may
//! only mention members of that server. We resolve the channel's server
//! from the live [`StoatState`], then fetch (and cache) that server's
//! member list. DM/group channels carry no server — there the mentionable
//! set is the channel's recipients, resolved from the `Ready` user
//! snapshot.
//!
//! The member list is fetched once per server per session and held in
//! memory. Members change rarely; a reconnect rebuilds the whole adapter
//! (and this cache) anyway, so there is no separate refresh path. A failed
//! fetch is **not** cached, so a later listing retries.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::client::StoatClient;
use crate::gateway::StoatState;

/// Lazily-populated `id → username` map per server id.
#[derive(Default)]
pub(super) struct MemberCache {
    by_server: RwLock<HashMap<String, Arc<HashMap<String, String>>>>,
}

impl MemberCache {
    /// Return the cached member map for a server, fetching it on first
    /// use. On fetch error returns an empty map without caching, so the
    /// next call retries.
    pub(super) async fn members_for(
        &self,
        server_id: &str,
        client: &StoatClient,
    ) -> Arc<HashMap<String, String>> {
        if let Some(map) = self.by_server.read().await.get(server_id) {
            return Arc::clone(map);
        }
        match client.list_server_members(server_id).await {
            Ok(map) => {
                let arc = Arc::new(map);
                self.by_server
                    .write()
                    .await
                    .insert(server_id.to_string(), Arc::clone(&arc));
                arc
            }
            // Error already logged by the client's http_log; degrade to an
            // empty completion set rather than failing the listing.
            Err(_) => Arc::new(HashMap::new()),
        }
    }
}

/// Resolve the set of mentionable users for a channel as an
/// `id → username` map: server members for a server channel, recipients
/// for a DM/group.
pub(super) async fn channel_user_map(
    state: &RwLock<StoatState>,
    members: &MemberCache,
    client: &StoatClient,
    channel_id: &str,
) -> Arc<HashMap<String, String>> {
    let (server, recipients) = {
        let st = state.read().await;
        match st.channels.get(channel_id) {
            Some(c) => (c.server.clone(), c.recipients.clone()),
            None => (None, None),
        }
    };

    if let Some(server_id) = server {
        return members.members_for(&server_id, client).await;
    }

    // DM / group: build from the recipient list via the Ready snapshot.
    let st = state.read().await;
    let map: HashMap<String, String> = recipients
        .into_iter()
        .flatten()
        .filter_map(|id| st.users.get(&id).map(|u| (id, u.username.clone())))
        .collect();
    Arc::new(map)
}
