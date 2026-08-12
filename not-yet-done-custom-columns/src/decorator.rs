//! The read/inject decorator and its factory wrapper.
//!
//! # Why a decorator (and why here, not in an adapter)
//!
//! Custom columns are a **local annotation layered on top of any adapter's
//! rows** — not adapter content. So no adapter implements anything: the host
//! wraps every factory in [`custom_columns_factory`], and each produced adapter
//! is wrapped in a [`CustomColumnsAdapter`]. From then on every row the adapter
//! hands out (list rows, eager subtrees, the post-edit row projection, detail
//! metadata) passes through this layer, which looks the row up by its `id` in
//! the single lib-owned [`LocalColumnStore`] (scoped to this adapter instance)
//! and appends any stored cells as [`MetadataField`]s. A `source: custom` view
//! column whose `key` matches a stored cell's `column_key` then renders it via
//! the ordinary metadata-field path — no change to column resolution, no
//! `ListResult` field, no adapter code.
//!
//! Writing is symmetric and equally adapter-free: every node carries synthetic
//! actions — `edit-cells`, `set-cell`, `clear-cell` and `retype-column`
//! ([`EDIT_CELLS_ACTION_ID`] / [`SET_CELL_ACTION_ID`] /
//! [`CLEAR_CELL_ACTION_ID`] / [`RETYPE_COLUMN_ACTION_ID`]) — handled here
//! against the store. Because they live on [`Node::actions`], the CLI
//! (`do set-cell … --field column_key=… --field value=…`) and the TUI menu
//! reach them with no front-end change.
//!
//! # Inert by default
//!
//! The wrapper is applied to *every* adapter unconditionally, but does nothing
//! observable until cells exist: each `list()` costs one batched
//! `row_id IN (…)` lookup, and a scope with no cells injects nothing. A store
//! read error degrades to "no cells" rather than failing the row.
//!
//! # Ordering with anonymization
//!
//! The host wraps this *inside* the anonymizer
//! (`anonymizing_factory(custom_columns_factory(inner))`), so in a screenshot
//! run the injected custom values are scrubbed like any other free text
//! (numbers/dates survive) — a user's local note can't leak past the mask.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use not_yet_done_content::*;

use crate::store::{Cell, LocalColumnStore, VALUE_TYPES};

/// Stable id of the synthetic "set a custom cell" action every node carries.
pub const SET_CELL_ACTION_ID: &str = "set-cell";
/// Stable id of the synthetic "clear a custom cell" action every node carries.
pub const CLEAR_CELL_ACTION_ID: &str = "clear-cell";
/// Stable id of the synthetic "edit all custom cells" action every node
/// carries — a typed form built from the described columns.
pub const EDIT_CELLS_ACTION_ID: &str = "edit-cells";
/// Stable id of the synthetic "change a custom column's type" action.
pub const RETYPE_COLUMN_ACTION_ID: &str = "retype-column";

/// The synthetic actions injected onto every node's [`Node::actions`]. Menu- /
/// form-driven (no default key), so they surface in the CLI `do`/`actions`
/// paths and the TUI action menu without any front-end wiring.
fn custom_column_actions() -> Vec<NodeAction> {
    vec![
        // Typed one-form editor over the already-defined custom columns. The
        // front-end resolves the fields from `describe_columns` at open time;
        // the value type of each column is authoritative (type-on-first-write),
        // so this form never needs to ask for a type — only `set-cell` does,
        // when *defining* a new column.
        NodeAction::new(
            EDIT_CELLS_ACTION_ID,
            "edit custom cells",
            InputSpec::ColumnForm,
        ),
        NodeAction::new(
            SET_CELL_ACTION_ID,
            "set custom cell",
            InputSpec::Form {
                fields: vec![
                    FormFieldSpec::text("column_key", "Column key"),
                    FormFieldSpec::text("value", "Value").optional(),
                    FormFieldSpec::select(
                        "value_type",
                        "Type",
                        VALUE_TYPES.iter().map(|t| (*t).to_string()).collect(),
                    )
                    .with_default("text"),
                ],
            },
        ),
        NodeAction::new(
            CLEAR_CELL_ACTION_ID,
            "clear custom cell",
            InputSpec::Form {
                fields: vec![FormFieldSpec::text("column_key", "Column key")],
            },
        ),
        // Changing a column's type is deliberately its own action rather than
        // a relaxation of `set-cell`. `set-cell`'s type select defaults to
        // `text`, and every value validates as text — folding a retype into it
        // would silently downgrade a `number` column to text whenever someone
        // set a cell without touching the default. Retyping is a decision, so
        // it gets an action one has to choose.
        NodeAction::new(
            RETYPE_COLUMN_ACTION_ID,
            "retype custom column",
            InputSpec::Form {
                fields: vec![
                    FormFieldSpec::text("column_key", "Column key"),
                    FormFieldSpec::select(
                        "value_type",
                        "New type",
                        VALUE_TYPES.iter().map(|t| (*t).to_string()).collect(),
                    ),
                ],
            },
        ),
    ]
}

