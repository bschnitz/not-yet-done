//! The container-level script tree, as [`Node`]s.
//!
//! Every SQL adapter grows the same branch below its container level
//! (a Postgres database, a SQLite file): a `DB Scripts` group folder,
//! arbitrarily nested folders below it, and script leaves that can be
//! executed, edited, renamed, moved and deleted. The storage half of
//! that already lives in [`crate::script_files`] /
//! [`crate::script_store`]; this module adds the node tree on top, so an
//! adapter gains the whole branch by holding one [`DbScriptTree`].
//!
//! The only backend-specific thing about these nodes is the `<adapter>:`
//! prefix of their type ids, which is why the three [`NodeType`]s are
//! built once per adapter into [`DbScriptNodeTypes`] and shared through
//! the tree rather than being `static`s.
//!
//! What stays adapter-side: how a script's SQL is *run*. The `execute`
//! action returns [`ActionDispatch::ExecuteQuery`] with the container key
//! (`database`) the host routes back into
//! `ContentAdapter::execute_custom_query`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use not_yet_done_content::{
    ActionContext, ActionDispatch, ActionInput, ActionOutcome, Content, ContentError, EditorPrep,
    FormFieldSpec, InputSpec, ListResult, Metadata, MetadataField, Node, NodeAction, NodeSummary,
    NodeType, Result, ScriptStore,
};

use crate::script_files as files;
use crate::script_store::SqlScriptStore;

/// Id segment that separates a container from its script tree:
/// `<key>/db_scripts/<rel_path…>`. The host keys its TUI-owned
/// mark/paste flow on exactly this shape, so it must not be renamed
/// per adapter.
pub const DB_SCRIPTS_GROUP_ID: &str = "db_scripts";

// ---------------------------------------------------------------------------
// Node types
// ---------------------------------------------------------------------------

/// The three node types the script branch introduces, prefixed for one
/// adapter. Held by [`DbScriptTree`] because [`Node::node_type`] returns
/// a reference and the prefix is only known at runtime.
pub struct DbScriptNodeTypes {
    pub group: NodeType,
    pub dir: NodeType,
    pub script: NodeType,
}

impl DbScriptNodeTypes {
    /// Build `<adapter>:db_scripts`, `<adapter>:db_script_dir` and
    /// `<adapter>:db_script` — the type ids a view spec refers to.
    pub fn for_adapter(adapter_type: &str) -> Self {
        Self {
            group: NodeType {
                type_id: format!("{adapter_type}:{DB_SCRIPTS_GROUP_ID}"),
                mime_type: String::new(),
                syntax: None,
                file_extension: String::new(),
                display_name: "DB Scripts".into(),
            },
            dir: NodeType {
                type_id: format!("{adapter_type}:db_script_dir"),
                mime_type: String::new(),
                syntax: None,
                file_extension: String::new(),
                display_name: "DB Script Folder".into(),
            },
            script: NodeType {
                type_id: format!("{adapter_type}:db_script"),
                mime_type: "text/x-sql".into(),
                syntax: Some("sql".into()),
                file_extension: "sql".into(),
                display_name: "DB Script".into(),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Metadata
// ---------------------------------------------------------------------------

fn field(key: &str, label: &str, value: &str) -> MetadataField {
    MetadataField {
        key: key.into(),
        value: value.into(),
        display_label: label.into(),
        editable: false,
        allowed_values: None,
    }
}

/// Metadata for a single script row. `label` is what the list view
/// shows in the `script` column — typically the leaf name (`"audit"`)
/// rather than the full rel_path (`"util/audit"`), so a long folder
/// chain doesn't crowd out the other columns.
fn script_metadata(key: &str, label: &str) -> Metadata {
    Metadata {
        fields: vec![
            field("script", "Script", label),
            field("database", "Database", key),
        ],
    }
}

/// Metadata for a folder row. Mirrors [`script_metadata`] so a generic
/// table view can render both kinds against the same column set; the
/// `script` field carries the folder name.
fn dir_metadata(key: &str, label: &str) -> Metadata {
    script_metadata(key, label)
}

// ---------------------------------------------------------------------------
// Action sets
// ---------------------------------------------------------------------------

/// Actions on the `DB Scripts` group folder: create a script or a
/// subfolder at the container root. Both collect their name through
/// `InputSpec::Form` and run via `execute`, so the TUI form popup and
/// the CLI's `--field name=…` share one write path.
pub fn group_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new(
            "add-script",
            "add",
            InputSpec::Form {
                fields: vec![FormFieldSpec::text("name", "Script name")],
            },
        ),
        NodeAction::new(
            "add-dir",
            "add-dir",
            InputSpec::Form {
                fields: vec![FormFieldSpec::text("name", "Directory name")],
            },
        ),
    ]
}

/// Actions on a folder inside the script tree. Suggested `shortcuts:`
/// in a view spec: `a`/`A` add-script/add-dir, `r` rename, `m`/`p`
/// mark/paste move, `M` move (target prompt), `d` delete-dir.
pub fn dir_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new(
            "add-script",
            "add",
            InputSpec::Form {
                fields: vec![FormFieldSpec::text("name", "Script name")],
            },
        ),
        NodeAction::new(
            "add-dir",
            "add-dir",
            InputSpec::Form {
                fields: vec![FormFieldSpec::text("name", "Directory name")],
            },
        ),
        NodeAction::new(
            "rename",
            "rename",
            InputSpec::Form {
                fields: vec![FormFieldSpec::text("name", "New name")],
            },
        ),
        NodeAction::new("mark-move", "mark", InputSpec::None),
        NodeAction::new("paste-move", "paste", InputSpec::None),
        // Adapter-executed move (CLI parity): the mark/paste pair above stays
        // the TUI's navigate-and-drop idiom, while `move` collects a target
        // folder up front (`InputSpec::Form`) and relocates the entry through
        // the `ScriptStore` — the same code path the CLI drives. Both keep the
        // entry's own name; see [`move_target_rel`].
        NodeAction::new(
            "move",
            "move",
            InputSpec::Form {
                fields: vec![FormFieldSpec::text(
                    "target",
                    "Target folder (empty = root)",
                )],
            },
        ),
        NodeAction::new("delete-dir", "del-dir", InputSpec::None),
    ]
}

