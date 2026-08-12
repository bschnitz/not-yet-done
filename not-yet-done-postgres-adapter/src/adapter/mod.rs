//! Postgres ContentAdapter: navigates the live catalogue
//!   root → database → "Schemas" → schema → "Tables" → table
//!                                        → "Views"  → view → rows
//! mirroring DBeaver's tree. The intermediate group nodes ("Schemas",
//! "Tables", "Views") have no per-instance state — they exist for visual
//! structure and to give a stable place to attach further sibling groups
//! (Functions, …).
//!
//! Tables and views are separate node types rather than one type with a
//! flag: a view carries editable SQL and a table does not, and a node type
//! is what `actions_for_type` keys the definition editor off. Everything
//! else they share — rows below them are listed by the same code, told
//! apart only by the group segment in the id.
//!
//! For convenience, `DatabaseNode` also lists `postgres:schema`
//! directly (and `SchemaNode` lists `postgres:table`/`postgres:view`
//! directly), so a YAML view can drill through without the group nodes.
//! The emitted IDs still encode the full path
//! (`<db>/schemas/<s>/tables/<t>`), which keeps `get_by_id`'s walker
//! happy — it always traverses the group nodes internally.

use std::sync::Arc;

use async_trait::async_trait;

use not_yet_done_content::{
    ActionContext, ActionDispatch, ActionInput, ActionOutcome, AdapterCapabilities, AdapterStatus,
    ContentAdapter, ContentError, CursorIntent, CustomQueryContext, CustomQueryResult, EditorPrep,
    InputSpec, ListParams, ListResult, Metadata, MetadataField, Node, NodeAction, NodeRef,
    NodeSummary, NodeType, PageInfo, PageRequest, Result, script_buffer,
};

use not_yet_done_sql_core::db_script_nodes::{DB_SCRIPTS_GROUP_ID, DbScriptTree};
use not_yet_done_sql_core::script_completions as completions;
use not_yet_done_sql_core::{RowKeySpec, RowSnapshot};
use not_yet_done_sql_core::{quote_ident, row_edit, view_ddl};

use crate::client::{DatabaseEntry, PostgresClient, RelationKind, SchemaEntry, TableEntry};

mod anonymize;
pub(crate) mod auth;
mod cursor_registry;
mod factory;

pub use auth::PostgresCredentials;
pub use cursor_registry::{CursorId, CursorRegistry};
pub use factory::PostgresAdapterFactory;

pub struct PostgresAdapter {
    client: Arc<PostgresClient>,
    cursor_registry: Arc<CursorRegistry>,
    connection_name: String,
    instance_id: String,
    /// The `DB Scripts` branch below each database — nodes, actions and
    /// the [`SqlScriptStore`](not_yet_done_sql_core::SqlScriptStore) they
    /// write through, all shared with the other SQL adapters. Held as an
    /// `Arc` so every node in the branch borrows the same store instead
    /// of rebuilding one per action.
    db_scripts: Arc<DbScriptTree>,
    /// The `auth:` block, when the config has one. The client resolves
    /// through it; the adapter only needs it for the frontend's half of
    /// the conversation — answering, cancelling, forgetting.
    credentials: Option<Arc<PostgresCredentials>>,
}

