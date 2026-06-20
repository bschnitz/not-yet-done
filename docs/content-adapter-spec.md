# ContentAdapter Specification

## Overview

A generic, frontend-agnostic abstraction for connecting to remote content
systems (ticket trackers, wikis, databases, etc.). Each backend implements
the same trait interface, allowing the TUI (or any other frontend) to work
with any system uniformly.

**Crate**: `not-yet-done-content` (standalone, no TUI dependency)

---

## Core Traits

### ContentAdapter

The entry point. One instance per configured connection.

```rust
#[async_trait]
pub trait ContentAdapter: Send + Sync {
    /// Human-readable name of this adapter type (e.g. "Jira", "Confluence").
    fn adapter_type(&self) -> &str;

    /// Navigate to the root node of the content tree.
    async fn root(&self) -> Result<Box<dyn Node>>;

    /// Direct access to a node by its ID (shortcut, avoids tree traversal).
    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>>;

    /// Capabilities of this adapter (for UI feature gating).
    fn capabilities(&self) -> AdapterCapabilities;

    /// Saved-query variables: extract `${name[:default]}`-style
    /// placeholders from a raw query. Each adapter chooses its own
    /// syntax; default impl returns an empty vec (no variables, the
    /// frontend skips the input popup).
    fn query_variables(&self, _query: &str) -> Vec<QueryVariable> {
        Vec::new()
    }

    /// Render a saved-query string by substituting variables. The
    /// returned string is what is passed into `ListParams::query`.
    /// Default impl returns `query` verbatim — pair with the default
    /// `query_variables` to opt out of variable handling entirely.
    fn render_query(
        &self,
        query: &str,
        _vars: &HashMap<String, String>,
    ) -> String {
        query.to_string()
    }
}
```

**Construction**: `fn from_config(config: &str) -> Result<Box<dyn ContentAdapter>>`
— the config is an opaque string (YAML/JSON), adapter-specific.

**Query variables.** Saved queries on a view can carry inline
placeholders. The TUI calls `query_variables(raw)` at apply time,
asks the user to fill any missing required values via a popup, and
passes the bindings back through `render_query(raw, &vars)` right
before the load. The `${name:default}` syntax (no default → required)
is implemented as an adapter-agnostic helper in
`not_yet_done_content::query_vars`; the Taiga and Jira adapters both
delegate to it (Taiga structured filters, Jira JQL — e.g. a `By Key`
saved query `key = ${key}`). Adapters that don't implement the methods
get the identity defaults and the popup is never opened.

### Node

A single item in the content tree. Can be a project, issue, page, table, row, etc.

```rust
#[async_trait]
pub trait Node: Send + Sync {
    /// Unique identifier within the adapter.
    fn id(&self) -> &str;

    /// Display label (e.g. ticket summary, page title).
    fn label(&self) -> &str;

    /// Type of this node.
    fn node_type(&self) -> &NodeType;

    /// Key-value metadata (status, priority, assignee, etc.).
    fn metadata(&self) -> &Metadata;

    /// Which child node types are available under this node.
    fn children_types(&self) -> Vec<NodeType>;

    /// List child nodes of a given type.
    async fn list(&self, params: ListParams) -> Result<ListResult>;

    /// List a whole subtree eagerly, `depth + 1` levels deep, in one call.
    /// Default impl recurses over `list()` + `get_child()` (one call per
    /// node — same work as the per-node cascade, just bundled adapter-side).
    /// In-memory adapters override it with a snapshot projection walk (no
    /// I/O). Only consulted when `capabilities().supports_eager_subtree`.
    async fn list_subtree(&self, params: ListParams, depth: u32) -> Result<Subtree>;

    /// Navigate to a specific child by ID.
    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>>;

    /// Content access (download/upload). None if this node has no content body.
    fn content(&self) -> Option<&dyn Content>;

    /// Mutable content access for editing. None if not editable.
    fn content_mut(&mut self) -> Option<&mut dyn ContentMut>;

    /// Create a new child node of the given type.
    async fn create_child(&self, node_type: &NodeType, data: CreateParams) -> Result<Box<dyn Node>>;

    /// Delete this node.
    async fn delete(&self) -> Result<()>;
}
```

### NodeSummary

Lightweight representation returned by `list()`. Contains only display-relevant
fields, not the full content.

```rust
pub struct NodeSummary {
    pub id: String,
    pub label: String,
    pub node_type: NodeType,
    pub metadata: Metadata,
}
```

### Content (read-only)

```rust
#[async_trait]
pub trait Content: Send + Sync {
    fn node_type(&self) -> &NodeType;
    fn is_editable(&self) -> bool;

    /// Version identifier for conflict detection.
    /// Compared before upload to detect concurrent modifications.
    fn version(&self) -> Option<&str>;

    /// Download the content body.
    async fn read(&self) -> Result<Vec<u8>>;

    /// Read as text (convenience, fails for binary).
    async fn read_text(&self) -> Result<String> {
        let bytes = self.read().await?;
        String::from_utf8(bytes).map_err(|e| e.into())
    }
}
```

