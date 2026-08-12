//! SQLite `ContentAdapter`: navigates
//!   root → database (one per file) → "Tables"/"Views" → table/view → rows.
//!
//! One level flatter than the Postgres adapter, because SQLite has no
//! schema namespace. The intermediate group nodes have no state of their
//! own — they exist for visual structure and to leave room for further
//! siblings (Indexes, Triggers, …) later. As in the Postgres adapter, a
//! `DatabaseNode` also lists tables and views directly, so a YAML view can
//! drill through without the group node; the ids stay the same either way
//! (`<key>/tables/<name>`, `<key>/views/<name>`), which is what keeps
//! `get_by_id`'s walker working from both routes.
//!
//! Views are their own node type rather than tables with a `kind` field:
//! they browse the same way but they can be *edited*, and an action that
//! only makes sense for half the nodes of a type is an action a view config
//! cannot bind cleanly. `sqlite:view` therefore carries the `edit_view`
//! editor action, `sqlite:table` does not.
//!
//! Each database also carries a second branch beside "Tables": the
//! editable `.sql` files of [`DbScriptTree`], shared verbatim with the
//! Postgres adapter. Everything about that branch — node types (prefixed
//! `sqlite:`), actions, on-disk layout — comes from `sql-core`; this
//! adapter only says where it hangs and which key addresses it.
//!
//! `<key>` is not the file path — see [`crate::sources::source_key`] for
//! why, and for what it is instead.

use std::sync::Arc;

use async_trait::async_trait;

use not_yet_done_content::script_buffer;
use not_yet_done_content::{
    ActionInput, ActionOutcome, AdapterCapabilities, AdapterStatus, ContentAdapter, ContentError,
    CustomQueryContext, CustomQueryResult, EditorPrep, ListParams, ListResult, Metadata,
    MetadataField, Node, NodeRef, NodeSummary, NodeType, PageInfo, Result,
};

use not_yet_done_sql_core::db_script_nodes::{DB_SCRIPTS_GROUP_ID, DbScriptTree};
use not_yet_done_sql_core::row_edit::{self, RowSnapshot};
use not_yet_done_sql_core::script_completions::{self as completions, Completion};
use not_yet_done_sql_core::view_ddl;

use crate::client::{DatabaseEntry, RowKeySpec, SqliteClient, TableEntry};

mod factory;

pub use factory::SqliteAdapterFactory;

/// Fixed id segment of the "Tables" group node.
pub(crate) const TABLES_GROUP_ID: &str = "tables";
/// Fixed id segment of the "Views" group node.
pub(crate) const VIEWS_GROUP_ID: &str = "views";
/// Fixed id segment introducing a row offset.
const ROWS_GROUP_ID: &str = "rows";
/// Rows per page when a view asks for no explicit page.
const DEFAULT_PAGE_SIZE: u32 = 100;
/// Action id of the view-definition editor. Bound in a view config as
/// `actions: [{type: edit, id: edit_view}]`.
const EDIT_VIEW_ACTION: &str = "edit_view";
/// Action id of the row editor. Bound in a view config as
/// `actions: [{type: edit, id: edit_row}]`.
const EDIT_ROW_ACTION: &str = "edit_row";
/// `sqlite_master.type` of the two catalogue objects this adapter lists.
const KIND_TABLE: &str = "table";
const KIND_VIEW: &str = "view";

pub struct SqliteAdapter {
    client: Arc<SqliteClient>,
    connection_name: String,
    instance_id: String,
    /// The `Scripts` branch below each database — nodes, actions and the
    /// [`SqlScriptStore`](not_yet_done_sql_core::SqlScriptStore) they
    /// write through, all shared with the other SQL adapters. Held as an
    /// `Arc` so every node in the branch borrows the same store instead
    /// of rebuilding one per action.
    db_scripts: Arc<DbScriptTree>,
}

impl SqliteAdapter {
    pub(crate) fn from_client(
        client: Arc<SqliteClient>,
        connection_name: String,
        instance_id: String,
    ) -> Self {
        // Resolve the same per-instance data dir the trait's default
        // `instance_data_dir()` produces, so the store and the adapter
        // agree on where scripts live. We can't call the method before the
        // struct exists, so mirror its layout here.
        let instance_data_dir = dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("not_yet_done")
            .join("sqlite")
            .join(&instance_id);
        let db_scripts = Arc::new(DbScriptTree::new(
            crate::script_store::sqlite_script_store(instance_data_dir),
            "sqlite",
        ));
        Self {
            client,
            connection_name,
            instance_id,
            db_scripts,
        }
    }

