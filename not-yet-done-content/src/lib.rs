//! Generic content adapter abstraction.
//!
//! Provides a frontend-agnostic interface for connecting to remote content
//! systems (ticket trackers, wikis, databases). Each backend implements the
//! same trait interface so any frontend can work with any system uniformly.

#[cfg(any(test, feature = "mock"))]
pub mod mock;

pub mod auth;
pub mod grouping;
pub mod http_log;
pub mod link_route;
pub mod node_ref;
pub mod query_vars;
pub mod slug;
pub mod sort_serde;

pub use grouping::{GroupBucket, GroupSpec};

pub use auth::{
    AuthError, AuthMechanism, AuthOrchestrator, AuthSpec, CredentialBinding, CredentialProvider,
    InMemorySessionStore, ResolvedSession, SessionCachePolicy, SessionEntry, SessionStore,
};
pub use link_route::{LinkRoute, LinkRouteError};
pub use node_ref::{NodeRef, NodeRefParseError};

use async_trait::async_trait;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum ContentError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("conflict: {0}")]
    Conflict(ConflictError),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("not supported: {0}")]
    NotSupported(String),

    #[error("{0}")]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}

pub type Result<T> = std::result::Result<T, ContentError>;

#[derive(Debug)]
pub struct ConflictError {
    pub remote_version: String,
    pub remote_content: Option<Vec<u8>>,
    pub message: String,
}

impl std::fmt::Display for ConflictError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

// ---------------------------------------------------------------------------
// NodeType
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NodeType {
    /// Unique type identifier (e.g. "jira:issue", "wiki:page", "db:row").
    pub type_id: String,
    /// MIME type of the content body (e.g. "text/plain", "text/x-jira-wiki").
    pub mime_type: String,
    /// Editor syntax identifier (e.g. "markdown", "jira", "sql").
    pub syntax: Option<String>,
    /// File extension for temporary editor files (e.g. ".md", ".jira", ".sql").
    pub file_extension: String,
    /// Human-readable label (e.g. "Issue", "Page", "Row").
    pub display_name: String,
}

// ---------------------------------------------------------------------------
// Metadata
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Metadata {
    pub fields: Vec<MetadataField>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetadataField {
    pub key: String,
    pub value: String,
    pub display_label: String,
    pub editable: bool,
    /// Allowed values (for dropdowns). None = free text.
    pub allowed_values: Option<Vec<String>>,
}

impl Metadata {
    /// Overwrite the `value` of an existing field by `key`, in place.
    ///
    /// A no-op if no field with that key exists (the caller asked to patch
    /// a column the row doesn't carry — silently ignored rather than
    /// inventing a field with an empty label). Lets a caller tweak a single
    /// column value on an existing summary without rebuilding the metadata.
    pub fn set_field(&mut self, key: &str, value: impl Into<String>) {
        if let Some(field) = self.fields.iter_mut().find(|f| f.key == key) {
            field.value = value.into();
        }
    }
}

#[cfg(test)]
mod metadata_tests {
    use super::*;

    fn field(key: &str, value: &str) -> MetadataField {
        MetadataField {
            key: key.into(),
            value: value.into(),
            display_label: key.into(),
            editable: false,
            allowed_values: None,
        }
    }

    #[test]
    fn set_field_overwrites_existing_value_in_place() {
        let mut md = Metadata {
            fields: vec![field("marker", ""), field("label", "keep")],
        };
        md.set_field("marker", "⏱");
        assert_eq!(md.fields[0].value, "⏱");
        // Other fields untouched; no field added.
        assert_eq!(md.fields[1].value, "keep");
        assert_eq!(md.fields.len(), 2);
    }

    #[test]
    fn set_field_is_noop_for_absent_key() {
        let mut md = Metadata {
            fields: vec![field("marker", "")],
        };
        md.set_field("missing", "x");
        assert_eq!(md.fields.len(), 1);
        assert_eq!(md.fields[0].value, "");
    }
}

#[cfg(test)]
mod apply_sort_tests {
    use super::*;

    fn col(key: &str, kind: SortKind) -> SortableColumn {
        SortableColumn {
            key: key.into(),
            label: key.into(),
            kind,
        }
    }

    fn key(column: &str, direction: SortDirection) -> SortKey {
        SortKey {
            column: column.into(),
            direction,
        }
    }

    /// Build a summary whose `label` is the id and which carries the given
    /// `(key, value)` metadata fields.
    fn summary(id: &str, fields: &[(&str, &str)]) -> NodeSummary {
        NodeSummary {
            id: id.into(),
            label: id.into(),
            node_type: NodeType {
                type_id: "test".into(),
                mime_type: "text/plain".into(),
                syntax: None,
                file_extension: String::new(),
                display_name: "Test".into(),
            },
            metadata: Metadata {
                fields: fields
                    .iter()
                    .map(|(k, v)| MetadataField {
                        key: (*k).into(),
                        value: (*v).into(),
                        display_label: (*k).into(),
                        editable: false,
                        allowed_values: None,
                    })
                    .collect(),
            },
            has_children: None,
        }
    }

    fn ids(items: &[NodeSummary]) -> Vec<&str> {
        items.iter().map(|s| s.id.as_str()).collect()
    }

    #[test]
    fn datetime_sort_orders_chronologically_both_directions() {
        let cols = [col("started", SortKind::DateTime)];
        let mut items = vec![
            summary("b", &[("started", "2026-06-02T10:00:00Z")]),
            summary("a", &[("started", "2026-06-01T10:00:00Z")]),
            summary("c", &[("started", "2026-06-03T10:00:00Z")]),
        ];
        let applied = apply_sort(&mut items, &[key("started", SortDirection::Asc)], &cols);
        assert_eq!(ids(&items), ["a", "b", "c"]);
        assert_eq!(applied, vec![key("started", SortDirection::Asc)]);

        apply_sort(&mut items, &[key("started", SortDirection::Desc)], &cols);
        assert_eq!(ids(&items), ["c", "b", "a"]);
    }

    #[test]
    fn number_sort_is_numeric_not_lexical() {
        let cols = [col("duration", SortKind::Number)];
        let mut items = vec![
            summary("x", &[("duration", "100")]),
            summary("y", &[("duration", "9")]),
            summary("z", &[("duration", "60")]),
        ];
        apply_sort(&mut items, &[key("duration", SortDirection::Asc)], &cols);
        // 9 < 60 < 100 numerically (lexically "100" would sort before "60").
        assert_eq!(ids(&items), ["y", "z", "x"]);
    }

    #[test]
    fn unparseable_typed_cells_sort_to_the_end_ascending() {
        let cols = [col("ended", SortKind::DateTime)];
        let mut items = vec![
            summary("running", &[("ended", "running")]),
            summary("late", &[("ended", "2026-06-02T10:00:00Z")]),
            summary("early", &[("ended", "2026-06-01T10:00:00Z")]),
        ];
        apply_sort(&mut items, &[key("ended", SortDirection::Asc)], &cols);
        assert_eq!(ids(&items), ["early", "late", "running"]);
    }

    #[test]
    fn falls_back_to_label_when_column_has_no_metadata_field() {
        // `description`/`task`-style columns whose value lives in the label.
        let cols = [col("description", SortKind::Text)];
        let mut items = vec![summary("Banana", &[]), summary("apple", &[])];
        apply_sort(&mut items, &[key("description", SortDirection::Asc)], &cols);
        // Case-insensitive: "apple" before "Banana".
        assert_eq!(ids(&items), ["apple", "Banana"]);
    }

    #[test]
    fn multi_key_sort_breaks_ties_with_later_keys_and_is_stable() {
        let cols = [col("status", SortKind::Text), col("priority", SortKind::Number)];
        let mut items = vec![
            summary("a", &[("status", "open"), ("priority", "2")]),
            summary("b", &[("status", "open"), ("priority", "1")]),
            summary("c", &[("status", "done"), ("priority", "1")]),
        ];
        let applied = apply_sort(
            &mut items,
            &[
                key("status", SortDirection::Asc),
                key("priority", SortDirection::Asc),
            ],
            &cols,
        );
        // Primary key status asc (done < open), secondary priority asc.
        assert_eq!(ids(&items), ["c", "b", "a"]);
        assert_eq!(applied.len(), 2);
    }

    #[test]
    fn unknown_columns_are_dropped_from_applied() {
        let cols = [col("started", SortKind::DateTime)];
        let mut items = vec![summary("a", &[("started", "2026-06-01T10:00:00Z")])];
        let applied = apply_sort(
            &mut items,
            &[
                key("nonsense", SortDirection::Asc),
                key("started", SortDirection::Desc),
            ],
            &cols,
        );
        // Only the recognised key is reported as applied.
        assert_eq!(applied, vec![key("started", SortDirection::Desc)]);
    }
}

// ---------------------------------------------------------------------------
// ListParams / ListResult / NodeSummary
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct ListParams {
    /// Which child type to list.
    pub node_type: NodeType,
    /// Opaque query string in the backend's native language (JQL, SQL, etc.).
    pub query: Option<String>,
    /// Requested sort. Adapters honour what they can and report what was
    /// actually applied via [`ListResult::applied_sort`]. Empty = adapter
    /// chooses (typically a stable backend default).
    pub sort: Vec<SortKey>,
    /// Pagination request. `None` = adapter default page (often the first
    /// page of a sensible size). Adapters that cannot paginate ignore this.
    pub page: Option<PageRequest>,
    /// If true, download full content for each item (batch).
    /// If false, return NodeSummary only (lazy).
    pub download: bool,
    /// The pane's active grouping, for adapters that group **adapter-side**
    /// (capability [`AdapterCapabilities::group_by_via_adapter`]). Tree views
    /// can't group engine-side — the adapter owns the per-bucket fold — so
    /// the engine passes the active `group_by` along on the root `list()`
    /// and the adapter returns one bucket node per group as the root level.
    /// `None` = ungrouped (or an engine-side-grouping flat view); adapters
    /// without the capability never see `Some` and may ignore the field.
    pub group_by: Option<GroupSpec>,
}

/// One sort key in a (potentially multi-column) sort spec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SortKey {
    /// Column key as advertised by [`Node::sortable_columns`].
    pub column: String,
    pub direction: SortDirection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDirection {
    Asc,
    Desc,
}

/// Pagination request: a half-open window `[offset, offset + limit)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRequest {
    pub offset: u32,
    pub limit: u32,
}

/// Pagination state of a list result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageInfo {
    pub offset: u32,
    pub limit: u32,
    /// Total matching items, if the backend reports it.
    pub total: Option<u64>,
    pub has_next: bool,
    pub has_prev: bool,
}

/// How a column's values compare. The adapter declares this per
/// [`SortableColumn`] so the generic [`apply_sort`] helper knows whether to
/// compare cells lexically, numerically, or as timestamps — only the adapter
/// knows what a given column actually holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortKind {
    /// Lexicographic (case-insensitive) string compare. The fallback for any
    /// free-text column.
    #[default]
    Text,
    /// Parse cells as `f64` and compare numerically. Unparseable cells
    /// (empty, non-numeric) sort to the end of an ascending run.
    Number,
    /// Parse cells as RFC 3339 timestamps and compare chronologically.
    /// Unparseable cells (empty, sentinels like `"running"`) sort to the
    /// end of an ascending run.
    DateTime,
}

