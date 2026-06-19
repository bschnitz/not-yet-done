//! Postgres ContentAdapter: navigates the live catalogue
//!   root → database → "Schemas" → schema → "Tables" → table
//! mirroring DBeaver's tree. The two intermediate group nodes
//! ("Schemas", "Tables") have no per-instance state — they exist for
//! visual structure and to give a stable place to attach future
//! sibling groups (Views, Functions, …).
//!
//! For convenience, `DatabaseNode` also lists `postgres:schema`
//! directly (and `SchemaNode` lists `postgres:table` directly), so a
//! YAML view can drill through without the group nodes. The emitted
//! IDs still encode the full path (`<db>/schemas/<s>/tables/<t>`),
//! which keeps `get_by_id`'s walker happy — it always traverses the
//! group nodes internally.

use std::sync::Arc;

use async_trait::async_trait;

use not_yet_done_content::{
    ActionContext, ActionDispatch, AdapterCapabilities, AdapterStatus, ContentAdapter,
    ContentError, CursorIntent, CustomQueryContext, CustomQueryResult, HintPlacement, InputSpec,
    ListParams, ListResult, Metadata, MetadataField, Node, NodeAction, NodeRef, NodeSummary,
    NodeType, PageInfo, PageRequest, Result,
};

use crate::client::{DatabaseEntry, PostgresClient, SchemaEntry, TableEntry};

mod cursor_registry;
mod factory;

pub use cursor_registry::{CursorId, CursorRegistry};
pub use factory::PostgresAdapterFactory;

pub struct PostgresAdapter {
    client: Arc<PostgresClient>,
    cursor_registry: Arc<CursorRegistry>,
    connection_name: String,
    instance_id: String,
}

impl PostgresAdapter {
    pub(crate) fn from_client(
        client: Arc<PostgresClient>,
        connection_name: String,
        instance_id: String,
    ) -> Self {
        let cursor_registry = Arc::new(CursorRegistry::new(Arc::clone(&client)));
        Self {
            client,
            cursor_registry,
            connection_name,
            instance_id,
        }
    }

    /// Read the persisted query file for `(database, schema, table, script)`,
    /// returning a fresh default template (see
    /// [`crate::query::default_query_file`]) if no file exists yet.
    /// Other I/O errors propagate.
    pub async fn load_query_file(
        &self,
        database: &str,
        schema: &str,
        table: &str,
        script: &str,
    ) -> Result<String> {
        let path = crate::query::query_file_path(
            &self.instance_data_dir(),
            database,
            schema,
            table,
            script,
        );
        match tokio::fs::read_to_string(&path).await {
            Ok(text) => Ok(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(crate::query::default_query_file(schema, table))
            }
            Err(e) => Err(ContentError::Other(Box::new(e))),
        }
    }