    /// Editor completions for one database file: every table *and view*
    /// it holds, as a single-level `tt_<name>` token expanding to
    /// `"<name>"`. There is no schema level to qualify with — the file
    /// *is* the namespace — and a view is as selectable as a table, so
    /// both belong in the list.
    ///
    /// A file that cannot be opened yields no completions rather than an
    /// error: the editor is expected to open either way.
    async fn completion_tables(&self, key: &str) -> Vec<Completion> {
        self.client
            .list_tables(key)
            .await
            .unwrap_or_default()
            .iter()
            .map(|table| completions::qualified_table(&[&table.name]))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// NodeType helpers
//
// Only the catalogue types live here. The three types of the Scripts
// branch come from [`DbScriptTree`], which prefixes them with this
// adapter's name at construction time; read them off
// `self.db_scripts.types()`.
// ---------------------------------------------------------------------------

fn root_node_type() -> NodeType {
    NodeType {
        type_id: "sqlite:root".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "SQLite Root".into(),
    }
}

fn database_node_type() -> NodeType {
    NodeType {
        type_id: "sqlite:database".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Database".into(),
    }
}

fn tables_group_node_type() -> NodeType {
    NodeType {
        type_id: "sqlite:tables".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Tables".into(),
    }
}

fn table_node_type() -> NodeType {
    NodeType {
        type_id: "sqlite:table".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Table".into(),
    }
}

fn views_group_node_type() -> NodeType {
    NodeType {
        type_id: "sqlite:views".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Views".into(),
    }
}

fn view_node_type() -> NodeType {
    NodeType {
        type_id: "sqlite:view".into(),
        mime_type: "".into(),
        // The one editable catalogue object: its whole content is SQL, so
        // an editor opened on it should highlight SQL.
        syntax: Some("sql".into()),
        file_extension: ".sql".into(),
        display_name: "View".into(),
    }
}

fn row_node_type() -> NodeType {
    NodeType {
        type_id: "sqlite:row".into(),
        // The row editor's buffer is a YAML mapping, so an editor that
        // picks its syntax from the node gets YAML highlighting.
        mime_type: "application/yaml".into(),
        syntax: Some("yaml".into()),
        file_extension: ".yaml".into(),
        display_name: "Row".into(),
    }
}

/// Addressing waypoint for the `rows` segment of a row id
/// (`<key>/tables/<t>/rows/<offset>`).
///
/// Never rendered and never bound in a view config: rows are listed
/// through the table's own fetcher in [`SqliteAdapter::childs`], and this
/// type exists only so `get_by_id` can walk *to* a row — which is what
/// the row editor needs, since an edit session resolves its node by id.
fn rows_group_node_type() -> NodeType {
    NodeType {
        type_id: "sqlite:rows".into(),
        mime_type: "".into(),
        syntax: None,
        file_extension: "".into(),
        display_name: "Rows".into(),
    }
}

// ---------------------------------------------------------------------------
// Metadata helpers
// ---------------------------------------------------------------------------

fn database_metadata(entry: &DatabaseEntry) -> Metadata {
    Metadata {
        fields: vec![
            field("name", "Name", &entry.label),
            field("path", "Path", &entry.path.display().to_string()),
            field("size", "Size", &format_size(entry.size_bytes)),
        ],
    }
}

fn table_metadata(entry: &TableEntry) -> Metadata {
    Metadata {
        fields: vec![
            field("name", "Name", &entry.name),
            field("database", "Database", &entry.database),
            field("kind", "Kind", &entry.kind),
            field(
                "estimated_rows",
                "Rows (est.)",
                &entry
                    .estimated_rows
                    .map(|n| n.to_string())
                    // No ANALYZE has run. An empty cell is the honest
                    // rendering; "0" would claim the table is empty.
                    .unwrap_or_default(),
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

/// Actions a table row offers. Only the query editor: the tree is a
/// browser, and everything else a table could do (drop, rename, …) would
/// need a writable adapter first.
///
/// The YAML binds this via `shortcuts:` — `Q: edit_sql` on the
/// `sqlite:table` level, `Q: parent:edit_sql` on `sqlite:row` so the key
/// keeps working one level deeper.
fn table_actions() -> Vec<not_yet_done_content::NodeAction> {
    vec![not_yet_done_content::NodeAction::new(
        "edit_sql",
        "sql",
        not_yet_done_content::InputSpec::None,
    )]
}

/// Actions a view offers: everything a table has, plus editing the
/// statement that *is* the view.
///
/// `edit_view` is an [`InputSpec::Editor`](not_yet_done_content::InputSpec)
/// action, so it has to be bound as an `actions:` entry of `type: edit`
/// with `id: edit_view` — not via `shortcuts:`, which routes through
/// `invoke_action` and cannot open an editor.
fn view_actions() -> Vec<not_yet_done_content::NodeAction> {
    let mut actions = table_actions();
    actions.push(not_yet_done_content::NodeAction::new(
        EDIT_VIEW_ACTION,
        "definition",
        not_yet_done_content::InputSpec::Editor,
    ));
    actions
}

/// Actions a data row offers: editing its cells.
///
/// Like `edit_view` this is an
/// [`InputSpec::Editor`](not_yet_done_content::InputSpec) action and has
/// to be bound as `actions: [{type: edit, id: edit_row}]`. Whether the
/// row can actually be written is not decided here — a view's rows carry
/// the same action and refuse when the editor opens, with the reason,
/// because "this is a view" is a far more useful answer than a key that
/// silently does nothing.
fn row_actions() -> Vec<not_yet_done_content::NodeAction> {
    vec![not_yet_done_content::NodeAction::new(
        EDIT_ROW_ACTION,
        "row",
        not_yet_done_content::InputSpec::Editor,
    )]
}

/// Human-readable file size. Binary units, one decimal from KiB up.
fn format_size(bytes: Option<u64>) -> String {
    let Some(bytes) = bytes else {
        return String::new();
    };
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[async_trait]
impl ContentAdapter for SqliteAdapter {
    fn adapter_type(&self) -> &str {
        "sqlite"
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn subscribe_status(&self) -> tokio::sync::watch::Receiver<AdapterStatus> {
        self.client.subscribe_status()
    }

    async fn root(&self) -> Result<Box<dyn Node>> {
        Ok(Box::new(SqliteRoot {
            connection_name: self.connection_name.clone(),
            client: Arc::clone(&self.client),
            db_scripts: Arc::clone(&self.db_scripts),
        }))
    }

    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
        // Composite path: `<key>` / `<key>/tables` / `<key>/tables/<t>`.
        // Walk from the root via `get_child` segment by segment so each
        // level's lookup logic stays where it belongs.
        let mut node: Box<dyn Node> = self.root().await?;
        for part in id.split('/') {
            if part.is_empty() {
                continue;
            }
            node = node.get_child(part).await?;
        }
        Ok(node)
    }

    /// Single source of truth about a node's children: everything comes
    /// from the node's full composite `id()` plus its node type, parsed
    /// exactly the way `get_by_id`'s walker consumes it. No downcast.
    fn childs<'a>(&'a self, node: &'a dyn Node) -> Vec<not_yet_done_content::Child<'a>> {
        use not_yet_done_content::Child;
        let id = node.id().to_string();
        let segs: Vec<String> = id.split('/').map(|s| s.to_string()).collect();

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
            "sqlite:root" => vec![
                child!(database_node_type(), move |_p| {
                    Box::pin(async move { list_databases_impl(&self.client).await })
                }),
                child!(table_node_type(), move |_p| {
                    Box::pin(async move { list_all_objects_impl(&self.client, KIND_TABLE).await })
                }),
                child!(view_node_type(), move |_p| {
                    Box::pin(async move { list_all_objects_impl(&self.client, KIND_VIEW).await })
                }),
            ],
            "sqlite:database" => {
                // id = `<key>`
                let key = id.clone();
                let key2 = key.clone();
                let key3 = key.clone();
                let key4 = key.clone();
                let key5 = key.clone();
                let key6 = key.clone();
                vec![
                    child!(tables_group_node_type(), move |_p| {
                        Box::pin(async move { Ok(list_tables_group_impl(&key)) })
                    }),
                    child!(table_node_type(), move |_p| {
                        Box::pin(
                            async move { list_objects_impl(&self.client, &key2, KIND_TABLE).await },
                        )
                    }),
                    child!(views_group_node_type(), move |_p| {
                        Box::pin(async move { Ok(list_views_group_impl(&key5)) })
                    }),
                    child!(view_node_type(), move |_p| {
                        Box::pin(
                            async move { list_objects_impl(&self.client, &key6, KIND_VIEW).await },
                        )
                    }),
                    child!(self.db_scripts.types().group.clone(), move |_p| {
                        Box::pin(async move { Ok(self.db_scripts.group_summary(&key3)) })
                    }),
                    // Direct shortcut past the group node, the same way
                    // `sqlite:table` is offered here beside `sqlite:tables`:
                    // a view can list a database's scripts without the
                    // intermediate folder. Flat — folders are only reachable
                    // through the group.
                    child!(self.db_scripts.types().script.clone(), move |_p| {
                        Box::pin(async move { self.db_scripts.list_scripts_flat(&key4).await })
                    }),
                ]
            }
            "sqlite:tables" => {
                // id = `<key>/tables`
                let key = segs.first().cloned().unwrap_or_default();
                vec![child!(table_node_type(), move |_p| {
                    Box::pin(async move { list_objects_impl(&self.client, &key, KIND_TABLE).await })
                })]
            }
            "sqlite:views" => {
                // id = `<key>/views`
                let key = segs.first().cloned().unwrap_or_default();
                vec![child!(view_node_type(), move |_p| {
                    Box::pin(async move { list_objects_impl(&self.client, &key, KIND_VIEW).await })
                })]
            }
            // id = `<key>/tables/<t>` resp. `<key>/views/<v>`. Rows read the
            // same way from either; only the id prefix differs, and that is
            // the group segment the id already carries.
            "sqlite:table" | "sqlite:view" => {
                let key = segs.first().cloned().unwrap_or_default();
                let group = segs.get(1).cloned().unwrap_or_default();
                let table = segs.get(2).cloned().unwrap_or_default();
                vec![child!(row_node_type(), move |p: ListParams| {
                    Box::pin(
                        async move { list_rows_impl(&self.client, &key, &group, &table, &p).await },
                    )
                })]
            }
            "sqlite:db_scripts" | "sqlite:db_script_dir" => {
                // id = `<key>/db_scripts` (group) or
                //      `<key>/db_scripts/<rel_path...>` (dir).
                let key = segs.first().cloned().unwrap_or_default();
                let rel_path = if segs.len() > 2 {
                    segs[2..].join("/")
                } else {
                    String::new()
                };
                let key2 = key.clone();
                let rel2 = rel_path.clone();
                vec![
                    child!(self.db_scripts.types().dir.clone(), move |_p| {
                        Box::pin(async move {
                            self.db_scripts
                                .list_entries(&key, &rel_path, true, false)
                                .await
                        })
                    }),
                    child!(self.db_scripts.types().script.clone(), move |_p| {
                        Box::pin(async move {
                            self.db_scripts
                                .list_entries(&key2, &rel2, false, true)
                                .await
                        })
                    }),
                ]
            }
            // Leaves: sqlite:row, sqlite:db_script.
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
            // A row id is an offset into one `SELECT *` page — it means
            // nothing once the table changes, so rows cannot be linked or
            // marked.
            unstable_node_ids: true,
        }
    }

    /// Every id this adapter mints starts with the source key, which is
    /// exactly the routing key a custom query needs: it says which
    /// database file to run against.
    fn custom_query_context(&self, node_id: &str) -> CustomQueryContext {
        let database = node_id.split('/').next().unwrap_or_default();
        CustomQueryContext::new().with("database", database.to_string())
    }

    fn script_store(&self) -> Option<&dyn not_yet_done_content::ScriptStore> {
        Some(self.db_scripts.store())
    }

    fn actions_for_type(&self, node_type: &NodeType) -> Vec<not_yet_done_content::NodeAction> {
        // The Scripts branch answers for its own three types; the
        // catalogue types are this adapter's own.
        if let Some(actions) = self.db_scripts.actions_for_type(&node_type.type_id) {
            return actions;
        }
        match node_type.type_id.as_str() {
            "sqlite:table" => table_actions(),
            "sqlite:view" => view_actions(),
            "sqlite:row" => row_actions(),
            _ => Vec::new(),
        }
    }

    /// Append a trailing `-- table completions: tt_<name>, …` comment
    /// listing every table and view in the script's database file, so the
    /// user can copy a token into their SQL (expanded to the quoted
    /// identifier at execute time). Only for SQL-flavored scripts;
    /// `.py`/`.md`/… scripts get the buffer back stripped but unaugmented.
    ///
    /// `node` carries the canonical ref `sqlite/<key>/db_scripts/<script>`,
    /// so the source key is segment[1] and the SQL gate keys off the final
    /// segment's extension. The append is idempotent: a stale completion
    /// line is stripped first.
    async fn augment_editor_buffer(&self, node: &NodeRef, buffer: String) -> String {
        let stripped = completions::strip_completions_line(&buffer);
        let script = node.segments().last().unwrap_or("");
        if !not_yet_done_sql_core::script_files::is_sql_extension(script) {
            return stripped;
        }
        let Some(key) = node.segments().nth(1).filter(|s| !s.is_empty()) else {
            return stripped;
        };
        let entries = self.completion_tables(key).await;
        match completions::build_completions_line(&entries) {
            Some(line) => completions::append_completions_line(&stripped, &line),
            None => stripped,
        }
    }

    fn strip_editor_hints(&self, text: &str) -> String {
        completions::strip_completions_line(text)
    }

    /// Free-form SQL, run against the one database file the
    /// [`CustomQueryContext`]'s `database` key names. Multi-statement
    /// scripts are allowed; the last statement's rows are what comes back,
    /// mapped to `qrow:<i>` summaries.
    ///
    /// A single-statement `SELECT` is wrapped in `LIMIT`/`OFFSET` when the
    /// caller asked for a page. There is no cursor branch: a cursor buys
    /// nothing here — the database is a local file, so re-running with a
    /// higher `OFFSET` costs a page scan, not a round trip, and holding a
    /// transaction open across pages would only keep a write lock alive.
    async fn execute_custom_query(
        &self,
        query: &str,
        context: &CustomQueryContext,
    ) -> Result<CustomQueryResult> {
        let database = context.get("database").ok_or_else(|| {
            ContentError::NotSupported(
                "sqlite execute_custom_query needs a `database` context field".into(),
            )
        })?;
        if context.cursor.is_some() {
            return Err(ContentError::NotSupported(
                "sqlite has no cursor pagination; use `pagination: mode: server`".into(),
            ));
        }

        // Expand `tt_<table>` completion tokens before the shape is
        // sniffed — a token sits where a table name goes, so the
        // pagination wrapper must see the resolved text. The catalogue
        // read is skipped unless the query mentions a token at all;
        // failing to read it leaves the query unchanged, and SQLite then
        // surfaces the literal token in its own error.
        let owned_query;
        let query_ref = if completions::may_contain_tokens(query) {
            let entries = self.completion_tables(database).await;
            owned_query = completions::substitute_tokens(query, &entries);
            owned_query.as_str()
        } else {
            query
        };

        // `None` = not a paginable shape (DML/DDL or multi-statement); run
        // it verbatim and report no page.
        let (effective_query, page_request) = match context.page {
            Some(req) => match not_yet_done_sql_core::sql_shape::wrap_for_pagination(
                query_ref,
                req.limit,
                req.offset,
                "_nyd_sqlite",
            ) {
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

        // The wrapper asked for `limit + 1` rows; the extra one only exists
        // to answer "is there a next page".
        let (rows, page_info) = match page_request {
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

        Ok(CustomQueryResult {
            items: rows_to_summaries(&outcome.columns, &rows),
            columns: outcome.columns,
            status: outcome.status,
            page: page_info,
            cursor_id: None,
        })
    }

    /// Re-glob `sources:` and drop every open handle, so a reload notices
    /// files that appeared, moved or vanished since the tab was opened.
    async fn refresh(&self) -> Result<()> {
        self.client.invalidate().await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Shared listing logic. One implementation per level, called by the
// `childs` fetchers above.
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

/// `sqlite:database` list under root — one entry per matched file.
async fn list_databases_impl(client: &SqliteClient) -> Result<ListResult> {
    let entries = client
        .list_databases()
        .await
        .map_err(|e| ContentError::Other(e.into()))?;
    let items = entries
        .into_iter()
        .map(|e| NodeSummary {
            id: e.key.clone(),
            label: e.label.clone(),
            node_type: database_node_type(),
            metadata: database_metadata(&e),
            has_children: None,
        })
        .collect();
    Ok(empty_list(items))
}

/// Flat list of one catalogue kind across every configured file.
async fn list_all_objects_impl(client: &SqliteClient, kind: &str) -> Result<ListResult> {
    let entries = client
        .list_all_tables()
        .await
        .map_err(|e| ContentError::Other(e.into()))?;
    Ok(empty_list(object_summaries(entries, kind)))
}

/// The single virtual "Tables" group folder under a database.
fn list_tables_group_impl(database: &str) -> ListResult {
    empty_list(vec![NodeSummary {
        id: format!("{database}/{TABLES_GROUP_ID}"),
        label: "Tables".into(),
        node_type: tables_group_node_type(),
        metadata: Metadata { fields: vec![] },
        has_children: None,
    }])
}

/// The single virtual "Views" group folder under a database.
fn list_views_group_impl(database: &str) -> ListResult {
    empty_list(vec![NodeSummary {
        id: format!("{database}/{VIEWS_GROUP_ID}"),
        label: "Views".into(),
        node_type: views_group_node_type(),
        metadata: Metadata { fields: vec![] },
        has_children: None,
    }])
}

/// Tables or views inside one database file. One catalogue read serves
/// both kinds — `sqlite_master` lists them together, and asking twice
/// would only reopen the same file.
async fn list_objects_impl(
    client: &SqliteClient,
    database: &str,
    kind: &str,
) -> Result<ListResult> {
    let entries = client
        .list_tables(database)
        .await
        .map_err(|e| ContentError::Other(e.into()))?;
    Ok(empty_list(object_summaries(entries, kind)))
}

/// Map a custom query's raw output to the `qrow:<i>` summaries the TUI's
/// query pane consumes. The id is an index into *this* result set and
/// nothing else — same shape as the Postgres adapter uses, and the reason
/// `unstable_node_ids` is set.
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

/// Keep the entries of one kind and render them as summaries of the
/// matching node type. The group segment in the id follows the kind, which
/// is what lets `get_by_id` walk back to the same node.
fn object_summaries(entries: Vec<TableEntry>, kind: &str) -> Vec<NodeSummary> {
    let (group, node_type) = if kind == KIND_VIEW {
        (VIEWS_GROUP_ID, view_node_type())
    } else {
        (TABLES_GROUP_ID, table_node_type())
    };
    entries
        .into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| NodeSummary {
            id: format!("{}/{group}/{}", e.database, e.name),
            label: e.name.clone(),
            node_type: node_type.clone(),
            metadata: table_metadata(&e),
            has_children: None,
        })
        .collect()
}

/// `sqlite:row` instances under a table or view (paginated `SELECT *`).
async fn list_rows_impl(
    client: &SqliteClient,
    database: &str,
    group: &str,
    table: &str,
    params: &ListParams,
) -> Result<ListResult> {
    let (offset, limit) = match params.page {
        Some(p) if p.limit > 0 => (p.offset, p.limit),
        _ => (0, DEFAULT_PAGE_SIZE),
    };
    let page = client
        .query_rows(database, table, offset, limit)
        .await
        .map_err(|e| ContentError::Other(e.into()))?;

    let id_prefix = format!("{database}/{group}/{table}");
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
                id: format!("{id_prefix}/{ROWS_GROUP_ID}/{row_offset}"),
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
// Nodes. Each holds only what its id is made of; all listing runs through
// the adapter's `childs` fetchers, which own the live client.
//
// The one exception is `ViewNode`: `prepare`/`execute` are methods on the
// *node*, not on the adapter, so a node that can be edited has to be able
// to reach the database itself. The client is therefore threaded down the
// catalogue branch — cheaply, since it is an `Arc` and every level shares
// the same pools.
// ---------------------------------------------------------------------------

struct SqliteRoot {
    connection_name: String,
    client: Arc<SqliteClient>,
    db_scripts: Arc<DbScriptTree>,
}

#[async_trait]
impl Node for SqliteRoot {
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
        // Constructed without touching the filesystem: during a
        // `get_by_id` walk the database node is only traversed, never
        // rendered (its summary comes from the root's fetcher, which does
        // read metadata). An unknown key surfaces as an error one level
        // down, when a listing actually asks for it.
        Ok(Box::new(DatabaseNode {
            key: id.to_string(),
            client: Arc::clone(&self.client),
            db_scripts: Arc::clone(&self.db_scripts),
        }))
    }
}

struct DatabaseNode {
    key: String,
    client: Arc<SqliteClient>,
    db_scripts: Arc<DbScriptTree>,
}

#[async_trait]
impl Node for DatabaseNode {
    fn id(&self) -> &str {
        &self.key
    }

    fn label(&self) -> &str {
        &self.key
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
            TABLES_GROUP_ID => Ok(Box::new(TablesGroupNode {
                database: self.key.clone(),
                client: Arc::clone(&self.client),
                node_id: format!("{}/{TABLES_GROUP_ID}", self.key),
            })),
            VIEWS_GROUP_ID => Ok(Box::new(ViewsGroupNode {
                database: self.key.clone(),
                client: Arc::clone(&self.client),
                node_id: format!("{}/{VIEWS_GROUP_ID}", self.key),
            })),
            DB_SCRIPTS_GROUP_ID => Ok(DbScriptTree::group_node(&self.db_scripts, &self.key)),
            other => Err(ContentError::NotFound(other.into())),
        }
    }
}

struct TablesGroupNode {
    database: String,
    client: Arc<SqliteClient>,
    /// Full composite id `<key>/tables`.
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
        Ok(Box::new(TableNode::new(
            self.database.clone(),
            id.to_string(),
            Arc::clone(&self.client),
        )))
    }
}

struct TableNode {
    database: String,
    name: String,
    client: Arc<SqliteClient>,
    /// Full composite id `<key>/tables/<t>`.
    node_id: String,
}

impl TableNode {
    fn new(database: String, name: String, client: Arc<SqliteClient>) -> Self {
        let node_id = format!("{database}/{TABLES_GROUP_ID}/{name}");
        Self {
            database,
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
            TABLES_GROUP_ID,
            &self.name,
            &self.client,
        )
    }
}

/// The `rows` segment below a table or a view. Shared by both, because a
/// view's rows are addressed exactly like a table's — only the group
/// segment of the id differs.
fn rows_group_child(
    id: &str,
    database: &str,
    group: &str,
    table: &str,
    client: &Arc<SqliteClient>,
) -> Result<Box<dyn Node>> {
    if id != ROWS_GROUP_ID {
        return Err(ContentError::NotFound(id.into()));
    }
    Ok(Box::new(RowsGroupNode {
        database: database.to_string(),
        group: group.to_string(),
        table: table.to_string(),
        client: Arc::clone(client),
        node_id: format!("{database}/{group}/{table}/{ROWS_GROUP_ID}"),
    }))
}

/// Pure addressing node: it exists so `get_by_id` can walk through the
/// `rows` segment of a row id. See [`rows_group_node_type`].
struct RowsGroupNode {
    database: String,
    group: String,
    table: String,
    client: Arc<SqliteClient>,
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
            self.group.clone(),
            self.table.clone(),
            offset,
            Arc::clone(&self.client),
        )))
    }
}

struct ViewsGroupNode {
    database: String,
    client: Arc<SqliteClient>,
    /// Full composite id `<key>/views`.
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
        Ok(Box::new(ViewNode::new(
            self.database.clone(),
            id.to_string(),
            Arc::clone(&self.client),
        )))
    }
}

/// The one editable catalogue node: a view's whole content is the
/// statement that defines it, so it can be edited exactly like a stored
/// script — [`Node::prepare`] hands out the `CREATE VIEW …` text,
/// [`Node::execute`] takes the edited buffer back.
struct ViewNode {
    database: String,
    name: String,
    client: Arc<SqliteClient>,
    /// Full composite id `<key>/views/<v>`.
    node_id: String,
}

impl ViewNode {
    fn new(database: String, name: String, client: Arc<SqliteClient>) -> Self {
        let node_id = format!("{database}/{VIEWS_GROUP_ID}/{name}");
        Self {
            database,
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
            .view_definition(&self.database, &self.name)
            .await
            .map_err(|e| ContentError::Other(e.into()))
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

    /// Same `rows` waypoint as a table's, so a row of a view is
    /// addressable too. Editing one is refused when the editor opens —
    /// SQLite cannot write through a view — but browsing and scripting it
    /// works exactly as under a table.
    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        rows_group_child(id, &self.database, VIEWS_GROUP_ID, &self.name, &self.client)
    }

    /// The buffer is the stored statement verbatim, and `version` is that
    /// same text — so a concurrent change is detectable by comparison
    /// alone, without a modification timestamp SQLite does not keep.
    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        if action_id != EDIT_VIEW_ACTION {
            return Err(ContentError::NotSupported(format!(
                "a view has no editor action `{action_id}`"
            )));
        }
        let definition = self
            .stored_definition()
            .await?
            .ok_or_else(|| ContentError::NotFound(format!("view {}", self.name)))?;
        Ok(EditorPrep {
            template: view_ddl::edit_buffer(&self.name, &definition, SQLITE_REPLACE_NOTE),
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
                    self.name, parsed.name, self.name
                ),
            ));
        }
        if view_ddl::same_definition(&parsed.sql, &version) {
            return Ok(ActionOutcome::NoChanges);
        }
        if self.client.is_read_only() {
            return Ok(Self::reject(
                &buffer,
                &format!(
                    "this adapter runs read_only, so the definition of {} cannot be \
                     replaced — set read_only: false in its config to allow writes",
                    self.name
                ),
            ));
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
                            self.name
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
                            self.name
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
            .replace_view(&self.database, &self.name, &parsed.sql)
            .await
        {
            Ok(()) => Ok(ActionOutcome::Done {
                message: Some(format!("view {} replaced", self.name)),
            }),
            // SQLite's own complaint — an unknown column, a missing table
            // the lazy body reference only reveals on read — is exactly
            // what the user needs to see, next to the text that caused it.
            Err(message) => Ok(Self::reject(&buffer, &message)),
        }
    }
}

/// One data row, editable through the same seam as a view definition:
/// [`Node::prepare`] renders the row as a YAML mapping,
/// [`Node::execute`] turns the edited mapping back into one `UPDATE`.
///
/// The offset in the id is only how the row was *found* — it is the key
/// values read at `prepare` time that every statement afterwards uses, so
/// a page that shifted underneath cannot redirect the write. See
/// [`row_edit`] for the buffer protocol.
struct RowNode {
    database: String,
    table: String,
    offset: u32,
    client: Arc<SqliteClient>,
    /// Full composite id `<key>/<group>/<table>/rows/<offset>`.
    node_id: String,
}

impl RowNode {
    /// `group` (`tables` or `views`) only shapes the node's own id, so
    /// that it matches the id the listing minted for this row.
    fn new(
        database: String,
        group: String,
        table: String,
        offset: u32,
        client: Arc<SqliteClient>,
    ) -> Self {
        let node_id = format!("{database}/{group}/{table}/{ROWS_GROUP_ID}/{offset}");
        Self {
            database,
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

    fn label_for_header(&self) -> String {
        format!("Row {} of {} in {}", self.offset, self.table, self.database)
    }

    async fn key_spec(&self) -> Result<RowKeySpec> {
        self.client
            .row_key_spec(&self.database, &self.table)
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
    /// `version` token carries both the cell values (to detect a
    /// concurrent change on save) and the key values (so the `UPDATE`
    /// addresses the row that was actually shown, not whatever now sits at
    /// the same offset).
    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        if action_id != EDIT_ROW_ACTION {
            return Err(ContentError::NotSupported(format!(
                "a row has no editor action `{action_id}`"
            )));
        }
        let keys = self.key_spec().await?;
        let read = self
            .client
            .read_row_at(&self.database, &self.table, &keys, self.offset)
            .await
            .map_err(|e| ContentError::Other(e.into()))?
            .ok_or_else(|| {
                ContentError::NotFound(format!(
                    "row {} of {} — the table has fewer rows than that now",
                    self.offset, self.table
                ))
            })?;

        let row = RowSnapshot::new(read.cells);
        Ok(EditorPrep {
            template: row_edit::edit_buffer(
                &self.label_for_header(),
                &keys.note(),
                SQLITE_WRITE_NOTE,
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
    /// from a good one. When the statement itself is refused by SQLite,
    /// the statement is shown next to the complaint — an "unknown column"
    /// is far easier to place with the `UPDATE` in front of you.
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
        if self.client.is_read_only() {
            return Ok(Self::reject(
                &buffer,
                &format!(
                    "this adapter runs read_only, so no row of {} can be changed — set \
                     read_only: false in its config to allow writes",
                    self.table
                ),
            ));
        }

        let keys = self.key_spec().await?;
        let where_sql = row_edit::render_where(&key_values);

        // Re-read before writing, exactly as the view editor does: the row
        // may have changed since the editor opened, and overwriting that
        // silently would drop somebody's change without anyone noticing.
        let current = self
            .client
            .read_rows_where(&self.database, &self.table, &keys, &where_sql)
            .await
            .map_err(|e| ContentError::Other(e.into()))?;
        match current.len() {
            0 => {
                return Ok(Self::reject(
                    &buffer,
                    &format!(
                        "no row of {} matches {where_sql} any more — it was deleted, or its \
                         key changed, since this editor opened. Nothing was written.",
                        self.table
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
                        "{where_sql} matches more than one row of {}, so a single row cannot \
                         be changed through it — use a DB script with a WHERE that is unique.",
                        self.table
                    ),
                ));
            }
        }

        let statement = row_edit::build_update(
            &not_yet_done_sql_core::quote_ident(&self.table),
            &changes,
            &key_values,
        );
        match self.client.execute_write(&self.database, &statement).await {
            // The re-read above proved the key matches exactly one row, so
            // a count other than 1 would mean the row moved between the
            // two statements — rare, but the user should hear about it
            // rather than see a success message for nothing.
            Ok(1) => Ok(ActionOutcome::Done {
                message: Some(format!(
                    "{} of {} updated",
                    row_edit::plural_columns(changes.len()),
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

/// What saving a row does, for the buffer header. SQLite's type affinity
/// is the part worth saying: values are written as text and converted by
/// the column, so a number does not have to be spelled a particular way.
const SQLITE_WRITE_NOTE: &str = "On save one UPDATE is built from the columns that changed and run on its own.\n\
     Values are written as text; the column's type converts them.";

/// How SQLite swaps a definition, for the buffer header. Worth saying
/// because it sounds more dangerous than it is: there is no `ALTER VIEW`,
/// but DDL is transactional, so the drop is never observable on its own.
const SQLITE_REPLACE_NOTE: &str = "On save the view is dropped and created again in one transaction.\n\
     A definition that fails to run leaves the old view in place.";

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::Duration;

    async fn adapter_for(dir: &Path) -> (SqliteAdapter, String) {
        adapter_with(dir, true).await
    }

    /// Same fixture, but with writes allowed — what the view editor needs
    /// to get past its read-only guard.
    async fn writable_adapter_for(dir: &Path) -> (SqliteAdapter, String) {
        adapter_with(dir, false).await
    }

    /// One table and one view over it, so both catalogue branches have
    /// something to list.
    async fn adapter_with(dir: &Path, read_only: bool) -> (SqliteAdapter, String) {
        let path = dir.join("fixture.db");
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true);
        let pool = sqlx::sqlite::SqlitePool::connect_with(options)
            .await
            .expect("create db");
        sqlx::query("CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT)")
            .execute(&pool)
            .await
            .expect("create table");
        sqlx::query("INSERT INTO notes VALUES (1, 'one'), (2, 'two')")
            .execute(&pool)
            .await
            .expect("insert");
        sqlx::query("CREATE VIEW recent AS SELECT id, body FROM notes WHERE id > 1")
            .execute(&pool)
            .await
            .expect("create view");
        pool.close().await;

        let client = Arc::new(SqliteClient::new(
            vec![path.display().to_string()],
            read_only,
            Duration::from_millis(500),
            None,
        ));
        let key = client.sources().await[0].key.clone();
        (
            SqliteAdapter::from_client(client, "test".into(), "test-instance".into()),
            key,
        )
    }

    /// Saving a buffer through the generic editor seam, the way
    /// `NodeActionEditSession` does it.
    async fn save(node: &mut dyn Node, text: &str, version: &str) -> ActionOutcome {
        node.execute(
            EDIT_VIEW_ACTION,
            ActionInput::Edited {
                text: text.to_string(),
                original: String::new(),
                version: version.to_string(),
            },
        )
        .await
        .expect("execute must answer with an outcome, not an error")
    }

    #[test]
    fn format_size_switches_units() {
        assert_eq!(format_size(None), "");
        assert_eq!(format_size(Some(512)), "512 B");
        assert_eq!(format_size(Some(2048)), "2.0 KiB");
        assert_eq!(format_size(Some(5 * 1024 * 1024)), "5.0 MiB");
    }

    #[tokio::test]
    async fn get_by_id_walks_every_level() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = adapter_for(tmp.path()).await;

        let root = adapter.root().await.expect("root");
        assert_eq!(root.node_type().type_id, "sqlite:root");

        let db = adapter.get_by_id(&key).await.expect("database");
        assert_eq!(db.node_type().type_id, "sqlite:database");

        let group = adapter
            .get_by_id(&format!("{key}/tables"))
            .await
            .expect("tables group");
        assert_eq!(group.node_type().type_id, "sqlite:tables");
        assert_eq!(group.label(), "Tables");

        let table = adapter
            .get_by_id(&format!("{key}/tables/notes"))
            .await
            .expect("table");
        assert_eq!(table.node_type().type_id, "sqlite:table");
        assert_eq!(table.label(), "notes");

        let views = adapter
            .get_by_id(&format!("{key}/views"))
            .await
            .expect("views group");
        assert_eq!(views.node_type().type_id, "sqlite:views");
        assert_eq!(views.label(), "Views");

        let view = adapter
            .get_by_id(&format!("{key}/views/recent"))
            .await
            .expect("view");
        assert_eq!(view.node_type().type_id, "sqlite:view");
        assert_eq!(view.label(), "recent");
    }

    #[tokio::test]
    async fn get_by_id_rejects_an_unknown_group_segment() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = adapter_for(tmp.path()).await;
        match adapter.get_by_id(&format!("{key}/schemas")).await {
            Err(ContentError::NotFound(what)) => assert_eq!(what, "schemas"),
            Err(other) => panic!("expected NotFound, got {other:?}"),
            Ok(node) => panic!("sqlite has no schema level, got {}", node.id()),
        }
    }

    /// The ids the listings hand out have to be exactly the ids
    /// `get_by_id` can resolve — that invariant is what lets the TUI
    /// address any row it renders.
    #[tokio::test]
    async fn listed_ids_are_resolvable() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = adapter_for(tmp.path()).await;

        let dbs = list_databases_impl(&adapter.client)
            .await
            .expect("databases");
        assert_eq!(dbs.items.len(), 1);
        adapter
            .get_by_id(&dbs.items[0].id)
            .await
            .expect("listed database id resolves");

        let tables = list_all_objects_impl(&adapter.client, KIND_TABLE)
            .await
            .expect("tables");
        let names: Vec<&str> = tables.items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            names,
            vec!["notes"],
            "the view must not be listed as a table"
        );
        adapter
            .get_by_id(&tables.items[0].id)
            .await
            .expect("listed table id resolves");

        let views = list_all_objects_impl(&adapter.client, KIND_VIEW)
            .await
            .expect("views");
        let names: Vec<&str> = views.items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(names, vec!["recent"]);
        assert_eq!(views.items[0].id, format!("{key}/views/recent"));
        adapter
            .get_by_id(&views.items[0].id)
            .await
            .expect("listed view id resolves");
    }

    #[tokio::test]
    async fn rows_paginate_and_expose_columns_as_metadata() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = adapter_for(tmp.path()).await;

        let params = ListParams {
            node_type: row_node_type(),
            query: None,
            sort: Vec::new(),
            page: Some(not_yet_done_content::PageRequest {
                offset: 0,
                limit: 1,
            }),
            download: false,
            group_by: None,
        };
        let first = list_rows_impl(&adapter.client, &key, TABLES_GROUP_ID, "notes", &params)
            .await
            .expect("first page");
        assert_eq!(first.items.len(), 1);
        assert_eq!(first.items[0].id, format!("{key}/tables/notes/rows/0"));
        let page = first.page.expect("page info");
        assert!(page.has_next && !page.has_prev);
        let body = first.items[0]
            .metadata
            .fields
            .iter()
            .find(|f| f.key == "body")
            .expect("body column");
        assert_eq!(body.value, "one");
    }

    #[tokio::test]
    async fn childs_are_wired_for_every_level_and_stop_at_a_row() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = adapter_for(tmp.path()).await;

        let root = adapter.root().await.expect("root");
        let kinds: Vec<String> = adapter
            .childs(root.as_ref())
            .iter()
            .map(|c| c.node_type.type_id.clone())
            .collect();
        assert_eq!(
            kinds,
            vec!["sqlite:database", "sqlite:table", "sqlite:view"]
        );

        for object in [format!("{key}/tables/notes"), format!("{key}/views/recent")] {
            let node = adapter.get_by_id(&object).await.expect("catalogue object");
            let kinds: Vec<String> = adapter
                .childs(node.as_ref())
                .iter()
                .map(|c| c.node_type.type_id.clone())
                .collect();
            assert_eq!(kinds, vec!["sqlite:row"], "{object}");
        }
    }

    /// A view's rows come from the same paginated `SELECT *` a table's do,
    /// and the ids they carry have to walk back through the `views`
    /// segment — otherwise a row of a view would resolve to a table.
    #[tokio::test]
    async fn a_view_lists_its_rows_under_its_own_id_prefix() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = adapter_for(tmp.path()).await;

        let params = ListParams {
            node_type: row_node_type(),
            query: None,
            sort: Vec::new(),
            page: None,
            download: false,
            group_by: None,
        };
        let rows = list_rows_impl(&adapter.client, &key, VIEWS_GROUP_ID, "recent", &params)
            .await
            .expect("view rows");
        // The view filters to `id > 1`, so exactly the second note.
        assert_eq!(rows.items.len(), 1);
        assert_eq!(rows.items[0].id, format!("{key}/views/recent/rows/0"));
    }

    #[tokio::test]
    async fn execute_custom_query_paginates_a_select() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = adapter_for(tmp.path()).await;

        let ctx = adapter
            .custom_query_context(&format!("{key}/tables/notes"))
            .with_page(not_yet_done_content::PageRequest {
                offset: 0,
                limit: 1,
            });
        let result = adapter
            .execute_custom_query("SELECT id, body FROM notes ORDER BY id", &ctx)
            .await
            .expect("run query");

        assert_eq!(result.columns, vec!["id", "body"]);
        // One row asked for, one row delivered — the extra row the wrapper
        // fetched only answers `has_next`.
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.items[0].id, "qrow:0");
        let page = result.page.expect("page info");
        assert!(page.has_next && !page.has_prev);
        assert!(result.status.is_none());
    }

    #[tokio::test]
    async fn execute_custom_query_reports_a_status_for_an_empty_result() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = adapter_for(tmp.path()).await;

        let ctx = adapter.custom_query_context(&key);
        let result = adapter
            .execute_custom_query("SELECT * FROM notes WHERE id < 0", &ctx)
            .await
            .expect("run query");
        assert!(result.items.is_empty());
        assert_eq!(result.status.as_deref(), Some("ok, no rows"));
    }

    /// Multi-statement scripts are not wrapped, so they report no page —
    /// paging them would mean re-running the prelude for every page.
    #[tokio::test]
    async fn execute_custom_query_runs_a_multi_statement_script_unpaged() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = adapter_for(tmp.path()).await;

        let ctx = adapter
            .custom_query_context(&key)
            .with_page(not_yet_done_content::PageRequest {
                offset: 0,
                limit: 1,
            });
        let result = adapter
            .execute_custom_query("SELECT 1; SELECT id FROM notes ORDER BY id", &ctx)
            .await
            .expect("run script");
        assert!(result.page.is_none(), "not a paginable shape");
        assert_eq!(result.items.len(), 2, "both rows, unpaged");
    }

    #[tokio::test]
    async fn execute_custom_query_without_a_database_is_a_clear_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, _key) = adapter_for(tmp.path()).await;