/// Actions on a script leaf. Suggested `shortcuts:` in a view spec:
/// `x` execute, `e` edit, `r` rename, `m` mark-move, `M` move,
/// `d` delete. Only `edit` belongs in the (highlighted) action bar —
/// editor-action convention.
pub fn script_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new("execute", "exec", InputSpec::None),
        // `InputSpec::Editor` is the generic editor contract the CLI honors
        // (`do_editor` → `prepare` → `$EDITOR` → `execute(Edited)`). The TUI's
        // keyboard `e` still routes through `invoke_action`, which returns the
        // richer `OpenEditor { script_editor }` dispatch (in-place session that
        // also re-runs the query) — `InputSpec::Editor` is not in the popup
        // reroute set, so the two front-ends share the storage but not the UX.
        NodeAction::new("edit", "edit", InputSpec::Editor),
        NodeAction::new(
            "rename",
            "rename",
            InputSpec::Form {
                fields: vec![FormFieldSpec::text("name", "New name")],
            },
        ),
        NodeAction::new("mark-move", "mark", InputSpec::None),
        // Adapter-executed move (CLI parity) — see the twin action on
        // [`dir_actions`]. `m` marks for the TUI paste idiom; `M` prompts for
        // a target folder and moves via the `ScriptStore`.
        NodeAction::new(
            "move",
            "move",
            InputSpec::Form {
                fields: vec![FormFieldSpec::text(
                    "target",
                    "Target folder (empty = root)",
                )],
            },
        ),
        NodeAction::new("delete", "del", InputSpec::None),
    ]
}

// ---------------------------------------------------------------------------
// Form input validation
// ---------------------------------------------------------------------------

/// Wrap a user-facing validation message as a [`ContentError`].
fn script_input_err(msg: impl Into<String>) -> ContentError {
    ContentError::Other(msg.into().into())
}

/// Read + validate the `name` field of an `add-script` / `add-dir` /
/// `rename` form. Mirrors the filesystem layer's own rules (no path
/// separators, no leading dot) so the user gets a clean error before
/// any io runs.
fn form_entry_name(input: &ActionInput) -> Result<String> {
    let values = match input {
        ActionInput::Form(v) => v,
        _ => {
            return Err(ContentError::NotSupported(
                "this action requires a form input".into(),
            ));
        }
    };
    let name = values
        .get("name")
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| script_input_err("field 'name' is required"))?;
    if name.contains('/') || name.contains('\\') || name.starts_with('.') {
        return Err(script_input_err(format!(
            "invalid name '{name}' (no slashes or leading dot)"
        )));
    }
    Ok(name.to_string())
}

/// Read the `target` field of a `move` form. The target is a
/// destination *folder* relative to `db_scripts/<key>/`; it may be empty
/// (= the db_scripts root) but must be POSIX-shaped (`/` separators
/// only) so the on-disk path stays portable.
fn form_target_dir(input: &ActionInput) -> Result<String> {
    let values = match input {
        ActionInput::Form(v) => v,
        _ => {
            return Err(ContentError::NotSupported(
                "this action requires a form input".into(),
            ));
        }
    };
    let target = values
        .get("target")
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    if target.contains('\\') {
        return Err(script_input_err(
            "invalid target (use '/' as the path separator)",
        ));
    }
    Ok(target)
}

/// Destination rel_path for a `move`: the entry keeps its own `name`
/// and lands under `target_dir` (relative to `db_scripts/<key>/`). An
/// empty target moves the entry to the db_scripts root. Mirrors the TUI
/// mark/paste idiom so both front-ends relocate an entry identically —
/// into a folder, name preserved.
fn move_target_rel(target_dir: &str, name: &str) -> String {
    let target = target_dir.trim().trim_matches('/');
    if target.is_empty() {
        name.to_string()
    } else {
        format!("{target}/{name}")
    }
}

/// Append the default `.sql` extension when the script name carries no
/// extension of its own (`migrate.py`, `notes.md`, … pass through). The
/// check is on the whole name; a dotless nested path still gets `.sql`.
fn script_file_name(name: &str) -> String {
    if name.contains('.') {
        name.to_string()
    } else {
        format!("{name}.sql")
    }
}

/// Resolve the final segment a script-leaf rename writes to. Keeps the
/// original file extension when the user-supplied name carries none
/// (`report` on `audit.sql` → `report.sql`), so a rename never silently
/// drops the script's type. A name that brings its own extension
/// (`report.py`) is taken verbatim. Directories have no extension, so
/// their rename target is always the raw name.
fn rename_target_name(new_name: &str, current_rel: &str) -> String {
    if new_name.contains('.') {
        return new_name.to_string();
    }
    match Path::new(current_rel).extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{new_name}.{ext}"),
        None => new_name.to_string(),
    }
}

// ---------------------------------------------------------------------------
// The tree
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

