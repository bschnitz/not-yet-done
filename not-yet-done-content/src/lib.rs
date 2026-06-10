//! Generic content adapter abstraction.
//!
//! Provides a frontend-agnostic interface for connecting to remote content
//! systems (ticket trackers, wikis, databases). Each backend implements the
//! same trait interface so any frontend can work with any system uniformly.

#[cfg(any(test, feature = "mock"))]
pub mod mock;

pub mod auth;
pub mod http_log;
pub mod link_route;
pub mod node_ref;
pub mod slug;
pub mod sort_serde;

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

#[derive(Clone, Debug, Default)]
pub struct Metadata {
    pub fields: Vec<MetadataField>,
}

#[derive(Clone, Debug)]
pub struct MetadataField {
    pub key: String,
    pub value: String,
    pub display_label: String,
    pub editable: bool,
    /// Allowed values (for dropdowns). None = free text.
    pub allowed_values: Option<Vec<String>>,
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
}

#[derive(Clone, Debug)]
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
    /// Open an editor session. `session_kind` is the discriminator the
    /// TUI uses to pick the right `EditSession` impl (e.g.
    /// `"postgres_query"`, `"postgres_db_script"`). `params` carries the
    /// session's setup data as opaque string key/value pairs (e.g.
    /// `{"database": "live", "script": "report"}`).
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
    DeleteSelf,
    /// Reload the current pane.
    Reload,
    /// No-op — useful as a default for adapters that haven't migrated.
    Noop,
    /// Adapter rejected the action with a user-displayable error.
    Error(String),
}

/// Context passed into [`Node::invoke_action`]. Empty for now — extended
/// in later phases (cursor pagination state, target pane id, …).
#[derive(Clone, Debug, Default)]
pub struct ActionContext {}

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
}

// ---------------------------------------------------------------------------
// Core Traits
// ---------------------------------------------------------------------------

/// The entry point. One instance per configured connection.
#[async_trait]
pub trait ContentAdapter: Send + Sync {
    /// Downcast hook so a caller with only `&dyn ContentAdapter` can
    /// reach adapter-specific APIs (e.g. the Postgres script editor
    /// needs the Postgres table list). Each impl returns `self` — the
    /// trait stays object-safe because we don't add an `Any` bound.
    fn as_any(&self) -> &dyn std::any::Any;

    /// Stable type identifier of this adapter (e.g. "jira", "postgres",
    /// "taiga"). Used as the first path component in
    /// [`ContentAdapter::instance_data_dir`] and as a prefix in
    /// scope keys (e.g. saved-query scope).
    fn adapter_type(&self) -> &str;

    /// Stable per-instance identifier. Comes from the YAML
    /// `adapter.id:` field; defaults to [`adapter_type`] when not set.
    /// Two adapter instances loaded into the same App run must have
    /// distinct `instance_id`s — the loader validates this.
    fn instance_id(&self) -> &str;

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

    /// Capabilities of this adapter (for UI feature gating).
    fn capabilities(&self) -> AdapterCapabilities;

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
    /// [`ActionDispatch::OpenEditor`] for `session_kind == "postgres_query"`.
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