        match adapter
            .execute_custom_query("SELECT 1", &CustomQueryContext::new())
            .await
        {
            Err(ContentError::NotSupported(msg)) => assert!(msg.contains("database"), "{msg}"),
            other => panic!("expected NotSupported, got {:?}", other.map(|_| "ok")),
        }
    }

    /// The per-object script directory has to be addressable for every
    /// table *and view* the tree lists — that is what `Q` writes into.
    #[tokio::test]
    async fn node_scripts_are_placeable_for_a_listed_table_or_view() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = adapter_for(tmp.path()).await;
        let store = adapter.script_store().expect("script store");

        for (kind, name) in [(KIND_TABLE, "notes"), (KIND_VIEW, "recent")] {
            let listed = list_all_objects_impl(&adapter.client, kind)
                .await
                .expect("catalogue objects");
            let path = store
                .node_script_path(&listed.items[0].id, store.default_node_script_name())
                .unwrap_or_else(|| panic!("a {kind} node owns scripts"));
            assert!(
                path.ends_with(format!("queries/{key}/{name}/default.sql")),
                "{}",
                path.display()
            );
        }
        // A row is one level too deep to own scripts of its own.
        assert!(
            store
                .node_script_path(&format!("{key}/tables/notes/rows/0"), "default")
                .is_none()
        );
    }

    #[tokio::test]
    async fn custom_query_context_carries_the_source_key() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = adapter_for(tmp.path()).await;
        let ctx = adapter.custom_query_context(&format!("{key}/tables/notes"));
        assert_eq!(ctx.get("database"), Some(key.as_str()));
    }

    /// The script editor's completion line: single-level tokens, because
    /// a database file has no schema level to qualify with.
    #[tokio::test]
    async fn a_sql_script_buffer_gets_single_level_completion_tokens() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = adapter_for(tmp.path()).await;
        let node = NodeRef::parse(&format!("sqlite/{key}/db_scripts/audit.sql")).expect("ref");

        let augmented = adapter
            .augment_editor_buffer(&node, "SELECT 1;\n".into())
            .await;
        assert!(augmented.contains("-- table completions: "), "{augmented}");
        assert!(augmented.contains("tt_notes"), "{augmented}");

        // Reopening must not stack lines, and the commit path has to get
        // the user's own text back verbatim.
        let reopened = adapter
            .augment_editor_buffer(&node, augmented.clone())
            .await;
        assert_eq!(reopened, augmented, "append is idempotent");
        assert_eq!(adapter.strip_editor_hints(&reopened), "SELECT 1;\n");
    }

    /// Views are as selectable as tables, so they belong in the list.
    #[tokio::test]
    async fn completions_cover_views_too() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = adapter_for(tmp.path()).await;
        let node = NodeRef::parse(&format!("sqlite/{key}/db_scripts/audit.sql")).expect("ref");
        let augmented = adapter.augment_editor_buffer(&node, String::new()).await;
        assert!(augmented.contains("tt_recent"), "{augmented}");
    }

    /// A non-SQL script gets no line — but a stale one is still stripped,
    /// which is what keeps a renamed script clean.
    #[tokio::test]
    async fn a_non_sql_script_buffer_gets_no_completion_line() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = adapter_for(tmp.path()).await;
        let node = NodeRef::parse(&format!("sqlite/{key}/db_scripts/report.py")).expect("ref");

        let augmented = adapter
            .augment_editor_buffer(&node, "print(1)\n\n-- table completions: tt_notes\n".into())
            .await;
        assert_eq!(augmented, "print(1)\n");
    }

    /// The token has to resolve against the file the context names, not
    /// against whatever database happens to be first.
    #[tokio::test]
    async fn a_completion_token_resolves_on_execute() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = adapter_for(tmp.path()).await;

        let ctx = adapter.custom_query_context(&key);
        let result = adapter
            .execute_custom_query("SELECT id FROM tt_notes ORDER BY id", &ctx)
            .await
            .expect("run query");
        assert_eq!(result.items.len(), 2);
    }

    // -----------------------------------------------------------------
    // The view editor. `prepare` → edit → `execute` is the whole seam;
    // these tests drive it exactly as the TUI's edit session does.
    // -----------------------------------------------------------------

    /// The buffer holds the stored statement below the marker, and its own
    /// body has to parse back — that round trip is what makes the editor
    /// usable at all.
    #[tokio::test]
    async fn prepare_hands_out_the_stored_definition() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = adapter_for(tmp.path()).await;
        let view = adapter
            .get_by_id(&format!("{key}/views/recent"))
            .await
            .expect("view");

        let prep = view.prepare(EDIT_VIEW_ACTION).await.expect("prepare");
        assert_eq!(prep.suffix, ".sql");
        assert!(prep.version.starts_with("CREATE VIEW"), "{}", prep.version);
        let body = script_buffer::parse_query_area(&prep.template);
        let parsed = view_ddl::parse_create_view(body).expect("the buffer parses back");
        assert_eq!(parsed.name, "recent");
        // The header says what saving does, because "drop and re-create"
        // sounds riskier than it is.
        assert!(
            prep.template.contains("one transaction"),
            "{}",
            prep.template
        );
    }

    /// A table has no definition to edit, so the action does not exist
    /// there — and the type list is what a view config binds against.
    #[tokio::test]
    async fn only_a_view_offers_the_definition_editor() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, _key) = adapter_for(tmp.path()).await;
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
    }

    #[tokio::test]
    async fn saving_a_new_definition_replaces_the_view() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = writable_adapter_for(tmp.path()).await;
        let mut view = adapter
            .get_by_id(&format!("{key}/views/recent"))
            .await
            .expect("view");
        let prep = view.prepare(EDIT_VIEW_ACTION).await.expect("prepare");

        let edited = prep.template.replace("id > 1", "id > 0");
        match save(view.as_mut(), &edited, &prep.version).await {
            ActionOutcome::Done { message } => {
                assert!(message.unwrap_or_default().contains("recent"))
            }
            other => panic!("expected Done, got {}", outcome_name(&other)),
        }

        // Read the file back through a connection of its own, so what is
        // asserted is the committed database and not the adapter's pool.
        let stored = stored_definition_of(tmp.path(), "recent").await;
        assert!(stored.contains("id > 0"), "{stored}");
    }

    /// One independent read of `sqlite_master`, bypassing the adapter.
    async fn stored_definition_of(dir: &Path, name: &str) -> String {
        let options = sqlx::sqlite::SqliteConnectOptions::new().filename(dir.join("fixture.db"));
        let pool = sqlx::sqlite::SqlitePool::connect_with(options)
            .await
            .expect("open fixture");
        let sql: String = sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE name = ?1")
            .bind(name.to_string())
            .fetch_one(&pool)
            .await
            .expect("the object exists");
        pool.close().await;
        sql
    }

    /// Reopening the editor and saving without touching anything must not
    /// drop and re-create the view for nothing.
    #[tokio::test]
    async fn an_untouched_buffer_reports_no_changes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = writable_adapter_for(tmp.path()).await;
        let mut view = adapter
            .get_by_id(&format!("{key}/views/recent"))
            .await
            .expect("view");
        let prep = view.prepare(EDIT_VIEW_ACTION).await.expect("prepare");

        match save(view.as_mut(), &prep.template, &prep.version).await {
            ActionOutcome::NoChanges => {}
            other => panic!("expected NoChanges, got {}", outcome_name(&other)),
        }
    }

    /// Every rejection keeps the user's text and explains itself in a
    /// banner — the buffer is the only copy of what they wrote.
    #[tokio::test]
    async fn rejections_reopen_the_buffer_with_a_banner() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = writable_adapter_for(tmp.path()).await;
        let mut view = adapter
            .get_by_id(&format!("{key}/views/recent"))
            .await
            .expect("view");
        let prep = view.prepare(EDIT_VIEW_ACTION).await.expect("prepare");

        for (label, body, needle) in [
            // A rename would leave the old view in place beside a new one.
            ("rename", "CREATE VIEW other AS SELECT 1", "Rename it back"),
            // A second statement would touch something never selected.
            (
                "second statement",
                "CREATE VIEW recent AS SELECT 1; DROP TABLE notes",
                "second statement",
            ),
            // SQLite resolves a body lazily, so only the smoke read finds
            // this — and it has to come back as a banner, not as a lost
            // buffer.
            (
                "unknown table",
                "CREATE VIEW recent AS SELECT * FROM notez",
                "notez",
            ),
        ] {
            let edited = format!("{}\n{body};\n", script_buffer::QUERY_MARKER);
            match save(view.as_mut(), &edited, &prep.version).await {
                ActionOutcome::Reopen {
                    content,
                    new_version,
                } => {
                    assert!(content.contains(needle), "{label}: {content}");
                    assert!(content.contains(body), "{label}: the user's text survives");
                    assert!(new_version.is_none(), "{label}: nothing was re-read");
                }
                other => panic!("{label}: expected Reopen, got {}", outcome_name(&other)),
            }
        }

        // None of that may have changed the view.
        let stored = adapter
            .client
            .view_definition(&key, "recent")
            .await
            .expect("read back")
            .expect("the view survived every rejection");
        assert!(stored.contains("id > 1"), "{stored}");
    }

    /// A read-only adapter refuses with the reason and the setting to
    /// change, rather than letting SQLite answer "attempt to write a
    /// readonly database".
    #[tokio::test]
    async fn a_read_only_adapter_explains_why_it_refuses() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = adapter_for(tmp.path()).await;
        let mut view = adapter
            .get_by_id(&format!("{key}/views/recent"))
            .await
            .expect("view");
        let prep = view.prepare(EDIT_VIEW_ACTION).await.expect("prepare");

        let edited = prep.template.replace("id > 1", "id > 0");
        match save(view.as_mut(), &edited, &prep.version).await {
            ActionOutcome::Reopen { content, .. } => {
                assert!(content.contains("read_only"), "{content}")
            }
            other => panic!("expected Reopen, got {}", outcome_name(&other)),
        }
    }

    /// Somebody else changed the view while the editor was open. The save
    /// must not overwrite that silently; it reopens with the fresh version
    /// token, so the *next* save is a deliberate overwrite.
    #[tokio::test]
    async fn a_concurrent_change_reopens_with_the_new_version() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = writable_adapter_for(tmp.path()).await;
        let mut view = adapter
            .get_by_id(&format!("{key}/views/recent"))
            .await
            .expect("view");
        let prep = view.prepare(EDIT_VIEW_ACTION).await.expect("prepare");

        // The "other session".
        adapter
            .client
            .replace_view(
                &key,
                "recent",
                "CREATE VIEW recent AS SELECT id, body FROM notes WHERE id > 99",
            )
            .await
            .expect("concurrent change");

        let edited = prep.template.replace("id > 1", "id > 0");
        let fresh = match save(view.as_mut(), &edited, &prep.version).await {
            ActionOutcome::Reopen {
                content,
                new_version,
            } => {
                assert!(content.contains("changed in the database"), "{content}");
                new_version.expect("the fresh definition is the new token")
            }
            other => panic!("expected Reopen, got {}", outcome_name(&other)),
        };
        assert!(fresh.contains("id > 99"), "{fresh}");

        // Saving again, now against the fresh token, goes through.
        match save(view.as_mut(), &edited, &fresh).await {
            ActionOutcome::Done { .. } => {}
            other => panic!(
                "expected Done on the second save, got {}",
                outcome_name(&other)
            ),
        }
    }

    /// For panic messages: `ActionOutcome` is not `Debug`.
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

    // -----------------------------------------------------------------
    // The row editor. Same seam as the view editor above, one level
    // deeper: `prepare` hands out a YAML mapping of the row, `execute`
    // turns what came back into a single UPDATE.
    // -----------------------------------------------------------------

    /// A second fixture for the cases the shared one cannot show: no
    /// primary key, so the row is addressed by its implicit `rowid`, and a
    /// BLOB column, whose rendered form describes the data instead of
    /// being it.
    async fn loose_adapter_for(dir: &Path) -> (SqliteAdapter, String) {
        let path = dir.join("loose.db");
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true);
        let pool = sqlx::sqlite::SqlitePool::connect_with(options)
            .await
            .expect("create db");
        sqlx::query("CREATE TABLE loose (name TEXT, payload BLOB)")
            .execute(&pool)
            .await
            .expect("create table");
        sqlx::query("INSERT INTO loose VALUES ('first', x'00ff10')")
            .execute(&pool)
            .await
            .expect("insert");
        pool.close().await;

        let client = Arc::new(SqliteClient::new(
            vec![path.display().to_string()],
            false,
            Duration::from_millis(500),
            None,
        ));
        let key = client.sources().await[0].key.clone();
        (
            SqliteAdapter::from_client(client, "test".into(), "test-instance".into()),
            key,
        )
    }

    /// Saving a row buffer the way `NodeActionEditSession` does.
    async fn save_row(node: &mut dyn Node, text: &str, version: &str) -> ActionOutcome {
        node.execute(
            EDIT_ROW_ACTION,
            ActionInput::Edited {
                text: text.to_string(),
                original: String::new(),
                version: version.to_string(),
            },
        )
        .await
        .expect("execute must answer with an outcome, not an error")
    }

    /// One independent read of the fixture, so the assertion sees the
    /// committed database rather than the adapter's pool.
    async fn stored_body_of(dir: &Path, id: i64) -> Option<String> {
        let options = sqlx::sqlite::SqliteConnectOptions::new().filename(dir.join("fixture.db"));
        let pool = sqlx::sqlite::SqlitePool::connect_with(options)
            .await
            .expect("open fixture");
        let body = sqlx::query_scalar("SELECT body FROM notes WHERE id = ?1")
            .bind(id)
            .fetch_optional(&pool)
            .await
            .expect("read back")
            .flatten();
        pool.close().await;
        body
    }

    async fn row_node(adapter: &SqliteAdapter, id: &str) -> Box<dyn Node> {
        adapter.get_by_id(id).await.expect("row id resolves")
    }

    /// A row id the listing minted has to resolve — the editor session
    /// looks its node up by id before it can prepare anything — and the
    /// waypoint segment in between resolves too.
    #[tokio::test]
    async fn a_row_id_resolves_through_its_waypoint() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = adapter_for(tmp.path()).await;

        let waypoint = row_node(&adapter, &format!("{key}/tables/notes/rows")).await;
        assert_eq!(waypoint.node_type().type_id, "sqlite:rows");

        let row = row_node(&adapter, &format!("{key}/tables/notes/rows/0")).await;
        assert_eq!(row.node_type().type_id, "sqlite:row");
        assert_eq!(row.id(), format!("{key}/tables/notes/rows/0"));
    }

    /// The buffer is the row: one YAML key per column, with the header
    /// saying how it is addressed and what saving does.
    #[tokio::test]
    async fn prepare_renders_every_cell_of_the_row() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = adapter_for(tmp.path()).await;
        let row = row_node(&adapter, &format!("{key}/tables/notes/rows/0")).await;

        let prep = row.prepare(EDIT_ROW_ACTION).await.expect("prepare");
        assert_eq!(prep.suffix, ".yaml");
        assert!(prep.template.contains("id: '1'"), "{}", prep.template);
        assert!(prep.template.contains("body: 'one'"), "{}", prep.template);
        assert!(prep.template.contains("primary key"), "{}", prep.template);
        // The body below the header has to parse back, or the editor is
        // useless on the second save.
        let parsed = row_edit::parse_row_buffer(&prep.template).expect("the buffer parses back");
        assert_eq!(parsed.len(), 2);
    }

    #[tokio::test]
    async fn saving_a_changed_cell_writes_only_that_column() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = writable_adapter_for(tmp.path()).await;
        let mut row = row_node(&adapter, &format!("{key}/tables/notes/rows/0")).await;
        let prep = row.prepare(EDIT_ROW_ACTION).await.expect("prepare");

        let edited = prep.template.replace("body: 'one'", "body: 'first'");
        match save_row(row.as_mut(), &edited, &prep.version).await {
            ActionOutcome::Done { message } => {
                assert!(message.unwrap_or_default().contains("1 column"))
            }
            other => panic!("expected Done, got {}", outcome_name(&other)),
        }

        assert_eq!(
            stored_body_of(tmp.path(), 1).await.as_deref(),
            Some("first")
        );
        // The row beside it is untouched: one UPDATE, one row.
        assert_eq!(stored_body_of(tmp.path(), 2).await.as_deref(), Some("two"));
    }

    /// `null` in the buffer is SQL NULL, and that is a change like any
    /// other — the distinction the quoting exists for.
    #[tokio::test]
    async fn a_null_written_from_the_buffer_clears_the_cell() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = writable_adapter_for(tmp.path()).await;
        let mut row = row_node(&adapter, &format!("{key}/tables/notes/rows/0")).await;
        let prep = row.prepare(EDIT_ROW_ACTION).await.expect("prepare");

        let edited = prep.template.replace("body: 'one'", "body: null");
        match save_row(row.as_mut(), &edited, &prep.version).await {
            ActionOutcome::Done { .. } => {}
            other => panic!("expected Done, got {}", outcome_name(&other)),
        }
        assert_eq!(stored_body_of(tmp.path(), 1).await, None);
    }

    /// Opening and saving without touching anything must not write.
    #[tokio::test]
    async fn an_untouched_row_buffer_reports_no_changes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = writable_adapter_for(tmp.path()).await;
        let mut row = row_node(&adapter, &format!("{key}/tables/notes/rows/0")).await;
        let prep = row.prepare(EDIT_ROW_ACTION).await.expect("prepare");

        match save_row(row.as_mut(), &prep.template, &prep.version).await {
            ActionOutcome::NoChanges => {}
            other => panic!("expected NoChanges, got {}", outcome_name(&other)),
        }
    }

    /// Every rejection keeps the user's text and explains itself, exactly
    /// as the view editor's does.
    #[tokio::test]
    async fn a_column_the_row_has_not_got_reopens_with_a_banner() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = writable_adapter_for(tmp.path()).await;
        let mut row = row_node(&adapter, &format!("{key}/tables/notes/rows/0")).await;
        let prep = row.prepare(EDIT_ROW_ACTION).await.expect("prepare");

        let edited = format!("{}note: 'typo in the column name'\n", prep.template);
        match save_row(row.as_mut(), &edited, &prep.version).await {
            ActionOutcome::Reopen {
                content,
                new_version,
            } => {
                assert!(content.contains("no column note"), "{content}");
                // The columns it does have, so the fix is one keystroke.
                assert!(content.contains("id, body"), "{content}");
                assert!(content.contains("typo in the column name"), "{content}");
                assert!(new_version.is_none());
            }
            other => panic!("expected Reopen, got {}", outcome_name(&other)),
        }
        assert_eq!(stored_body_of(tmp.path(), 1).await.as_deref(), Some("one"));
    }

    /// A statement SQLite refuses comes back *with the statement*: an
    /// "UNIQUE constraint failed" is far easier to place next to the
    /// UPDATE that caused it.
    #[tokio::test]
    async fn a_failing_statement_is_shown_beside_the_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = writable_adapter_for(tmp.path()).await;
        let mut row = row_node(&adapter, &format!("{key}/tables/notes/rows/0")).await;
        let prep = row.prepare(EDIT_ROW_ACTION).await.expect("prepare");

        // The other row already holds this key.
        let edited = prep.template.replace("id: '1'", "id: '2'");
        match save_row(row.as_mut(), &edited, &prep.version).await {
            ActionOutcome::Reopen { content, .. } => {
                assert!(content.to_uppercase().contains("UNIQUE"), "{content}");
                assert!(content.contains("The statement that failed"), "{content}");
                assert!(content.contains("UPDATE \"notes\" SET"), "{content}");
                assert!(content.contains("WHERE \"id\" = '1'"), "{content}");
            }
            other => panic!("expected Reopen, got {}", outcome_name(&other)),
        }
    }

    /// Renaming the key *is* allowed when it is free — the WHERE uses the
    /// values from before the edit, so the row is found either way.
    #[tokio::test]
    async fn changing_a_key_column_addresses_the_row_by_its_old_value() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = writable_adapter_for(tmp.path()).await;
        let mut row = row_node(&adapter, &format!("{key}/tables/notes/rows/0")).await;
        let prep = row.prepare(EDIT_ROW_ACTION).await.expect("prepare");

        let edited = prep.template.replace("id: '1'", "id: '7'");
        match save_row(row.as_mut(), &edited, &prep.version).await {
            ActionOutcome::Done { .. } => {}
            other => panic!("expected Done, got {}", outcome_name(&other)),
        }
        assert_eq!(stored_body_of(tmp.path(), 7).await.as_deref(), Some("one"));
        assert_eq!(stored_body_of(tmp.path(), 1).await, None, "the row moved");
    }

    /// A read-only adapter says so, and names the setting to change,
    /// before SQLite gets a chance to answer "readonly database".
    #[tokio::test]
    async fn a_read_only_adapter_refuses_a_row_write() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = adapter_for(tmp.path()).await;
        let mut row = row_node(&adapter, &format!("{key}/tables/notes/rows/0")).await;
        let prep = row.prepare(EDIT_ROW_ACTION).await.expect("prepare");

        let edited = prep.template.replace("body: 'one'", "body: 'nope'");
        match save_row(row.as_mut(), &edited, &prep.version).await {
            ActionOutcome::Reopen { content, .. } => {
                assert!(content.contains("read_only"), "{content}")
            }
            other => panic!("expected Reopen, got {}", outcome_name(&other)),
        }
        assert_eq!(stored_body_of(tmp.path(), 1).await.as_deref(), Some("one"));
    }

    /// SQLite cannot write through a view, so the editor refuses before it
    /// opens — with the reason, and with what to do instead.
    #[tokio::test]
    async fn a_row_of_a_view_refuses_with_the_reason() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = writable_adapter_for(tmp.path()).await;
        let row = row_node(&adapter, &format!("{key}/views/recent/rows/0")).await;

        match row.prepare(EDIT_ROW_ACTION).await {
            Err(ContentError::NotSupported(message)) => {
                assert!(message.contains("is a view"), "{message}");
                assert!(message.contains("underlying table"), "{message}");
            }
            Err(other) => panic!("expected NotSupported, got {other:?}"),
            Ok(_) => panic!("a view's row must not open an editor"),
        }
    }

    /// Somebody else changed the row while the editor was open. Saving
    /// must not overwrite that silently: it reopens with the fresh token,
    /// so the *next* save is a deliberate overwrite.
    #[tokio::test]
    async fn a_concurrently_changed_row_reopens_with_the_new_version() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = writable_adapter_for(tmp.path()).await;
        let mut row = row_node(&adapter, &format!("{key}/tables/notes/rows/0")).await;
        let prep = row.prepare(EDIT_ROW_ACTION).await.expect("prepare");

        adapter
            .client
            .execute_write(&key, "UPDATE notes SET body = 'somebody else' WHERE id = 1")
            .await
            .expect("concurrent change");

        let edited = prep.template.replace("body: 'one'", "body: 'mine'");
        let fresh = match save_row(row.as_mut(), &edited, &prep.version).await {
            ActionOutcome::Reopen {
                content,
                new_version,
            } => {
                assert!(content.contains("changed in the database"), "{content}");
                assert!(content.contains("body: 'mine'"), "the user's text survives");
                new_version.expect("the re-read row is the new token")
            }
            other => panic!("expected Reopen, got {}", outcome_name(&other)),
        };
        assert_eq!(
            stored_body_of(tmp.path(), 1).await.as_deref(),
            Some("somebody else"),
            "nothing was written on the rejected save"
        );

        // Saving again, now against the fresh token, is the deliberate
        // overwrite — and the banner from the rejection does not reach the
        // database.
        match save_row(row.as_mut(), &edited, &fresh).await {
            ActionOutcome::Done { .. } => {}
            other => panic!(
                "expected Done on the second save, got {}",
                outcome_name(&other)
            ),
        }
        assert_eq!(stored_body_of(tmp.path(), 1).await.as_deref(), Some("mine"));
    }

    /// The row is gone by the time the editor saves. Its key matches
    /// nothing, and the buffer says so rather than writing nowhere.
    #[tokio::test]
    async fn a_deleted_row_refuses_the_write() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = writable_adapter_for(tmp.path()).await;
        let mut row = row_node(&adapter, &format!("{key}/tables/notes/rows/0")).await;
        let prep = row.prepare(EDIT_ROW_ACTION).await.expect("prepare");

        adapter
            .client
            .execute_write(&key, "DELETE FROM notes WHERE id = 1")
            .await
            .expect("delete");

        let edited = prep.template.replace("body: 'one'", "body: 'mine'");
        match save_row(row.as_mut(), &edited, &prep.version).await {
            ActionOutcome::Reopen { content, .. } => {
                assert!(content.contains("was deleted"), "{content}");
                assert!(content.contains("Nothing was written"), "{content}");
            }
            other => panic!("expected Reopen, got {}", outcome_name(&other)),
        }
    }

    /// A table without a primary key is addressed by its `rowid`, and that
    /// is worth saying in the header: the column is not part of the data.
    #[tokio::test]
    async fn a_table_without_a_primary_key_is_keyed_by_its_rowid() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = loose_adapter_for(tmp.path()).await;
        let mut row = row_node(&adapter, &format!("{key}/tables/loose/rows/0")).await;
        let prep = row.prepare(EDIT_ROW_ACTION).await.expect("prepare");
        assert!(prep.template.contains("rowid"), "{}", prep.template);

        let edited = prep.template.replace("name: 'first'", "name: 'renamed'");
        match save_row(row.as_mut(), &edited, &prep.version).await {
            ActionOutcome::Done { .. } => {}
            other => panic!("expected Done, got {}", outcome_name(&other)),
        }
        let stored = adapter
            .client
            .execute_raw_sql(&key, "SELECT name FROM loose")
            .await
            .expect("read back");
        assert_eq!(stored.rows[0][0].as_deref(), Some("renamed"));
    }

    /// A BLOB renders as a description of itself, so writing that text
    /// back would replace the data with the description. The cell is shown
    /// as a comment for context, and uncommenting it is refused.
    #[tokio::test]
    async fn a_blob_cell_is_context_only_and_refuses_to_be_written() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = loose_adapter_for(tmp.path()).await;
        let mut row = row_node(&adapter, &format!("{key}/tables/loose/rows/0")).await;
        let prep = row.prepare(EDIT_ROW_ACTION).await.expect("prepare");

        assert!(prep.template.contains("#   payload:"), "{}", prep.template);
        assert!(
            !prep.template.contains("\npayload:"),
            "the blob must not be an editable key: {}",
            prep.template
        );

        // Uncommenting it anyway.
        let edited = format!("{}payload: 'plain text'\n", prep.template);
        match save_row(row.as_mut(), &edited, &prep.version).await {
            ActionOutcome::Reopen { content, .. } => {
                assert!(content.contains("cannot be written from here"), "{content}");
                assert!(content.contains("payload"), "{content}");
            }
            other => panic!("expected Reopen, got {}", outcome_name(&other)),
        }

        let stored = adapter
            .client
            .execute_raw_sql(&key, "SELECT hex(payload) FROM loose")
            .await
            .expect("read back");
        assert_eq!(stored.rows[0][0].as_deref(), Some("00FF10"));
    }

    /// An unknown token is left in place: SQLite's own error then names it,
    /// which is the recognisable failure the user can act on.
    #[tokio::test]
    async fn an_unknown_token_surfaces_as_a_sql_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (adapter, key) = adapter_for(tmp.path()).await;

        let ctx = adapter.custom_query_context(&key);
        match adapter
            .execute_custom_query("SELECT * FROM tt_missing", &ctx)
            .await
        {
            Err(e) => assert!(format!("{e}").contains("tt_missing"), "{e}"),
            Ok(_) => panic!("expected an error for an unresolved token"),
        }
    }
}