/// One adapter instance's script branch: the store the nodes write
/// through, plus the prefixed node types they report.
///
/// Wrap it in an [`Arc`] and hand a clone to every node — that keeps a
/// single store per adapter (rather than rebuilding one per action) and
/// lets `ContentAdapter::script_store` return [`Self::store`].
pub struct DbScriptTree {
    store: SqlScriptStore,
    types: DbScriptNodeTypes,
}

impl DbScriptTree {
    /// `adapter_type` is the adapter's `ContentAdapter::adapter_type()`
    /// — it becomes the `<adapter>:` prefix of the three node types.
    pub fn new(store: SqlScriptStore, adapter_type: &str) -> Self {
        Self {
            store,
            types: DbScriptNodeTypes::for_adapter(adapter_type),
        }
    }

    pub fn store(&self) -> &SqlScriptStore {
        &self.store
    }

    pub fn types(&self) -> &DbScriptNodeTypes {
        &self.types
    }

    fn data_dir(&self) -> &Path {
        self.store.instance_data_dir()
    }

    /// The group node for one container, addressed `<key>/db_scripts`.
    pub fn group_node(tree: &Arc<Self>, key: &str) -> Box<dyn Node> {
        Box::new(DbScriptsGroupNode {
            tree: Arc::clone(tree),
            node_id: format!("{key}/{DB_SCRIPTS_GROUP_ID}"),
            key: key.to_string(),
        })
    }

    /// The single virtual `DB Scripts` folder shown under a container.
    /// Always one row, so the branch is visible before anything loaded.
    pub fn group_summary(&self, key: &str) -> ListResult {
        empty_list(vec![NodeSummary {
            id: format!("{key}/{DB_SCRIPTS_GROUP_ID}"),
            label: "DB Scripts".into(),
            node_type: self.types.group.clone(),
            metadata: Metadata { fields: vec![] },
            has_children: None,
        }])
    }

    /// One level of the tree below `db_scripts/<key>/<rel_path>/` (empty
    /// `rel_path` = the group root). `want_dirs` selects folders,
    /// `want_scripts` selects scripts — the two child kinds a view spec
    /// declares separately, each fetched on its own.
    pub async fn list_entries(
        &self,
        key: &str,
        rel_path: &str,
        want_dirs: bool,
        want_scripts: bool,
    ) -> Result<ListResult> {
        let entries = files::list_db_script_entries(self.data_dir(), key, Path::new(rel_path))
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))?;
        // Root (group node) composes `<key>/db_scripts/<name>`; a nested
        // folder composes `<key>/db_scripts/<rel_path>/<name>`.
        let compose = |name: &str| -> String {
            if rel_path.is_empty() {
                format!("{key}/{DB_SCRIPTS_GROUP_ID}/{name}")
            } else {
                format!("{key}/{DB_SCRIPTS_GROUP_ID}/{rel_path}/{name}")
            }
        };
        let mut items = Vec::new();
        for e in entries {
            let name = e.name().to_string();
            match e {
                files::DbScriptTreeEntry::Dir { .. } if want_dirs => {
                    items.push(NodeSummary {
                        id: compose(&name),
                        label: name.clone(),
                        node_type: self.types.dir.clone(),
                        metadata: dir_metadata(key, &name),
                        has_children: None,
                    });
                }
                files::DbScriptTreeEntry::Script { .. } if want_scripts => {
                    items.push(NodeSummary {
                        id: compose(&name),
                        label: name.clone(),
                        node_type: self.types.script.clone(),
                        metadata: script_metadata(key, &name),
                        has_children: None,
                    });
                }
                _ => {}
            }
        }
        Ok(empty_list(items))
    }

    /// Flat script list for one container — every script directly in
    /// `db_scripts/<key>/`, no folders. Lets a view spec offer the
    /// scripts as a direct child of the container, skipping the group
    /// node.
    pub async fn list_scripts_flat(&self, key: &str) -> Result<ListResult> {
        let scripts = files::list_db_scripts_in_database(self.data_dir(), key)
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))?;
        let items = scripts
            .into_iter()
            .map(|script| NodeSummary {
                id: format!("{key}/{DB_SCRIPTS_GROUP_ID}/{script}"),
                label: script.clone(),
                node_type: self.types.script.clone(),
                metadata: script_metadata(key, &script),
                has_children: None,
            })
            .collect();
        Ok(empty_list(items))
    }

    /// Action set for one of the three type ids, or `None` when the type
    /// isn't part of this branch — so an adapter's `actions_for_type`
    /// can delegate here first and fall through to its own types.
    pub fn actions_for_type(&self, type_id: &str) -> Option<Vec<NodeAction>> {
        if type_id == self.types.group.type_id {
            Some(group_actions())
        } else if type_id == self.types.dir.type_id {
            Some(dir_actions())
        } else if type_id == self.types.script.type_id {
            Some(script_actions())
        } else {
            None
        }
    }

    /// Resolve one child segment below `db_scripts/<key>/<parent_rel>`:
    /// a folder if the path is a directory on disk, a script if it is a
    /// file. Shared by the group node and the folder node so root-flat
    /// and nested resolution behave identically. The dir probe wins on
    /// collision (mkdir/touch would refuse anyway, but order matters
    /// for determinism).
    async fn resolve_child(
        tree: &Arc<Self>,
        key: &str,
        parent_rel: &str,
        id: &str,
    ) -> Result<Box<dyn Node>> {
        let child_rel = if parent_rel.is_empty() {
            PathBuf::from(id)
        } else {
            PathBuf::from(parent_rel).join(id)
        };
        let dir_abs = files::db_script_dir_path(tree.data_dir(), key, &child_rel);
        if tokio::fs::metadata(&dir_abs)
            .await
            .map(|m| m.is_dir())
            .unwrap_or(false)
        {
            let rel = child_rel.to_string_lossy().into_owned();
            return Ok(Box::new(DbScriptDirNode {
                tree: Arc::clone(tree),
                node_id: format!("{key}/{DB_SCRIPTS_GROUP_ID}/{rel}"),
                metadata: dir_metadata(key, id),
                key: key.to_string(),
                rel_path: rel,
                name: id.to_string(),
            }));
        }
        let file_abs = files::db_script_path(tree.data_dir(), key, &child_rel);
        if tokio::fs::metadata(&file_abs)
            .await
            .map(|m| m.is_file())
            .unwrap_or(false)
        {
            let rel = child_rel.to_string_lossy().into_owned();
            return Ok(Box::new(DbScriptNode {
                tree: Arc::clone(tree),
                node_id: format!("{key}/{DB_SCRIPTS_GROUP_ID}/{rel}"),
                metadata: script_metadata(key, id),
                key: key.to_string(),
                rel_path: rel,
                name: id.to_string(),
            }));
        }
        Err(ContentError::NotFound(format!("db script or folder {id}")))
    }
}

