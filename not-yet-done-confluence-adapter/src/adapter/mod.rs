//! Confluence ContentAdapter — CF-3 slice.
//!
//! Adapter owns an [`AuthBridge`] and a cache [`DatabaseConnection`]
//! (CF-2a/b). `root()` lists `confluence:space` children via the live
//! `/rest/api/space` endpoint and returns a [`ConfluenceRoot`] that
//! paginates over the same call.

use std::sync::Arc;

use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use not_yet_done_content::*;

mod attachment;
mod auth_bridge;
mod comment;
mod conflict_banner;
mod create_template;
mod factory;
mod page;
mod space;

pub use factory::ConfluenceAdapterFactory;

use crate::client::ConfluenceClient;
use attachment::attachment_actions;
use auth_bridge::AuthBridge;
use comment::comment_actions;
use page::{ConfluencePageNode, page_actions, page_node_type};
use space::{ConfluenceSpaceNode, space_actions, space_node_type};

pub struct ConfluenceAdapter {
    auth: Arc<AuthBridge>,
    instance_id: String,
    connection_name: String,
    base_url: String,
    db: Arc<DatabaseConnection>,
    scope_id: Uuid,
    saved_queries: FsSavedQueryStore,
    /// CF-16: when `Some`, the spaces listing is restricted to these keys
    /// and emitted in the configured order. `None` keeps the historic
    /// behaviour of paginating through every readable space.
    space_keys: Option<Vec<String>>,
}

impl ConfluenceAdapter {
    pub(in crate::adapter) fn from_parts(
        auth: Arc<AuthBridge>,
        instance_id: String,
        connection_name: String,
        base_url: String,
        db: Arc<DatabaseConnection>,
        scope_id: Uuid,
        space_keys: Option<Vec<String>>,
    ) -> Self {
        let queries_root = dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("not_yet_done")
            .join("confluence")
            .join(&instance_id)
            .join("queries");
        Self {
            auth,
            instance_id,
            connection_name,
            base_url,
            db,
            scope_id,
            saved_queries: FsSavedQueryStore::new(queries_root),
            space_keys,
        }
    }

    /// Scope-id this adapter writes under. Exposed for tests; the rest
    /// of the adapter uses it via [`Self::db`] and the entity helpers.
    #[allow(dead_code)]
    pub(crate) fn scope_id(&self) -> Uuid {
        self.scope_id
    }
}

pub(in crate::adapter) fn other_err(e: impl std::fmt::Display) -> ContentError {
    ContentError::Other(e.to_string().into())
}

#[async_trait]
impl ContentAdapter for ConfluenceAdapter {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn adapter_type(&self) -> &str {
        "confluence"
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    async fn root(&self) -> Result<Box<dyn Node>> {
        let client = self.auth.get_client().await.map_err(other_err)?;
        Ok(Box::new(ConfluenceRoot {
            client,
            base_url: self.base_url.clone(),
            connection_name: self.connection_name.clone(),
            space_keys: self.space_keys.clone(),
        }))
    }

    /// Route an opaque id to the right Node kind. Confluence content ids
    /// are numeric strings (pages, comments — comments use composite ids
    /// like `<page_id>/comment/<id>`); space keys are alphabetic
    /// (`DEMO`, `MX`, …). The tree-expand path calls this for every
    /// expanded parent — if a numeric page id were treated as a space
    /// key, the SpaceNode's lazy homepage lookup would issue
    /// `/rest/api/space/<page_id>?expand=homepage` and get a 404.
    /// Composite ids (page id followed by `/attachment/…` or
    /// `/comment/…`) flow through [`ConfluencePageNode::get_child`] so
    /// the matching leaf node is synthesized in one place.
    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
        let client = self.auth.get_client().await.map_err(other_err)?;
        match classify_id(id) {
            IdKind::Page { head, composite } => {
                let page = ConfluencePageNode::new(
                    Arc::clone(&client),
                    &self.base_url,
                    crate::client::PageMeta {
                        id: head.to_string(),
                        title: head.to_string(),
                        page_type: "page".into(),
                        webui: String::new(),
                        has_children: None,
                    },
                );
                if composite {
                    page.get_child(id).await
                } else {
                    Ok(Box::new(page))
                }
            }
            IdKind::Space => Ok(Box::new(ConfluenceSpaceNode::new(
                client,
                &self.base_url,
                // Synthesize a minimal SpaceMeta — children listings will
                // refresh from the network anyway; the open-in-browser
                // action needs only the key to construct `/spaces/<KEY>`.
                crate::client::SpaceMeta {
                    id: 0,
                    key: id.to_string(),
                    name: id.to_string(),
                    space_type: String::new(),
                    webui: format!("/spaces/{id}"),
                    // Lookup-path: homepage id resolves lazily on first
                    // page-listing call (SpaceNode fetches it on demand).
                    homepage_id: String::new(),
                },
            ))),
        }
    }

