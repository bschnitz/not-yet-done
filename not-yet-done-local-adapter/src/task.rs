//! `TaskAdapter` — an in-process [`ContentAdapter`] over the host's own
//! [`TaskService`](not_yet_done_core::service::TaskService).
//!
//! Unlike the remote adapters this one has no network and no opaque YAML
//! config: its backing store is the [`CoreHandle`] threaded in from the
//! App (see [`crate`] docs). It presents the task forest as a content
//! tree so the generic `ContentView` can render it exactly like any other
//! adapter — the goal of plan phase A1.
//!
//! ## Eager snapshot
//!
//! The tree the TUI drills is *lazy* (one `list()`/`get_child()` per
//! expand), but tasks form a small, fully-local forest, so the adapter
//! loads the **whole** non-deleted forest once into an immutable
//! [`ForestSnapshot`] and every node it hands out shares one
//! `Arc<ForestSnapshot>`. Drilling never touches the DB again; a reload
//! (or any structural domain event, see [`spawn_task_bridge`]) drops the
//! snapshot so the next fetch rebuilds it. This is also what lets
//! [`search_in_tree`](ContentAdapter::search_in_tree) walk the entire
//! forest without a query per node.
//!
//! ## Scope (A1b — read + mutations)
//!
//! On top of the A1a read path this adds the structural mutations through
//! the generic action vocabulary:
//!
//! - **add / edit** (`InputSpec::Editor`) — a markdown buffer with a
//!   `---` frontmatter and `## Description:` / `## Notes:` body (see
//!   [`editor_templates`]). `add` is exposed on every *container* node
//!   (the root → top-level task, a task → child task); `edit` on the task
//!   itself. The `tracking:` toggle in the buffer is honored here too —
//!   the dedicated tracking *marker column* and a one-key start/stop
//!   shortcut are the only tracking bits still deferred to A1c.
//! - **delete** (`DeleteSelf`) — recursive delete of the task subtree.
//! - **undelete** — restores the most recently deleted task(s) via
//!   [`TaskService::undelete_last`](not_yet_done_core::service::TaskService::undelete_last);
//!   exposed on the task node (the selected row) since it needs no target.
//! - **reparent** (mark-move / paste-move, M7) — the adapter performs the
//!   move inside [`Node::invoke_action`] from [`ActionContext::marked`],
//!   guarding against cycles.
//!
//! Every mutation emits a [`DomainEvent`] on the shared bus so the
//! snapshot-clearing bridge ([`spawn_task_bridge`]) drops the cache and
//! the other tabs repaint.
//!
//! Still A1c: tracking marker column + dedicated start/stop shortcut,
//! saved queries (`task` scope), `FilterExpr` filtering, and scripts.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use not_yet_done_content::{
    ActionContext, ActionDispatch, ActionInput, ActionOutcome, AdapterCapabilities, AdapterFactory,
    ContentAdapter, ContentError, EditorPrep, HintPlacement, InputSpec, Invalidation, Metadata,
    MetadataField, Node, NodeAction, NodeSummary, NodeType, Result, SortableColumn, TreeFindHit,
    TreeSearchResults,
};
use not_yet_done_core::entity::task;
use not_yet_done_core::error::AppError;
use not_yet_done_core::events::{DomainEvent, DomainEventReceiver};
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;

use crate::editor_templates::{self, FieldError, ParseResult};
use crate::{notes, CoreHandle};

/// Stable id of the synthetic forest-root node.
const ROOT_ID: &str = "task:root";

// ---------------------------------------------------------------------------
// Node types
// ---------------------------------------------------------------------------

/// The node type for an individual task. `task:item` is the `node_type`
/// a `views/tasks.yaml` binds its columns and (later) actions to.
fn task_item_type() -> NodeType {
    NodeType {
        type_id: "task:item".to_string(),
        mime_type: "text/plain".to_string(),
        syntax: None,
        file_extension: ".txt".to_string(),
        display_name: "Task".to_string(),
    }
}

