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

use crate::bookmark_store::SqlBookmarkStore;
use crate::client::{JiraClient, JiraTicket};

mod anonymize;
mod attachment;
mod auth_bridge;
mod cache;
mod comment;
pub mod config;
mod create;
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
use types::{bookmark_node_type, issue_node_type, label_node_type, user_node_type};
use util::other_err;

/// File-name suffix for a saved-query body: Jira queries are JQL.
/// Used both as the on-disk extension and as the name the external editor
/// sees, so the two can never disagree.
const QUERY_BODY_SUFFIX: &str = ".jql";

pub struct JiraAdapter {
    auth: Arc<AuthBridge>,
    connection_name: String,
    instance_id: String,
    cache: Arc<Mutex<JiraCache>>,
    db: Arc<DatabaseConnection>,
    saved_queries: FsQueryStore,
    bookmarks: Arc<dyn BookmarkStore>,
    /// Glyph for the `bookmarked` marker column (config `bookmark_marker`,
    /// default `★`). Threaded into `JiraRoot` and emitted per row.
    bookmark_marker: String,
    /// Base directory for the persistent per-ticket workspace
    /// (`edit (markdown)` + `export workspace`). Config `ticket_workspace`
    /// (tilde-expanded) or, when unset, `<instance_root>/tickets`. Handed to
    /// each `JiraIssueNode` built at a `get_by_id`/`get_child` boundary.
    workspace_base: Arc<std::path::PathBuf>,
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
        bookmark_marker: String,
        ticket_workspace: Option<String>,
    ) -> Self {
        let cache = Arc::new(Mutex::new(JiraCache::new(Some(db.clone()), scope_id)));
        hydrate_from_db(&cache, &db, scope_id);

        let instance_root = dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("not_yet_done")
            .join("jira")
            .join(&instance_id);

        // Ticket workspace base: honour `ticket_workspace` (tilde-expanded)
        // when set, otherwise fall back to `<instance_root>/tickets`.
        let workspace_base = Arc::new(match ticket_workspace {
            Some(raw) if !raw.trim().is_empty() => {
                std::path::PathBuf::from(not_yet_done_content::download::expand_tilde(raw.trim()))
            }
            _ => instance_root.join("tickets"),
        });

        // Bookmarks live in the cache DB keyed by `scope_id` (per server),
        // not under the per-`instance_id` FS root — so the normal tickets
        // subtab and the bookmarks subtab share one set even when their
        // view-files carry different `adapter.id`s. sea-orm stays hidden
        // behind the `BookmarkStore` trait (see `SqlBookmarkStore`).
        let bookmarks: Arc<dyn BookmarkStore> =
            Arc::new(SqlBookmarkStore::new(Arc::clone(&db), scope_id));

        Self {
            auth,
            connection_name,
            instance_id,
            cache,
            db,
            saved_queries: FsQueryStore::new(instance_root.join("queries"), QUERY_BODY_SUFFIX),
            bookmarks,
            bookmark_marker,
            workspace_base,
        }
    }
}