    fn actions_for_type(&self, node_type: &NodeType) -> Vec<NodeAction> {
        match node_type.type_id.as_str() {
            "confluence:space" => space_actions(),
            "confluence:page" => page_actions(),
            "confluence:attachment" => attachment_actions(),
            "confluence:comment" => comment_actions(),
            _ => Vec::new(),
        }
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            supports_create: false,
            supports_delete: false,
            supports_search: false,
            supports_batch_download: false,
            supports_total_count: false,
            supports_tree_aggregation: false,
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
        Ok(row
            .map(|r| not_yet_done_content::sort_serde::parse(&r.sort))
            .unwrap_or_default())
    }

    async fn search_in_tree(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Option<TreeSearchResults>> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Some(TreeSearchResults { hits: Vec::new(), truncated: false }));
        }
        let cql = build_tree_find_cql(trimmed, self.space_keys.as_deref());
        let client = self.auth.get_client().await.map_err(other_err)?;
        let results = client
            .cql_search(&cql, 0, limit)
            .await
            .map_err(other_err)?;
        let hits = sort_hits_in_tree_order(
            results
                .items
                .into_iter()
                .filter_map(row_to_hit)
                .collect(),
            self.space_keys.as_deref(),
        );
        Ok(Some(TreeSearchResults { hits, truncated: results.has_next }))
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
// ConfluenceRoot — virtual root, children are `confluence:space` rows
// loaded via the live `/rest/api/space` endpoint. Pages (CF-4) hang off
// the SpaceNode, not the root.
// ---------------------------------------------------------------------------

struct ConfluenceRoot {
    client: Arc<ConfluenceClient>,
    base_url: String,
    connection_name: String,
    /// CF-16: optional whitelist passed through from
    /// [`ConfluenceAdapter::space_keys`]. `None` keeps the historic
    /// paginate-everything behaviour; `Some(keys)` server-filters via
    /// repeated `spaceKey=` params and reorders client-side.
    space_keys: Option<Vec<String>>,
}

impl ConfluenceRoot {
    fn type_for_root() -> &'static NodeType {
        static T: std::sync::LazyLock<NodeType> = std::sync::LazyLock::new(|| NodeType {
            type_id: "confluence:root".into(),
            mime_type: "application/x-not-yet-done-root".into(),
            syntax: None,
            file_extension: String::new(),
            display_name: "Confluence Root".into(),
        });
        &T
    }
}

/// Default page size for `/rest/api/space` — matches Confluence's own
/// default of 25 if no `limit` is supplied. Kept conservative because the
/// endpoint has no `total` field, so larger pages mostly just delay the
/// "have we hit the end" signal.
const DEFAULT_SPACE_PAGE_SIZE: u32 = 50;

/// Default page size for the page-listing endpoints (top-pages /
/// child-pages). Same reasoning as [`DEFAULT_SPACE_PAGE_SIZE`].
pub(in crate::adapter) const DEFAULT_PAGE_PAGE_SIZE: u32 = 50;

#[async_trait]
impl Node for ConfluenceRoot {
    fn id(&self) -> &str {
        "root"
    }