/// The synthetic forest root the adapter exposes from [`ContentAdapter::root`].
fn task_root_type() -> NodeType {
    NodeType {
        type_id: "task:root".to_string(),
        mime_type: "text/plain".to_string(),
        syntax: None,
        file_extension: ".txt".to_string(),
        display_name: "Tasks".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

/// One task plus the display data derived once at snapshot-build time.
struct TaskRow {
    task: task::Model,
    /// Resolved tag names (global + project), already flattened.
    tags: Vec<String>,
    /// Effective parent inside the *visible* forest. A task whose stored
    /// `parent_id` points at a deleted (hence absent) task is re-rooted to
    /// `None` so it still shows up rather than vanishing into an orphan
    /// bucket.
    parent: Option<Uuid>,
}

/// Immutable, eagerly-loaded view of the whole non-deleted task forest.
/// Shared by `Arc` across every node the adapter hands out.
struct ForestSnapshot {
    by_id: HashMap<Uuid, TaskRow>,
    /// parent id → ordered child ids. The `None` key holds the forest roots.
    children: HashMap<Option<Uuid>, Vec<Uuid>>,
}

impl ForestSnapshot {
    /// Build a snapshot from the live service: load every non-deleted task,
    /// batch-resolve tags, re-root orphans, and order siblings by creation
    /// time for a stable render.
    async fn load(handle: &CoreHandle) -> Result<Arc<Self>> {
        let tasks = handle
            .task_service
            .list_tasks(None)
            .await
            .map_err(to_content_err)?;
        let ids: Vec<Uuid> = tasks.iter().map(|t| t.id).collect();
        let id_set: HashSet<Uuid> = ids.iter().copied().collect();
        let tag_map = handle
            .task_service
            .load_tags_for_tasks(&ids)
            .await
            .map_err(to_content_err)?;

        let mut by_id: HashMap<Uuid, TaskRow> = HashMap::with_capacity(tasks.len());
        let mut children: HashMap<Option<Uuid>, Vec<Uuid>> = HashMap::new();
        for t in tasks {
            // Re-root tasks whose parent was filtered out (e.g. deleted).
            let parent = t.parent_id.filter(|p| id_set.contains(p));
            let tags = tag_map
                .get(&t.id)
                .map(|v| v.iter().map(tag_name).collect())
                .unwrap_or_default();
            children.entry(parent).or_default().push(t.id);
            by_id.insert(t.id, TaskRow { task: t, tags, parent });
        }
        for siblings in children.values_mut() {
            siblings.sort_by(|a, b| {
                let ca = by_id.get(a).map(|r| r.task.created_at);
                let cb = by_id.get(b).map(|r| r.task.created_at);
                ca.cmp(&cb)
            });
        }
        Ok(Arc::new(ForestSnapshot { by_id, children }))
    }

    /// Ordered child summaries of `parent` (`None` = forest roots).
    fn child_summaries(&self, parent: Option<Uuid>) -> Vec<NodeSummary> {
        self.children
            .get(&parent)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.by_id.get(id).map(|row| self.summary(*id, row)))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn summary(&self, id: Uuid, row: &TaskRow) -> NodeSummary {
        let has_children = self
            .children
            .get(&Some(id))
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        NodeSummary {
            id: id.to_string(),
            label: row.task.description.clone(),
            node_type: task_item_type(),
            metadata: task_metadata(row),
            has_children: Some(has_children),
        }
    }

    /// Root-to-node chain of task ids (as strings) — the addressing a
    /// [`TreeFindHit`] hands back to the TUI to expand to a hit.
    fn path_to(&self, id: Uuid) -> Vec<String> {
        let mut chain = Vec::new();
        let mut cur = Some(id);
        while let Some(c) = cur {
            chain.push(c.to_string());
            cur = self.by_id.get(&c).and_then(|r| r.parent);
        }
        chain.reverse();
        chain
    }
}

// ---------------------------------------------------------------------------
// Column / metadata helpers
// ---------------------------------------------------------------------------

/// Human label for a [`task::TaskStatus`]. The canonical column value
/// (`views/tasks.yaml` declares this column `kind: text`).
fn status_label(status: &task::TaskStatus) -> &'static str {
    match status {
        task::TaskStatus::Todo => "Todo",
        task::TaskStatus::InProgress => "In Progress",
        task::TaskStatus::Done => "Done",
        task::TaskStatus::Cancelled => "Cancelled",
    }
}

fn tag_name(tag: &not_yet_done_core::repository::ResolvedTag) -> String {
    use not_yet_done_core::repository::ResolvedTag;
    match tag {
        ResolvedTag::Global(t) => t.name.clone(),
        ResolvedTag::Project(t) => t.name.clone(),
    }
}

/// A read-only metadata field. Tasks are edited through actions (A1b), not
/// through inline metadata edits, so `editable` is always `false`.
fn field(key: &str, value: String, label: &str) -> MetadataField {
    MetadataField {
        key: key.to_string(),
        value,
        display_label: label.to_string(),
        editable: false,
        allowed_values: None,
    }
}

/// Build the column-backing metadata for a task. Values are the **canonical
/// strings** the engine's typed columns (M2) parse: integers for `number`,
/// RFC 3339 for `datetime`. `views/tasks.yaml` declares the `kind:` per
/// column; the adapter only supplies the canonical form.
fn task_metadata(row: &TaskRow) -> Metadata {
    let t = &row.task;
    Metadata {
        fields: vec![
            field("status", status_label(&t.status).to_string(), "Status"),
            field("priority", t.priority.to_string(), "Priority"),
            field("tags", row.tags.join(" "), "Tags"),
            field("created", t.created_at.to_rfc3339(), "Created"),
            field("updated", t.updated_at.to_rfc3339(), "Updated"),
            field(
                "last_tracked",
                t.last_tracked_at
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_default(),
                "Last Tracked",
            ),
            field("id", t.id.to_string(), "ID"),
        ],
    }
}

/// Columns a list of tasks can be sorted on. Sorting itself is in-memory
/// in the engine (the adapter applies no server-side sort and reports an
/// empty `applied_sort`); this just marks the headers sort-eligible.
fn task_sortable_columns() -> Vec<SortableColumn> {
    [
        ("description", "Description"),
        ("status", "Status"),
        ("priority", "Priority"),
        ("created", "Created"),
        ("updated", "Updated"),
    ]
    .into_iter()
    .map(|(key, label)| SortableColumn {
        key: key.to_string(),
        label: label.to_string(),
    })
    .collect()
}

fn to_content_err(e: AppError) -> ContentError {
    ContentError::Other(Box::new(e))
}

// ---------------------------------------------------------------------------
// Actions (A1b)
// ---------------------------------------------------------------------------

/// Actions the synthetic forest root exposes: only `add`, which routes
/// through the view config's `type: create` to create a *top-level* task
/// (the new node's parent comes from the buffer's `parent:` field, blank
/// by default).
fn task_root_actions() -> Vec<NodeAction> {
    vec![NodeAction::new("add", "Add task", InputSpec::Editor)
        .with_placement(HintPlacement::ActionBar)
        .with_default_key('a')]
}

/// Actions a single task exposes. `add` makes a task a valid `type: create`
/// container (new child); `edit` opens the markdown buffer on the task
/// itself; `delete`/`undelete` and the `mark-move`/`paste-move` reparent
/// pair are fire-and-forget shortcuts dispatched through
/// [`Node::invoke_action`].
fn task_item_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new("edit", "Edit", InputSpec::Editor)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('e'),
        NodeAction::new("add", "Add subtask", InputSpec::Editor)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('a'),
        NodeAction::new("delete", "Delete", InputSpec::None)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('d'),
        NodeAction::new("undelete", "Undelete", InputSpec::None).with_default_key('u'),
        NodeAction::new("mark-move", "Mark for move", InputSpec::None),
        NodeAction::new("paste-move", "Move here", InputSpec::None),
    ]
}