#[async_trait]
impl ContentAdapter for JiraAdapter {
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
            workspace_base: Arc::clone(&self.workspace_base),
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
                let comment = comments
                    .into_iter()
                    .find(|c| c.id == comment_id)
                    .ok_or_else(|| {
                        other_err(format!("Comment {comment_id} not found on {issue_key}"))
                    })?;
                return Ok(Box::new(JiraCommentNode::new(
                    client,
                    comment,
                    issue_key.to_string(),
                )));
            }
            if let Some(attachment_id) = rest.strip_prefix("attachment/") {
                let attachments = client.get_attachments(issue_key).await.map_err(other_err)?;
                let attachment = attachments
                    .into_iter()
                    .find(|a| a.id == attachment_id)
                    .ok_or_else(|| {
                        other_err(format!(
                            "Attachment {attachment_id} not found on {issue_key}"
                        ))
                    })?;
                return Ok(Box::new(JiraAttachmentNode::new(
                    client,
                    attachment,
                    issue_key.to_string(),
                )));
            }
        }
        // Lazy: build the node with the key only. The full detail is
        // fetched on first `detail()` await — child operations like
        // `list_attachments` only need the key and may still succeed even
        // when the user lacks issue-level read permission.
        Ok(Box::new(
            JiraIssueNode::from_key(
                client,
                Arc::clone(&self.cache),
                id.to_string(),
                String::new(),
                Some(Arc::clone(&self.bookmarks)),
            )
            .with_workspace_base(Arc::clone(&self.workspace_base)),
        ))
    }

    /// The single source of truth about what lives under a Jira node: the
    /// root lists `jira:issue` / `jira:bookmark` / `jira:label` / `jira:user`
    /// rows; an issue lists `jira:comment` / `jira:attachment` children;
    /// comments and attachments are leaves. Each `list` callback fetches lazily
    /// through the same free functions the legacy per-node `list` delegates to,
    /// reconstructing state from adapter fields (`auth`/`cache`/`bookmarks`/
    /// `bookmark_marker`) plus `node.id()` (the issue key for a `jira:issue`),
    /// so the type set, its sort columns and its fetch can never disagree.
    fn childs<'a>(&'a self, node: &'a dyn Node) -> Vec<Child<'a>> {
        match node.node_type().type_id.as_str() {
            "jira:root" => vec![
                Child {
                    node_type: issue_node_type(),
                    columns: jql::issue_columns(),
                    list: Box::new(move |params| {
                        Box::pin(async move {
                            let client = self.auth.get_client().await.map_err(other_err)?;
                            list_issues(&client, &self.bookmarks, &self.bookmark_marker, params)
                                .await
                        })
                    }),
                },
                Child {
                    node_type: bookmark_node_type(),
                    columns: jql::bookmark_columns(),
                    list: Box::new(move |params| {
                        Box::pin(async move {
                            let client = self.auth.get_client().await.map_err(other_err)?;
                            list_bookmarked_issues(
                                &client,
                                &self.bookmarks,
                                &self.bookmark_marker,
                                params,
                            )
                            .await
                        })
                    }),
                },
                Child {
                    node_type: label_node_type(),
                    columns: Vec::new(),
                    list: Box::new(move |_params| {
                        Box::pin(async move {
                            let client = self.auth.get_client().await.map_err(other_err)?;
                            list_labels(&client, &self.cache).await
                        })
                    }),
                },
                Child {
                    node_type: user_node_type(),
                    columns: Vec::new(),
                    list: Box::new(move |_params| {
                        Box::pin(async move {
                            let client = self.auth.get_client().await.map_err(other_err)?;
                            list_users(&client, &self.cache).await
                        })
                    }),
                },
            ],
            "jira:issue" => {
                // The issue key is the node's id (a `get_by_id`/`get_child`
                // node carries it directly); the comment/attachment listings
                // need only that plus the client (and, for comments, the cache).
                let key = node.id().to_string();
                let key2 = key.clone();
                vec![
                    Child {
                        node_type: types::comment_node_type(),
                        columns: Vec::new(),
                        list: Box::new(move |_params| {
                            Box::pin(async move {
                                let client = self.auth.get_client().await.map_err(other_err)?;
                                issue::list_comments(&client, &self.cache, &key).await
                            })
                        }),
                    },
                    Child {
                        node_type: types::attachment_node_type(),
                        columns: Vec::new(),
                        list: Box::new(move |_params| {
                            Box::pin(async move {
                                let client = self.auth.get_client().await.map_err(other_err)?;
                                issue::list_attachments(&client, &key2).await
                            })
                        }),
                    },
                ]
            }
            _ => Vec::new(),
        }
    }

    /// Realism anonymizer: keeps issue keys key-shaped, assignees as person
    /// names, filenames with extensions. The safe StandardAnonymizer is the
    /// fallback for anything unrecognised. See [`anonymize`](self::anonymize).
    fn anonymizer(&self) -> std::sync::Arc<dyn not_yet_done_content::Anonymizer> {
        std::sync::Arc::new(anonymize::JiraAnonymizer::default())
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            supports_create: true,
            supports_delete: false,
            supports_search: true,
            supports_batch_download: false,
            supports_total_count: true,
            supports_tree_aggregation: false,
            propagates_query_to_subtree: false,
            group_by_via_adapter: false,
            supports_eager_subtree: false,
            ..Default::default()
        }
    }

    fn actions_for_type(&self, node_type: &NodeType) -> Vec<NodeAction> {
        match node_type.type_id.as_str() {
            "jira:root" => vec![create::create_action()],
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

    fn query_body_suffix(&self) -> &str {
        QUERY_BODY_SUFFIX
    }

    async fn load_view_sort(&self, scope: &str) -> Result<Vec<SortKey>> {
        use crate::entity::view_sort_state;
        use sea_orm::EntityTrait;
        let row = view_sort_state::Entity::find_by_id(scope.to_string())
            .one(self.db.as_ref())
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))?;
        Ok(row
            .map(|r| not_yet_done_content::sort_serde::parse(&r.sort))
            .unwrap_or_default())
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
            active
                .update(self.db.as_ref())
                .await
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
    /// Ticket-workspace base, forwarded to every issue node built via
    /// `get_child` so actions dispatched during tree navigation see the same
    /// persistent folder as the `get_by_id` path.
    workspace_base: Arc<std::path::PathBuf>,
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

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        let detail = fetch_issue(&self.client, &self.cache, id)
            .await
            .map_err(other_err)?;
        Ok(Box::new(
            JiraIssueNode::new(Arc::clone(&self.client), Arc::clone(&self.cache), detail)
                .with_workspace_base(Arc::clone(&self.workspace_base)),
        ))
    }

    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        match (action_id, input) {
            ("create", ActionInput::Form(values)) => {
                create::execute_create(&self.client, &values).await
            }
            (other, _) => Err(ContentError::NotSupported(format!(
                "execute: unknown action {other}"
            ))),
        }
    }
}