    fn label(&self) -> &str {
        &self.connection_name
    }

    fn node_type(&self) -> &NodeType {
        ConfluenceRoot::type_for_root()
    }

    fn metadata(&self) -> &Metadata {
        static EMPTY: std::sync::LazyLock<Metadata> = std::sync::LazyLock::new(Metadata::default);
        &EMPTY
    }

    fn children_types(&self) -> Vec<NodeType> {
        vec![space_node_type(), page_node_type()]
    }

    async fn list(&self, params: ListParams) -> Result<ListResult> {
        match params.node_type.type_id.as_str() {
            "confluence:space" => self.list_spaces(params).await,
            "confluence:page" => self.list_cql_results(params).await,
            other => Err(ContentError::NotSupported(format!(
                "ConfluenceRoot does not list {other}"
            ))),
        }
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        // CQL-view drill: numeric ids belong to pages (and other content
        // types — for CF-8 read-only we treat them uniformly as page
        // nodes). Alphabetic keys (`DEMO`, `MX`, …) belong to spaces.
        if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) {
            return Ok(Box::new(ConfluencePageNode::new(
                Arc::clone(&self.client),
                &self.base_url,
                crate::client::PageMeta {
                    id: id.to_string(),
                    title: id.to_string(),
                    page_type: "page".into(),
                    webui: String::new(),
                    has_children: None,
                },
            )));
        }
        // CF-3 first cut: lookup by key without a dedicated REST round-trip.
        // CF-4+ will switch to `GET /space/{KEY}?expand=homepage` so we get
        // the canonical name + type instead of falling back to the key.
        Ok(Box::new(ConfluenceSpaceNode::new(
            Arc::clone(&self.client),
            &self.base_url,
            crate::client::SpaceMeta {
                id: 0,
                key: id.to_string(),
                name: id.to_string(),
                space_type: String::new(),
                webui: format!("/spaces/{id}"),
                // Lookup-path: homepage id resolves lazily on first
                // page-listing call (SpaceNode fetches it on demand).
                homepage_id: String::new(),
            },
        )))
    }
}