/// Wiring tests for the Scripts branch: the branch itself (nodes, actions,
/// CRUD) is tested in `not_yet_done_sql_core::db_script_nodes`; what has to
/// hold *here* is that this adapter hangs it off the right parents, under
/// the `sqlite:` type prefix, reachable through `childs`, and routing to
/// the source key that names the database file.
#[cfg(test)]
mod db_script_tree_tests {
    use super::*;
    use not_yet_done_content::{ActionDispatch, children};
    use std::path::PathBuf;
    use std::time::Duration;

    /// The key of the one database the tests pretend to have. Never
    /// touched on disk — a `DatabaseNode` is built from its key alone, and
    /// the Scripts branch below it reads only the instance data dir.
    const KEY: &str = "notes-0badc0de";

    /// Offline adapter whose `instance_data_dir()` resolves to a fresh
    /// unique directory, returned alongside it. No database file and no
    /// client round trip: the branch is filesystem-only.
    fn build_adapter() -> (SqliteAdapter, PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        // Unique instance id → unique per-instance data dir (the default
        // `<data_local>/not_yet_done/sqlite/<instance_id>` layout).
        let instance_id = format!("test-{nanos}-{n}-{}", std::process::id());
        let client = Arc::new(SqliteClient::new(
            Vec::new(),
            true,
            Duration::from_millis(500),
            None,
        ));
        let adapter = SqliteAdapter::from_client(client, "test-conn".into(), instance_id);
        let dir = adapter.instance_data_dir();
        std::fs::create_dir_all(&dir).unwrap();
        (adapter, dir)
    }