/// A column that the adapter can sort on. Returned by
/// [`Node::sortable_columns`] so the UI knows which table headers to mark
/// sort-eligible and what label to render in hint mode.
#[derive(Clone, Debug)]
pub struct SortableColumn {
    /// Stable key referenced from [`SortKey::column`] and matched against
    /// the view config's column `key`.
    pub key: String,
    /// Display label for sort hints / debug surfaces.
    pub label: String,
    /// How values in this column compare. See [`SortKind`].
    pub kind: SortKind,
}

/// Sort `items` in place by a multi-column `sort` spec, using `columns` to
/// resolve each requested key to a [`SortKind`].
///
/// This is the generic engine that powers the `S` (sort) action across every
/// adapter: an adapter advertises its sortable columns (with kinds) via
/// [`Node::sortable_columns`] and calls this from its `list()` before any
/// grouping, so the within-group order follows the requested item sort. The
/// frontend stays adapter-agnostic — it just forwards [`SortKey`]s and renders
/// whatever the adapter reports as applied.
///
/// A cell's value is the matching [`MetadataField`] by key, falling back to
/// the summary's `label` when the column carries no metadata field (e.g. a
/// `description` column rendered straight from the label). Keys not present in
/// `columns` are skipped. The sort is **stable** and applied
/// least-significant-key-first, so a multi-key spec orders by the first key
/// with later keys breaking ties. Returns the subset of `sort` keys that were
/// recognised (suitable for [`ListResult::applied_sort`]).
pub fn apply_sort(
    items: &mut [NodeSummary],
    sort: &[SortKey],
    columns: &[SortableColumn],
) -> Vec<SortKey> {
    let resolved: Vec<(&SortKey, SortKind)> = sort
        .iter()
        .filter_map(|k| {
            columns
                .iter()
                .find(|c| c.key == k.column)
                .map(|c| (k, c.kind))
        })
        .collect();

    let cell = |s: &NodeSummary, key: &str| -> String {
        s.metadata
            .fields
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.value.clone())
            .unwrap_or_else(|| s.label.clone())
    };

    // Apply keys least-significant first; a stable sort preserves the order
    // established by earlier (more significant) passes for equal elements.
    for (key, kind) in resolved.iter().rev() {
        items.sort_by(|a, b| {
            let va = cell(a, &key.column);
            let vb = cell(b, &key.column);
            let ord = compare_cells(&va, &vb, *kind);
            match key.direction {
                SortDirection::Asc => ord,
                SortDirection::Desc => ord.reverse(),
            }
        });
    }

    resolved.into_iter().map(|(k, _)| k.clone()).collect()
}

/// Compare two cell strings under a [`SortKind`]. Unparseable values under a
/// typed kind (Number/DateTime) sort *after* parseable ones in ascending
/// order, so sentinels like an empty `ended` or a literal `"running"` land at
/// the end rather than at an arbitrary position.
fn compare_cells(a: &str, b: &str, kind: SortKind) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match kind {
        SortKind::Text => a.to_lowercase().cmp(&b.to_lowercase()),
        SortKind::Number => {
            let pa = a.trim().parse::<f64>().ok();
            let pb = b.trim().parse::<f64>().ok();
            match (pa, pb) {
                (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            }
        }
        SortKind::DateTime => {
            let pa = chrono::DateTime::parse_from_rfc3339(a.trim()).ok();
            let pb = chrono::DateTime::parse_from_rfc3339(b.trim()).ok();
            match (pa, pb) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeSummary {
    pub id: String,
    pub label: String,
    pub node_type: NodeType,
    pub metadata: Metadata,
    /// Live expandability hint from the adapter. `None` = unknown
    /// (frontend falls back to the static "does this level have a
    /// tree-continuing child?" config check). `Some(true)` / `Some(false)`
    /// override that check, letting adapters whose backend reports
    /// per-row child counts (e.g. Confluence's `childTypes.page.value`)
    /// distinguish actually-leaf pages from expandable ones inside a
    /// `recursive: true` ChildDef where the static check would always
    /// say "expandable".
    pub has_children: Option<bool>,
}

pub struct ListResult {
    pub items: Vec<NodeSummary>,
    /// What sort the adapter actually applied. May differ from
    /// [`ListParams::sort`] (unsupported columns dropped, default added).
    pub applied_sort: Vec<SortKey>,
    /// Pagination state of this result, if the adapter paginates.
    pub page: Option<PageInfo>,
    /// Whether batch download is available for this node type.
    pub batch_download_available: bool,
    /// If download=true was requested, the full nodes.
    pub downloaded: Vec<Box<dyn Node>>,
}

// ---------------------------------------------------------------------------
// Subtree / SubtreeNode (eager multi-level expansion)
// ---------------------------------------------------------------------------

/// One **level** of an eagerly-expanded tree: the nodes at this level plus
/// that level's pagination state. Mirrors [`ListResult`] (`items` + `page`)
/// but recursively — each [`SubtreeNode`] carries its own children as a
/// nested `Subtree`. Returned by [`Node::list_subtree`].
///
/// The split (a level owns its `page`, a node owns its `children`) lets the
/// frontend ingest the whole structure into its per-parent tree cache in one
/// pass: one cache slot per node's `children` level, each with its own page.
#[derive(Clone, Debug, Default)]
pub struct Subtree {
    pub items: Vec<SubtreeNode>,
    /// Pagination state of THIS level, if the adapter paginates. Local
    /// adapters (the only ones that opt into eager subtrees) load
    /// all-or-nothing, so this is typically `None` for them.
    pub page: Option<PageInfo>,
}

/// A single node in an eagerly-expanded tree.
///
/// `children.items` is empty when the node is a genuine leaf **or** the
/// requested depth limit was reached (i.e. "not expanded here"). The two
/// cases are distinguished by `summary.has_children`: `Some(false)` = real
/// leaf, otherwise depth-limited and the frontend may still lazy-expand it
/// on demand via the ordinary cascade.
#[derive(Clone, Debug)]
pub struct SubtreeNode {
    /// Carries `id`, `label`, `node_type`, `metadata`, `has_children`.
    pub summary: NodeSummary,
    /// This node's already-expanded children (one tree level deeper).
    pub children: Subtree,
}

// ---------------------------------------------------------------------------
// QueryVariable (saved-query parameter binding)
// ---------------------------------------------------------------------------

/// A single variable extracted from a raw saved-query string by the adapter.
///
/// Adapters define their own inline syntax (e.g. `${name:default}` for
/// Taiga); the frontend stays syntax-agnostic and only knows that the
/// adapter reports a list of variables to gather before the query can be
/// rendered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryVariable {
    /// Variable name (as referenced in the raw query).
    pub name: String,
    /// Default value parsed out of the raw query. `None` means the
    /// adapter could not extract a default — the frontend must require
    /// input from the user.
    pub default: Option<String>,
}

// ---------------------------------------------------------------------------
// AdapterCapabilities
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct AdapterCapabilities {
    pub supports_create: bool,
    pub supports_delete: bool,
    pub supports_search: bool,
    pub supports_batch_download: bool,
    /// Whether list() can return total counts.
    pub supports_total_count: bool,
    /// Whether the adapter computes subtree-cumulated values for tree nodes
    /// (M4 tree-fold aggregation). When `true`, a `tree_aggregate` column may
    /// toggle between a node's own value and the adapter's cumulated field;
    /// the TUI never folds the (lazy-loaded) tree itself. Adapters that cannot
    /// cumulate leave this `false` and the `toggle_tree_aggregate` action and
    /// hint stay hidden.
    pub supports_tree_aggregation: bool,
    /// Whether the active query applies to child node lists at every
    /// depth, not just the root `list()`. When `true`, the engine
    /// threads the pane's active (rendered) query into tree-expansion
    /// and drill-down `list()` calls so a filtered tree stays filtered
    /// below the root. Adapters whose child node types carry *different*
    /// query semantics than their parent (e.g. Jira epic → story, where
    /// the parent's JQL must not leak onto the children) leave this
    /// `false` — child loads then receive `query: None` as before. Only
    /// homogeneous trees (the task forest: `task:item` → `task:item`,
    /// one `FilterExpr` valid at every depth) opt in.
    pub propagates_query_to_subtree: bool,
    /// Whether the adapter can group its **tree** root level itself when the
    /// engine passes the active grouping in [`ListParams::group_by`]: it then
    /// returns one bucket node per group (each holding that bucket's folded
    /// subtree) instead of the plain root rows. Engine-side grouping (M3)
    /// only partitions flat lists; a tree's per-bucket aggregates must be
    /// folded by whoever owns the data, so tree views group only through
    /// this capability. With it set, the `cycle_grouping` / group-menu keys
    /// become claimable in tree mode and trigger a *reload* (the adapter
    /// must re-list) instead of an in-memory rebuild.
    pub group_by_via_adapter: bool,
    /// Whether the adapter can build a whole multi-level subtree in one
    /// [`Node::list_subtree`] call without per-level round-trips. Local
    /// adapters that hold the full forest in memory (Tasks, Trackings) set
    /// this `true`; the engine then expands a tree's initial / reloaded
    /// state with a single eager call instead of the per-node `list()`
    /// cascade (which is O(N²) in tree-rebuilds for `expand_depth: all`).
    ///
    /// Remote adapters leave this `false` on purpose: a blocking
    /// `list_subtree` over a slow backend would freeze the UI, whereas the
    /// progressive cascade keeps it responsive and shows levels as they
    /// arrive. They still get a correct (but synchronous, round-trip-per-
    /// level) default `list_subtree` impl — it is simply never driven by the
    /// engine for them. Interactive single-node expansion always uses the
    /// cascade regardless of this flag.
    pub supports_eager_subtree: bool,
}

// ---------------------------------------------------------------------------
// Custom queries (free-form, adapter-native query strings)
// ---------------------------------------------------------------------------

/// Addressing data threaded into [`ContentAdapter::execute_custom_query`].
/// The map is opaque to the caller — each adapter declares the keys it
/// needs (postgres reads `database`; future adapters may read other keys).
/// The optional `page` is honoured by adapters that can paginate
/// resultset queries (postgres wraps a SELECT with `LIMIT/OFFSET`).
///
/// `cursor` is the cursor-pagination opt-in. When `Some(_)` the adapter
/// runs the cursor path instead of the regular execute; the variants
/// distinguish the three lifecycle steps (open / fetch next / close).
/// Adapters that don't implement cursor pagination return
/// `NotSupported` for `Some(Open)`.
#[derive(Clone, Debug, Default)]
pub struct CustomQueryContext {
    pub fields: std::collections::HashMap<String, String>,
    pub page: Option<PageRequest>,
    pub cursor: Option<CursorIntent>,
}

/// One step in the cursor-pagination lifecycle. The TUI threads these
/// through [`ContentAdapter::execute_custom_query`] when a view uses
/// `PaginationMode::Cursor`.
#[derive(Clone, Debug)]
pub enum CursorIntent {
    /// Open a fresh server-side cursor for the query and return the
    /// first page. The resulting [`CustomQueryResult::cursor_id`]
    /// carries the opaque handle the caller must pass back on the
    /// next steps.
    Open,
    /// Fetch the next page from an already-open cursor.
    Continue { cursor_id: String },
    /// Tear down a cursor. Returns an empty result (no rows, no
    /// `cursor_id`).
    Close { cursor_id: String },
}

impl CustomQueryContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }

    pub fn with_page(mut self, page: PageRequest) -> Self {
        self.page = Some(page);
        self
    }

    pub fn with_cursor(mut self, cursor: CursorIntent) -> Self {
        self.cursor = Some(cursor);
        self
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }
}

