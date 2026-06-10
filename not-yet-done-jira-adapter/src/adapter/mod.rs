//! Jira ContentAdapter implementation.
//!
//! Maps the existing `JiraClient` to the generic `ContentAdapter` trait
//! interface from `not-yet-done-content`. The bulk of the editing /
//! template logic lives in [`issue`]; cache, auth bootstrap, factory, and
//! the small per-node modules each get their own file.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use not_yet_done_content::*;

use crate::client::JiraClient;

mod attachment;
mod auth_bridge;
mod cache;
mod comment;
mod config;
mod factory;
mod issue;
mod jql;
mod types;
mod util;

pub use factory::JiraAdapterFactory;

use attachment::JiraAttachmentNode;
use auth_bridge::AuthBridge;
use cache::{JiraCache, fetch_comments, fetch_issue, hydrate_from_db};
use comment::JiraCommentNode;
use issue::JiraIssueNode;
use types::{issue_node_type, label_node_type, user_node_type};
use util::other_err;

pub struct JiraAdapter {
    auth: Arc<AuthBridge>,
    connection_name: String,
    instance_id: String,
    cache: Arc<Mutex<JiraCache>>,
    db: Arc<DatabaseConnection>,
    saved_queries: FsSavedQueryStore,
}

impl JiraAdapter {
    /// Build from a pre-built [`AuthBridge`] and an already-opened DB
    /// connection. Both are produced by the factory, which resolves
    /// `cfg.db.url` into a pooled connection and wires the bridge
    /// against the [`AuthOrchestrator`].
    pub(in crate::adapter) fn from_parts(
        auth: Arc<AuthBridge>,
        connection_name: String,
        instance_id: String,
        db: Arc<DatabaseConnection>,
        scope_id: Uuid,
    ) -> Self {
        let cache = Arc::new(Mutex::new(JiraCache::new(Some(db.clone()), scope_id)));
        hydrate_from_db(&cache, &db, scope_id);

        let queries_root = dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("not_yet_done")
            .join("jira")
            .join(&instance_id)
            .join("queries");

        Self {
            auth,
            connection_name,
            instance_id,
            cache,
            db,
            saved_queries: FsSavedQueryStore::new(queries_root),
        }
    }
}