    /// Persist the editor buffer for `(database, schema, table, script)`.
    /// Creates the `queries/<database>/<schema>/<table>/` parent
    /// directory tree on first save.
    pub async fn save_query_file(
        &self,
        database: &str,
        schema: &str,
        table: &str,
        script: &str,
        content: &str,
    ) -> Result<()> {
        let path = crate::query::query_file_path(
            &self.instance_data_dir(),
            database,
            schema,
            table,
            script,
        );
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ContentError::Other(Box::new(e)))?;
        }
        tokio::fs::write(&path, content)
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))?;
        Ok(())
    }

    /// Read the persisted DB-level script file for `(database, script)`,
    /// returning a fresh default template if no file exists yet. Mirrors
    /// [`Self::load_query_file`] but with the two-segment layout.
    pub async fn load_db_script_file(&self, database: &str, script: &str) -> Result<String> {
        crate::query::read_db_script(&self.instance_data_dir(), database, script)
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))
    }

    /// Persist a DB-level script. Creates the
    /// `db_scripts/<database>/` parent directory on first save.
    pub async fn save_db_script_file(
        &self,
        database: &str,
        script: &str,
        content: &str,
    ) -> Result<()> {
        crate::query::write_db_script(&self.instance_data_dir(), database, script, content)
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))
    }

    /// `(schema, table)` pairs for every base table in `database`,
    /// used by the script editor to populate the trailing `tt_*`
    /// completion comment. Errors are swallowed and reported as an
    /// empty list so a failing catalog query never blocks the editor
    /// from opening — the user can still type SQL by hand.
    pub async fn list_completion_tables(
        &self,
        database: &str,
    ) -> Vec<(String, String)> {
        self.client
            .list_tables_in_database(database)
            .await
            .unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// NodeType helpers
// ---------------------------------------------------------------------------

fn root_node_type() -> NodeType {
    NodeType {
        type_id: "postgres:root".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Postgres Root".into(),
    }
}

fn database_node_type() -> NodeType {
    NodeType {
        type_id: "postgres:database".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Database".into(),
    }
}

fn schemas_group_node_type() -> NodeType {
    NodeType {
        type_id: "postgres:schemas".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Schemas".into(),
    }
}

fn schema_node_type() -> NodeType {
    NodeType {
        type_id: "postgres:schema".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Schema".into(),
    }
}

fn tables_group_node_type() -> NodeType {
    NodeType {
        type_id: "postgres:tables".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Tables".into(),
    }
}

fn table_node_type() -> NodeType {
    NodeType {
        type_id: "postgres:table".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Table".into(),
    }
}

fn row_node_type() -> NodeType {
    NodeType {
        type_id: "postgres:row".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Row".into(),
    }
}

fn script_node_type() -> NodeType {
    NodeType {
        type_id: "postgres:script".into(),
        mime_type: "text/x-sql".into(),
        syntax: Some("sql".into()),
        file_extension: "sql".into(),
        display_name: "Script".into(),
    }
}

fn db_scripts_group_node_type() -> NodeType {
    NodeType {
        type_id: "postgres:db_scripts".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "DB Scripts".into(),
    }
}

fn db_script_node_type() -> NodeType {
    NodeType {
        type_id: "postgres:db_script".into(),
        mime_type: "text/x-sql".into(),
        syntax: Some("sql".into()),
        file_extension: "sql".into(),
        display_name: "DB Script".into(),
    }
}

fn db_script_dir_node_type() -> NodeType {
    NodeType {
        type_id: "postgres:db_script_dir".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "DB Script Folder".into(),
    }
}

// ---------------------------------------------------------------------------
// Metadata helpers
// ---------------------------------------------------------------------------

fn database_metadata(entry: &DatabaseEntry) -> Metadata {
    Metadata {
        fields: vec![
            field("name", "Name", &entry.name),
            field("owner", "Owner", &entry.owner),
            field("encoding", "Encoding", &entry.encoding),
        ],
    }
}

fn schema_metadata(entry: &SchemaEntry) -> Metadata {
    Metadata {
        fields: vec![
            field("name", "Name", &entry.name),
            field("owner", "Owner", &entry.owner),
        ],
    }
}

fn script_metadata(entry: &crate::query::ScriptEntry) -> Metadata {
    Metadata {
        fields: vec![
            field("script", "Script", &entry.script),
            field("database", "Database", &entry.database),
            field("schema", "Schema", &entry.schema),
            field("table", "Table", &entry.table),
        ],
    }
}

/// Metadata for a single DB-level script row. `script_label` is what the
/// list view should display in the `script` column — typically the leaf
/// name (`"audit"`) rather than the full rel_path (`"util/audit"`), so a
/// long folder chain doesn't crowd out the other columns.
fn db_script_metadata(database: &str, script_label: &str) -> Metadata {
    Metadata {
        fields: vec![
            field("script", "Script", script_label),
            field("database", "Database", database),
        ],
    }
}

/// Metadata for a folder row under DB Scripts. Mirrors
/// [`db_script_metadata`] so a generic table view can render both kinds
/// against the same column set; the `script` field carries the folder
/// name. DSF-4 will surface a distinct icon via `tree_label` in YAML.
fn db_script_dir_metadata(database: &str, dir_label: &str) -> Metadata {
    Metadata {
        fields: vec![
            field("script", "Script", dir_label),
            field("database", "Database", database),
        ],
    }
}

fn table_metadata(entry: &TableEntry) -> Metadata {
    Metadata {
        fields: vec![
            field("name", "Name", &entry.name),
            field("database", "Database", &entry.database),
            field("schema", "Schema", &entry.schema),
            field("owner", "Owner", &entry.owner),
            field(
                "estimated_rows",
                "Rows (est.)",
                &entry.estimated_rows.to_string(),
            ),
        ],
    }
}

fn field(key: &str, label: &str, value: &str) -> MetadataField {
    MetadataField {
        key: key.into(),
        value: value.into(),
        display_label: label.into(),
        editable: false,
        allowed_values: None,
    }
}

// ---------------------------------------------------------------------------
// Static action sets per node type. Same lists the per-node `actions()`
// impls return for a fresh instance — exposed as free functions so the
// adapter-level `actions_for_type` (instance-free, no DB walk) can serve
// shortcut-hint rendering without a `get_by_id` chain walk per cursor move.
// ---------------------------------------------------------------------------

fn db_scripts_group_actions() -> Vec<NodeAction> {
    // YAML `shortcuts:` on the `postgres:db_scripts` ChildDef:
    //   a: add-script → :db-script new prompt
    //   A: add-dir    → :db-script new-dir prompt (DSF-2)
    vec![
        NodeAction::new("add-script", "add", InputSpec::None)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('a'),
        NodeAction::new("add-dir", "add-dir", InputSpec::None)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('A'),
    ]
}

fn db_script_dir_actions() -> Vec<NodeAction> {
    // YAML `shortcuts:` on the `postgres:db_script_dir` ChildDef:
    //   a/A: add-script/add-dir | r: rename
    //   m/p: mark/paste move | d: delete-dir
    vec![
        NodeAction::new("add-script", "add", InputSpec::None)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('a'),
        NodeAction::new("add-dir", "add-dir", InputSpec::None)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('A'),
        NodeAction::new("rename", "rename", InputSpec::None)
            .with_placement(HintPlacement::StatusBar)
            .with_default_key('r'),
        NodeAction::new("mark-move", "mark", InputSpec::None)
            .with_placement(HintPlacement::StatusBar)
            .with_default_key('m'),
        NodeAction::new("paste-move", "paste", InputSpec::None)
            .with_placement(HintPlacement::StatusBar)
            .with_default_key('p'),
        NodeAction::new("delete-dir", "del-dir", InputSpec::None)
            .with_placement(HintPlacement::StatusBar)
            .with_default_key('d'),
    ]
}

fn db_script_actions() -> Vec<NodeAction> {
    // YAML `shortcuts:` on the `postgres:db_script` ChildDef:
    //   x: execute | e: edit | r: rename | m: mark-move | d: delete
    // Only `edit` lands in the (highlighted) action bar — editor-action
    // convention (see ~/.claude memory feedback_bar_placement).
    vec![
        NodeAction::new("execute", "exec", InputSpec::None)
            .with_placement(HintPlacement::StatusBar)
            .with_default_key('x'),
        NodeAction::new("edit", "edit", InputSpec::None)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('e'),
        NodeAction::new("rename", "rename", InputSpec::None)
            .with_placement(HintPlacement::StatusBar)
            .with_default_key('r'),
        NodeAction::new("mark-move", "mark", InputSpec::None)
            .with_placement(HintPlacement::StatusBar)
            .with_default_key('m'),
        NodeAction::new("delete", "del", InputSpec::None)
            .with_placement(HintPlacement::StatusBar)
            .with_default_key('d'),
    ]
}

fn table_actions() -> Vec<NodeAction> {
    // YAML `shortcuts:` on the `postgres:tables` (Q on table row) or
    // `postgres:rows` (Q: parent:edit_sql when drilled in) ChildDef.
    vec![
        NodeAction::new("edit_sql", "sql", InputSpec::None)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('Q'),
    ]
}

// ---------------------------------------------------------------------------
// Adapter
// ---------------------------------------------------------------------------

#[async_trait]
impl ContentAdapter for PostgresAdapter {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn adapter_type(&self) -> &str {
        "postgres"
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn subscribe_status(&self) -> tokio::sync::watch::Receiver<AdapterStatus> {
        self.client.subscribe_status()
    }

    async fn root(&self) -> Result<Box<dyn Node>> {
        Ok(Box::new(PostgresRoot {
            client: Arc::clone(&self.client),
            connection_name: self.connection_name.clone(),
            instance_data_dir: self.instance_data_dir(),
        }))
    }

    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
        // Composite path: `<db>` / `<db>/schemas` / `<db>/schemas/<s>`
        // / `<db>/schemas/<s>/tables` / `<db>/schemas/<s>/tables/<t>`.
        // Walk from the root via `get_child` segment-by-segment so each
        // level's lookup logic (db lookup, fixed-sentinel match, schema
        // lookup, …) stays where it belongs.
        let mut node: Box<dyn Node> = self.root().await?;
        for part in id.split('/') {
            if part.is_empty() {
                continue;
            }
            node = node.get_child(part).await?;
        }
        Ok(node)
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            supports_create: false,
            supports_delete: false,
            supports_search: false,
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
            "postgres:db_scripts" => db_scripts_group_actions(),
            "postgres:db_script_dir" => db_script_dir_actions(),
            "postgres:db_script" => db_script_actions(),
            "postgres:table" => table_actions(),
            _ => Vec::new(),
        }
    }

    /// Expose libpq-style env vars (`PGHOST`/`PGPORT`/`PGUSER`/
    /// `PGPASSWORD`/`PGDATABASE`/`PGSSLMODE`) to children spawned in
    /// this adapter's context — primarily the editor's
    /// `postgres-language-server` LSP, which needs a live DB connection
    /// for any kind of completion.
    ///
    /// Source values are snapshotted in [`PostgresClient`]'s
    /// `env_cache` whenever a session connect succeeds; the cache is
    /// dropped on tear-down. If no live connection exists the map is
    /// empty, so the LSP just runs offline (and yields no completions).
    ///
    /// `PGDATABASE` is derived from the second segment of `node`
    /// (canonical Postgres node id: `<tab>/<db>/...`). When the spawn
    /// happens on a node without a database in scope (e.g. the
    /// database-list view itself, or a tasks-tab spawn that never
    /// reaches this adapter), we fall back to the configured admin
    /// database — same default `tokio_postgres` would pick.
    fn child_process_env(
        &self,
        node: &NodeRef,
    ) -> std::collections::HashMap<String, String> {
        let segs: Vec<&str> = node.segments().collect();
        let Some(mut env) = self.client.child_env_base() else {
            not_yet_done_content::http_log::log_debug(
                "pg.child_process_env",
                &format!("base=None segs={:?} -> empty", segs),
            );
            return std::collections::HashMap::new();
        };
        let db = node
            .segments()
            .nth(1)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| self.client.admin_database().to_string());
        env.insert("PGDATABASE".to_string(), db.clone());
        let mut keys: Vec<&String> = env.keys().collect();
        keys.sort();
        not_yet_done_content::http_log::log_debug(
            "pg.child_process_env",
            &format!(
                "segs={:?} db={} keys={:?} len={}",
                segs,
                db,
                keys,
                env.len()
            ),
        );
        env
    }

    /// Free-form SQL (potentially multi-statement). The
    /// [`CustomQueryContext`] must carry a `database` key — one
    /// `tokio_postgres` session is opened per database, so the caller
    /// has to pin the connection to one. Returns the last statement's
    /// rows (mapped to `qrow:<i>` `NodeSummary`s) or, for non-resultset
    /// statements, an empty list with a status string.
    async fn execute_custom_query(
        &self,
        query: &str,
        context: &CustomQueryContext,
    ) -> Result<CustomQueryResult> {
        let database = context.get("database").ok_or_else(|| {
            ContentError::NotSupported(
                "postgres execute_custom_query needs a `database` context field".into(),
            )
        })?;

        // Expand `tt_<schema>__<table>` completion tokens before either
        // branch sees the query. Fast-path: skip the catalog round trip
        // unless the body actually contains the prefix. Failure to
        // enumerate tables leaves the query unchanged — Postgres then
        // surfaces the literal token in its own error.
        let owned_query;
        let query_ref = if query.contains("tt_") {
            let tables = self.client.list_tables_in_database(database).await
                .unwrap_or_default();
            owned_query =
                crate::script_completions::substitute_table_tokens(query, &tables);
            owned_query.as_str()
        } else {
            query
        };

        if let Some(intent) = &context.cursor {
            return self
                .execute_custom_query_cursor(database, query_ref, context, intent)
                .await;
        }

        // Try to wrap a single-statement SELECT/WITH with LIMIT/OFFSET
        // for automatic pagination. We fetch `limit+1` rows so the
        // last page can be detected without an extra COUNT(*) round
        // trip. For DML/DDL or multi-statement queries the original
        // text is executed as-is and no pagination is reported.
        let (effective_query, page_request) = match context.page {
            Some(req) => match wrap_for_pagination(query_ref, req) {
                Some(wrapped) => (wrapped, Some(req)),
                None => (query_ref.to_string(), None),
            },
            None => (query_ref.to_string(), None),
        };

        let outcome = self
            .client
            .execute_raw_sql(database, &effective_query)
            .await
            .map_err(|e| ContentError::Other(e.into()))?;

        let (display_rows, page_info) = match page_request {
            Some(req) => {
                let has_next = outcome.rows.len() > req.limit as usize;
                let trimmed: Vec<_> =
                    outcome.rows.iter().take(req.limit as usize).cloned().collect();
                let info = PageInfo {
                    offset: req.offset,
                    limit: req.limit,
                    total: None,
                    has_next,
                    has_prev: req.offset > 0,
                };
                (trimmed, Some(info))
            }
            None => (outcome.rows.clone(), None),
        };

        let items = rows_to_summaries(&outcome.columns, &display_rows);

        Ok(CustomQueryResult {
            columns: outcome.columns,
            items,
            status: outcome.status,
            page: page_info,
            cursor_id: None,
        })
    }
}

