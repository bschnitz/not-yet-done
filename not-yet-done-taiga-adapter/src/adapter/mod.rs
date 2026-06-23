//! Taiga ContentAdapter.

use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use not_yet_done_content::*;

mod anonymize;
mod attachment;
mod auth_bridge;
mod comment;
mod config;
mod factory;
mod item;
mod notification;
mod types;

pub use factory::TaigaAdapterFactory;

use attachment::TaigaAttachmentNode;
use auth_bridge::AuthBridge;
use comment::TaigaCommentNode;
use item::TaigaItemNode;
use notification::TaigaNotificationNode;
use types::{
    epic_type, issue_type, item_type as taiga_item_type, node_type_for,
    notification_type, task_type, userstory_type,
};

use crate::client::{
    ItemType, TaigaClient, apply_query_sort, default_sort, parse_taiga_query, run_queries,
    sortable_column_keys,
};

pub struct TaigaAdapter {
    auth: Arc<AuthBridge>,
    connection_name: String,
    instance_id: String,
    #[allow(dead_code)] // wired in layer 2 (token reuse) and layer 3 (project meta)
    db: Arc<DatabaseConnection>,
    #[allow(dead_code)]
    scope_id: Uuid,
    saved_queries: FsSavedQueryStore,
}

impl TaigaAdapter {
    pub(in crate::adapter) fn from_parts(
        auth: Arc<AuthBridge>,
        connection_name: String,
        instance_id: String,
        db: Arc<DatabaseConnection>,
        scope_id: Uuid,
    ) -> Self {
        let queries_root = dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("not_yet_done")
            .join("taiga")
            .join(&instance_id)
            .join("queries");
        Self {
            auth,
            connection_name,
            instance_id,
            db,
            scope_id,
            saved_queries: FsSavedQueryStore::new(queries_root),
        }
    }
}

#[async_trait]
impl ContentAdapter for TaigaAdapter {
    fn adapter_type(&self) -> &str {
        "taiga"
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    async fn root(&self) -> Result<Box<dyn Node>> {
        Ok(Box::new(TaigaRoot {
            auth: Arc::clone(&self.auth),
            connection_name: self.connection_name.clone(),
        }))
    }

    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
        // Composite ids:
        //   "notification:{id}"
        //   "{type}:{id}"
        //   "{type}:{id}/comment/{comment_id}"
        //   "{type}:{id}/attachment/{attachment_id}"
        // The closure runs at most twice (retry-on-401 in `with_client`),
        // so its captures must be Clone-friendly. `id` is by &str (Copy).
        let id_owned = id.to_string();
        self.auth
            .with_client(|client| {
                let id = id_owned.clone();
                async move { build_node_from_id(client, id).await }
            })
            .await
            .map_err(|e| ContentError::Other(e.into()))
    }