### ContentMut (read-write)

```rust
#[async_trait]
pub trait ContentMut: Content {
    /// Upload new content. Returns the new version identifier.
    ///
    /// `expected_version`: the version from the last read. If the remote
    /// version differs, returns `Err(ConflictError { remote_version, remote_content })`.
    async fn write(&mut self, data: &[u8], expected_version: Option<&str>) -> Result<String>;

    /// Write text content (convenience).
    async fn write_text(&mut self, text: &str, expected_version: Option<&str>) -> Result<String> {
        self.write(text.as_bytes(), expected_version).await
    }
}
```

### Metadata

Editable key-value metadata on a node (e.g. ticket status, assignee, labels).

```rust
pub struct Metadata {
    pub fields: Vec<MetadataField>,
}

pub struct MetadataField {
    pub key: String,
    pub value: String,
    pub display_label: String,
    pub editable: bool,
    /// Allowed values (for dropdowns). None = free text.
    pub allowed_values: Option<Vec<String>>,
}
```

**Editing metadata**: Update via `Node::update_metadata(changes: &[MetadataChange])`.
Conflict resolution: metadata carries a version too (same `version()` as content,
or a separate metadata version if the system supports it).

```rust
pub struct MetadataChange {
    pub key: String,
    pub new_value: String,
}

#[async_trait]
impl Node {
    /// Update metadata fields. Returns Err(ConflictError) if version mismatch.
    async fn update_metadata(
        &self,
        changes: &[MetadataChange],
        expected_version: Option<&str>,
    ) -> Result<()>;
}
```

---

## Supporting Types

### NodeType

Describes what kind of content a node represents.

```rust
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
```

### ListParams

Parameters for listing child nodes.

```rust
pub struct ListParams {
    /// Which child type to list.
    pub node_type: NodeType,
    /// Opaque query string in the backend's native language (JQL, SQL, etc.).
    pub query: Option<String>,
    /// Opaque ordering string.
    pub order: Option<String>,
    /// Pagination: starting offset.
    pub offset: u32,
    /// Pagination: max items to return.
    pub limit: u32,
    /// If true, download full content for each item (batch).
    /// If false, return NodeSummary only (lazy).
    pub download: bool,
}
```

### ListResult

```rust
pub struct ListResult {
    pub items: Vec<NodeSummary>,
    /// Total count of matching items (if the backend supports it).
    pub total: Option<u64>,
    /// Whether batch download is available for this node type.
    pub batch_download_available: bool,
    /// If download=true was requested, the full nodes.
    pub downloaded: Vec<Box<dyn Node>>,
}
```

### AdapterCapabilities

Feature flags for the UI to know what's possible.

```rust
pub struct AdapterCapabilities {
    pub supports_create: bool,
    pub supports_delete: bool,
    pub supports_search: bool,
    pub supports_batch_download: bool,
    /// Whether list() can return total counts.
    pub supports_total_count: bool,
    /// Adapter computes subtree-cumulated values for tree nodes (M4).
    pub supports_tree_aggregation: bool,
    /// Active query threads into child list() calls at every depth, not
    /// just the root — keeps a filtered tree filtered below the root.
    pub propagates_query_to_subtree: bool,
    /// Adapter groups its tree root level itself (one bucket node per
    /// group with a folded subtree) when given `ListParams::group_by`.
    pub group_by_via_adapter: bool,
    /// Adapter can build a whole multi-level subtree in one
    /// `list_subtree` call. In-memory adapters (Tasks, Trackings) set this
    /// `true`; the engine then expands a tree's initial/reloaded state with
    /// one eager call instead of the O(N²) per-node cascade. Remote
    /// adapters leave it `false` to keep the progressive, responsive
    /// cascade. See `docs/generic-view-spec.md` → Eager-Subtree.
    pub supports_eager_subtree: bool,
}
```

### ConflictError

```rust
pub struct ConflictError {
    pub remote_version: String,
    pub remote_content: Option<Vec<u8>>,
    pub message: String,
}
```

---

## Example: Jira Adapter

```
JiraAdapter (config = connection YAML with url + auth)
  └── root: JiraRoot
        └── children_types: [Project]
              └── list(Project) → [PROJ, OTHER, ...]
                    └── children_types: [Issue, Sprint, Board]
                          └── list(Issue, query="assignee=currentUser()") →
                                [PROJ-202, PROJ-101, ...]
                                  ├── content: description (text/x-jira-wiki, editable)
                                  ├── metadata: status, priority, assignee, labels
                                  └── children_types: [Comment, Attachment]
                                        ├── Comment → content (text, editable)
                                        └── Attachment → content (binary, read-only)
```

## Example: Confluence Adapter