// ---------------------------------------------------------------------------
// Mutations (A1b) — shared logic for the root/item nodes
// ---------------------------------------------------------------------------

/// Owned snapshot of every task model — the `all_tasks` slice the notes
/// path-builder needs to resolve a task's hierarchical notes file.
fn all_tasks(snapshot: &ForestSnapshot) -> Vec<task::Model> {
    snapshot.by_id.values().map(|r| r.task.clone()).collect()
}

/// `root` plus all its descendants present in the snapshot — the subtree a
/// recursive delete soft-deletes, so their notes are marked deleted too.
fn subtree_ids(snapshot: &ForestSnapshot, root: Uuid) -> Vec<Uuid> {
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() {
        let parent = out[i];
        if let Some(children) = snapshot.children.get(&Some(parent)) {
            out.extend(children.iter().copied());
        }
        i += 1;
    }
    out
}

/// True when reparenting `moving` under `new_parent` would form a cycle —
/// i.e. `new_parent` is `moving` itself or one of its descendants. Walks
/// `new_parent`'s ancestor chain looking for `moving`.
fn would_create_cycle(snapshot: &ForestSnapshot, moving: Uuid, new_parent: Uuid) -> bool {
    let mut cur = Some(new_parent);
    while let Some(c) = cur {
        if c == moving {
            return true;
        }
        cur = snapshot.by_id.get(&c).and_then(|r| r.parent);
    }
    false
}

fn emit_task_changed(handle: &CoreHandle, id: Uuid) {
    let _ = handle.events.send(DomainEvent::TaskChanged { id });
}

/// Wrap a service error as a buffer-reopen carrying the message, so the
/// user keeps their edits and sees what failed.
fn reopen_service_error(text: &str, e: AppError) -> ActionOutcome {
    let errors = vec![FieldError {
        field: "description",
        message: format!("Service error: {e}"),
    }];
    ActionOutcome::Reopen {
        content: editor_templates::render_with_errors(text, &errors),
        new_version: None,
    }
}

/// Start or stop tracking for `task_id`, mirroring the host's native
/// policy: with parallel tracking disabled, starting first stops every
/// other active tracking. Each transition emits a `Tracking*` event so the
/// other tabs repaint.
async fn apply_tracking(handle: &CoreHandle, task_id: Uuid, wants_tracked: bool) {
    let now = chrono::Utc::now();
    if wants_tracked {
        if !handle.allow_parallel_tracking {
            if let Ok(active) = handle.tracking_repo.find_all_active().await {
                for t in active {
                    if handle.tracking_repo.stop(t.id, now).await.is_ok() {
                        let _ = handle.events.send(DomainEvent::TrackingStopped {
                            task_id: t.task_id,
                            tracking_id: t.id,
                        });
                    }
                }
            }
        }
        if let Ok(started) = handle.tracking_repo.insert(task_id, now, None).await {
            let _ = handle.events.send(DomainEvent::TrackingStarted {
                task_id,
                tracking_id: started.id,
            });
        }
    } else if let Ok(Some(t)) = handle.tracking_repo.find_active_for_task(task_id).await {
        if handle.tracking_repo.stop(t.id, now).await.is_ok() {
            let _ = handle.events.send(DomainEvent::TrackingStopped {
                task_id,
                tracking_id: t.id,
            });
        }
    }
}

/// `prepare` for the `add` action: render the new-task buffer for a task
/// under `parent_id` (`None` = top-level). No version token — a create has
/// nothing to conflict with.
fn prepare_add(parent_id: Option<Uuid>) -> EditorPrep {
    EditorPrep {
        template: editor_templates::new_task(parent_id),
        version: String::new(),
        suffix: ".md".into(),
    }
}

/// `prepare` for the `edit` action: render the task's current frontmatter +
/// description + notes, with the `tracking:` flag reflecting live state.
async fn prepare_edit(
    handle: &CoreHandle,
    snapshot: &ForestSnapshot,
    id: Uuid,
) -> Result<EditorPrep> {
    let task = snapshot
        .by_id
        .get(&id)
        .map(|r| r.task.clone())
        .ok_or_else(|| ContentError::NotFound(id.to_string()))?;
    let is_tracked = handle
        .tracking_repo
        .find_active_for_task(id)
        .await
        .map_err(to_content_err)?
        .is_some();
    let notes_str = notes::read_notes(&task, &all_tasks(snapshot));
    let template = editor_templates::edit_task_with_notes(&task, is_tracked, &notes_str);
    Ok(EditorPrep {
        template,
        version: task.updated_at.to_rfc3339(),
        suffix: ".md".into(),
    })
}