    /// Realism anonymizer: keeps refs ref-shaped, assignees/authors as person
    /// names, filenames with extensions; safe StandardAnonymizer fallback.
    /// See [`anonymize`](self::anonymize).
    fn anonymizer(&self) -> std::sync::Arc<dyn not_yet_done_content::Anonymizer> {
        std::sync::Arc::new(anonymize::TaigaAnonymizer::default())
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            supports_create: false,
            supports_delete: false,
            supports_search: true,
            supports_batch_download: false,
            supports_total_count: false,
            supports_tree_aggregation: false,
            propagates_query_to_subtree: false,
            group_by_via_adapter: false,
            supports_eager_subtree: false,
        }
    }

    fn actions_for_type(&self, node_type: &NodeType) -> Vec<NodeAction> {
        match node_type.type_id.as_str() {
            "taiga:task" | "taiga:issue" | "taiga:epic" | "taiga:userstory"
            | "taiga:item" => item::item_actions(),
            "taiga:comment" => comment::comment_actions(),
            "taiga:attachment" => attachment::attachment_actions(),
            "taiga:notification" => notification::notification_actions(),
            _ => Vec::new(),
        }
    }

    fn subscribe_status(&self) -> tokio::sync::watch::Receiver<AdapterStatus> {
        self.auth.subscribe_status()
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

    async fn invalidate_session(&self) -> Result<()> {
        self.auth.invalidate_session().await;
        Ok(())
    }

    async fn invalidate_credentials(&self) -> Result<()> {
        self.auth.invalidate_credentials().await;
        Ok(())
    }

    fn query_variables(&self, query: &str) -> Vec<QueryVariable> {
        not_yet_done_content::query_vars::parse_variables(query)
    }

    fn render_query(
        &self,
        query: &str,
        vars: &std::collections::HashMap<String, String>,
    ) -> String {
        not_yet_done_content::query_vars::render(query, vars)
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

fn parse_item_id(s: &str) -> Result<(ItemType, u64)> {
    let (kind, num) = s.split_once(':').ok_or_else(|| {
        ContentError::NotFound(format!("malformed taiga id (expected type:id): {s}"))
    })?;
    let item_type = match kind {
        "task" => ItemType::Task,
        "issue" => ItemType::Issue,
        "epic" => ItemType::Epic,
        "userstory" => ItemType::UserStory,
        other => return Err(ContentError::NotFound(format!("unknown taiga type: {other}"))),
    };
    let id: u64 = num
        .parse()
        .map_err(|_| ContentError::NotFound(format!("non-numeric id: {num}")))?;
    Ok((item_type, id))
}

// ---------------------------------------------------------------------------
// TaigaRoot — virtual root, resolves view queries to merged item lists.
// ---------------------------------------------------------------------------

struct TaigaRoot {
    /// Holds `AuthBridge` instead of a fixed client so list-call paths
    /// can transparently re-authenticate when a cached JWT is rejected.
    auth: Arc<AuthBridge>,
    connection_name: String,
}

/// Free fn used by `TaigaAdapter::get_by_id` so the `with_client`
/// retry closure can re-run the whole resolution against a fresh
/// client. Returns `Result<Box<dyn Node>, String>` to match the
/// closure's return type; the caller maps `String` → `ContentError`.
async fn build_node_from_id(
    client: Arc<TaigaClient>,
    id: String,
) -> std::result::Result<Box<dyn Node>, String> {
    if let Some(notif_id) = notification::parse_notification_id(&id) {
        let all = crate::client::fetch_all_web_notifications(&client).await?;
        let n = all
            .into_iter()
            .find(|n| n.id == notif_id)
            .ok_or_else(|| format!("notification {notif_id} not found"))?;
        return Ok(Box::new(TaigaNotificationNode::new(client, n)));
    }
    let (head, tail) = match id.split_once('/') {
        Some((h, t)) => (h.to_string(), Some(t.to_string())),
        None => (id.clone(), None),
    };
    let (item_type, raw_id) =
        parse_item_id(&head).map_err(|e| format!("{e:?}"))?;

    if let Some(rest) = tail.as_deref() {
        if let Some(comment_id) = rest.strip_prefix("comment/") {
            let comments =
                crate::client::fetch_comments(&client, item_type, raw_id).await?;
            let comment = comments
                .into_iter()
                .find(|c| c.id == comment_id)
                .ok_or_else(|| {
                    format!("comment {comment_id} not found on {item_type:?}#{raw_id}")
                })?;
            return Ok(Box::new(TaigaCommentNode::new(
                Arc::clone(&client),
                comment,
                head,
                item_type,
                raw_id,
            )));
        }
        if let Some(att_str) = rest.strip_prefix("attachment/") {
            let attachment_id: u64 = att_str
                .parse()
                .map_err(|_| format!("non-numeric attachment id: {att_str}"))?;
            let item = TaigaItemNode::new(Arc::clone(&client), item_type, raw_id)
                .await
                .map_err(|e| format!("{e:?}"))?;
            let attachment = item
                .find_attachment(attachment_id)
                .await
                .map_err(|e| format!("{e:?}"))?;
            return Ok(Box::new(TaigaAttachmentNode::new(
                Arc::clone(&client),
                attachment,
                head,
            )));
        }
    }
    Ok(Box::new(
        TaigaItemNode::new(client, item_type, raw_id)
            .await
            .map_err(|e| format!("{e:?}"))?,
    ))
}

#[async_trait]
impl Node for TaigaRoot {
    fn id(&self) -> &str {
        "root"
    }

    fn label(&self) -> &str {
        &self.connection_name
    }

    fn node_type(&self) -> &NodeType {
        static ROOT_TYPE: std::sync::LazyLock<NodeType> = std::sync::LazyLock::new(|| NodeType {
            type_id: "taiga:root".into(),
            mime_type: "".into(),
            syntax: None,
            file_extension: "".into(),
            display_name: "Taiga Root".into(),
        });
        &ROOT_TYPE
    }

    fn metadata(&self) -> &Metadata {
        static EMPTY: Metadata = Metadata { fields: vec![] };
        &EMPTY
    }

    fn children_types(&self) -> Vec<NodeType> {
        vec![
            taiga_item_type().clone(),
            task_type().clone(),
            issue_type().clone(),
            epic_type().clone(),
            userstory_type().clone(),
            notification_type().clone(),
        ]
    }

    fn sortable_columns(&self, node_type: &NodeType) -> Vec<SortableColumn> {
        match node_type.type_id.as_str() {
            "taiga:item" | "taiga:task" | "taiga:issue" | "taiga:epic" | "taiga:userstory" => {
                sortable_column_keys()
                    .iter()
                    .map(|k| SortableColumn {
                        key: (*k).into(),
                        label: column_label(k).into(),
                        // Taiga sorts server-side; the kind is unused here.
                        kind: SortKind::Text,
                    })
                    .collect()
            }
            "taiga:notification" => notification::sortable_columns(),
            _ => Vec::new(),
        }
    }

    async fn list(&self, params: ListParams) -> Result<ListResult> {
        if params.node_type.type_id == "taiga:notification" {
            return self.list_notifications(params).await;
        }
        let yaml = params.query.as_deref().ok_or_else(|| {
            ContentError::Other(
                "taiga adapter requires a `query` body (mapping with `queries:` key)".into(),
            )
        })?;
        let parsed = parse_taiga_query(yaml)
            .map_err(|e| ContentError::Other(e.into()))?;
        let mut specs = parsed.queries;

        // If the view scopes itself to a single type via `node_type`, narrow
        // the merged result. This lets a `taiga:task`-typed child view reuse
        // the same query mechanism but only show one kind.
        let scope = match params.node_type.type_id.as_str() {
            "taiga:task" => Some(ItemType::Task),
            "taiga:issue" => Some(ItemType::Issue),
            "taiga:epic" => Some(ItemType::Epic),
            "taiga:userstory" => Some(ItemType::UserStory),
            _ => None, // "taiga:item" or unknown → keep all
        };
        if let Some(t) = scope {
            specs.retain(|s| s.item_type == t);
        }

        let mut items = self
            .auth
            .with_client(|client| {
                let specs = specs.clone();
                async move { run_queries(client, specs).await }
            })
            .await
            .map_err(|e| ContentError::Other(e.into()))?;

        // Sort precedence: caller-supplied → YAML default → adapter default.
        let requested_sort: Vec<SortKey> = if !params.sort.is_empty() {
            params.sort.clone()
        } else if !parsed.sort.is_empty() {
            parsed.sort.clone()
        } else {
            default_sort()
        };
        let applied_sort = apply_query_sort(&mut items, &requested_sort);

        let total = items.len() as u64;

        // Page precedence: caller-supplied → YAML page-size at offset 0 →
        // adapter default (one big page).
        let effective_page = params.page.unwrap_or_else(|| PageRequest {
            offset: 0,
            limit: parsed.page_size.unwrap_or(u32::MAX),
        });
        let start = (effective_page.offset as u64).min(total) as usize;
        let end = ((effective_page.offset as u64) + (effective_page.limit as u64))
            .min(total) as usize;
        let page_items = items.into_iter().skip(start).take(end - start).collect::<Vec<_>>();
        let returned = page_items.len() as u64;

        let summaries = page_items
            .into_iter()
            .map(item_summary_to_node_summary)
            .collect::<Vec<_>>();

        let page_info = PageInfo {
            offset: effective_page.offset,
            limit: effective_page.limit,
            total: Some(total),
            has_next: (effective_page.offset as u64) + returned < total,
            has_prev: effective_page.offset > 0,
        };

        Ok(ListResult {
            items: summaries,
            applied_sort,
            page: Some(page_info),
            batch_download_available: false,
            downloaded: vec![],
        })
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        let id_owned = id.to_string();
        self.auth
            .with_client(|client| {
                let id = id_owned.clone();
                async move { build_node_from_id(client, id).await }
            })
            .await
            .map_err(|e| ContentError::Other(e.into()))
    }
}

impl TaigaRoot {
    /// Caller-supplied `params.page` is the signal that selects the
    /// fetch strategy:
    ///
    /// - `Some(page)` → server-paginated path: one HTTP round-trip
    ///   for `page.offset / page.limit + 1`. Sort applies only to that
    ///   page (Taiga's `/web-notifications` does not expose ordering
    ///   parameters, so any global sort would be incorrect anyway).
    /// - `None` → fetch-all + client-side sort + no slicing. Used when
    ///   the YAML view has no `pagination` block.
    async fn list_notifications(&self, params: ListParams) -> Result<ListResult> {
        match params.page {
            Some(page) => self.list_notifications_paginated(page, &params.sort).await,
            None => self.list_notifications_all(&params.sort).await,
        }
    }

    async fn list_notifications_all(&self, sort: &[SortKey]) -> Result<ListResult> {
        let mut items = self
            .auth
            .with_client(|client| async move {
                crate::client::fetch_all_web_notifications(&client).await
            })
            .await
            .map_err(|e| ContentError::Other(e.into()))?;
        let applied_sort = notification::apply_sort(&mut items, sort);
        let total = items.len() as u64;
        let summaries = items
            .iter()
            .map(notification::notification_to_summary)
            .collect::<Vec<_>>();
        let page_info = PageInfo {
            offset: 0,
            limit: u32::MAX,
            total: Some(total),
            has_next: false,
            has_prev: false,
        };
        Ok(ListResult {
            items: summaries,
            applied_sort,
            page: Some(page_info),
            batch_download_available: false,
            downloaded: vec![],
        })
    }

    async fn list_notifications_paginated(
        &self,
        page: PageRequest,
        sort: &[SortKey],
    ) -> Result<ListResult> {
        // `limit == 0` is the TUI's "use server default" sentinel. We
        // omit the `?page_size=` parameter and let Taiga apply its own
        // default; the actual page size echoes back via `chunk.raw_count`
        // so `PageInfo.limit` carries it forward for `>`/`<` navigation.
        // For non-zero limits we forward both `page_size` and the offset
        // → page math (TUI navigates in whole-page steps so offset is
        // always a multiple of limit).
        let (api_page, page_size_param) = if page.limit == 0 {
            (1, None)
        } else {
            ((page.offset / page.limit) + 1, Some(page.limit))
        };
        let chunk = self
            .auth
            .with_client(|client| async move {
                crate::client::fetch_notifications_page(&client, api_page, page_size_param)
                    .await
            })
            .await
            .map_err(|e| ContentError::Other(e.into()))?;
        let mut items = chunk.items;
        // Local sort scoped to this page only — global ordering across
        // pages may be inconsistent (e.g. unread-first), which is the
        // documented trade-off for native pagination.
        let applied_sort = notification::apply_sort(&mut items, sort);

        let summaries = items
            .iter()
            .map(notification::notification_to_summary)
            .collect::<Vec<_>>();

        let total = chunk.total;
        // Effective limit: caller-supplied if non-zero, else what the
        // server actually returned. `raw_count` may be smaller than the
        // server's default on the last page — in that case `has_next` is
        // false anyway, so the under-reported limit doesn't matter.
        let effective_limit = if page.limit != 0 {
            page.limit
        } else {
            u32::try_from(chunk.raw_count).unwrap_or(u32::MAX)
        };
        let page_info = PageInfo {
            offset: page.offset,
            limit: effective_limit,
            total: Some(total),
            has_next: (page.offset as u64) + chunk.raw_count < total,
            has_prev: page.offset > 0,
        };

        Ok(ListResult {
            items: summaries,
            applied_sort,
            page: Some(page_info),
            batch_download_available: false,
            downloaded: vec![],
        })
    }
}

fn column_label(key: &str) -> &'static str {
    match key {
        "ref" => "Ref",
        "type" => "Type",
        "status" => "Status",
        "assignee" => "Assignee",
        "subject" => "Subject",
        "modified" => "Modified",
        "project" => "Project",
        _ => "",
    }
}

fn item_summary_to_node_summary(s: crate::client::ItemSummary) -> NodeSummary {
    use crate::client::ItemSummary;
    let ItemSummary {
        item_type,
        id,
        r#ref,
        project_slug,
        subject,
        status,
        assignees,
        modified,
        total_attachments,
        ..
    } = s;
    let ref_num = r#ref;
    let display_ref = match &project_slug {
        Some(slug) if !slug.is_empty() => format!("{slug}#{ref_num}"),
        _ => format!("#{ref_num}"),
    };
    let composite_id = format!("{}:{}", item_type.as_str(), id);
    NodeSummary {
        id: composite_id,
        label: subject.clone(),
        node_type: node_type_for(item_type).clone(),
        metadata: Metadata {
            fields: vec![
                MetadataField {
                    key: "ref".into(),
                    value: display_ref,
                    display_label: "Ref".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "type".into(),
                    value: item_type.as_str().to_string(),
                    display_label: "Type".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "status".into(),
                    value: status,
                    display_label: "Status".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "assignee".into(),
                    value: assignees.join(", "),
                    display_label: "Assignee".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "modified".into(),
                    value: modified.unwrap_or_default(),
                    display_label: "Modified".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "attachments".into(),
                    value: total_attachments.to_string(),
                    display_label: "Attachm.".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "subject".into(),
                    value: subject,
                    display_label: "Subject".into(),
                    editable: false,
                    allowed_values: None,
                },
            ],
        },
        has_children: None,
    }
}