// ---------------------------------------------------------------------------
// Group node
// ---------------------------------------------------------------------------

/// The `DB Scripts` folder under one container. Distinct from
/// [`DbScriptDirNode`] (which never has an empty rel_path) so it can
/// carry its own, smaller action set: there is nothing to rename, move
/// or delete about the branch root.
struct DbScriptsGroupNode {
    tree: Arc<DbScriptTree>,
    key: String,
    /// Full composite id `<key>/db_scripts`.
    node_id: String,
}

#[async_trait]
impl Node for DbScriptsGroupNode {
    fn id(&self) -> &str {
        &self.node_id
    }

    fn label(&self) -> &str {
        "DB Scripts"
    }

    fn node_type(&self) -> &NodeType {
        &self.tree.types.group
    }

    fn metadata(&self) -> &Metadata {
        static EMPTY: Metadata = Metadata { fields: vec![] };
        &EMPTY
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        DbScriptTree::resolve_child(&self.tree, &self.key, "", id).await
    }

    async fn invoke_action(&self, name: &str, _ctx: &ActionContext) -> Result<ActionDispatch> {
        // `add-script` / `add-dir` collect a name via `InputSpec::Form` and
        // run through `execute` (below) — they never reach `invoke_action`.
        Err(ContentError::NotSupported(format!(
            "db_scripts group action '{name}' is not supported"
        )))
    }

    /// `add-script` / `add-dir` receive the new entry's name via the
    /// generic form popup (TUI) or `--field name=…` (CLI) and create it
    /// through the [`ScriptStore`], so both frontends share one write
    /// path. The new entry lives at the container root (this is the
    /// group node).
    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        let store = self.tree.store();
        match action_id {
            "add-script" => {
                let rel_path = script_file_name(&form_entry_name(&input)?);
                if store.create_db_script(&self.key, &rel_path).await? {
                    Ok(ActionOutcome::Done {
                        message: Some(format!("Created DB script '{rel_path}'")),
                    })
                } else {
                    Err(script_input_err(format!(
                        "DB script '{rel_path}' already exists"
                    )))
                }
            }
            "add-dir" => {
                let rel_path = form_entry_name(&input)?;
                store.create_db_dir(&self.key, &rel_path).await?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("Created DB-script folder '{rel_path}'")),
                })
            }
            other => Err(ContentError::NotSupported(format!(
                "db_scripts group action '{other}' is not supported"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Folder node
// ---------------------------------------------------------------------------

/// A folder inside the script tree. Holds its full rel_path from the
/// container root so `get_child` can compose absolute node ids
/// (`<key>/db_scripts/<rel_path>/<seg>`).
struct DbScriptDirNode {
    tree: Arc<DbScriptTree>,
    key: String,
    /// Full path relative to `db_scripts/<key>/`, joined with `/`. Never
    /// empty (root is [`DbScriptsGroupNode`]).
    rel_path: String,
    /// Last segment of `rel_path`; used by [`Node::label`] to keep the
    /// row label compact in tree views.
    name: String,
    /// Full composite id `<key>/db_scripts/<rel_path>` — the
    /// addressability invariant. `label()` still returns the leaf `name`.
    node_id: String,
    metadata: Metadata,
}

#[async_trait]
impl Node for DbScriptDirNode {
    fn id(&self) -> &str {
        &self.node_id
    }

    fn label(&self) -> &str {
        &self.name
    }

    fn node_type(&self) -> &NodeType {
        &self.tree.types.dir
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        DbScriptTree::resolve_child(&self.tree, &self.key, &self.rel_path, id).await
    }

    async fn invoke_action(&self, name: &str, _ctx: &ActionContext) -> Result<ActionDispatch> {
        match name {
            // `add-script` / `add-dir` / `rename` collect input via
            // `InputSpec::Form` and run through `execute` (below) — they
            // never reach here. The empty-check for `delete-dir` happens
            // inside the [`ScriptStore`], which surfaces the not-empty
            // error verbatim; returning DeleteSelf keeps the same shape as
            // the script-leaf delete, and the generic confirm path then
            // calls `execute("delete-dir")`.
            "delete-dir" => Ok(ActionDispatch::DeleteSelf { confirm: None }),
            // mark-move / paste-move stay pure TUI flows: the adapter has
            // no work to do until the user pastes.
            "mark-move" | "paste-move" => Ok(ActionDispatch::Noop),
            other => Err(ContentError::NotSupported(format!(
                "db_script_dir action '{other}' is not supported"
            ))),
        }
    }

    /// `add-script` / `add-dir` create a new entry under this folder from
    /// the form's `name` field; `delete-dir` removes the folder through
    /// the [`ScriptStore`] (a non-empty directory surfaces the store's
    /// "not empty (N entries)" error verbatim). Confirmation / name entry
    /// already happened frontend-side, so the TUI and the CLI share one
    /// code path.
    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        let store = self.tree.store();
        match action_id {
            "add-script" => {
                let file_name = script_file_name(&form_entry_name(&input)?);
                let rel_path = format!("{}/{file_name}", self.rel_path);
                if store.create_db_script(&self.key, &rel_path).await? {
                    Ok(ActionOutcome::Done {
                        message: Some(format!("Created DB script '{rel_path}'")),
                    })
                } else {
                    Err(script_input_err(format!(
                        "DB script '{rel_path}' already exists"
                    )))
                }
            }
            "add-dir" => {
                let name = form_entry_name(&input)?;
                let rel_path = format!("{}/{name}", self.rel_path);
                store.create_db_dir(&self.key, &rel_path).await?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("Created DB-script folder '{rel_path}'")),
                })
            }
            "rename" => {
                let new_name = form_entry_name(&input)?;
                if new_name == self.name {
                    return Ok(ActionOutcome::NoChanges);
                }
                store
                    .rename_db_entry(&self.key, &self.rel_path, &new_name)
                    .await?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("Renamed folder '{}' → '{new_name}'", self.name)),
                })
            }
            "move" => {
                let dst_rel = move_target_rel(&form_target_dir(&input)?, &self.name);
                if dst_rel == self.rel_path {
                    return Ok(ActionOutcome::NoChanges);
                }
                store
                    .move_db_entry(&self.key, &self.rel_path, &dst_rel)
                    .await?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("Moved folder '{}' → '{dst_rel}'", self.rel_path)),
                })
            }
            "delete-dir" => {
                store.delete_db_dir(&self.key, &self.rel_path).await?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("Deleted folder '{}'", self.rel_path)),
                })
            }
            other => Err(ContentError::NotSupported(format!(
                "db_script_dir action '{other}' is not supported"
            ))),
        }
    }

    /// Prefill the `rename` form with the folder's current name so the
    /// user edits it in place rather than re-typing from scratch.
    async fn form_prep(&self, action_id: &str) -> Result<HashMap<String, String>> {
        if action_id == "rename" {
            return Ok(HashMap::from([("name".to_string(), self.name.clone())]));
        }
        Ok(HashMap::new())
    }
}