/// Result of a free-form custom query.
///
/// For result-set queries (SELECT…) `items` carries the rows and
/// `status` is `None`. For non-resultset statements (UPDATE/DELETE/INSERT
/// without RETURNING, or DDL) `items` is empty and `status` carries a
/// human-readable summary like `"5 row(s) affected"`.
///
/// When several `;`-separated statements are executed in one call, only
/// the **last** statement's output is reflected here.
pub struct CustomQueryResult {
    /// Column names of the last result set, in order. Empty when the
    /// last statement had no result set.
    pub columns: Vec<String>,
    /// Rows of the last result set, mapped to `NodeSummary`s with one
    /// `MetadataField` per column. IDs are `qrow:<row_index>`.
    pub items: Vec<NodeSummary>,
    /// Status text for non-resultset statements. `None` when the last
    /// statement returned a result set.
    pub status: Option<String>,
    /// Pagination state when the adapter wrapped the last statement
    /// with `LIMIT/OFFSET`. `None` when no pagination was applied
    /// (non-SELECT, multi-statement, or `context.page` was `None`).
    pub page: Option<PageInfo>,
    /// Opaque cursor handle when the adapter opened or continued a
    /// server-side cursor for this query. The caller stores this and
    /// threads it back on the next page via
    /// [`CursorIntent::Continue`]. `None` when no cursor is in play
    /// (LIMIT/OFFSET path, full-load, or cursor was just closed).
    pub cursor_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Tree search
// ---------------------------------------------------------------------------

/// Result set for an adapter-side tree search.
///
/// Returned by [`ContentAdapter::search_in_tree`] — used by tree-mode
/// views to perform a server-side search (e.g. CQL for Confluence,
/// JQL `text ~` for Jira) and locate each hit inside the lazy tree
/// without forcing the user to open every parent by hand.
///
/// Hits are pre-sorted in tree-render order by the adapter (e.g. for
/// Confluence: configured space order, then ancestor DFS, then title),
/// so the caller can step `current` forward / backward without
/// re-sorting.
#[derive(Debug, Clone)]
pub struct TreeSearchResults {
    pub hits: Vec<TreeFindHit>,
    /// The server reported more matches than `hits.len()` (we hit the
    /// per-call limit). UI surfaces this so the user knows to refine
    /// the query for completeness.
    pub truncated: bool,
}

/// One hit from [`ContentAdapter::search_in_tree`].
///
/// `path` is the chain of node ids from the tree root down to and
/// including the hit itself, ready to be fed into a lazy-expand
/// driver. For Confluence: `["SPACE", "ancestor_id_1", …, "page_id"]`.
#[derive(Debug, Clone)]
pub struct TreeFindHit {
    pub path: Vec<String>,
    /// Display title for status-bar feedback.
    pub label: String,
    /// Root-level grouping key (e.g. Confluence space key). Empty when
    /// the adapter has no such notion. Cosmetic only — `path[0]` is
    /// the canonical addressing.
    pub space_key: String,
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// An action a node exposes to the user (edit, delete, transition, …).
///
/// Adapters declare their actions; the TUI dispatches them along two paths:
///
/// 1. **Menu/picker path (existing)** — the user opens the action menu and
///    selects one. The TUI honours the [`InputSpec`] (editor / picker /
///    file-picker / none) and the result routes back through
///    [`Node::execute`] returning [`ActionOutcome`].
/// 2. **Shortcut/dispatch path (new)** — the YAML view config binds a key
///    directly to an action `id`. The TUI calls [`Node::invoke_action`]
///    which returns an [`ActionDispatch`] describing what UI flow to start
///    (open editor, execute query, create child, …).
///
/// `placement` and `default_key` only influence the new shortcut path —
/// adapters that only support the menu path can leave them at their
/// defaults via [`NodeAction::new`].
#[derive(Clone, Debug)]
pub struct NodeAction {
    /// Stable identifier referenced from view config (e.g. `"edit_full"`,
    /// `"edit_with_comments"`, `"transition"`, `"delete"`).
    pub id: String,
    /// Default human-readable label. The view config may override.
    pub label: String,
    /// What kind of input the action needs from the user.
    pub input: InputSpec,
    /// Where the action's hint renders when shortcut-bound. Adapters that
    /// only surface via the menu can leave the default (`StatusBar`).
    pub placement: HintPlacement,
    /// Suggested key when the YAML view config doesn't bind one. Purely
    /// cosmetic — explicit YAML mappings always win.
    pub default_key: Option<char>,
}

impl NodeAction {
    /// Build a menu-only action with default placement (`StatusBar`) and
    /// no key suggestion. Use the builder methods below for shortcut
    /// metadata.
    pub fn new(id: impl Into<String>, label: impl Into<String>, input: InputSpec) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            input,
            placement: HintPlacement::StatusBar,
            default_key: None,
        }
    }

    pub fn with_placement(mut self, placement: HintPlacement) -> Self {
        self.placement = placement;
        self
    }

    pub fn with_default_key(mut self, key: char) -> Self {
        self.default_key = Some(key);
        self
    }
}

/// Shape of input an action consumes.
#[derive(Clone, Debug)]
pub enum InputSpec {
    /// No user input — fire and forget (e.g. delete, refresh-style).
    None,
    /// Multi-line editor buffer. Adapter renders the template in
    /// [`Node::prepare`] and parses the result inside [`Node::execute`].
    Editor,
    /// Picker over an option list. Options come from
    /// [`Node::picker_options`] when triggered.
    Picker,
    /// File picker — the TUI opens its file-picker widget and returns the
    /// chosen path(s). `multi: false` constrains the user to one file.
    FilePicker { multi: bool },
    /// Structured multi-field form. The TUI renders the [`FormFieldSpec`]
    /// list generically (text / select / toggle widgets), collects the
    /// values, and delivers them via [`ActionInput::Form`]. Initial values
    /// for an edit flow come from [`Node::form_prep`]; static fallbacks
    /// come from each field's [`FormFieldSpec::default`].
    Form { fields: Vec<FormFieldSpec> },
}

/// One field in an [`InputSpec::Form`].
#[derive(Clone, Debug)]
pub struct FormFieldSpec {
    /// Stable key the value is returned under in [`ActionInput::Form`] and
    /// the key [`Node::form_prep`] uses to prefill an initial value.
    pub key: String,
    /// Human-readable label shown next to the widget.
    pub label: String,
    /// Which widget renders this field.
    pub kind: FormFieldKind,
    /// When true the TUI rejects submission while the value is empty
    /// (text/select). Toggles are always satisfied.
    pub required: bool,
    /// Static initial value used when [`Node::form_prep`] supplies none.
    /// For a toggle this is `"true"`/`"false"`; for a select it must be
    /// one of `allowed_values`.
    pub default: Option<String>,
}

impl FormFieldSpec {
    /// A required single-line text field.
    pub fn text(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: FormFieldKind::Text,
            required: true,
            default: None,
        }
    }

    /// A select field over a fixed option list.
    pub fn select(
        key: impl Into<String>,
        label: impl Into<String>,
        allowed_values: Vec<String>,
    ) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: FormFieldKind::Select { allowed_values },
            required: true,
            default: None,
        }
    }

    /// A boolean toggle.
    pub fn toggle(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: FormFieldKind::Toggle,
            required: false,
            default: None,
        }
    }

    /// Mark the field optional (empty values are accepted).
    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    /// Set the static initial value.
    pub fn with_default(mut self, default: impl Into<String>) -> Self {
        self.default = Some(default.into());
        self
    }
}

/// Widget kind for a [`FormFieldSpec`].
#[derive(Clone, Debug)]
pub enum FormFieldKind {
    /// Single-line free text.
    Text,
    /// One choice out of a fixed list.
    Select { allowed_values: Vec<String> },
    /// Boolean on/off.
    Toggle,
}

/// The user's input for an action invocation.
pub enum ActionInput {
    /// `InputSpec::None` actions.
    None,
    /// `InputSpec::Editor` actions: the saved buffer plus the original
    /// template (for 3-way merge) and the version token returned by
    /// [`Node::prepare`].
    Edited {
        text: String,
        original: String,
        version: String,
    },
    /// `InputSpec::Picker` actions: the value of the selected option.
    Picked(String),
    /// `InputSpec::FilePicker` actions: the absolute paths the user chose.
    /// Always non-empty when delivered by the TUI.
    Files(Vec<std::path::PathBuf>),
    /// `InputSpec::Form` actions: the collected field values keyed by
    /// [`FormFieldSpec::key`]. Toggles deliver `"true"`/`"false"`; text and
    /// select fields deliver their string value (possibly empty for an
    /// optional field).
    Form(std::collections::HashMap<String, String>),
}

/// What the adapter wants the TUI to do after executing an action.
pub enum ActionOutcome {
    /// Persisted. Optional notification text.
    Done { message: Option<String> },
    /// Validation, parse error, or conflict. The adapter has rendered a
    /// fresh buffer in its own syntax (banners, conflict markers); the
    /// TUI should reopen the editor with this content. `new_version`
    /// reflects an upstream re-fetch when applicable.
    Reopen {
        content: String,
        new_version: Option<String>,
    },
    /// Nothing changed — no roundtrip needed.
    NoChanges,
    /// The action created or surfaced a new node — caller may navigate.
    Navigate {
        node_id: String,
        node_type: NodeType,
    },
}

/// Initial state for an `InputSpec::Editor` action.
pub struct EditorPrep {
    /// Initial buffer content written to the temp file.
    pub template: String,
    /// Backend version token — passed back via [`ActionInput::Edited`]
    /// for conflict detection.
    pub version: String,
    /// File suffix for `$EDITOR` syntax highlighting (e.g. `".jira"`).
    pub suffix: String,
}

/// A selectable option for an `InputSpec::Picker` action.
#[derive(Clone, Debug)]
pub struct ActionOption {
    /// Display label (e.g. "In Progress").
    pub label: String,
    /// Value passed back via [`ActionInput::Picked`] (e.g. transition ID).
    pub value: String,
}

/// Where a hint should render — drives the split between the highlighted
/// action bar and the dimmer status bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HintPlacement {
    ActionBar,
    StatusBar,
}

// ---------------------------------------------------------------------------
// Per-node action dispatch (shortcut path)
// ---------------------------------------------------------------------------