/// Append each stored cell as a metadata field, unless a field with that key
/// already exists — a real adapter field always wins over a custom cell of the
/// same key, so custom columns can only *add* columns, never shadow real ones.
fn inject(metadata: &mut Metadata, cells: &[Cell]) {
    for cell in cells {
        if metadata.fields.iter().any(|f| f.key == cell.column_key) {
            continue;
        }
        metadata.fields.push(MetadataField {
            key: cell.column_key.clone(),
            value: cell.value.clone(),
            display_label: cell.column_key.clone(),
            editable: true,
            allowed_values: None,
        });
    }
}

/// Project every *defined* custom column into a row, blank where the row has
/// no stored cell. Without this a column would be missing from most rows
/// rather than empty in them, and "has no value" would be indistinguishable
/// from "is not a column here" — the difference `is_null` filters on and the
/// reason [`ColumnSchema::in_rows`] can be trusted.
fn fill_defined(metadata: &mut Metadata, defined: &[ColumnSchema]) {
    for col in defined {
        if metadata.fields.iter().any(|f| f.key == col.key) {
            continue;
        }
        metadata.fields.push(MetadataField {
            key: col.key.clone(),
            value: String::new(),
            display_label: col.display_label().to_string(),
            editable: true,
            allowed_values: None,
        });
    }
}

/// Inject the matching row's cells into a summary, if any.
fn inject_summary(summary: &mut NodeSummary, cells: &HashMap<String, Vec<Cell>>) {
    if let Some(row_cells) = cells.get(&summary.id) {
        inject(&mut summary.metadata, row_cells);
    }
}

/// Collect every node id in an eager subtree (so one batched lookup covers the
/// whole structure).
fn collect_ids<'a>(subtree: &'a Subtree, out: &mut Vec<&'a str>) {
    for node in &subtree.items {
        out.push(node.summary.id.as_str());
        collect_ids(&node.children, out);
    }
}

/// Inject stored cells into every summary of an eager subtree.
fn inject_subtree(subtree: &mut Subtree, cells: &HashMap<String, Vec<Cell>>) {
    for node in subtree.items.iter_mut() {
        inject_summary(&mut node.summary, cells);
        inject_subtree(&mut node.children, cells);
    }
}

// ---------------------------------------------------------------------------
// Factory decorator — the single injection point
// ---------------------------------------------------------------------------

/// Wrap a factory so every adapter it produces is wrapped in a
/// [`CustomColumnsAdapter`] backed by the process-wide shared store. Applied to
/// every registered factory by the host, making custom columns universal across
/// front-ends. Inert until cells exist, so this is free in normal use.
pub fn custom_columns_factory(inner: Box<dyn AdapterFactory>) -> Box<dyn AdapterFactory> {
    Box::new(CustomColumnsFactory {
        inner,
        store: crate::store::shared_store(),
    })
}

struct CustomColumnsFactory {
    inner: Box<dyn AdapterFactory>,
    store: Arc<LocalColumnStore>,
}

impl AdapterFactory for CustomColumnsFactory {
    fn adapter_type(&self) -> &str {
        self.inner.adapter_type()
    }

    fn create(
        &self,
        instance_id: &str,
        config: &str,
        ctx: &HostContext,
    ) -> Result<Box<dyn ContentAdapter>> {
        let adapter = self.inner.create(instance_id, config, ctx)?;
        Ok(Box::new(CustomColumnsAdapter::new(
            adapter,
            self.store.clone(),
        )))
    }

    fn config_schema(&self) -> fieldsmith::TypeSchema {
        self.inner.config_schema()
    }

    fn auth_mechanisms(&self) -> &'static [not_yet_done_content::MechanismSpec] {
        self.inner.auth_mechanisms()
    }
}

// ---------------------------------------------------------------------------
// Adapter decorator
// ---------------------------------------------------------------------------

/// Wraps a [`ContentAdapter`], injecting stored custom cells into every row it
/// serves and exposing the `set-cell` / `clear-cell` write actions. Everything
/// else is delegated verbatim.
pub struct CustomColumnsAdapter {
    inner: Box<dyn ContentAdapter>,
    store: Arc<LocalColumnStore>,
    /// `"<adapter_type>/<instance_id>"` — the store scope for this instance.
    scope: Arc<str>,
}

impl CustomColumnsAdapter {
    pub fn new(inner: Box<dyn ContentAdapter>, store: Arc<LocalColumnStore>) -> Self {
        let scope: Arc<str> =
            Arc::from(format!("{}/{}", inner.adapter_type(), inner.instance_id()));
        Self {
            inner,
            store,
            scope,
        }
    }

    async fn wrap(&self, inner: Box<dyn Node>) -> Box<dyn Node> {
        wrap_node(inner, self.store.clone(), self.scope.clone()).await
    }
}

#[async_trait]
impl ContentAdapter for CustomColumnsAdapter {
    fn adapter_type(&self) -> &str {
        self.inner.adapter_type()
    }
    fn instance_id(&self) -> &str {
        self.inner.instance_id()
    }
    fn instance_data_dir(&self) -> PathBuf {
        self.inner.instance_data_dir()
    }