    /// See the note on `list_params` in the catalogue tests: `children::list`
    /// locates its `Child` by full-struct `NodeType` equality, so the type
    /// has to come from the adapter's own `types()`.
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
    async fn group_of(adapter: &SqliteAdapter) -> Box<dyn Node> {
        adapter
            .get_by_id(&format!("{KEY}/{DB_SCRIPTS_GROUP_ID}"))
            .await
            .expect("db_scripts group")
    }

    #[tokio::test]
    async fn database_walk_reaches_the_group_under_the_sqlite_prefix() {
        let (adapter, tmp) = build_adapter();
        let g = group_of(&adapter).await;
        assert_eq!(g.id(), format!("{KEY}/db_scripts"));
        assert_eq!(g.node_type().type_id, "sqlite:db_scripts");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn group_lists_dirs_and_scripts_separately() {
        let (adapter, tmp) = build_adapter();
        let store = adapter.script_store().expect("script store");
        store.create_db_dir(KEY, "util").await.unwrap();
        store.create_db_script(KEY, "audit.sql").await.unwrap();
        let g = group_of(&adapter).await;
        let types = adapter.db_scripts.types();

        let dirs = children::list(&adapter, g.as_ref(), list_params(types.dir.clone()))
            .await
            .unwrap();
        let dir_names: Vec<&str> = dirs.items.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(dir_names, vec!["util"]);
        assert_eq!(dirs.items[0].node_type.type_id, "sqlite:db_script_dir");

        let scripts = children::list(&adapter, g.as_ref(), list_params(types.script.clone()))
            .await
            .unwrap();
        let script_names: Vec<&str> = scripts.items.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(script_names, vec!["audit.sql"]);
        assert_eq!(scripts.items[0].node_type.type_id, "sqlite:db_script");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn dir_node_lists_nested_children_with_full_ids() {
        let (adapter, tmp) = build_adapter();
        let store = adapter.script_store().expect("script store");
        store.create_db_dir(KEY, "util/inner").await.unwrap();
        store
            .create_db_script(KEY, "util/helper.sql")
            .await
            .unwrap();
        let dir_node = group_of(&adapter).await.get_child("util").await.unwrap();
        let types = adapter.db_scripts.types();

        let scripts = children::list(
            &adapter,
            dir_node.as_ref(),
            list_params(types.script.clone()),
        )
        .await
        .unwrap();
        assert_eq!(scripts.items.len(), 1);
        // Node id encodes the full path so the segment walker can resolve
        // it back via root → database → db_scripts → util → helper.sql.
        assert_eq!(
            scripts.items[0].id,
            format!("{KEY}/db_scripts/util/helper.sql")
        );

        let dirs = children::list(&adapter, dir_node.as_ref(), list_params(types.dir.clone()))
            .await
            .unwrap();
        assert_eq!(dirs.items.len(), 1);
        assert_eq!(dirs.items[0].id, format!("{KEY}/db_scripts/util/inner"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The database level also offers its scripts flat, skipping the group
    /// node — the same shortcut it offers for tables. Folders and their
    /// contents never show up in that list.
    #[tokio::test]
    async fn database_offers_a_flat_script_shortcut() {
        let (adapter, tmp) = build_adapter();
        let store = adapter.script_store().expect("script store");
        store.create_db_script(KEY, "audit.sql").await.unwrap();
        store.create_db_script(KEY, "util/deep.sql").await.unwrap();
        let db = adapter.get_by_id(KEY).await.unwrap();

        let flat = children::list(
            &adapter,
            db.as_ref(),
            list_params(adapter.db_scripts.types().script.clone()),
        )
        .await
        .unwrap();
        assert_eq!(flat.items.len(), 1);
        assert_eq!(flat.items[0].id, format!("{KEY}/db_scripts/audit.sql"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// `x` on a script has to run against the file the script lives under.
    /// The dispatch carries the source key, which is exactly what
    /// `execute_custom_query` resolves back into a database file.
    #[tokio::test]
    async fn executing_a_script_routes_to_its_own_database_file() {
        let (adapter, tmp) = build_adapter();
        let store = adapter.script_store().expect("script store");
        store.create_db_script(KEY, "audit.sql").await.unwrap();
        let script = adapter
            .get_by_id(&format!("{KEY}/db_scripts/audit.sql"))
            .await
            .unwrap();

        let ctx = not_yet_done_content::ActionContext::default();
        match script.invoke_action("execute", &ctx).await.unwrap() {
            ActionDispatch::ExecuteQuery { database, sql, .. } => {
                assert_eq!(database, KEY);
                // The template's body below the marker, nothing above it.
                assert_eq!(sql.trim(), "SELECT 1;");
            }
            other => panic!("expected ExecuteQuery, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// `actions_for_type` must answer for the branch's three types without
    /// a node walk — that's what renders the shortcut hints.
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
        // — the query editor belongs to the table, and a row config reaches
        // it via `parent:edit_sql`.
        assert_eq!(ids(&row_node_type()), vec!["edit_row".to_string()]);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