/// What [`Node::invoke_action`] tells the TUI to do.
///
/// Distinct from [`ActionOutcome`] (which describes the *result* of a
/// completed [`Node::execute`] roundtrip). `ActionDispatch` describes the
/// *intent* — what UI flow the TUI should start in response to a
/// shortcut press.
///
/// Note: the design doc (`docs/plan-cursor-pagination-and-node-actions.md`)
/// originally called this `ActionOutcome`; the name was already in use,
/// so we use `ActionDispatch` here. Semantics are identical.
#[derive(Clone, Debug)]
pub enum ActionDispatch {
    /// Open an editor session. `session_kind` is a *generic* role
    /// discriminator the TUI uses to pick the right `EditSession` impl —
    /// it MUST NOT encode an adapter name. The TUI understands a small
    /// fixed vocabulary; adapters reuse those role names rather than
    /// inventing adapter-prefixed ones:
    ///
    /// - `"query_editor"` — an editor that edits and (re)runs a query
    ///   against a backend (Postgres uses it for the per-table SQL editor).
    /// - `"script_editor"` — an editor for a named, persisted script that
    ///   the user re-executes separately (Postgres uses it for DB-level
    ///   scripts).
    ///
    /// `params` carries the session's setup data as opaque string
    /// key/value pairs (e.g. `{"database": "live", "script": "report"}`).
    OpenEditor {
        session_kind: String,
        params: std::collections::HashMap<String, String>,
    },
    /// Execute a query and (optionally) open a paginated result pane.
    /// Used by per-node `execute` shortcuts (e.g. `x` on a DB script).
    ExecuteQuery {
        database: String,
        sql: String,
        /// When true the TUI opens a paginated result pane. When false
        /// the query is fire-and-forget (e.g. DDL); status surfaces in
        /// the status bar.
        paged: bool,
    },
    /// Create a new child node under the invoking node. `hint` is an
    /// adapter-defined string that the TUI uses to prompt the user
    /// (e.g. `"script_name"` → cmdline prompt).
    CreateChild { hint: String },
    /// Delete the invoking node. TUI confirms before the actual delete.
    ///
    /// `confirm` lets the adapter override the confirmation prompt — e.g.
    /// a node that deletes its whole subtree should say so ("Delete 'X'
    /// and its 3 subtasks (recursive)? (y/n)"). `None` falls back to the
    /// TUI's generic `Delete '<label>'? (y/n)`. The adapter is the only
    /// authority on whether its delete cascades, so it owns the wording.
    DeleteSelf { confirm: Option<String> },
    /// Reload the current pane.
    Reload,
    /// Ask the user to confirm before the action does its (often
    /// irreversible) work. The adapter returns this on the *first*
    /// invocation — when [`ActionContext::confirmed`] is still `false` —
    /// with a user-facing `prompt`. The TUI shows a `(y/n)` confirmation
    /// and, on "y", re-invokes the *same* action on the *same* node with
    /// `confirmed: true`, at which point the adapter performs the work.
    ///
    /// Unlike [`DeleteSelf`] (whose confirm/execute split lives in the
    /// frontend's delete plumbing), `Confirm` is generic: any action can
    /// gate itself behind a confirmation, and the adapter authors the
    /// wording because only it knows what the action will actually do
    /// (e.g. how many successor intervals a restore will purge).
    Confirm { prompt: String },
    /// No-op — useful as a default for adapters that haven't migrated.
    Noop,
    /// Adapter rejected the action with a user-displayable error.
    Error(String),
}

/// A node the user marked for a subsequent move — the "clipboard" of the
/// generic mark-move / paste-move vocabulary (M7).
///
/// Marking is session state owned by the frontend; it travels into the
/// adapter only when a `paste-move` action fires, via
/// [`ActionContext::marked`]. The adapter uses it to perform the
/// structural move (reparent / relocate the marked node under or next to
/// the invoking node) and is free to reject an incompatible
/// [`MarkedNode::node_type`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkedNode {
    /// Adapter-local id of the marked node — the same id accepted by
    /// [`ContentAdapter::get_by_id`].
    pub node_id: String,
    /// Type of the marked node, so a `paste-move` target can validate
    /// the move (e.g. only accept a task under another task).
    pub node_type: NodeType,
    /// Human-readable label, for the frontend's "marked …" indicator.
    pub label: String,
}

/// Context passed into [`Node::invoke_action`].
///
/// Extended per phase; today it carries the [`MarkedNode`] for the
/// generic mark/paste-move flow. The frontend populates `marked` only on
/// a `paste-move` invocation (and only when something is marked); every
/// other action sees `None`.
#[derive(Clone, Debug, Default)]
pub struct ActionContext {
    /// The node the user previously marked for a move, if any. Populated
    /// by the frontend on a `paste-move` invocation so the adapter can
    /// relocate the marked node relative to the invoking (target) node.
    pub marked: Option<MarkedNode>,
    /// Whether the user has already confirmed this action. `false` on the
    /// first invocation; the adapter may return
    /// [`ActionDispatch::Confirm`] to request a `(y/n)` prompt. The TUI
    /// re-invokes with `confirmed: true` on "y", and the adapter then does
    /// the work instead of asking again.
    pub confirmed: bool,
    /// The pane's currently-active query text — the *same* filter string the
    /// frontend already hands [`ContentAdapter::list`] via
    /// [`LoadParams::query`] on every load. Carried here so a **set-scoped**
    /// action (one that operates on more than the invoking node — e.g. a
    /// container/list-wide `restore-all`, a bulk delete, an aggregate) can
    /// re-resolve the *visible* set and act only on it, never on the whole
    /// universe.
    ///
    /// This is the active query's *identity*, not a round-trip of rendered
    /// content: the adapter produced the listing from this same query, so it
    /// re-derives the set itself (just like `list`) rather than receiving a
    /// list of ids back. `None`/empty = no active filter → the whole list is
    /// in scope (which equals what the pane shows).
    ///
    /// Single-node actions (delete one row, restore one row, toggle) ignore
    /// it — their target is already the invoking node.
    pub query: Option<String>,
}

// ---------------------------------------------------------------------------
// AdapterStatus (live connection state, observable by frontends)
// ---------------------------------------------------------------------------

/// One field requested from the user during interactive credential entry.
/// Frontends render a form built from a `Vec<AuthField>` and pass the
/// collected values back via [`ContentAdapter::submit_credentials`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthField {
    /// Stable key the adapter uses to look the value up (e.g. "username").
    pub name: String,
    /// Display label for the form (e.g. "Username").
    pub label: String,
    /// If true, the frontend must mask the input (passwords).
    pub masked: bool,
    /// Optional pre-filled value (e.g. username from YAML config).
    pub prefill: Option<String>,
}

/// Live connection state of an adapter, observable through
/// [`ContentAdapter::subscribe_status`]. Adapters that need to surface
/// async login progress (cookie scripts, OAuth flows) publish updates
/// here; frontends render the current state in the relevant view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdapterStatus {
    /// Auth has not started yet.
    Idle,
    /// Auth is in progress. `retry` is 1-based; `max_retries` and
    /// `timeout_secs` come from the adapter's config so the view can
    /// render "Connecting… (retry/max_retries) Timeout: timeout_secs".
    Connecting {
        retry: u32,
        max_retries: u32,
        timeout_secs: u64,
    },
    /// Adapter needs interactive credentials. The frontend renders a form
    /// for `fields` and submits the collected values via
    /// [`ContentAdapter::submit_credentials`].
    NeedsCreds { fields: Vec<AuthField> },
    /// Auth completed; the adapter is ready to serve requests.
    Ready,
    /// Auth gave up after exhausting retries (or hit a non-retryable
    /// error). `reason` is suitable for direct display.
    Failed { reason: String },
    /// A specific request is in flight against a (presumably) connected
    /// backend. Surfaces a countdown so users can see the adapter is
    /// still working before its timeout kicks in. Implies the connection
    /// is otherwise live — `Busy` only ever flips out of (and back to)
    /// `Ready` in practice, never out of `Connecting`/`NeedsCreds`.
    ///
    /// `started_at_unix_ms` is wall-clock at request start (so the
    /// frontend can compute `elapsed = now − started_at` on every
    /// render-tick without a separate counter). `timeout_secs` is the
    /// adapter-configured deadline that fires the reconnect.
    Busy {
        label: String,
        started_at_unix_ms: u64,
        timeout_secs: u64,
    },
}

/// Out-of-band signal from an adapter that its content changed and the
/// affected views should reload / redraw — the push counterpart to the
/// pull-based `list()`/`get_by_id()` model. Emitted by *streaming*
/// adapters (the Stoat gateway pushes one per relevant WebSocket event);
/// pull-only adapters never send (see [`ContentAdapter::subscribe_invalidations`]).
///
/// The payload is deliberately adapter-internal (a raw node id, not an
/// app-wide [`NodeRef`]): the frontend already routes each subscription
/// to a known view, so all it needs is *which level within that view*
/// changed. Keeping it internal avoids coupling the adapter to the
/// frontend's path encoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Invalidation {
    /// The child list of the node with this adapter-internal id is stale
    /// (e.g. a new message arrived in the channel with this id). The
    /// frontend reloads a view's current level **only** if that level's
    /// parent is this node — so a message in a channel nobody is looking
    /// at costs nothing.
    Node { id: String },
    /// The whole adapter tree is stale (reconnect, fresh bootstrap
    /// snapshot, or a structural change). The frontend reloads the
    /// current level of every view bound to this adapter.
    All,
    /// Redraw only — **no** refetch. The adapter's data is unchanged but a
    /// time-derived rendering (e.g. a live "elapsed since" cell) needs to
    /// re-render. The dirty-gated render loop is otherwise parked, so this
    /// is the wake that makes such cells tick. Bridged from a periodic
    /// domain heartbeat (`DomainEvent::TrackingTick`); cheap by design, so
    /// adapters may send it at ~1 Hz without touching their backend.
    Repaint,
    /// A single row's **complete** new state (M9 — adapter-driven live
    /// rows). Carries the full [`NodeSummary`], so the frontend replaces
    /// the matching row (by `id`) **in place** — no refetch, selection and
    /// scroll preserved. The push counterpart to [`Repaint`]: where
    /// `Repaint` re-renders a *time-derived* cell against a fresh `now`,
    /// `Row` lets the adapter itself compute the new value (e.g. a running
    /// tracking's elapsed duration, where the *same* column must read live
    /// for active rows and static for completed ones — something a single
    /// render-time `kind: elapsed` column cannot express). Generic over any
    /// adapter: a chat adapter can push an edited message, a CI adapter a
    /// build-progress row. A row whose `id` matches no visible item is a
    /// no-op.
    Row(NodeSummary),
    /// Re-pace the frontend's per-view **live-refresh timer** (M9). The
    /// adapter dictates the cadence at which the frontend should pull its
    /// [`live_rows`](ContentAdapter::live_rows) and patch them; `Some(d)`
    /// (re)starts the timer at interval `d`, `None` stops it. Sent whenever
    /// the adapter's set of live rows changes shape — e.g. the tracking
    /// adapter sends `Some(1s)` when a tracking starts and `None` when the
    /// last one stops, so the 1 Hz pull only runs while something actually
    /// ticks. The timer is owned by the frontend (one place to budget
    /// wake-ups against the dirty-gated render loop); the adapter only
    /// declares the interval.
    RefreshInterval(Option<std::time::Duration>),
    /// Data anchored to the **current instant** changed structurally, so a
    /// view grouped into buckets (one independently-aggregated subtree per
    /// group) must re-resolve and reload *only* the bucket that "now" falls
    /// into — not every bucket. Payload-free on purpose: the adapter knows
    /// *that* its now-anchored data moved, but not which bucket a given pane
    /// shows it under, because the active grouping ([`GroupSpec`]) is
    /// per-pane frontend state. So the frontend, on receiving this, asks the
    /// adapter [`bucket_for_now`](ContentAdapter::bucket_for_now) for each
    /// grouped pane's spec and reloads that one bucket's subtree in place.
    ///
    /// Emitted by the tracking adapter when a tracking starts/stops (the
    /// running interval's bucket totals shift) — the targeted counterpart to
    /// the coarse [`All`] a structural change (e.g. a deleted tracking) sends.
    /// Generic over any now-anchored grouping (e.g. a CI adapter grouping
    /// builds by day could re-fold today's bucket on a new build).
    NowAnchored,
}