impl PostgresAdapter {
    pub(crate) fn from_client(
        client: Arc<PostgresClient>,
        connection_name: String,
        instance_id: String,
        credentials: Option<Arc<PostgresCredentials>>,
    ) -> Self {
        let cursor_registry = Arc::new(CursorRegistry::new(Arc::clone(&client)));
        // Resolve the same per-instance data dir the trait's default
        // `instance_data_dir()` produces, so the store and the adapter
        // agree on where scripts live. We can't call the method before
        // the struct exists, so mirror its layout here.
        let instance_data_dir = dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("not_yet_done")
            .join("postgres")
            .join(&instance_id);
        let db_scripts = Arc::new(DbScriptTree::new(
            crate::script_store::postgres_script_store(instance_data_dir),
            "postgres",
        ));
        Self {
            client,
            cursor_registry,
            connection_name,
            instance_id,
            db_scripts,
            credentials,
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
    pub async fn list_completion_tables(&self, database: &str) -> Vec<(String, String)> {
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

fn views_group_node_type() -> NodeType {
    NodeType {
        type_id: "postgres:views".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Views".into(),
    }
}

/// A view is its own type rather than a table with a flag: it is the one
/// catalogue object that carries editable SQL, and giving it a type is what
/// lets `actions_for_type` offer the definition editor on views alone.
/// `syntax`/`file_extension` are set for that editor's buffer.
fn view_node_type() -> NodeType {
    NodeType {
        type_id: "postgres:view".into(),
        mime_type: "".into(),
        syntax: Some("sql".into()),
        file_extension: ".sql".into(),
        display_name: "View".into(),
    }
}

fn row_node_type() -> NodeType {
    NodeType {
        type_id: "postgres:row".into(),
        // The row editor's buffer is a YAML mapping, so an editor that
        // picks its syntax from the node gets YAML highlighting.
        mime_type: "application/yaml".into(),
        syntax: Some("yaml".into()),
        file_extension: ".yaml".into(),
        display_name: "Row".into(),
    }
}

/// Addressing waypoint for the `rows` segment of a row id
/// (`<db>/schemas/<s>/tables/<t>/rows/<offset>`).
///
/// Never rendered and never bound in a view config: rows are listed
/// through the relation's own fetcher in [`PostgresAdapter::childs`], and
/// this type exists only so `get_by_id` can walk *to* a row — which is what
/// the row editor needs, since an edit session resolves its node by id.
fn rows_group_node_type() -> NodeType {
    NodeType {
        type_id: "postgres:rows".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Rows".into(),
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

// The three `postgres:db_script*` types are not built here: the whole
// DB-Scripts branch comes from [`DbScriptTree`], which prefixes them with
// this adapter's type at construction time. Read them off
// `self.db_scripts.types()`.

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

/// Metadata for a table or a view. The estimated row count is a table-only
/// column: `pg_class.reltuples` is `-1` for a view, and rendering that as a
/// row estimate would be worse than leaving the cell empty.
fn table_metadata(entry: &TableEntry) -> Metadata {
    let estimated_rows = match entry.kind {
        RelationKind::Table => entry.estimated_rows.to_string(),
        RelationKind::View => String::new(),
    };
    Metadata {
        fields: vec![
            field("name", "Name", &entry.name),
            field("database", "Database", &entry.database),
            field("schema", "Schema", &entry.schema),
            field("owner", "Owner", &entry.owner),
            field("estimated_rows", "Rows (est.)", &estimated_rows),
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
// The DB-Scripts branch's three sets live in
// [`not_yet_done_sql_core::db_script_nodes`] and are reached through
// [`DbScriptTree::actions_for_type`].
// ---------------------------------------------------------------------------

fn table_actions() -> Vec<NodeAction> {
    // YAML `shortcuts:` on the `postgres:tables` (Q on table row) or
    // `postgres:rows` (Q: parent:edit_sql when drilled in) ChildDef.
    vec![NodeAction::new("edit_sql", "sql", InputSpec::None)]
}

/// A view does everything a table does, plus edit its own definition.
///
/// `InputSpec::Editor` has to be bound from YAML as
/// `actions: [{type: edit, id: edit_view}]` — not as a `shortcuts:` entry,
/// which routes through `invoke_action` and cannot open an editor.
fn view_actions() -> Vec<NodeAction> {
    let mut actions = table_actions();
    actions.push(NodeAction::new(
        EDIT_VIEW_ACTION,
        "definition",
        InputSpec::Editor,
    ));
    actions
}

/// One action on a data row: edit it. Like `edit_view` this is an
/// `InputSpec::Editor` action and has to be bound from YAML as
/// `actions: [{type: edit, id: edit_row}]`.
///
/// Deliberately *not* `edit_sql`: the query editor belongs to the relation,
/// and a row level reaches it through `parent:edit_sql`.
fn row_actions() -> Vec<NodeAction> {
    vec![NodeAction::new(EDIT_ROW_ACTION, "row", InputSpec::Editor)]
}

// ---------------------------------------------------------------------------
// Adapter
// ---------------------------------------------------------------------------

#[async_trait]
impl ContentAdapter for PostgresAdapter {
    fn adapter_type(&self) -> &str {
        "postgres"
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn script_store(&self) -> Option<&dyn not_yet_done_content::ScriptStore> {
        Some(self.db_scripts.store())
    }

    /// Realism anonymizer: catalogue names become `<adjective>_<noun>`
    /// placeholders (`big_database`, `nifty_schema`) so the tree still reads as a
    /// database/schema/table; the structural group nodes stay verbatim. The safe
    /// StandardAnonymizer is the fallback. See [`anonymize`](self::anonymize).
    fn anonymizer(&self) -> std::sync::Arc<dyn not_yet_done_content::Anonymizer> {
        std::sync::Arc::new(anonymize::PostgresAnonymizer::default())
    }

    fn subscribe_status(&self) -> tokio::sync::watch::Receiver<AdapterStatus> {
        self.client.subscribe_status()
    }

    /// The frontend's answer to a `NeedsCreds` the auth block put up.
    /// Without an auth block nothing can be waiting for one, which is a
    /// programming error rather than a user-facing state.
    async fn submit_credentials(
        &self,
        fields: std::collections::HashMap<String, String>,
    ) -> Result<()> {
        match &self.credentials {
            Some(c) => c
                .submit(fields)
                .await
                .map_err(|e| ContentError::Other(e.into())),
            None => Err(ContentError::Other(
                "this connection asks for no credentials".into(),
            )),
        }
    }

    async fn cancel_credentials(&self) -> Result<()> {
        match &self.credentials {
            Some(c) => c.cancel().await.map_err(|e| ContentError::Other(e.into())),
            None => Ok(()),
        }
    }

    /// Forget the resolved secrets so the next connect asks again — the
    /// user's way out when a rotated password is cached.
    async fn invalidate_credentials(&self) -> Result<()> {
        if let Some(c) = &self.credentials {
            c.invalidate().await;
        }
        Ok(())
    }

    async fn root(&self) -> Result<Box<dyn Node>> {
        Ok(Box::new(PostgresRoot {
            connection_name: self.connection_name.clone(),
            client: Arc::clone(&self.client),
            db_scripts: Arc::clone(&self.db_scripts),
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

    /// Single source of truth about a node's children. Reads the node's
    /// FULL composite `id()` (the addressability invariant restored in this
    /// refactor), parses it exactly as `get_by_id`'s segment walker would,
    /// and hands each child type a lazy fetcher onto the shared `*_impl`
    /// listing functions. No downcast — every fact comes from `node.id()`
    /// and `node.node_type()`.
    fn childs<'a>(&'a self, node: &'a dyn Node) -> Vec<not_yet_done_content::Child<'a>> {
        use not_yet_done_content::Child;
        // Composite id, parsed the same way the walker consumes it.
        let id = node.id().to_string();
        let segs: Vec<String> = id.split('/').map(|s| s.to_string()).collect();

        // Small constructors so each arm reads as `child(type, fetcher)`.
        macro_rules! child {
            ($nt:expr, $fetch:expr) => {
                Child {
                    node_type: $nt,
                    columns: Vec::new(),
                    list: Box::new($fetch),
                }
            };
        }

        match node.node_type().type_id.as_str() {
            "postgres:root" => vec![
                child!(database_node_type(), move |_p| {
                    Box::pin(async move { list_databases_impl(&self.client).await })
                }),
                child!(table_node_type(), move |_p| {
                    Box::pin(async move {
                        list_all_relations_impl(&self.client, RelationKind::Table).await
                    })
                }),
                child!(view_node_type(), move |_p| {
                    Box::pin(async move {
                        list_all_relations_impl(&self.client, RelationKind::View).await
                    })
                }),
                child!(script_node_type(), move |_p| {
                    let dir = self.instance_data_dir();
                    Box::pin(async move { list_all_scripts_impl(&dir).await })
                }),
            ],
            "postgres:database" => {
                // id = `<db>`
                let db = id.clone();
                let db2 = db.clone();
                let db3 = db.clone();
                let db4 = db.clone();
                vec![
                    child!(schemas_group_node_type(), move |_p| {
                        Box::pin(async move { Ok(list_schemas_group_impl(&db)) })
                    }),
                    child!(schema_node_type(), move |_p| {
                        Box::pin(async move { list_schemas_impl(&self.client, &db2).await })
                    }),
                    child!(self.db_scripts.types().group.clone(), move |_p| {
                        Box::pin(async move { Ok(self.db_scripts.group_summary(&db3)) })
                    }),
                    // db_script_dir + db_script direct shortcuts. The
                    // direct-shortcut list only surfaces scripts (matching
                    // the legacy `DatabaseNode::list` `postgres:db_script`
                    // arm); the `db_script_dir` shortcut yields nothing on
                    // its own (legacy `DatabaseNode::list` had no arm for
                    // it — its `NotSupported` would produce no rows).
                    child!(self.db_scripts.types().dir.clone(), move |_p| {
                        Box::pin(async move {
                            Err(ContentError::NotSupported(
                                "database has no direct db_script_dir shortcut".into(),
                            ))
                        })
                    }),
                    child!(self.db_scripts.types().script.clone(), move |_p| {
                        Box::pin(async move { self.db_scripts.list_scripts_flat(&db4).await })
                    }),
                ]
            }
            "postgres:schemas" => {
                // id = `<db>/schemas`
                let db = segs.first().cloned().unwrap_or_default();
                vec![child!(schema_node_type(), move |_p| {
                    Box::pin(async move { list_schemas_impl(&self.client, &db).await })
                })]
            }
            "postgres:schema" => {
                // id = `<db>/schemas/<s>`
                let db = segs.first().cloned().unwrap_or_default();
                let schema = segs.get(2).cloned().unwrap_or_default();
                let (db2, schema2) = (db.clone(), schema.clone());
                let (db3, schema3) = (db.clone(), schema.clone());
                let (db4, schema4) = (db.clone(), schema.clone());
                vec![
                    child!(tables_group_node_type(), move |_p| {
                        Box::pin(async move {
                            Ok(list_relations_group_impl(&db, &schema, RelationKind::Table))
                        })
                    }),
                    child!(table_node_type(), move |_p| {
                        Box::pin(async move {
                            list_relations_impl(&self.client, &db2, &schema2, RelationKind::Table)
                                .await
                        })
                    }),
                    child!(views_group_node_type(), move |_p| {
                        Box::pin(async move {
                            Ok(list_relations_group_impl(
                                &db3,
                                &schema3,
                                RelationKind::View,
                            ))
                        })
                    }),
                    child!(view_node_type(), move |_p| {
                        Box::pin(async move {
                            list_relations_impl(&self.client, &db4, &schema4, RelationKind::View)
                                .await
                        })
                    }),
                ]
            }
            "postgres:tables" | "postgres:views" => {
                // id = `<db>/schemas/<s>/tables` or `<db>/schemas/<s>/views`
                let db = segs.first().cloned().unwrap_or_default();
                let schema = segs.get(2).cloned().unwrap_or_default();
                let kind = match node.node_type().type_id.as_str() {
                    "postgres:views" => RelationKind::View,
                    _ => RelationKind::Table,
                };
                let node_type = match kind {
                    RelationKind::View => view_node_type(),
                    RelationKind::Table => table_node_type(),
                };
                vec![child!(node_type, move |_p| {
                    Box::pin(
                        async move { list_relations_impl(&self.client, &db, &schema, kind).await },
                    )
                })]
            }
            "postgres:table" | "postgres:view" => {
                // id = `<db>/schemas/<s>/tables/<t>` or `…/views/<v>`. The
                // group segment is read off the id rather than assumed, so
                // one row lister serves both branches and `get_by_id`
                // walks back to whichever node minted the row.
                let db = segs.first().cloned().unwrap_or_default();
                let schema = segs.get(2).cloned().unwrap_or_default();
                let group = segs.get(3).cloned().unwrap_or_default();
                let table = segs.get(4).cloned().unwrap_or_default();
                let kind = match node.node_type().type_id.as_str() {
                    "postgres:view" => RelationKind::View,
                    _ => RelationKind::Table,
                };
                vec![child!(row_node_type(), move |p: ListParams| {
                    Box::pin(async move {
                        list_rows_impl(&self.client, &db, &schema, &group, &table, kind, &p).await
                    })
                })]
            }
            "postgres:db_scripts" | "postgres:db_script_dir" => {
                // id = `<db>/db_scripts` (group) or
                //      `<db>/db_scripts/<rel_path...>` (dir).
                let db = segs.first().cloned().unwrap_or_default();
                let rel_path = if segs.len() > 2 {
                    segs[2..].join("/")
                } else {
                    String::new()
                };
                let db2 = db.clone();
                let rel2 = rel_path.clone();
                vec![
                    child!(self.db_scripts.types().dir.clone(), move |_p| {
                        Box::pin(async move {
                            self.db_scripts
                                .list_entries(&db, &rel_path, true, false)
                                .await
                        })
                    }),
                    child!(self.db_scripts.types().script.clone(), move |_p| {
                        Box::pin(async move {
                            self.db_scripts.list_entries(&db2, &rel2, false, true).await
                        })
                    }),
                ]
            }
            // Leaves: postgres:row, postgres:db_script, postgres:script.
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
            propagates_query_to_subtree: false,
            group_by_via_adapter: false,
            supports_eager_subtree: false,
            // Per-table SQL scripts: the `q` menu and the `Q` editor.
            supports_node_query_editor: true,
            // Row ids are `qrow:<n>` — an offset into one specific query's
            // result set, meaningless once the query or its ordering
            // changes. Nothing here can be linked or marked.
            unstable_node_ids: true,
        }
    }

    /// Every node id this adapter mints starts with the database name
    /// (`<db>`, `<db>/schemas/…`, `<db>/db_scripts/…`), which is exactly
    /// the routing key `execute_custom_query` needs — `dbname` is fixed at
    /// connect time, so a query has to be told which session to run on.
    fn custom_query_context(&self, node_id: &str) -> CustomQueryContext {
        let database = node_id
            .split('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.client.admin_database());
        CustomQueryContext::new().with("database", database.to_string())
    }

    fn actions_for_type(&self, node_type: &NodeType) -> Vec<NodeAction> {
        // The DB-Scripts branch answers for its own three types; the
        // catalogue types are this adapter's own.
        if let Some(actions) = self.db_scripts.actions_for_type(&node_type.type_id) {
            return actions;
        }
        match node_type.type_id.as_str() {
            "postgres:table" => table_actions(),
            "postgres:view" => view_actions(),
            "postgres:row" => row_actions(),
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
    fn child_process_env(&self, node: &NodeRef) -> std::collections::HashMap<String, String> {
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

    /// Append a trailing `-- table completions: tt_<schema>__<table>, …`
    /// comment listing every base table in the script's database, so the
    /// user can copy a `tt_*` token into their SQL (substituted to the
    /// quoted identifier at execute time). Only for SQL-flavored scripts;
    /// `.py`/`.md`/… scripts get the buffer back stripped but unaugmented.
    ///
    /// `node` carries the canonical id `postgres/<db>/db_scripts/<script>`;
    /// the database is segment[1] and the SQL gate keys off the final
    /// segment's extension. Enumeration failures yield no line (the editor
    /// still opens). The append is idempotent: any stale completion line is
    /// stripped first.
    async fn augment_editor_buffer(&self, node: &NodeRef, buffer: String) -> String {
        let stripped = completions::strip_completions_line(&buffer);
        let script = node.segments().last().unwrap_or("");
        if !crate::query::is_sql_extension(script) {
            return stripped;
        }
        let Some(db) = node.segments().nth(1).filter(|s| !s.is_empty()) else {
            return stripped;
        };
        let tables = self.list_completion_tables(db).await;
        let entries = crate::script_completions::completions_for_tables(&tables);
        match completions::build_completions_line(&entries) {
            Some(line) => completions::append_completions_line(&stripped, &line),
            None => stripped,
        }
    }

    fn strip_editor_hints(&self, text: &str) -> String {
        completions::strip_completions_line(text)
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
        let query_ref = if completions::may_contain_tokens(query) {
            let tables = self
                .client
                .list_tables_in_database(database)
                .await
                .unwrap_or_default();
            let entries = crate::script_completions::completions_for_tables(&tables);
            owned_query = completions::substitute_tokens(query, &entries);
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
                let trimmed: Vec<_> = outcome
                    .rows
                    .iter()
                    .take(req.limit as usize)
                    .cloned()
                    .collect();
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
        let page_size = context.page.map(|p| p.limit).unwrap_or(CURSOR_PAGE_DEFAULT);
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
fn rows_to_summaries(columns: &[String], rows: &[Vec<Option<String>>]) -> Vec<NodeSummary> {
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
/// pagination. Returns `None` (caller runs the original) when the query
/// isn't a paginable shape: not SELECT/WITH, or multi-statement. We
/// fetch one extra row (`limit + 1`) so the caller can decide `has_next`
/// without a second round trip.
///
/// The shape check and the wrapping itself are dialect-independent and
/// live in `not-yet-done-sql-core`; all this adds is the derived-table
/// alias.
fn wrap_for_pagination(query: &str, page: PageRequest) -> Option<String> {
    not_yet_done_sql_core::sql_shape::wrap_for_pagination(query, page.limit, page.offset, "_nyd_pg")
}

// ---------------------------------------------------------------------------
// Root node — children are databases
// ---------------------------------------------------------------------------

struct PostgresRoot {
    connection_name: String,
    /// Only one node below the root actually queries through this handle
    /// ([`ViewNode`], whose `prepare`/`execute` read and replace a view's
    /// definition). Everything else lists through the adapter's `childs`
    /// fetchers — but a node's own action methods have no other way to
    /// reach the database, so the client is threaded down the walk.
    client: Arc<PostgresClient>,
    db_scripts: Arc<DbScriptTree>,
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

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        // Lazy construction: skip the `list_databases` round trip that the
        // old code used purely to populate owner/encoding metadata. During a
        // `get_by_id` walk through `<db>/...` the database node is only
        // traversed, never rendered (its NodeSummary comes from the root's
        // `childs` database fetcher with full metadata), so the cheap path
        // is correct.
        Ok(Box::new(DatabaseNode::new(
            id.to_string(),
            Arc::clone(&self.client),
            Arc::clone(&self.db_scripts),
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
    name: String,
    client: Arc<PostgresClient>,
    db_scripts: Arc<DbScriptTree>,
}

impl DatabaseNode {
    fn new(name: String, client: Arc<PostgresClient>, db_scripts: Arc<DbScriptTree>) -> Self {
        Self {
            name,
            client,
            db_scripts,
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

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        match id {
            SCHEMAS_GROUP_ID => Ok(Box::new(SchemasGroupNode {
                database: self.name.clone(),
                client: Arc::clone(&self.client),
                node_id: format!("{}/{SCHEMAS_GROUP_ID}", self.name),
            })),
            DB_SCRIPTS_GROUP_ID => Ok(DbScriptTree::group_node(&self.db_scripts, &self.name)),
            other => Err(ContentError::NotFound(other.into())),
        }
    }
}

const SCHEMAS_GROUP_ID: &str = "schemas";
pub(crate) const TABLES_GROUP_ID: &str = "tables";
pub(crate) const VIEWS_GROUP_ID: &str = "views";
const ROWS_GROUP_ID: &str = "rows";
const EDIT_VIEW_ACTION: &str = "edit_view";
const EDIT_ROW_ACTION: &str = "edit_row";

// ---------------------------------------------------------------------------
// Shared listing logic. Each of these is the single implementation of one
// per-node `list` body, keyed by the parsed path parts + the pieces of
// adapter/node state it needs. BOTH the legacy `Node::list` match arms AND
// the `PostgresAdapter::childs` fetch closures call these — no duplication.
// ---------------------------------------------------------------------------

fn empty_list(items: Vec<NodeSummary>) -> ListResult {
    ListResult {
        items,
        applied_sort: Vec::new(),
        page: None,
        batch_download_available: false,
        downloaded: vec![],
    }
}

/// `postgres:database` list under root.
async fn list_databases_impl(client: &PostgresClient) -> Result<ListResult> {
    let entries = client
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
    Ok(empty_list(items))
}

/// Cross-DB / cross-schema flat `postgres:table` (or `postgres:view`) list
/// under root. IDs use the composite `<db>/schemas/<s>/<group>/<name>` form.
async fn list_all_relations_impl(
    client: &PostgresClient,
    kind: RelationKind,
) -> Result<ListResult> {
    let entries = match kind {
        RelationKind::Table => client.list_all_tables().await,
        RelationKind::View => client.list_all_views().await,
    }
    .map_err(|e| ContentError::Other(e.into()))?;
    Ok(empty_list(relation_summaries(entries, kind)))
}

/// The group segment and node type one [`RelationKind`] renders as.
fn relation_shape(kind: RelationKind) -> (&'static str, NodeType) {
    match kind {
        RelationKind::Table => (TABLES_GROUP_ID, table_node_type()),
        RelationKind::View => (VIEWS_GROUP_ID, view_node_type()),
    }
}

/// Catalogue entries → node summaries. Shared by the flat root list and
/// the per-schema one so an id is minted in exactly one place.
fn relation_summaries(entries: Vec<TableEntry>, kind: RelationKind) -> Vec<NodeSummary> {
    let (group, node_type) = relation_shape(kind);
    entries
        .into_iter()
        .map(|e| NodeSummary {
            id: format!(
                "{}/{SCHEMAS_GROUP_ID}/{}/{group}/{}",
                e.database, e.schema, e.name
            ),
            label: e.name.clone(),
            node_type: node_type.clone(),
            metadata: table_metadata(&e),
            has_children: None,
        })
        .collect()
}

/// Filesystem-backed flat `postgres:script` list under root (mixes
/// table-level `queries/` and DB-level `db_scripts/`).
async fn list_all_scripts_impl(instance_data_dir: &std::path::Path) -> Result<ListResult> {
    let table_scripts = crate::query::list_all_scripts(instance_data_dir)
        .await
        .map_err(|e| ContentError::Other(Box::new(e)))?;
    let db_scripts = crate::query::list_all_db_scripts(instance_data_dir)
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
                field("schema", "Schema", ""),
                field("table", "Table", ""),
            ],
        },
        has_children: None,
    }));
    Ok(empty_list(items))
}

/// The single virtual "Schemas" group folder under a database.
fn list_schemas_group_impl(database: &str) -> ListResult {
    empty_list(vec![NodeSummary {
        id: format!("{database}/{SCHEMAS_GROUP_ID}"),
        label: "Schemas".into(),
        node_type: schemas_group_node_type(),
        metadata: Metadata { fields: vec![] },
        has_children: None,
    }])
}

/// `postgres:schema` instances under a database (composite IDs go through
/// the `schemas` group segment so `get_by_id` resolves both routes).
async fn list_schemas_impl(client: &PostgresClient, database: &str) -> Result<ListResult> {
    let entries = client
        .list_schemas(database)
        .await
        .map_err(|e| ContentError::Other(e.into()))?;
    let items = entries
        .into_iter()
        .map(|e| NodeSummary {
            id: format!("{database}/{SCHEMAS_GROUP_ID}/{}", e.name),
            label: e.name.clone(),
            node_type: schema_node_type(),
            metadata: schema_metadata(&e),
            has_children: None,
        })
        .collect();
    Ok(empty_list(items))
}

/// The single virtual "Tables" (or "Views") group folder under a schema.
fn list_relations_group_impl(database: &str, schema: &str, kind: RelationKind) -> ListResult {
    let (group, _) = relation_shape(kind);
    let node_type = match kind {
        RelationKind::Table => tables_group_node_type(),
        RelationKind::View => views_group_node_type(),
    };
    empty_list(vec![NodeSummary {
        id: format!("{database}/{SCHEMAS_GROUP_ID}/{schema}/{group}"),
        label: node_type.display_name.clone(),
        node_type,
        metadata: Metadata { fields: vec![] },
        has_children: None,
    }])
}

/// `postgres:table` / `postgres:view` instances under a schema.
async fn list_relations_impl(
    client: &PostgresClient,
    database: &str,
    schema: &str,
    kind: RelationKind,
) -> Result<ListResult> {
    let entries = match kind {
        RelationKind::Table => client.list_tables(database, schema).await,
        RelationKind::View => client.list_views(database, schema).await,
    }
    .map_err(|e| ContentError::Other(e.into()))?;
    Ok(empty_list(relation_summaries(entries, kind)))
}

/// `postgres:row` instances under a table or a view (paginated `SELECT *`).
///
/// `group` is the id segment the parent was addressed by (`tables` or
/// `views`) so the row ids stay under the node that listed them.
#[allow(clippy::too_many_arguments)]
async fn list_rows_impl(
    client: &PostgresClient,
    database: &str,
    schema: &str,
    group: &str,
    table: &str,
    kind: RelationKind,
    params: &ListParams,
) -> Result<ListResult> {
    let (offset, limit) = match params.page {
        Some(p) if p.limit > 0 => (p.offset, p.limit),
        _ => (0, 100),
    };
    let page = client
        .query_rows(database, schema, table, kind, offset, limit)
        .await
        .map_err(|e| ContentError::Other(e.into()))?;

    let id_prefix = format!("{database}/{SCHEMAS_GROUP_ID}/{schema}/{group}/{table}");
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

// ---------------------------------------------------------------------------
// "Schemas" group — children are individual schemas
// ---------------------------------------------------------------------------

struct SchemasGroupNode {
    database: String,
    client: Arc<PostgresClient>,
    /// Full composite id `<db>/schemas` — the addressability invariant:
    /// equals the id `get_by_id` consumes to rebuild this node.
    node_id: String,
}

#[async_trait]
impl Node for SchemasGroupNode {
    fn id(&self) -> &str {
        &self.node_id
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

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        // Lazy: see comment on `PostgresAdapter::get_child`.
        Ok(Box::new(SchemaNode::new(
            self.database.clone(),
            id.to_string(),
            Arc::clone(&self.client),
        )))
    }
}

// ---------------------------------------------------------------------------
// Schema node — single virtual child "Tables"
// ---------------------------------------------------------------------------

// Holds only the schema name; same rationale as [`DatabaseNode`]. Table
// listing runs through the adapter's `childs` fetcher (which owns the
// live client), so the node itself no longer needs a client handle.
struct SchemaNode {
    database: String,
    name: String,
    client: Arc<PostgresClient>,
    /// Full composite id `<db>/schemas/<s>`.
    node_id: String,
}

impl SchemaNode {
    fn new(database: String, name: String, client: Arc<PostgresClient>) -> Self {
        let node_id = format!("{database}/{SCHEMAS_GROUP_ID}/{name}");
        Self {
            database,
            name,
            client,
            node_id,
        }
    }
}

#[async_trait]
impl Node for SchemaNode {
    fn id(&self) -> &str {
        &self.node_id
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

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        match id {
            TABLES_GROUP_ID => Ok(Box::new(TablesGroupNode {
                database: self.database.clone(),
                schema: self.name.clone(),
                client: Arc::clone(&self.client),
                node_id: format!(
                    "{}/{SCHEMAS_GROUP_ID}/{}/{TABLES_GROUP_ID}",
                    self.database, self.name
                ),
            })),
            VIEWS_GROUP_ID => Ok(Box::new(ViewsGroupNode {
                database: self.database.clone(),
                schema: self.name.clone(),
                client: Arc::clone(&self.client),
                node_id: format!(
                    "{}/{SCHEMAS_GROUP_ID}/{}/{VIEWS_GROUP_ID}",
                    self.database, self.name
                ),
            })),
            other => Err(ContentError::NotFound(other.into())),
        }
    }
}

// ---------------------------------------------------------------------------
// "Tables" group — children are individual tables
// ---------------------------------------------------------------------------

struct TablesGroupNode {
    database: String,
    schema: String,
    /// Handed down to the table so its rows can be read for editing.
    client: Arc<PostgresClient>,
    /// Full composite id `<db>/schemas/<s>/tables`.
    node_id: String,
}

#[async_trait]
impl Node for TablesGroupNode {
    fn id(&self) -> &str {
        &self.node_id
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

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        // Lazy: see comment on `PostgresAdapter::get_child`.
        Ok(Box::new(TableNode::new(
            self.database.clone(),
            self.schema.clone(),
            id.to_string(),
            Arc::clone(&self.client),
        )))
    }
}

// ---------------------------------------------------------------------------
// Table node — children are individual data rows (postgres:row), one
// per row of a paginated `SELECT * FROM schema.table`. Each row's
// metadata exposes the column→value pairs so the TUI can render them
// as table columns.
// ---------------------------------------------------------------------------

// Row *listing* runs through the adapter's `childs` fetcher (which owns the
// live client). The client handle here is for the other direction: walking
// down to a single row, which the row editor does by id.
struct TableNode {
    database: String,
    schema: String,
    name: String,
    client: Arc<PostgresClient>,
    /// Full composite id `<db>/schemas/<s>/tables/<t>`.
    node_id: String,
}

impl TableNode {
    fn new(database: String, schema: String, name: String, client: Arc<PostgresClient>) -> Self {
        let node_id = format!("{database}/{SCHEMAS_GROUP_ID}/{schema}/{TABLES_GROUP_ID}/{name}");
        Self {
            database,
            schema,
            name,
            client,
            node_id,
        }
    }
}

#[async_trait]
impl Node for TableNode {
    fn id(&self) -> &str {
        &self.node_id
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

    /// Only the `rows` waypoint — a table has no other addressable child,
    /// and the rows themselves are listed by the adapter's fetcher.
    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        rows_group_child(
            id,
            &self.database,
            &self.schema,
            TABLES_GROUP_ID,
            &self.name,
            &self.client,
        )
    }

    async fn invoke_action(&self, name: &str, _ctx: &ActionContext) -> Result<ActionDispatch> {
        match name {
            "edit_sql" => Ok(ActionDispatch::OpenEditor {
                session_kind: "query_editor".into(),
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
}

// ---------------------------------------------------------------------------
// The `rows` waypoint and the row itself
// ---------------------------------------------------------------------------

/// The `rows` segment below a table or a view. Shared by both, because a
/// view's rows are addressed exactly like a table's — only the group
/// segment of the id differs.
fn rows_group_child(
    id: &str,
    database: &str,
    schema: &str,
    group: &str,
    table: &str,
    client: &Arc<PostgresClient>,
) -> Result<Box<dyn Node>> {
    if id != ROWS_GROUP_ID {
        return Err(ContentError::NotFound(id.into()));
    }
    Ok(Box::new(RowsGroupNode {
        database: database.to_string(),
        schema: schema.to_string(),
        group: group.to_string(),
        table: table.to_string(),
        client: Arc::clone(client),
        node_id: format!("{database}/{SCHEMAS_GROUP_ID}/{schema}/{group}/{table}/{ROWS_GROUP_ID}"),
    }))
}

/// Pure addressing node: it exists so `get_by_id` can walk through the
/// `rows` segment of a row id. See [`rows_group_node_type`].
struct RowsGroupNode {
    database: String,
    schema: String,
    group: String,
    table: String,
    client: Arc<PostgresClient>,
    node_id: String,
}

#[async_trait]
impl Node for RowsGroupNode {
    fn id(&self) -> &str {
        &self.node_id
    }

    fn label(&self) -> &str {
        "Rows"
    }

    fn node_type(&self) -> &NodeType {
        static T: std::sync::LazyLock<NodeType> = std::sync::LazyLock::new(rows_group_node_type);
        &T
    }

    fn metadata(&self) -> &Metadata {
        static EMPTY: Metadata = Metadata { fields: vec![] };
        &EMPTY
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        let offset: u32 = id.parse().map_err(|_| {
            ContentError::NotFound(format!("`{id}` is not a row offset in {}", self.table))
        })?;
        Ok(Box::new(RowNode::new(
            self.database.clone(),
            self.schema.clone(),
            self.group.clone(),
            self.table.clone(),
            offset,
            Arc::clone(&self.client),
        )))
    }
}

/// One data row, editable as a YAML mapping of its cells.
///
/// The offset in the id is only how the row was *found* — it is the key
/// values read at `prepare` time that every statement afterwards uses, so a
/// page that shifted underneath cannot redirect the write. See
/// [`row_edit`](not_yet_done_sql_core::row_edit) for the buffer protocol,
/// which is shared with every other SQL adapter.
struct RowNode {
    database: String,
    schema: String,
    table: String,
    offset: u32,
    client: Arc<PostgresClient>,
    /// Full composite id `<db>/schemas/<s>/<group>/<table>/rows/<offset>`.
    node_id: String,
}

impl RowNode {
    /// `group` (`tables` or `views`) only shapes the node's own id, so that
    /// it matches the id the listing minted for this row.
    fn new(
        database: String,
        schema: String,
        group: String,
        table: String,
        offset: u32,
        client: Arc<PostgresClient>,
    ) -> Self {
        let node_id = format!(
            "{database}/{SCHEMAS_GROUP_ID}/{schema}/{group}/{table}/{ROWS_GROUP_ID}/{offset}"
        );
        Self {
            database,
            schema,
            table,
            offset,
            client,
            node_id,
        }
    }

    /// Reject the save without losing the user's text — same contract as
    /// the view editor's rejection: the message becomes a banner above
    /// their own buffer, which the editor reopens.
    fn reject(buffer: &str, message: &str) -> ActionOutcome {
        ActionOutcome::Reopen {
            content: row_edit::render_with_error(buffer, message),
            new_version: None,
        }
    }

    /// `schema.table`, as a statement has to spell it.
    fn qualified(&self) -> String {
        format!("{}.{}", quote_ident(&self.schema), quote_ident(&self.table))
    }

    fn label_for_header(&self) -> String {
        format!(
            "Row {} of {}.{} in {}",
            self.offset, self.schema, self.table, self.database
        )
    }

    async fn key_spec(&self) -> Result<RowKeySpec> {
        self.client
            .row_key_spec(&self.database, &self.schema, &self.table)
            .await
            .map_err(ContentError::NotSupported)
    }
}

#[async_trait]
impl Node for RowNode {
    fn id(&self) -> &str {
        &self.node_id
    }

    fn label(&self) -> &str {
        &self.table
    }

    fn node_type(&self) -> &NodeType {
        static T: std::sync::LazyLock<NodeType> = std::sync::LazyLock::new(row_node_type);
        &T
    }

    fn metadata(&self) -> &Metadata {
        // The values the tree renders come from the listing's summaries;
        // this node is only ever resolved to be edited.
        static EMPTY: Metadata = Metadata { fields: vec![] };
        &EMPTY
    }

    /// Read the row, render it, and remember what it looked like: the
    /// `version` token carries both the cell values (to detect a concurrent
    /// change on save) and the key values (so the `UPDATE` addresses the row
    /// that was actually shown, not whatever now sits at the same offset).
    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        if action_id != EDIT_ROW_ACTION {
            return Err(ContentError::NotSupported(format!(
                "a row has no editor action `{action_id}`"
            )));
        }
        let keys = self.key_spec().await?;
        let read = self
            .client
            .read_row_at(
                &self.database,
                &self.schema,
                &self.table,
                &keys,
                self.offset,
            )
            .await
            .map_err(|e| ContentError::Other(e.into()))?
            .ok_or_else(|| {
                ContentError::NotFound(format!(
                    "row {} of {}.{} — the table has fewer rows than that now",
                    self.offset, self.schema, self.table
                ))
            })?;

        let row = RowSnapshot::new(read.cells);
        Ok(EditorPrep {
            template: row_edit::edit_buffer(
                &self.label_for_header(),
                &keys.note(),
                POSTGRES_WRITE_NOTE,
                &row,
            ),
            version: row_edit::version_token(&read.key_values, &row),
            suffix: ".yaml".into(),
            file_path: None,
        })
    }

    /// Everything that can go wrong reopens the editor with a banner
    /// instead of failing the action: the buffer is the only copy of what
    /// the user typed, and a rejected edit is usually one keystroke away
    /// from a good one. When the statement itself is refused, the statement
    /// is shown next to the complaint — a type or constraint error is far
    /// easier to place with the `UPDATE` in front of you.
    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        if action_id != EDIT_ROW_ACTION {
            return Err(ContentError::NotSupported(format!(
                "a row has no editor action `{action_id}`"
            )));
        }
        let ActionInput::Edited { text, version, .. } = input else {
            return Err(ContentError::NotSupported(
                "editing a row needs the editor's saved buffer".into(),
            ));
        };
        // A banner from the previous attempt must reach neither the
        // database nor the next buffer.
        let buffer = row_edit::strip_error_banner(&text).to_string();
        let (key_values, original) = row_edit::parse_version_token(&version).ok_or_else(|| {
            ContentError::Other(
                "the editor session lost the row it opened on"
                    .to_string()
                    .into(),
            )
        })?;

        let edited = match row_edit::parse_row_buffer(&buffer) {
            Ok(edited) => edited,
            Err(message) => return Ok(Self::reject(&buffer, &message)),
        };
        let changes = match row_edit::changed_cells(&original, &edited) {
            Ok(changes) => changes,
            Err(message) => return Ok(Self::reject(&buffer, &message)),
        };
        if changes.is_empty() {
            return Ok(ActionOutcome::NoChanges);
        }

        let keys = self.key_spec().await?;
        let where_sql = row_edit::render_where(&key_values);

        // Re-read before writing, exactly as the view editor does: the row
        // may have changed since the editor opened, and overwriting that
        // silently would drop somebody's change without anyone noticing.
        let current = self
            .client
            .read_rows_where(&self.database, &self.schema, &self.table, &keys, &where_sql)
            .await
            .map_err(|e| ContentError::Other(e.into()))?;
        match current.len() {
            0 => {
                return Ok(Self::reject(
                    &buffer,
                    &format!(
                        "no row of {}.{} matches {where_sql} any more — it was deleted, or its \
                         key changed, since this editor opened. Nothing was written.",
                        self.schema, self.table
                    ),
                ));
            }
            1 => {
                let now = RowSnapshot::new(current[0].cells.clone());
                if now.version_token() != original.version_token() {
                    return Ok(ActionOutcome::Reopen {
                        content: row_edit::render_with_error(
                            &buffer,
                            "this row changed in the database since the editor opened. Your \
                             text is unchanged above; saving again overwrites the new values \
                             with it.",
                        ),
                        new_version: Some(row_edit::version_token(&key_values, &now)),
                    });
                }
            }
            _ => {
                return Ok(Self::reject(
                    &buffer,
                    &format!(
                        "{where_sql} matches more than one row of {}.{}, so a single row \
                         cannot be changed through it — use a DB script with a WHERE that is \
                         unique.",
                        self.schema, self.table
                    ),
                ));
            }
        }

        let statement = row_edit::build_update(&self.qualified(), &changes, &key_values);
        match self.client.execute_write(&self.database, &statement).await {
            // The re-read above proved the key matches exactly one row, so
            // a count other than 1 would mean the row moved between the two
            // statements — rare, but the user should hear about it rather
            // than see a success message for nothing.
            Ok(1) => Ok(ActionOutcome::Done {
                message: Some(format!(
                    "{} of {}.{} updated",
                    row_edit::plural_columns(changes.len()),
                    self.schema,
                    self.table
                )),
            }),
            Ok(affected) => Ok(Self::reject(
                &buffer,
                &format!("the statement changed {affected} rows instead of one:\n{statement}"),
            )),
            Err(message) => Ok(Self::reject(
                &buffer,
                &format!("{message}\n\nThe statement that failed:\n{statement}"),
            )),
        }
    }
}

/// What saving a row does, for the buffer header. Worth saying that the
/// values go in as text: Postgres converts a literal to the column's type,
/// so a number, a date or a JSON document does not have to be spelled in
/// any particular way beyond what that type accepts.
const POSTGRES_WRITE_NOTE: &str = "On save one UPDATE is built from the columns that changed and run on its own.\n\
     Values are written as text literals; the column's type converts them.";

// ---------------------------------------------------------------------------
// "Views" group — children are individual views
// ---------------------------------------------------------------------------

struct ViewsGroupNode {
    database: String,
    schema: String,
    client: Arc<PostgresClient>,
    /// Full composite id `<db>/schemas/<s>/views`.
    node_id: String,
}

#[async_trait]
impl Node for ViewsGroupNode {
    fn id(&self) -> &str {
        &self.node_id
    }

    fn label(&self) -> &str {
        "Views"
    }

    fn node_type(&self) -> &NodeType {
        static T: std::sync::LazyLock<NodeType> = std::sync::LazyLock::new(views_group_node_type);
        &T
    }

    fn metadata(&self) -> &Metadata {
        static EMPTY: Metadata = Metadata { fields: vec![] };
        &EMPTY
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        // Lazy: see comment on `PostgresAdapter::get_child`.
        Ok(Box::new(ViewNode::new(
            self.database.clone(),
            self.schema.clone(),
            id.to_string(),
            Arc::clone(&self.client),
        )))
    }
}

// ---------------------------------------------------------------------------
// View node — a table node that can also edit what defines it
// ---------------------------------------------------------------------------

/// Rows below a view work exactly as they do below a table; what a view
/// adds is that its whole content *is* SQL, so it can be edited like a
/// stored script: [`Node::prepare`] hands out the `CREATE OR REPLACE VIEW …`
/// statement, [`Node::execute`] takes the edited buffer back.
struct ViewNode {
    database: String,
    schema: String,
    name: String,
    client: Arc<PostgresClient>,
    /// Full composite id `<db>/schemas/<s>/views/<v>`.
    node_id: String,
}

impl ViewNode {
    fn new(database: String, schema: String, name: String, client: Arc<PostgresClient>) -> Self {
        let node_id = format!("{database}/{SCHEMAS_GROUP_ID}/{schema}/{VIEWS_GROUP_ID}/{name}");
        Self {
            database,
            schema,
            name,
            client,
            node_id,
        }
    }

    /// Reject the save without losing the user's text: the message becomes
    /// a banner above their own buffer, which the editor reopens.
    fn reject(buffer: &str, message: &str) -> ActionOutcome {
        ActionOutcome::Reopen {
            content: script_buffer::render_with_error(buffer, message),
            new_version: None,
        }
    }

    async fn stored_definition(&self) -> Result<Option<String>> {
        self.client
            .view_definition(&self.database, &self.schema, &self.name)
            .await
            .map_err(|e| ContentError::Other(e.into()))
    }

    /// `schema.name`, as the statement has to spell it.
    fn qualified(&self) -> String {
        format!("{}.{}", self.schema, self.name)
    }
}

#[async_trait]
impl Node for ViewNode {
    fn id(&self) -> &str {
        &self.node_id
    }

    fn label(&self) -> &str {
        &self.name
    }

    fn node_type(&self) -> &NodeType {
        static T: std::sync::LazyLock<NodeType> = std::sync::LazyLock::new(view_node_type);
        &T
    }

    fn metadata(&self) -> &Metadata {
        static EMPTY: Metadata = Metadata { fields: vec![] };
        &EMPTY
    }

    /// A view's rows are addressed exactly like a table's; only the group
    /// segment of the id differs.
    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        rows_group_child(
            id,
            &self.database,
            &self.schema,
            VIEWS_GROUP_ID,
            &self.name,
            &self.client,
        )
    }

    async fn invoke_action(&self, name: &str, _ctx: &ActionContext) -> Result<ActionDispatch> {
        match name {
            // Same per-node SQL scripts a table has — a view is queryable
            // the same way, so `Q` has to work the same way.
            "edit_sql" => Ok(ActionDispatch::OpenEditor {
                session_kind: "query_editor".into(),
                params: std::collections::HashMap::from([
                    ("database".into(), self.database.clone()),
                    ("schema".into(), self.schema.clone()),
                    ("table".into(), self.name.clone()),
                ]),
            }),
            // `edit_view` is an `InputSpec::Editor` action and never
            // arrives here: it goes through `prepare`/`execute` below.
            other => Err(ContentError::NotSupported(format!(
                "view node action '{other}' is not supported"
            ))),
        }
    }

    /// The buffer is the statement postgres re-prints for the view, and
    /// `version` is that same text — so a concurrent change is detectable
    /// by comparison alone, without a modification timestamp postgres does
    /// not keep for a relation.
    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        if action_id != EDIT_VIEW_ACTION {
            return Err(ContentError::NotSupported(format!(
                "a view has no editor action `{action_id}`"
            )));
        }
        let definition = self
            .stored_definition()
            .await?
            .ok_or_else(|| ContentError::NotFound(format!("view {}", self.qualified())))?;
        Ok(EditorPrep {
            template: view_ddl::edit_buffer(&self.qualified(), &definition, POSTGRES_REPLACE_NOTE),
            version: definition,
            suffix: ".sql".into(),
            file_path: None,
        })
    }

    /// Every way this can go wrong reopens the editor with a banner rather
    /// than failing the action: the user's text is the only copy of what
    /// they wrote, and a rejected definition is usually one edit away from
    /// a good one.
    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        if action_id != EDIT_VIEW_ACTION {
            return Err(ContentError::NotSupported(format!(
                "a view has no editor action `{action_id}`"
            )));
        }
        let ActionInput::Edited { text, version, .. } = input else {
            return Err(ContentError::NotSupported(
                "editing a view definition needs the editor's saved buffer".into(),
            ));
        };
        // A banner from the previous attempt must reach neither the
        // database nor the next buffer.
        let buffer = script_buffer::strip_error_banner(&text).to_string();

        let parsed = match view_ddl::parse_create_view(script_buffer::parse_query_area(&buffer)) {
            Ok(parsed) => parsed,
            Err(message) => return Ok(Self::reject(&buffer, &message)),
        };
        if !view_ddl::same_object_name(&parsed.name, &self.name) {
            return Ok(Self::reject(
                &buffer,
                &format!(
                    "this editor edits {}, but the statement creates {} — saving it would \
                     leave {} in place beside a second view. Rename it back, or create the \
                     other view from a DB script.",
                    self.qualified(),
                    parsed.name,
                    self.name
                ),
            ));
        }
        // The qualifier is not decoration here: an unqualified name
        // resolves against the session's `search_path`, so dropping it
        // could put the definition in a different schema than the one
        // being edited — see `PostgresClient::replace_view`.
        match parsed.qualifier.as_deref() {
            Some(q) if view_ddl::same_object_name(q, &self.schema) => {}
            Some(q) => {
                return Ok(Self::reject(
                    &buffer,
                    &format!(
                        "the statement names schema {q}, but this editor edits {} — saving it \
                         would write to a different schema. Use a DB script for that.",
                        self.qualified()
                    ),
                ));
            }
            None => {
                return Ok(Self::reject(
                    &buffer,
                    &format!(
                        "the view has to stay schema-qualified as {}: an unqualified name \
                         follows the connection's search_path and could land in another schema.",
                        self.qualified()
                    ),
                ));
            }
        }
        if view_ddl::same_definition(&parsed.sql, &version) {
            return Ok(ActionOutcome::NoChanges);
        }

        // Re-read before writing: the view may have changed since the
        // editor opened (another session, a DB script), and replacing it
        // silently would drop that change without anyone noticing. The
        // fresh text becomes the new `version`, so saving again is a
        // deliberate overwrite.
        match self.stored_definition().await? {
            Some(current) if !view_ddl::same_definition(&current, &version) => {
                return Ok(ActionOutcome::Reopen {
                    content: script_buffer::render_with_error(
                        &buffer,
                        &format!(
                            "the definition of {} changed in the database since this editor \
                             opened. Your text is unchanged above; saving again replaces the \
                             new definition with it.",
                            self.qualified()
                        ),
                    ),
                    new_version: Some(current),
                });
            }
            None => {
                return Ok(ActionOutcome::Reopen {
                    content: script_buffer::render_with_error(
                        &buffer,
                        &format!(
                            "the view {} no longer exists — it was dropped since this editor \
                             opened. Saving again re-creates it from your text.",
                            self.qualified()
                        ),
                    ),
                    // Empty token: the view is gone, so the next save has
                    // nothing to conflict with.
                    new_version: Some(String::new()),
                });
            }
            Some(_) => {}
        }

        match self
            .client
            .replace_view(&self.database, &self.schema, &self.name, &parsed.sql)
            .await
        {
            Ok(()) => Ok(ActionOutcome::Done {
                message: Some(format!("view {} replaced", self.qualified())),
            }),
            // Postgres' own complaint — a missing table, an unknown
            // column, a changed column list, a missing privilege — is
            // exactly what the user needs to see, next to the text that
            // caused it.
            Err(message) => Ok(Self::reject(&buffer, &message)),
        }
    }
}

/// How postgres swaps a definition, for the buffer header. Both halves are
/// worth saying: nothing is dropped (so dependent views and grants
/// survive), but in exchange the result columns are fixed.
const POSTGRES_REPLACE_NOTE: &str = "On save the definition is replaced in place — nothing is dropped, so\n\
     dependent views and privileges survive.\n\
     The result columns have to stay as they are; postgres only allows\n\
     appending new ones. Restructure a view from a DB script instead.";

#[cfg(test)]
mod pagination_tests {
    use super::*;

    fn page(offset: u32, limit: u32) -> PageRequest {
        PageRequest { offset, limit }
    }

    #[test]
    fn wraps_simple_select() {
        let got = wrap_for_pagination("SELECT * FROM t", page(0, 100)).unwrap();
        assert_eq!(
            got,
            "SELECT * FROM (SELECT * FROM t) AS _nyd_pg LIMIT 101 OFFSET 0"
        );
    }

    #[test]
    fn wraps_select_with_quoted_identifiers() {
        let q = r#"SELECT * FROM "public"."01_Sample_Item";"#;
        let got = wrap_for_pagination(q, page(0, 100));
        assert!(
            got.is_some(),
            "quoted identifiers must not block pagination"
        );
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
        assert!(
            got.is_some(),
            "single-statement SELECT with quoted ; should wrap"
        );
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

/// Wiring tests for the DB-Scripts branch: the branch itself (nodes,
/// actions, CRUD) is tested in `not_yet_done_sql_core::db_script_nodes`;
/// what has to hold *here* is that this adapter hangs it off the right
/// parents, under the `postgres:` type prefix, and reachable through
/// `childs`. No Postgres client required — the branch is filesystem-only.
#[cfg(test)]
mod db_script_tree_tests {
    use super::*;
    use not_yet_done_content::children;
    use std::path::{Path, PathBuf};

    /// Build an offline [`PostgresAdapter`] whose `instance_data_dir()`
    /// resolves to a fresh unique directory, and return both. The
    /// db-script listing is filesystem-only, so no live Postgres client
    /// is needed — the adapter's `childs` fetchers read the returned dir.
    fn build_adapter() -> (PostgresAdapter, PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        // Unique instance id → unique per-instance data dir (the default
        // `<data_local>/not_yet_done/postgres/<instance_id>` layout).
        let instance_id = format!("test-{nanos}-{n}-{}", std::process::id());
        let client = crate::client::PostgresClient::new(
            not_yet_done_transport::TransportConfig {
                mode: not_yet_done_transport::TransportMode::Direct,
                ssh: vec![],
                target: not_yet_done_transport::Endpoint {
                    host: "db.invalid".into(),
                    port: 5432,
                },
            },
            crate::config::PostgresAuth {
                user: "u".into(),
                password: not_yet_done_content::CredentialProvider::Literal { value: "x".into() },
                admin_database: "postgres".into(),
                sslmode: crate::config::SslMode::Disable,
            },
            None,
            None,
        );
        let adapter =
            PostgresAdapter::from_client(Arc::new(client), "test-conn".into(), instance_id, None);
        let dir = adapter.instance_data_dir();
        std::fs::create_dir_all(&dir).unwrap();
        (adapter, dir)
    }

    // Build ListParams for a given child type. Uses the canonical
    // NodeType constructor (not a stripped-down placeholder) so it
    // matches by full-struct equality inside `children::list`, which
    // locates the `Child` by `NodeType == NodeType` rather than only
    // the `type_id` the removed `Node::list` match arms keyed on.
    fn list_params(node_type: NodeType) -> ListParams {
        ListParams {
            node_type,
            query: None,
            sort: Vec::new(),
            page: None,
            download: false,
            group_by: None,
        }
    }

    /// Walk root → database → `db_scripts`, the same route `get_by_id`
    /// takes. Proves the group node is reachable from the catalogue side.
    async fn group_of(adapter: &PostgresAdapter, database: &str) -> Box<dyn Node> {
        adapter
            .get_by_id(&format!("{database}/{DB_SCRIPTS_GROUP_ID}"))
            .await
            .expect("db_scripts group")
    }

    #[tokio::test]
    async fn database_walk_reaches_the_group_under_the_postgres_prefix() {
        let (adapter, tmp) = build_adapter();
        let g = group_of(&adapter, "mydb").await;
        assert_eq!(g.id(), "mydb/db_scripts");
        assert_eq!(g.node_type().type_id, "postgres:db_scripts");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn group_lists_dirs_and_scripts_separately() {
        let (adapter, tmp) = build_adapter();
        // Layout: root has a folder `util`, a SQL script, and a Python
        // script — the second one verifies that the listing is no longer
        // gated on the `.sql` extension.
        crate::query::create_db_script_dir(&tmp, "mydb", Path::new("util"))
            .await
            .unwrap();
        crate::query::write_db_script(&tmp, "mydb", "audit.sql", "SELECT 1;")
            .await
            .unwrap();
        crate::query::write_db_script(&tmp, "mydb", "migrate.py", "print('hi')")
            .await
            .unwrap();
        let g = group_of(&adapter, "mydb").await;
        let types = adapter.db_scripts.types();

        let dirs = children::list(&adapter, g.as_ref(), list_params(types.dir.clone()))
            .await
            .unwrap();
        let dir_names: Vec<&str> = dirs.items.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(dir_names, vec!["util"]);
        assert_eq!(dirs.items[0].node_type.type_id, "postgres:db_script_dir");

        let scripts = children::list(&adapter, g.as_ref(), list_params(types.script.clone()))
            .await
            .unwrap();
        let script_names: Vec<&str> = scripts.items.iter().map(|n| n.label.as_str()).collect();
        // Labels carry the extension so the user sees what type each
        // file is at a glance.
        assert_eq!(script_names, vec!["audit.sql", "migrate.py"]);
        assert_eq!(scripts.items[0].node_type.type_id, "postgres:db_script");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn dir_node_lists_nested_children_with_full_ids() {
        let (adapter, tmp) = build_adapter();
        crate::query::create_db_script_dir(&tmp, "mydb", Path::new("util/inner"))
            .await
            .unwrap();
        crate::query::write_db_script(&tmp, "mydb", "util/helper.sql", "SELECT 1;")
            .await
            .unwrap();
        let g = group_of(&adapter, "mydb").await;
        let dir_node_box = g.get_child("util").await.unwrap();
        let types = adapter.db_scripts.types();

        let scripts = children::list(
            &adapter,
            dir_node_box.as_ref(),
            list_params(types.script.clone()),
        )
        .await
        .unwrap();
        assert_eq!(scripts.items.len(), 1);
        assert_eq!(scripts.items[0].label, "helper.sql");
        // Node id encodes the full path so the segment walker can later
        // resolve it back via root → database → db_scripts → util → helper.sql.
        assert_eq!(scripts.items[0].id, "mydb/db_scripts/util/helper.sql");

        let dirs = children::list(
            &adapter,
            dir_node_box.as_ref(),
            list_params(types.dir.clone()),
        )
        .await
        .unwrap();
        assert_eq!(dirs.items.len(), 1);
        assert_eq!(dirs.items[0].label, "inner");
        assert_eq!(dirs.items[0].id, "mydb/db_scripts/util/inner");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The database level also offers its scripts flat, skipping the
    /// group node — a view spec can drill straight from database to
    /// script. Folders never show up in that list.
    #[tokio::test]
    async fn database_offers_a_flat_script_shortcut() {
        let (adapter, tmp) = build_adapter();
        crate::query::create_db_script_dir(&tmp, "mydb", Path::new("util"))
            .await
            .unwrap();
        crate::query::write_db_script(&tmp, "mydb", "audit.sql", "SELECT 1;")
            .await
            .unwrap();
        crate::query::write_db_script(&tmp, "mydb", "util/deep.sql", "SELECT 1;")
            .await
            .unwrap();
        let db = adapter.get_by_id("mydb").await.unwrap();
        let types = adapter.db_scripts.types();

        let flat = children::list(&adapter, db.as_ref(), list_params(types.script.clone()))
            .await
            .unwrap();
        assert_eq!(flat.items.len(), 1);
        assert_eq!(flat.items[0].id, "mydb/db_scripts/audit.sql");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// `actions_for_type` must answer for the branch's three types
    /// without a node walk — that's what renders the shortcut hints.
    #[tokio::test]
    async fn actions_for_type_covers_the_branch_and_the_catalogue() {
        let (adapter, tmp) = build_adapter();
        let types = adapter.db_scripts.types();
        let ids = |nt: &NodeType| -> Vec<String> {
            adapter
                .actions_for_type(nt)
                .into_iter()
                .map(|a| a.id)
                .collect()
        };
        assert!(ids(&types.group).iter().any(|n| n == "add-script"));
        assert!(ids(&types.dir).iter().any(|n| n == "delete-dir"));
        assert!(ids(&types.script).iter().any(|n| n == "execute"));
        assert!(ids(&table_node_type()).iter().any(|n| n == "edit_sql"));
        // A row carries exactly one action: its own editor. Not `edit_sql`
        // — the query editor belongs to the relation, and a row config
        // reaches it via `parent:edit_sql`.
        assert_eq!(ids(&row_node_type()), vec![EDIT_ROW_ACTION.to_string()]);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The definition editor is what makes a view its own node type, so it
    /// has to be offered on views and only there. A view keeps the table's
    /// `edit_sql` too — it is queryable the same way.
    #[tokio::test]
    async fn only_a_view_offers_the_definition_editor() {
        let (adapter, tmp) = build_adapter();
        let ids = |nt: &NodeType| -> Vec<String> {
            adapter
                .actions_for_type(nt)
                .into_iter()
                .map(|a| a.id)
                .collect()
        };
        assert!(
            ids(&view_node_type())
                .iter()
                .any(|id| id == EDIT_VIEW_ACTION)
        );
        assert!(ids(&view_node_type()).iter().any(|id| id == "edit_sql"));
        assert!(
            !ids(&table_node_type())
                .iter()
                .any(|id| id == EDIT_VIEW_ACTION)
        );
        // …and it has to be an editor action: bound from YAML as
        // `type: edit`, never as a `shortcuts:` entry.
        let editor = adapter
            .actions_for_type(&view_node_type())
            .into_iter()
            .find(|a| a.id == EDIT_VIEW_ACTION)
            .expect("the view offers its definition editor");
        assert!(matches!(editor.input, InputSpec::Editor));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Unit-level round trip for the catalogue branch's composition
    /// (`*Node::new` composes the id purely — no Postgres round trip).
    #[test]
    fn table_node_id_round_trips() {
        let (adapter, tmp) = build_adapter();
        let schema = SchemaNode::new("mydb".into(), "public".into(), Arc::clone(&adapter.client));
        assert_eq!(schema.id(), "mydb/schemas/public");
        assert_eq!(schema.label(), "public");
        let table = TableNode::new(
            "mydb".into(),
            "public".into(),
            "users".into(),
            Arc::clone(&adapter.client),
        );
        assert_eq!(table.id(), "mydb/schemas/public/tables/users");
        assert_eq!(table.label(), "users");
        let view = ViewNode::new(
            "mydb".into(),
            "public".into(),
            "v_balance".into(),
            Arc::clone(&adapter.client),
        );
        assert_eq!(view.id(), "mydb/schemas/public/views/v_balance");
        assert_eq!(view.label(), "v_balance");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A view is reachable through its own group segment, and the walk is
    /// lazy — no catalogue query is needed to address one, which is also
    /// what lets the tests below run against an unreachable database.
    #[tokio::test]
    async fn a_view_is_addressable_through_its_own_group() {
        let (adapter, tmp) = build_adapter();
        let node = adapter
            .get_by_id("mydb/schemas/public/views/v_balance")
            .await
            .expect("the views group resolves");
        assert_eq!(node.node_type().type_id, "postgres:view");
        assert_eq!(node.id(), "mydb/schemas/public/views/v_balance");
        // The group itself, too — a YAML view can list it as a folder.
        let group = adapter
            .get_by_id("mydb/schemas/public/views")
            .await
            .expect("the views group node resolves");
        assert_eq!(group.node_type().type_id, "postgres:views");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Rows of a view are minted under `views/<v>`, not under `tables/` —
    /// otherwise `get_by_id` would walk a row back to a table that may not
    /// even exist. Only the id shape is asserted here; fetching the rows
    /// themselves needs a live server.
    #[test]
    fn view_rows_stay_under_the_view_id() {
        let (_adapter, tmp) = build_adapter();
        let summaries = relation_summaries(
            vec![TableEntry {
                database: "mydb".into(),
                schema: "public".into(),
                name: "v_balance".into(),
                kind: RelationKind::View,
                owner: "u".into(),
                estimated_rows: -1,
            }],
            RelationKind::View,
        );
        assert_eq!(summaries[0].id, "mydb/schemas/public/views/v_balance");
        assert_eq!(summaries[0].node_type.type_id, "postgres:view");
        // `reltuples` is meaningless for a view — the cell stays empty
        // rather than claiming -1 rows.
        let estimated = summaries[0]
            .metadata
            .fields
            .iter()
            .find(|f| f.key == "estimated_rows")
            .expect("the column exists for both kinds");
        assert_eq!(estimated.value, "");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Everything a save can be rejected for, before any statement
    /// reaches the database: the buffer comes back with a banner and the
    /// user's own text, never an error that loses it.
    ///
    /// These run against an unreachable host on purpose — each rejection
    /// has to happen *before* the network, and the test proves it does by
    /// completing at all.
    #[tokio::test]
    async fn rejections_reopen_the_buffer_before_touching_the_database() {
        let (adapter, tmp) = build_adapter();
        let stored = "CREATE OR REPLACE VIEW \"public\".\"v_balance\" AS\n SELECT 1;";
        let save = |sql: &str| {
            let mut node = ViewNode::new(
                "mydb".into(),
                "public".into(),
                "v_balance".into(),
                Arc::clone(&adapter.client),
            );
            let text = view_ddl::edit_buffer("public.v_balance", sql, POSTGRES_REPLACE_NOTE);
            async move {
                node.execute(
                    EDIT_VIEW_ACTION,
                    ActionInput::Edited {
                        text,
                        original: stored.to_string(),
                        version: stored.to_string(),
                    },
                )
                .await
                .expect("a rejected save is an outcome, not an error")
            }
        };

        for (sql, needle) in [
            // A rename would leave the old view in place beside a new one.
            (
                "CREATE OR REPLACE VIEW public.v_balance_renamed AS SELECT 1",
                "Rename it back",
            ),
            // A second statement would change something else entirely.
            (
                "CREATE OR REPLACE VIEW public.v_balance AS SELECT 1; DROP TABLE t;",
                "second statement",
            ),
            // Another schema is another object, same as another name.
            (
                "CREATE OR REPLACE VIEW other.v_balance AS SELECT 1",
                "different schema",
            ),
            // Unqualified, the target depends on the session search_path.
            (
                "CREATE OR REPLACE VIEW v_balance AS SELECT 1",
                "search_path",
            ),
        ] {
            match save(sql).await {
                ActionOutcome::Reopen {
                    content,
                    new_version,
                } => {
                    assert!(content.contains(needle), "{sql} → {content}");
                    // The user's own statement survives above the banner.
                    assert!(content.contains("SELECT 1"), "{sql} → {content}");
                    assert!(new_version.is_none(), "a rejection keeps the old token");
                }
                other => panic!("{sql} should reopen, got {}", outcome_name(&other)),
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Saving an untouched buffer must not replace the view: postgres
    /// re-prints the definition without the trailing `;` the buffer ends
    /// in, so comparing verbatim would report a change on every save.
    #[tokio::test]
    async fn an_untouched_buffer_reports_no_changes() {
        let (adapter, tmp) = build_adapter();
        let stored = "CREATE OR REPLACE VIEW \"public\".\"v_balance\" AS\n SELECT 1";
        let mut node = ViewNode::new(
            "mydb".into(),
            "public".into(),
            "v_balance".into(),
            Arc::clone(&adapter.client),
        );
        let outcome = node
            .execute(
                EDIT_VIEW_ACTION,
                ActionInput::Edited {
                    text: view_ddl::edit_buffer("public.v_balance", stored, POSTGRES_REPLACE_NOTE),
                    original: stored.to_string(),
                    version: stored.to_string(),
                },
            )
            .await
            .expect("outcome");
        assert!(
            matches!(outcome, ActionOutcome::NoChanges),
            "got {}",
            outcome_name(&outcome)
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ── The row editor ──────────────────────────────────────────────────
    //
    // Reading a row needs a live server, so what is testable offline is the
    // addressing and everything the save decides *before* the network. That
    // is also where the interesting contract sits: a rejected save must come
    // back as a buffer, and the statement must address the row the editor
    // opened on rather than whatever now sits at its offset.

    /// The row's own snapshot and the keys that address it — the pair a
    /// session token carries.
    fn row_session() -> (Vec<(String, Option<String>)>, RowSnapshot) {
        use not_yet_done_sql_core::RowCell;
        (
            vec![("id".to_string(), Some("1".to_string()))],
            RowSnapshot::new(vec![
                RowCell::editable("id", Some("1".into())),
                RowCell::editable("email", Some("someone@example.invalid".into())),
                RowCell::editable("note", None),
            ]),
        )
    }

    fn users_row(adapter: &PostgresAdapter, offset: u32) -> RowNode {
        RowNode::new(
            "mydb".into(),
            "public".into(),
            TABLES_GROUP_ID.into(),
            "users".into(),
            offset,
            Arc::clone(&adapter.client),
        )
    }

    /// A row id has to walk through its `rows` waypoint: the edit session
    /// resolves the node it opened by id, so without that segment the whole
    /// editor would be unreachable.
    #[tokio::test]
    async fn a_row_id_resolves_through_its_waypoint() {
        let (adapter, tmp) = build_adapter();
        let node = adapter
            .get_by_id("mydb/schemas/public/tables/users/rows/7")
            .await
            .expect("a row resolves without touching the database");
        assert_eq!(node.node_type().type_id, "postgres:row");
        assert_eq!(node.id(), "mydb/schemas/public/tables/users/rows/7");

        // A view's rows stay under `views/`, so the walk never lands on a
        // table of the same name.
        let of_view = adapter
            .get_by_id("mydb/schemas/public/views/v_balance/rows/0")
            .await
            .expect("a view row resolves too");
        assert_eq!(of_view.id(), "mydb/schemas/public/views/v_balance/rows/0");

        // The waypoint itself is addressable — that is what makes the walk
        // work — but it is not a row.
        let waypoint = adapter
            .get_by_id("mydb/schemas/public/tables/users/rows")
            .await
            .expect("the waypoint resolves");
        assert_eq!(waypoint.node_type().type_id, "postgres:rows");

        // Anything that is not an offset is not a row.
        assert!(
            adapter
                .get_by_id("mydb/schemas/public/tables/users/rows/last")
                .await
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Both halves of the seam answer only their own action, so a
    /// misconfigured `id:` in a view YAML says what is wrong instead of
    /// opening an editor on nothing.
    #[tokio::test]
    async fn a_row_answers_only_its_own_editor_action() {
        let (adapter, tmp) = build_adapter();
        let mut node = users_row(&adapter, 0);
        assert!(node.prepare("edit_full").await.is_err());
        assert!(
            node.execute(
                "edit_full",
                ActionInput::Edited {
                    text: String::new(),
                    original: String::new(),
                    version: String::new(),
                },
            )
            .await
            .is_err()
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Without the token there are no key values, so there is no row to
    /// write to — that is an error, not a rejection: the buffer cannot be
    /// fixed by editing it.
    #[tokio::test]
    async fn a_save_without_its_session_token_is_refused() {
        let (adapter, tmp) = build_adapter();
        let mut node = users_row(&adapter, 0);
        let outcome = node
            .execute(
                EDIT_ROW_ACTION,
                ActionInput::Edited {
                    text: "id: 1\n".into(),
                    original: String::new(),
                    version: "not a token".into(),
                },
            )
            .await;
        assert!(outcome.is_err(), "a lost session cannot be saved");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Everything the save can refuse before the network, against an
    /// unreachable host on purpose: each case has to be decided from the
    /// buffer alone, and the test proves it is by completing at all.
    #[tokio::test]
    async fn rejections_reopen_the_row_buffer_before_touching_the_database() {
        let (adapter, tmp) = build_adapter();
        let (keys, row) = row_session();
        let version = row_edit::version_token(&keys, &row);

        for (buffer, needle) in [
            // Not YAML at all.
            ("id: 1\n  email: broken\n", "line"),
            // A column this row has not got — a typo, most likely.
            ("id: 1\nemial: someone@example.invalid\n", "emial"),
        ] {
            let mut node = users_row(&adapter, 0);
            let outcome = node
                .execute(
                    EDIT_ROW_ACTION,
                    ActionInput::Edited {
                        text: buffer.to_string(),
                        original: String::new(),
                        version: version.clone(),
                    },
                )
                .await
                .expect("a rejected save is an outcome, not an error");
            match outcome {
                ActionOutcome::Reopen {
                    content,
                    new_version,
                } => {
                    assert!(content.contains(needle), "{buffer} → {content}");
                    // The user's own text survives above the banner.
                    assert!(content.contains("id: 1"), "{buffer} → {content}");
                    assert!(new_version.is_none(), "a rejection keeps the old token");
                }
                other => panic!("{buffer} should reopen, got {}", outcome_name(&other)),
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Saving an untouched row must not build a statement at all — the
    /// unreachable host is what proves it does not.
    #[tokio::test]
    async fn an_untouched_row_buffer_reports_no_changes() {
        let (adapter, tmp) = build_adapter();
        let (keys, row) = row_session();
        let mut node = users_row(&adapter, 0);
        let outcome = node
            .execute(
                EDIT_ROW_ACTION,
                ActionInput::Edited {
                    text: row_edit::edit_buffer("Row 0", "keyed by id", POSTGRES_WRITE_NOTE, &row),
                    original: String::new(),
                    version: row_edit::version_token(&keys, &row),
                },
            )
            .await
            .expect("outcome");
        assert!(
            matches!(outcome, ActionOutcome::NoChanges),
            "got {}",
            outcome_name(&outcome)
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The statement is schema-qualified and quoted, and it addresses the
    /// row by the key values the editor read — not by the offset in the id,
    /// which is only how the row was found.
    #[test]
    fn the_update_addresses_the_row_it_opened_on() {
        let (adapter, tmp) = build_adapter();
        let node = users_row(&adapter, 42);
        assert_eq!(node.qualified(), "\"public\".\"users\"");

        let sql = row_edit::build_update(
            &node.qualified(),
            &[row_edit::CellChange {
                column: "email".into(),
                value: Some("other@example.invalid".into()),
            }],
            &[("id".to_string(), Some("1".to_string()))],
        );
        assert_eq!(
            sql,
            "UPDATE \"public\".\"users\" SET\n    \"email\" = 'other@example.invalid'\n  \
             WHERE \"id\" = '1'"
        );
        assert!(
            !sql.contains("42"),
            "the offset never reaches the statement"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// `ActionOutcome` has no `Debug`, so a failing assertion needs a name
    /// by hand.
    fn outcome_name(outcome: &ActionOutcome) -> &'static str {
        match outcome {
            ActionOutcome::Done { .. } => "Done",
            ActionOutcome::Reopen { .. } => "Reopen",
            ActionOutcome::NoChanges => "NoChanges",
            ActionOutcome::Navigate { .. } => "Navigate",
            ActionOutcome::OpenExternal { .. } => "OpenExternal",
            ActionOutcome::OpenEditor { .. } => "OpenEditor",
        }
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
            None,
        ))
    }

    fn adapter(client: Arc<PostgresClient>) -> PostgresAdapter {
        PostgresAdapter::from_client(client, "test-conn".into(), "test".into(), None)
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
                a.child_process_env(&nref)
                    .get("PGSSLMODE")
                    .map(String::as_str),
                Some(expect)
            );
        }
    }
}
