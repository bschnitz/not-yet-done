//! `TrackingAdapter` — an in-process [`ContentAdapter`] over the host's own
//! [`TrackingRepository`](not_yet_done_core::repository::TrackingRepository).
//!
//! Sibling to [`crate::task`]: where the `TaskAdapter` presents the task
//! *forest*, this one presents the flat list of time **trackings** (one row
//! per started/stopped interval) so the generic `ContentView` renders it
//! like any other adapter — plan phase A2.
//!
//! ## Eager snapshot
//!
//! Like the task adapter, the whole non-deleted tracking list loads once
//! into an immutable [`TrackingSnapshot`] shared by `Arc`. Each row carries
//! the display data derived at build time: the task's description and its
//! **task path** (the parent chain, resolved once from a single
//! `list_tasks` call rather than per row), plus whether the tracking is
//! still running. A structural domain event (see [`spawn_tracking_bridge`])
//! drops the snapshot so the next fetch rebuilds it.
//!
//! ## Live durations (M9)
//!
//! A running tracking's duration ticks every second. Rather than a
//! render-time `kind: elapsed` column (which can't also show the *static*
//! `ended − started` of a completed tracking in the same column), the
//! adapter drives the generic **live-row** mechanism: while ≥1 tracking is
//! active it asks the frontend to pull [`live_rows`](ContentAdapter::live_rows)
//! at 1 Hz (via [`Invalidation::RefreshInterval`]); each pull recomputes
//! `now − started` for the active rows and the frontend patches them in
//! place ([`Invalidation::Row`]). When the last tracking stops the adapter
//! sends `RefreshInterval(None)` and the pull stops.
//!
//! ## Scope (A2a — read path)
//!
//! Flat list + typed columns (taskpath `kind: path`, started/ended
//! `kind: datetime`, duration `kind: duration`), saved-query `FilterExpr`
//! filtering (via [`TrackingRepository::find_filtered`]), and live
//! durations. Grouping (Day/Week/Month/Year) + per-group totals are pure
//! engine features driven from `views/trackings.yaml` (`group_by` /
//! `aggregates`).
//!
//! ## Scope (A2b — mutations)
//!
//! Per-row `delete` (soft-delete keeping times, via the generic `DeleteSelf`
//! confirm flow), `restore`/`restore-all` (undelete + purge the successors a
//! split produced), and `toggle-tracking` (flips tracking on the row's task,
//! reusing [`crate::task::apply_tracking`] so the host's exclusivity policy
//! is identical across both tabs). Each mutation emits a
//! [`DomainEvent::TrackingChanged`] so the bridge rebuilds the snapshot and
//! refreshes both this list and the task tracking marker. Scripts run via the
//! generic `:script` menu. The Condensed/Tree sub-views land in A2c.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use not_yet_done_content::{
    ActionContext, ActionDispatch, ActionInput, ActionOutcome, AdapterCapabilities, AdapterFactory,
    ContentAdapter, ContentError, FsSavedQueryStore, HintPlacement, InputSpec, Invalidation,
    Metadata, MetadataField, Node, NodeAction, NodeSummary, NodeType, Result, SavedQueryStore,
    SortableColumn,
};
use not_yet_done_core::entity::tracking;
use not_yet_done_core::error::AppError;
use not_yet_done_core::events::{DomainEvent, DomainEventReceiver};
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;

use crate::CoreHandle;

/// Stable id of the synthetic list-root node.
const ROOT_ID: &str = "tracking:root";

/// The 1 Hz cadence the adapter requests while a tracking is running, so a
/// running duration ticks once a second (see the module-level M9 note).
const LIVE_INTERVAL: Duration = Duration::from_secs(1);

// ---------------------------------------------------------------------------
// Node types
// ---------------------------------------------------------------------------

/// The node type for a single tracking row. `tracking:entry` is what a
/// `views/trackings.yaml` binds its columns to.
fn tracking_entry_type() -> NodeType {
    NodeType {
        type_id: "tracking:entry".to_string(),
        mime_type: "text/plain".to_string(),
        syntax: None,
        file_extension: ".txt".to_string(),
        display_name: "Tracking".to_string(),
    }
}