// ---------------------------------------------------------------------------
// Core Traits
// ---------------------------------------------------------------------------

/// The entry point. One instance per configured connection.
#[async_trait]
pub trait ContentAdapter: Send + Sync {
    /// Stable type identifier of this adapter (e.g. "jira", "postgres",
    /// "taiga"). Used as the first path component in
    /// [`ContentAdapter::instance_data_dir`] and as a prefix in
    /// scope keys (e.g. saved-query scope).
    fn adapter_type(&self) -> &str;

    /// Stable per-instance identifier. Comes from the YAML
    /// `adapter.id:` field; defaults to [`adapter_type`](Self::adapter_type)
    /// when not set. Two adapter instances loaded into the same App run must
    /// have distinct `instance_id`s — the loader validates this.
    ///
    /// The default mirrors the "no `adapter.id:`" case: a single-instance
    /// adapter that stores no separate id reports its type. Multi-instance
    /// adapters override to return the configured id.
    fn instance_id(&self) -> &str {
        self.adapter_type()
    }

    /// Per-instance writable data directory. Default:
    /// `<XDG_DATA_HOME>/not_yet_done/<adapter_type>/<instance_id>/`.
    /// Caller is responsible for creating it on first write. Adapters
    /// may override if they need a different layout.
    fn instance_data_dir(&self) -> std::path::PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("not_yet_done")
            .join(self.adapter_type())
            .join(self.instance_id())
    }

    /// Navigate to the root node of the content tree.
    async fn root(&self) -> Result<Box<dyn Node>>;

    /// Direct access to a node by its ID (shortcut, avoids tree traversal).
    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>>;

    /// Synchronous, instance-free lookup of the action set for a given
    /// node type. Used by the TUI to render shortcut hints without
    /// instantiating a node — the alternative (`get_by_id(id).actions()`)
    /// triggers a full chain walk per cursor move, which on Postgres
    /// means a `list_databases` call just to read a constant list.
    ///
    /// Adapters return the same set their `Node::actions()` impl would
    /// return for a fresh instance of that type. Default is empty —
    /// adapters with no shortcuts don't need to override.
    fn actions_for_type(&self, _node_type: &NodeType) -> Vec<NodeAction> {
        Vec::new()
    }

    /// Environment variables to propagate to child processes (editors,
    /// scripts) spawned in this adapter's context. Default is empty —
    /// adapters without connection state (or that don't expose their
    /// credentials to child tools) don't need to override.
    ///
    /// Use case: the Postgres adapter exposes its live tunnel and
    /// credentials as libpq-style `PG*` vars so the editor's LSP
    /// (`postgres-language-server`) can talk to the same database the
    /// TUI is connected to without the user duplicating credentials in
    /// a sidecar config file.
    ///
    /// `node` identifies which item in the adapter's tree the spawn is
    /// for, so the env can be node-specific (e.g. `PGDATABASE` derived
    /// from the selected database row). The map is a snapshot at spawn
    /// time — adapters should not assume it will be re-queried if
    /// connection state changes later.
    ///
    /// Sync because the password/port are already resolved in adapter
    /// RAM; no I/O should be needed here. If the underlying connection
    /// is not yet open, return an empty map rather than blocking.
    fn child_process_env(
        &self,
        _node: &NodeRef,
    ) -> std::collections::HashMap<String, String> {
        std::collections::HashMap::new()
    }

    /// Augment an editor buffer with adapter-specific, **editor-only**
    /// completion hints before the external editor opens. `node` is the
    /// canonical [`NodeRef`] of the item being edited; the adapter scopes
    /// the hint from it. The returned string REPLACES the buffer: the
    /// adapter must first strip any stale hint it previously added (so
    /// repeated opens stay idempotent) and then append a fresh one.
    ///
    /// The hint is a convenience only and MUST be removed again on commit
    /// via [`strip_editor_hints`](Self::strip_editor_hints) so the
    /// persisted file never contains it. Default returns the buffer
    /// unchanged — adapters with no editor hints don't override.
    ///
    /// Use case: the Postgres DB-script editor appends a trailing SQL
    /// comment listing the database's tables as copy-paste tokens, which
    /// previously required a concrete-type downcast from the TUI. Routing
    /// it through the trait keeps the TUI free of any adapter-specific
    /// type knowledge.
    async fn augment_editor_buffer(&self, _node: &NodeRef, buffer: String) -> String {
        buffer
    }

    /// Inverse of [`augment_editor_buffer`](Self::augment_editor_buffer):
    /// strip any editor-only hint the adapter previously injected so the
    /// committed/persisted text is clean user content. Called on save.
    /// Default returns the text unchanged.
    fn strip_editor_hints(&self, text: &str) -> String {
        text.to_string()
    }

    /// Capabilities of this adapter (for UI feature gating). The default is
    /// all-`false` ([`AdapterCapabilities::default`]) — a minimal read-only
    /// adapter. Adapters opt into features (create/delete/search, eager
    /// subtree, adapter-side grouping, …) by overriding.
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::default()
    }

    /// Subscribe to live connection-status updates. Default returns a
    /// receiver that is permanently `Ready` — adapters that don't perform
    /// async auth (or that block synchronously inside `create`) don't
    /// need to override this.
    ///
    /// The sender backing the default channel lives for the duration of
    /// the process so consumers can `borrow()` the current value or
    /// `await` forever on `changed()` without spurious sender-dropped
    /// errors.
    fn subscribe_status(&self) -> tokio::sync::watch::Receiver<AdapterStatus> {
        static READY_TX: std::sync::OnceLock<tokio::sync::watch::Sender<AdapterStatus>> =
            std::sync::OnceLock::new();
        let tx = READY_TX
            .get_or_init(|| tokio::sync::watch::channel(AdapterStatus::Ready).0);
        tx.subscribe()
    }

    /// Subscribe to out-of-band [`Invalidation`] signals. A `broadcast`
    /// receiver (not `watch`) because invalidations are discrete *events*,
    /// not a latest-value state — coalescing them would drop intermediate
    /// changes — and because one adapter instance can back several views,
    /// each of which subscribes independently.
    ///
    /// Default returns a receiver whose sender lives for the duration of
    /// the process and never sends — pull-only adapters (Jira, Taiga,
    /// Postgres, …) don't need to override and their forwarder simply
    /// parks forever. Mirrors the [`subscribe_status`](Self::subscribe_status)
    /// default's static-`OnceLock` keepalive.
    fn subscribe_invalidations(&self) -> tokio::sync::broadcast::Receiver<Invalidation> {
        static SINK_TX: std::sync::OnceLock<tokio::sync::broadcast::Sender<Invalidation>> =
            std::sync::OnceLock::new();
        let tx = SINK_TX.get_or_init(|| tokio::sync::broadcast::channel(1).0);
        tx.subscribe()
    }

    /// Recompute the adapter's currently-live rows (M9). Called by the
    /// frontend's per-view refresh timer — whose cadence the adapter sets
    /// via [`Invalidation::RefreshInterval`] — once per tick; each returned
    /// [`NodeSummary`] is patched into the matching visible row in place
    /// (see [`Invalidation::Row`]). The adapter returns *only* the rows
    /// whose rendering actually changes between ticks (e.g. the entries
    /// for running trackings, with a freshly-computed elapsed duration),
    /// not its whole list — a row not returned keeps its current cells.
    ///
    /// Pure recompute from already-loaded state: no backend round-trip, so
    /// it stays cheap at 1 Hz. Default returns an empty list — pull-only
    /// adapters with no time-derived rows never need to override and their
    /// timer (if any) patches nothing.
    async fn live_rows(&self) -> Vec<NodeSummary> {
        Vec::new()
    }

    /// Resolve the **id of the bucket the current instant falls into** for a
    /// view grouped by `group_by`. The frontend calls this when it receives
    /// [`Invalidation::NowAnchored`]: each grouped pane hands in its own
    /// [`GroupSpec`] (per-pane frontend state the adapter can't know) and gets
    /// back the single bucket node id whose subtree it should reload in place,
    /// leaving every other bucket untouched.
    ///
    /// Returns the bucket the *most recent* item falls into — e.g. the
    /// tracking adapter returns the group node of the youngest tracking, which
    /// is the one a start/stop just shifted (a start mints the newest interval;
    /// a stop freezes it). Computed with the **same** bucketing the grouped
    /// list uses, so the returned id always matches a real bucket row when one
    /// exists. `None` when nothing is grouped this way (no items, or the
    /// adapter has no now-anchored data) — the frontend then leaves the pane
    /// as-is. Default returns `None`: adapters without now-anchored grouping
    /// never need to override.
    async fn bucket_for_now(&self, _group_by: &GroupSpec) -> Option<String> {
        None
    }

    /// The refreshed **rows of the now-bucket**, folded against the *live*
    /// current instant — the live-tick counterpart of
    /// [`bucket_for_now`](Self::bucket_for_now) for a grouped tree whose
    /// durations count up. While [`live_rows`](Self::live_rows) ticks the flat
    /// list (its rows are keyed independently of any grouping), a grouped
    /// tree's rows carry grouping-dependent ids the spec-less `live_rows`
    /// can't produce — so the frontend hands in the pane's [`GroupSpec`] and
    /// saved-query `query` and gets back the bucket header (its total re-summed
    /// to *now*) plus the tree rows on the running chain (their own/cumulated
    /// re-folded to *now*), each keyed exactly as the rendered tree row so the
    /// frontend's in-place row patch swaps the ticking cells without a reload.
    ///
    /// Returns only the rows that actually move (the running tasks and their
    /// ancestors, whose cumulated grows, plus the header) — untouched siblings
    /// keep their frozen value and aren't churned. Empty when nothing is
    /// running (no now-bucket) or the bucket is empty. Default returns empty:
    /// adapters without now-anchored grouping never tick a tree.
    async fn live_group_rows(
        &self,
        _group_by: &GroupSpec,
        _query: Option<&str>,
    ) -> Vec<NodeSummary> {
        Vec::new()
    }

    /// Cheap staleness probe, called by the frontend when the user switches
    /// to a tab backed by this adapter. An adapter whose backing store can
    /// change *outside* the process (e.g. the local task/tracking DB written
    /// by the CLI or a waybar module — no in-process domain event fires)
    /// should compare a low-cost fingerprint against its cache and, on
    /// drift, drop the cache and emit [`Invalidation::All`] through its
    /// invalidation stream so the visible panes reload. Must be cheap
    /// enough to run on every tab switch. Default: no-op — pull-only remote
    /// adapters revalidate via their normal reload/HTTP paths instead.
    async fn revalidate(&self) {}

    /// Submit credentials gathered from a [`AdapterStatus::NeedsCreds`]
    /// form. The adapter performs the login and updates its status
    /// channel — `Ok(())` means the login round-trip succeeded; the
    /// frontend should refresh views once the status flips to `Ready`.
    /// Default impl returns `NotSupported` for adapters that do not use
    /// interactive auth.
    async fn submit_credentials(
        &self,
        _fields: std::collections::HashMap<String, String>,
    ) -> Result<()> {
        Err(ContentError::NotSupported(
            "this adapter does not support interactive credential submission".into(),
        ))
    }

    /// Attempt a silent session refresh (e.g. via OAuth refresh token,
    /// JWT refresh endpoint). Returns `Ok(())` if the live session is
    /// now valid again. Default impl reports `NotSupported`, signalling
    /// the orchestrator to fall back to a full re-authentication.
    async fn try_refresh_session(&self) -> Result<()> {
        Err(ContentError::NotSupported(
            "this adapter does not support session refresh".into(),
        ))
    }

    /// Drop the cached session (derived token / login cookie) and force
    /// the next request to re-authenticate using the configured
    /// credential providers. Primary credentials (keyring entry, file
    /// on disk, …) stay untouched. Default impl is a no-op for adapters
    /// without a session cache.
    async fn invalidate_session(&self) -> Result<()> {
        Ok(())
    }

    /// Drop both the session cache and any cached primary credentials
    /// (e.g. a keyring entry written on first login). The next request
    /// re-runs the providers from scratch, prompting the user when
    /// applicable. Default impl is a no-op.
    async fn invalidate_credentials(&self) -> Result<()> {
        Ok(())
    }

    /// Load the persisted sort spec for a view scope. `scope` is an
    /// opaque key chosen by the frontend (e.g. `"jira:items"`).
    /// Default impl returns an empty list — adapters that don't
    /// persist sort state are simply stateless.
    async fn load_view_sort(&self, _scope: &str) -> Result<Vec<SortKey>> {
        Ok(Vec::new())
    }

    /// Persist the sort spec for a view scope. Empty `sort` should
    /// remove any stored entry. Default impl is a no-op.
    async fn save_view_sort(&self, _scope: &str, _sort: &[SortKey]) -> Result<()> {
        Ok(())
    }

    /// Extract the variables referenced in a saved-query string using the
    /// adapter's own inline syntax (e.g. `${name:default}` for Taiga).
    ///
    /// Returned in source order (or whatever order the adapter chooses to
    /// present them in the input popup). Adapters without variable
    /// support return an empty vec — the frontend then treats the query
    /// as having no variables and skips the popup.
    fn query_variables(&self, _query: &str) -> Vec<QueryVariable> {
        Vec::new()
    }

    /// Substitute variable bindings into a raw saved-query string and
    /// return the final string passed to `list(...)`. Variables not
    /// present in `vars` should fall back to whatever default the
    /// adapter parses from the raw query; if no default exists, the
    /// adapter is free to leave the placeholder, error, or drop the
    /// filter — frontends prevent this case by validating presence
    /// before calling.
    ///
    /// Adapters without variable support keep the identity default
    /// (return `query` verbatim).
    fn render_query(
        &self,
        query: &str,
        _vars: &std::collections::HashMap<String, String>,
    ) -> String {
        query.to_string()
    }

    /// Run a free-form, adapter-native query (e.g. raw SQL for
    /// Postgres). `context` carries adapter-specific addressing data
    /// (for Postgres: the `database` to connect to). Multi-statement
    /// queries are allowed; only the last statement's output is
    /// returned in [`CustomQueryResult`]. Whether this is exposed to
    /// the user at a given drill level is decided by the YAML
    /// `shortcuts:` map binding a key to a [`NodeAction`] whose
    /// [`Node::invoke_action`] returns an
    /// [`ActionDispatch::OpenEditor`] for `session_kind == "query_editor"`.
    ///
    /// Default impl returns `NotSupported`.
    async fn execute_custom_query(
        &self,
        _query: &str,
        _context: &CustomQueryContext,
    ) -> Result<CustomQueryResult> {
        Err(ContentError::NotSupported(
            "this adapter does not support custom queries".into(),
        ))
    }

    /// Adapter-managed persistence for named saved queries.
    ///
    /// `Some(store)` when the adapter owns a flat `(name → query body)`
    /// namespace it persists itself (Jira, Taiga, …). `None` when the
    /// adapter has a different query story (Postgres: per-table SQL
    /// scripts via its own API) or stores nothing at all. Frontends
    /// fall back to the inline `default:` query from view-YAML when
    /// this returns `None`.
    fn saved_query_store(&self) -> Option<&dyn SavedQueryStore> {
        None
    }

    /// Adapter-side full-tree search.
    ///
    /// `Some(results)` when the adapter can return hits with full
    /// ancestor paths (so the TUI can lazy-expand the tree to each
    /// hit). `None` signals "not supported" — the frontend then falls
    /// back to local in-memory filtering over the already-loaded rows.
    ///
    /// `query` is the raw user-typed string; the adapter is responsible
    /// for translating it into its native query language (CQL, JQL,
    /// SQL `ILIKE`, …) and for applying any per-instance scoping
    /// (e.g. space whitelist). `limit` caps the result set; the
    /// adapter signals truncation via [`TreeSearchResults::truncated`].
    ///
    /// The default impl returns `Ok(None)` — adapters opt in by
    /// overriding.
    async fn search_in_tree(
        &self,
        _query: &str,
        _limit: u32,
    ) -> Result<Option<TreeSearchResults>> {
        Ok(None)
    }
}