// ---------------------------------------------------------------------------
// Script leaf
// ---------------------------------------------------------------------------

/// A single script. The body lives in the filesystem; every read and
/// write goes through [`crate::script_files`], so the CLI's `$EDITOR`
/// round trip and the TUI's in-place session touch the same file.
struct DbScriptNode {
    tree: Arc<DbScriptTree>,
    key: String,
    /// Full path relative to `db_scripts/<key>/`, joined with `/`. For a
    /// flat root script this equals `name`; nested it looks like
    /// `util/audit.sql`. The extension is part of the path — the storage
    /// layer invents none.
    rel_path: String,
    /// Last segment of `rel_path`. Returned by [`Node::label`] so the
    /// row label stays compact in tree views.
    name: String,
    /// Full composite id `<key>/db_scripts/<rel_path>` — the
    /// addressability invariant. `label()` still returns the leaf `name`.
    node_id: String,
    metadata: Metadata,
}

impl DbScriptNode {
    async fn read_body(&self) -> Result<String> {
        files::read_db_script(self.tree.data_dir(), &self.key, &self.rel_path)
            .await
            .map_err(|e| ContentError::Other(Box::new(e)))
    }
}

#[async_trait]
impl Node for DbScriptNode {
    fn id(&self) -> &str {
        &self.node_id
    }

    fn label(&self) -> &str {
        &self.name
    }