/// The synthetic list root the adapter exposes from [`ContentAdapter::root`].
fn tracking_root_type() -> NodeType {
    NodeType {
        type_id: "tracking:root".to_string(),
        mime_type: "text/plain".to_string(),
        syntax: None,
        file_extension: ".txt".to_string(),
        display_name: "Trackings".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

/// One tracking plus the display data derived once at snapshot-build time.
struct TrackingRow {
    tracking: tracking::Model,
    /// The tracked task's description (the row's label / `task` column).
    task_description: String,
    /// The task's ancestor chain, root-first, **excluding** the task
    /// itself — rendered as the `kind: path` taskpath column. Resolved once
    /// from the task forest at build time.
    task_path: Vec<String>,
    /// `true` while the tracking is still running (`ended_at IS NULL`).
    /// Seeds the `⏱` marker and the set of rows [`live_rows`] ticks.
    active: bool,
}

/// Immutable, eagerly-loaded view of the whole non-deleted tracking list.
/// Shared by `Arc` across every node the adapter hands out.
struct TrackingSnapshot {
    by_id: HashMap<Uuid, TrackingRow>,
    /// Display order — newest first, as [`TrackingRepository::find_all`]
    /// returns them.
    order: Vec<Uuid>,
}

impl TrackingSnapshot {
    /// Build a snapshot from the live services: load every non-deleted
    /// tracking, resolve each task's description + ancestor path from a
    /// single `list_tasks` pass, and record running state.
    async fn load(handle: &CoreHandle) -> Result<Arc<Self>> {
        let tasks = handle
            .task_service
            .list_tasks(None)
            .await
            .map_err(to_content_err)?;
        // task id → (description, parent) for path resolution.
        let task_map: HashMap<Uuid, (String, Option<Uuid>)> = tasks
            .into_iter()
            .map(|t| (t.id, (t.description, t.parent_id)))
            .collect();

        let trackings = handle
            .tracking_repo
            .find_all()
            .await
            .map_err(to_content_err)?;

        let mut by_id = HashMap::with_capacity(trackings.len());
        let mut order = Vec::with_capacity(trackings.len());
        for t in trackings {
            let task_description = task_map
                .get(&t.task_id)
                .map(|(desc, _)| desc.clone())
                .unwrap_or_else(|| "(unknown task)".to_string());
            let task_path = path_for(&task_map, t.task_id);
            let active = t.ended_at.is_none();
            order.push(t.id);
            by_id.insert(
                t.id,
                TrackingRow {
                    tracking: t,
                    task_description,
                    task_path,
                    active,
                },
            );
        }
        Ok(Arc::new(TrackingSnapshot { by_id, order }))
    }

    /// Number of running trackings — drives the live-refresh cadence.
    fn active_count(&self) -> usize {
        self.by_id.values().filter(|r| r.active).count()
    }

    /// Ordered entry summaries (newest first). When `filter` is `Some`,
    /// only trackings in the visible set are returned (a saved-query filter
    /// is active); `None` lists all.
    fn entries(
        &self,
        filter: Option<&HashSet<Uuid>>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Vec<NodeSummary> {
        self.order
            .iter()
            .filter(|id| filter.map_or(true, |f| f.contains(id)))
            .filter_map(|id| self.by_id.get(id).map(|row| entry_summary(*id, row, now)))
            .collect()
    }
}

/// Walk `task_id`'s ancestor chain, root-first, excluding the task itself.
/// Cycle-guarded (a corrupt parent loop just stops). Mirrors the native
/// trackings tab's path builder.
fn path_for(task_map: &HashMap<Uuid, (String, Option<Uuid>)>, task_id: Uuid) -> Vec<String> {
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    seen.insert(task_id);
    let mut current = task_map.get(&task_id).and_then(|(_, p)| *p);
    while let Some(id) = current {
        if !seen.insert(id) {
            break; // cycle guard
        }
        match task_map.get(&id) {
            Some((desc, parent)) => {
                chain.push(desc.clone());
                current = *parent;
            }
            None => break,
        }
    }
    chain.reverse(); // root first
    chain
}

// ---------------------------------------------------------------------------
// Column / metadata helpers
// ---------------------------------------------------------------------------

fn to_content_err(e: AppError) -> ContentError {
    ContentError::Other(Box::new(e))
}

/// A read-only metadata field. Trackings carry no inline-editable fields.
fn field(key: &str, value: String, label: &str) -> MetadataField {
    MetadataField {
        key: key.to_string(),
        value,
        display_label: label.to_string(),
        editable: false,
        allowed_values: None,
    }
}

/// Canonical `kind: path` value for the task path: `/a/b/c` (leading slash,
/// `/`-separated). Empty when the task is top-level (no ancestors).
fn canonical_path(segments: &[String]) -> String {
    if segments.is_empty() {
        String::new()
    } else {
        format!("/{}", segments.join("/"))
    }
}

/// A tracking's duration in **seconds** (the canonical `kind: duration`
/// input). Completed: `ended − started`; running: `now − started`. Clamped
/// to zero so a clock skew never renders a negative duration.
fn duration_seconds(row: &TrackingRow, now: chrono::DateTime<chrono::Utc>) -> i64 {
    let end = row.tracking.ended_at.unwrap_or(now);
    (end - row.tracking.started_at).num_seconds().max(0)
}

/// Build the column-backing metadata for a tracking. Values are the
/// **canonical strings** the engine's typed columns parse: integer seconds
/// for `duration`, RFC 3339 for `datetime`, `/`-segments for `path`.
fn entry_metadata(row: &TrackingRow, now: chrono::DateTime<chrono::Utc>) -> Metadata {
    Metadata {
        fields: vec![
            // Marker: a running-stopwatch glyph while the tracking is open.
            field(
                "marker",
                if row.active { "⏱".to_string() } else { String::new() },
                "Active",
            ),
            field("taskpath", canonical_path(&row.task_path), "Task Path"),
            field("task", row.task_description.clone(), "Task"),
            field("started", row.tracking.started_at.to_rfc3339(), "Started"),
            field(
                "ended",
                row.tracking
                    .ended_at
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_default(),
                "Ended",
            ),
            field("duration", duration_seconds(row, now).to_string(), "Duration"),
            field("id", row.tracking.id.to_string(), "ID"),
            // Stable per-task key for the Condensed view's inner grouping
            // level (`then_by: [{ column: task_id }]`). Never shown as a
            // column — distinct tasks with the same description must not
            // coalesce, so the inner group keys on the id, not the label.
            field("task_id", row.tracking.task_id.to_string(), "Task ID"),
        ],
    }
}

/// Build a tracking row's [`NodeSummary`]. Entries are leaves
/// (`has_children: Some(false)`) — the list is flat.
fn entry_summary(
    id: Uuid,
    row: &TrackingRow,
    now: chrono::DateTime<chrono::Utc>,
) -> NodeSummary {
    NodeSummary {
        id: id.to_string(),
        label: row.task_description.clone(),
        node_type: tracking_entry_type(),
        metadata: entry_metadata(row, now),
        has_children: Some(false),
    }
}

/// Columns a list of trackings can be sorted on (engine sorts in memory).
fn tracking_sortable_columns() -> Vec<SortableColumn> {
    [
        ("task", "Task"),
        ("started", "Started"),
        ("ended", "Ended"),
        ("duration", "Duration"),
    ]
    .into_iter()
    .map(|(key, label)| SortableColumn {
        key: key.to_string(),
        label: label.to_string(),
    })
    .collect()
}

/// Resolve the pane's active saved query into the set of *visible* tracking
/// ids via [`TrackingRepository::find_filtered`] (the repo understands both
/// tracking columns and the joined `task.description`). Returns `None` when
/// there is no query — the whole list is visible. A body that fails to
/// parse surfaces as a load error rather than silently showing everything.
async fn resolve_visible_set(
    handle: &CoreHandle,
    query: &Option<String>,
) -> Result<Option<HashSet<Uuid>>> {
    let raw = match query.as_deref().map(str::trim) {
        Some(q) if !q.is_empty() => q,
        _ => return Ok(None),
    };
    let parsed = not_yet_done_core::filter::query_filter::parse(raw)
        .map_err(|e| ContentError::Other(Box::new(e)))?;
    let matches = handle
        .tracking_repo
        .find_filtered(&parsed.expr)
        .await
        .map_err(to_content_err)?;
    Ok(Some(matches.into_iter().map(|t| t.id).collect()))
}

// ---------------------------------------------------------------------------
// Actions + mutations (A2b)
// ---------------------------------------------------------------------------

/// Actions the synthetic list root exposes. `restore-all` is a list-wide
/// operation (no target row), so it lives on the root rather than an entry.
fn tracking_root_actions() -> Vec<NodeAction> {
    vec![NodeAction::new("restore-all", "Restore all deleted", InputSpec::None)]
}

/// Actions a single tracking row exposes. All are fire-and-forget shortcuts
/// dispatched through [`Node::invoke_action`] (`delete` routes through the
/// generic delete-confirm flow). The `fuzzy filter` / `run script` / `reload`
/// affordances are generic frontend actions declared in `views/trackings.yaml`,
/// not adapter actions, so they are not listed here.
fn tracking_entry_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new("delete", "Delete", InputSpec::None)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('d'),
        NodeAction::new("restore", "Restore", InputSpec::None).with_default_key('R'),
        NodeAction::new("toggle-tracking", "Start/Stop tracking", InputSpec::None)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('t'),
    ]
}

/// Announce a non-transition tracking change (delete/restore) so the bridge
/// drops the snapshot and every view (this tab + the task tracking marker)
/// refetches. See [`DomainEvent::TrackingChanged`].
fn emit_tracking_changed(handle: &CoreHandle, tracking_id: Uuid) {
    let _ = handle
        .events
        .send(DomainEvent::TrackingChanged { tracking_id });
}

/// `execute("delete")` — soft-delete a tracking, keeping its start/end times
/// (mirrors the native tab's `soft_delete_keeping_times`). Reached via the
/// generic `DeleteSelf` confirm flow. The deleted row drops out of the list
/// (`find_all` excludes deleted); a [`DomainEvent::TrackingChanged`] then
/// rebuilds the snapshot and updates the task marker if the row was running.
async fn execute_delete(handle: &CoreHandle, tracking_id: Uuid) -> Result<ActionOutcome> {
    handle
        .tracking_repo
        .soft_delete_keeping_times(tracking_id)
        .await
        .map_err(to_content_err)?;
    emit_tracking_changed(handle, tracking_id);
    Ok(ActionOutcome::Done {
        message: Some("Tracking deleted".to_string()),
    })
}

/// Hard-delete every successor of `tracking_id` (the predecessor chain that a
/// split/edit produced), so a restore doesn't resurrect a row alongside the
/// successors that replaced it. BFS, deepest-first. Mirrors the native tab.
async fn purge_successors(handle: &CoreHandle, tracking_id: Uuid) -> std::result::Result<(), AppError> {
    let mut queue = vec![tracking_id];
    let mut to_delete = Vec::new();
    while let Some(id) = queue.pop() {
        for s in handle.tracking_repo.find_by_predecessor(id).await? {
            queue.push(s.id);
            to_delete.push(s.id);
        }
    }
    for id in to_delete.into_iter().rev() {
        handle.tracking_repo.hard_delete(id).await?;
    }
    Ok(())
}

/// `invoke_action("restore")` — undelete a previously soft-deleted tracking
/// (and purge the successors that replaced it). Errors if the target is not
/// deleted. Note: because the list shows only non-deleted rows, a *visible*
/// row is never deletable here — restore is reachable from a future
/// show-deleted sub-view; today it mirrors the native tab's behaviour.
async fn invoke_restore(handle: &CoreHandle, tracking_id: Uuid) -> ActionDispatch {
    let restore = async {
        let tracking = handle
            .tracking_repo
            .find_by_id(tracking_id)
            .await?
            .ok_or(AppError::TrackingNotFound(tracking_id))?;
        if !tracking.deleted {
            return Err(AppError::TrackingNotDeleted(tracking_id));
        }
        purge_successors(handle, tracking_id).await?;
        handle.tracking_repo.undelete(tracking_id).await?;
        Ok::<_, AppError>(())
    };
    match restore.await {
        Ok(()) => {
            emit_tracking_changed(handle, tracking_id);
            ActionDispatch::Reload
        }
        Err(e) => ActionDispatch::Error(format!("Restore failed: {e}")),
    }
}

/// `invoke_action("restore-all")` on the root — best-effort restore of every
/// deleted tracking among the candidate ids (non-deleted ones are skipped).
/// Mirrors the native tab, which restores over the currently-loaded rows.
async fn invoke_restore_all(handle: &CoreHandle, candidates: &[Uuid]) -> ActionDispatch {
    let run = async {
        let mut restored = 0u32;
        for &id in candidates {
            let Some(tracking) = handle.tracking_repo.find_by_id(id).await? else {
                continue;
            };
            if !tracking.deleted {
                continue;
            }
            purge_successors(handle, id).await?;
            handle.tracking_repo.undelete(id).await?;
            restored += 1;
        }
        Ok::<_, AppError>(restored)
    };
    match run.await {
        Ok(0) => ActionDispatch::Error("No deleted trackings to restore".to_string()),
        Ok(_) => {
            emit_tracking_changed(handle, Uuid::nil());
            ActionDispatch::Reload
        }
        Err(e) => ActionDispatch::Error(format!("Restore-all failed: {e}")),
    }
}

/// `invoke_action("toggle-tracking")` — flip time tracking for the row's
/// task. Reuses the task adapter's [`apply_tracking`](crate::task::apply_tracking)
/// so the host's exclusivity policy and `Tracking*` events stay identical
/// across both tabs. State is read live, never from the (possibly stale)
/// snapshot marker.
async fn invoke_toggle_tracking(handle: &CoreHandle, task_id: Uuid) -> ActionDispatch {
    let is_tracked = matches!(
        handle.tracking_repo.find_active_for_task(task_id).await,
        Ok(Some(_))
    );
    crate::task::apply_tracking(handle, task_id, !is_tracked).await;
    ActionDispatch::Reload
}

// ---------------------------------------------------------------------------
// Nodes
// ---------------------------------------------------------------------------

/// Synthetic list root. Lists every (optionally filtered) tracking.
struct TrackingRootNode {
    snapshot: Arc<TrackingSnapshot>,
    handle: CoreHandle,
    node_type: NodeType,
    metadata: Metadata,
}

#[async_trait]
impl Node for TrackingRootNode {
    fn id(&self) -> &str {
        ROOT_ID
    }
    fn label(&self) -> &str {
        "Trackings"
    }
    fn node_type(&self) -> &NodeType {
        &self.node_type
    }
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }
    fn children_types(&self) -> Vec<NodeType> {
        vec![tracking_entry_type()]
    }
    fn sortable_columns(&self, _node_type: &NodeType) -> Vec<SortableColumn> {
        tracking_sortable_columns()
    }
    fn actions(&self) -> Vec<NodeAction> {
        tracking_root_actions()
    }
    async fn list(
        &self,
        params: not_yet_done_content::ListParams,
    ) -> Result<not_yet_done_content::ListResult> {
        let filter = resolve_visible_set(&self.handle, &params.query).await?;
        let now = chrono::Utc::now();
        Ok(list_result(self.snapshot.entries(filter.as_ref(), now)))
    }
    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        TrackingEntryNode::fetch(&self.snapshot, &self.handle, id)
    }
    async fn invoke_action(&self, name: &str, _ctx: &ActionContext) -> Result<ActionDispatch> {
        Ok(match name {
            "restore-all" => {
                let candidates: Vec<Uuid> = self.snapshot.order.clone();
                invoke_restore_all(&self.handle, &candidates).await
            }
            _ => ActionDispatch::Noop,
        })
    }
}