/// A single item in the content tree.
#[async_trait]
pub trait Node: Send + Sync {
    fn id(&self) -> &str;
    fn label(&self) -> &str;
    fn node_type(&self) -> &NodeType;
    fn metadata(&self) -> &Metadata;

    /// Populate the display fields (`label`, `metadata`) that a lazily
    /// constructed node leaves as placeholders.
    ///
    /// Several adapters build a node from just an id/key without a network
    /// round-trip — [`ContentAdapter::get_by_id`] does this so child-only
    /// operations (`list`, `invoke_action`) don't pay for a detail fetch they
    /// don't need, and so a user who can see an issue in search but lacks
    /// read permission on it still gets a usable node. Such a stub reports its
    /// **id** as `label()` and a sparse `metadata()` until its detail is first
    /// awaited.
    ///
    /// Consumers that read those display fields *directly* off a re-resolved
    /// node — the post-edit row patch (`patch_content_row`) is the only one —
    /// must call `hydrate` first. Without it the patched row shows the id
    /// instead of the title and drops every other column until a full reload.
    /// Centralising the call here (rather than eagerly hydrating inside each
    /// adapter's `get_by_id`) keeps `get_by_id` uniformly cheap and stops the
    /// fix from being re-derived per adapter.
    ///
    /// Default = no-op: adapters whose nodes are display-ready on construction
    /// (eager fetchers like Taiga, in-memory local Tasks/Trackings, the mock)
    /// need not override it. A failed hydration must degrade gracefully —
    /// leave the stub, never panic.
    async fn hydrate(&mut self) {}

    /// Which child node types can be listed under this node.
    fn children_types(&self) -> Vec<NodeType> {
        Vec::new()
    }

    /// Columns the adapter can sort lists of `node_type`'s children on.
    /// Empty (default) = no server-side sort; the UI may still sort
    /// in-memory if it sees fit.
    fn sortable_columns(&self, _node_type: &NodeType) -> Vec<SortableColumn> {
        Vec::new()
    }

    /// List child nodes of a given type.
    async fn list(&self, _params: ListParams) -> Result<ListResult> {
        Err(ContentError::NotSupported("list not supported".into()))
    }

    /// List child nodes **and** eagerly expand their descendants up to
    /// `depth` additional levels below the directly-listed children.
    ///
    /// Depth semantics (total visible levels = `depth + 1`):
    /// - `depth == 0` — exactly [`Node::list`]: one level, every returned
    ///   node has empty `children`. This is the default the engine requests
    ///   for ordinary lists and interactive single-node expansion.
    /// - `depth == 1` — the listed children plus one level beneath them.
    /// - `depth == u32::MAX` — fully expanded (`expand_depth: all`); the
    ///   recursion stops naturally at leaves (`has_children == Some(false)`
    ///   or an empty `list`).
    ///
    /// The default implementation walks [`Node::list`] / [`Node::get_child`]
    /// recursively, one round-trip per node per level. It is correct for any
    /// adapter but only *fast* for adapters that hold their data in memory;
    /// remote adapters keep [`AdapterCapabilities::supports_eager_subtree`]
    /// `false` so the engine never drives this path for them (see that flag).
    /// Adapters that can build the whole structure cheaply (Tasks, Trackings)
    /// override this with a single in-memory projection walk.
    ///
    /// `params.query` is threaded down into every level's child `list`
    /// (honouring [`AdapterCapabilities::propagates_query_to_subtree`]);
    /// child levels are requested unpaginated (`page: None`).
    async fn list_subtree(&self, params: ListParams, depth: u32) -> Result<Subtree> {
        let query = params.query.clone();
        let result = self.list(params).await?;
        let page = result.page;
        let mut items = Vec::with_capacity(result.items.len());
        for summary in result.items {
            // Only descend when we have budget AND the node isn't a known
            // leaf. A node that claims children but whose get_child fails is
            // treated as a leaf here (graceful) rather than failing the whole
            // subtree; a genuine list error below propagates.
            let children = if depth > 0 && summary.has_children != Some(false) {
                match self.get_child(&summary.id).await {
                    Ok(child) => {
                        let mut merged = Subtree::default();
                        for child_type in child.children_types() {
                            let child_params = ListParams {
                                node_type: child_type,
                                query: query.clone(),
                                sort: Vec::new(),
                                page: None,
                                download: false,
                                group_by: None,
                            };
                            let mut sub = child.list_subtree(child_params, depth - 1).await?;
                            merged.items.append(&mut sub.items);
                            // Single child-type is the common case; keep the
                            // first level's page. Multi-type local adapters
                            // load all-or-nothing (page stays None).
                            if merged.page.is_none() {
                                merged.page = sub.page;
                            }
                        }
                        merged
                    }
                    Err(_) => Subtree::default(),
                }
            } else {
                Subtree::default()
            };
            items.push(SubtreeNode { summary, children });
        }
        Ok(Subtree { items, page })
    }