    async fn root(&self) -> Result<Box<dyn Node>> {
        Ok(self.wrap(self.inner.root().await?).await)
    }
    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
        Ok(self.wrap(self.inner.get_by_id(id).await?).await)
    }

    fn childs<'a>(&'a self, node: &'a dyn Node) -> Vec<Child<'a>> {
        // Delegate to the inner adapter (it reads only the forwarded
        // `id()`/`node_type()`), then inject this instance's stored cells into
        // each fetched row as the closure runs.
        self.inner
            .childs(node)
            .into_iter()
            .map(|c| {
                let store = self.store.clone();
                let scope = self.scope.clone();
                let type_id = c.node_type.type_id.clone();
                Child {
                    node_type: c.node_type,
                    columns: c.columns,
                    list: Box::new(move |params| {
                        Box::pin(async move {
                            let mut result = (c.list)(params).await?;
                            let ids: Vec<&str> =
                                result.items.iter().map(|s| s.id.as_str()).collect();
                            let cells = store.get_for_rows(&scope, &ids).await.unwrap_or_default();
                            let defined = store.columns(&scope, &type_id).await.unwrap_or_default();
                            for item in result.items.iter_mut() {
                                inject_summary(item, &cells);
                                fill_defined(&mut item.metadata, &defined);
                            }
                            Ok(result)
                        })
                    }),
                }
            })
            .collect()
    }

    async fn eager_subtree(
        &self,
        node: &dyn Node,
        params: &ListParams,
        depth: u32,
    ) -> Option<Result<Subtree>> {
        // Preserve the inner adapter's one-pass expansion, injecting cells into
        // the whole subtree afterwards in a single batched store read.
        match self.inner.eager_subtree(node, params, depth).await {
            Some(Ok(mut subtree)) => {
                let mut ids: Vec<&str> = Vec::new();
                collect_ids(&subtree, &mut ids);
                let cells = self
                    .store
                    .get_for_rows(&self.scope, &ids)
                    .await
                    .unwrap_or_default();
                inject_subtree(&mut subtree, &cells);
                Some(Ok(subtree))
            }
            other => other,
        }
    }

    async fn download_asset(&self, url: &str) -> Result<Vec<u8>> {
        self.inner.download_asset(url).await
    }
    fn actions_for_type(&self, node_type: &NodeType) -> Vec<NodeAction> {
        // The custom-column cell actions are cross-cutting: every wrapped node
        // gets them on top of the inner adapter's type-level set. Mirrors the
        // node wrapper ([`CustomColumnsNode::actions`] before the trait method
        // was removed) so the type-level set is the single source of truth.
        let mut actions = self.inner.actions_for_type(node_type);
        for a in custom_column_actions() {
            if !actions.iter().any(|x| x.id == a.id) {
                actions.push(a);
            }
        }
        actions
    }
    fn child_process_env(&self, node: &NodeRef) -> HashMap<String, String> {
        self.inner.child_process_env(node)
    }
    async fn augment_editor_buffer(&self, node: &NodeRef, buffer: String) -> String {
        self.inner.augment_editor_buffer(node, buffer).await
    }
    fn strip_editor_hints(&self, text: &str) -> String {
        self.inner.strip_editor_hints(text)
    }
    fn capabilities(&self) -> AdapterCapabilities {
        self.inner.capabilities()
    }
    fn has_active_tracking(&self) -> bool {
        self.inner.has_active_tracking()
    }
    async fn list_values(&self, source: &str) -> Result<Vec<ValueOption>> {
        self.inner.list_values(source).await
    }
    fn subscribe_status(&self) -> tokio::sync::watch::Receiver<AdapterStatus> {
        self.inner.subscribe_status()
    }
    fn subscribe_invalidations(&self) -> tokio::sync::broadcast::Receiver<Invalidation> {
        self.inner.subscribe_invalidations()
    }
    fn subscribe_reminders(&self) -> tokio::sync::broadcast::Receiver<Reminder> {
        self.inner.subscribe_reminders()
    }
    fn take_prompt_requests(&self) -> Option<tokio::sync::mpsc::Receiver<PromptRequest>> {
        self.inner.take_prompt_requests()
    }
    async fn live_rows(&self) -> Vec<NodeSummary> {
        self.inner.live_rows().await
    }
    async fn bucket_for_now(&self, group_by: &GroupSpec) -> Option<String> {
        self.inner.bucket_for_now(group_by).await
    }
    async fn live_group_rows(&self, group_by: &GroupSpec, query: Option<&str>) -> Vec<NodeSummary> {
        self.inner.live_group_rows(group_by, query).await
    }
    async fn revalidate(&self) {
        self.inner.revalidate().await
    }
    async fn submit_credentials(&self, fields: HashMap<String, String>) -> Result<()> {
        self.inner.submit_credentials(fields).await
    }

    async fn cancel_credentials(&self) -> Result<()> {
        self.inner.cancel_credentials().await
    }
    async fn try_refresh_session(&self) -> Result<()> {
        self.inner.try_refresh_session().await
    }
    async fn invalidate_session(&self) -> Result<()> {
        self.inner.invalidate_session().await
    }
    async fn invalidate_credentials(&self) -> Result<()> {
        self.inner.invalidate_credentials().await
    }
    async fn load_view_sort(&self, scope: &str) -> Result<Vec<SortKey>> {
        self.inner.load_view_sort(scope).await
    }
    async fn save_view_sort(&self, scope: &str, sort: &[SortKey]) -> Result<()> {
        self.inner.save_view_sort(scope, sort).await
    }
    fn query_variables(&self, query: &str) -> Vec<QueryVariable> {
        self.inner.query_variables(query)
    }
    fn render_query(&self, query: &str, vars: &HashMap<String, String>) -> String {
        self.inner.render_query(query, vars)
    }
    async fn execute_custom_query(
        &self,
        query: &str,
        context: &CustomQueryContext,
    ) -> Result<CustomQueryResult> {
        self.inner.execute_custom_query(query, context).await
    }
    fn saved_query_store(&self) -> Option<&dyn SavedQueryStore> {
        self.inner.saved_query_store()
    }
    fn query_body_suffix(&self) -> &str {
        self.inner.query_body_suffix()
    }
    fn script_store(&self) -> Option<&dyn ScriptStore> {
        self.inner.script_store()
    }
    async fn search_in_tree(&self, query: &str, limit: u32) -> Result<Option<TreeSearchResults>> {
        self.inner.search_in_tree(query, limit).await
    }
    async fn locate_node_path(&self, node_id: &str) -> Result<Option<Vec<String>>> {
        self.inner.locate_node_path(node_id).await
    }
    fn hooks(&self) -> Vec<&str> {
        self.inner.hooks()
    }
    fn anonymizer(&self) -> Arc<dyn Anonymizer> {
        self.inner.anonymizer()
    }

    /// The custom columns stored for this instance's `node_type`, merged over
    /// whatever the inner adapter describes. The store is authoritative for a
    /// key it defines (type-on-first-write), so a stored column wins over an
    /// inner one of the same key — mirroring how injected cells never shadow a
    /// real metadata field, only add.
    async fn describe_columns(&self, node_type: &str) -> Vec<ColumnSchema> {
        let mut columns = self.inner.describe_columns(node_type).await;
        let stored = self
            .store
            .columns(&self.scope, node_type)
            .await
            .unwrap_or_default();
        for col in stored {
            columns.retain(|c| c.key != col.key);
            columns.push(col);
        }
        columns
    }
}

