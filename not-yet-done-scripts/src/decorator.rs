//! The adapter-action surface for view-scripts — a factory/adapter/node
//! decorator that exposes the [`ScriptRepo`] CRUD kernel through the ordinary
//! [`ContentAdapter`]/[`Node`] protocol, so **any** front-end reaches it with
//! no bespoke wiring.
//!
//! # Why a decorator (mirrors custom-columns)
//!
//! View-scripts are a local capability layered on top of every adapter, not
//! adapter content — so no adapter implements anything. The host wraps every
//! factory in [`scripts_factory`], and each produced adapter is wrapped in a
//! [`ScriptsAdapter`]. Every node the adapter hands out is wrapped in a
//! [`ScriptsNode`], which injects two synthetic actions:
//!
//! * `scripts` ([`SCRIPTS_ACTION_ID`], `InputSpec::None`) — list the scripts at
//!   this view level; the outcome message is one addressable script id per line.
//! * `script-new` ([`SCRIPT_NEW_ACTION_ID`], `InputSpec::Form`) — create a new
//!   empty script from a `name`.
//!
//! An individual script is itself addressable as a [`ScriptNode`] via the
//! synthetic id `script:<esc-seg…>/<name>` (see [`encode_script_id`]): the
//! adapter's [`ContentAdapter::get_by_id`] recognises the `script:` prefix and
//! resolves the leaf directly. From there the ordinary machinery applies — `cat`
//! reads [`Node::content`], `edit` is an `InputSpec::Editor` action, and
//! `delete` is an [`ActionDispatch::DeleteSelf`]. No new input plumbing.
//!
//! # Scope derivation
//!
//! The scope of a level is `<adapter_type>/<node-type path>`, reproducing the
//! TUI's on-disk layout. Because CLI navigation resolves each node via
//! `get_by_id` (which carries no ancestor context), a node resolved that way is
//! scoped to its **own** node type — correct for the common top-level case and
//! for the adapter root (scope `[]`). Navigation via [`Node::get_child`] threads
//! the full path (parent segments + child type). The TUI does not go through
//! this decorator at all — it builds the same [`ScriptScope`] from its view-path
//! and talks to [`ScriptRepo`] directly — so the two only need to agree on the
//! path scheme, which they do by construction.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use not_yet_done_content::*;

use crate::{ScriptRepo, ScriptScope};

/// Stable id of the synthetic "list this level's scripts" action.
pub const SCRIPTS_ACTION_ID: &str = "scripts";
/// Stable id of the synthetic "create a new script" action.
pub const SCRIPT_NEW_ACTION_ID: &str = "script-new";
/// Stable id of the per-script "edit" action.
pub const SCRIPT_EDIT_ACTION_ID: &str = "edit";
/// Stable id of the per-script "delete" action.
pub const SCRIPT_DELETE_ACTION_ID: &str = "delete";
/// Prefix marking a synthetic script-node id in [`ContentAdapter::get_by_id`].
pub const SCRIPT_ID_PREFIX: &str = "script:";

/// Encode a script's addressable node id: `script:<esc-seg…>/<name>`. Segments
/// are the scope's filesystem-escaped node types; with no segments the id is
/// simply `script:<name>`.
pub fn encode_script_id(scope: &ScriptScope, name: &str) -> String {
    let mut path = scope.escaped_segments();
    path.push(name.to_string());
    format!("{SCRIPT_ID_PREFIX}{}", path.join("/"))
}

/// Parse a `script:<esc-seg…>/<name>` id back into a scope (under `adapter`) and
/// the script name. Returns `None` when `id` is not a script id. The recovered
/// segments are the escaped forms; since escaping is idempotent, the resulting
/// [`ScriptScope::dir`] is identical to the level scope that produced the id.
pub fn parse_script_id(adapter: &str, id: &str) -> Option<(ScriptScope, String)> {
    let rest = id.strip_prefix(SCRIPT_ID_PREFIX)?;
    let mut parts: Vec<&str> = rest.split('/').collect();
    let name = parts.pop().filter(|n| !n.is_empty())?.to_string();
    let segments = parts.into_iter().map(str::to_string).collect();
    Some((ScriptScope::new(adapter, segments), name))
}