/// A single tracking row. A leaf: it has no children. Carries the live
/// [`CoreHandle`] and the row's `task_id` so its actions (delete / restore /
/// toggle-tracking) can mutate through the services.
struct TrackingEntryNode {
    id_str: String,
    label: String,
    node_type: NodeType,
    metadata: Metadata,
    handle: CoreHandle,
    /// The tracked task — `toggle-tracking` flips tracking on it.
    task_id: Uuid,
}

impl TrackingEntryNode {
    fn fetch(
        snapshot: &Arc<TrackingSnapshot>,
        handle: &CoreHandle,
        id: &str,
    ) -> Result<Box<dyn Node>> {
        let uuid = Uuid::parse_str(id).map_err(|_| ContentError::NotFound(id.to_string()))?;
        let row = snapshot
            .by_id
            .get(&uuid)
            .ok_or_else(|| ContentError::NotFound(id.to_string()))?;
        let now = chrono::Utc::now();
        Ok(Box::new(TrackingEntryNode {
            id_str: id.to_string(),
            label: row.task_description.clone(),
            node_type: tracking_entry_type(),
            metadata: entry_metadata(row, now),
            handle: handle.clone(),
            task_id: row.tracking.task_id,
        }))
    }

    /// The row's own tracking id, parsed from [`Node::id`].
    fn tracking_id(&self) -> Result<Uuid> {
        Uuid::parse_str(&self.id_str).map_err(|_| ContentError::NotFound(self.id_str.clone()))
    }
}