// ---------------------------------------------------------------------------
// Node decorator
// ---------------------------------------------------------------------------

/// Construct a wrapped node, reading its own stored cells once so the sync
/// accessors (`metadata`, `row_summary`) can inject without a per-call query
/// (mirrors the anonymizer's cache-at-construction pattern). A store read error
/// degrades to "no cells".
async fn wrap_node(
    inner: Box<dyn Node>,
    store: Arc<LocalColumnStore>,
    scope: Arc<str>,
) -> Box<dyn Node> {
    let cells = store
        .get_for_row(&scope, inner.id())
        .await
        .unwrap_or_default();
    let mut metadata = inner.metadata().clone();
    inject(&mut metadata, &cells);
    Box::new(CustomColumnsNode {
        inner,
        store,
        scope,
        cells,
        metadata,
    })
}

/// Wraps a [`Node`]. Sync accessors serve cached injections computed at
/// construction; async list surfaces inject freshly per call.
pub struct CustomColumnsNode {
    inner: Box<dyn Node>,
    store: Arc<LocalColumnStore>,
    scope: Arc<str>,
    /// This node's own stored cells (for its detail/row projections).
    cells: Vec<Cell>,
    /// `inner.metadata()` with this node's cells injected.
    metadata: Metadata,
}

impl CustomColumnsNode {
    /// Persist one custom cell for this node: an empty (whitespace-only) value
    /// clears the cell, anything else sets it with `value_type` (which the
    /// store applies only when *defining* the column — type-on-first-write).
    async fn write_cell(
        &self,
        node_type: &str,
        key: &str,
        value: &str,
        value_type: &str,
    ) -> Result<()> {
        if value.trim().is_empty() {
            self.store
                .clear_cell(&self.scope, self.inner.id(), key)
                .await
        } else {
            self.store
                .set_cell(
                    &self.scope,
                    node_type,
                    self.inner.id(),
                    key,
                    value,
                    value_type,
                )
                .await
        }
    }
}

#[async_trait]
impl Node for CustomColumnsNode {
    fn id(&self) -> &str {
        self.inner.id()
    }
    fn label(&self) -> &str {
        self.inner.label()
    }
    fn node_type(&self) -> &NodeType {
        self.inner.node_type()
    }
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    async fn hydrate(&mut self) {
        self.inner.hydrate().await;
        self.cells = self
            .store
            .get_for_row(&self.scope, self.inner.id())
            .await
            .unwrap_or_default();
        let mut metadata = self.inner.metadata().clone();
        inject(&mut metadata, &self.cells);
        self.metadata = metadata;
    }