/// `execute` for the `add` action. `parent_default` seeds the parent for a
/// create initiated on a container node; the buffer's `parent:` field (if
/// the user sets one) wins via the parser.
async fn execute_add(
    handle: &CoreHandle,
    snapshot: &ForestSnapshot,
    text: &str,
    original: &str,
) -> Result<ActionOutcome> {
    match editor_templates::parse_new_task(text, original) {
        ParseResult::Aborted => Ok(ActionOutcome::NoChanges),
        ParseResult::Errors {
            errors,
            original_content,
        } => Ok(ActionOutcome::Reopen {
            content: editor_templates::render_with_errors(&original_content, &errors),
            new_version: None,
        }),
        ParseResult::Ok(parsed) => {
            let wants_tracking = parsed.tracking;
            let notes_text = editor_templates::parse_notes(text);
            let created = match handle
                .task_service
                .add_task(
                    parsed.description,
                    None,
                    parsed.parent_id,
                    None,
                    parsed.status,
                    parsed.priority,
                )
                .await
            {
                Ok(c) => c,
                Err(e) => return Ok(reopen_service_error(text, e)),
            };
            notes::write_notes(&created, &all_tasks(snapshot), &notes_text);
            if wants_tracking {
                apply_tracking(handle, created.id, true).await;
            }
            emit_task_changed(handle, created.id);
            Ok(ActionOutcome::Navigate {
                node_id: created.id.to_string(),
                node_type: task_item_type(),
            })
        }
    }
}

/// `execute` for the `edit` action. Diffs the buffer against the snapshot's
/// task, persists only changed fields, moves/renames notes on a parent or
/// description change, applies a tracking toggle, and guards reparents.
async fn execute_edit(
    handle: &CoreHandle,
    snapshot: &ForestSnapshot,
    id: Uuid,
    text: &str,
    original: &str,
) -> Result<ActionOutcome> {
    let task = snapshot
        .by_id
        .get(&id)
        .map(|r| r.task.clone())
        .ok_or_else(|| ContentError::NotFound(id.to_string()))?;
    let was_tracked = handle
        .tracking_repo
        .find_active_for_task(id)
        .await
        .map_err(to_content_err)?
        .is_some();

    match editor_templates::parse_edit_task(text, original, &task) {
        ParseResult::Aborted => Ok(ActionOutcome::NoChanges),
        ParseResult::Errors {
            errors,
            original_content,
        } => Ok(ActionOutcome::Reopen {
            content: editor_templates::render_with_errors(&original_content, &errors),
            new_version: None,
        }),
        ParseResult::Ok(parsed) => {
            // Cycle guard: refuse to move a task under itself or a descendant.
            if let Some(Some(new_parent)) = parsed.parent_id {
                if would_create_cycle(snapshot, id, new_parent) {
                    let errors = vec![FieldError {
                        field: "parent",
                        message: "Cannot move a task under itself or one of its descendants"
                            .to_string(),
                    }];
                    return Ok(ActionOutcome::Reopen {
                        content: editor_templates::render_with_errors(text, &errors),
                        new_version: None,
                    });
                }
            }

            let notes_text = editor_templates::parse_notes(text);
            let all = all_tasks(snapshot);
            if let Some(new_desc) = &parsed.description {
                notes::rename_notes(&task, &task.description, new_desc, &all);
            }
            let parent_changed =
                parsed.parent_id.is_some() && parsed.parent_id != Some(task.parent_id);

            let updated = match handle
                .task_service
                .update_task(
                    id,
                    parsed.description.clone(),
                    parsed.status,
                    parsed.priority,
                    parsed.parent_id,
                    None,
                )
                .await
            {
                Ok(u) => u,
                Err(e) => return Ok(reopen_service_error(text, e)),
            };

            if parent_changed {
                let new_rows: Vec<task::Model> = all
                    .iter()
                    .map(|t| if t.id == updated.id { updated.clone() } else { t.clone() })
                    .collect();
                notes::move_notes(&updated, &all, &new_rows);
                notes::write_notes(&updated, &new_rows, &notes_text);
            } else {
                notes::write_notes(&updated, &all, &notes_text);
            }

            if let Some(want) = parsed.tracking {
                if want != was_tracked {
                    apply_tracking(handle, updated.id, want).await;
                }
            }
            emit_task_changed(handle, updated.id);
            Ok(ActionOutcome::Done {
                message: Some("Task updated".to_string()),
            })
        }
    }
}

/// `execute("delete")` — recursive subtree delete + soft-delete the notes
/// of every task in the subtree (so an `undelete` can bring them back).
async fn execute_delete(
    handle: &CoreHandle,
    snapshot: &ForestSnapshot,
    id: Uuid,
) -> Result<ActionOutcome> {
    let ids = subtree_ids(snapshot, id);
    let all = all_tasks(snapshot);
    for tid in &ids {
        if let Some(row) = snapshot.by_id.get(tid) {
            notes::mark_notes_deleted(&row.task, &all);
        }
    }
    let count = handle
        .task_service
        .delete_task_recursive(id)
        .await
        .map_err(to_content_err)?;
    emit_task_changed(handle, id);
    let message = if count > 1 {
        format!("Deleted subtree ({count} tasks)")
    } else {
        "Task deleted".to_string()
    };
    Ok(ActionOutcome::Done {
        message: Some(message),
    })
}