#[async_trait]
impl Node for TrackingEntryNode {
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
        Vec::new()
    }
    fn actions(&self) -> Vec<NodeAction> {
        tracking_entry_actions()
    }
    async fn execute(&mut self, action_id: &str, _input: ActionInput) -> Result<ActionOutcome> {
        match action_id {
            // Reached via the generic `DeleteSelf` confirm flow, which calls
            // `execute("delete")` after the user confirms.
            "delete" => execute_delete(&self.handle, self.tracking_id()?).await,
            other => Err(ContentError::NotSupported(format!(
                "action `{other}` not supported on a tracking"
            ))),
        }
    }
    async fn invoke_action(&self, name: &str, _ctx: &ActionContext) -> Result<ActionDispatch> {
        Ok(match name {
            // Routed to the generic delete-confirm flow; the actual delete
            // happens in `execute("delete")` after confirmation.
            "delete" => ActionDispatch::DeleteSelf,
            "restore" => match self.tracking_id() {
                Ok(id) => invoke_restore(&self.handle, id).await,
                Err(_) => ActionDispatch::Error("Invalid tracking id".to_string()),
            },
            "toggle-tracking" => invoke_toggle_tracking(&self.handle, self.task_id).await,
            _ => ActionDispatch::Noop,
        })
    }
}