fn io_err(e: std::io::Error) -> ContentError {
    ContentError::Other(Box::new(e))
}

// ---------------------------------------------------------------------------
// Factory decorator
// ---------------------------------------------------------------------------

/// Wrap a factory so every adapter it produces exposes the view-script CRUD
/// surface. Applied to every registered factory by the host, making scripts
/// universally addressable across front-ends. Inert until a `scripts`/`script-*`
/// action is invoked, so this is free in normal traversal.
pub fn scripts_factory(inner: Box<dyn AdapterFactory>) -> Box<dyn AdapterFactory> {
    Box::new(ScriptsFactory { inner })
}

struct ScriptsFactory {
    inner: Box<dyn AdapterFactory>,
}

impl AdapterFactory for ScriptsFactory {
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
        Ok(Box::new(ScriptsAdapter::new(
            adapter,
            ScriptRepo::default(),
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

/// Wraps a [`ContentAdapter`], wrapping every node it serves in a
/// [`ScriptsNode`] and resolving `script:` ids to a [`ScriptNode`]. Everything
/// else is delegated verbatim.
pub struct ScriptsAdapter {
    inner: Box<dyn ContentAdapter>,
    repo: ScriptRepo,
}

impl ScriptsAdapter {
    pub fn new(inner: Box<dyn ContentAdapter>, repo: ScriptRepo) -> Self {
        Self { inner, repo }
    }

    /// Wrap `inner` as a [`ScriptsNode`] scoped to `segments` (the node-type
    /// path below the adapter root).
    fn wrap(&self, inner: Box<dyn Node>, segments: Vec<String>) -> Box<dyn Node> {
        let scope = ScriptScope::new(self.inner.adapter_type().to_string(), segments);
        Box::new(ScriptsNode::new(inner, self.repo.clone(), scope))
    }
}

#[async_trait]
impl ContentAdapter for ScriptsAdapter {
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
        // The adapter root is the tab level: scope with no segments.
        Ok(self.wrap(self.inner.root().await?, Vec::new()))
    }

    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
        // A synthetic script id resolves straight to its leaf node.
        if let Some((scope, name)) = parse_script_id(self.inner.adapter_type(), id) {
            return Ok(Box::new(ScriptNode::new(self.repo.clone(), scope, name)));
        }
        // Otherwise resolve the real node and scope it to its own node type —
        // id-based navigation carries no ancestor context (see module docs).
        let inner = self.inner.get_by_id(id).await?;
        let segments = vec![inner.node_type().type_id.clone()];
        Ok(self.wrap(inner, segments))
    }

    fn childs<'a>(&'a self, node: &'a dyn Node) -> Vec<Child<'a>> {
        self.inner.childs(node)
    }

    async fn eager_subtree(
        &self,
        node: &dyn Node,
        params: &ListParams,
        depth: u32,
    ) -> Option<Result<Subtree>> {
        self.inner.eager_subtree(node, params, depth).await
    }

    async fn download_asset(&self, url: &str) -> Result<Vec<u8>> {
        self.inner.download_asset(url).await
    }
    fn actions_for_type(&self, node_type: &NodeType) -> Vec<NodeAction> {
        // Script leaf files carry their own CRUD surface; every other (wrapped)
        // node type gets the injected `scripts` listing/create actions layered
        // on top of the inner adapter's type-level set. This mirrors the node
        // wrappers ([`ScriptsNode`]/[`ScriptNode`]) so the type-level set is the
        // single source of truth for what an instance exposes.
        if node_type.type_id == SCRIPT_NODE_TYPE_ID {
            return script_leaf_actions();
        }
        let mut actions = self.inner.actions_for_type(node_type);
        for a in scripts_actions() {
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
    async fn describe_columns(&self, node_type: &str) -> Vec<ColumnSchema> {
        self.inner.describe_columns(node_type).await
    }
}

// ---------------------------------------------------------------------------
// Node decorator (level node)
// ---------------------------------------------------------------------------

/// The `type_id` a script leaf file carries (see [`script_node_type`]).
const SCRIPT_NODE_TYPE_ID: &str = "script";

/// The synthetic actions injected onto every wrapped node.
fn scripts_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new(SCRIPTS_ACTION_ID, "list scripts", InputSpec::None),
        NodeAction::new(
            SCRIPT_NEW_ACTION_ID,
            "new script",
            InputSpec::Form {
                fields: vec![FormFieldSpec::text("name", "Script name")],
            },
        ),
    ]
}

/// The CRUD surface a script leaf file exposes.
fn script_leaf_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new(SCRIPT_EDIT_ACTION_ID, "edit script", InputSpec::Editor),
        NodeAction::new(SCRIPT_DELETE_ACTION_ID, "delete script", InputSpec::None),
    ]
}