impl ConfluenceRoot {
    async fn list_spaces(&self, params: ListParams) -> Result<ListResult> {
        let page_req = params.page.unwrap_or(PageRequest {
            offset: 0,
            limit: DEFAULT_SPACE_PAGE_SIZE,
        });
        // CF-16: when a whitelist is configured, the server already
        // returns ≤ |keys| rows and pagination is moot — request the
        // whole list at once and reorder client-side. Without a
        // whitelist, keep the historic paginated path.
        let (spaces, page_info) = match self.space_keys.as_deref() {
            Some(keys) if !keys.is_empty() => {
                let limit = std::cmp::max(keys.len() as u32, page_req.limit);
                let page = self
                    .client
                    .list_spaces_filtered(0, limit, keys)
                    .await
                    .map_err(other_err)?;
                let ordered = reorder_spaces_by_keys(page.spaces, keys);
                let info = PageInfo {
                    offset: 0,
                    limit,
                    total: Some(ordered.len() as u64),
                    has_next: false,
                    has_prev: false,
                };
                (ordered, info)
            }
            _ => {
                let page = self
                    .client
                    .list_spaces(page_req.offset, page_req.limit)
                    .await
                    .map_err(other_err)?;
                let info = PageInfo {
                    offset: page.start,
                    limit: page.limit,
                    total: None,
                    has_next: page.has_next,
                    has_prev: page.start > 0,
                };
                (page.spaces, info)
            }
        };
        let items = spaces
            .into_iter()
            .map(|space| NodeSummary {
                id: space.key.clone(),
                label: space.name.clone(),
                node_type: space_node_type(),
                metadata: Metadata {
                    fields: vec![
                        MetadataField {
                            key: "key".into(),
                            value: space.key,
                            display_label: "Key".into(),
                            editable: false,
                            allowed_values: None,
                        },
                        MetadataField {
                            key: "type".into(),
                            value: space.space_type,
                            display_label: "Type".into(),
                            editable: false,
                            allowed_values: None,
                        },
                        MetadataField {
                            key: "id".into(),
                            value: space.id.to_string(),
                            display_label: "ID".into(),
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
            page: Some(page_info),
            batch_download_available: false,
            downloaded: Vec::new(),
        })
    }

    /// CF-8 CQL listing path. `params.query` carries the saved-query
    /// body (a raw CQL string) — the view-YAML `query.default` is the
    /// fallback when no saved query is applied. Results are flattened
    /// into `NodeSummary` rows with `confluence:page` node-type even
    /// for blogposts/attachments/comments — drill-into-row uses the
    /// page node's `get_child` to navigate further, which is correct
    /// for pages and harmless for the others until CF-12+.
    async fn list_cql_results(&self, params: ListParams) -> Result<ListResult> {
        let cql = params.query.as_deref().unwrap_or(
            // Same default the example saved-query mirrors. Kept here as
            // the safety-net so the view always shows *something* on
            // first load even before any saved query is applied.
            "type = page ORDER BY lastModified DESC",
        );
        let page_req = params.page.unwrap_or(PageRequest {
            offset: 0,
            limit: DEFAULT_PAGE_PAGE_SIZE,
        });
        let results = self
            .client
            .cql_search(cql, page_req.offset, page_req.limit)
            .await
            .map_err(other_err)?;
        let items = results
            .items
            .into_iter()
            .map(|row| NodeSummary {
                id: row.id.clone(),
                label: row.title.clone(),
                node_type: page_node_type(),
                metadata: Metadata {
                    fields: vec![
                        MetadataField {
                            key: "id".into(),
                            value: row.id,
                            display_label: "ID".into(),
                            editable: false,
                            allowed_values: None,
                        },
                        MetadataField {
                            key: "title".into(),
                            value: row.title,
                            display_label: "Title".into(),
                            editable: false,
                            allowed_values: None,
                        },
                        MetadataField {
                            key: "type".into(),
                            value: row.content_type,
                            display_label: "Type".into(),
                            editable: false,
                            allowed_values: None,
                        },
                        MetadataField {
                            key: "space".into(),
                            value: row.space_key,
                            display_label: "Space".into(),
                            editable: false,
                            allowed_values: None,
                        },
                        MetadataField {
                            key: "modified".into(),
                            value: row.last_modified,
                            display_label: "Modified".into(),
                            editable: false,
                            allowed_values: None,
                        },
                    ],
                },
                has_children: None,
            })
            .collect();
        let page_info = PageInfo {
            offset: results.start,
            limit: results.limit,
            total: None,
            has_next: results.has_next,
            has_prev: results.start > 0,
        };
        Ok(ListResult {
            items,
            applied_sort: Vec::new(),
            page: Some(page_info),
            batch_download_available: false,
            downloaded: Vec::new(),
        })
    }
}

/// Classifier output for [`classify_id`]. Splits the "is this a page-
/// flavoured id" question into its three observable shapes so callers
/// can route without re-parsing.
#[derive(Debug, PartialEq, Eq)]
enum IdKind<'a> {
    /// Numeric head segment — a Confluence content id. `composite` is
    /// true when the id carries more than the head (e.g.
    /// `<page_id>/comment/<c_id>`); the caller delegates those to
    /// [`ConfluencePageNode::get_child`].
    Page { head: &'a str, composite: bool },
    /// Anything else — treated as a space key.
    Space,
}

/// Decide whether an opaque id from `get_by_id` is a Confluence content
/// id (numeric, possibly with composite suffix) or a space key.
/// Extracted as a pure function so the routing decision can be unit-
/// tested without going through the network-validating `AuthBridge`.
fn classify_id(id: &str) -> IdKind<'_> {
    let head = id.split('/').next().unwrap_or("");
    if !head.is_empty() && head.chars().all(|c| c.is_ascii_digit()) {
        return IdKind::Page {
            head,
            composite: head.len() != id.len(),
        };
    }
    IdKind::Space
}

/// CT-3: build the CQL for [`ContentAdapter::search_in_tree`]. Matches
/// the query against title OR text, restricted to pages and (if the
/// adapter carries a whitelist) the configured spaces. The user-typed
/// query is sanitised — `"` and `\` are stripped rather than escaped
/// because the field-syntax `~ "..."` is a Lucene fuzzy-match where
/// preserving operator characters would silently change semantics.
pub(crate) fn build_tree_find_cql(query: &str, space_keys: Option<&[String]>) -> String {
    let sanitized: String = query
        .chars()
        .filter(|c| *c != '"' && *c != '\\')
        .collect();
    let mut cql = format!(
        "(title ~ \"{sanitized}\" OR text ~ \"{sanitized}\") AND type = page"
    );
    if let Some(keys) = space_keys {
        if !keys.is_empty() {
            let list: Vec<String> = keys
                .iter()
                .map(|k| {
                    let k = k.replace('"', "");
                    format!("\"{k}\"")
                })
                .collect();
            cql.push_str(&format!(" AND space in ({})", list.join(",")));
        }
    }
    cql
}

/// CT-3: lift a [`crate::client::SearchResultMeta`] row into a
/// [`TreeFindHit`]. Skips rows without a space key (orphans the tree
/// view cannot address) and rows without an id.
fn row_to_hit(row: crate::client::SearchResultMeta) -> Option<TreeFindHit> {
    if row.id.is_empty() || row.space_key.is_empty() {
        return None;
    }
    let mut path: Vec<String> = Vec::with_capacity(row.ancestors.len() + 2);
    path.push(row.space_key.clone());
    path.extend(row.ancestors.iter().map(|a| a.id.clone()));
    path.push(row.id.clone());
    Some(TreeFindHit {
        path,
        label: row.title,
        space_key: row.space_key,
    })
}

/// CT-3: sort hits in tree-render order so the TUI can step forward /
/// backward without consulting any extra state.
///
/// Primary key: index in the configured `space_keys` (or alpha by key
/// when no whitelist is set — that matches the Confluence /space
/// listing's natural order).
/// Secondary key: full path lexicographic — places parents in front of
/// their children (shorter paths sort first) and siblings together.
pub(crate) fn sort_hits_in_tree_order(
    mut hits: Vec<TreeFindHit>,
    space_keys: Option<&[String]>,
) -> Vec<TreeFindHit> {
    use std::collections::HashMap;
    let space_rank: HashMap<String, usize> = space_keys
        .map(|keys| {
            keys.iter()
                .enumerate()
                .map(|(i, k)| (k.clone(), i))
                .collect()
        })
        .unwrap_or_default();
    hits.sort_by(|a, b| {
        let ra = space_rank.get(&a.space_key).copied().unwrap_or(usize::MAX);
        let rb = space_rank.get(&b.space_key).copied().unwrap_or(usize::MAX);
        ra.cmp(&rb)
            .then_with(|| a.space_key.cmp(&b.space_key))
            .then_with(|| a.path.cmp(&b.path))
    });
    hits
}

/// CF-16: reorder `spaces` so they appear in the same sequence as the
/// configured `keys`. Spaces whose key is not in `keys` are dropped;
/// keys that have no matching space are silently skipped (Confluence
/// returns nothing for unknown / inaccessible space keys, and a typo
/// in the YAML shouldn't brick the entire listing). Stable for keys
/// that appear more than once in `spaces`.
fn reorder_spaces_by_keys(
    spaces: Vec<crate::client::SpaceMeta>,
    keys: &[String],
) -> Vec<crate::client::SpaceMeta> {
    use std::collections::HashMap;
    let mut by_key: HashMap<String, crate::client::SpaceMeta> =
        spaces.into_iter().map(|s| (s.key.clone(), s)).collect();
    keys.iter().filter_map(|k| by_key.remove(k)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{AncestorMeta, SearchResultMeta};
    use not_yet_done_content::{
        AuthMechanism, AuthSpec, CredentialBinding, CredentialProvider, InMemorySessionStore,
        SessionCachePolicy,
    };
    use sea_orm::Database;

    async fn fresh_db() -> Arc<DatabaseConnection> {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("open in-memory");
        db.get_schema_registry("not_yet_done_confluence_adapter::entity::*")
            .sync(&db)
            .await
            .expect("schema sync");
        Arc::new(db)
    }

    fn dummy_auth_bridge() -> Arc<AuthBridge> {
        let spec = AuthSpec {
            mechanism: AuthMechanism::Cookie,
            bindings: vec![CredentialBinding {
                field: "cookie".to_string(),
                provider: CredentialProvider::Literal {
                    value: "JSESSIONID=test".to_string(),
                },
                label: None,
                masked: None,
            }],
            session_cache: SessionCachePolicy::UntilRejected,
        };
        AuthBridge::new(
            "https://wiki.example.invalid".to_string(),
            false,
            spec,
            Box::new(InMemorySessionStore::new()),
        )
        .expect("bridge")
    }

    #[tokio::test]
    async fn view_sort_save_load_delete_roundtrip() {
        let db = fresh_db().await;
        let adapter = ConfluenceAdapter::from_parts(
            dummy_auth_bridge(),
            "instance-a".into(),
            "wiki".into(),
            "https://wiki.example.invalid".into(),
            db,
            Uuid::new_v4(),
            None,
        );

        assert!(
            adapter
                .load_view_sort("confluence:spaces")
                .await
                .unwrap()
                .is_empty()
        );

        let sort = vec![
            SortKey {
                column: "title".into(),
                direction: SortDirection::Asc,
            },
            SortKey {
                column: "updated".into(),
                direction: SortDirection::Desc,
            },
        ];
        adapter
            .save_view_sort("confluence:spaces", &sort)
            .await
            .expect("save");
        let loaded = adapter.load_view_sort("confluence:spaces").await.unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].column, "title");
        assert_eq!(loaded[1].direction, SortDirection::Desc);

        adapter
            .save_view_sort("confluence:spaces", &[])
            .await
            .expect("delete via empty");
        assert!(
            adapter
                .load_view_sort("confluence:spaces")
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// Regression: tree-expand on a page row called
    /// `adapter.get_by_id(page_id)` which used to always classify as a
    /// space. The SpaceNode's lazy homepage lookup then hit
    /// `/rest/api/space/<page_id>?expand=homepage` and the server
    /// answered with 404, killing the expand. Numeric ids must classify
    /// as page so the recursive children pull from
    /// `/content/{id}/child/page` instead.
    #[test]
    fn classify_id_routes_numeric_to_page() {
        assert_eq!(
            classify_id("98765"),
            IdKind::Page {
                head: "98765",
                composite: false
            }
        );
    }

    /// Alphabetic ids stay on the space path — the original lookup-path
    /// behaviour for direct space references.
    #[test]
    fn classify_id_routes_alpha_to_space() {
        assert_eq!(classify_id("DEMO"), IdKind::Space);
    }

    /// Mixed-case alphanumeric (some spaces have digits in their key,
    /// e.g. `DEMO2`). As long as the head segment isn't fully numeric,
    /// it's a space key.
    #[test]
    fn classify_id_routes_alphanumeric_to_space() {
        assert_eq!(classify_id("DEMO2"), IdKind::Space);
    }

    /// Composite ids (page id + `/comment/…` or `/attachment/…`) route
    /// to the page path with `composite = true`, so the adapter
    /// delegates to `ConfluencePageNode::get_child` instead of
    /// duplicating the suffix parser.
    #[test]
    fn classify_id_marks_composite_page_paths() {
        assert_eq!(
            classify_id("12345/attachment/att999"),
            IdKind::Page {
                head: "12345",
                composite: true
            }
        );
        assert_eq!(
            classify_id("12345/comment/c1001"),
            IdKind::Page {
                head: "12345",
                composite: true
            }
        );
    }

    /// Empty input shouldn't crash the classifier — `split('/').next()`
    /// returns `""` for `""`, which the all-digits check rejects via the
    /// non-empty guard. Falls through to space (the SpaceNode-with-
    /// empty-key will surface a server-side 404 instead of a panic).
    #[test]
    fn classify_id_handles_empty_input() {
        assert_eq!(classify_id(""), IdKind::Space);
    }

    // ---- CT-3: tree-find helpers --------------------------------------

    #[test]
    fn build_tree_find_cql_no_whitelist() {
        let cql = build_tree_find_cql("alpha", None);
        assert_eq!(
            cql,
            r#"(title ~ "alpha" OR text ~ "alpha") AND type = page"#
        );
    }

    #[test]
    fn build_tree_find_cql_with_whitelist() {
        let keys = vec!["DEMO".into(), "MX".into()];
        let cql = build_tree_find_cql("alpha", Some(&keys));
        assert_eq!(
            cql,
            r#"(title ~ "alpha" OR text ~ "alpha") AND type = page AND space in ("DEMO","MX")"#
        );
    }

    #[test]
    fn build_tree_find_cql_strips_quotes_and_backslashes() {
        // Stops a stray `"` from breaking out of the CQL string literal.
        let cql = build_tree_find_cql(r#"foo " AND \\bar"#, None);
        assert!(!cql.contains('\\'));
        // Both occurrences of the sanitized term land in the OR clause.
        let occurrences = cql.matches("foo  AND bar").count();
        assert_eq!(occurrences, 2);
    }

    #[test]
    fn build_tree_find_cql_empty_whitelist_omits_space_clause() {
        let keys: Vec<String> = Vec::new();
        let cql = build_tree_find_cql("alpha", Some(&keys));
        assert!(!cql.contains("space in"));
    }

    fn mk_hit(space: &str, path_tail: &[&str], label: &str) -> TreeFindHit {
        let mut path = vec![space.to_string()];
        path.extend(path_tail.iter().map(|s| s.to_string()));
        TreeFindHit {
            path,
            label: label.to_string(),
            space_key: space.to_string(),
        }
    }

    #[test]
    fn sort_hits_in_tree_order_uses_yaml_space_order() {
        let hits = vec![
            mk_hit("BETA", &["1"], "b1"),
            mk_hit("ALPHA", &["1"], "a1"),
            mk_hit("BETA", &["2"], "b2"),
            mk_hit("ALPHA", &["2"], "a2"),
        ];
        // YAML whitelist puts BETA before ALPHA → BETA hits come first.
        let keys = vec!["BETA".into(), "ALPHA".into()];
        let sorted = sort_hits_in_tree_order(hits, Some(&keys));
        assert_eq!(
            sorted.iter().map(|h| h.label.as_str()).collect::<Vec<_>>(),
            vec!["b1", "b2", "a1", "a2"]
        );
    }

    #[test]
    fn sort_hits_in_tree_order_no_whitelist_falls_back_to_alpha() {
        let hits = vec![
            mk_hit("BETA", &["1"], "b1"),
            mk_hit("ALPHA", &["1"], "a1"),
        ];
        let sorted = sort_hits_in_tree_order(hits, None);
        assert_eq!(sorted[0].space_key, "ALPHA");
        assert_eq!(sorted[1].space_key, "BETA");
    }

    #[test]
    fn sort_hits_path_lex_groups_siblings_and_orders_ancestors_first() {
        // Within one space: a parent page and two of its children.
        // Lexicographic path order places the parent first, then its
        // descendants — the shape a depth-first tree walker would emit.
        let hits = vec![
            mk_hit("S", &["10", "30"], "child2"),
            mk_hit("S", &["10"], "parent"),
            mk_hit("S", &["10", "20"], "child1"),
        ];
        let sorted = sort_hits_in_tree_order(hits, None);
        assert_eq!(
            sorted.iter().map(|h| h.label.as_str()).collect::<Vec<_>>(),
            vec!["parent", "child1", "child2"]
        );
    }

    #[test]
    fn row_to_hit_drops_orphans() {
        let row = SearchResultMeta {
            id: "12345".into(),
            content_type: "page".into(),
            title: "Orphan".into(),
            webui: String::new(),
            space_key: String::new(),
            last_modified: String::new(),
            ancestors: Vec::new(),
        };
        assert!(row_to_hit(row).is_none());
    }

    #[test]
    fn row_to_hit_builds_path_space_ancestors_self() {
        let row = SearchResultMeta {
            id: "12345".into(),
            content_type: "page".into(),
            title: "Hit".into(),
            webui: String::new(),
            space_key: "DEMO".into(),
            last_modified: String::new(),
            ancestors: vec![
                AncestorMeta { id: "1000".into(), title: "Top".into() },
                AncestorMeta { id: "1100".into(), title: "Mid".into() },
            ],
        };
        let hit = row_to_hit(row).expect("non-orphan");
        assert_eq!(hit.path, vec!["DEMO", "1000", "1100", "12345"]);
        assert_eq!(hit.space_key, "DEMO");
        assert_eq!(hit.label, "Hit");
    }

    #[tokio::test]
    async fn subscribe_status_yields_initial_value() {
        let db = fresh_db().await;
        let adapter = ConfluenceAdapter::from_parts(
            dummy_auth_bridge(),
            "instance-a".into(),
            "wiki".into(),
            "https://wiki.example.invalid".into(),
            db,
            Uuid::new_v4(),
            None,
        );
        let rx = adapter.subscribe_status();
        // `watch::Receiver::borrow()` always observes the initial value,
        // so this is a non-blocking smoke test that the channel is wired.
        let _ = rx.borrow().clone();
    }

    fn synth_space(key: &str, name: &str) -> crate::client::SpaceMeta {
        crate::client::SpaceMeta {
            id: 0,
            key: key.into(),
            name: name.into(),
            space_type: "global".into(),
            webui: format!("/spaces/{key}"),
            homepage_id: String::new(),
        }
    }

    #[test]
    fn reorder_follows_yaml_sequence_not_api_order() {
        // Server returns alphabetical; YAML asks for the reverse — the
        // result must come back in YAML order.
        let api = vec![
            synth_space("AAA", "Apples"),
            synth_space("BBB", "Bananas"),
            synth_space("CCC", "Cherries"),
        ];
        let keys = vec!["CCC".into(), "AAA".into(), "BBB".into()];
        let out = reorder_spaces_by_keys(api, &keys);
        let observed: Vec<_> = out.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(observed, vec!["CCC", "AAA", "BBB"]);
    }

    #[test]
    fn reorder_drops_api_rows_not_in_keys() {
        // Server still includes a tenant the user didn't whitelist (e.g.
        // a personal space). It must not leak into the result.
        let api = vec![
            synth_space("KEEP", "Keep me"),
            synth_space("DROP", "Drop me"),
        ];
        let keys = vec!["KEEP".into()];
        let out = reorder_spaces_by_keys(api, &keys);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].key, "KEEP");
    }

    #[test]
    fn reorder_tolerates_missing_keys() {
        // Typos in YAML or spaces the user lost access to: the missing
        // key is silently skipped, the rest still come through in order.
        let api = vec![synth_space("AAA", "Apples"), synth_space("BBB", "Bananas")];
        let keys = vec!["AAA".into(), "GONE".into(), "BBB".into()];
        let out = reorder_spaces_by_keys(api, &keys);
        let observed: Vec<_> = out.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(observed, vec!["AAA", "BBB"]);
    }

    #[test]
    fn reorder_empty_keys_returns_empty() {
        // Explicit empty whitelist would produce an empty listing — but
        // the adapter handles `Some(empty)` as "no filter" upstream, so
        // this helper just needs to behave like a normal filter.
        let api = vec![synth_space("AAA", "A")];
        let out = reorder_spaces_by_keys(api, &[]);
        assert!(out.is_empty());
    }
}