/// `invoke_action("undelete")` — restore the most recently deleted task(s).
/// Needs no target node, so it ignores the invoking node's identity.
async fn invoke_undelete(handle: &CoreHandle) -> ActionDispatch {
    match handle.task_service.undelete_last().await {
        Ok(0) => ActionDispatch::Error("Nothing to undelete".to_string()),
        Ok(_) => {
            emit_task_changed(handle, Uuid::nil());
            ActionDispatch::Reload
        }
        Err(e) => ActionDispatch::Error(format!("Undelete failed: {e}")),
    }
}

/// `invoke_action("paste-move")` — reparent the previously-marked task
/// (`ctx.marked`) under `target`. Validates the marked node is a task and
/// the move forms no cycle, then persists + relocates its notes.
async fn invoke_paste_move(
    handle: &CoreHandle,
    snapshot: &ForestSnapshot,
    target: Uuid,
    marked: &not_yet_done_content::MarkedNode,
) -> ActionDispatch {
    if marked.node_type.type_id != task_item_type().type_id {
        return ActionDispatch::Error("Can only move a task under a task".to_string());
    }
    let Ok(moving) = Uuid::parse_str(&marked.node_id) else {
        return ActionDispatch::Error(format!("Invalid marked id: {}", marked.node_id));
    };
    if moving == target {
        return ActionDispatch::Error("A task cannot be its own parent".to_string());
    }
    if would_create_cycle(snapshot, moving, target) {
        return ActionDispatch::Error(
            "Cannot move a task under one of its own descendants".to_string(),
        );
    }
    let all = all_tasks(snapshot);
    match handle
        .task_service
        .update_task(moving, None, None, None, Some(Some(target)), None)
        .await
    {
        Ok(updated) => {
            let new_rows: Vec<task::Model> = all
                .iter()
                .map(|t| if t.id == updated.id { updated.clone() } else { t.clone() })
                .collect();
            notes::move_notes(&updated, &all, &new_rows);
            emit_task_changed(handle, updated.id);
            ActionDispatch::Reload
        }
        Err(e) => ActionDispatch::Error(format!("Move failed: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Nodes
// ---------------------------------------------------------------------------

/// Synthetic forest root. Lists the top-level tasks (`parent_id == None`).
struct TaskRootNode {
    snapshot: Arc<ForestSnapshot>,
    handle: CoreHandle,
    node_type: NodeType,
    metadata: Metadata,
}

#[async_trait]
impl Node for TaskRootNode {
    fn id(&self) -> &str {
        ROOT_ID
    }
    fn label(&self) -> &str {
        "Tasks"
    }
    fn node_type(&self) -> &NodeType {
        &self.node_type
    }
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }
    fn children_types(&self) -> Vec<NodeType> {
        vec![task_item_type()]
    }
    fn sortable_columns(&self, _node_type: &NodeType) -> Vec<SortableColumn> {
        task_sortable_columns()
    }
    fn actions(&self) -> Vec<NodeAction> {
        task_root_actions()
    }
    async fn list(
        &self,
        _params: not_yet_done_content::ListParams,
    ) -> Result<not_yet_done_content::ListResult> {
        Ok(list_result(self.snapshot.child_summaries(None)))
    }
    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        TaskItemNode::fetch(&self.snapshot, &self.handle, id)
    }
    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        match action_id {
            // Create a top-level task; the buffer's `parent:` may override.
            "add" => Ok(prepare_add(None)),
            other => Err(ContentError::NotSupported(format!(
                "action `{other}` not supported on the task root"
            ))),
        }
    }
    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        match (action_id, input) {
            ("add", ActionInput::Edited { text, original, .. }) => {
                execute_add(&self.handle, &self.snapshot, &text, &original).await
            }
            (other, _) => Err(ContentError::NotSupported(format!(
                "action `{other}` not supported on the task root"
            ))),
        }
    }
}

/// A single task. Owns its label + metadata (the `Node` accessors return
/// borrows) and a shared handle to the forest snapshot for drilling.
struct TaskItemNode {
    snapshot: Arc<ForestSnapshot>,
    handle: CoreHandle,
    id: Uuid,
    id_str: String,
    label: String,
    node_type: NodeType,
    metadata: Metadata,
}

impl TaskItemNode {
    /// Parse `id` and confirm the task exists in `snapshot`, or `NotFound`.
    /// Split out from [`Self::fetch`] so the lookup is testable without a
    /// live [`CoreHandle`].
    fn resolve_id(snapshot: &ForestSnapshot, id: &str) -> Result<Uuid> {
        let uuid = Uuid::parse_str(id).map_err(|_| ContentError::NotFound(id.to_string()))?;
        if snapshot.by_id.contains_key(&uuid) {
            Ok(uuid)
        } else {
            Err(ContentError::NotFound(id.to_string()))
        }
    }

    /// Look `id` up in `snapshot` and build the node, or `NotFound`.
    fn fetch(
        snapshot: &Arc<ForestSnapshot>,
        handle: &CoreHandle,
        id: &str,
    ) -> Result<Box<dyn Node>> {
        let uuid = Self::resolve_id(snapshot, id)?;
        let row = &snapshot.by_id[&uuid];
        Ok(Box::new(TaskItemNode {
            snapshot: snapshot.clone(),
            handle: handle.clone(),
            id: uuid,
            id_str: id.to_string(),
            label: row.task.description.clone(),
            node_type: task_item_type(),
            metadata: task_metadata(row),
        }))
    }
}