/// Wraps a [`Node`], injecting the level's script CRUD surface. Non-script
/// behaviour is delegated verbatim to the inner node.
pub struct ScriptsNode {
    inner: Box<dyn Node>,
    repo: ScriptRepo,
    scope: ScriptScope,
}

impl ScriptsNode {
    pub fn new(inner: Box<dyn Node>, repo: ScriptRepo, scope: ScriptScope) -> Self {
        Self { inner, repo, scope }
    }

    /// Build the `scripts` listing message — one addressable script id per line,
    /// or a note naming the (empty) directory.
    fn list_message(&self) -> Result<String> {
        let entries = self.repo.list(&self.scope).map_err(io_err)?;
        if entries.is_empty() {
            return Ok(format!(
                "No scripts at this level.\nDirectory: {}",
                self.repo.dir(&self.scope).display()
            ));
        }
        Ok(entries
            .into_iter()
            .map(|e| encode_script_id(&self.scope, &e.name))
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

#[async_trait]
impl Node for ScriptsNode {
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
        self.inner.metadata()
    }

    async fn hydrate(&mut self) {
        self.inner.hydrate().await;
    }

    fn row_summary(&self) -> NodeSummary {
        self.inner.row_summary()
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        // A script id resolves to its leaf node regardless of how it is reached.
        if let Some((scope, name)) = parse_script_id(&self.scope.adapter, id) {
            return Ok(Box::new(ScriptNode::new(self.repo.clone(), scope, name)));
        }
        // Real child: thread the full node-type path (parent segments + child).
        let inner = self.inner.get_child(id).await?;
        let mut segments = self.scope.segments.clone();
        segments.push(inner.node_type().type_id.clone());
        let scope = ScriptScope::new(self.scope.adapter.clone(), segments);
        Ok(Box::new(ScriptsNode::new(inner, self.repo.clone(), scope)))
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
        self.inner.form_prep(action_id).await
    }

    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        match action_id {
            SCRIPTS_ACTION_ID => Ok(ActionOutcome::Done {
                message: Some(self.list_message()?),
            }),
            SCRIPT_NEW_ACTION_ID => {
                let fields = form_fields(input)?;
                let name = required(&fields, "name")?;
                let path = self.repo.create(&self.scope, &name, "").map_err(io_err)?;
                let file = path
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or(name);
                Ok(ActionOutcome::Done {
                    message: Some(format!(
                        "Created script {file}\nEdit it via id: {}",
                        encode_script_id(&self.scope, &file)
                    )),
                })
            }
            _ => self.inner.execute(action_id, input).await,
        }
    }
}

// ---------------------------------------------------------------------------
// Script leaf node
// ---------------------------------------------------------------------------

/// A single script file, exposed as a leaf [`Node`] with readable content and
/// `edit`/`delete` actions. Addressed by the synthetic id [`encode_script_id`].
pub struct ScriptNode {
    repo: ScriptRepo,
    scope: ScriptScope,
    name: String,
    id: String,
    node_type: NodeType,
    metadata: Metadata,
}

impl ScriptNode {
    pub fn new(repo: ScriptRepo, scope: ScriptScope, name: String) -> Self {
        let id = encode_script_id(&scope, &name);
        let node_type = script_node_type(&name);
        Self {
            repo,
            scope,
            name,
            id,
            node_type,
            metadata: Metadata::default(),
        }
    }
}

/// A `NodeType` for a script file, taking its extension/syntax from the name.
fn script_node_type(name: &str) -> NodeType {
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str());
    NodeType {
        type_id: "script".into(),
        mime_type: "text/plain".into(),
        syntax: ext.map(str::to_string),
        file_extension: ext.map(|e| format!(".{e}")).unwrap_or_default(),
        display_name: "Script".into(),
    }
}

