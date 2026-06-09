//! Per-project metadata cache (statuses / members / tags).
//!
//! Slug-aware editor templates need allowed-value lists (status names,
//! assignee usernames, tag names). These come from a handful of project-
//! scoped endpoints that don't change often, so we cache them lazily for
//! the lifetime of the `TaigaClient`.
//!
//! Cache layout: `RwLock<HashMap<project_id, ProjectMeta>>`. Each
//! `ProjectMeta` is filled on first need by the relevant `ensure_*` call;
//! statuses are sub-keyed by `ItemType` because each item type has its own
//! status workflow in Taiga.

use std::collections::HashMap;

use not_yet_done_content::http_log;
use tokio::sync::RwLock;

use super::TaigaClient;
use super::query::ItemType;
use crate::cache_store;

#[derive(Clone, Debug)]
pub struct TaigaStatus {
    pub id: u64,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct TaigaMember {
    pub id: u64,
    pub username: String,
    pub full_name: String,
}

#[derive(Default)]
pub struct ProjectMeta {
    pub statuses: HashMap<ItemType, Vec<TaigaStatus>>,
    pub members: Option<Vec<TaigaMember>>,
    pub tags: Option<Vec<String>>,
}

#[derive(Default)]
pub(crate) struct ProjectMetaCache {
    inner: RwLock<HashMap<u64, ProjectMeta>>,
}

impl ProjectMetaCache {
    pub(crate) async fn statuses_snapshot(
        &self,
        project_id: u64,
        item_type: ItemType,
    ) -> Option<Vec<TaigaStatus>> {
        self.inner
            .read()
            .await
            .get(&project_id)
            .and_then(|m| m.statuses.get(&item_type).cloned())
    }

    pub(crate) async fn members_snapshot(&self, project_id: u64) -> Option<Vec<TaigaMember>> {
        self.inner
            .read()
            .await
            .get(&project_id)
            .and_then(|m| m.members.clone())
    }

    pub(crate) async fn tags_snapshot(&self, project_id: u64) -> Option<Vec<String>> {
        self.inner
            .read()
            .await
            .get(&project_id)
            .and_then(|m| m.tags.clone())
    }

    async fn store_statuses(
        &self,
        project_id: u64,
        item_type: ItemType,
        statuses: Vec<TaigaStatus>,
    ) {
        let mut g = self.inner.write().await;
        g.entry(project_id)
            .or_default()
            .statuses
            .insert(item_type, statuses);
    }

    async fn store_members(&self, project_id: u64, members: Vec<TaigaMember>) {
        let mut g = self.inner.write().await;
        g.entry(project_id).or_default().members = Some(members);
    }

    async fn store_tags(&self, project_id: u64, tags: Vec<String>) {
        let mut g = self.inner.write().await;
        g.entry(project_id).or_default().tags = Some(tags);
    }
}

impl TaigaClient {
    pub async fn ensure_statuses(
        &self,
        project_id: u64,
        item_type: ItemType,
    ) -> Result<Vec<TaigaStatus>, String> {
        if let Some(s) = self.project_meta.statuses_snapshot(project_id, item_type).await {
            return Ok(s);
        }
        if let Ok(rows) =
            cache_store::load_statuses(&self.db, self.scope_id, project_id, item_type.as_str())
                .await
        {
            if !rows.is_empty() {
                self.project_meta
                    .store_statuses(project_id, item_type, rows.clone())
                    .await;
                return Ok(rows);
            }
        }
        let segment = match item_type {
            ItemType::Task => "task-statuses",
            ItemType::Issue => "issue-statuses",
            ItemType::Epic => "epic-statuses",
            ItemType::UserStory => "userstory-statuses",
        };
        let url = format!(
            "{}/api/v1/{segment}?project={project_id}",
            self.base_url,
        );
        let headers = self.auth_headers()?;
        http_log::log_request("GET", &url);
        let resp = self
            .send_retrying("GET", &url, || self.http.get(&url).headers(headers.clone()))
            .await?;
        let resp = http_log::check_status("GET", &url, resp).await?;
        let raw: Vec<serde_json::Value> = resp
            .json()
            .await
            .map_err(|e| format!("statuses parse: {e}"))?;
        let statuses: Vec<TaigaStatus> = raw
            .into_iter()
            .filter_map(|v| {
                Some(TaigaStatus {
                    id: v.get("id").and_then(|x| x.as_u64())?,
                    name: v.get("name").and_then(|x| x.as_str())?.to_string(),
                })
            })
            .collect();
        let _ = cache_store::merge_statuses(
            &self.db,
            self.scope_id,
            project_id,
            item_type.as_str(),
            &statuses,
        )
        .await;
        self.project_meta
            .store_statuses(project_id, item_type, statuses.clone())
            .await;
        Ok(statuses)
    }