/// Wrap a summary list into a `ListResult`. No server-side sort or
/// pagination — the list is in-memory; the engine sorts/slices locally.
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

/// Bridge the core domain-event bus into this adapter's invalidation stream
/// **and** drop the eager snapshot on a change so the next fetch rebuilds.
///
/// - `TrackingStarted`/`Stopped` → a tracking appeared/closed: the list and
///   the active set change, so clear + [`Invalidation::All`]. The reload it
///   triggers rebuilds the snapshot and re-announces the live cadence (see
///   [`TrackingAdapter::announce_interval`]).
/// - `TaskChanged` → a task's description/path may have changed → clear +
///   `All`.
/// - `TrackingTick` is ignored: the per-second duration tick is driven by
///   the M9 live-row pull, not this global heartbeat (the heartbeat still
///   serves the native tab until the C1 cutover).
fn spawn_tracking_bridge(
    mut events: DomainEventReceiver,
    inv_tx: broadcast::Sender<Invalidation>,
    snapshot: Arc<RwLock<Option<Arc<TrackingSnapshot>>>>,
) {
    tokio::spawn(async move {
        use broadcast::error::RecvError;
        loop {
            match events.recv().await {
                Ok(DomainEvent::TrackingTick) => {}
                Ok(DomainEvent::TaskChanged { .. })
                | Ok(DomainEvent::TrackingStarted { .. })
                | Ok(DomainEvent::TrackingStopped { .. })
                | Ok(DomainEvent::TrackingChanged { .. }) => {
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

/// Builds [`TrackingAdapter`] instances bound to a captured [`CoreHandle`].
pub struct TrackingAdapterFactory {
    handle: CoreHandle,
}

impl TrackingAdapterFactory {
    pub fn new(handle: CoreHandle) -> Self {
        Self { handle }
    }
}

impl AdapterFactory for TrackingAdapterFactory {
    fn adapter_type(&self) -> &str {
        "trackings"
    }

    fn create(&self, instance_id: &str, _config: &str) -> Result<Box<dyn ContentAdapter>> {
        let (inv_tx, _) = broadcast::channel(64);
        let snapshot: Arc<RwLock<Option<Arc<TrackingSnapshot>>>> = Arc::new(RwLock::new(None));
        spawn_tracking_bridge(
            self.handle.events.subscribe(),
            inv_tx.clone(),
            snapshot.clone(),
        );
        let queries_root = dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("not_yet_done")
            .join("trackings")
            .join(instance_id)
            .join("queries");
        Ok(Box::new(TrackingAdapter {
            instance_id: instance_id.to_string(),
            handle: self.handle.clone(),
            inv_tx,
            snapshot,
            saved_queries: FsSavedQueryStore::new(queries_root),
        }))
    }
}

/// In-process adapter presenting the host's tracking list as content rows.
pub struct TrackingAdapter {
    instance_id: String,
    handle: CoreHandle,
    inv_tx: broadcast::Sender<Invalidation>,
    /// Eager snapshot, shared with every node. `None` until first load;
    /// cleared by [`spawn_tracking_bridge`] on change.
    snapshot: Arc<RwLock<Option<Arc<TrackingSnapshot>>>>,
    /// Filesystem-backed saved queries (`<data>/not_yet_done/trackings/<id>/
    /// queries/*.yaml`). Bodies are `name`/`query`(`FilterExpr`)/`options`;
    /// applying one filters the list via [`resolve_visible_set`].
    saved_queries: FsSavedQueryStore,
}

impl TrackingAdapter {
    /// Return the cached snapshot, loading it from the services if absent.
    async fn snapshot(&self) -> Result<Arc<TrackingSnapshot>> {
        if let Some(snap) = self.snapshot.read().await.as_ref() {
            return Ok(snap.clone());
        }
        let mut guard = self.snapshot.write().await;
        if let Some(snap) = guard.as_ref() {
            return Ok(snap.clone());
        }
        let snap = TrackingSnapshot::load(&self.handle).await?;
        *guard = Some(snap.clone());
        drop(guard);
        self.announce_interval(&snap);
        Ok(snap)
    }

    /// Force a fresh load (reload semantics) and cache it.
    async fn reload_snapshot(&self) -> Result<Arc<TrackingSnapshot>> {
        let snap = TrackingSnapshot::load(&self.handle).await?;
        *self.snapshot.write().await = Some(snap.clone());
        self.announce_interval(&snap);
        Ok(snap)
    }

    /// Tell the frontend whether to run the 1 Hz live-row pull: `Some` while
    /// a tracking is running, `None` when none is (M9). Emitted on every
    /// (re)load so a tracking that started/stopped in another tab re-paces
    /// the timer after the reload its event triggered.
    fn announce_interval(&self, snapshot: &TrackingSnapshot) {
        let interval = (snapshot.active_count() > 0).then_some(LIVE_INTERVAL);
        let _ = self.inv_tx.send(Invalidation::RefreshInterval(interval));
    }
}

#[async_trait]
impl ContentAdapter for TrackingAdapter {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn adapter_type(&self) -> &str {
        "trackings"
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            // A2b: delete (soft) + restore. No create (trackings are born
            // from the task toggle, not authored here) and no reparent.
            supports_delete: true,
            supports_search: true,
            ..AdapterCapabilities::default()
        }
    }

    fn actions_for_type(&self, node_type: &NodeType) -> Vec<NodeAction> {
        match node_type.type_id.as_str() {
            "tracking:root" => tracking_root_actions(),
            "tracking:entry" => tracking_entry_actions(),
            _ => Vec::new(),
        }
    }

    async fn root(&self) -> Result<Box<dyn Node>> {
        let snapshot = self.reload_snapshot().await?;
        Ok(Box::new(TrackingRootNode {
            snapshot,
            handle: self.handle.clone(),
            node_type: tracking_root_type(),
            metadata: Metadata::default(),
        }))
    }

    async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
        let snapshot = self.snapshot().await?;
        if id == ROOT_ID {
            return Ok(Box::new(TrackingRootNode {
                snapshot,
                handle: self.handle.clone(),
                node_type: tracking_root_type(),
                metadata: Metadata::default(),
            }));
        }
        TrackingEntryNode::fetch(&snapshot, &self.handle, id)
    }

    fn subscribe_invalidations(&self) -> broadcast::Receiver<Invalidation> {
        self.inv_tx.subscribe()
    }

    fn saved_query_store(&self) -> Option<&dyn SavedQueryStore> {
        Some(&self.saved_queries)
    }

    async fn live_rows(&self) -> Vec<NodeSummary> {
        let Ok(snapshot) = self.snapshot().await else {
            return Vec::new();
        };
        let now = chrono::Utc::now();
        snapshot
            .order
            .iter()
            .filter_map(|id| {
                let row = snapshot.by_id.get(id)?;
                row.active.then(|| entry_summary(*id, row, now))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracking(id: Uuid, task_id: Uuid, started_min_ago: i64, ended: bool) -> tracking::Model {
        let started = chrono::Utc::now() - chrono::Duration::minutes(started_min_ago);
        tracking::Model {
            id,
            task_id,
            predecessor_id: None,
            started_at: started,
            ended_at: ended.then(|| started + chrono::Duration::minutes(30)),
            deleted: false,
            created_at: started,
        }
    }

    fn row(model: tracking::Model, desc: &str, path: Vec<&str>) -> TrackingRow {
        let active = model.ended_at.is_none();
        TrackingRow {
            tracking: model,
            task_description: desc.to_string(),
            task_path: path.into_iter().map(str::to_string).collect(),
            active,
        }
    }

    fn snapshot_from(rows: Vec<(Uuid, TrackingRow)>) -> Arc<TrackingSnapshot> {
        let mut by_id = HashMap::new();
        let mut order = Vec::new();
        for (id, r) in rows {
            order.push(id);
            by_id.insert(id, r);
        }
        Arc::new(TrackingSnapshot { by_id, order })
    }

    #[test]
    fn path_for_walks_ancestors_root_first_excluding_self() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let c = Uuid::from_u128(3);
        let map: HashMap<Uuid, (String, Option<Uuid>)> = [
            (a, ("A".to_string(), None)),
            (b, ("B".to_string(), Some(a))),
            (c, ("C".to_string(), Some(b))),
        ]
        .into_iter()
        .collect();
        // Path of C excludes C itself: [A, B], root first.
        assert_eq!(path_for(&map, c), vec!["A".to_string(), "B".to_string()]);
        // A top-level task has an empty path.
        assert!(path_for(&map, a).is_empty());
    }

    #[test]
    fn path_for_is_cycle_guarded() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        // a → b → a (corrupt loop).
        let map: HashMap<Uuid, (String, Option<Uuid>)> = [
            (a, ("A".to_string(), Some(b))),
            (b, ("B".to_string(), Some(a))),
        ]
        .into_iter()
        .collect();
        // Terminates rather than looping forever.
        let p = path_for(&map, a);
        assert!(p.len() <= 2);
    }

    #[test]
    fn canonical_path_formats_leading_slash() {
        assert_eq!(canonical_path(&[]), "");
        assert_eq!(
            canonical_path(&["a".to_string(), "b".to_string()]),
            "/a/b"
        );
    }

    #[test]
    fn duration_seconds_completed_is_static() {
        let now = chrono::Utc::now();
        let m = tracking(Uuid::from_u128(1), Uuid::from_u128(9), 60, true); // 30-min span
        let r = row(m, "Task", vec![]);
        assert_eq!(duration_seconds(&r, now), 30 * 60);
    }

    #[test]
    fn duration_seconds_active_grows_to_now() {
        let now = chrono::Utc::now();
        let m = tracking(Uuid::from_u128(1), Uuid::from_u128(9), 10, false); // started 10 min ago, open
        let r = row(m, "Task", vec![]);
        let secs = duration_seconds(&r, now);
        // ~600s (10 min); allow a little slack for test wall-clock.
        assert!((595..=605).contains(&secs), "got {secs}");
    }

    #[test]
    fn entry_metadata_carries_canonical_typed_values() {
        let now = chrono::Utc::now();
        let m = tracking(Uuid::from_u128(7), Uuid::from_u128(9), 60, true);
        let r = row(m, "Write report", vec!["Work", "Q2"]);
        let md = entry_metadata(&r, now);
        let get = |k: &str| md.fields.iter().find(|f| f.key == k).map(|f| f.value.clone());
        assert_eq!(get("marker").as_deref(), Some("")); // completed → no glyph
        assert_eq!(get("taskpath").as_deref(), Some("/Work/Q2"));
        assert_eq!(get("task").as_deref(), Some("Write report"));
        assert_eq!(get("duration").as_deref(), Some("1800"));
        // Hidden per-task key for the Condensed view's inner grouping level.
        assert_eq!(get("task_id").as_deref(), Some(Uuid::from_u128(9).to_string().as_str()));
        let started = get("started").unwrap();
        assert!(chrono::DateTime::parse_from_rfc3339(&started).is_ok());
        assert!(chrono::DateTime::parse_from_rfc3339(&get("ended").unwrap()).is_ok());
    }

    #[test]
    fn active_entry_shows_marker_and_empty_ended() {
        let now = chrono::Utc::now();
        let m = tracking(Uuid::from_u128(7), Uuid::from_u128(9), 5, false);
        let r = row(m, "Task", vec![]);
        let md = entry_metadata(&r, now);
        let get = |k: &str| md.fields.iter().find(|f| f.key == k).map(|f| f.value.clone());
        assert_eq!(get("marker").as_deref(), Some("⏱"));
        assert_eq!(get("ended").as_deref(), Some(""));
    }

    #[test]
    fn entries_filter_keeps_only_visible_ids() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let snap = snapshot_from(vec![
            (a, row(tracking(a, Uuid::from_u128(9), 60, true), "A", vec![])),
            (b, row(tracking(b, Uuid::from_u128(9), 30, true), "B", vec![])),
        ]);
        let visible: HashSet<Uuid> = [b].into_iter().collect();
        let now = chrono::Utc::now();
        let rows = snap.entries(Some(&visible), now);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, b.to_string());
        // No filter → both, in insertion (newest-first) order.
        assert_eq!(snap.entries(None, now).len(), 2);
    }

    #[test]
    fn active_count_counts_running() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let snap = snapshot_from(vec![
            (a, row(tracking(a, Uuid::from_u128(9), 60, false), "A", vec![])), // active
            (b, row(tracking(b, Uuid::from_u128(9), 30, true), "B", vec![])),  // done
        ]);
        assert_eq!(snap.active_count(), 1);
    }

    fn has(actions: &[NodeAction], id: &str) -> bool {
        actions.iter().any(|a| a.id == id)
    }

    #[test]
    fn entry_actions_expose_delete_restore_toggle() {
        let a = tracking_entry_actions();
        assert!(has(&a, "delete"));
        assert!(has(&a, "restore"));
        assert!(has(&a, "toggle-tracking"));
        // `delete` and `toggle-tracking` show in the action bar; `restore`
        // is a recovery shortcut only.
        let key = |id: &str| a.iter().find(|x| x.id == id).and_then(|x| x.default_key);
        assert_eq!(key("delete"), Some('d'));
        assert_eq!(key("toggle-tracking"), Some('t'));
        assert_eq!(key("restore"), Some('R'));
    }

    #[test]
    fn root_actions_expose_restore_all_only() {
        let a = tracking_root_actions();
        assert!(has(&a, "restore-all"));
        // No per-row affordances on the list root.
        assert!(!has(&a, "delete"));
        assert!(!has(&a, "toggle-tracking"));
    }

    #[test]
    fn entries_are_leaves() {
        let a = Uuid::from_u128(1);
        let snap = snapshot_from(vec![(
            a,
            row(tracking(a, Uuid::from_u128(9), 60, true), "A", vec![]),
        )]);
        let now = chrono::Utc::now();
        assert_eq!(snap.entries(None, now)[0].has_children, Some(false));
    }
}