    /// Navigate to a specific child by ID.
    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        Err(ContentError::NotFound(id.to_string()))
    }

    /// Read-only content body, if any.
    fn content(&self) -> Option<&dyn Content> {
        None
    }

    /// Actions this node currently supports. Empty for read-only/leaf
    /// nodes.
    ///
    /// **Contract:** the returned set MUST be deterministic per
    /// `node_type()` within an adapter session. The TUI caches the
    /// list by node_type so that cursor navigation over a uniform
    /// list of e.g. `postgres:table` rows doesn't trigger one
    /// `get_by_id` walk per row. Adapters whose action availability
    /// genuinely depends on per-instance state must flatten that
    /// into a static superset and reject inapplicable actions at
    /// invocation time instead (see
    /// `not_yet_done_taiga_adapter::adapter::notification` for an
    /// example of an idempotent action exposed unconditionally).
    fn actions(&self) -> Vec<NodeAction> {
        Vec::new()
    }

    /// Dispatch a shortcut-bound action by its `id`. Distinct from
    /// [`Node::execute`] (which completes a menu-driven action with a
    /// user input). `invoke_action` decides what UI flow the TUI should
    /// start — open an editor, execute a query, create a child, …
    ///
    /// Default impl returns [`ActionDispatch::Noop`] so adapters can
    /// adopt the new dispatch path incrementally. Adapters that don't
    /// implement this method simply have non-functional shortcuts;
    /// existing menu/picker dispatch through [`Node::execute`] is
    /// unaffected.
    async fn invoke_action(
        &self,
        _name: &str,
        _ctx: &ActionContext,
    ) -> Result<ActionDispatch> {
        Ok(ActionDispatch::Noop)
    }

    /// Render the initial buffer for an `InputSpec::Editor` action.
    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        let _ = action_id;
        Err(ContentError::NotSupported(
            "prepare not supported".into(),
        ))
    }

    /// Fetch the option list for an `InputSpec::Picker` action.
    async fn picker_options(&self, action_id: &str) -> Result<Vec<ActionOption>> {
        let _ = action_id;
        Ok(Vec::new())
    }

    /// Prefill values for an `InputSpec::Form` action, keyed by
    /// [`FormFieldSpec::key`]. Used by edit flows to seed the form with the
    /// node's current values; keys without an entry fall back to the
    /// field's [`FormFieldSpec::default`]. The default impl returns no
    /// prefills (suitable for a create flow).
    async fn form_prep(
        &self,
        action_id: &str,
    ) -> Result<std::collections::HashMap<String, String>> {
        let _ = action_id;
        Ok(std::collections::HashMap::new())
    }

    /// Execute an action with the user's input.
    async fn execute(
        &mut self,
        action_id: &str,
        input: ActionInput,
    ) -> Result<ActionOutcome> {
        let _ = (action_id, input);
        Err(ContentError::NotSupported(
            "execute not supported".into(),
        ))
    }
}

/// Read-only content access.
#[async_trait]
pub trait Content: Send + Sync {
    fn node_type(&self) -> &NodeType;

    /// Version identifier for conflict detection.
    fn version(&self) -> Option<&str>;

    /// Download the content body.
    async fn read(&self) -> Result<Vec<u8>>;

    /// Read as text (convenience, fails for binary).
    async fn read_text(&self) -> Result<String> {
        let bytes = self.read().await?;
        String::from_utf8(bytes).map_err(|e| ContentError::Other(Box::new(e)))
    }
}

// ---------------------------------------------------------------------------
// SavedQueryStore
// ---------------------------------------------------------------------------

/// Adapter-owned persistence layer for named saved queries.
///
/// Storage and file format are the adapter's concern — frontends only
/// see a flat namespace of `(name → raw query body)` pairs. The body is
/// adapter-specific text (a JQL string for Jira, a YAML block for
/// Taiga, …) parsed by the same code path that handles the inline
/// `default:` query in view-YAML. Adapters typically write under
/// [`ContentAdapter::instance_data_dir`]`/queries/<name>.yaml`, but
/// implementations are free to pick any layout (a single database
/// row, a remote KV store, …) as long as the trait contract holds.
#[async_trait]
pub trait SavedQueryStore: Send + Sync {
    /// All saved-query names known to this adapter instance, sorted
    /// for stable UI listing. Empty when nothing has been saved yet.
    async fn list(&self) -> Result<Vec<String>>;

    /// Raw query body for `name`. Returns
    /// [`ContentError::NotFound`] when the entry doesn't exist.
    async fn load(&self, name: &str) -> Result<String>;

    /// Persist `body` under `name`. Creates the entry if missing,
    /// overwrites if present. Implementations are responsible for
    /// creating any parent directories.
    async fn save(&self, name: &str, body: &str) -> Result<()>;

    /// Remove the entry. Missing entries are not an error (idempotent
    /// delete).
    async fn delete(&self, name: &str) -> Result<()>;

    /// Optional: the on-disk path of the entry, for adapters that
    /// expose one. Used by `:query edit` to launch a `FileEditSession`
    /// against the file. Adapters without a filesystem-backed store
    /// return `None`; the frontend falls back to a text-buffer edit
    /// via `load`/`save`.
    fn path(&self, _name: &str) -> Option<std::path::PathBuf> {
        None
    }
}

/// Filesystem-backed `SavedQueryStore` implementation. One file per
/// query under `<root>/<name>.yaml`. Names are passed through unchanged
/// (no escaping) — adapters that need exotic characters in query names
/// should layer their own validation on top. The root directory is
/// created lazily on first `save`.
pub struct FsSavedQueryStore {
    root: std::path::PathBuf,
}

impl FsSavedQueryStore {
    /// Use `<instance_data_dir>/queries/` as the storage root.
    pub fn new(root: std::path::PathBuf) -> Self {
        Self { root }
    }

    fn file_path(&self, name: &str) -> std::path::PathBuf {
        self.root.join(format!("{name}.yaml"))
    }
}

#[async_trait]
impl SavedQueryStore for FsSavedQueryStore {
    async fn list(&self) -> Result<Vec<String>> {
        let mut rd = match tokio::fs::read_dir(&self.root).await {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(ContentError::Other(Box::new(e))),
        };
        let mut names = Vec::new();
        while let Some(entry) = rd
            .next_entry()
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))?
        {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                names.push(stem.to_string());
            }
        }
        names.sort();
        Ok(names)
    }

    async fn load(&self, name: &str) -> Result<String> {
        let path = self.file_path(name);
        match tokio::fs::read_to_string(&path).await {
            Ok(s) => Ok(s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(ContentError::NotFound(name.to_string()))
            }
            Err(e) => Err(ContentError::Other(Box::new(e))),
        }
    }

    async fn save(&self, name: &str, body: &str) -> Result<()> {
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))?;
        tokio::fs::write(self.file_path(name), body)
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))?;
        Ok(())
    }

    async fn delete(&self, name: &str) -> Result<()> {
        match tokio::fs::remove_file(self.file_path(name)).await {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(ContentError::Other(Box::new(e))),
        }
    }

    fn path(&self, name: &str) -> Option<std::path::PathBuf> {
        Some(self.file_path(name))
    }
}

// ---------------------------------------------------------------------------
// AdapterFactory (registry pattern)
// ---------------------------------------------------------------------------

/// Factory for creating adapter instances from config strings.
pub trait AdapterFactory: Send + Sync {
    /// Adapter type name (e.g. "jira", "confluence").
    fn adapter_type(&self) -> &str;

    /// Create an adapter from an opaque config string (YAML/JSON).
    /// `instance_id` comes from the YAML `adapter.id:` field, with the
    /// adapter type as fallback default. The factory must thread it
    /// into the produced adapter so [`ContentAdapter::instance_id`]
    /// returns it.
    fn create(&self, instance_id: &str, config: &str) -> Result<Box<dyn ContentAdapter>>;
}

// ---------------------------------------------------------------------------
// Tests — InputSpec::Form contract round-trip (M6/E5)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod form_contract_tests {
    use super::*;
    use std::collections::HashMap;

    /// A node that exposes a single `InputSpec::Form` action, prefills it,
    /// and records the values delivered back through `execute`.
    struct FormNode {
        node_type: NodeType,
        metadata: Metadata,
        last_form: std::sync::Mutex<Option<HashMap<String, String>>>,
    }

    impl FormNode {
        fn new() -> Self {
            Self {
                node_type: NodeType {
                    type_id: "test:item".into(),
                    mime_type: "text/plain".into(),
                    syntax: None,
                    file_extension: ".txt".into(),
                    display_name: "Item".into(),
                },
                metadata: Metadata::default(),
                last_form: std::sync::Mutex::new(None),
            }
        }
    }

    #[async_trait::async_trait]
    impl Node for FormNode {
        fn id(&self) -> &str {
            "n1"
        }
        fn label(&self) -> &str {
            "Item"
        }
        fn node_type(&self) -> &NodeType {
            &self.node_type
        }
        fn metadata(&self) -> &Metadata {
            &self.metadata
        }

        fn actions(&self) -> Vec<NodeAction> {
            vec![NodeAction::new(
                "edit",
                "Edit",
                InputSpec::Form {
                    fields: vec![
                        FormFieldSpec::text("title", "Title"),
                        FormFieldSpec::select(
                            "status",
                            "Status",
                            vec!["todo".into(), "done".into()],
                        ),
                        FormFieldSpec::toggle("urgent", "Urgent"),
                    ],
                },
            )]
        }

        async fn form_prep(&self, action_id: &str) -> Result<HashMap<String, String>> {
            assert_eq!(action_id, "edit");
            let mut m = HashMap::new();
            m.insert("title".to_string(), "current".to_string());
            m.insert("status".to_string(), "done".to_string());
            Ok(m)
        }

        async fn execute(
            &mut self,
            action_id: &str,
            input: ActionInput,
        ) -> Result<ActionOutcome> {
            assert_eq!(action_id, "edit");
            match input {
                ActionInput::Form(values) => {
                    *self.last_form.lock().unwrap() = Some(values);
                    Ok(ActionOutcome::Done {
                        message: Some("saved".into()),
                    })
                }
                _ => panic!("expected ActionInput::Form"),
            }
        }
    }

    #[test]
    fn action_advertises_form_input_spec() {
        let node = FormNode::new();
        let action = node.actions().into_iter().find(|a| a.id == "edit").unwrap();
        match action.input {
            InputSpec::Form { fields } => {
                assert_eq!(fields.len(), 3);
                assert_eq!(fields[0].key, "title");
                assert!(matches!(fields[0].kind, FormFieldKind::Text));
                assert!(matches!(fields[2].kind, FormFieldKind::Toggle));
            }
            _ => panic!("expected Form input spec"),
        }
    }

    #[tokio::test]
    async fn form_prep_supplies_edit_prefill() {
        let node = FormNode::new();
        let prefill = node.form_prep("edit").await.unwrap();
        assert_eq!(prefill.get("title").unwrap(), "current");
        assert_eq!(prefill.get("status").unwrap(), "done");
    }

    #[tokio::test]
    async fn execute_receives_form_values() {
        let mut node = FormNode::new();
        let mut values = HashMap::new();
        values.insert("title".to_string(), "new title".to_string());
        values.insert("status".to_string(), "todo".to_string());
        values.insert("urgent".to_string(), "true".to_string());

        let outcome = node
            .execute("edit", ActionInput::Form(values))
            .await
            .unwrap();
        assert!(matches!(outcome, ActionOutcome::Done { .. }));

        let recorded = node.last_form.lock().unwrap().clone().unwrap();
        assert_eq!(recorded.get("title").unwrap(), "new title");
        assert_eq!(recorded.get("status").unwrap(), "todo");
        assert_eq!(recorded.get("urgent").unwrap(), "true");
    }
}