    pub async fn ensure_members(&self, project_id: u64) -> Result<Vec<TaigaMember>, String> {
        if let Some(m) = self.project_meta.members_snapshot(project_id).await {
            return Ok(m);
        }
        if let Ok(rows) = cache_store::load_members(&self.db, self.scope_id, project_id).await {
            if !rows.is_empty() {
                self.project_meta
                    .store_members(project_id, rows.clone())
                    .await;
                return Ok(rows);
            }
        }
        // `/memberships?project=N` does not include a top-level `username`
        // and does not nest `user_extra_info` (only the *inviter* gets
        // that), so we use `/users?project=N` which returns a clean list
        // of user objects with `id` / `username` / `full_name_display`.
        let url = format!("{}/api/v1/users?project={project_id}", self.base_url);
        let headers = self.auth_headers()?;
        http_log::log_request("GET", &url);
        let resp = self
            .send_retrying("GET", &url, || self.http.get(&url).headers(headers.clone()))
            .await?;
        let resp = http_log::check_status("GET", &url, resp).await?;
        let raw: Vec<serde_json::Value> = resp
            .json()
            .await
            .map_err(|e| format!("users parse: {e}"))?;
        let mut members: Vec<TaigaMember> = raw
            .into_iter()
            .filter_map(|v| {
                let id = v.get("id").and_then(|x| x.as_u64())?;
                let username = v
                    .get("username")
                    .and_then(|x| x.as_str())
                    .filter(|s| !s.is_empty())?
                    .to_string();
                let full_name = v
                    .get("full_name_display")
                    .and_then(|x| x.as_str())
                    .or_else(|| v.get("full_name").and_then(|x| x.as_str()))
                    .unwrap_or("")
                    .to_string();
                Some(TaigaMember { id, username, full_name })
            })
            .collect();
        // De-dup by username (project members can appear multiple times via roles).
        members.sort_by(|a, b| a.username.cmp(&b.username));
        members.dedup_by(|a, b| a.username == b.username);
        let _ =
            cache_store::merge_members(&self.db, self.scope_id, project_id, &members).await;
        self.project_meta
            .store_members(project_id, members.clone())
            .await;
        Ok(members)
    }

    pub async fn ensure_tags(&self, project_id: u64) -> Result<Vec<String>, String> {
        if let Some(t) = self.project_meta.tags_snapshot(project_id).await {
            return Ok(t);
        }
        if let Ok(rows) = cache_store::load_tags(&self.db, self.scope_id, project_id).await {
            if !rows.is_empty() {
                self.project_meta
                    .store_tags(project_id, rows.clone())
                    .await;
                return Ok(rows);
            }
        }
        let url = format!("{}/api/v1/projects/{project_id}", self.base_url);
        let headers = self.auth_headers()?;
        http_log::log_request("GET", &url);
        let resp = self
            .send_retrying("GET", &url, || self.http.get(&url).headers(headers.clone()))
            .await?;
        let resp = http_log::check_status("GET", &url, resp).await?;
        let raw: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("project parse: {e}"))?;
        // Taiga returns `tags_colors` as `{ "<tag>": "<#hex|null>", … }` —
        // keys are the tag names. (Older / array-of-pairs forms are kept
        // for compatibility.)
        let tags: Vec<String> = match raw.get("tags_colors") {
            Some(serde_json::Value::Object(map)) => map
                .keys()
                .filter(|k| !k.is_empty())
                .cloned()
                .collect(),
            Some(serde_json::Value::Array(arr)) => arr
                .iter()
                .filter_map(|entry| match entry {
                    serde_json::Value::Array(pair) => pair
                        .first()
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string()),
                    serde_json::Value::String(s) => {
                        if s.is_empty() { None } else { Some(s.clone()) }
                    }
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        let _ = cache_store::merge_tags(&self.db, self.scope_id, project_id, &tags).await;
        self.project_meta.store_tags(project_id, tags.clone()).await;
        Ok(tags)
    }

    /// Look up a status by name (case-sensitive). Returns the numeric id
    /// expected by the PATCH endpoint.
    pub async fn status_id_by_name(
        &self,
        project_id: u64,
        item_type: ItemType,
        name: &str,
    ) -> Result<Option<u64>, String> {
        let statuses = self.ensure_statuses(project_id, item_type).await?;
        Ok(statuses.into_iter().find(|s| s.name == name).map(|s| s.id))
    }
}