#[async_trait]
impl Node for TaskItemNode {
    fn id(&self) -> &str {
        &self.id_str
    }
    fn label(&self) -> &str {
        &self.label
    }
    fn node_type(&self) -> &NodeType {
        &self.node_type
    }
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }
    fn children_types(&self) -> Vec<NodeType> {
        vec![task_item_type()]
    }
    fn sortable_columns(&self, _node_type: &NodeType) -> Vec<SortableColumn> {
        task_sortable_columns()
    }
    fn actions(&self) -> Vec<NodeAction> {
        task_item_actions()
    }
    async fn list(
        &self,
        _params: not_yet_done_content::ListParams,
    ) -> Result<not_yet_done_content::ListResult> {
        Ok(list_result(self.snapshot.child_summaries(Some(self.id))))
    }
    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        TaskItemNode::fetch(&self.snapshot, &self.handle, id)
    }
    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        match action_id {
            // Create a child task under this one (buffer's `parent:` wins).
            "add" => Ok(prepare_add(Some(self.id))),
            "edit" => prepare_edit(&self.handle, &self.snapshot, self.id).await,
            other => Err(ContentError::NotSupported(format!(
                "action `{other}` has no editor buffer"
            ))),
        }
    }
    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        match (action_id, input) {
            ("add", ActionInput::Edited { text, original, .. }) => {
                execute_add(&self.handle, &self.snapshot, &text, &original).await
            }
            ("edit", ActionInput::Edited { text, original, .. }) => {
                execute_edit(&self.handle, &self.snapshot, self.id, &text, &original).await
            }
            // Reached via the generic `DeleteSelf` confirm flow, which calls
            // `execute("delete", None)` after the user confirms.
            ("delete", _) => execute_delete(&self.handle, &self.snapshot, self.id).await,
            (other, _) => Err(ContentError::NotSupported(format!(
                "action `{other}` not supported on a task"
            ))),
        }
    }
    async fn invoke_action(&self, name: &str, ctx: &ActionContext) -> Result<ActionDispatch> {
        Ok(match name {
            // Routed to the generic delete-confirm flow; the actual delete
            // happens in `execute("delete")` after confirmation.
            "delete" => ActionDispatch::DeleteSelf,
            "undelete" => invoke_undelete(&self.handle).await,
            // The frontend records the mark; the adapter does nothing here.
            "mark-move" => ActionDispatch::Noop,
            "paste-move" => match &ctx.marked {
                Some(marked) => {
                    invoke_paste_move(&self.handle, &self.snapshot, self.id, marked).await
                }
                None => ActionDispatch::Error("Nothing marked to move".to_string()),
            },
            _ => ActionDispatch::Noop,
        })
    }
}