```
ConfluenceAdapter
  └── root → children_types: [Space]
        └── Space → children_types: [Page]
              └── Page → content: body (text/x-confluence-wiki, editable)
                       → children_types: [Page, Attachment]
```

## Example: PostgreSQL Adapter

```
PostgresAdapter (config = connection string)
  └── root → children_types: [Schema]
        └── Schema → children_types: [Table, View, Function]
              └── Table → children_types: [Row]
                    └── list(Row, query="WHERE active=true", limit=100)
                          → Row → content: JSON (application/json, editable)
                                → metadata: column values
```

---

## Metadata Editing & Conflict Resolution

Metadata fields (status, assignee, labels, etc.) are exposed via `Node::metadata()`.
They can be edited via `Node::update_metadata()`.

Conflict resolution works the same as for content:

- Read includes a `version`
- Write sends `expected_version`
- If the remote version has changed → `ConflictError` with the remote state
- Frontend (TUI) can show a diff/merge UI or retry

For systems where metadata and content have independent versioning (e.g. Jira:
updating description doesn't change the status version), the adapter should
track the most relevant version. For simple cases, using the `updated_at`
timestamp of the whole node is sufficient.

---

## Design Decisions

1. **Opaque query/order strings** — No attempt to unify query languages.
   The user must know the backend's query syntax (JQL, SQL, etc.).
   The adapter can provide help text / schema discovery as metadata.

2. **Pagination is optional** — `ListResult.total` is `Option<u64>`.
   Backends that don't support counting return `None`.
   The UI adapts (infinite scroll vs. page indicator).

3. **Batch download is opt-in** — `ListParams.download = true` asks the
   adapter to prefetch full content. `ListResult.batch_download_available`
   lets the UI know if this is possible. Some backends (wikis) need
   individual API calls per item; others (DBs) can batch efficiently.

4. **Content vs Metadata separation** — Content is the "body" (description,
   page text, row data). Metadata is structured key-value fields. Both are
   independently editable with conflict detection.

5. **Frontend-agnostic** — No TUI, no Ratatui, no tuirealm dependencies.
   The crate provides only traits and data types. Any frontend can use it.

6. **Adapter registration** — A registry pattern allows dynamic adapter
   discovery:
   ```rust
   pub trait AdapterFactory: Send + Sync {
       fn adapter_type(&self) -> &str;
       fn create(
           &self,
           instance_id: &str,
           config: &str,
           ctx: &HostContext,
       ) -> Result<Box<dyn ContentAdapter>>;
   }
   ```

   - `instance_id` — stable per-tab id, used to scope adapter-side
     filesystem state (e.g. saved queries).
   - `config` — the opaque adapter-specific string (YAML/JSON) from the
     tab's `config_inline` / `config:` path.
   - `ctx` — the **`HostContext`** the host owns and passes to every
     adapter. It carries a `HostEventBus` (`publish(channel, event)` /
     `subscribe(channel) -> Receiver<HostEvent>`) for cross-adapter
     coordination: the host is a dumb broker, the payload is an opaque
     `Arc<dyn Any + Send + Sync>`, and adapters that share a channel
     privately agree on the concrete payload type and downcast it. The
     in-process Tasks/Trackings adapters use this — keyed by their database
     DSN — so a tracking toggle in one tab repaints the other; remote
     adapters that need no coordination simply ignore `ctx`.

---

## Open Questions

- **Streaming large content**: Should `read()` return `AsyncRead` instead of
  `Vec<u8>` for large files/attachments? Pro: memory-efficient. Con: more
  complex API. Could offer both (`read()` for small, `read_stream()` for large).

- **Caching layer**: Should the adapter handle caching, or is that a separate
  concern (middleware/wrapper)? A `CachingAdapter<T: ContentAdapter>` wrapper
  could be useful.

- **Event/notification support**: Some systems support webhooks or change feeds.
  Out of scope for v1, but the trait could be extended later with
  `fn watch(&self) -> Stream<ChangeEvent>`.

- **Schema discovery**: How does the UI know what metadata fields exist?
  `Node::metadata()` returns the current fields, but for _creating_ a node,
  the UI needs to know the schema upfront. Could be exposed via
  `NodeType::schema() -> Vec<MetadataFieldSchema>`.

---

## Implementation Plan

### Phase 1: Trait Crate

- Create `not-yet-done-content` crate with all traits and types
- No implementations yet, just the interface

### Phase 2: Jira Adapter

- Implement `JiraAdapter` using existing `JiraClient`
- Map: Root → Projects → Issues → Comments/Attachments
- Content: description (editable), attachments (read-only)
- Metadata: status, priority, assignee, labels

### Phase 3: TUI Integration

- Refactor JiraView to use `ContentAdapter` instead of `JiraClient` directly
- Generic "content browser" component that works with any adapter

### Phase 4: Additional Adapters

- Confluence adapter
- Database adapter (PostgreSQL/SQLite)