impl PostgresAdapter {
    /// Cursor-pagination branch of `execute_custom_query`. The caller
    /// has already validated the `database` context field; this
    /// dispatches on the [`CursorIntent`] lifecycle step. `page_size`
    /// comes from `context.page.limit` and falls back to a sensible
    /// default when the caller didn't supply one.
    async fn execute_custom_query_cursor(
        &self,
        database: &str,
        query: &str,
        context: &CustomQueryContext,
        intent: &CursorIntent,
    ) -> Result<CustomQueryResult> {
        const CURSOR_PAGE_DEFAULT: u32 = 100;
        let page_size = context
            .page
            .map(|p| p.limit)
            .unwrap_or(CURSOR_PAGE_DEFAULT);
        let offset = context.page.map(|p| p.offset).unwrap_or(0);

        match intent {
            CursorIntent::Open => {
                let (id, page) = self
                    .cursor_registry
                    .open(database, query, page_size)
                    .await
                    .map_err(|e| ContentError::Other(e.into()))?;
                let items = rows_to_summaries(&page.columns, &page.rows);
                let info = PageInfo {
                    offset: 0,
                    limit: page_size,
                    total: None,
                    has_next: page.has_more,
                    has_prev: false,
                };
                Ok(CustomQueryResult {
                    columns: page.columns,
                    items,
                    status: None,
                    page: Some(info),
                    cursor_id: Some(id),
                })
            }
            CursorIntent::Continue { cursor_id } => {
                let page = self
                    .cursor_registry
                    .fetch(cursor_id, page_size)
                    .await
                    .map_err(|e| ContentError::Other(e.into()))?;
                let items = rows_to_summaries(&page.columns, &page.rows);
                let info = PageInfo {
                    offset,
                    limit: page_size,
                    total: None,
                    has_next: page.has_more,
                    has_prev: offset > 0,
                };
                Ok(CustomQueryResult {
                    columns: page.columns,
                    items,
                    status: None,
                    page: Some(info),
                    cursor_id: Some(cursor_id.clone()),
                })
            }
            CursorIntent::Close { cursor_id } => {
                self.cursor_registry
                    .close(cursor_id)
                    .await
                    .map_err(|e| ContentError::Other(e.into()))?;
                Ok(CustomQueryResult {
                    columns: vec![],
                    items: vec![],
                    status: Some("cursor closed".into()),
                    page: None,
                    cursor_id: None,
                })
            }
        }
    }
}

/// Map raw column-and-rows output to the `qrow:<i>` `NodeSummary` shape
/// the TUI's custom-query pane consumes. Shared between the LIMIT/OFFSET
/// branch and the cursor branch of `execute_custom_query`.
fn rows_to_summaries(
    columns: &[String],
    rows: &[Vec<Option<String>>],
) -> Vec<NodeSummary> {
    let row_type = row_node_type();
    rows.iter()
        .enumerate()
        .map(|(i, row)| {
            let fields = columns
                .iter()
                .zip(row.iter())
                .map(|(col, val)| MetadataField {
                    key: col.clone(),
                    value: val.clone().unwrap_or_else(|| "(null)".into()),
                    display_label: col.clone(),
                    editable: false,
                    allowed_values: None,
                })
                .collect();
            NodeSummary {
                id: format!("qrow:{i}"),
                label: format!("row {i}"),
                node_type: row_type.clone(),
                metadata: Metadata { fields },
                has_children: None,
            }
        })
        .collect()
}

/// Wrap a SELECT/WITH query with `LIMIT/OFFSET` for automatic
/// pagination. Returns `None` (caller runs the original) when the
/// query isn't a paginable shape: not SELECT/WITH, multi-statement,
/// or already contains an outer `LIMIT`/`OFFSET` we'd rather not
/// fight with. We fetch one extra row (`limit + 1`) so the caller
/// can decide `has_next` without a second round trip.
fn wrap_for_pagination(query: &str, page: PageRequest) -> Option<String> {
    use crate::client::sql_shape::{has_multiple_statements, looks_like_select_or_with};
    let trimmed = query.trim().trim_end_matches(';').trim();
    if !looks_like_select_or_with(trimmed) {
        return None;
    }
    if has_multiple_statements(trimmed) {
        return None;
    }
    Some(format!(
        "SELECT * FROM ({}) AS _nyd_pg LIMIT {} OFFSET {}",
        trimmed,
        page.limit.saturating_add(1),
        page.offset,
    ))
}

// ---------------------------------------------------------------------------
// Root node — children are databases
// ---------------------------------------------------------------------------

struct PostgresRoot {
    client: Arc<PostgresClient>,
    connection_name: String,
    instance_data_dir: std::path::PathBuf,
}

#[async_trait]
impl Node for PostgresRoot {
    fn id(&self) -> &str {
        "root"
    }

    fn label(&self) -> &str {
        &self.connection_name
    }

    fn node_type(&self) -> &NodeType {
        static T: std::sync::LazyLock<NodeType> = std::sync::LazyLock::new(root_node_type);
        &T
    }

    fn metadata(&self) -> &Metadata {
        static EMPTY: Metadata = Metadata { fields: vec![] };
        &EMPTY
    }

    fn children_types(&self) -> Vec<NodeType> {
        vec![database_node_type(), table_node_type(), script_node_type()]
    }