#[async_trait]
impl Node for ScriptNode {
    fn id(&self) -> &str {
        &self.id
    }
    fn label(&self) -> &str {
        &self.name
    }
    fn node_type(&self) -> &NodeType {
        &self.node_type
    }
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn content(&self) -> Option<&dyn Content> {
        Some(self)
    }
    async fn invoke_action(&self, name: &str, _ctx: &ActionContext) -> Result<ActionDispatch> {
        match name {
            SCRIPT_DELETE_ACTION_ID => Ok(ActionDispatch::DeleteSelf {
                confirm: Some(format!("Delete script '{}'? (y/n)", self.name)),
            }),
            _ => Ok(ActionDispatch::Noop),
        }
    }

    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        if action_id == SCRIPT_EDIT_ACTION_ID {
            let template = self.repo.read(&self.scope, &self.name).map_err(io_err)?;
            return Ok(EditorPrep {
                template,
                version: String::new(),
                suffix: self.node_type.file_extension.clone(),
                file_path: None,
            });
        }
        Err(ContentError::NotSupported(format!(
            "script node has no editor for '{action_id}'"
        )))
    }

    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        match (action_id, input) {
            (SCRIPT_EDIT_ACTION_ID, ActionInput::Edited { text, .. }) => {
                self.repo
                    .write(&self.scope, &self.name, &text)
                    .map_err(io_err)?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("Saved script {}", self.name)),
                })
            }
            (SCRIPT_DELETE_ACTION_ID, _) => {
                self.repo.delete(&self.scope, &self.name).map_err(io_err)?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("Deleted script {}", self.name)),
                })
            }
            (other, _) => Err(ContentError::NotSupported(format!(
                "script node has no action '{other}'"
            ))),
        }
    }
}

#[async_trait]
impl Content for ScriptNode {
    fn node_type(&self) -> &NodeType {
        &self.node_type
    }
    fn version(&self) -> Option<&str> {
        None
    }
    async fn read(&self) -> Result<Vec<u8>> {
        Ok(self
            .repo
            .read(&self.scope, &self.name)
            .map_err(io_err)?
            .into_bytes())
    }
}

// ---------------------------------------------------------------------------
// Input helpers
// ---------------------------------------------------------------------------

fn form_fields(input: ActionInput) -> Result<HashMap<String, String>> {
    match input {
        ActionInput::Form(map) => Ok(map),
        _ => Err(ContentError::Other(
            "script action expects form input".into(),
        )),
    }
}

