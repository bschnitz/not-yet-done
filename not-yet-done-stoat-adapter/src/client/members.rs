//! Server member listing — the source for mention autocomplete.
//!
//! `GET /api/servers/{id}/members` returns a `members[]` array (server-
//! specific data: nickname, roles) alongside a `users[]` array carrying
//! the global account fields. We only need `id → username` to build the
//! `@uu_…` completion slugs, so we read the `users[]` side. Scoped to a
//! single server on purpose: completions must only offer people who are
//! actually in the server the channel belongs to.

use std::collections::HashMap;

use serde::Deserialize;

use not_yet_done_content::http_log;

use super::StoatClient;

#[derive(Deserialize)]
struct MembersResponse {
    #[serde(default)]
    users: Vec<MemberUser>,
}

#[derive(Deserialize)]
struct MemberUser {
    #[serde(rename = "_id")]
    id: String,
    #[serde(default)]
    username: String,
}

impl StoatClient {
    /// Fetch all members of a server as an `id → username` map. Offline
    /// members are included (`exclude_offline=false`) — a completion list
    /// that hides offline users would be surprising.
    pub async fn list_server_members(
        &self,
        server_id: &str,
    ) -> Result<HashMap<String, String>, String> {
        let url = format!("{}/api/servers/{}/members", self.base_url(), server_id);
        http_log::log_request("GET", &url);
        let resp = self
            .http
            .get(&url)
            .headers(self.auth_headers()?)
            .query(&[("exclude_offline", "false")])
            .send()
            .await
            .map_err(|e| http_log::network_error("GET", &url, e))?;
        let resp = http_log::check_status("GET", &url, resp).await?;
        let body = resp
            .json::<MembersResponse>()
            .await
            .map_err(|e| format!("parse server members: {e}"))?;
        Ok(body
            .users
            .into_iter()
            .filter(|u| !u.username.is_empty())
            .map(|u| (u.id, u.username))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_users_array_into_id_username_map() {
        // Fully invented payload — no real instance data.
        let body: MembersResponse = serde_json::from_str(
            r#"{
                "members": [
                    {"_id": {"server": "S1", "user": "U1"}, "nickname": "Ali"},
                    {"_id": {"server": "S1", "user": "U2"}}
                ],
                "users": [
                    {"_id": "U1", "username": "alice"},
                    {"_id": "U2", "username": "bob"},
                    {"_id": "U3", "username": ""}
                ]
            }"#,
        )
        .unwrap();
        let map: HashMap<String, String> = body
            .users
            .into_iter()
            .filter(|u| !u.username.is_empty())
            .map(|u| (u.id, u.username))
            .collect();
        assert_eq!(map.get("U1").map(String::as_str), Some("alice"));
        assert_eq!(map.get("U2").map(String::as_str), Some("bob"));
        // Blank usernames are dropped.
        assert!(!map.contains_key("U3"));
    }
}