    fn node_type(&self) -> &NodeType {
        &self.tree.types.script
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Expose the script body so the CLI `cat` command can print it.
    /// [`Content::read`] reads the same on-disk file the editor edits.
    fn content(&self) -> Option<&dyn Content> {
        Some(self)
    }

    async fn invoke_action(&self, name: &str, _ctx: &ActionContext) -> Result<ActionDispatch> {
        match name {
            "execute" => {
                let body = self.read_body().await?;
                let sql = not_yet_done_content::script_buffer::parse_query_area(&body)
                    .trim()
                    .to_string();
                if sql.is_empty() {
                    return Ok(ActionDispatch::Error(format!(
                        "script '{}' has no SQL below the marker",
                        self.rel_path
                    )));
                }
                Ok(ActionDispatch::ExecuteQuery {
                    database: self.key.clone(),
                    sql,
                    paged: true,
                })
            }
            "edit" => Ok(ActionDispatch::OpenEditor {
                session_kind: "script_editor".into(),
                // `script` carries the FULL rel_path (may contain `/`);
                // the host's session resolves the on-disk file through
                // `ScriptStore::db_script_path`, which is a `PathBuf::join`
                // and so accepts slashes.
                params: HashMap::from([
                    ("database".into(), self.key.clone()),
                    ("script".into(), self.rel_path.clone()),
                ]),
            }),
            "delete" => Ok(ActionDispatch::DeleteSelf { confirm: None }),
            // `rename` collects a name via `InputSpec::Form` and runs
            // through `execute` (below). `mark-move` stays a TUI-owned flow.
            "mark-move" => Ok(ActionDispatch::Noop),
            other => Err(ContentError::NotSupported(format!(
                "db_script action '{other}' is not supported"
            ))),
        }
    }

    /// `edit` writes an editor buffer straight back to the file (the CLI
    /// save path — the TUI's rich session writes the same file on `:w`);
    /// `delete` unlinks it and `rename` / `move` relocate it through the
    /// [`ScriptStore`]. Confirmation (delete) and name entry (rename)
    /// already happened frontend-side, so both front-ends share one path.
    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        if action_id == "edit" {
            let ActionInput::Edited { text, .. } = input else {
                return Err(ContentError::NotSupported(
                    "edit requires an editor buffer".into(),
                ));
            };
            files::write_db_script(self.tree.data_dir(), &self.key, &self.rel_path, &text)
                .await
                .map_err(|e| ContentError::Other(Box::new(e)))?;
            return Ok(ActionOutcome::Done {
                message: Some(format!("Saved DB script '{}'", self.rel_path)),
            });
        }
        let store = self.tree.store();
        match action_id {
            "delete" => {
                store.delete_db_script(&self.key, &self.rel_path).await?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("Deleted DB script '{}'", self.rel_path)),
                })
            }
            "rename" => {
                let new_name = rename_target_name(&form_entry_name(&input)?, &self.rel_path);
                if new_name == self.name {
                    return Ok(ActionOutcome::NoChanges);
                }
                store
                    .rename_db_entry(&self.key, &self.rel_path, &new_name)
                    .await?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("Renamed DB script '{}' → '{new_name}'", self.name)),
                })
            }
            "move" => {
                let dst_rel = move_target_rel(&form_target_dir(&input)?, &self.name);
                if dst_rel == self.rel_path {
                    return Ok(ActionOutcome::NoChanges);
                }
                store
                    .move_db_entry(&self.key, &self.rel_path, &dst_rel)
                    .await?;
                Ok(ActionOutcome::Done {
                    message: Some(format!("Moved DB script '{}' → '{dst_rel}'", self.rel_path)),
                })
            }
            other => Err(ContentError::NotSupported(format!(
                "db_script action '{other}' is not supported"
            ))),
        }
    }

    /// Seed the CLI editor buffer for `edit` with the current body, so
    /// `do_editor` opens `$EDITOR` pre-filled and diffs against it. The
    /// TUI never calls this (its `e` opens the rich session via
    /// `invoke_action`); this exists purely for the generic CLI path.
    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        if action_id == "edit" {
            return Ok(EditorPrep {
                template: self.read_body().await?,
                version: String::new(),
                suffix: Node::node_type(self).file_extension.clone(),
                file_path: None,
            });
        }
        Err(ContentError::NotSupported(format!(
            "db_script has no editor for '{action_id}'"
        )))
    }

    /// Prefill the `rename` form with the script's current file name
    /// (extension included) so the user edits it in place. Dropping the
    /// extension on save re-applies the original one via
    /// [`rename_target_name`].
    async fn form_prep(&self, action_id: &str) -> Result<HashMap<String, String>> {
        if action_id == "rename" {
            return Ok(HashMap::from([("name".to_string(), self.name.clone())]));
        }
        Ok(HashMap::new())
    }
}

#[async_trait]
impl Content for DbScriptNode {
    fn node_type(&self) -> &NodeType {
        Node::node_type(self)
    }

    fn version(&self) -> Option<&str> {
        None
    }