#[async_trait]
impl ContentAdapter for JiraAdapter {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn adapter_type(&self) -> &str {
        "jira"
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    async fn root(&self) -> Result<Box<dyn Node>> {
        let client = self.auth.get_client().await.map_err(other_err)?;
        Ok(Box::new(JiraRoot {
            client,
            cache: Arc::clone(&self.cache),
            connection_name: self.connection_name.clone(),
        }))
    }

    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
        let client = self.auth.get_client().await.map_err(other_err)?;
        // Composite IDs: "{issue_key}/comment/{comment_id}" or
        //                "{issue_key}/attachment/{attachment_id}"
        if let Some((issue_key, rest)) = id.split_once('/') {
            if let Some(comment_id) = rest.strip_prefix("comment/") {
                let comments = fetch_comments(&client, &self.cache, issue_key)
                    .await
                    .map_err(other_err)?;
                let comment = comments.into_iter()
                    .find(|c| c.id == comment_id)
                    .ok_or_else(|| other_err(format!("Comment {comment_id} not found on {issue_key}")))?;
                return Ok(Box::new(JiraCommentNode::new(
                    client, comment, issue_key.to_string(),
                )));
            }
            if let Some(attachment_id) = rest.strip_prefix("attachment/") {
                let attachments = client.get_attachments(issue_key).await.map_err(other_err)?;
                let attachment = attachments.into_iter()
                    .find(|a| a.id == attachment_id)
                    .ok_or_else(|| other_err(format!("Attachment {attachment_id} not found on {issue_key}")))?;
                return Ok(Box::new(JiraAttachmentNode::new(
                    client, attachment, issue_key.to_string(),
                )));
            }
        }
        // Lazy: build the node with the key only. The full detail is
        // fetched on first `detail()` await — child operations like
        // `list_attachments` only need the key and may still succeed even
        // when the user lacks issue-level read permission.
        Ok(Box::new(JiraIssueNode::from_key(
            client,
            Arc::clone(&self.cache),
            id.to_string(),
            String::new(),
        )))
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            supports_create: false,
            supports_delete: false,
            supports_search: true,
            supports_batch_download: false,
            supports_total_count: true,
            supports_tree_aggregation: false,
        }
    }

    fn actions_for_type(&self, node_type: &NodeType) -> Vec<NodeAction> {
        match node_type.type_id.as_str() {
            "jira:issue" => issue::issue_actions(),
            "jira:comment" => comment::comment_actions(),
            "jira:attachment" => attachment::attachment_actions(),
            _ => Vec::new(),
        }
    }

    fn subscribe_status(&self) -> tokio::sync::watch::Receiver<AdapterStatus> {
        self.auth.subscribe_status()
    }

    async fn invalidate_session(&self) -> Result<()> {
        self.auth.invalidate_session().await;
        Ok(())
    }

    async fn invalidate_credentials(&self) -> Result<()> {
        self.auth.invalidate_credentials().await;
        Ok(())
    }

    fn saved_query_store(&self) -> Option<&dyn SavedQueryStore> {
        Some(&self.saved_queries)
    }

    async fn load_view_sort(&self, scope: &str) -> Result<Vec<SortKey>> {
        use crate::entity::view_sort_state;
        use sea_orm::EntityTrait;
        let row = view_sort_state::Entity::find_by_id(scope.to_string())
            .one(self.db.as_ref())
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))?;
        Ok(row.map(|r| not_yet_done_content::sort_serde::parse(&r.sort)).unwrap_or_default())
    }

    async fn save_view_sort(&self, scope: &str, sort: &[SortKey]) -> Result<()> {
        use crate::entity::view_sort_state;
        use sea_orm::{ActiveModelTrait, EntityTrait, Set};
        if sort.is_empty() {
            view_sort_state::Entity::delete_by_id(scope.to_string())
                .exec(self.db.as_ref())
                .await
                .map_err(|e| ContentError::Other(Box::new(e)))?;
            return Ok(());
        }
        let value = not_yet_done_content::sort_serde::serialize(sort);
        let existing = view_sort_state::Entity::find_by_id(scope.to_string())
            .one(self.db.as_ref())
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))?;
        if let Some(model) = existing {
            let mut active: view_sort_state::ActiveModel = model.into();
            active.sort = Set(value);
            active.update(self.db.as_ref()).await
                .map_err(|e| ContentError::Other(Box::new(e)))?;
        } else {
            view_sort_state::ActiveModel {
                scope: Set(scope.to_string()),
                sort: Set(value),
            }
            .insert(self.db.as_ref())
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// JiraRoot — virtual root node, children are issues via JQL search
// ---------------------------------------------------------------------------

struct JiraRoot {
    client: Arc<JiraClient>,
    cache: Arc<Mutex<JiraCache>>,
    connection_name: String,
}

#[async_trait]
impl Node for JiraRoot {
    fn id(&self) -> &str {
        "root"
    }

    fn label(&self) -> &str {
        &self.connection_name
    }

    fn node_type(&self) -> &NodeType {
        static ROOT_TYPE: std::sync::LazyLock<NodeType> = std::sync::LazyLock::new(|| NodeType {
            type_id: "jira:root".into(),
            mime_type: "".into(),
            syntax: None,
            file_extension: "".into(),
            display_name: "Jira Root".into(),
        });
        &ROOT_TYPE
    }

    fn metadata(&self) -> &Metadata {
        static EMPTY: Metadata = Metadata { fields: vec![] };
        &EMPTY
    }

    fn children_types(&self) -> Vec<NodeType> {
        vec![issue_node_type(), label_node_type(), user_node_type()]
    }

    fn sortable_columns(&self, node_type: &NodeType) -> Vec<SortableColumn> {
        match node_type.type_id.as_str() {
            "jira:issue" => jql::issue_sortable_columns(),
            _ => Vec::new(),
        }
    }

    async fn list(&self, params: ListParams) -> Result<ListResult> {
        match params.node_type.type_id.as_str() {
            "jira:issue" => self.list_issues(params).await,
            "jira:label" => self.list_labels().await,
            "jira:user" => self.list_users().await,
            other => Err(ContentError::NotSupported(format!("Unknown node type: {other}"))),
        }
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        let detail = fetch_issue(&self.client, &self.cache, id).await.map_err(other_err)?;
        Ok(Box::new(JiraIssueNode::new(Arc::clone(&self.client), Arc::clone(&self.cache), detail)))
    }
}

impl JiraRoot {
    async fn list_issues(&self, params: ListParams) -> Result<ListResult> {
        let base_jql = params
            .query
            .as_deref()
            .unwrap_or("assignee = currentUser() ORDER BY updated DESC");

        let (jql, applied_sort) = jql::apply_sort(base_jql, &params.sort);

        let page_req = params.page.unwrap_or(PageRequest { offset: 0, limit: 50 });
        let page = self
            .client
            .search(&jql, page_req.offset, page_req.limit)
            .await
            .map_err(other_err)?;

        let returned = page.tickets.len() as u64;
        let items = page
            .tickets
            .into_iter()
            .map(|t| NodeSummary {
                id: t.key.clone(),
                label: t.summary,
                node_type: issue_node_type(),
                metadata: Metadata {
                    fields: vec![
                        MetadataField {
                            key: "key".into(),
                            value: t.key,
                            display_label: "Key".into(),
                            editable: false,
                            allowed_values: None,
                        },
                        MetadataField {
                            key: "type".into(),
                            value: t.issue_type,
                            display_label: "Type".into(),
                            editable: false,
                            allowed_values: None,
                        },
                        MetadataField {
                            key: "status".into(),
                            value: t.status,
                            display_label: "Status".into(),
                            editable: false,
                            allowed_values: None,
                        },
                        MetadataField {
                            key: "priority".into(),
                            value: t.priority,
                            display_label: "Priority".into(),
                            editable: false,
                            allowed_values: None,
                        },
                        MetadataField {
                            key: "assignee".into(),
                            value: t.assignee,
                            display_label: "Assignee".into(),
                            editable: false,
                            allowed_values: None,
                        },
                        MetadataField {
                            key: "updated".into(),
                            value: t.updated,
                            display_label: "Updated".into(),
                            editable: false,
                            allowed_values: None,
                        },
                        MetadataField {
                            key: "attachments".into(),
                            value: t.attachments_count.to_string(),
                            display_label: "Attachm.".into(),
                            editable: false,
                            allowed_values: None,
                        },
                    ],
                },
                has_children: None,
            })
            .collect();

        let page_info = {
            let next_after = (page.start_at as u64) + returned;
            let has_next = match page.total {
                Some(total) => next_after < total,
                None => returned == page.max_results as u64,
            };
            let has_prev = page.start_at > 0;
            PageInfo {
                offset: page.start_at,
                limit: page.max_results,
                total: page.total,
                has_next,
                has_prev,
            }
        };

        Ok(ListResult {
            items,
            applied_sort,
            page: Some(page_info),
            batch_download_available: false,
            downloaded: vec![],
        })
    }

    async fn list_labels(&self) -> Result<ListResult> {
        let labels = self.ensure_labels_cached().await?;
        let items = labels
            .into_iter()
            .map(|name| NodeSummary {
                id: name.clone(),
                label: name,
                node_type: label_node_type(),
                metadata: Metadata::default(),
                has_children: None,
            })
            .collect();
        Ok(ListResult {
            items,
            applied_sort: Vec::new(),
            page: None,
            batch_download_available: false,
            downloaded: vec![],
        })
    }

    async fn list_users(&self) -> Result<ListResult> {
        let users = self.ensure_users_cached().await?;
        let items = users
            .into_iter()
            .map(|u| NodeSummary {
                id: u.name.clone(),
                label: u.display_name.clone(),
                node_type: user_node_type(),
                metadata: Metadata {
                    fields: vec![
                        MetadataField {
                            key: "username".into(),
                            value: u.name,
                            display_label: "Username".into(),
                            editable: false,
                            allowed_values: None,
                        },
                        MetadataField {
                            key: "display_name".into(),
                            value: u.display_name,
                            display_label: "Display Name".into(),
                            editable: false,
                            allowed_values: None,
                        },
                        MetadataField {
                            key: "email".into(),
                            value: u.email_address.unwrap_or_default(),
                            display_label: "Email".into(),
                            editable: false,
                            allowed_values: None,
                        },
                    ],
                },
                has_children: None,
            })
            .collect();
        Ok(ListResult {
            items,
            applied_sort: Vec::new(),
            page: None,
            batch_download_available: false,
            downloaded: vec![],
        })
    }

    /// CLI-export path: pull the full label list straight from Jira. The
    /// result is also merged into the cache so a follow-up TUI session
    /// doesn't have to refetch.
    async fn ensure_labels_cached(&self) -> Result<Vec<String>> {
        let labels = self.client.all_labels().await.map_err(other_err)?;
        cache::persist_labels(&self.cache, labels.clone()).await;
        Ok(labels)
    }

    /// CLI-export path: pull the full user list straight from Jira. The
    /// result is also merged into the cache so a follow-up TUI session
    /// doesn't have to refetch.
    async fn ensure_users_cached(&self) -> Result<Vec<crate::client::JiraUser>> {
        let users = self.client.all_users().await.map_err(other_err)?;
        cache::persist_users(&self.cache, users.clone()).await;
        Ok(users)
    }
}