    fn row_summary(&self) -> NodeSummary {
        let mut summary = self.inner.row_summary();
        inject(&mut summary.metadata, &self.cells);
        summary
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        Ok(wrap_node(
            self.inner.get_child(id).await?,
            self.store.clone(),
            self.scope.clone(),
        )
        .await)
    }

    fn content(&self) -> Option<&dyn Content> {
        self.inner.content()
    }
    async fn invoke_action(&self, name: &str, ctx: &ActionContext) -> Result<ActionDispatch> {
        self.inner.invoke_action(name, ctx).await
    }

    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        self.inner.prepare(action_id).await
    }

    async fn picker_options(&self, action_id: &str) -> Result<Vec<ActionOption>> {
        self.inner.picker_options(action_id).await
    }

    async fn form_prep(&self, action_id: &str) -> Result<HashMap<String, String>> {
        // The edit-cells form is prefilled with this row's current cell values
        // (keyed by column_key), so an unset column shows empty and a set one
        // shows its stored value.
        if action_id == EDIT_CELLS_ACTION_ID {
            return Ok(self
                .cells
                .iter()
                .map(|c| (c.column_key.clone(), c.value.clone()))
                .collect());
        }
        self.inner.form_prep(action_id).await
    }

    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        match action_id {
            SET_CELL_ACTION_ID => {
                let fields = form_fields(input)?;
                let column_key = required(&fields, "column_key")?;
                let value = fields.get("value").cloned().unwrap_or_default();
                let value_type = match fields.get("value_type") {
                    Some(t) if !t.trim().is_empty() => t.clone(),
                    _ => "text".to_string(),
                };
                self.store
                    .set_cell(
                        &self.scope,
                        &self.inner.node_type().type_id,
                        self.inner.id(),
                        &column_key,
                        &value,
                        &value_type,
                    )
                    .await?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("Set custom column `{column_key}` = {value}")),
                })
            }
            CLEAR_CELL_ACTION_ID => {
                let fields = form_fields(input)?;
                let column_key = required(&fields, "column_key")?;
                self.store
                    .clear_cell(&self.scope, self.inner.id(), &column_key)
                    .await?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("Cleared custom column `{column_key}`")),
                })
            }
            // Scope-wide, despite being invoked from a row: a column's type
            // belongs to the column, so this touches every row's cell for it.
            // The store either migrates them all or refuses and names the ones
            // in the way — the error travels back to the front-end verbatim.
            RETYPE_COLUMN_ACTION_ID => {
                let fields = form_fields(input)?;
                let column_key = required(&fields, "column_key")?;
                let value_type = required(&fields, "value_type")?;
                let migrated = self
                    .store
                    .retype_column(
                        &self.scope,
                        &self.inner.node_type().type_id,
                        &column_key,
                        &value_type,
                    )
                    .await?;
                Ok(ActionOutcome::Done {
                    message: Some(format!(
                        "Custom column `{column_key}` is now `{value_type}` ({migrated} cell(s) migrated)"
                    )),
                })
            }
            EDIT_CELLS_ACTION_ID => {
                let node_type = self.inner.node_type().type_id.clone();
                let current: HashMap<&str, &str> = self
                    .cells
                    .iter()
                    .map(|c| (c.column_key.as_str(), c.value.as_str()))
                    .collect();
                let changed = match input {
                    // Front-end-typed cells: the `value_type` travels with each
                    // cell, so a column the store has never seen is created on
                    // first write (type-on-first-write). The store stays
                    // authoritative — it rejects a type conflict with a column
                    // it already defines.
                    ActionInput::ColumnForm(cells) => {
                        let mut changed = 0usize;
                        for cell in &cells {
                            if current.get(cell.key.as_str()).copied().unwrap_or("")
                                == cell.value.as_str()
                            {
                                continue;
                            }
                            self.write_cell(&node_type, &cell.key, &cell.value, &cell.value_type)
                                .await?;
                            changed += 1;
                        }
                        changed
                    }
                    // Untyped form (e.g. the CLI, which builds fields from
                    // `describe_columns`): no types supplied, so only columns
                    // the store already defines can be edited — their type is
                    // resolved here and unknown keys are ignored.
                    ActionInput::Form(fields) => {
                        let schema = self
                            .store
                            .columns(&self.scope, &node_type)
                            .await
                            .unwrap_or_default();
                        let types: HashMap<&str, &str> = schema
                            .iter()
                            .map(|c| (c.key.as_str(), c.value_type.as_str()))
                            .collect();
                        let mut changed = 0usize;
                        for (key, value) in &fields {
                            let Some(&value_type) = types.get(key.as_str()) else {
                                continue;
                            };
                            if current.get(key.as_str()).copied().unwrap_or("") == value.as_str() {
                                continue;
                            }
                            self.write_cell(&node_type, key, value, value_type).await?;
                            changed += 1;
                        }
                        changed
                    }
                    _ => {
                        return Err(ContentError::Other(
                            "edit-cells expects a column form".into(),
                        ));
                    }
                };
                Ok(ActionOutcome::Done {
                    message: Some(match changed {
                        0 => "No custom columns changed".to_string(),
                        1 => "Updated 1 custom column".to_string(),
                        n => format!("Updated {n} custom columns"),
                    }),
                })
            }
            _ => self.inner.execute(action_id, input).await,
        }
    }
}