    async fn read(&self) -> Result<Vec<u8>> {
        Ok(self.read_body().await?.into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_content::script_buffer::QUERY_MARKER;

    /// Minimal layout — this module only touches the container-level half
    /// of the store, so node-scoped addressing can stay unplaceable.
    struct NoNodeScripts;

    impl crate::script_store::NodeScriptLayout for NoNodeScripts {
        fn node_segments(&self, _node_id: &str) -> Option<Vec<String>> {
            None
        }

        fn default_node_script_body(&self, _node_id: &str) -> String {
            String::new()
        }
    }

    fn tree(dir: &Path) -> Arc<DbScriptTree> {
        Arc::new(DbScriptTree::new(
            SqlScriptStore::new(dir.to_path_buf(), Arc::new(NoNodeScripts)),
            "demo",
        ))
    }

    #[test]
    fn node_types_carry_the_adapter_prefix() {
        let types = DbScriptNodeTypes::for_adapter("sqlite");
        assert_eq!(types.group.type_id, "sqlite:db_scripts");
        assert_eq!(types.dir.type_id, "sqlite:db_script_dir");
        assert_eq!(types.script.type_id, "sqlite:db_script");
        // Scripts are the only editable leaf, so only they carry a syntax.
        assert_eq!(types.script.syntax.as_deref(), Some("sql"));
    }

    #[test]
    fn actions_are_offered_for_the_branch_types_only() {
        let t = tree(Path::new("/tmp/nyd/demo"));
        assert!(t.actions_for_type("demo:db_scripts").is_some());
        assert!(t.actions_for_type("demo:db_script_dir").is_some());
        assert!(t.actions_for_type("demo:db_script").is_some());
        assert!(t.actions_for_type("demo:table").is_none());
        // A sibling adapter's branch must not answer here either.
        assert!(t.actions_for_type("postgres:db_script").is_none());
    }

    #[test]
    fn group_summary_is_one_addressable_row() {
        let t = tree(Path::new("/tmp/nyd/demo"));
        let list = t.group_summary("notes");
        assert_eq!(list.items.len(), 1);
        assert_eq!(list.items[0].id, "notes/db_scripts");
        assert_eq!(list.items[0].node_type.type_id, "demo:db_scripts");
    }

    #[tokio::test]
    async fn group_creates_scripts_and_folders_at_the_root() {
        let dir = tempfile::tempdir().expect("temp dir");
        let t = tree(dir.path());
        let mut group = DbScriptTree::group_node(&t, "notes");
        group
            .execute(
                "add-script",
                ActionInput::Form(HashMap::from([("name".into(), "audit".into())])),
            )
            .await
            .expect("add-script");
        group
            .execute(
                "add-dir",
                ActionInput::Form(HashMap::from([("name".into(), "util".into())])),
            )
            .await
            .expect("add-dir");

        // A dotless name gains `.sql`; the template carries the marker.
        let body = tokio::fs::read_to_string(t.store().db_script_path("notes", "audit.sql"))
            .await
            .expect("script body");
        assert!(body.contains(QUERY_MARKER));

        let dirs = t.list_entries("notes", "", true, false).await.unwrap();
        assert_eq!(dirs.items.len(), 1);
        assert_eq!(dirs.items[0].id, "notes/db_scripts/util");
        let scripts = t.list_entries("notes", "", false, true).await.unwrap();
        assert_eq!(scripts.items.len(), 1);
        assert_eq!(scripts.items[0].id, "notes/db_scripts/audit.sql");
    }

    #[tokio::test]
    async fn adding_a_duplicate_script_is_an_error_not_an_overwrite() {
        let dir = tempfile::tempdir().expect("temp dir");
        let t = tree(dir.path());
        let mut group = DbScriptTree::group_node(&t, "notes");
        let form = || ActionInput::Form(HashMap::from([("name".into(), "audit".into())]));
        group.execute("add-script", form()).await.expect("first");
        assert!(group.execute("add-script", form()).await.is_err());
    }

    #[tokio::test]
    async fn get_child_tells_folders_from_scripts_and_nests() {
        let dir = tempfile::tempdir().expect("temp dir");
        let t = tree(dir.path());
        t.store().create_db_dir("notes", "util").await.unwrap();
        t.store()
            .create_db_script("notes", "util/deep.sql")
            .await
            .unwrap();

        let group = DbScriptTree::group_node(&t, "notes");
        let folder = group.get_child("util").await.expect("folder");
        assert_eq!(folder.node_type().type_id, "demo:db_script_dir");
        assert_eq!(folder.id(), "notes/db_scripts/util");

        let script = folder.get_child("deep.sql").await.expect("script");
        assert_eq!(script.node_type().type_id, "demo:db_script");
        assert_eq!(script.id(), "notes/db_scripts/util/deep.sql");
        assert_eq!(script.label(), "deep.sql");

        assert!(group.get_child("nope").await.is_err());
    }

    #[tokio::test]
    async fn execute_dispatches_the_sql_below_the_marker() {
        let dir = tempfile::tempdir().expect("temp dir");
        let t = tree(dir.path());
        t.store()
            .create_db_script("notes", "audit.sql")
            .await
            .unwrap();
        let path = t.store().db_script_path("notes", "audit.sql");
        let body = not_yet_done_content::script_buffer::default_buffer("SELECT 42;\n");
        tokio::fs::write(&path, &body).await.unwrap();

        let group = DbScriptTree::group_node(&t, "notes");
        let script = group.get_child("audit.sql").await.expect("script");
        let dispatch = script
            .invoke_action("execute", &ActionContext::default())
            .await
            .expect("execute");
        match dispatch {
            ActionDispatch::ExecuteQuery {
                database,
                sql,
                paged,
            } => {
                assert_eq!(database, "notes");
                assert_eq!(sql, "SELECT 42;");
                assert!(paged);
            }
            other => panic!("expected ExecuteQuery, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn execute_reports_an_empty_query_area_instead_of_running_nothing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let t = tree(dir.path());
        t.store()
            .create_db_script("notes", "audit.sql")
            .await
            .unwrap();
        let path = t.store().db_script_path("notes", "audit.sql");
        tokio::fs::write(
            &path,
            not_yet_done_content::script_buffer::default_buffer(""),
        )
        .await
        .unwrap();

        let group = DbScriptTree::group_node(&t, "notes");
        let script = group.get_child("audit.sql").await.expect("script");
        let dispatch = script
            .invoke_action("execute", &ActionContext::default())
            .await
            .expect("execute");
        assert!(matches!(dispatch, ActionDispatch::Error(msg) if msg.contains("marker")));
    }

    #[tokio::test]
    async fn edit_dispatches_the_full_rel_path_as_the_script_param() {
        let dir = tempfile::tempdir().expect("temp dir");
        let t = tree(dir.path());
        t.store().create_db_dir("notes", "util").await.unwrap();
        t.store()
            .create_db_script("notes", "util/deep.sql")
            .await
            .unwrap();

        let group = DbScriptTree::group_node(&t, "notes");
        let script = group
            .get_child("util")
            .await
            .unwrap()
            .get_child("deep.sql")
            .await
            .unwrap();
        match script
            .invoke_action("edit", &ActionContext::default())
            .await
            .expect("edit")
        {
            ActionDispatch::OpenEditor {
                session_kind,
                params,
            } => {
                assert_eq!(session_kind, "script_editor");
                assert_eq!(params.get("database").map(String::as_str), Some("notes"));
                assert_eq!(
                    params.get("script").map(String::as_str),
                    Some("util/deep.sql")
                );
            }
            other => panic!("expected OpenEditor, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn renaming_a_script_keeps_its_extension() {
        let dir = tempfile::tempdir().expect("temp dir");
        let t = tree(dir.path());
        t.store()
            .create_db_script("notes", "audit.sql")
            .await
            .unwrap();
        let group = DbScriptTree::group_node(&t, "notes");
        let mut script = group.get_child("audit.sql").await.expect("script");
        script
            .execute(
                "rename",
                ActionInput::Form(HashMap::from([("name".into(), "report".into())])),
            )
            .await
            .expect("rename");
        let scripts = t.list_entries("notes", "", false, true).await.unwrap();
        assert_eq!(scripts.items.len(), 1);
        assert_eq!(scripts.items[0].label, "report.sql");
    }

    #[tokio::test]
    async fn moving_a_script_keeps_its_name_and_lands_in_the_target_folder() {
        let dir = tempfile::tempdir().expect("temp dir");
        let t = tree(dir.path());
        t.store().create_db_dir("notes", "util").await.unwrap();
        t.store()
            .create_db_script("notes", "audit.sql")
            .await
            .unwrap();
        let group = DbScriptTree::group_node(&t, "notes");
        let mut script = group.get_child("audit.sql").await.expect("script");
        script
            .execute(
                "move",
                ActionInput::Form(HashMap::from([("target".into(), "util".into())])),
            )
            .await
            .expect("move");
        let moved = t.list_entries("notes", "util", false, true).await.unwrap();
        assert_eq!(moved.items.len(), 1);
        assert_eq!(moved.items[0].id, "notes/db_scripts/util/audit.sql");
    }

    #[tokio::test]
    async fn a_windows_style_move_target_is_rejected_before_any_io() {
        let dir = tempfile::tempdir().expect("temp dir");
        let t = tree(dir.path());
        t.store()
            .create_db_script("notes", "audit.sql")
            .await
            .unwrap();
        let group = DbScriptTree::group_node(&t, "notes");
        let mut script = group.get_child("audit.sql").await.expect("script");
        assert!(
            script
                .execute(
                    "move",
                    ActionInput::Form(HashMap::from([("target".into(), "util\\deep".into())])),
                )
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_name_with_a_separator_is_rejected() {
        let dir = tempfile::tempdir().expect("temp dir");
        let t = tree(dir.path());
        let mut group = DbScriptTree::group_node(&t, "notes");
        for bad in ["a/b", "a\\b", ".hidden"] {
            assert!(
                group
                    .execute(
                        "add-script",
                        ActionInput::Form(HashMap::from([("name".into(), bad.into())])),
                    )
                    .await
                    .is_err(),
                "should reject: {bad}"
            );
        }
    }

    #[tokio::test]
    async fn a_non_empty_folder_refuses_to_be_deleted() {
        let dir = tempfile::tempdir().expect("temp dir");
        let t = tree(dir.path());
        t.store().create_db_dir("notes", "util").await.unwrap();
        t.store()
            .create_db_script("notes", "util/deep.sql")
            .await
            .unwrap();
        let group = DbScriptTree::group_node(&t, "notes");
        let mut folder = group.get_child("util").await.expect("folder");
        // `ActionOutcome` has no `Debug`, so no `expect_err` here.
        match folder.execute("delete-dir", ActionInput::None).await {
            Ok(_) => panic!("a non-empty folder must not be deleted"),
            Err(e) => assert!(e.to_string().contains("not empty"), "{e}"),
        }
    }

    #[tokio::test]
    async fn flat_script_list_skips_folders() {
        let dir = tempfile::tempdir().expect("temp dir");
        let t = tree(dir.path());
        t.store().create_db_dir("notes", "util").await.unwrap();
        t.store()
            .create_db_script("notes", "audit.sql")
            .await
            .unwrap();
        t.store()
            .create_db_script("notes", "util/deep.sql")
            .await
            .unwrap();
        let flat = t.list_scripts_flat("notes").await.unwrap();
        assert_eq!(flat.items.len(), 1);
        assert_eq!(flat.items[0].id, "notes/db_scripts/audit.sql");
    }

    #[tokio::test]
    async fn the_script_body_is_readable_as_content() {
        let dir = tempfile::tempdir().expect("temp dir");
        let t = tree(dir.path());
        t.store()
            .create_db_script("notes", "audit.sql")
            .await
            .unwrap();
        let path = t.store().db_script_path("notes", "audit.sql");
        tokio::fs::write(&path, "SELECT 7;\n").await.unwrap();
        let group = DbScriptTree::group_node(&t, "notes");
        let script = group.get_child("audit.sql").await.expect("script");
        let content = script.content().expect("content");
        assert_eq!(content.read().await.unwrap(), b"SELECT 7;\n");
    }

    #[tokio::test]
    async fn rename_forms_start_from_the_current_name() {
        let dir = tempfile::tempdir().expect("temp dir");
        let t = tree(dir.path());
        t.store().create_db_dir("notes", "util").await.unwrap();
        t.store()
            .create_db_script("notes", "audit.sql")
            .await
            .unwrap();
        let group = DbScriptTree::group_node(&t, "notes");
        let folder = group.get_child("util").await.unwrap();
        assert_eq!(
            folder.form_prep("rename").await.unwrap().get("name"),
            Some(&"util".to_string())
        );
        let script = group.get_child("audit.sql").await.unwrap();
        assert_eq!(
            script.form_prep("rename").await.unwrap().get("name"),
            Some(&"audit.sql".to_string())
        );
    }
}