    async fn list(&self, params: ListParams) -> Result<ListResult> {
        match params.node_type.type_id.as_str() {
            "postgres:database" => {
                let entries = self
                    .client
                    .list_databases()
                    .await
                    .map_err(|e| ContentError::Other(e.into()))?;
                let items = entries
                    .into_iter()
                    .map(|e| NodeSummary {
                        id: e.name.clone(),
                        label: e.name.clone(),
                        node_type: database_node_type(),
                        metadata: database_metadata(&e),
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
            // Cross-DB / cross-schema flat list. IDs still use the
            // composite `<db>/schemas/<s>/tables/<t>` form so a future
            // drilldown to rows would resolve via the existing
            // `get_by_id` walker without needing a separate code path.
            "postgres:table" => {
                let entries = self
                    .client
                    .list_all_tables()
                    .await
                    .map_err(|e| ContentError::Other(e.into()))?;
                let items = entries
                    .into_iter()
                    .map(|e| NodeSummary {
                        id: format!(
                            "{}/{SCHEMAS_GROUP_ID}/{}/{TABLES_GROUP_ID}/{}",
                            e.database, e.schema, e.name
                        ),
                        label: e.name.clone(),
                        node_type: table_node_type(),
                        metadata: table_metadata(&e),
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
            // Filesystem-backed flat list of every saved SQL script.
            // Mixes two roots:
            //  • `<instance_data_dir>/queries/` (table-level, four path
            //    segments). ID `script/<db>/<schema>/<table>/<script>`.
            //  • `<instance_data_dir>/db_scripts/` (DB-level, two path
            //    segments). ID `db_script/<db>/<script>`.
            // DB-level scripts surface with empty schema/table metadata
            // fields so a single column set can render both kinds.
            "postgres:script" => {
                let table_scripts = crate::query::list_all_scripts(&self.instance_data_dir)
                    .await
                    .map_err(|e| ContentError::Other(Box::new(e)))?;
                let db_scripts = crate::query::list_all_db_scripts(&self.instance_data_dir)
                    .await
                    .map_err(|e| ContentError::Other(Box::new(e)))?;
                let mut items: Vec<NodeSummary> = table_scripts
                    .into_iter()
                    .map(|e| NodeSummary {
                        id: format!(
                            "script/{}/{}/{}/{}",
                            e.database, e.schema, e.table, e.script
                        ),
                        label: e.script.clone(),
                        node_type: script_node_type(),
                        metadata: script_metadata(&e),
                        has_children: None,
                    })
                    .collect();
                items.extend(db_scripts.into_iter().map(|e| NodeSummary {
                    id: format!("db_script/{}/{}", e.database, e.script),
                    label: e.script.clone(),
                    node_type: script_node_type(),
                    metadata: Metadata {
                        fields: vec![
                            field("script", "Script", &e.script),
                            field("database", "Database", &e.database),
                            // The legacy flat list shares a column set with
                            // `postgres:script`; leave schema/table empty for
                            // DB-level scripts so one renderer fits both.
                            field("schema", "Schema", ""),
                            field("table", "Table", ""),
                        ],
                    },
                    has_children: None,
                }));
                Ok(ListResult {
                    items,
                    applied_sort: Vec::new(),
                    page: None,
                    batch_download_available: false,
                    downloaded: vec![],
                })
            }
            other => Err(ContentError::NotSupported(format!(
                "unknown child type: {other}"
            ))),
        }
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        // Lazy construction: skip the `list_databases` round trip that the
        // old code used purely to populate owner/encoding metadata. During a
        // `get_by_id` walk through `<db>/...` the database node is only
        // traversed, never rendered (its NodeSummary comes from `root.list()`
        // with full metadata), so the cheap path is correct.
        Ok(Box::new(DatabaseNode::new(
            Arc::clone(&self.client),
            id.to_string(),
            self.instance_data_dir.clone(),
        )))
    }
}

// ---------------------------------------------------------------------------
// Database node — virtual children "Schemas" and "DB Scripts"
// ---------------------------------------------------------------------------

// Holds only the database name. Owner/encoding metadata is built by
// `root.list()` directly into `NodeSummary`, never read off this node
// during navigation — keeping the constructor free of a `list_databases`
// round trip on every `get_by_id` walk through a `<db>/...` path.
struct DatabaseNode {
    client: Arc<PostgresClient>,
    name: String,
    instance_data_dir: std::path::PathBuf,
}

impl DatabaseNode {
    fn new(
        client: Arc<PostgresClient>,
        name: String,
        instance_data_dir: std::path::PathBuf,
    ) -> Self {
        Self {
            client,
            name,
            instance_data_dir,
        }
    }
}

#[async_trait]
impl Node for DatabaseNode {
    fn id(&self) -> &str {
        &self.name
    }

    fn label(&self) -> &str {
        &self.name
    }

    fn node_type(&self) -> &NodeType {
        static T: std::sync::LazyLock<NodeType> = std::sync::LazyLock::new(database_node_type);
        &T
    }

    fn metadata(&self) -> &Metadata {
        static EMPTY: Metadata = Metadata { fields: vec![] };
        &EMPTY
    }

    fn children_types(&self) -> Vec<NodeType> {
        vec![
            schemas_group_node_type(),
            schema_node_type(),
            db_scripts_group_node_type(),
            db_script_dir_node_type(),
            db_script_node_type(),
        ]
    }

    async fn list(&self, params: ListParams) -> Result<ListResult> {
        match params.node_type.type_id.as_str() {
            // Original DBeaver-style group: a single virtual "Schemas"
            // folder. Kept for views that want a place to attach
            // sibling groups (Views, Functions, …).
            "postgres:schemas" => Ok(ListResult {
                items: vec![NodeSummary {
                    id: format!("{}/{}", self.name, SCHEMAS_GROUP_ID),
                    label: "Schemas".into(),
                    node_type: schemas_group_node_type(),
                    metadata: Metadata { fields: vec![] },
                    has_children: None,
                }],
                applied_sort: Vec::new(),
                page: None,
                batch_download_available: false,
                downloaded: vec![],
            }),
            // Direct shortcut: skip the group node so a view can drill
            // database → schema in one step. Composite IDs still go
            // through the group segment so `get_by_id`'s walker stays
            // unchanged.
            "postgres:schema" => {
                let entries = self
                    .client
                    .list_schemas(&self.name)
                    .await
                    .map_err(|e| ContentError::Other(e.into()))?;
                let db = &self.name;
                let items = entries
                    .into_iter()
                    .map(|e| NodeSummary {
                        id: format!("{db}/{SCHEMAS_GROUP_ID}/{}", e.name),
                        label: e.name.clone(),
                        node_type: schema_node_type(),
                        metadata: schema_metadata(&e),
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
            // Sibling group node, lives next to "Schemas". Stable place
            // to hang the per-database script directory.
            "postgres:db_scripts" => Ok(ListResult {
                items: vec![NodeSummary {
                    id: format!("{}/{}", self.name, DB_SCRIPTS_GROUP_ID),
                    label: "DB Scripts".into(),
                    node_type: db_scripts_group_node_type(),
                    metadata: Metadata { fields: vec![] },
                    has_children: None,
                }],
                applied_sort: Vec::new(),
                page: None,
                batch_download_available: false,
                downloaded: vec![],
            }),
            // Direct shortcut analogous to `postgres:schema`: skips the
            // group node so a view can drill database → db_script in one
            // step. Composite IDs still embed the group segment so the
            // `get_by_id` walker resolves both routes.
            "postgres:db_script" => {
                let scripts = crate::query::list_db_scripts_in_database(
                    &self.instance_data_dir,
                    &self.name,
                )
                .await
                .map_err(|e| ContentError::Other(Box::new(e)))?;
                let db = &self.name;
                let items = scripts
                    .into_iter()
                    .map(|script| NodeSummary {
                        id: format!("{db}/{DB_SCRIPTS_GROUP_ID}/{script}"),
                        label: script.clone(),
                        node_type: db_script_node_type(),
                        metadata: db_script_metadata(db, &script),
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
            other => Err(ContentError::NotSupported(format!(
                "unknown child type: {other}"
            ))),
        }
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        match id {
            SCHEMAS_GROUP_ID => Ok(Box::new(SchemasGroupNode {
                client: Arc::clone(&self.client),
                database: self.name.clone(),
            })),
            DB_SCRIPTS_GROUP_ID => Ok(Box::new(DbScriptsGroupNode {
                database: self.name.clone(),
                instance_data_dir: self.instance_data_dir.clone(),
            })),
            other => Err(ContentError::NotFound(other.into())),
        }
    }
}

const SCHEMAS_GROUP_ID: &str = "schemas";
const TABLES_GROUP_ID: &str = "tables";
const DB_SCRIPTS_GROUP_ID: &str = "db_scripts";

// ---------------------------------------------------------------------------
// "Schemas" group — children are individual schemas
// ---------------------------------------------------------------------------

struct SchemasGroupNode {
    client: Arc<PostgresClient>,
    database: String,
}

#[async_trait]
impl Node for SchemasGroupNode {
    fn id(&self) -> &str {
        SCHEMAS_GROUP_ID
    }

    fn label(&self) -> &str {
        "Schemas"
    }

    fn node_type(&self) -> &NodeType {
        static T: std::sync::LazyLock<NodeType> = std::sync::LazyLock::new(schemas_group_node_type);
        &T
    }

    fn metadata(&self) -> &Metadata {
        static EMPTY: Metadata = Metadata { fields: vec![] };
        &EMPTY
    }

    fn children_types(&self) -> Vec<NodeType> {
        vec![schema_node_type()]
    }

    async fn list(&self, params: ListParams) -> Result<ListResult> {
        if params.node_type.type_id != "postgres:schema" {
            return Err(ContentError::NotSupported(format!(
                "unknown child type: {}",
                params.node_type.type_id
            )));
        }
        let entries = self
            .client
            .list_schemas(&self.database)
            .await
            .map_err(|e| ContentError::Other(e.into()))?;
        let db = &self.database;
        let items = entries
            .into_iter()
            .map(|e| NodeSummary {
                id: format!("{db}/{SCHEMAS_GROUP_ID}/{}", e.name),
                label: e.name.clone(),
                node_type: schema_node_type(),
                metadata: schema_metadata(&e),
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

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        // Lazy: see comment on `PostgresAdapter::get_child`.
        Ok(Box::new(SchemaNode::new(
            Arc::clone(&self.client),
            self.database.clone(),
            id.to_string(),
        )))
    }
}

// ---------------------------------------------------------------------------
// "DB Scripts" group — children are individual DB-level scripts loaded
// from `<instance_data_dir>/db_scripts/<database>/<script>.sql`. Sits
// alongside the `Schemas` branch so a view can mix both at the
// database level (multi-tree-continuation, MT-1).
// ---------------------------------------------------------------------------

struct DbScriptsGroupNode {
    database: String,
    instance_data_dir: std::path::PathBuf,
}

#[async_trait]
impl Node for DbScriptsGroupNode {
    fn id(&self) -> &str {
        DB_SCRIPTS_GROUP_ID
    }

    fn label(&self) -> &str {
        "DB Scripts"
    }

    fn node_type(&self) -> &NodeType {
        static T: std::sync::LazyLock<NodeType> =
            std::sync::LazyLock::new(db_scripts_group_node_type);
        &T
    }

    fn metadata(&self) -> &Metadata {
        static EMPTY: Metadata = Metadata { fields: vec![] };
        &EMPTY
    }

    fn children_types(&self) -> Vec<NodeType> {
        vec![db_script_dir_node_type(), db_script_node_type()]
    }

    async fn list(&self, params: ListParams) -> Result<ListResult> {
        // After DSF-1/DSF-2 the group lists BOTH folders and scripts;
        // a YAML view binds either child type to its own ChildDef and
        // the filter below decides which set surfaces in this call.
        let want_dirs = params.node_type.type_id == "postgres:db_script_dir";
        let want_scripts = params.node_type.type_id == "postgres:db_script";
        if !want_dirs && !want_scripts {
            return Err(ContentError::NotSupported(format!(
                "unknown child type: {}",
                params.node_type.type_id
            )));
        }
        let entries = crate::query::list_db_script_entries(
            &self.instance_data_dir,
            &self.database,
            std::path::Path::new(""),
        )
        .await
        .map_err(|e| ContentError::Other(Box::new(e)))?;
        let db = &self.database;
        let mut items = Vec::new();
        for e in entries {
            let name = e.name().to_string();
            match e {
                crate::query::DbScriptTreeEntry::Dir { .. } if want_dirs => {
                    items.push(NodeSummary {
                        id: format!("{db}/{DB_SCRIPTS_GROUP_ID}/{name}"),
                        label: name.clone(),
                        node_type: db_script_dir_node_type(),
                        metadata: db_script_dir_metadata(db, &name),
                        has_children: None,
                    });
                }
                crate::query::DbScriptTreeEntry::Script { .. } if want_scripts => {
                    items.push(NodeSummary {
                        id: format!("{db}/{DB_SCRIPTS_GROUP_ID}/{name}"),
                        label: name.clone(),
                        node_type: db_script_node_type(),
                        metadata: db_script_metadata(db, &name),
                        has_children: None,
                    });
                }
                _ => {}
            }
        }
        Ok(ListResult {
            items,
            applied_sort: Vec::new(),
            page: None,
            batch_download_available: false,
            downloaded: vec![],
        })
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        // Disambiguate dir vs script with a filesystem probe — same
        // strategy `DbScriptDirNode::get_child` uses, kept identical so
        // both root-flat and nested resolutions behave the same. Dir
        // probe wins on collision (shouldn't happen because mkdir/touch
        // would refuse anyway, but order matters for correctness).
        let child_rel = std::path::PathBuf::from(id);
        let dir_abs =
            crate::query::db_script_dir_path(&self.instance_data_dir, &self.database, &child_rel);
        if tokio::fs::metadata(&dir_abs).await.map(|m| m.is_dir()).unwrap_or(false) {
            return Ok(Box::new(DbScriptDirNode {
                database: self.database.clone(),
                rel_path: child_rel.to_string_lossy().into_owned(),
                name: id.to_string(),
                instance_data_dir: self.instance_data_dir.clone(),
                metadata: db_script_dir_metadata(&self.database, id),
            }));
        }
        let file_abs =
            crate::query::db_script_path(&self.instance_data_dir, &self.database, &child_rel);
        if tokio::fs::metadata(&file_abs).await.map(|m| m.is_file()).unwrap_or(false) {
            return Ok(Box::new(DbScriptNode {
                database: self.database.clone(),
                rel_path: child_rel.to_string_lossy().into_owned(),
                name: id.to_string(),
                instance_data_dir: self.instance_data_dir.clone(),
                metadata: db_script_metadata(&self.database, id),
            }));
        }
        Err(ContentError::NotFound(format!("db script or folder {id}")))
    }

    fn actions(&self) -> Vec<NodeAction> {
        db_scripts_group_actions()
    }

    async fn invoke_action(
        &self,
        name: &str,
        _ctx: &ActionContext,
    ) -> Result<ActionDispatch> {
        match name {
            // `hint` carries the database (and empty parent_rel) for the
            // TUI's cmdline prompt — see `dispatch_to_view_request` for
            // the CreateChild arm. The legacy "add" alias keeps any
            // out-of-tree caller working through one release.
            "add-script" | "add" => Ok(ActionDispatch::CreateChild {
                hint: format!("db_script:{}", self.database),
            }),
            "add-dir" => Ok(ActionDispatch::CreateChild {
                hint: format!("db_script_dir:{}", self.database),
            }),
            other => Err(ContentError::NotSupported(format!(
                "db_scripts group action '{other}' is not supported"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// DbScriptDirNode — a folder inside the DB-Scripts tree. Holds its full
// rel_path from the database root so list/get_child can compose absolute
// node ids (`<db>/db_scripts/<rel_path>/<seg>`). Empty rel_path is the
// database root and is represented by [`DbScriptsGroupNode`] instead —
// keeping the two distinct lets the group node carry its own action set.
// ---------------------------------------------------------------------------

struct DbScriptDirNode {
    database: String,
    /// Full path relative to `db_scripts/<db>/`, joined with `/`. Never
    /// empty (root is handled by [`DbScriptsGroupNode`]).
    rel_path: String,
    /// Last segment of `rel_path`; the segment walker hands this to
    /// `get_child` so we keep a copy for [`Node::id`] without a string
    /// scan on every poll.
    name: String,
    instance_data_dir: std::path::PathBuf,
    metadata: Metadata,
}

#[async_trait]
impl Node for DbScriptDirNode {
    fn id(&self) -> &str {
        &self.name
    }

    fn label(&self) -> &str {
        &self.name
    }

    fn node_type(&self) -> &NodeType {
        static T: std::sync::LazyLock<NodeType> =
            std::sync::LazyLock::new(db_script_dir_node_type);
        &T
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn children_types(&self) -> Vec<NodeType> {
        vec![db_script_dir_node_type(), db_script_node_type()]
    }

    async fn list(&self, params: ListParams) -> Result<ListResult> {
        let want_dirs = params.node_type.type_id == "postgres:db_script_dir";
        let want_scripts = params.node_type.type_id == "postgres:db_script";
        if !want_dirs && !want_scripts {
            return Err(ContentError::NotSupported(format!(
                "unknown child type: {}",
                params.node_type.type_id
            )));
        }
        let entries = crate::query::list_db_script_entries(
            &self.instance_data_dir,
            &self.database,
            std::path::Path::new(&self.rel_path),
        )
        .await
        .map_err(|e| ContentError::Other(Box::new(e)))?;
        let db = &self.database;
        let prefix = &self.rel_path;
        let mut items = Vec::new();
        for e in entries {
            let name = e.name().to_string();
            match e {
                crate::query::DbScriptTreeEntry::Dir { .. } if want_dirs => {
                    items.push(NodeSummary {
                        id: format!("{db}/{DB_SCRIPTS_GROUP_ID}/{prefix}/{name}"),
                        label: name.clone(),
                        node_type: db_script_dir_node_type(),
                        metadata: db_script_dir_metadata(db, &name),
                        has_children: None,
                    });
                }
                crate::query::DbScriptTreeEntry::Script { .. } if want_scripts => {
                    items.push(NodeSummary {
                        id: format!("{db}/{DB_SCRIPTS_GROUP_ID}/{prefix}/{name}"),
                        label: name.clone(),
                        node_type: db_script_node_type(),
                        metadata: db_script_metadata(db, &name),
                        has_children: None,
                    });
                }
                _ => {}
            }
        }
        Ok(ListResult {
            items,
            applied_sort: Vec::new(),
            page: None,
            batch_download_available: false,
            downloaded: vec![],
        })
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        let child_rel = std::path::PathBuf::from(&self.rel_path).join(id);
        let dir_abs =
            crate::query::db_script_dir_path(&self.instance_data_dir, &self.database, &child_rel);
        if tokio::fs::metadata(&dir_abs).await.map(|m| m.is_dir()).unwrap_or(false) {
            return Ok(Box::new(DbScriptDirNode {
                database: self.database.clone(),
                rel_path: child_rel.to_string_lossy().into_owned(),
                name: id.to_string(),
                instance_data_dir: self.instance_data_dir.clone(),
                metadata: db_script_dir_metadata(&self.database, id),
            }));
        }
        let file_abs =
            crate::query::db_script_path(&self.instance_data_dir, &self.database, &child_rel);
        if tokio::fs::metadata(&file_abs).await.map(|m| m.is_file()).unwrap_or(false) {
            return Ok(Box::new(DbScriptNode {
                database: self.database.clone(),
                rel_path: child_rel.to_string_lossy().into_owned(),
                name: id.to_string(),
                instance_data_dir: self.instance_data_dir.clone(),
                metadata: db_script_metadata(&self.database, id),
            }));
        }
        Err(ContentError::NotFound(format!("db script or folder {id}")))
    }

    fn actions(&self) -> Vec<NodeAction> {
        db_script_dir_actions()
    }

    async fn invoke_action(
        &self,
        name: &str,
        _ctx: &ActionContext,
    ) -> Result<ActionDispatch> {
        match name {
            // `db_script:<db>:<parent_rel>` lets the TUI prompt scope to
            // this folder and pre-fill the parent path.
            "add-script" => Ok(ActionDispatch::CreateChild {
                hint: format!("db_script:{}:{}", self.database, self.rel_path),
            }),
            "add-dir" => Ok(ActionDispatch::CreateChild {
                hint: format!("db_script_dir:{}:{}", self.database, self.rel_path),
            }),
            // Empty-check happens inside the TUI handler which calls
            // `delete_db_script_dir` — surface the not-empty error via
            // Notify in DSF-4. Returning DeleteSelf keeps the same shape
            // as the script-leaf delete; the TUI tells them apart by the
            // node_type before picking which confirm popup to show.
            "delete-dir" => Ok(ActionDispatch::DeleteSelf { confirm: None }),
            // rename / mark-move / paste-move are pure TUI flows: the
            // adapter has no work to do until the user supplies the new
            // name (rename) or pastes (paste-move). DSF-4 wires the
            // dispatcher on action name + node_type.
            "rename" | "mark-move" | "paste-move" => Ok(ActionDispatch::Noop),
            other => Err(ContentError::NotSupported(format!(
                "db_script_dir action '{other}' is not supported"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Leaf node for a single DB-level script. The actual SQL body lives in
// the filesystem; load/save go through
// [`PostgresAdapter::load_db_script_file`] / `save_db_script_file`.
// ---------------------------------------------------------------------------

struct DbScriptNode {
    database: String,
    /// Full path relative to `db_scripts/<db>/`, joined with `/`. For a
    /// flat root script this equals `name`; for a nested script it looks
    /// like `util/audit`. The on-disk file is `<rel_path>.sql` —
    /// [`crate::query::db_script_path`] does the append.
    rel_path: String,
    /// Last segment of `rel_path`. Returned by [`Node::id`] /
    /// [`Node::label`] so the row label stays compact in tree views.
    name: String,
    /// Needed so [`Self::invoke_action`] can read the on-disk script body
    /// without an adapter handle. Populated by
    /// [`DbScriptsGroupNode::get_child`] / [`DbScriptDirNode::get_child`].
    instance_data_dir: std::path::PathBuf,
    metadata: Metadata,
}

#[async_trait]
impl Node for DbScriptNode {
    fn id(&self) -> &str {
        &self.name
    }

    fn label(&self) -> &str {
        &self.name
    }

    fn node_type(&self) -> &NodeType {
        static T: std::sync::LazyLock<NodeType> = std::sync::LazyLock::new(db_script_node_type);
        &T
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn children_types(&self) -> Vec<NodeType> {
        vec![]
    }

    fn actions(&self) -> Vec<NodeAction> {
        db_script_actions()
    }

    async fn invoke_action(
        &self,
        name: &str,
        _ctx: &ActionContext,
    ) -> Result<ActionDispatch> {
        match name {
            "execute" => {
                let body = crate::query::read_db_script(
                    &self.instance_data_dir,
                    &self.database,
                    &self.rel_path,
                )
                .await
                .map_err(|e| ContentError::Other(Box::new(e)))?;
                let sql = crate::query::parse_query_area(&body).trim().to_string();
                if sql.is_empty() {
                    return Ok(ActionDispatch::Error(format!(
                        "script '{}' has no SQL below the marker",
                        self.rel_path
                    )));
                }
                Ok(ActionDispatch::ExecuteQuery {
                    database: self.database.clone(),
                    sql,
                    paged: true,
                })
            }
            "edit" => Ok(ActionDispatch::OpenEditor {
                session_kind: "postgres_db_script".into(),
                // `script` carries the FULL rel_path (may contain `/`).
                // `PostgresDbScriptSession` resolves the on-disk file via
                // `db_script_file_path(..., &script)`, which already
                // accepts slashes (it's a `PathBuf::join`).
                params: std::collections::HashMap::from([
                    ("database".into(), self.database.clone()),
                    ("script".into(), self.rel_path.clone()),
                ]),
            }),
            "delete" => Ok(ActionDispatch::DeleteSelf { confirm: None }),
            // TUI-owned flows; DSF-4 inspects action name + node_type and
            // emits the right ViewRequest. Adapter has no work here.
            "rename" | "mark-move" => Ok(ActionDispatch::Noop),
            other => Err(ContentError::NotSupported(format!(
                "db_script action '{other}' is not supported"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Schema node — single virtual child "Tables"
// ---------------------------------------------------------------------------

// Holds only the schema name; same rationale as [`DatabaseNode`].
struct SchemaNode {
    client: Arc<PostgresClient>,
    database: String,
    name: String,
}

impl SchemaNode {
    fn new(client: Arc<PostgresClient>, database: String, name: String) -> Self {
        Self {
            client,
            database,
            name,
        }
    }
}

#[async_trait]
impl Node for SchemaNode {
    fn id(&self) -> &str {
        &self.name
    }

    fn label(&self) -> &str {
        &self.name
    }

    fn node_type(&self) -> &NodeType {
        static T: std::sync::LazyLock<NodeType> = std::sync::LazyLock::new(schema_node_type);
        &T
    }

    fn metadata(&self) -> &Metadata {
        static EMPTY: Metadata = Metadata { fields: vec![] };
        &EMPTY
    }

    fn children_types(&self) -> Vec<NodeType> {
        vec![tables_group_node_type(), table_node_type()]
    }

    async fn list(&self, params: ListParams) -> Result<ListResult> {
        match params.node_type.type_id.as_str() {
            "postgres:tables" => Ok(ListResult {
                items: vec![NodeSummary {
                    id: format!(
                        "{}/{SCHEMAS_GROUP_ID}/{}/{TABLES_GROUP_ID}",
                        self.database, self.name
                    ),
                    label: "Tables".into(),
                    node_type: tables_group_node_type(),
                    metadata: Metadata { fields: vec![] },
                    has_children: None,
                }],
                applied_sort: Vec::new(),
                page: None,
                batch_download_available: false,
                downloaded: vec![],
            }),
            "postgres:table" => {
                let entries = self
                    .client
                    .list_tables(&self.database, &self.name)
                    .await
                    .map_err(|e| ContentError::Other(e.into()))?;
                let db = &self.database;
                let schema = &self.name;
                let items = entries
                    .into_iter()
                    .map(|e| NodeSummary {
                        id: format!(
                            "{db}/{SCHEMAS_GROUP_ID}/{schema}/{TABLES_GROUP_ID}/{}",
                            e.name
                        ),
                        label: e.name.clone(),
                        node_type: table_node_type(),
                        metadata: table_metadata(&e),
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
            other => Err(ContentError::NotSupported(format!(
                "unknown child type: {other}"
            ))),
        }
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        if id != TABLES_GROUP_ID {
            return Err(ContentError::NotFound(id.into()));
        }
        Ok(Box::new(TablesGroupNode {
            client: Arc::clone(&self.client),
            database: self.database.clone(),
            schema: self.name.clone(),
        }))
    }
}

// ---------------------------------------------------------------------------
// "Tables" group — children are individual tables
// ---------------------------------------------------------------------------

struct TablesGroupNode {
    client: Arc<PostgresClient>,
    database: String,
    schema: String,
}

#[async_trait]
impl Node for TablesGroupNode {
    fn id(&self) -> &str {
        TABLES_GROUP_ID
    }

    fn label(&self) -> &str {
        "Tables"
    }

    fn node_type(&self) -> &NodeType {
        static T: std::sync::LazyLock<NodeType> = std::sync::LazyLock::new(tables_group_node_type);
        &T
    }

    fn metadata(&self) -> &Metadata {
        static EMPTY: Metadata = Metadata { fields: vec![] };
        &EMPTY
    }

    fn children_types(&self) -> Vec<NodeType> {
        vec![table_node_type()]
    }

    async fn list(&self, params: ListParams) -> Result<ListResult> {
        if params.node_type.type_id != "postgres:table" {
            return Err(ContentError::NotSupported(format!(
                "unknown child type: {}",
                params.node_type.type_id
            )));
        }
        let entries = self
            .client
            .list_tables(&self.database, &self.schema)
            .await
            .map_err(|e| ContentError::Other(e.into()))?;
        let db = &self.database;
        let schema = &self.schema;
        let items = entries
            .into_iter()
            .map(|e| NodeSummary {
                id: format!(
                    "{db}/{SCHEMAS_GROUP_ID}/{schema}/{TABLES_GROUP_ID}/{}",
                    e.name
                ),
                label: e.name.clone(),
                node_type: table_node_type(),
                metadata: table_metadata(&e),
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

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        // Lazy: see comment on `PostgresAdapter::get_child`.
        Ok(Box::new(TableNode::new(
            Arc::clone(&self.client),
            self.database.clone(),
            self.schema.clone(),
            id.to_string(),
        )))
    }
}

// ---------------------------------------------------------------------------
// Table node — children are individual data rows (postgres:row), one
// per row of a paginated `SELECT * FROM schema.table`. Each row's
// metadata exposes the column→value pairs so the TUI can render them
// as table columns.
// ---------------------------------------------------------------------------

// Holds only the table name; same rationale as [`DatabaseNode`].
struct TableNode {
    client: Arc<PostgresClient>,
    database: String,
    schema: String,
    name: String,
}

impl TableNode {
    fn new(
        client: Arc<PostgresClient>,
        database: String,
        schema: String,
        name: String,
    ) -> Self {
        Self {
            client,
            database,
            schema,
            name,
        }
    }
}

#[async_trait]
impl Node for TableNode {
    fn id(&self) -> &str {
        &self.name
    }

    fn label(&self) -> &str {
        &self.name
    }

    fn node_type(&self) -> &NodeType {
        static T: std::sync::LazyLock<NodeType> = std::sync::LazyLock::new(table_node_type);
        &T
    }

    fn metadata(&self) -> &Metadata {
        static EMPTY: Metadata = Metadata { fields: vec![] };
        &EMPTY
    }

    fn children_types(&self) -> Vec<NodeType> {
        vec![row_node_type()]
    }

    fn actions(&self) -> Vec<NodeAction> {
        table_actions()
    }

    async fn invoke_action(
        &self,
        name: &str,
        _ctx: &ActionContext,
    ) -> Result<ActionDispatch> {
        match name {
            "edit_sql" => Ok(ActionDispatch::OpenEditor {
                session_kind: "postgres_query".into(),
                // Params are informational — the TUI uses `node_id`
                // (the TableNode's path id) to address the editor.
                // We pass the parts back too so future session_kinds
                // could read them without re-parsing.
                params: std::collections::HashMap::from([
                    ("database".into(), self.database.clone()),
                    ("schema".into(), self.schema.clone()),
                    ("table".into(), self.name.clone()),
                ]),
            }),
            other => Err(ContentError::NotSupported(format!(
                "table node action '{other}' is not supported"
            ))),
        }
    }

    async fn list(&self, params: ListParams) -> Result<ListResult> {
        if params.node_type.type_id != "postgres:row" {
            return Err(ContentError::NotSupported(format!(
                "unknown child type: {}",
                params.node_type.type_id
            )));
        }
        let (offset, limit) = match params.page {
            Some(p) if p.limit > 0 => (p.offset, p.limit),
            _ => (0, 100),
        };
        let page = self
            .client
            .query_rows(&self.database, &self.schema, &self.name, offset, limit)
            .await
            .map_err(|e| ContentError::Other(e.into()))?;

        let id_prefix = format!(
            "{}/{SCHEMAS_GROUP_ID}/{}/{TABLES_GROUP_ID}/{}",
            self.database, self.schema, self.name,
        );
        let items = page
            .rows
            .into_iter()
            .enumerate()
            .map(|(i, row)| {
                let row_offset = offset as u64 + i as u64;
                let fields = page
                    .columns
                    .iter()
                    .zip(row.iter())
                    .map(|(col, val)| MetadataField {
                        key: col.clone(),
                        value: val.clone().unwrap_or_else(|| "(null)".into()),
                        display_label: col.clone(),
                        editable: false,
                        allowed_values: None,
                    })
                    .collect();
                NodeSummary {
                    id: format!("{id_prefix}/rows/{row_offset}"),
                    label: format!("row {row_offset}"),
                    node_type: row_node_type(),
                    metadata: Metadata { fields },
                    has_children: None,
                }
            })
            .collect();

        Ok(ListResult {
            items,
            applied_sort: Vec::new(),
            page: Some(PageInfo {
                offset,
                limit,
                total: None,
                has_next: page.has_more,
                has_prev: offset > 0,
            }),
            batch_download_available: false,
            downloaded: vec![],
        })
    }
}

#[cfg(test)]
mod pagination_tests {
    use super::*;

    fn page(offset: u32, limit: u32) -> PageRequest {
        PageRequest { offset, limit }
    }

    #[test]
    fn wraps_simple_select() {
        let got = wrap_for_pagination("SELECT * FROM t", page(0, 100)).unwrap();
        assert_eq!(got, "SELECT * FROM (SELECT * FROM t) AS _nyd_pg LIMIT 101 OFFSET 0");
    }

    #[test]
    fn wraps_select_with_quoted_identifiers() {
        let q = r#"SELECT * FROM "public"."01_Sample_Item";"#;
        let got = wrap_for_pagination(q, page(0, 100));
        assert!(got.is_some(), "quoted identifiers must not block pagination");
        let wrapped = got.unwrap();
        assert!(wrapped.contains("LIMIT 101"));
        assert!(wrapped.contains("OFFSET 0"));
        assert!(wrapped.contains(r#""public"."01_Sample_Item""#));
    }

    #[test]
    fn wraps_select_with_trailing_semicolon() {
        let got = wrap_for_pagination("SELECT 1;", page(0, 50)).unwrap();
        assert_eq!(got, "SELECT * FROM (SELECT 1) AS _nyd_pg LIMIT 51 OFFSET 0");
    }

    #[test]
    fn wraps_lowercase_select_and_with() {
        assert!(wrap_for_pagination("select 1", page(0, 10)).is_some());
        assert!(wrap_for_pagination("WITH x AS (SELECT 1) SELECT * FROM x", page(0, 10)).is_some());
        assert!(wrap_for_pagination("with x as (select 1) select * from x", page(0, 10)).is_some());
    }

    #[test]
    fn skips_non_select() {
        assert!(wrap_for_pagination("UPDATE t SET x = 1", page(0, 100)).is_none());
        assert!(wrap_for_pagination("INSERT INTO t VALUES (1)", page(0, 100)).is_none());
        assert!(wrap_for_pagination("DELETE FROM t", page(0, 100)).is_none());
        assert!(wrap_for_pagination("CREATE TABLE x ()", page(0, 100)).is_none());
    }

    #[test]
    fn skips_multi_statement() {
        assert!(wrap_for_pagination("SELECT 1; SELECT 2", page(0, 100)).is_none());
        assert!(wrap_for_pagination("UPDATE t SET x = 1; SELECT 1", page(0, 100)).is_none());
    }

    #[test]
    fn semicolon_inside_string_does_not_count_as_multi_statement() {
        let got = wrap_for_pagination("SELECT ';' FROM t", page(0, 100));
        assert!(got.is_some(), "single-statement SELECT with quoted ; should wrap");
    }

    #[test]
    fn semicolon_inside_line_comment_does_not_count() {
        let q = "SELECT 1 -- trailing ; comment\nFROM t";
        assert!(wrap_for_pagination(q, page(0, 100)).is_some());
    }

    #[test]
    fn semicolon_inside_block_comment_does_not_count() {
        let q = "SELECT 1 /* split ; here */ FROM t";
        assert!(wrap_for_pagination(q, page(0, 100)).is_some());
    }

    #[test]
    fn leading_comment_and_whitespace_still_detects_select() {
        let q = "-- explain\n  /* why */  SELECT 1";
        assert!(wrap_for_pagination(q, page(0, 100)).is_some());
    }

    #[test]
    fn offset_propagates() {
        let got = wrap_for_pagination("SELECT 1", page(200, 100)).unwrap();
        assert!(got.contains("OFFSET 200"));
        assert!(got.contains("LIMIT 101"));
    }

    #[test]
    fn word_boundary_rejects_selectively_named_identifiers() {
        // A column named `selectish` (or similar) at the start is
        // nonsensical SQL but `looks_like_select_or_with` should not
        // be tripped up by partial keyword matches.
        use crate::client::sql_shape::looks_like_select_or_with;
        assert!(!looks_like_select_or_with("selectish 1"));
        assert!(!looks_like_select_or_with("within scope"));
    }
}

/// DSF-2 tests: exercise the DB-script tree at the Node trait level
/// (no Postgres client required — the group node only needs an
/// `instance_data_dir` for its file ops).
#[cfg(test)]
mod db_script_tree_tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn unique_tmpdir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!(
            "nyd-adapter-test-{nanos}-{n}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn list_params(type_id: &str) -> ListParams {
        ListParams {
            node_type: NodeType {
                type_id: type_id.into(),
                mime_type: "".into(),
                syntax: None,
                file_extension: "".into(),
                display_name: "".into(),
            },
            query: None,
            sort: Vec::new(),
            page: None,
            download: false,
            group_by: None,
        }
    }

    async fn build_group(database: &str, instance: &Path) -> DbScriptsGroupNode {
        DbScriptsGroupNode {
            database: database.to_string(),
            instance_data_dir: instance.to_path_buf(),
        }
    }

    #[tokio::test]
    async fn group_lists_dirs_and_scripts_separately() {
        let tmp = unique_tmpdir();
        // Layout: root has a folder `util`, a SQL script, and a Python
        // script — the second one verifies that the listing is no longer
        // gated on the `.sql` extension.
        crate::query::create_db_script_dir(&tmp, "mydb", Path::new("util")).await.unwrap();
        crate::query::write_db_script(&tmp, "mydb", "audit.sql", "SELECT 1;").await.unwrap();
        crate::query::write_db_script(&tmp, "mydb", "migrate.py", "print('hi')").await.unwrap();
        let g = build_group("mydb", &tmp).await;

        let dirs = g.list(list_params("postgres:db_script_dir")).await.unwrap();
        let dir_names: Vec<&str> = dirs.items.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(dir_names, vec!["util"]);
        assert_eq!(dirs.items[0].node_type.type_id, "postgres:db_script_dir");

        let scripts = g.list(list_params("postgres:db_script")).await.unwrap();
        let script_names: Vec<&str> = scripts.items.iter().map(|n| n.label.as_str()).collect();
        // Labels carry the extension so the user sees what type each
        // file is at a glance.
        assert_eq!(script_names, vec!["audit.sql", "migrate.py"]);
        assert_eq!(scripts.items[0].node_type.type_id, "postgres:db_script");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn group_get_child_resolves_dir_vs_script() {
        let tmp = unique_tmpdir();
        crate::query::create_db_script_dir(&tmp, "mydb", Path::new("util")).await.unwrap();
        crate::query::write_db_script(&tmp, "mydb", "audit.sql", "SELECT 1;").await.unwrap();
        let g = build_group("mydb", &tmp).await;

        let dir_node = g.get_child("util").await.unwrap();
        assert_eq!(dir_node.node_type().type_id, "postgres:db_script_dir");
        // get_child takes the rel-path segment including extension —
        // that's how it disambiguates from a folder of the same stem.
        let script_node = g.get_child("audit.sql").await.unwrap();
        assert_eq!(script_node.node_type().type_id, "postgres:db_script");
        assert!(g.get_child("nope").await.is_err());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn dir_node_lists_nested_children_with_full_ids() {
        let tmp = unique_tmpdir();
        crate::query::create_db_script_dir(&tmp, "mydb", Path::new("util/inner")).await.unwrap();
        crate::query::write_db_script(&tmp, "mydb", "util/helper.sql", "SELECT 1;").await.unwrap();
        let g = build_group("mydb", &tmp).await;
        let dir_node_box = g.get_child("util").await.unwrap();
        // get_child returns Box<dyn Node>; the runtime type is DbScriptDirNode
        // but we exercise it via the Node trait.

        let scripts = dir_node_box.list(list_params("postgres:db_script")).await.unwrap();
        assert_eq!(scripts.items.len(), 1);
        assert_eq!(scripts.items[0].label, "helper.sql");
        // Node id encodes the full path so the segment walker can later
        // resolve it back via root → database → db_scripts → util → helper.sql.
        assert_eq!(scripts.items[0].id, "mydb/db_scripts/util/helper.sql");

        let dirs = dir_node_box.list(list_params("postgres:db_script_dir")).await.unwrap();
        assert_eq!(dirs.items.len(), 1);
        assert_eq!(dirs.items[0].label, "inner");
        assert_eq!(dirs.items[0].id, "mydb/db_scripts/util/inner");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn group_action_set_includes_add_script_and_add_dir() {
        let tmp = unique_tmpdir();
        let g = build_group("mydb", &tmp).await;
        let names: Vec<String> = g.actions().into_iter().map(|a| a.id).collect();
        assert!(names.iter().any(|n| n == "add-script"));
        assert!(names.iter().any(|n| n == "add-dir"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn dir_action_set_covers_full_set() {
        let dir = DbScriptDirNode {
            database: "mydb".into(),
            rel_path: "util".into(),
            name: "util".into(),
            instance_data_dir: unique_tmpdir(),
            metadata: db_script_dir_metadata("mydb", "util"),
        };
        let names: Vec<String> = dir.actions().into_iter().map(|a| a.id).collect();
        for want in ["add-script", "add-dir", "rename", "mark-move", "paste-move", "delete-dir"] {
            assert!(names.iter().any(|n| n == want), "missing action: {want}");
        }
        let _ = std::fs::remove_dir_all(&dir.instance_data_dir);
    }

    #[tokio::test]
    async fn leaf_action_set_extended_with_rename_and_mark_move() {
        let leaf = DbScriptNode {
            database: "mydb".into(),
            rel_path: "util/audit".into(),
            name: "audit".into(),
            instance_data_dir: unique_tmpdir(),
            metadata: db_script_metadata("mydb", "audit"),
        };
        let names: Vec<String> = leaf.actions().into_iter().map(|a| a.id).collect();
        for want in ["execute", "edit", "rename", "mark-move", "delete"] {
            assert!(names.iter().any(|n| n == want), "missing action: {want}");
        }
        let _ = std::fs::remove_dir_all(&leaf.instance_data_dir);
    }

    #[tokio::test]
    async fn dir_add_actions_emit_create_child_with_parent_rel_in_hint() {
        let dir = DbScriptDirNode {
            database: "mydb".into(),
            rel_path: "util/inner".into(),
            name: "inner".into(),
            instance_data_dir: unique_tmpdir(),
            metadata: db_script_dir_metadata("mydb", "inner"),
        };
        let ctx = ActionContext::default();
        match dir.invoke_action("add-script", &ctx).await.unwrap() {
            ActionDispatch::CreateChild { hint } => {
                assert_eq!(hint, "db_script:mydb:util/inner");
            }
            other => panic!("expected CreateChild, got {other:?}"),
        }
        match dir.invoke_action("add-dir", &ctx).await.unwrap() {
            ActionDispatch::CreateChild { hint } => {
                assert_eq!(hint, "db_script_dir:mydb:util/inner");
            }
            other => panic!("expected CreateChild, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir.instance_data_dir);
    }

    #[tokio::test]
    async fn leaf_edit_action_passes_full_rel_path_in_script_param() {
        let leaf = DbScriptNode {
            database: "mydb".into(),
            rel_path: "util/audit".into(),
            name: "audit".into(),
            instance_data_dir: unique_tmpdir(),
            metadata: db_script_metadata("mydb", "audit"),
        };
        let ctx = ActionContext::default();
        match leaf.invoke_action("edit", &ctx).await.unwrap() {
            ActionDispatch::OpenEditor { session_kind, params } => {
                assert_eq!(session_kind, "postgres_db_script");
                assert_eq!(params.get("database").map(String::as_str), Some("mydb"));
                // Full rel_path including slashes so the session opens
                // the nested file, not a sibling at root.
                assert_eq!(params.get("script").map(String::as_str), Some("util/audit"));
            }
            other => panic!("expected OpenEditor, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&leaf.instance_data_dir);
    }

    #[tokio::test]
    async fn dir_mark_paste_rename_return_noop() {
        let dir = DbScriptDirNode {
            database: "mydb".into(),
            rel_path: "util".into(),
            name: "util".into(),
            instance_data_dir: unique_tmpdir(),
            metadata: db_script_dir_metadata("mydb", "util"),
        };
        let ctx = ActionContext::default();
        for name in ["rename", "mark-move", "paste-move"] {
            assert!(matches!(
                dir.invoke_action(name, &ctx).await.unwrap(),
                ActionDispatch::Noop
            ));
        }
        // delete-dir takes the same DeleteSelf path as leaf delete; the
        // TUI tells them apart via node_type.
        assert!(matches!(
            dir.invoke_action("delete-dir", &ctx).await.unwrap(),
            ActionDispatch::DeleteSelf { .. }
        ));
        let _ = std::fs::remove_dir_all(&dir.instance_data_dir);
    }
}

/// `ContentAdapter::child_process_env` tests for the Postgres adapter
/// (AE feature). Cover the four interesting states: tunnel closed,
/// tunnel open + db in NodeRef, tunnel open + missing db (fallback),
/// and PGSSLMODE rendering.
#[cfg(test)]
mod child_process_env_tests {
    use super::*;
    use crate::client::PostgresClient;
    use crate::config::{PostgresAuth, SslMode};
    use not_yet_done_content::{CredentialProvider, NodeRef};
    use not_yet_done_transport::{Endpoint, TransportConfig, TransportMode};

    fn dummy_client(sslmode: SslMode) -> Arc<PostgresClient> {
        Arc::new(PostgresClient::new(
            TransportConfig {
                mode: TransportMode::Direct,
                ssh: vec![],
                target: Endpoint {
                    host: "db.invalid".to_string(),
                    port: 5432,
                },
            },
            PostgresAuth {
                user: "alice".to_string(),
                password: CredentialProvider::Literal {
                    value: "ignored-in-test".to_string(),
                },
                admin_database: "postgres".to_string(),
                sslmode,
            },
            None,
        ))
    }

    fn adapter(client: Arc<PostgresClient>) -> PostgresAdapter {
        PostgresAdapter::from_client(client, "test-conn".into(), "test".into())
    }

    #[test]
    fn empty_when_tunnel_closed() {
        let a = adapter(dummy_client(SslMode::Disable));
        let nref = NodeRef::parse("postgres/mydb/db_scripts/foo.sql").unwrap();
        assert!(a.child_process_env(&nref).is_empty());
    }

    #[test]
    fn returns_pg_vars_with_db_from_node_ref() {
        let client = dummy_client(SslMode::Disable);
        client.set_env_cache_for_test("127.0.0.1", 41234, "secret123", SslMode::Disable);
        let a = adapter(client);
        let nref = NodeRef::parse("postgres/inventory_db/db_scripts/foo.sql").unwrap();
        let env = a.child_process_env(&nref);
        assert_eq!(env.get("PGHOST").map(String::as_str), Some("127.0.0.1"));
        assert_eq!(env.get("PGPORT").map(String::as_str), Some("41234"));
        assert_eq!(env.get("PGUSER").map(String::as_str), Some("alice"));
        assert_eq!(env.get("PGPASSWORD").map(String::as_str), Some("secret123"));
        assert_eq!(
            env.get("PGDATABASE").map(String::as_str),
            Some("inventory_db")
        );
        assert_eq!(env.get("PGSSLMODE").map(String::as_str), Some("disable"));
    }

    #[test]
    fn falls_back_to_admin_database_for_single_segment_ref() {
        let client = dummy_client(SslMode::Disable);
        client.set_env_cache_for_test("127.0.0.1", 5555, "pw", SslMode::Disable);
        let a = adapter(client);
        // No tail segment → no db in path → fallback to auth.admin_database.
        let nref = NodeRef::parse("postgres").unwrap();
        let env = a.child_process_env(&nref);
        assert_eq!(env.get("PGDATABASE").map(String::as_str), Some("postgres"));
    }

    #[test]
    fn sslmode_renders_all_three_variants() {
        for (mode, expect) in [
            (SslMode::Disable, "disable"),
            (SslMode::Prefer, "prefer"),
            (SslMode::Require, "require"),
        ] {
            let client = dummy_client(mode);
            client.set_env_cache_for_test("h", 1, "p", mode);
            let a = adapter(client);
            let nref = NodeRef::parse("postgres/anydb").unwrap();
            assert_eq!(
                a.child_process_env(&nref).get("PGSSLMODE").map(String::as_str),
                Some(expect)
            );
        }
    }
}