// ---------------------------------------------------------------------------
// Tests — mark/paste-move contract round-trip (M7/E6)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod mark_move_contract_tests {
    use super::*;

    /// A node that records the [`MarkedNode`] handed to it via
    /// [`ActionContext`] when a `paste-move` action is invoked.
    struct MoveNode {
        node_type: NodeType,
        metadata: Metadata,
        last_marked: std::sync::Mutex<Option<MarkedNode>>,
    }

    impl MoveNode {
        fn new() -> Self {
            Self {
                node_type: NodeType {
                    type_id: "test:item".into(),
                    mime_type: "text/plain".into(),
                    syntax: None,
                    file_extension: ".txt".into(),
                    display_name: "Item".into(),
                },
                metadata: Metadata::default(),
                last_marked: std::sync::Mutex::new(None),
            }
        }
    }

    #[async_trait::async_trait]
    impl Node for MoveNode {
        fn id(&self) -> &str {
            "target"
        }
        fn label(&self) -> &str {
            "Target"
        }
        fn node_type(&self) -> &NodeType {
            &self.node_type
        }
        fn metadata(&self) -> &Metadata {
            &self.metadata
        }

        fn actions(&self) -> Vec<NodeAction> {
            vec![
                NodeAction::new("mark-move", "Mark for move", InputSpec::None),
                NodeAction::new("paste-move", "Paste here", InputSpec::None),
            ]
        }

        async fn invoke_action(
            &self,
            name: &str,
            ctx: &ActionContext,
        ) -> Result<ActionDispatch> {
            match name {
                // `mark-move` is frontend-owned (records the marked node in
                // session state); the adapter has nothing to do → Noop.
                "mark-move" => Ok(ActionDispatch::Noop),
                // `paste-move` reads the marked node out of the context and
                // performs the move (here: just records what it received).
                "paste-move" => {
                    *self.last_marked.lock().unwrap() = ctx.marked.clone();
                    if ctx.marked.is_some() {
                        Ok(ActionDispatch::Reload)
                    } else {
                        Ok(ActionDispatch::Error("nothing marked".into()))
                    }
                }
                other => Ok(ActionDispatch::Error(format!("unknown action {other}"))),
            }
        }
    }

    #[test]
    fn action_context_default_has_no_mark() {
        let ctx = ActionContext::default();
        assert!(ctx.marked.is_none());
    }

    #[tokio::test]
    async fn paste_move_receives_marked_node_from_context() {
        let node = MoveNode::new();
        let marked = MarkedNode {
            node_id: "src-1".into(),
            node_type: node.node_type().clone(),
            label: "Source task".into(),
        };
        let ctx = ActionContext {
            marked: Some(marked.clone()),
            confirmed: false,
            query: None,
        };

        let dispatch = node.invoke_action("paste-move", &ctx).await.unwrap();
        assert!(matches!(dispatch, ActionDispatch::Reload));

        let recorded = node.last_marked.lock().unwrap().clone().unwrap();
        assert_eq!(recorded, marked);
        assert_eq!(recorded.node_id, "src-1");
    }

    #[tokio::test]
    async fn paste_move_without_mark_is_rejected() {
        let node = MoveNode::new();
        let dispatch = node
            .invoke_action("paste-move", &ActionContext::default())
            .await
            .unwrap();
        assert!(matches!(dispatch, ActionDispatch::Error(_)));
        assert!(node.last_marked.lock().unwrap().is_none());
    }
}

#[cfg(test)]
mod list_subtree_default_tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    fn nt(type_id: &str) -> NodeType {
        NodeType {
            type_id: type_id.into(),
            mime_type: "text/plain".into(),
            syntax: None,
            file_extension: ".txt".into(),
            display_name: type_id.into(),
        }
    }

    fn params() -> ListParams {
        ListParams {
            node_type: nt("mock:item"),
            query: None,
            sort: Vec::new(),
            page: None,
            download: false,
            group_by: None,
        }
    }

    /// Shared adjacency for the mock forest: `parent id -> [(child id, child
    /// type)]`. `liars` forces `has_children = Some(false)` on a node that
    /// actually *does* have edges, so we can prove the depth walk honours the
    /// leaf hint without calling `get_child`.
    struct Tree {
        edges: HashMap<String, Vec<(String, String)>>,
        liars: HashSet<String>,
    }

    fn tree(edges: &[(&str, &str, &str)]) -> Arc<Tree> {
        let mut map: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for (p, c, t) in edges {
            map.entry((*p).into())
                .or_default()
                .push(((*c).into(), (*t).into()));
        }
        Arc::new(Tree {
            edges: map,
            liars: HashSet::new(),
        })
    }

    /// A node backed by the shared [`Tree`]. `list` returns the edges whose
    /// child type matches the requested `node_type`; `children_types` reports
    /// the distinct child types in insertion order; `get_child` re-roots a
    /// fresh node at the requested id.
    struct MockNode {
        id: String,
        node_type: NodeType,
        metadata: Metadata,
        tree: Arc<Tree>,
    }

    fn root(tree: Arc<Tree>) -> MockNode {
        MockNode {
            id: "root".into(),
            node_type: nt("mock:node"),
            metadata: Metadata::default(),
            tree,
        }
    }

    #[async_trait::async_trait]
    impl Node for MockNode {
        fn id(&self) -> &str {
            &self.id
        }
        fn label(&self) -> &str {
            &self.id
        }
        fn node_type(&self) -> &NodeType {
            &self.node_type
        }
        fn metadata(&self) -> &Metadata {
            &self.metadata
        }

        fn children_types(&self) -> Vec<NodeType> {
            let mut out = Vec::new();
            let mut seen = Vec::new();
            if let Some(edges) = self.tree.edges.get(&self.id) {
                for (_c, t) in edges {
                    if !seen.contains(t) {
                        seen.push(t.clone());
                        out.push(nt(t));
                    }
                }
            }
            out
        }

        async fn list(&self, params: ListParams) -> Result<ListResult> {
            let want = &params.node_type.type_id;
            let items = self
                .tree
                .edges
                .get(&self.id)
                .map(|edges| {
                    edges
                        .iter()
                        .filter(|(_c, t)| t == want)
                        .map(|(c, _t)| {
                            let has = !self.tree.liars.contains(c)
                                && self
                                    .tree
                                    .edges
                                    .get(c)
                                    .map(|e| !e.is_empty())
                                    .unwrap_or(false);
                            NodeSummary {
                                id: c.clone(),
                                label: c.clone(),
                                node_type: nt(want),
                                metadata: Metadata::default(),
                                has_children: Some(has),
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            Ok(ListResult {
                items,
                applied_sort: Vec::new(),
                page: None,
                batch_download_available: false,
                downloaded: Vec::new(),
            })
        }

        async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
            Ok(Box::new(MockNode {
                id: id.to_string(),
                node_type: nt("mock:node"),
                metadata: Metadata::default(),
                tree: self.tree.clone(),
            }))
        }
    }

    fn child_ids(st: &Subtree) -> Vec<String> {
        st.items.iter().map(|n| n.summary.id.clone()).collect()
    }

    fn find<'a>(st: &'a Subtree, id: &str) -> &'a SubtreeNode {
        st.items
            .iter()
            .find(|n| n.summary.id == id)
            .unwrap_or_else(|| panic!("node {id} not found"))
    }

    // root → a, b ; a → a1, a2 ; a1 → a1x ; (b, a2, a1x are leaves)
    fn sample() -> Arc<Tree> {
        tree(&[
            ("root", "a", "mock:item"),
            ("root", "b", "mock:item"),
            ("a", "a1", "mock:item"),
            ("a", "a2", "mock:item"),
            ("a1", "a1x", "mock:item"),
        ])
    }

    #[tokio::test]
    async fn depth_zero_is_single_level() {
        let r = root(sample());
        let st = r.list_subtree(params(), 0).await.unwrap();
        assert_eq!(child_ids(&st), vec!["a", "b"]);
        // depth 0 ⇔ list(): no node is expanded.
        assert!(st.items.iter().all(|n| n.children.items.is_empty()));
    }

    #[tokio::test]
    async fn depth_one_expands_exactly_one_level() {
        let r = root(sample());
        let st = r.list_subtree(params(), 1).await.unwrap();

        let a = find(&st, "a");
        assert_eq!(child_ids(&a.children), vec!["a1", "a2"]);
        // one level only: a's children are not themselves expanded.
        assert!(a.children.items.iter().all(|n| n.children.items.is_empty()));

        // b is a genuine leaf (has_children == Some(false)) → not descended.
        let b = find(&st, "b");
        assert!(b.children.items.is_empty());
    }

    #[tokio::test]
    async fn depth_all_reaches_deepest_leaf() {
        let r = root(sample());
        let st = r.list_subtree(params(), u32::MAX).await.unwrap();
        let a = find(&st, "a");
        let a1 = find(&a.children, "a1");
        assert_eq!(child_ids(&a1.children), vec!["a1x"]);
        // recursion stops naturally at the leaf, no depth limit needed.
        assert!(a1.children.items[0].children.items.is_empty());
        // sibling leaf a2 stays empty.
        assert!(find(&a.children, "a2").children.items.is_empty());
    }

    #[tokio::test]
    async fn has_children_false_short_circuits_descent() {
        // `trap` has real edges but is flagged a liar → summary says leaf.
        let mut map: HashMap<String, Vec<(String, String)>> = HashMap::new();
        map.insert("root".into(), vec![("trap".into(), "mock:item".into())]);
        map.insert("trap".into(), vec![("hidden".into(), "mock:item".into())]);
        let t = Arc::new(Tree {
            edges: map,
            liars: HashSet::from(["trap".to_string()]),
        });
        let r = root(t);
        let st = r.list_subtree(params(), u32::MAX).await.unwrap();
        let trap = find(&st, "trap");
        assert_eq!(trap.summary.has_children, Some(false));
        // Even at depth all, the leaf hint prevents descent.
        assert!(trap.children.items.is_empty());
    }

    #[tokio::test]
    async fn multi_child_type_merges_typed_lists_in_order() {
        // root → p (mock:item); p has two child types x then y.
        let t = tree(&[
            ("root", "p", "mock:item"),
            ("p", "x1", "mock:x"),
            ("p", "y1", "mock:y"),
        ]);
        let r = root(t);
        let st = r.list_subtree(params(), 1).await.unwrap();
        let p = find(&st, "p");
        // Both typed child lists merged, child-type (insertion) order kept.
        assert_eq!(child_ids(&p.children), vec!["x1", "y1"]);
    }
}