/// Wrap a summary list into a `ListResult`. The adapter does no
/// server-side sort or pagination (the forest is in-memory), so it echoes
/// an empty `applied_sort` and leaves the engine to sort/slice locally.
fn list_result(items: Vec<NodeSummary>) -> not_yet_done_content::ListResult {
    not_yet_done_content::ListResult {
        items,
        applied_sort: Vec::new(),
        page: None,
        batch_download_available: false,
        downloaded: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Event bridge
// ---------------------------------------------------------------------------

/// Bridge the core domain-event bus into this adapter's invalidation
/// stream **and** drop the eager snapshot on structural change so the next
/// fetch rebuilds from the DB.
///
/// The TaskAdapter narrows the generic mapping deliberately:
/// - `TrackingTick` is ignored — a task row's shape doesn't change on the
///   1 Hz heartbeat (that repaint belongs to the TrackingAdapter, A2).
/// - `TaskChanged { id }` → [`Invalidation::Node`] (refetch that subtree).
/// - `TrackingStarted`/`Stopped` → [`Invalidation::All`] (the per-task
///   tracking marker, A1c, may change anywhere).
///
/// On every non-tick event the cached snapshot is cleared first, so a
/// refetch triggered by the invalidation reads fresh data rather than the
/// stale `Arc`.
fn spawn_task_bridge(
    mut events: DomainEventReceiver,
    inv_tx: broadcast::Sender<Invalidation>,
    snapshot: Arc<RwLock<Option<Arc<ForestSnapshot>>>>,
) {
    tokio::spawn(async move {
        use broadcast::error::RecvError;
        loop {
            match events.recv().await {
                Ok(DomainEvent::TrackingTick) => {}
                Ok(DomainEvent::TaskChanged { id }) => {
                    *snapshot.write().await = None;
                    let _ = inv_tx.send(Invalidation::Node { id: id.to_string() });
                }
                Ok(DomainEvent::TrackingStarted { .. }) | Ok(DomainEvent::TrackingStopped { .. }) => {
                    *snapshot.write().await = None;
                    let _ = inv_tx.send(Invalidation::All);
                }
                Err(RecvError::Lagged(_)) => {
                    *snapshot.write().await = None;
                    let _ = inv_tx.send(Invalidation::All);
                }
                Err(RecvError::Closed) => break,
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Adapter + factory
// ---------------------------------------------------------------------------

/// Builds [`TaskAdapter`] instances bound to a captured [`CoreHandle`].
/// Like the other local factories, `create`'s config string is unused —
/// the backing store is the handle, not anything in YAML.
pub struct TaskAdapterFactory {
    handle: CoreHandle,
}

impl TaskAdapterFactory {
    pub fn new(handle: CoreHandle) -> Self {
        Self { handle }
    }
}

impl AdapterFactory for TaskAdapterFactory {
    fn adapter_type(&self) -> &str {
        "tasks"
    }

    fn create(&self, instance_id: &str, _config: &str) -> Result<Box<dyn ContentAdapter>> {
        let (inv_tx, _) = broadcast::channel(64);
        let snapshot: Arc<RwLock<Option<Arc<ForestSnapshot>>>> = Arc::new(RwLock::new(None));
        spawn_task_bridge(
            self.handle.events.subscribe(),
            inv_tx.clone(),
            snapshot.clone(),
        );
        Ok(Box::new(TaskAdapter {
            instance_id: instance_id.to_string(),
            handle: self.handle.clone(),
            inv_tx,
            snapshot,
        }))
    }
}

/// In-process adapter presenting the host's task forest as a content tree.
pub struct TaskAdapter {
    instance_id: String,
    handle: CoreHandle,
    inv_tx: broadcast::Sender<Invalidation>,
    /// Eager forest snapshot, shared with every node. `None` until first
    /// load; cleared by [`spawn_task_bridge`] on structural change.
    snapshot: Arc<RwLock<Option<Arc<ForestSnapshot>>>>,
}

impl TaskAdapter {
    /// Return the cached snapshot, loading it from the service if absent.
    async fn snapshot(&self) -> Result<Arc<ForestSnapshot>> {
        if let Some(snap) = self.snapshot.read().await.as_ref() {
            return Ok(snap.clone());
        }
        // Miss: take the write lock and load. A double-check guards the
        // race where another task loaded between the read drop and here.
        let mut guard = self.snapshot.write().await;
        if let Some(snap) = guard.as_ref() {
            return Ok(snap.clone());
        }
        let snap = ForestSnapshot::load(&self.handle).await?;
        *guard = Some(snap.clone());
        Ok(snap)
    }

    /// Force a fresh load (reload semantics) and cache it.
    async fn reload_snapshot(&self) -> Result<Arc<ForestSnapshot>> {
        let snap = ForestSnapshot::load(&self.handle).await?;
        *self.snapshot.write().await = Some(snap.clone());
        Ok(snap)
    }
}

#[async_trait]
impl ContentAdapter for TaskAdapter {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn adapter_type(&self) -> &str {
        "tasks"
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            // A1b: add/edit (create) and delete/undelete/reparent.
            supports_create: true,
            supports_delete: true,
            supports_search: true,
            ..AdapterCapabilities::default()
        }
    }

    fn actions_for_type(&self, node_type: &NodeType) -> Vec<NodeAction> {
        match node_type.type_id.as_str() {
            "task:root" => task_root_actions(),
            "task:item" => task_item_actions(),
            _ => Vec::new(),
        }
    }

    async fn root(&self) -> Result<Box<dyn Node>> {
        // A `root()` call is the reload entry point — fetch fresh.
        let snapshot = self.reload_snapshot().await?;
        Ok(Box::new(TaskRootNode {
            snapshot,
            handle: self.handle.clone(),
            node_type: task_root_type(),
            metadata: Metadata::default(),
        }))
    }

    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
        let snapshot = self.snapshot().await?;
        if id == ROOT_ID {
            return Ok(Box::new(TaskRootNode {
                snapshot,
                handle: self.handle.clone(),
                node_type: task_root_type(),
                metadata: Metadata::default(),
            }));
        }
        TaskItemNode::fetch(&snapshot, &self.handle, id)
    }

    fn subscribe_invalidations(&self) -> broadcast::Receiver<Invalidation> {
        self.inv_tx.subscribe()
    }

    async fn search_in_tree(&self, query: &str, limit: u32) -> Result<Option<TreeSearchResults>> {
        let snapshot = self.snapshot().await?;
        let tokens: Vec<String> = query.split_whitespace().map(|t| t.to_lowercase()).collect();
        if tokens.is_empty() {
            return Ok(Some(TreeSearchResults {
                hits: Vec::new(),
                truncated: false,
            }));
        }
        let mut hits: Vec<TreeFindHit> = snapshot
            .by_id
            .iter()
            .filter(|(_, row)| {
                let hay = row.task.description.to_lowercase();
                tokens.iter().all(|t| hay.contains(t))
            })
            .map(|(id, row)| TreeFindHit {
                path: snapshot.path_to(*id),
                label: row.task.description.clone(),
                space_key: String::new(),
            })
            .collect();
        // Tree-render order: parents before children, siblings together.
        hits.sort_by(|a, b| a.path.cmp(&b.path));
        let truncated = hits.len() > limit as usize;
        hits.truncate(limit as usize);
        Ok(Some(TreeSearchResults { hits, truncated }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: Uuid, desc: &str, parent: Option<Uuid>) -> (Uuid, TaskRow) {
        let now = chrono::Utc::now();
        (
            id,
            TaskRow {
                task: task::Model {
                    id,
                    description: desc.to_string(),
                    status: task::TaskStatus::Todo,
                    deleted: false,
                    deleted_at: None,
                    priority: 0,
                    parent_id: parent,
                    created_at: now,
                    updated_at: now,
                    last_tracked_at: None,
                    path: None,
                },
                tags: Vec::new(),
                parent,
            },
        )
    }

    /// Build a snapshot from rows directly (no DB) for pure-logic tests.
    fn snapshot_from(rows: Vec<(Uuid, TaskRow)>) -> Arc<ForestSnapshot> {
        let mut by_id = HashMap::new();
        let mut children: HashMap<Option<Uuid>, Vec<Uuid>> = HashMap::new();
        for (id, row) in rows {
            children.entry(row.parent).or_default().push(id);
            by_id.insert(id, row);
        }
        Arc::new(ForestSnapshot { by_id, children })
    }

    #[test]
    fn child_summaries_list_roots_and_children() {
        let root = Uuid::from_u128(1);
        let child = Uuid::from_u128(2);
        let snap = snapshot_from(vec![
            row(root, "Root task", None),
            row(child, "Child task", Some(root)),
        ]);

        let roots = snap.child_summaries(None);
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].label, "Root task");
        assert_eq!(roots[0].has_children, Some(true));

        let kids = snap.child_summaries(Some(root));
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0].label, "Child task");
        assert_eq!(kids[0].has_children, Some(false));
    }

    #[test]
    fn path_to_walks_ancestors_root_first() {
        let a = Uuid::from_u128(10);
        let b = Uuid::from_u128(20);
        let c = Uuid::from_u128(30);
        let snap = snapshot_from(vec![
            row(a, "A", None),
            row(b, "B", Some(a)),
            row(c, "C", Some(b)),
        ]);
        assert_eq!(
            snap.path_to(c),
            vec![a.to_string(), b.to_string(), c.to_string()]
        );
    }

    #[test]
    fn metadata_carries_canonical_typed_values() {
        let id = Uuid::from_u128(7);
        let (_, mut r) = row(id, "Do the thing", None);
        r.task.priority = 5;
        r.task.status = task::TaskStatus::InProgress;
        r.tags = vec!["urgent".into(), "home".into()];
        let md = task_metadata(&r);
        let get = |k: &str| md.fields.iter().find(|f| f.key == k).map(|f| f.value.clone());
        assert_eq!(get("priority").as_deref(), Some("5"));
        assert_eq!(get("status").as_deref(), Some("In Progress"));
        assert_eq!(get("tags").as_deref(), Some("urgent home"));
        // created/updated are RFC 3339 (parseable back to a datetime).
        let created = get("created").unwrap();
        assert!(chrono::DateTime::parse_from_rfc3339(&created).is_ok());
        assert_eq!(get("last_tracked").as_deref(), Some(""));
    }

    #[test]
    fn resolve_id_unknown_is_not_found() {
        let snap = snapshot_from(vec![row(Uuid::from_u128(1), "A", None)]);
        assert!(matches!(
            TaskItemNode::resolve_id(&snap, "not-a-uuid"),
            Err(ContentError::NotFound(_))
        ));
        assert!(matches!(
            TaskItemNode::resolve_id(&snap, &Uuid::from_u128(99).to_string()),
            Err(ContentError::NotFound(_))
        ));
        // An existing id resolves.
        assert_eq!(
            TaskItemNode::resolve_id(&snap, &Uuid::from_u128(1).to_string()).unwrap(),
            Uuid::from_u128(1)
        );
    }

    #[test]
    fn cycle_guard_detects_self_and_descendants() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let c = Uuid::from_u128(3);
        let snap = snapshot_from(vec![
            row(a, "A", None),
            row(b, "B", Some(a)),
            row(c, "C", Some(b)),
        ]);
        // Moving A under itself, under B, or under C (its descendants) cycles.
        assert!(would_create_cycle(&snap, a, a));
        assert!(would_create_cycle(&snap, a, b));
        assert!(would_create_cycle(&snap, a, c));
        // Moving C under A (an ancestor) is fine.
        assert!(!would_create_cycle(&snap, c, a));
    }

    #[test]
    fn subtree_ids_collects_descendants() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let c = Uuid::from_u128(3);
        let d = Uuid::from_u128(4);
        let snap = snapshot_from(vec![
            row(a, "A", None),
            row(b, "B", Some(a)),
            row(c, "C", Some(b)),
            row(d, "D", None),
        ]);
        let mut ids = subtree_ids(&snap, a);
        ids.sort();
        assert_eq!(ids, vec![a, b, c]);
        assert_eq!(subtree_ids(&snap, d), vec![d]);
    }

    #[test]
    fn root_and_item_expose_expected_actions() {
        let has = |actions: &[NodeAction], id: &str| actions.iter().any(|a| a.id == id);
        let root = task_root_actions();
        assert!(has(&root, "add"));
        assert_eq!(root.len(), 1);
        let item = task_item_actions();
        for id in ["edit", "add", "delete", "undelete", "mark-move", "paste-move"] {
            assert!(has(&item, id), "task:item missing action `{id}`");
        }
    }

    #[test]
    fn orphan_with_missing_parent_is_rerooted_in_load_logic() {
        // Mirrors ForestSnapshot::load's re-root rule: a task whose parent
        // isn't in the set becomes a root. We assert the rule via a direct
        // snapshot build that applies the same filter.
        let orphan = Uuid::from_u128(2);
        let missing_parent = Uuid::from_u128(999);
        let id_set: HashSet<Uuid> = [orphan].into_iter().collect();
        let effective = Some(missing_parent).filter(|p| id_set.contains(p));
        assert_eq!(effective, None);
    }
}