fn required(fields: &HashMap<String, String>, key: &str) -> Result<String> {
    match fields.get(key) {
        Some(v) if !v.trim().is_empty() => Ok(v.clone()),
        _ => Err(ContentError::Other(
            format!("script action requires a non-empty `{key}`").into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_content::mock::{issue_type, MockAdapterBuilder, MockNodeData};

    fn adapter(repo: ScriptRepo) -> ScriptsAdapter {
        let inner = MockAdapterBuilder::new("jira")
            .instance_id("j1")
            .node(
                MockNodeData::new("root", "Root")
                    .child_type(issue_type())
                    .child(MockNodeData::new("ISS-1", "Bug").node_type(issue_type())),
            )
            .build();
        ScriptsAdapter::new(Box::new(inner), repo)
    }

    fn repo() -> (tempfile::TempDir, ScriptRepo) {
        let t = tempfile::tempdir().unwrap();
        let repo = ScriptRepo::new(t.path());
        (t, repo)
    }

    #[test]
    fn id_round_trips_through_encode_parse() {
        let scope = ScriptScope::new("jira", vec!["jira:issue".into()]);
        let id = encode_script_id(&scope, "foo.py");
        assert_eq!(id, "script:jira_issue/foo.py");
        let (parsed, name) = parse_script_id("jira", &id).unwrap();
        assert_eq!(name, "foo.py");
        // Escaped segments recover to the same on-disk directory.
        assert_eq!(
            parsed.dir(std::path::Path::new("/r")),
            scope.dir(std::path::Path::new("/r"))
        );
        // Non-script ids are rejected.
        assert!(parse_script_id("jira", "ISS-1").is_none());
    }

    #[test]
    fn empty_scope_id_round_trips() {
        let scope = ScriptScope::new("jira", vec![]);
        let id = encode_script_id(&scope, "top.py");
        assert_eq!(id, "script:top.py");
        let (parsed, name) = parse_script_id("jira", &id).unwrap();
        assert_eq!(name, "top.py");
        assert!(parsed.segments.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn resolved_node_carries_its_own_type_scope() {
        let (_t, repo) = repo();
        let a = adapter(repo);
        let node = a.get_by_id("ISS-1").await.unwrap();
        // The scripts action lists this level's (empty) directory.
        let mut node = node;
        let outcome = node
            .execute(SCRIPTS_ACTION_ID, ActionInput::None)
            .await
            .unwrap();
        match outcome {
            ActionOutcome::Done { message } => {
                assert!(message.unwrap().contains("jira/mock_issue"));
            }
            _ => panic!("expected Done"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_list_cat_edit_delete_full_cycle() {
        let (_t, repo) = repo();
        let a = adapter(repo);

        // Create a script at the issue level.
        let mut node = a.get_by_id("ISS-1").await.unwrap();
        node.execute(
            SCRIPT_NEW_ACTION_ID,
            ActionInput::Form(HashMap::from([("name".to_string(), "report".to_string())])),
        )
        .await
        .unwrap();

        // List surfaces the addressable id.
        let listed = match node
            .execute(SCRIPTS_ACTION_ID, ActionInput::None)
            .await
            .unwrap()
        {
            ActionOutcome::Done { message } => message.unwrap(),
            _ => panic!("expected Done"),
        };
        let id = "script:mock_issue/report.py";
        assert!(listed.contains(id), "listing: {listed}");

        // Resolve the leaf directly and edit it.
        let mut leaf = a.get_by_id(id).await.unwrap();
        assert_eq!(leaf.label(), "report.py");
        leaf.execute(
            SCRIPT_EDIT_ACTION_ID,
            ActionInput::Edited {
                text: "print('hi')".into(),
                original: String::new(),
                version: String::new(),
            },
        )
        .await
        .unwrap();

        // cat reads the body back through Content.
        let leaf = a.get_by_id(id).await.unwrap();
        let body = leaf.content().unwrap().read_text().await.unwrap();
        assert_eq!(body, "print('hi')");

        // delete: invoke_action asks for confirmation, execute removes it.
        let mut leaf = a.get_by_id(id).await.unwrap();
        let dispatch = leaf
            .invoke_action(SCRIPT_DELETE_ACTION_ID, &ActionContext::default())
            .await
            .unwrap();
        assert!(matches!(dispatch, ActionDispatch::DeleteSelf { .. }));
        leaf.execute(SCRIPT_DELETE_ACTION_ID, ActionInput::None)
            .await
            .unwrap();

        // Gone from the listing.
        let listed = match a
            .get_by_id("ISS-1")
            .await
            .unwrap()
            .execute(SCRIPTS_ACTION_ID, ActionInput::None)
            .await
            .unwrap()
        {
            ActionOutcome::Done { message } => message.unwrap(),
            _ => panic!("expected Done"),
        };
        assert!(!listed.contains(id));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn wrapped_node_keeps_inner_actions_plus_scripts() {
        let (_t, repo) = repo();
        let a = adapter(repo);
        let node = a.get_by_id("ISS-1").await.unwrap();
        let ids: Vec<String> = a
            .actions_for_type(node.node_type())
            .into_iter()
            .map(|x| x.id)
            .collect();
        assert!(ids.contains(&SCRIPTS_ACTION_ID.to_string()));
        assert!(ids.contains(&SCRIPT_NEW_ACTION_ID.to_string()));
    }
}