/// Extract the form-field map from an [`ActionInput`], erroring on the wrong
/// input shape.
fn form_fields(input: ActionInput) -> Result<HashMap<String, String>> {
    match input {
        ActionInput::Form(map) => Ok(map),
        _ => Err(ContentError::Other(
            "custom-column action expects form input".into(),
        )),
    }
}

/// Read a required, non-empty field, erroring with a clear message otherwise.
fn required(fields: &HashMap<String, String>, key: &str) -> Result<String> {
    match fields.get(key) {
        Some(v) if !v.trim().is_empty() => Ok(v.clone()),
        _ => Err(ContentError::Other(
            format!("custom-column action requires a non-empty `{key}`").into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::connect;
    use not_yet_done_content::mock::{MockAdapterBuilder, MockNodeData, issue_type};

    async fn mem_store(name: &str) -> Arc<LocalColumnStore> {
        let url = format!("sqlite:file:{name}?mode=memory&cache=shared");
        Arc::new(LocalColumnStore::new(Arc::new(
            connect(&url).await.unwrap(),
        )))
    }

    fn taiga_adapter() -> Box<dyn ContentAdapter> {
        Box::new(
            MockAdapterBuilder::new("taiga")
                .instance_id("t1")
                .node(
                    MockNodeData::new("root", "Root")
                        .child_type(issue_type())
                        .child(MockNodeData::new("ISS-1", "Bug").node_type(issue_type())),
                )
                .build(),
        )
    }

    fn cell(key: &str, value: &str, value_type: &str) -> ColumnCellInput {
        ColumnCellInput {
            key: key.into(),
            value: value.into(),
            value_type: value_type.into(),
        }
    }

    /// A typed `ColumnForm` submission for a column the store has never seen
    /// creates it on first write, with the front-end-supplied type — the whole
    /// point of the bootstrap redesign (no separate "define a column" step).
    #[tokio::test(flavor = "multi_thread")]
    async fn column_form_bootstraps_a_new_column_on_first_write() {
        let store = mem_store("cc_bootstrap").await;
        let scope = "taiga/t1";
        let adapter = CustomColumnsAdapter::new(taiga_adapter(), store.clone());

        // Fresh store: no columns described beyond what the inner adapter offers.
        assert!(store.columns(scope, "mock:issue").await.unwrap().is_empty());

        let mut node = adapter.get_by_id("ISS-1").await.unwrap();
        let outcome = node
            .execute(
                EDIT_CELLS_ACTION_ID,
                ActionInput::ColumnForm(vec![cell("estimate", "5", "number")]),
            )
            .await
            .unwrap();
        assert!(matches!(outcome, ActionOutcome::Done { .. }));

        // The column now exists, typed exactly as the front-end declared, and
        // its value is stored against the row.
        let cols = store.columns(scope, "mock:issue").await.unwrap();
        let est = cols.iter().find(|c| c.key == "estimate").unwrap();
        assert_eq!(est.value_type, "number");
        let cells = store.get_for_row(scope, "ISS-1").await.unwrap();
        assert_eq!(
            cells
                .iter()
                .find(|c| c.column_key == "estimate")
                .unwrap()
                .value,
            "5"
        );
    }

    /// The retype action turns a column pinned to `text` on first write into a
    /// `number` one, so a sort over it compares numerically from then on — and
    /// it refuses, naming ids and values, while a value still does not fit.
    #[tokio::test(flavor = "multi_thread")]
    async fn retype_action_reports_what_blocks_it_then_migrates_the_column() {
        let store = mem_store("cc_retype_action").await;
        let adapter = CustomColumnsAdapter::new(taiga_adapter(), store.clone());

        // Two rows, both typed `text` — the default the forms offer.
        for (row, rank) in [("ISS-1", "30"), ("root", "later")] {
            let mut node = adapter.get_by_id(row).await.unwrap();
            node.execute(
                EDIT_CELLS_ACTION_ID,
                ActionInput::ColumnForm(vec![cell("rank", rank, "text")]),
            )
            .await
            .unwrap();
        }

        let retype = |t: &str| {
            HashMap::from([
                ("column_key".to_string(), "rank".to_string()),
                ("value_type".to_string(), t.to_string()),
            ])
        };

        // `later` blocks the retype and is named with its row id.
        let mut node = adapter.get_by_id("ISS-1").await.unwrap();
        let err = match node
            .execute(RETYPE_COLUMN_ACTION_ID, ActionInput::Form(retype("number")))
            .await
        {
            Err(e) => format!("{e}"),
            Ok(_) => panic!("retype should have refused while `later` is stored"),
        };
        assert!(err.contains("root: `later`"), "got: {err}");
        assert_eq!(
            adapter.describe_columns("mock:issue").await[0].value_type,
            "text"
        );

        // Correct the offending value, then the same retype succeeds.
        let mut node = adapter.get_by_id("root").await.unwrap();
        node.execute(
            EDIT_CELLS_ACTION_ID,
            ActionInput::ColumnForm(vec![cell("rank", "40", "text")]),
        )
        .await
        .unwrap();
        let mut node = adapter.get_by_id("ISS-1").await.unwrap();
        node.execute(RETYPE_COLUMN_ACTION_ID, ActionInput::Form(retype("number")))
            .await
            .unwrap();

        // The described column — what a sort resolves its `SortKind` from —
        // now says number.
        let cols = adapter.describe_columns("mock:issue").await;
        let rank = cols.iter().find(|c| c.key == "rank").unwrap();
        assert_eq!(rank.value_type, "number");
    }

    /// An empty value in the `ColumnForm` clears the cell rather than storing an
    /// empty string, and an unchanged value is a no-op (nothing "changed").
    #[tokio::test(flavor = "multi_thread")]
    async fn column_form_clears_on_empty_and_skips_unchanged() {
        let store = mem_store("cc_clear").await;
        let scope = "taiga/t1";
        let adapter = CustomColumnsAdapter::new(taiga_adapter(), store.clone());

        // Seed a value.
        {
            let mut node = adapter.get_by_id("ISS-1").await.unwrap();
            node.execute(
                EDIT_CELLS_ACTION_ID,
                ActionInput::ColumnForm(vec![cell("note", "look into", "text")]),
            )
            .await
            .unwrap();
        }

        // Re-open (so the node's cached cells reflect the seeded value), then
        // submit the same value for `note` plus an empty `estimate`.
        let mut node = adapter.get_by_id("ISS-1").await.unwrap();
        node.execute(
            EDIT_CELLS_ACTION_ID,
            ActionInput::ColumnForm(vec![
                cell("note", "look into", "text"),
                cell("estimate", "", "number"),
            ]),
        )
        .await
        .unwrap();

        // `note` still set (unchanged, no rewrite churn), `estimate` never created
        // (empty value clears/skips, so no bootstrap of an empty column).
        let cells = store.get_for_row(scope, "ISS-1").await.unwrap();
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].column_key, "note");
        assert!(
            store
                .columns(scope, "mock:issue")
                .await
                .unwrap()
                .iter()
                .all(|c| c.key != "estimate")
        );
    }

    /// The whole point of the second declaration channel: a column that exists
    /// only in the local store has to reach a front-end through the *same*
    /// list the adapter's own columns come out of, or nothing can offer it.
    /// `columns_for` is where the two are unioned, so this is the seam that
    /// decides whether a custom column can be sorted or filtered at all.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_stored_column_joins_the_adapters_own_in_columns_for() {
        let store = mem_store("cc_columns_for").await;
        // Unlike `taiga_adapter`, this one declares a column of its own, so the
        // union has two sides to it.
        let inner = MockAdapterBuilder::new("taiga")
            .instance_id("t1")
            .node(
                MockNodeData::new("root", "Root")
                    .child_type_with_columns(
                        issue_type(),
                        vec![ColumnSchema::new("status", "Status")],
                    )
                    .child(
                        MockNodeData::new("ISS-1", "Bug")
                            .node_type(issue_type())
                            .meta("status", "Open"),
                    ),
            )
            .build();
        let adapter = CustomColumnsAdapter::new(Box::new(inner), store.clone());

        let mut node = adapter.get_by_id("ISS-1").await.unwrap();
        node.execute(
            EDIT_CELLS_ACTION_ID,
            ActionInput::ColumnForm(vec![cell("estimate", "5", "number")]),
        )
        .await
        .unwrap();

        let root = adapter.root().await.unwrap();
        let columns =
            not_yet_done_content::children::columns_for(&adapter, root.as_ref(), &issue_type())
                .await;

        let est = columns
            .iter()
            .find(|c| c.key == "estimate")
            .expect("the stored column is missing from columns_for");
        // Sortable and in the rows — the decorator projects every defined
        // column into every row it injects into, blank where no cell is
        // stored, so both a sort and an `is_null` see a real cell everywhere.
        assert!(est.sortable);
        assert!(est.in_rows);
        // The store's type survives the union, so the column sorts numerically
        // rather than as text.
        assert_eq!(est.sort_kind(), not_yet_done_content::SortKind::Number);
        // And it joins the adapter's own columns instead of replacing them.
        assert!(columns.iter().any(|c| c.key == "status"));
    }

    /// A stored column can collide with one the adapter declares — nothing stops
    /// a user from naming a custom column `status`. The store's statement wins
    /// (it may have been retyped) *except* for the display name it does not
    /// have: a local column carries no label, and taking that as "no label"
    /// would leave the user staring at a raw key where a name used to be.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_stored_column_that_shadows_a_declared_one_keeps_its_label() {
        let store = mem_store("cc_shadow").await;
        let inner = MockAdapterBuilder::new("taiga")
            .instance_id("t1")
            .node(
                MockNodeData::new("root", "Root")
                    .child_type_with_columns(
                        issue_type(),
                        vec![ColumnSchema::new("status", "Status")],
                    )
                    .child(
                        MockNodeData::new("ISS-1", "Bug")
                            .node_type(issue_type())
                            .meta("status", "Open"),
                    ),
            )
            .build();
        let adapter = CustomColumnsAdapter::new(Box::new(inner), store.clone());

        let mut node = adapter.get_by_id("ISS-1").await.unwrap();
        node.execute(
            EDIT_CELLS_ACTION_ID,
            ActionInput::ColumnForm(vec![cell("status", "42", "number")]),
        )
        .await
        .unwrap();

        let root = adapter.root().await.unwrap();
        let columns =
            not_yet_done_content::children::columns_for(&adapter, root.as_ref(), &issue_type())
                .await;

        // One column, not two: the keys collide, so the union merges them.
        assert_eq!(columns.iter().filter(|c| c.key == "status").count(), 1);
        let status = columns.iter().find(|c| c.key == "status").unwrap();
        assert_eq!(status.display_label(), "Status");
        assert_eq!(status.value_type, "number");
    }

    /// Three issues with ranks out of order, and a store that knows the rank
    /// column. Build them once for the two sorting tests below.
    async fn ranked_adapter(store: Arc<LocalColumnStore>) -> CustomColumnsAdapter {
        let inner = MockAdapterBuilder::new("taiga")
            .instance_id("t1")
            .node(
                MockNodeData::new("root", "Root")
                    .child_type_with_columns(
                        issue_type(),
                        vec![ColumnSchema::new("status", "Status")],
                    )
                    .child(
                        MockNodeData::new("ISS-1", "One")
                            .node_type(issue_type())
                            .meta("status", "Open"),
                    )
                    .child(
                        MockNodeData::new("ISS-2", "Two")
                            .node_type(issue_type())
                            .meta("status", "Open"),
                    )
                    .child(
                        MockNodeData::new("ISS-3", "Three")
                            .node_type(issue_type())
                            .meta("status", "Open"),
                    ),
            )
            .build();
        let adapter = CustomColumnsAdapter::new(Box::new(inner), store);
        for (id, rank) in [("ISS-1", "30"), ("ISS-2", "10"), ("ISS-3", "20")] {
            let mut node = adapter.get_by_id(id).await.unwrap();
            node.execute(
                EDIT_CELLS_ACTION_ID,
                ActionInput::ColumnForm(vec![cell("rank", rank, "number")]),
            )
            .await
            .unwrap();
        }
        adapter
    }

    fn list_params(page: Option<not_yet_done_content::PageRequest>) -> ListParams {
        ListParams {
            node_type: issue_type(),
            query: None,
            sort: vec![not_yet_done_content::SortKey {
                column: "rank".into(),
                direction: not_yet_done_content::SortDirection::Asc,
            }],
            page,
            download: false,
            group_by: None,
        }
    }

    /// The point of a custom column that holds a rank. The inner adapter never
    /// heard of `rank`, so it returns the rows in its own order and reports an
    /// empty `applied_sort`; the list path finishes the job from the described
    /// columns and says so.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_sort_the_adapter_could_not_serve_is_finished_on_the_way_out() {
        let adapter = ranked_adapter(mem_store("cc_sort").await).await;
        let root = adapter.root().await.unwrap();

        let result =
            not_yet_done_content::children::list(&adapter, root.as_ref(), list_params(None))
                .await
                .unwrap();

        let order: Vec<&str> = result.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(order, vec!["ISS-2", "ISS-3", "ISS-1"], "ranks 10, 20, 30");
        // Reported, not just done — the footer tells the user what took effect.
        assert_eq!(result.applied_sort.len(), 1);
        assert_eq!(result.applied_sort[0].column, "rank");
    }

    /// The same sort over one page of a longer result is refused. These three
    /// rows are a sample of the query, and ordering a sample would present it
    /// as the whole — so the rows stay as they came and `applied_sort` stays
    /// empty rather than claiming an order the result does not have.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_page_of_a_longer_result_is_left_in_the_adapters_order() {
        let adapter = ranked_adapter(mem_store("cc_sort_paged").await).await;
        let root = adapter.root().await.unwrap();

        let page = not_yet_done_content::PageRequest {
            offset: 0,
            limit: 2,
        };
        let result =
            not_yet_done_content::children::list(&adapter, root.as_ref(), list_params(Some(page)))
                .await
                .unwrap();

        let order: Vec<&str> = result.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(order, vec!["ISS-1", "ISS-2"], "untouched, ranks 30 then 10");
        assert!(result.applied_sort.is_empty());
    }
}