/// Map a [`JiraTicket`] to the list-row [`NodeSummary`] shared by the normal
/// and the bookmarks listing, so both render identical issue columns. When
/// `bookmarked_at` is `Some`, an extra `bookmarked_at` field is appended for
/// the bookmarks view's locally-sortable column. `bookmarked` drives the
/// `bookmarked` marker column on the tickets list (so a row shows at a glance
/// whether it is bookmarked); it is always `true` for bookmarks-view rows.
/// `marker` is the configured glyph shown for a bookmarked row (blank
/// otherwise).
fn issue_summary(
    t: JiraTicket,
    bookmarked_at: Option<String>,
    bookmarked: bool,
    marker: &str,
) -> NodeSummary {
    let mut fields = vec![
        MetadataField {
            key: "bookmarked".into(),
            value: if bookmarked {
                marker.to_string()
            } else {
                String::new()
            },
            display_label: "Bookmark".into(),
            editable: false,
            allowed_values: None,
        },
        MetadataField {
            key: "key".into(),
            value: t.key.clone(),
            display_label: "Key".into(),
            editable: false,
            allowed_values: None,
        },
        // The title lives in `label` *and* here. `label` is the structural
        // slot every front-end renders a node by; the field is what makes
        // `summary` a column like any other — sortable and filterable
        // without anyone having to know it is also the label.
        MetadataField {
            key: "summary".into(),
            value: t.summary.clone(),
            display_label: "Summary".into(),
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
            key: "creator".into(),
            value: t.creator,
            display_label: "Creator".into(),
            editable: false,
            allowed_values: None,
        },
        MetadataField {
            key: "fix_versions".into(),
            value: t.fix_versions,
            display_label: "Fix Versions".into(),
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
    ];
    if let Some(stamp) = bookmarked_at {
        fields.push(MetadataField {
            key: "bookmarked_at".into(),
            value: stamp,
            display_label: "Bookmarked".into(),
            editable: false,
            allowed_values: None,
        });
    }
    NodeSummary {
        id: t.key.clone(),
        label: t.summary,
        node_type: issue_node_type(),
        metadata: Metadata { fields },
        has_children: None,
    }
}

/// List issues via JQL search — the single fetch source behind both the
/// root's legacy `list` (for `jira:issue`) and the adapter's
/// [`ContentAdapter::childs`] declaration. Reconstructs from adapter state
/// (`client`, `bookmarks`, `bookmark_marker`); no per-node fields are needed.
async fn list_issues(
    client: &Arc<JiraClient>,
    bookmarks: &Arc<dyn BookmarkStore>,
    bookmark_marker: &str,
    params: ListParams,
) -> Result<ListResult> {
    let base_jql = params
        .query
        .as_deref()
        .unwrap_or("assignee = currentUser() ORDER BY updated DESC");

    let (jql, applied_sort) = jql::apply_order_by(base_jql, &params.sort);

    let page_req = params.page.unwrap_or(PageRequest {
        offset: 0,
        limit: 50,
    });
    let page = client
        .search(&jql, page_req.offset, page_req.limit)
        .await
        .map_err(other_err)?;

    // Bookmark set for the `bookmarked` marker column. One store read
    // per listing; toggling a bookmark reloads the pane, so the marker
    // stays in sync. A store error must not break the issue list — fall
    // back to "nothing bookmarked".
    let bookmarked: std::collections::HashSet<String> = bookmarks
        .list()
        .await
        .map(|bs| bs.into_iter().map(|b| b.id).collect())
        .unwrap_or_default();

    let returned = page.tickets.len() as u64;
    let items = page
        .tickets
        .into_iter()
        .map(|t| {
            let is_bm = bookmarked.contains(&t.key);
            issue_summary(t, None, is_bm, bookmark_marker)
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

/// Bookmarks view: list exactly the issues recorded in the bookmark
/// store, as ordinary `jira:issue` rows. One JQL `key in (...)` call (no
/// `ORDER BY`); the synthetic `bookmarked_at` column is injected per row
/// and any requested sort is applied locally via [`apply_sort`]. An empty
/// store short-circuits without touching the network. Single fetch source
/// behind the root's legacy `list` (for `jira:bookmark`) and `childs`.
async fn list_bookmarked_issues(
    client: &Arc<JiraClient>,
    bookmarks: &Arc<dyn BookmarkStore>,
    bookmark_marker: &str,
    params: ListParams,
) -> Result<ListResult> {
    let bookmarks = bookmarks.list().await?;
    if bookmarks.is_empty() {
        return Ok(ListResult {
            items: vec![],
            applied_sort: Vec::new(),
            page: None,
            batch_download_available: false,
            downloaded: vec![],
        });
    }

    // key -> bookmarked_at, to stamp each returned row.
    let stamps: std::collections::HashMap<String, String> = bookmarks
        .iter()
        .map(|b| (b.id.clone(), b.bookmarked_at.clone()))
        .collect();

    let keys: Vec<String> = bookmarks.iter().map(|b| b.id.clone()).collect();
    let jql = format!("key in ({})", keys.join(","));
    let limit = keys.len() as u32;
    let page = client.search(&jql, 0, limit).await.map_err(other_err)?;

    let mut items: Vec<NodeSummary> = page
        .tickets
        .into_iter()
        .map(|t| {
            let stamp = stamps.get(&t.key).cloned();
            issue_summary(t, stamp, true, bookmark_marker)
        })
        .collect();

    let applied_sort = apply_sort(&mut items, &params.sort, &jql::bookmark_columns());

    Ok(ListResult {
        items,
        applied_sort,
        page: None,
        batch_download_available: false,
        downloaded: vec![],
    })
}

/// List labels (from cache, refreshed from Jira). Single fetch source behind
/// the root's legacy `list` (for `jira:label`) and `childs`.
async fn list_labels(
    client: &Arc<JiraClient>,
    cache: &Arc<Mutex<JiraCache>>,
) -> Result<ListResult> {
    let labels = client.all_labels().await.map_err(other_err)?;
    cache::persist_labels(cache, labels.clone()).await;
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

/// List users (from cache, refreshed from Jira). Single fetch source behind
/// the root's legacy `list` (for `jira:user`) and `childs`.
async fn list_users(client: &Arc<JiraClient>, cache: &Arc<Mutex<JiraCache>>) -> Result<ListResult> {
    let users = client.all_users().await.map_err(other_err)?;
    cache::persist_users(cache, users.clone()).await;
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

/// Shared test-only builder for a `JiraAdapter` backed by an in-memory
/// SQLite DB and a dummy cookie auth spec. It is enough to drive the
/// `childs`-derived child helpers (`children::child_types`), which only read
/// `node.node_type()`/`node.id()` and never touch auth or the network. Used by
/// the per-node test modules that assert child-type declarations.
#[cfg(test)]
pub(crate) async fn test_adapter() -> JiraAdapter {
    use crate::auth_session_store::SqlAuthSessionStore;
    use sea_orm::Database;

    let db = Database::connect("sqlite::memory:")
        .await
        .expect("open in-memory");
    db.get_schema_registry("not_yet_done_jira_adapter::entity::*")
        .sync(&db)
        .await
        .expect("schema sync");
    let db = Arc::new(db);

    let spec: AuthSpec = serde_yaml::from_str(
        r#"
mechanism: cookie
bindings:
  - field: cookie
    provider:
      type: command
      script: /bin/true
"#,
    )
    .expect("parse auth spec");

    let scope_id = Uuid::new_v4();
    let store = SqlAuthSessionStore::new(Arc::clone(&db), scope_id);
    let auth = AuthBridge::new("http://localhost:0".into(), false, spec, Box::new(store))
        .expect("auth bridge");

    JiraAdapter::from_parts(
        auth,
        "test".into(),
        "test".into(),
        db,
        scope_id,
        "★".into(),
        None,
    )
}

#[cfg(test)]
mod bookmark_marker_tests {
    use super::*;

    fn ticket() -> JiraTicket {
        JiraTicket {
            key: "TEST-1".into(),
            summary: "A ticket".into(),
            status: "Open".into(),
            priority: "High".into(),
            assignee: "me".into(),
            creator: "someone else".into(),
            fix_versions: "1.2.0".into(),
            issue_type: "Bug".into(),
            updated: "2026-06-30".into(),
            attachments_count: 0,
        }
    }

    fn bookmarked_field(s: &NodeSummary) -> &str {
        s.metadata
            .fields
            .iter()
            .find(|f| f.key == "bookmarked")
            .map(|f| f.value.as_str())
            .expect("bookmarked field present")
    }

    #[test]
    fn issue_summary_marks_bookmarked_rows() {
        let yes = issue_summary(ticket(), None, true, "★");
        let no = issue_summary(ticket(), None, false, "★");
        assert_eq!(bookmarked_field(&yes), "★");
        assert_eq!(bookmarked_field(&no), "");
    }

    #[test]
    fn issue_summary_honours_custom_marker() {
        let s = issue_summary(ticket(), None, true, "");
        assert_eq!(bookmarked_field(&s), "");
        let blank = issue_summary(ticket(), None, false, "");
        assert_eq!(bookmarked_field(&blank), "");
    }

    #[test]
    fn bookmarks_view_rows_are_always_marked() {
        // list_bookmarked_issues passes `true`; the synthetic bookmarked_at
        // column is independent of the marker.
        let s = issue_summary(ticket(), Some("2026-06-30T10:00:00Z".into()), true, "★");
        assert_eq!(bookmarked_field(&s), "★");
        assert!(s.metadata.fields.iter().any(|f| f.key == "bookmarked_at"));
    }
}
