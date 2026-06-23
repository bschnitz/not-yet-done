//! `TrackingAdapter` — an in-process [`ContentAdapter`] over the host's own
//! [`TrackingRepository`](not_yet_done_task_core::repository::TrackingRepository).
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
//! A running tracking's duration ticks while it runs. Rather than a
//! render-time `kind: elapsed` column (which can't also show the *static*
//! `ended − started` of a completed tracking in the same column), the
//! adapter drives the generic **live-row** mechanism: while ≥1 tracking is
//! active it asks the frontend to pull [`live_rows`](ContentAdapter::live_rows)
//! (via [`Invalidation::RefreshInterval`]); each pull recomputes
//! `now − started` for the active rows and the frontend patches them in
//! place ([`Invalidation::Row`]). The cadence is **adaptive** like the
//! native tab's ([`live_interval_for`]: 5 s under a minute, then 10 s/30 s/
//! 60 s as the youngest tracking ages) and re-paces itself from each pull.
//! When the last tracking stops the adapter sends `RefreshInterval(None)`
//! and the pull stops.
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
//! generic `:script` menu.
//!
//! ## Scope (A2c — Condensed + Tree sub-views)
//!
//! Two further views over the same data, declared in `views/trackings.yaml`:
//!
//! - **Condensed** (`tracking:condensed-row`) — *adapter-side* condensing: the
//!   adapter collapses each day's intervals of a task into one row carrying the
//!   per-cell duration sum ([`TrackingSnapshot::condensed_summaries`], built on
//!   the generic [`grouping::condense_cells`] kernel). Condensing is
//!   interpretation of the data, not rendering, so it belongs to the data
//!   owner — the engine then only single-level-groups the rows into `── day ──`
//!   headers, which lets the requested item sort (`S`) order the task rows
//!   within each day. No adapter is forced to condense; one that can't simply
//!   exposes no condensed view.
//! - **Tree** — a second projection of the same loads: the **task forest**
//!   (`tracking:tree-item`) where each node carries its own tracked seconds
//!   plus the subtree-cumulated total ([`TreeProjection`], folded bottom-up).
//!   The view declares a `tree_aggregate` column that toggles own↔cumulated
//!   (M4); the adapter advertises [`AdapterCapabilities::supports_tree_aggregation`].
//!   The tree is pruned to tasks with tracked time and bakes durations at
//!   load (no live tick — like Condensed).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use not_yet_done_content::{
    apply_sort, grouping, ActionContext, ActionDispatch, ActionInput, ActionOutcome,
    AdapterCapabilities, AdapterFactory, ContentAdapter, ContentError, FormFieldSpec,
    FsSavedQueryStore, GroupBucket, GroupSpec, HintPlacement, HostContext, HostEvent, InputSpec,
    Invalidation, Metadata, MetadataField, Node, NodeAction, NodeSummary, NodeType, Result,
    SavedQueryStore, SortDirection, SortKey, SortKind, SortableColumn, Subtree, SubtreeNode,
};
use not_yet_done_task_core::entity::granularity::Granularity;
use not_yet_done_task_core::entity::tracking;
use not_yet_done_task_core::error::AppError;
use not_yet_done_task_core::events::DomainEvent;
use not_yet_done_task_core::service::{GravityDirection, MoveOptions};

use crate::datetime::{LocalDateTime, LocalOffset};
use crate::form::{form_flag, form_opt, form_required, invalid_input};
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;

use crate::{as_domain_event, publish_row_patches, CoreHandle};

/// Stable id of the synthetic list-root node.
const ROOT_ID: &str = "tracking:root";

/// Adaptive live-refresh cadence (M9), matching the native Trackings tab
/// (`App::tick_active_trackings`): a young tracking ticks fast (the user is
/// watching the seconds), an old one slowly (the displayed value barely
/// changes and every tick costs a recompute + repaint). Keyed off the
/// *shortest* active duration so the most recently started tracking sets
/// the pace.
fn live_interval_for(shortest_active_secs: i64) -> Duration {
    let secs = if shortest_active_secs < 60 {
        5
    } else if shortest_active_secs < 600 {
        10
    } else if shortest_active_secs < 3600 {
        30
    } else {
        60
    };
    Duration::from_secs(secs)
}

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

/// The node type for a **condensed row** (A2c Condensed view): one
/// representative row per `(day, task)` cell, carrying that task's *summed*
/// tracked seconds for the day. Distinct from `tracking:entry` (a single
/// interval) because the condensing — collapsing a day's many intervals of one
/// task into a single aggregate row — is the adapter's job (interpretation of
/// the data, not rendering). The Condensed view in `views/trackings.yaml` binds
/// its columns to this type; the engine then only single-level-groups the rows
/// into `── day ──` headers, so the requested item sort (`S`) orders the task
/// rows *within* each day. See [`TrackingSnapshot::condensed_summaries`].
fn tracking_condensed_type() -> NodeType {
    NodeType {
        type_id: "tracking:condensed-row".to_string(),
        mime_type: "text/plain".to_string(),
        syntax: None,
        file_extension: ".txt".to_string(),
        display_name: "Tracking (condensed)".to_string(),
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

/// The node type for a task node in the **duration tree** (A2c Tree view):
/// the task forest projected so each node carries its own + subtree-cumulated
/// tracked time. Distinct from `tracking:entry` (a single interval) — a
/// `views/trackings.yaml` binds the tree view's columns to this type and
/// declares the `tree_aggregate` column on it.
fn tracking_tree_item_type() -> NodeType {
    NodeType {
        type_id: "tracking:tree-item".to_string(),
        mime_type: "text/plain".to_string(),
        syntax: None,
        file_extension: ".txt".to_string(),
        display_name: "Task".to_string(),
    }
}

/// The node type for a **group bucket** in the grouped tree (generic
/// `group_by_via_adapter` mechanism): one node per day/week/month/year (or
/// verbatim value) the active grouping partitions the trackings into. Each
/// bucket's children are `tracking:tree-item` nodes whose durations are
/// re-folded from that bucket's trackings only — the per-bucket fold the
/// engine cannot do itself. The tree view in `views/trackings.yaml` uses this
/// as its root `node_type`; when grouping is cycled off the adapter answers
/// the same root request with plain `tracking:tree-item` rows instead (the
/// frontend's type-based chain resolution handles both shapes).
fn tracking_tree_group_type() -> NodeType {
    NodeType {
        type_id: "tracking:tree-group".to_string(),
        mime_type: "text/plain".to_string(),
        syntax: None,
        file_extension: ".txt".to_string(),
        display_name: "Group".to_string(),
    }
}

/// Prefix on a `tracking:tree-item` node id. Tree nodes are addressed by the
/// *task* UUID, which is indistinguishable from a tracking UUID, so the id is
/// `tree:<task-uuid>` — [`TrackingAdapter::get_by_id`] routes on the prefix to
/// the right node kind. Inside a group bucket the id additionally embeds the
/// bucket scope: `tree:<column>:<gran>:<key>:<task-uuid>` (see [`BucketScope`]).
const TREE_ID_PREFIX: &str = "tree:";

/// Prefix on a `tracking:tree-group` node id: `treegrp:<column>:<gran>:<key>`.
const GROUP_ID_PREFIX: &str = "treegrp:";

/// Prefix on a `tracking:condensed-row` node id: `cond:<day-key>:<task-uuid>`.
/// The day key is the ISO bucket key (`2026-06-09`, no `:`), so a single
/// `rsplit_once(':')` recovers the task uuid from the day. Routes in
/// [`TrackingAdapter::get_by_id`] / [`TrackingRootNode::get_child`].
const CONDENSED_ID_PREFIX: &str = "cond:";

// ---------------------------------------------------------------------------
// Bucket scope (grouped tree)
// ---------------------------------------------------------------------------

/// The bucket a grouped-tree level is scoped to. Encoded into every node id
/// under a group (`treegrp:…` and the scoped `tree:…` items) because that is
/// the only context [`ContentAdapter::get_by_id`] has: the same task appears
/// in several buckets, so the id alone must say *which* bucket's re-folded
/// durations the node carries. The pane's saved query, by contrast, arrives
/// per `list()` call (capability `propagates_query_to_subtree`) and is
/// intersected on top.
#[derive(Clone, Debug, PartialEq, Eq)]
struct BucketScope {
    /// The grouped column (`GroupSpec::column`), so bucket membership can be
    /// recomputed from an id without the original spec.
    column: String,
    /// The date granularity; `None` groups the column's value verbatim.
    bucket: Option<GroupBucket>,
    /// The bucket key ([`grouping::group_key`]), e.g. `2026-06-09`.
    key: String,
}

impl BucketScope {
    /// `<column>:<gran>:<key>` — the id-embedded form. The key goes last so
    /// a verbatim key containing `:` survives (parsers split off the fixed
    /// prefix fields and keep the remainder).
    fn encode(&self) -> String {
        format!("{}:{}:{}", self.column, gran_token(self.bucket), self.key)
    }

    fn parse(s: &str) -> Option<Self> {
        let (column, rest) = s.split_once(':')?;
        let (gran, key) = rest.split_once(':')?;
        Some(BucketScope {
            column: column.to_string(),
            bucket: parse_gran(gran)?,
            key: key.to_string(),
        })
    }
}

/// Id token for a bucket granularity (`day`/`week`/`month`/`year`; `none`
/// = verbatim grouping).
fn gran_token(bucket: Option<GroupBucket>) -> &'static str {
    match bucket {
        None => "none",
        Some(GroupBucket::Day) => "day",
        Some(GroupBucket::Week) => "week",
        Some(GroupBucket::Month) => "month",
        Some(GroupBucket::Year) => "year",
    }
}

/// Inverse of [`gran_token`]. Outer `None` = unknown token (bad id).
fn parse_gran(s: &str) -> Option<Option<GroupBucket>> {
    Some(match s {
        "none" => None,
        "day" => Some(GroupBucket::Day),
        "week" => Some(GroupBucket::Week),
        "month" => Some(GroupBucket::Month),
        "year" => Some(GroupBucket::Year),
        _ => return None,
    })
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

/// One task in the **duration tree** projection (A2c Tree view): its label,
/// effective parent, and the tracked time rolled up two ways — its own and
/// its whole subtree's.
#[derive(Clone)]
struct TreeTaskRow {
    /// The task's description (the tree row's label / `task` column).
    description: String,
    /// Sum of this task's own non-deleted trackings' durations, in seconds
    /// (a running interval counts `now − started` at snapshot-build time —
    /// the tree does not tick live, see the M9 note).
    own_secs: i64,
    /// `own_secs` plus every descendant's `own_secs`, folded bottom-up. This
    /// is the value the `tree_aggregate` column shows in its cumulated state.
    cumulated_secs: i64,
    /// `true` while this task has a running tracking — seeds the `⏱` marker.
    active: bool,
}

/// The task forest projected with per-task tracked durations (A2c Tree).
/// Built once alongside the flat list (both read the same task + tracking
/// loads), and pruned to the tasks that carry tracked time anywhere in their
/// subtree (`cumulated_secs > 0`) so the tree shows only worked-on work.
#[derive(Clone, Default)]
struct TreeProjection {
    by_id: HashMap<Uuid, TreeTaskRow>,
    /// parent id → ordered child ids (`None` key = forest roots).
    children: HashMap<Option<Uuid>, Vec<Uuid>>,
}

impl TreeProjection {
    /// A task belongs in the tree iff it (or some descendant) has tracked
    /// time. Pruning keeps the path down to every tracked leaf while hiding
    /// untouched branches.
    fn is_visible(&self, id: Uuid) -> bool {
        self.by_id.get(&id).is_some_and(|r| r.cumulated_secs > 0)
    }

    /// Visible child summaries of `parent` (`None` = forest roots), ordered
    /// as the children map records them. Inside a group bucket the `scope`
    /// is embedded in every id (see [`BucketScope`]); `None` keeps the plain
    /// `tree:<uuid>` ids of the ungrouped tree.
    fn child_summaries(&self, parent: Option<Uuid>, scope: Option<&BucketScope>) -> Vec<NodeSummary> {
        self.children
            .get(&parent)
            .map(|ids| {
                ids.iter()
                    .filter(|id| self.is_visible(**id))
                    .filter_map(|id| self.by_id.get(id).map(|row| self.summary(*id, row, scope)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Walk this projection from `parent` (`None` = forest roots) down `depth`
    /// additional levels, building the eager [`Subtree`] the engine ingests in
    /// one shot (capability `supports_eager_subtree`). One level mirrors
    /// [`Self::child_summaries`]; a node is only descended while there is depth
    /// budget left and it actually has visible children. `depth == u32::MAX`
    /// expands to every visible leaf. Pure in-memory — no DB, no async, no
    /// per-node round-trip (the whole point: it replaces the O(N²) TUI
    /// expand-cascade with a single pass).
    fn subtree(&self, parent: Option<Uuid>, scope: Option<&BucketScope>, depth: u32) -> Subtree {
        let items = self
            .children
            .get(&parent)
            .map(|ids| {
                ids.iter()
                    .filter(|id| self.is_visible(**id))
                    .filter_map(|id| {
                        let row = self.by_id.get(id)?;
                        let summary = self.summary(*id, row, scope);
                        let children = if depth > 0 && summary.has_children == Some(true) {
                            self.subtree(Some(*id), scope, depth - 1)
                        } else {
                            Subtree::default()
                        };
                        Some(SubtreeNode { summary, children })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Subtree { items, page: None }
    }

    fn summary(&self, id: Uuid, row: &TreeTaskRow, scope: Option<&BucketScope>) -> NodeSummary {
        let has_children = self
            .children
            .get(&Some(id))
            .map(|kids| kids.iter().any(|c| self.is_visible(*c)))
            .unwrap_or(false);
        NodeSummary {
            id: tree_item_id(scope, id),
            label: row.description.clone(),
            node_type: tracking_tree_item_type(),
            metadata: tree_metadata(id, row),
            has_children: Some(has_children),
        }
    }
}

/// A tree item's node id: `tree:<uuid>` ungrouped, `tree:<scope>:<uuid>`
/// inside a group bucket.
fn tree_item_id(scope: Option<&BucketScope>, id: Uuid) -> String {
    match scope {
        Some(s) => format!("{TREE_ID_PREFIX}{}:{id}", s.encode()),
        None => format!("{TREE_ID_PREFIX}{id}"),
    }
}

/// Immutable, eagerly-loaded view of the whole non-deleted tracking list.
/// Shared by `Arc` across every node the adapter hands out.
struct TrackingSnapshot {
    by_id: HashMap<Uuid, TrackingRow>,
    /// Display order — newest first, as [`TrackingRepository::find_all`]
    /// returns them.
    order: Vec<Uuid>,
    /// The task forest projected with per-task durations — the A2c Tree view
    /// (`tracking:tree-item`). Built from the same task + tracking loads.
    /// This is the *unfiltered* projection; a saved-query filter re-folds
    /// on demand via [`Self::tree_for`].
    tree: Arc<TreeProjection>,
    /// task id → (description, parent) — kept so [`Self::tree_for`] can
    /// re-fold the projection for a filtered tracking set.
    task_map: HashMap<Uuid, (String, Option<Uuid>)>,
    /// The instant the snapshot baked its durations. Filtered re-folds
    /// reuse it so a running tracking contributes the *same* seconds at
    /// every tree level (root and branches load moments apart) and the
    /// filtered tree matches the unfiltered one's bake-at-load semantics.
    built_at: chrono::DateTime<chrono::Utc>,
    /// Saved-query → resolved visible-tracking set, memoized per snapshot.
    /// An `expand_depth: all` cascade calls `list()` once per expanded node
    /// and each call used to re-run the filter against the DB; the result
    /// only changes when the data does, and any mutation replaces the whole
    /// snapshot, so entries can never go stale.
    visible_cache: std::sync::RwLock<HashMap<String, Arc<HashSet<Uuid>>>>,
    /// `(bucket scope, saved query)` → folded projection, memoized per
    /// snapshot for the same reason: without it every scoped `fetch`/`list`
    /// re-folded the whole task forest (O(tasks + trackings)), which made
    /// grouped trees visibly slow to expand.
    fold_cache: std::sync::RwLock<HashMap<(String, String), Arc<TreeProjection>>>,
}

impl TrackingSnapshot {
    /// Build a snapshot from the live services: load *every* tracking
    /// (deleted included), resolve each task's description + ancestor path
    /// from a single `list_tasks` pass, and record running state.
    ///
    /// The universe is unfiltered on purpose (adapter contract): the saved
    /// query is the single, replaceable filter. The shipped default query
    /// `[deleted, =, false]` hides deleted rows; a query that drops or
    /// flips that clause surfaces them so `restore` has a target. Deleted
    /// trackings still never count as *running* and never contribute to
    /// tracked-time roll-ups — that's a fact about their state, not a view
    /// filter, so it stays hard-coded here.
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
            .find_all_including_deleted()
            .await
            .map_err(to_content_err)?;

        // Durations bake at load time (the running interval counts up to
        // `now`); the flat list re-derives them live per `entries(now)`, but
        // the tree projection is a static snapshot (no live tick — M9).
        let now = chrono::Utc::now();
        let mut by_id = HashMap::with_capacity(trackings.len());
        let mut order = Vec::with_capacity(trackings.len());
        // Per-task own seconds + the set of tasks with a running tracking,
        // accumulated in the same pass that builds the flat rows. Both are
        // computed over the *live* set only — a deleted tracking is neither
        // running nor tracked time, regardless of any query.
        let mut own_secs: HashMap<Uuid, i64> = HashMap::new();
        let mut active_tasks: HashSet<Uuid> = HashSet::new();
        for t in trackings {
            let task_description = task_map
                .get(&t.task_id)
                .map(|(desc, _)| desc.clone())
                .unwrap_or_else(|| "(unknown task)".to_string());
            let task_path = path_for(&task_map, t.task_id);
            let active = t.ended_at.is_none() && !t.deleted;
            if !t.deleted {
                *own_secs.entry(t.task_id).or_default() += model_duration_seconds(&t, now);
            }
            if active {
                active_tasks.insert(t.task_id);
            }
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
        let tree = Arc::new(build_tree_projection(&task_map, &own_secs, &active_tasks));
        Ok(Arc::new(TrackingSnapshot {
            by_id,
            order,
            tree,
            task_map,
            built_at: now,
            visible_cache: Default::default(),
            fold_cache: Default::default(),
        }))
    }

    /// The duration-tree projection for an optional saved-query filter.
    /// `None` reuses the eagerly built full projection; `Some(visible)`
    /// re-folds the tree from the visible trackings only — own seconds and
    /// running markers come from the filtered set, and tasks whose subtree
    /// carries no visible tracked time prune away, exactly like untracked
    /// tasks do in the unfiltered tree.
    fn tree_for(&self, filter: Option<&HashSet<Uuid>>) -> std::borrow::Cow<'_, TreeProjection> {
        let Some(visible) = filter else {
            return std::borrow::Cow::Borrowed(self.tree.as_ref());
        };
        let now = self.built_at;
        let mut own_secs: HashMap<Uuid, i64> = HashMap::new();
        let mut active_tasks: HashSet<Uuid> = HashSet::new();
        for id in visible {
            let Some(row) = self.by_id.get(id) else { continue };
            *own_secs.entry(row.tracking.task_id).or_default() += duration_seconds(row, now);
            if row.active {
                active_tasks.insert(row.tracking.task_id);
            }
        }
        std::borrow::Cow::Owned(build_tree_projection(
            &self.task_map,
            &own_secs,
            &active_tasks,
        ))
    }

    /// The visible-tracking set for a pane's saved query, memoized in
    /// [`Self::visible_cache`]. `None` = no query → everything visible.
    async fn visible_set(
        &self,
        handle: &CoreHandle,
        query: &Option<String>,
    ) -> Result<Option<Arc<HashSet<Uuid>>>> {
        let raw = match query.as_deref().map(str::trim) {
            Some(q) if !q.is_empty() => q.to_string(),
            _ => return Ok(None),
        };
        if let Some(hit) = self.visible_cache.read().unwrap().get(&raw) {
            return Ok(Some(Arc::clone(hit)));
        }
        let set = resolve_visible_set(handle, query).await?.unwrap_or_default();
        let set = Arc::new(set);
        self.visible_cache
            .write()
            .unwrap()
            .insert(raw, Arc::clone(&set));
        Ok(Some(set))
    }

    /// The folded projection for a bucket scope without any query filter,
    /// memoized in [`Self::fold_cache`]. Synchronous — `fetch` paths have no
    /// query context and must not touch the DB.
    fn unfiltered_projection(&self, scope: Option<&BucketScope>) -> Arc<TreeProjection> {
        let Some(scope) = scope else {
            return Arc::clone(&self.tree);
        };
        let key = (scope.encode(), String::new());
        if let Some(hit) = self.fold_cache.read().unwrap().get(&key) {
            return Arc::clone(hit);
        }
        let members = self.bucket_members(None, scope);
        let folded = Arc::new(self.tree_for(Some(&members)).into_owned());
        self.fold_cache
            .write()
            .unwrap()
            .insert(key, Arc::clone(&folded));
        folded
    }

    /// The folded projection for a `(bucket scope, saved query)` pair,
    /// memoized in [`Self::fold_cache`]. This is the hot path of an
    /// `expand_depth: all` cascade — every expanded node lands here, so the
    /// first call per pair folds and the rest are map lookups.
    async fn scoped_projection(
        &self,
        handle: &CoreHandle,
        scope: Option<&BucketScope>,
        query: &Option<String>,
    ) -> Result<Arc<TreeProjection>> {
        let Some(visible) = self.visible_set(handle, query).await? else {
            return Ok(self.unfiltered_projection(scope));
        };
        let key = (
            scope.map(BucketScope::encode).unwrap_or_default(),
            query.as_deref().map(str::trim).unwrap_or("").to_string(),
        );
        if let Some(hit) = self.fold_cache.read().unwrap().get(&key) {
            return Ok(Arc::clone(hit));
        }
        let folded = match scope {
            Some(s) => {
                let members = self.bucket_members(Some(visible.as_ref()), s);
                Arc::new(self.tree_for(Some(&members)).into_owned())
            }
            None => Arc::new(self.tree_for(Some(visible.as_ref())).into_owned()),
        };
        self.fold_cache
            .write()
            .unwrap()
            .insert(key, Arc::clone(&folded));
        Ok(folded)
    }

    /// Elapsed seconds of the *youngest* running tracking (`None` when none
    /// runs) — the input to [`live_interval_for`]'s adaptive cadence.
    fn shortest_active_secs(&self, now: chrono::DateTime<chrono::Utc>) -> Option<i64> {
        self.by_id
            .values()
            .filter(|r| r.active)
            .map(|r| (now - r.tracking.started_at).num_seconds().max(0))
            .min()
    }

    /// The ids of the currently running trackings — what
    /// [`TrackingAdapter::revalidate`] diffs against the live DB to detect
    /// out-of-process starts/stops.
    fn active_ids(&self) -> HashSet<Uuid> {
        self.by_id
            .iter()
            .filter(|(_, r)| r.active)
            .map(|(id, _)| *id)
            .collect()
    }

    /// One summary per bucket the active grouping (`spec`) partitions the
    /// visible trackings into — the root level of the grouped tree. Each
    /// bucket carries the sum of its trackings' durations (baked at
    /// `built_at`, like the tree fold) and a `⏱` marker when one of them is
    /// still running. Buckets are ordered by their ISO key (lexical =
    /// chronological) per `spec.order`.
    fn group_summaries(&self, filter: Option<&HashSet<Uuid>>, spec: &GroupSpec) -> Vec<NodeSummary> {
        let now = self.built_at;
        let mut buckets: HashMap<String, (i64, bool)> = HashMap::new();
        for id in &self.order {
            if !filter.map_or(true, |f| f.contains(id)) {
                continue;
            }
            let Some(row) = self.by_id.get(id) else { continue };
            let key = grouping::group_key(&bucket_raw_value(row, &spec.column), spec.bucket);
            let entry = buckets.entry(key).or_insert((0, false));
            entry.0 += duration_seconds(row, now);
            entry.1 |= row.active;
        }
        let mut keys: Vec<String> = buckets.keys().cloned().collect();
        keys.sort();
        if spec.order == SortDirection::Desc {
            keys.reverse();
        }
        keys.into_iter()
            .map(|key| {
                let (total_secs, active) = buckets[&key];
                let scope = BucketScope {
                    column: spec.column.clone(),
                    bucket: spec.bucket,
                    key,
                };
                group_summary(&scope, total_secs, active)
            })
            .collect()
    }

    /// Eager analogue of [`Self::group_summaries`]: build the grouped tree's
    /// bucket level **and** fold each bucket's task subtree `depth - 1` levels
    /// deep, reusing the same `(scope, query)`-memoized projections the
    /// per-node cascade would. Bucket nodes form level 0, so a bucket's folded
    /// forest hangs one level beneath them.
    async fn group_subtree(
        &self,
        handle: &CoreHandle,
        filter: Option<&HashSet<Uuid>>,
        spec: &GroupSpec,
        query: &Option<String>,
        depth: u32,
    ) -> Result<Subtree> {
        let mut items = Vec::new();
        for summary in self.group_summaries(filter, spec) {
            // Decode the bucket scope back out of the summary id — the same
            // `treegrp:<scope>` shape `TrackingGroupNode::fetch` parses.
            let scope = summary
                .id
                .strip_prefix(GROUP_ID_PREFIX)
                .and_then(BucketScope::parse);
            let children = match (depth > 0).then_some(()).and(scope) {
                Some(scope) => self
                    .scoped_projection(handle, Some(&scope), query)
                    .await?
                    .subtree(None, Some(&scope), depth - 1),
                None => Subtree::default(),
            };
            items.push(SubtreeNode { summary, children });
        }
        Ok(Subtree { items, page: None })
    }

    /// The id of the group bucket the **current instant** falls into for a
    /// grouped tree under `spec` — the [`ContentAdapter::bucket_for_now`]
    /// answer (M9 now-bucket refresh). Resolves to the bucket of the
    /// *youngest* tracking (max `started_at`): a start mints the newest
    /// interval and a stop freezes it, so the youngest is the one a toggle
    /// just shifted, and its start day is the bucket the row actually lives in
    /// (a tracking spanning midnight files under its start, not "today").
    /// Computed with the **same** [`bucket_raw_value`] + [`grouping::group_key`]
    /// the bucket level uses, so the returned `treegrp:` id always matches a
    /// real bucket row when one exists. `None` when there are no trackings.
    fn bucket_for_now(&self, spec: &GroupSpec) -> Option<String> {
        let scope = self.now_scope(spec)?;
        Some(format!("{GROUP_ID_PREFIX}{}", scope.encode()))
    }

    /// The bucket scope the current instant falls into — the youngest
    /// tracking's bucket (see [`Self::bucket_for_now`] for why youngest).
    /// Shared by `bucket_for_now` (id only) and [`Self::live_group_rows`]
    /// (which re-folds this bucket against live `now`). `None` with no
    /// trackings.
    fn now_scope(&self, spec: &GroupSpec) -> Option<BucketScope> {
        let row = self
            .by_id
            .values()
            .filter(|r| !r.tracking.deleted)
            .max_by_key(|r| r.tracking.started_at)?;
        Some(BucketScope {
            column: spec.column.clone(),
            bucket: spec.bucket,
            key: grouping::group_key(&bucket_raw_value(row, &spec.column), spec.bucket),
        })
    }

    /// The now-bucket's ticking rows folded against the *live* `now` — the
    /// M9 live-tick counterpart of the static `built_at` fold ([`group_subtree`]
    /// bakes durations at snapshot time; this re-folds them to the current
    /// instant). Returns the bucket header (total re-summed to `now`) plus the
    /// tree rows on the **running chain** — every task with a running tracking
    /// and all its ancestors, whose cumulated grows — keyed exactly as the
    /// rendered grouped tree (`treegrp:`/`tree:<scope>:<uuid>`), so the
    /// frontend's `patch_row` swaps each ticking cell in place. Untouched
    /// sibling tasks keep their frozen value and aren't returned (no churn).
    /// Empty when nothing is running or the bucket is empty.
    ///
    /// [`group_subtree`]: Self::group_subtree
    fn live_group_rows(
        &self,
        spec: &GroupSpec,
        filter: Option<&HashSet<Uuid>>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Vec<NodeSummary> {
        let Some(scope) = self.now_scope(spec) else {
            return Vec::new();
        };
        let members = self.bucket_members(filter, &scope);
        if members.is_empty() {
            return Vec::new();
        }
        // Re-fold own durations + the bucket total against live `now`, and
        // note which tasks are still running.
        let mut own_secs: HashMap<Uuid, i64> = HashMap::new();
        let mut active_tasks: HashSet<Uuid> = HashSet::new();
        let mut total_secs = 0i64;
        for id in &members {
            let Some(row) = self.by_id.get(id) else { continue };
            let secs = duration_seconds(row, now);
            *own_secs.entry(row.tracking.task_id).or_default() += secs;
            total_secs += secs;
            if row.active {
                active_tasks.insert(row.tracking.task_id);
            }
        }
        if active_tasks.is_empty() {
            return Vec::new(); // nothing running in this bucket → nothing ticks
        }
        let projection = build_tree_projection(&self.task_map, &own_secs, &active_tasks);
        // The running chain: each active task plus its ancestors (their
        // cumulated grows). Cycle-guarded by the `insert` short-circuit.
        let mut chain: HashSet<Uuid> = HashSet::new();
        for &task in &active_tasks {
            let mut current = Some(task);
            while let Some(id) = current {
                if !chain.insert(id) {
                    break;
                }
                current = self.task_map.get(&id).and_then(|(_, parent)| *parent);
            }
        }
        let mut rows = vec![group_summary(&scope, total_secs, true)];
        for id in chain {
            if projection.is_visible(id) {
                if let Some(row) = projection.by_id.get(&id) {
                    rows.push(projection.summary(id, row, Some(&scope)));
                }
            }
        }
        rows
    }

    /// The tracking ids that fall into `scope`'s bucket, optionally
    /// intersected with a saved-query `filter`. This is what a group node's
    /// subtree re-folds from.
    fn bucket_members(&self, filter: Option<&HashSet<Uuid>>, scope: &BucketScope) -> HashSet<Uuid> {
        self.by_id
            .iter()
            .filter(|(id, row)| {
                filter.map_or(true, |f| f.contains(id)) && scope_member(row, scope)
            })
            .map(|(id, _)| *id)
            .collect()
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

    /// Condensed rows (A2c Condensed view): one representative row per
    /// `(day, task)` cell, the task's summed tracked seconds for that day.
    ///
    /// The condensing lives here — in the adapter — because it is an
    /// *interpretation* of the data, not a rendering concern: collapsing a
    /// day's many intervals of one task into one aggregate is the kind of
    /// `GROUP BY` the data owner does best (a SQL-backed adapter could push it
    /// to the store). The generic, domain-free part (bucket identity + stable
    /// partitioning) is [`grouping::condense_cells`]; the per-cell aggregation
    /// (sum the duration, carry the label/marker/taskpath of a representative)
    /// is the trackings-specific part done here.
    ///
    /// The requested item sort (`S`) is applied to the condensed rows before
    /// they leave the adapter. The engine then only single-level-groups them
    /// into `── day ──` headers (`group_by: [started/day]`, stable), so the
    /// sort orders the task rows *within* each day — the agreed semantics.
    fn condensed_summaries(
        &self,
        filter: Option<&HashSet<Uuid>>,
        now: chrono::DateTime<chrono::Utc>,
        sort: &[SortKey],
    ) -> (Vec<NodeSummary>, Vec<SortKey>) {
        // Visible rows in display order (newest first); each cell's
        // representative is therefore its newest interval.
        let ids: Vec<Uuid> = self
            .order
            .iter()
            .copied()
            .filter(|id| filter.map_or(true, |f| f.contains(id)))
            .filter(|id| self.by_id.contains_key(id))
            .collect();
        // The condensing keys: outer = the interval's start (day-bucketed),
        // inner = the task id (not its label, so identically-named tasks stay
        // distinct — the same reason the engine `then_by` used `task_id`).
        let started: Vec<String> = ids
            .iter()
            .map(|id| self.by_id[id].tracking.started_at.to_rfc3339())
            .collect();
        let task_ids: Vec<String> = ids
            .iter()
            .map(|id| self.by_id[id].tracking.task_id.to_string())
            .collect();
        let cells = grouping::condense_cells(
            started
                .iter()
                .zip(task_ids.iter())
                .map(|(s, t)| (s.as_str(), t.as_str())),
            Some(GroupBucket::Day),
        );
        let mut items: Vec<NodeSummary> = cells
            .into_iter()
            .map(|cell| {
                let rep = &self.by_id[&ids[cell.members[0]]];
                let total_secs: i64 = cell
                    .members
                    .iter()
                    .map(|&m| duration_seconds(&self.by_id[&ids[m]], now))
                    .sum();
                let active = cell.members.iter().any(|&m| self.by_id[&ids[m]].active);
                condensed_summary(&cell.bucket_key, rep, total_secs, active)
            })
            .collect();
        let applied = apply_sort(&mut items, sort, &tracking_sortable_columns());
        (items, applied)
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

/// A tracking model's duration in **seconds**. Completed: `ended − started`;
/// running: `now − started`. Clamped to zero so a clock skew never renders a
/// negative duration. The canonical `kind: duration` input.
fn model_duration_seconds(t: &tracking::Model, now: chrono::DateTime<chrono::Utc>) -> i64 {
    let end = t.ended_at.unwrap_or(now);
    (end - t.started_at).num_seconds().max(0)
}

/// A tracking row's duration in seconds (see [`model_duration_seconds`]).
fn duration_seconds(row: &TrackingRow, now: chrono::DateTime<chrono::Utc>) -> i64 {
    model_duration_seconds(&row.tracking, now)
}

/// The raw value the grouped tree buckets a row by — the same canonical
/// string [`entry_metadata`] exposes for the column, so a day bucket here
/// matches the flat view's engine-side grouping exactly. Unknown columns
/// fall back to `started` (the shipped grouping).
fn bucket_raw_value(row: &TrackingRow, column: &str) -> String {
    match column {
        "task" => row.task_description.clone(),
        "taskpath" => canonical_path(&row.task_path),
        "ended" => row
            .tracking
            .ended_at
            .map(|d| d.to_rfc3339())
            .unwrap_or_else(|| "running".to_string()),
        _ => row.tracking.started_at.to_rfc3339(),
    }
}

/// Whether `row` falls into `scope`'s bucket.
fn scope_member(row: &TrackingRow, scope: &BucketScope) -> bool {
    grouping::group_key(&bucket_raw_value(row, &scope.column), scope.bucket) == scope.key
}

/// Column-backing metadata for a group-bucket node. `task` carries the
/// human label (the tree view's `tree_label` column); both duration columns
/// carry the bucket total so the `tree_aggregate` toggle (`zt`) never blanks
/// the group row.
fn group_metadata(label: &str, total_secs: i64, active: bool) -> Metadata {
    Metadata {
        fields: vec![
            field(
                "marker",
                if active { "⏱".to_string() } else { String::new() },
                "Active",
            ),
            field("task", label.to_string(), "Task"),
            field("duration", total_secs.to_string(), "Duration"),
            field("duration_cumulated", total_secs.to_string(), "Total"),
        ],
    }
}

/// Build a group bucket's [`NodeSummary`]. Always expandable — a bucket
/// exists only because at least one tracking fell into it.
fn group_summary(scope: &BucketScope, total_secs: i64, active: bool) -> NodeSummary {
    let label = grouping::bucket_display_label(&scope.key, scope.bucket);
    NodeSummary {
        id: format!("{GROUP_ID_PREFIX}{}", scope.encode()),
        label: label.clone(),
        node_type: tracking_tree_group_type(),
        metadata: group_metadata(&label, total_secs, active),
        has_children: Some(true),
    }
}

/// Build the duration-tree projection (A2c Tree view) from the loaded task
/// map and the per-task own seconds. Re-roots orphans (a task whose parent
/// was deleted) to the forest top, then folds each subtree's cumulated total
/// bottom-up. Sibling order follows the task map's iteration; the engine
/// sorts the rendered rows.
fn build_tree_projection(
    task_map: &HashMap<Uuid, (String, Option<Uuid>)>,
    own_secs: &HashMap<Uuid, i64>,
    active_tasks: &HashSet<Uuid>,
) -> TreeProjection {
    let mut children: HashMap<Option<Uuid>, Vec<Uuid>> = HashMap::new();
    for (&id, (_, parent)) in task_map {
        // Re-root a task whose parent is absent (deleted) so it still shows.
        let effective = parent.filter(|p| task_map.contains_key(p));
        children.entry(effective).or_default().push(id);
    }
    // Stable sibling order: by id (the engine re-sorts on display anyway, but
    // a deterministic order keeps tests and snapshots reproducible).
    for kids in children.values_mut() {
        kids.sort();
    }
    // Fold cumulated totals bottom-up, cycle-guarded.
    let mut memo: HashMap<Uuid, i64> = HashMap::new();
    for &id in task_map.keys() {
        let mut stack = HashSet::new();
        cumulated_for(id, &children, own_secs, &mut memo, &mut stack);
    }
    let by_id = task_map
        .iter()
        .map(|(&id, (desc, _))| {
            (
                id,
                TreeTaskRow {
                    description: desc.clone(),
                    own_secs: own_secs.get(&id).copied().unwrap_or(0),
                    cumulated_secs: memo.get(&id).copied().unwrap_or(0),
                    active: active_tasks.contains(&id),
                },
            )
        })
        .collect();
    TreeProjection { by_id, children }
}

/// Recursive subtree total: `own[id]` plus every child's cumulated total.
/// Memoized; the `stack` set guards a corrupt parent cycle (a node already on
/// the path contributes only its own seconds and recursion stops).
fn cumulated_for(
    id: Uuid,
    children: &HashMap<Option<Uuid>, Vec<Uuid>>,
    own: &HashMap<Uuid, i64>,
    memo: &mut HashMap<Uuid, i64>,
    stack: &mut HashSet<Uuid>,
) -> i64 {
    if let Some(&v) = memo.get(&id) {
        return v;
    }
    if !stack.insert(id) {
        return own.get(&id).copied().unwrap_or(0); // cycle guard
    }
    let mut total = own.get(&id).copied().unwrap_or(0);
    if let Some(kids) = children.get(&Some(id)) {
        for &c in kids {
            total += cumulated_for(c, children, own, memo, stack);
        }
    }
    stack.remove(&id);
    memo.insert(id, total);
    total
}

/// Column-backing metadata for a duration-tree task node. `task` is the
/// label; `duration` is the task's **own** tracked seconds and
/// `duration_cumulated` the subtree total — the `tree_aggregate` column in
/// `views/trackings.yaml` reads `duration` by default and toggles
/// (`zt`) to `duration_cumulated`. Both are canonical integer seconds.
fn tree_metadata(id: Uuid, row: &TreeTaskRow) -> Metadata {
    Metadata {
        fields: vec![
            field(
                "marker",
                if row.active { "⏱".to_string() } else { String::new() },
                "Active",
            ),
            field("task", row.description.clone(), "Task"),
            field("duration", row.own_secs.to_string(), "Duration"),
            field(
                "duration_cumulated",
                row.cumulated_secs.to_string(),
                "Total",
            ),
            field("id", id.to_string(), "ID"),
        ],
    }
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
            // An open tracking has no end; the literal "running" matches the
            // native trackings view. The engine's `datetime` column renders
            // unparseable values verbatim, so the word passes through.
            field(
                "ended",
                row.tracking
                    .ended_at
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_else(|| "running".to_string()),
                "Ended",
            ),
            field("duration", duration_seconds(row, now).to_string(), "Duration"),
            field("id", row.tracking.id.to_string(), "ID"),
            // Stable per-task key on the entry row. The Condensed view's
            // per-task condensing now happens adapter-side
            // ([`TrackingSnapshot::condensed_summaries`]) keyed on this id (so
            // identically-named tasks stay distinct), no longer via an
            // engine-side `then_by` over these flat rows.
            field("task_id", row.tracking.task_id.to_string(), "Task ID"),
            // Deleted flag (`"true"`/`""`) — not a visible column, a styling
            // signal: the TUI renders rows whose `deleted` field is `"true"`
            // dimmed. The snapshot loads the full include-deleted universe
            // (#33), so a query that surfaces deleted rows (e.g. dropping
            // `[deleted, =, false]`) shows them greyed-out as context rather
            // than as live entries. Mirrors the Tasks `deleted` signal.
            field(
                "deleted",
                if row.tracking.deleted {
                    "true".to_string()
                } else {
                    String::new()
                },
                "Deleted",
            ),
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

/// Column-backing metadata for a condensed row. `duration` is the **summed**
/// seconds of the `(day, task)` cell (not a single interval). `started` is a
/// representative instant in the day — the engine single-level-groups the rows
/// by it (`group_by: [started/day]`) to render the `── day ──` headers and the
/// per-day `total`. `task`/`taskpath`/`marker` come from the representative.
fn condensed_metadata(
    day_key: &str,
    rep: &TrackingRow,
    total_secs: i64,
    active: bool,
) -> Metadata {
    Metadata {
        fields: vec![
            field(
                "marker",
                if active { "⏱".to_string() } else { String::new() },
                "Active",
            ),
            field("taskpath", canonical_path(&rep.task_path), "Task Path"),
            field("task", rep.task_description.clone(), "Task"),
            // Drives the engine's single-level day grouping; any member's
            // instant works (all fall in the same day as `day_key`).
            field("started", rep.tracking.started_at.to_rfc3339(), "Started"),
            field("duration", total_secs.to_string(), "Duration"),
            field("task_id", rep.tracking.task_id.to_string(), "Task ID"),
            field(
                "id",
                format!("{day_key}:{}", rep.tracking.task_id),
                "ID",
            ),
        ],
    }
}

/// Build a condensed row's [`NodeSummary`]. A leaf (`has_children: false`):
/// the cell is an aggregate, not a drill target. Its id encodes the day +
/// task so [`TrackingCondensedNode::fetch`] can rebuild it for an action.
fn condensed_summary(
    day_key: &str,
    rep: &TrackingRow,
    total_secs: i64,
    active: bool,
) -> NodeSummary {
    NodeSummary {
        id: format!("{CONDENSED_ID_PREFIX}{day_key}:{}", rep.tracking.task_id),
        label: rep.task_description.clone(),
        node_type: tracking_condensed_type(),
        metadata: condensed_metadata(day_key, rep, total_secs, active),
        has_children: Some(false),
    }
}

/// Columns a list of trackings can be sorted on. The adapter applies the sort
/// itself in [`Tracking::list`] (before any grouping, so the within-group item
/// order follows the requested sort) via the generic
/// [`not_yet_done_content::apply_sort`]; each column declares the [`SortKind`]
/// that helper needs to compare its cells correctly.
fn tracking_sortable_columns() -> Vec<SortableColumn> {
    [
        ("marker", "Active", SortKind::Text),
        ("task", "Task", SortKind::Text),
        ("taskpath", "Task path", SortKind::Text),
        ("started", "Started", SortKind::DateTime),
        ("ended", "Ended", SortKind::DateTime),
        ("duration", "Duration", SortKind::Number),
    ]
    .into_iter()
    .map(|(key, label, kind)| SortableColumn {
        key: key.to_string(),
        label: label.to_string(),
        kind,
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
    let parsed = not_yet_done_task_core::filter::query_filter::parse(raw)
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
    vec![
        NodeAction::new("restore-all", "Restore all deleted", InputSpec::None),
        NodeAction::new("backup", "Backup database", InputSpec::None),
    ]
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
        NodeAction::new("toggle-tracking", "track", InputSpec::None)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('s'),
        NodeAction::new("split", "Split", split_input_spec()).with_default_key('i'),
        NodeAction::new("move", "Move", move_input_spec()).with_default_key('m'),
    ]
}

/// Form for `split`: the time point to cut at, plus an optional reassignment of
/// the second part to a different task. Mirrors the CLI `track split <id> <at>
/// [--task <id>]`.
fn split_input_spec() -> InputSpec {
    InputSpec::Form {
        fields: vec![
            FormFieldSpec::text("at", "Split at (e.g. '10:30', 'yesterday 14:00')"),
            FormFieldSpec::text("task", "Reassign 2nd part to task id (optional)").optional(),
        ],
    }
}

/// Form for `move`: the new start time, plus the same gravity / overlap /
/// future guards the CLI `track move` exposes. `gravity` snaps to a boundary
/// and finds the next free slot; `offset` is applied after the snap.
fn move_input_spec() -> InputSpec {
    InputSpec::Form {
        fields: vec![
            FormFieldSpec::text("start", "New start (e.g. 'yesterday 9am', '2026-03-22')"),
            FormFieldSpec::select(
                "gravity",
                "Gravity (snap + next free slot)",
                vec!["start".to_string(), "end".to_string()],
            )
            .optional(),
            FormFieldSpec::text("offset", "Offset after gravity (e.g. +1h, -30min)").optional(),
            FormFieldSpec::toggle("allow_overlap", "Allow overlap with other tasks"),
            FormFieldSpec::toggle("allow_same_task_overlap", "Allow overlap with same task"),
            FormFieldSpec::toggle("allow_future", "Allow moving into the future"),
        ],
    }
}

/// `execute("split")` — cut the tracking at `at` into two parts (the original
/// is soft-deleted, both new parts reference it as predecessor). An optional
/// `task` id reassigns the second part. Delegates to
/// [`TrackingService::split_tracking`]; emits [`DomainEvent::TrackingChanged`]
/// so the snapshot rebuilds. Mirrors the CLI `track split`.
async fn execute_split(
    handle: &CoreHandle,
    tracking_id: Uuid,
    values: &HashMap<String, String>,
) -> Result<ActionOutcome> {
    let at: LocalDateTime = form_required(values, "at")?
        .parse()
        .map_err(invalid_input)?;
    let second_task_id = match form_opt(values, "task") {
        Some(s) => Some(Uuid::parse_str(&s).map_err(|_| invalid_input(format!("invalid task id '{s}'")))?),
        None => None,
    };
    handle
        .tracking_service
        .split_tracking(tracking_id, at.into(), second_task_id)
        .await
        .map_err(to_content_err)?;
    emit_tracking_changed(handle, tracking_id);
    Ok(ActionOutcome::Done {
        message: Some("Tracking split".to_string()),
    })
}

/// `execute("move")` — move the tracking to a new start time, honouring the
/// gravity / overlap / future guards. Granularity is derived from how the user
/// expressed `start` (so a bare date snaps to the day, `9am` to the hour, …),
/// but only when a gravity is set — matching the CLI `track move`. Delegates to
/// [`TrackingService::move_tracking`]; emits [`DomainEvent::TrackingChanged`].
async fn execute_move(
    handle: &CoreHandle,
    tracking_id: Uuid,
    values: &HashMap<String, String>,
) -> Result<ActionOutcome> {
    let start: LocalDateTime = form_required(values, "start")?
        .parse()
        .map_err(invalid_input)?;

    let gravity = match form_opt(values, "gravity").as_deref() {
        Some("start") => Some(GravityDirection::Start),
        Some("end") => Some(GravityDirection::End),
        Some(other) => return Err(invalid_input(format!("invalid gravity '{other}'"))),
        None => None,
    };
    // Granularity only matters when snapping to a boundary, so derive it only
    // when gravity is set (mirrors the CLI).
    let granularity = gravity
        .as_ref()
        .map(|_| Granularity::from_original(&start.original));

    let offset = match form_opt(values, "offset") {
        Some(s) => Some(
            s.parse::<LocalOffset>()
                .map_err(invalid_input)?
                .duration,
        ),
        None => None,
    };

    let options = MoveOptions {
        allow_overlap: form_flag(values, "allow_overlap"),
        allow_same_task_overlap: form_flag(values, "allow_same_task_overlap"),
        allow_future: form_flag(values, "allow_future"),
        gravity,
        granularity,
        offset,
    };

    handle
        .tracking_service
        .move_tracking(tracking_id, start.into(), options)
        .await
        .map_err(to_content_err)?;
    emit_tracking_changed(handle, tracking_id);
    Ok(ActionOutcome::Done {
        message: Some("Tracking moved".to_string()),
    })
}

/// Actions a duration-tree task node (`tracking:tree-item`) exposes. The tree
/// is a read-only aggregate, so the only mutation is `toggle-tracking` —
/// start/stop time tracking on the task under the cursor, reusing the same
/// [`crate::task::apply_tracking`] policy as the other views. (Reload /
/// fuzzy-filter are generic frontend actions in `views/trackings.yaml`.)
fn tracking_tree_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new("toggle-tracking", "track", InputSpec::None)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('s'),
    ]
}

/// Actions a condensed row (`tracking:condensed-row`) exposes. A cell
/// aggregates many intervals of one task, so per-interval `delete`/`restore`
/// make no sense here; the one meaningful mutation is `toggle-tracking` —
/// start/stop tracking on the cell's task, reusing the same policy as the
/// other views. (Reload / fuzzy-filter are generic frontend actions in
/// `views/trackings.yaml`.)
fn tracking_condensed_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new("toggle-tracking", "track", InputSpec::None)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('s'),
    ]
}

/// Announce a non-transition tracking change (delete/restore) so the bridge
/// drops the snapshot and every view (this tab + the task tracking marker)
/// refetches. See [`DomainEvent::TrackingChanged`].
fn emit_tracking_changed(handle: &CoreHandle, tracking_id: Uuid) {
    handle.publish(DomainEvent::TrackingChanged { tracking_id });
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

/// Count the successors of `tracking_id` (the predecessor chain a split/edit
/// produced) without deleting anything — used to tell the user, before they
/// confirm, how many intervals a restore would irreversibly purge. Same BFS
/// walk as [`purge_successors`].
async fn count_successors(handle: &CoreHandle, tracking_id: Uuid) -> std::result::Result<u32, AppError> {
    let mut queue = vec![tracking_id];
    let mut count = 0u32;
    while let Some(id) = queue.pop() {
        for s in handle.tracking_repo.find_by_predecessor(id).await? {
            queue.push(s.id);
            count += 1;
        }
    }
    Ok(count)
}

/// Build the `(y/n)` confirm prompt for a restore that will purge `purged`
/// successor intervals. `subject` is the leading clause (e.g.
/// `"Restore this tracking"` or `"Restore 3 deleted trackings"`).
fn restore_confirm_prompt(subject: &str, purged: u32) -> String {
    if purged == 0 {
        format!("{subject}? (y/n)")
    } else {
        let plural = if purged == 1 { "interval" } else { "intervals" };
        format!("{subject}? Purges {purged} successor {plural} — irreversible. (y/n)")
    }
}

/// `invoke_action("restore")` — undelete a previously soft-deleted tracking
/// (and purge the successors that replaced it). Errors if the target is not
/// deleted. Note: because the list shows only non-deleted rows, a *visible*
/// row is never deletable here — restore is reachable from a future
/// show-deleted sub-view; today it mirrors the native tab's behaviour.
///
/// Two-phase: on the first invocation (`confirmed == false`) it validates the
/// target, counts the successors it would purge, and returns
/// [`ActionDispatch::Confirm`]. The frontend re-invokes with `confirmed ==
/// true`, at which point the purge + undelete actually run.
async fn invoke_restore(handle: &CoreHandle, tracking_id: Uuid, confirmed: bool) -> ActionDispatch {
    let restore = async {
        let tracking = handle
            .tracking_repo
            .find_by_id(tracking_id)
            .await?
            .ok_or(AppError::TrackingNotFound(tracking_id))?;
        if !tracking.deleted {
            return Err(AppError::TrackingNotDeleted(tracking_id));
        }
        if !confirmed {
            let purged = count_successors(handle, tracking_id).await?;
            return Ok(Some(restore_confirm_prompt("Restore this tracking", purged)));
        }
        purge_successors(handle, tracking_id).await?;
        handle.tracking_repo.undelete(tracking_id).await?;
        Ok::<_, AppError>(None)
    };
    match restore.await {
        Ok(Some(prompt)) => ActionDispatch::Confirm { prompt },
        Ok(None) => {
            emit_tracking_changed(handle, tracking_id);
            ActionDispatch::Reload
        }
        Err(e) => ActionDispatch::Error(format!("Restore failed: {e}")),
    }
}

/// `invoke_action("restore-all")` on the root — best-effort restore of every
/// deleted tracking among the candidate ids (non-deleted ones are skipped).
/// Mirrors the native tab, which restores over the currently-loaded rows.
///
/// Two-phase like [`invoke_restore`]: the first invocation counts the deleted
/// candidates and the successor intervals they would purge and returns
/// [`ActionDispatch::Confirm`] (or an error if nothing is deletable); the
/// confirmed re-invocation does the work.
async fn invoke_restore_all(
    handle: &CoreHandle,
    candidates: &[Uuid],
    confirmed: bool,
) -> ActionDispatch {
    let run = async {
        // Phase 1: tally what a restore-all would touch.
        let mut deleted_ids = Vec::new();
        for &id in candidates {
            let Some(tracking) = handle.tracking_repo.find_by_id(id).await? else {
                continue;
            };
            if tracking.deleted {
                deleted_ids.push(id);
            }
        }
        if deleted_ids.is_empty() {
            return Ok(RestoreAll::Nothing);
        }
        if !confirmed {
            let mut purged = 0u32;
            for &id in &deleted_ids {
                purged += count_successors(handle, id).await?;
            }
            let n = deleted_ids.len();
            let subject = format!(
                "Restore {n} deleted tracking{}",
                if n == 1 { "" } else { "s" }
            );
            return Ok(RestoreAll::Confirm(restore_confirm_prompt(&subject, purged)));
        }
        // Phase 2: do the work.
        for &id in &deleted_ids {
            purge_successors(handle, id).await?;
            handle.tracking_repo.undelete(id).await?;
        }
        Ok::<_, AppError>(RestoreAll::Done)
    };
    match run.await {
        Ok(RestoreAll::Nothing) => {
            ActionDispatch::Error("No deleted trackings to restore".to_string())
        }
        Ok(RestoreAll::Confirm(prompt)) => ActionDispatch::Confirm { prompt },
        Ok(RestoreAll::Done) => {
            emit_tracking_changed(handle, Uuid::nil());
            ActionDispatch::Reload
        }
        Err(e) => ActionDispatch::Error(format!("Restore-all failed: {e}")),
    }
}

/// Phase outcome of [`invoke_restore_all`]'s inner async — keeps the
/// match arms readable.
enum RestoreAll {
    Nothing,
    Confirm(String),
    Done,
}

/// Flip time tracking for `task_id` and return the **new** tracked state
/// (`true` = now tracking). Reads the current state live (never from the
/// possibly-stale snapshot) and reuses the task adapter's
/// [`apply_tracking`](crate::task::apply_tracking) so the host's
/// exclusivity policy and `Tracking*` events stay identical across both
/// tabs. The emitted event drives the bridge's in-place row patches (M9).
async fn toggle_tracking(handle: &CoreHandle, task_id: Uuid) -> bool {
    let is_tracked = matches!(
        handle.tracking_repo.find_active_for_task(task_id).await,
        Ok(Some(_))
    );
    let now_tracked = !is_tracked;
    crate::task::apply_tracking(handle, task_id, now_tracked).await;
    now_tracked
}

async fn invoke_toggle_tracking(handle: &CoreHandle, task_id: Uuid) -> ActionDispatch {
    toggle_tracking(handle, task_id).await;
    // Reload the pane: starting a tracking mints a fresh interval (a row
    // that wasn't visible to patch) and may auto-stop another, while a stop
    // freezes a duration and re-folds ancestor aggregates — none of which a
    // single in-place row patch can express. The reload re-reads a fresh
    // snapshot (`root()` → `reload_snapshot`) and, for the duration tree,
    // renews every level in one eager `list_subtree` call (capability
    // `supports_eager_subtree`) — so it no longer triggers the per-node
    // expand cascade that once made a deep-tree reload too costly for `s`.
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
        // The flat/condensed entry rows, the A2c duration-tree task nodes
        // and the grouped tree's bucket nodes all hang off this one root;
        // the loader picks the type matching the active view's `node_type`,
        // and `list` dispatches on it.
        vec![
            tracking_entry_type(),
            tracking_condensed_type(),
            tracking_tree_item_type(),
            tracking_tree_group_type(),
        ]
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
        // The Tree view asks for top-level task projections (grouped into
        // bucket nodes while a `group_by` rides along — the generic
        // `group_by_via_adapter` mechanism); every other view (flat list /
        // condensed) asks for the flat tracking entries. All honor the
        // pane's saved query — the tree re-folds its durations from the
        // visible trackings only.
        if params.node_type.type_id == tracking_tree_group_type().type_id {
            if let Some(spec) = &params.group_by {
                let filter = self.snapshot.visible_set(&self.handle, &params.query).await?;
                return Ok(list_result(
                    self.snapshot.group_summaries(filter.as_deref(), spec),
                ));
            }
            // Grouping cycled off at runtime: the same root request serves
            // the plain task tree — the frontend's type-based chain
            // resolution matches the recursive tree-item level from depth 0.
            return Ok(list_result(
                self.snapshot
                    .scoped_projection(&self.handle, None, &params.query)
                    .await?
                    .child_summaries(None, None),
            ));
        }
        if params.node_type.type_id == tracking_tree_item_type().type_id {
            return Ok(list_result(
                self.snapshot
                    .scoped_projection(&self.handle, None, &params.query)
                    .await?
                    .child_summaries(None, None),
            ));
        }
        let filter = self.snapshot.visible_set(&self.handle, &params.query).await?;
        let now = chrono::Utc::now();
        // Condensed view: the adapter collapses the day's intervals of each
        // task into one summed row and sorts those rows (`S`); the engine then
        // single-level-groups them into day headers.
        if params.node_type.type_id == tracking_condensed_type().type_id {
            let (items, applied) =
                self.snapshot
                    .condensed_summaries(filter.as_deref(), now, &params.sort);
            return Ok(list_result_with_sort(items, applied));
        }
        // Flat list: apply the requested item sort here (before any
        // engine-side grouping, whose group bucketing is stable, so the
        // within-group order follows this sort). `S` drives `params.sort`.
        let mut items = self.snapshot.entries(filter.as_deref(), now);
        let applied = apply_sort(&mut items, &params.sort, &tracking_sortable_columns());
        Ok(list_result_with_sort(items, applied))
    }
    async fn list_subtree(
        &self,
        params: not_yet_done_content::ListParams,
        depth: u32,
    ) -> Result<Subtree> {
        // Mirrors `list`'s view dispatch, but expands the whole tree in one
        // pass (capability `supports_eager_subtree`).
        if params.node_type.type_id == tracking_tree_group_type().type_id {
            if let Some(spec) = &params.group_by {
                let filter = self.snapshot.visible_set(&self.handle, &params.query).await?;
                return self
                    .snapshot
                    .group_subtree(&self.handle, filter.as_deref(), spec, &params.query, depth)
                    .await;
            }
            // Grouping cycled off: the plain task forest, same as `list`.
            return Ok(self
                .snapshot
                .scoped_projection(&self.handle, None, &params.query)
                .await?
                .subtree(None, None, depth));
        }
        if params.node_type.type_id == tracking_tree_item_type().type_id {
            return Ok(self
                .snapshot
                .scoped_projection(&self.handle, None, &params.query)
                .await?
                .subtree(None, None, depth));
        }
        // Flat / condensed entry views aren't trees: one level of leaves,
        // exactly what `list` returns (depth is irrelevant for leaf rows).
        let result = self.list(params).await?;
        Ok(Subtree {
            items: result
                .items
                .into_iter()
                .map(|summary| SubtreeNode {
                    summary,
                    children: Subtree::default(),
                })
                .collect(),
            page: result.page,
        })
    }
    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        if id.starts_with(GROUP_ID_PREFIX) {
            return TrackingGroupNode::fetch(&self.snapshot, &self.handle, id);
        }
        if id.starts_with(TREE_ID_PREFIX) {
            return TrackingTreeNode::fetch(&self.snapshot, &self.handle, id);
        }
        if id.starts_with(CONDENSED_ID_PREFIX) {
            return TrackingCondensedNode::fetch(&self.snapshot, &self.handle, id);
        }
        TrackingEntryNode::fetch(&self.snapshot, &self.handle, id)
    }
    async fn invoke_action(&self, name: &str, ctx: &ActionContext) -> Result<ActionDispatch> {
        Ok(match name {
            "restore-all" => {
                // Scope to the pane's *active query* — the visible set — so
                // restore-all never reaches beyond what the current filter
                // shows. To restore deleted trackings, the query must select
                // them (the query is the sole filter; `deleted=false` is not
                // baked in). With no query, the whole list is in scope, which
                // is exactly what the pane shows.
                let candidates: Vec<Uuid> = match resolve_visible_set(&self.handle, &ctx.query).await
                {
                    Ok(Some(set)) => set.into_iter().collect(),
                    Ok(None) => self.snapshot.order.clone(),
                    Err(e) => return Ok(ActionDispatch::Error(format!("Restore-all failed: {e}"))),
                };
                invoke_restore_all(&self.handle, &candidates, ctx.confirmed).await
            }
            "backup" => crate::invoke_backup(&self.handle).await,
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

    /// Whether this row is already soft-deleted — read from the `deleted`
    /// styling field the snapshot stamped (see [`entry_metadata`]). Lets the
    /// `delete` action short-circuit with an "Already deleted" notice instead
    /// of running the generic confirm flow for a no-op re-delete.
    fn is_deleted(&self) -> bool {
        self.metadata
            .fields
            .iter()
            .any(|f| f.key == "deleted" && f.value == "true")
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
    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        match (action_id, input) {
            // Reached via the generic `DeleteSelf` confirm flow, which calls
            // `execute("delete")` after the user confirms.
            ("delete", _) => execute_delete(&self.handle, self.tracking_id()?).await,
            ("split", ActionInput::Form(values)) => {
                execute_split(&self.handle, self.tracking_id()?, &values).await
            }
            ("move", ActionInput::Form(values)) => {
                execute_move(&self.handle, self.tracking_id()?, &values).await
            }
            (other, _) => Err(ContentError::NotSupported(format!(
                "action `{other}` not supported on a tracking"
            ))),
        }
    }
    async fn invoke_action(&self, name: &str, ctx: &ActionContext) -> Result<ActionDispatch> {
        Ok(match name {
            // Routed to the generic delete-confirm flow; the actual delete
            // happens in `execute("delete")` after confirmation. A row that
            // the query already surfaces as deleted has nothing to delete —
            // short-circuit with a neutral notice rather than asking the user
            // to confirm a no-op re-delete.
            "delete" if self.is_deleted() => ActionDispatch::Error("Already deleted".to_string()),
            "delete" => ActionDispatch::DeleteSelf { confirm: None },
            "restore" => match self.tracking_id() {
                Ok(id) => invoke_restore(&self.handle, id, ctx.confirmed).await,
                Err(_) => ActionDispatch::Error("Invalid tracking id".to_string()),
            },
            "toggle-tracking" => invoke_toggle_tracking(&self.handle, self.task_id).await,
            _ => ActionDispatch::Noop,
        })
    }
}

/// A condensed row (A2c Condensed view): the `(day, task)` aggregate the
/// cursor sits on. A leaf — no children. Carries the `task_id` so
/// `toggle-tracking` can start/stop tracking on the cell's task. Rebuilt from
/// its `cond:<day>:<task>` id by re-summing that day's intervals of the task.
struct TrackingCondensedNode {
    id_str: String,
    label: String,
    node_type: NodeType,
    metadata: Metadata,
    handle: CoreHandle,
    task_id: Uuid,
}

impl TrackingCondensedNode {
    /// Rebuild a condensed node from its `cond:<day-key>:<task-uuid>` id by
    /// re-summing the snapshot's intervals of that task on that day. The day
    /// key has no `:`, so a single `rsplit_once` recovers the task uuid.
    fn fetch(
        snapshot: &Arc<TrackingSnapshot>,
        handle: &CoreHandle,
        id: &str,
    ) -> Result<Box<dyn Node>> {
        let not_found = || ContentError::NotFound(id.to_string());
        let rest = id.strip_prefix(CONDENSED_ID_PREFIX).ok_or_else(not_found)?;
        let (day_key, task_str) = rest.rsplit_once(':').ok_or_else(not_found)?;
        let task_id = Uuid::parse_str(task_str).map_err(|_| not_found())?;
        let now = chrono::Utc::now();
        let mut total_secs = 0i64;
        let mut active = false;
        let mut rep: Option<&TrackingRow> = None;
        for tid in &snapshot.order {
            let Some(row) = snapshot.by_id.get(tid) else {
                continue;
            };
            if row.tracking.task_id != task_id {
                continue;
            }
            if grouping::group_key(
                &row.tracking.started_at.to_rfc3339(),
                Some(GroupBucket::Day),
            ) != day_key
            {
                continue;
            }
            total_secs += duration_seconds(row, now);
            active |= row.active;
            // `order` is newest-first, so the first match is the newest
            // interval — the same representative `condensed_summaries` picks.
            if rep.is_none() {
                rep = Some(row);
            }
        }
        let rep = rep.ok_or_else(not_found)?;
        Ok(Box::new(TrackingCondensedNode {
            id_str: id.to_string(),
            label: rep.task_description.clone(),
            node_type: tracking_condensed_type(),
            metadata: condensed_metadata(day_key, rep, total_secs, active),
            handle: handle.clone(),
            task_id,
        }))
    }
}

#[async_trait]
impl Node for TrackingCondensedNode {
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
        tracking_condensed_actions()
    }
    async fn invoke_action(&self, name: &str, _ctx: &ActionContext) -> Result<ActionDispatch> {
        Ok(match name {
            "toggle-tracking" => invoke_toggle_tracking(&self.handle, self.task_id).await,
            _ => ActionDispatch::Noop,
        })
    }
}

/// A group bucket in the grouped tree (generic `group_by_via_adapter`): one
/// day/week/month/year (or verbatim value) of trackings. Listing it re-folds
/// the task tree from this bucket's trackings only (intersected with the
/// pane's saved query, which the engine propagates into subtree loads).
/// Read-only — a bucket is an aggregate, not a thing to act on.
struct TrackingGroupNode {
    snapshot: Arc<TrackingSnapshot>,
    handle: CoreHandle,
    scope: BucketScope,
    id_str: String,
    label: String,
    node_type: NodeType,
    metadata: Metadata,
}

impl TrackingGroupNode {
    /// Rebuild a group node from its `treegrp:…` id. No query context here —
    /// a direct fetch sees the unfiltered bucket; the pane's query arrives
    /// on [`Node::list`] and is intersected there.
    fn fetch(
        snapshot: &Arc<TrackingSnapshot>,
        handle: &CoreHandle,
        id: &str,
    ) -> Result<Box<dyn Node>> {
        let scope = id
            .strip_prefix(GROUP_ID_PREFIX)
            .and_then(BucketScope::parse)
            .ok_or_else(|| ContentError::NotFound(id.to_string()))?;
        let now = snapshot.built_at;
        let mut total_secs = 0;
        let mut active = false;
        for row in snapshot.by_id.values() {
            if scope_member(row, &scope) {
                total_secs += duration_seconds(row, now);
                active |= row.active;
            }
        }
        let label = grouping::bucket_display_label(&scope.key, scope.bucket);
        Ok(Box::new(TrackingGroupNode {
            snapshot: snapshot.clone(),
            handle: handle.clone(),
            metadata: group_metadata(&label, total_secs, active),
            scope,
            id_str: id.to_string(),
            label,
            node_type: tracking_tree_group_type(),
        }))
    }
}

#[async_trait]
impl Node for TrackingGroupNode {
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
        vec![tracking_tree_item_type()]
    }
    async fn list(
        &self,
        params: not_yet_done_content::ListParams,
    ) -> Result<not_yet_done_content::ListResult> {
        Ok(list_result(
            self.snapshot
                .scoped_projection(&self.handle, Some(&self.scope), &params.query)
                .await?
                .child_summaries(None, Some(&self.scope)),
        ))
    }
    async fn list_subtree(
        &self,
        params: not_yet_done_content::ListParams,
        depth: u32,
    ) -> Result<Subtree> {
        Ok(self
            .snapshot
            .scoped_projection(&self.handle, Some(&self.scope), &params.query)
            .await?
            .subtree(None, Some(&self.scope), depth))
    }
    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        TrackingTreeNode::fetch(&self.snapshot, &self.handle, id)
    }
}

/// A task node in the duration tree (A2c Tree view). Drilling lists its child
/// tasks (also `tracking:tree-item`); each carries its own + subtree-cumulated
/// tracked time. Holds the shared snapshot for in-memory drilling and the
/// `task_id` so `toggle-tracking` can flip tracking on the task. Inside a
/// group bucket the node additionally carries its [`BucketScope`] (parsed
/// back out of the id), and every duration re-folds from that bucket's
/// trackings only.
struct TrackingTreeNode {
    snapshot: Arc<TrackingSnapshot>,
    handle: CoreHandle,
    task_id: Uuid,
    /// `Some` when this node lives under a group bucket.
    scope: Option<BucketScope>,
    id_str: String,
    label: String,
    node_type: NodeType,
    metadata: Metadata,
}

impl TrackingTreeNode {
    /// Look up the `tree:[<scope>:]<uuid>` node `id` in the (bucket-scoped)
    /// projection, or `NotFound`.
    fn fetch(
        snapshot: &Arc<TrackingSnapshot>,
        handle: &CoreHandle,
        id: &str,
    ) -> Result<Box<dyn Node>> {
        let (scope, task_id) = parse_tree_id(id)?;
        let projection = snapshot.unfiltered_projection(scope.as_ref());
        let row = projection
            .by_id
            .get(&task_id)
            .filter(|_| projection.is_visible(task_id))
            .ok_or_else(|| ContentError::NotFound(id.to_string()))?;
        Ok(Box::new(TrackingTreeNode {
            snapshot: snapshot.clone(),
            handle: handle.clone(),
            task_id,
            id_str: id.to_string(),
            label: row.description.clone(),
            node_type: tracking_tree_item_type(),
            metadata: tree_metadata(task_id, row),
            scope,
        }))
    }
}

/// Parse a `tree:` node id back into its optional [`BucketScope`] and the
/// task UUID: `tree:<uuid>` (ungrouped) or `tree:<column>:<gran>:<key>:<uuid>`
/// (inside a group bucket).
fn parse_tree_id(id: &str) -> Result<(Option<BucketScope>, Uuid)> {
    let not_found = || ContentError::NotFound(id.to_string());
    let raw = id.strip_prefix(TREE_ID_PREFIX).ok_or_else(not_found)?;
    if let Ok(uuid) = Uuid::parse_str(raw) {
        return Ok((None, uuid));
    }
    let (scope_str, uuid_str) = raw.rsplit_once(':').ok_or_else(not_found)?;
    let uuid = Uuid::parse_str(uuid_str).map_err(|_| not_found())?;
    let scope = BucketScope::parse(scope_str).ok_or_else(not_found)?;
    Ok((Some(scope), uuid))
}

#[async_trait]
impl Node for TrackingTreeNode {
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
        vec![tracking_tree_item_type()]
    }
    fn actions(&self) -> Vec<NodeAction> {
        tracking_tree_actions()
    }
    async fn list(
        &self,
        params: not_yet_done_content::ListParams,
    ) -> Result<not_yet_done_content::ListResult> {
        // The engine propagates the pane's query into subtree loads
        // (capability `propagates_query_to_subtree`), so an expanded
        // branch shows the same filtered durations as the root level.
        // Under a group bucket the query intersects the bucket's set.
        Ok(list_result(
            self.snapshot
                .scoped_projection(&self.handle, self.scope.as_ref(), &params.query)
                .await?
                .child_summaries(Some(self.task_id), self.scope.as_ref()),
        ))
    }
    async fn list_subtree(
        &self,
        params: not_yet_done_content::ListParams,
        depth: u32,
    ) -> Result<Subtree> {
        // Same scope + query semantics as `list`, but walk the whole subtree
        // in one in-memory pass instead of a single level (capability
        // `supports_eager_subtree`).
        Ok(self
            .snapshot
            .scoped_projection(&self.handle, self.scope.as_ref(), &params.query)
            .await?
            .subtree(Some(self.task_id), self.scope.as_ref(), depth))
    }
    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        TrackingTreeNode::fetch(&self.snapshot, &self.handle, id)
    }
    async fn invoke_action(&self, name: &str, _ctx: &ActionContext) -> Result<ActionDispatch> {
        Ok(match name {
            // Toggle, then let the snapshot reload + `NowAnchored` the bridge
            // emits drive the refresh: the frontend reloads only the bucket
            // `bucket_for_now` resolves (the one this task's interval lands
            // in), not the whole grouped forest. Returning `Reload` here would
            // rebuild every bucket — exactly the cost the now-bucket path
            // avoids. The flat/condensed entry node keeps `Reload`
            // (`invoke_toggle_tracking`); they have no per-bucket fold to
            // localise, so a full pane reload is already cheap there.
            "toggle-tracking" => {
                toggle_tracking(&self.handle, self.task_id).await;
                ActionDispatch::Noop
            }
            _ => ActionDispatch::Noop,
        })
    }
}

/// Wrap a summary list into a `ListResult` with no applied sort (tree/grouped
/// paths, whose order is structural).
fn list_result(items: Vec<NodeSummary>) -> not_yet_done_content::ListResult {
    list_result_with_sort(items, Vec::new())
}

/// Wrap a summary list into a `ListResult`, reporting which sort keys the
/// adapter applied (so the footer can surface the active sort).
fn list_result_with_sort(
    items: Vec<NodeSummary>,
    applied_sort: Vec<not_yet_done_content::SortKey>,
) -> not_yet_done_content::ListResult {
    not_yet_done_content::ListResult {
        items,
        applied_sort,
        page: None,
        batch_download_available: false,
        downloaded: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Event bridge
// ---------------------------------------------------------------------------

/// Tell the frontend whether (and how fast) to run the live-row pull, and
/// record the announced cadence in `last_live_secs`. Free function so both
/// [`TrackingAdapter::announce_interval`] and [`spawn_tracking_bridge`]
/// re-pace the timer through the *same* shared atomic — a start/stop
/// handled in the bridge updates the bracket the next `live_rows` tick
/// compares against, so it never emits a redundant re-announce.
fn announce_live_interval(
    inv_tx: &broadcast::Sender<Invalidation>,
    last_live_secs: &std::sync::atomic::AtomicU64,
    snapshot: &TrackingSnapshot,
) {
    let interval = snapshot
        .shortest_active_secs(chrono::Utc::now())
        .map(live_interval_for);
    last_live_secs.store(
        interval.map(|d| d.as_secs()).unwrap_or(0),
        std::sync::atomic::Ordering::Relaxed,
    );
    let _ = inv_tx.send(Invalidation::RefreshInterval(interval));
}

/// Bridge the core domain-event bus into this adapter's invalidation stream.
///
/// - `TrackingStarted`/`Stopped` → a tracking's running marker flips and (on
///   a stop) its duration freezes, but the task forest's *shape* is
///   unchanged. Reload the in-memory snapshot (so `live_rows` and a later
///   `r` are fresh), re-pace the live timer, **patch the affected entry row
///   in place** (M9, [`publish_row_patches`] — the flat list / cross-tab
///   marker) instead of a coarse [`Invalidation::All`], and emit
///   [`Invalidation::NowAnchored`] so a *grouped tree* pane reloads only the
///   now-bucket (its totals shifted and a start may add a row — neither a row
///   patch can express, but a whole-forest rebuild is what we're avoiding).
///   A deep, fully-expanded tree must not rebuild on every `s`.
/// - `TaskChanged`/`TrackingChanged` → genuinely structural (task edited, or
///   a tracking deleted/restored) → clear + [`Invalidation::All`].
/// - `TrackingTick` is ignored: the per-second duration tick is driven by
///   the M9 live-row pull, not this global heartbeat (the heartbeat still
///   serves the native tab until the C1 cutover).
fn spawn_tracking_bridge(
    mut events: broadcast::Receiver<HostEvent>,
    inv_tx: broadcast::Sender<Invalidation>,
    snapshot: Arc<RwLock<Option<Arc<TrackingSnapshot>>>>,
    handle: CoreHandle,
    last_live_secs: Arc<std::sync::atomic::AtomicU64>,
) {
    tokio::spawn(async move {
        use broadcast::error::RecvError;
        loop {
            let ev = match events.recv().await {
                // Opaque host payload → the DomainEvent the local adapters
                // privately exchange on this channel; foreign payloads skipped.
                Ok(payload) => match as_domain_event(&payload) {
                    Some(ev) => ev,
                    None => continue,
                },
                Err(RecvError::Lagged(_)) => {
                    *snapshot.write().await = None;
                    let _ = inv_tx.send(Invalidation::All);
                    continue;
                }
                Err(RecvError::Closed) => break,
            };
            match ev {
                DomainEvent::TrackingTick => {}
                DomainEvent::TrackingStarted { tracking_id, .. }
                | DomainEvent::TrackingStopped { tracking_id, .. } => {
                    match TrackingSnapshot::load(&handle).await {
                        Ok(snap) => {
                            *snapshot.write().await = Some(snap.clone());
                            announce_live_interval(&inv_tx, &last_live_secs, &snap);
                            if let Some(row) = snap.by_id.get(&tracking_id) {
                                publish_row_patches(
                                    &inv_tx,
                                    [entry_summary(tracking_id, row, chrono::Utc::now())],
                                );
                            }
                            // A grouped tree can't be expressed by a single
                            // row patch — its bucket totals shift and (on a
                            // start) a row may appear. Signal the now-bucket
                            // refresh; the frontend reloads only the bucket
                            // `bucket_for_now` resolves for each grouped pane,
                            // not the whole forest. Sent *after* the snapshot
                            // write so `bucket_for_now` reads the post-toggle
                            // state.
                            let _ = inv_tx.send(Invalidation::NowAnchored);
                        }
                        // Couldn't refresh in place — fall back to a full reload.
                        Err(_) => {
                            *snapshot.write().await = None;
                            let _ = inv_tx.send(Invalidation::All);
                        }
                    }
                }
                DomainEvent::TaskChanged { .. }
                | DomainEvent::TrackingChanged { .. }
                // A project rename/cascade-delete can change a task's path or
                // remove tracked tasks, so resync the whole forest.
                | DomainEvent::ProjectChanged { .. } => {
                    *snapshot.write().await = None;
                    let _ = inv_tx.send(Invalidation::All);
                }
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Adapter + factory
// ---------------------------------------------------------------------------

/// Builds self-contained [`TrackingAdapter`] instances. Stateless: each
/// `create` opens its own database from the tab's `config`
/// (see [`crate::open_core_handle`] / [`crate::LocalAdapterConfig`]) and wires
/// the resulting [`CoreHandle`] to the host bus from [`HostContext`].
#[derive(Default)]
pub struct TrackingAdapterFactory;

impl TrackingAdapterFactory {
    pub fn new() -> Self {
        Self
    }
}

impl AdapterFactory for TrackingAdapterFactory {
    fn adapter_type(&self) -> &str {
        "trackings"
    }

    fn create(
        &self,
        instance_id: &str,
        config: &str,
        ctx: &HostContext,
    ) -> Result<Box<dyn ContentAdapter>> {
        let handle = crate::open_core_handle(config, ctx)?;
        Ok(Box::new(TrackingAdapter::new(instance_id, handle)))
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
    /// The cadence last announced via [`Invalidation::RefreshInterval`],
    /// in seconds (`0` = stopped). [`ContentAdapter::live_rows`] compares
    /// the adaptive target against this on every tick and re-paces the
    /// frontend timer only when the bracket actually changes. Shared
    /// (`Arc`) with [`spawn_tracking_bridge`] so a start/stop re-paces the
    /// timer through the same atomic without an extra redundant announce.
    last_live_secs: Arc<std::sync::atomic::AtomicU64>,
}

impl TrackingAdapter {
    /// Build an adapter over an already-opened [`CoreHandle`]: set up the
    /// invalidation broadcast, the live-cadence atomic, spawn the
    /// domain-event → invalidation bridge, and resolve the per-instance
    /// saved-query root. The factory uses this after [`crate::open_core_handle`];
    /// tests use it over a handle built on their own in-memory database.
    pub(crate) fn new(instance_id: &str, handle: CoreHandle) -> Self {
        let (inv_tx, _) = broadcast::channel(64);
        let snapshot: Arc<RwLock<Option<Arc<TrackingSnapshot>>>> = Arc::new(RwLock::new(None));
        let last_live_secs = Arc::new(std::sync::atomic::AtomicU64::new(0));
        spawn_tracking_bridge(
            handle.subscribe(),
            inv_tx.clone(),
            snapshot.clone(),
            handle.clone(),
            last_live_secs.clone(),
        );
        let queries_root = dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("not_yet_done")
            .join("trackings")
            .join(instance_id)
            .join("queries");
        Self {
            instance_id: instance_id.to_string(),
            handle,
            inv_tx,
            snapshot,
            saved_queries: FsSavedQueryStore::new(queries_root),
            last_live_secs,
        }
    }

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

    /// Tell the frontend whether (and how fast) to run the live-row pull:
    /// the adaptive [`live_interval_for`] cadence while a tracking is
    /// running, `None` when none is (M9). Emitted on every (re)load so a
    /// tracking that started/stopped in another tab re-paces the timer
    /// after the reload its event triggered; `live_rows` keeps re-pacing
    /// as the tracking ages and the bracket slows down.
    fn announce_interval(&self, snapshot: &TrackingSnapshot) {
        announce_live_interval(&self.inv_tx, &self.last_live_secs, snapshot);
    }
}

#[async_trait]
impl ContentAdapter for TrackingAdapter {
    fn adapter_type(&self) -> &str {
        "trackings"
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Fires `connected` after construction (every program start for this
    /// in-process adapter). The tracking root also exposes the `backup` action,
    /// so a `connected`→`backup` binding works here too.
    fn hooks(&self) -> Vec<&str> {
        vec!["connected"]
    }

    /// Anonymize the task name a tracking carries (`label` / `task` / each
    /// `taskpath` segment) with the same lookup the tasks adapter uses, so a
    /// tracking shows the pseudo-name its task carries. See [`crate::anonymize`].
    fn anonymizer(&self) -> std::sync::Arc<dyn not_yet_done_content::Anonymizer> {
        std::sync::Arc::new(crate::anonymize::LocalAnonymizer::tracking())
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities {
            // A2b: delete (soft) + restore. No create (trackings are born
            // from the task toggle, not authored here) and no reparent.
            supports_delete: true,
            supports_search: true,
            // A2c Tree: the adapter supplies each task node's own +
            // subtree-cumulated duration, so the engine can render a
            // `tree_aggregate` column that toggles between the two (M4).
            supports_tree_aggregation: true,
            // The duration tree filters by the pane's saved query at every
            // depth (root + expanded branches re-fold from the visible
            // trackings), so subtree loads must carry the query along.
            propagates_query_to_subtree: true,
            // Grouped tree: the engine hands the pane's active `group_by`
            // to the root `list()` and this adapter returns one bucket node
            // per group, each with a per-bucket re-folded subtree (the fold
            // the engine can't do itself). `zg`/`u` regroup via reload.
            group_by_via_adapter: true,
            // The whole tracking forest is in memory, so the duration tree
            // (and its grouped variants) builds its entire expanded shape in
            // one `list_subtree` projection walk — the engine skips the
            // per-node expand cascade for it (see the `list_subtree` impls).
            supports_eager_subtree: true,
            ..AdapterCapabilities::default()
        }
    }

    /// A running tracking is one whose snapshot row is still `active`
    /// (`ended_at IS NULL`). Best-effort, non-blocking: a contended lock or
    /// unloaded snapshot reads as "none".
    fn has_active_tracking(&self) -> bool {
        self.snapshot
            .try_read()
            .ok()
            .and_then(|guard| {
                guard
                    .as_ref()
                    .map(|snap| snap.by_id.values().any(|row| row.active))
            })
            .unwrap_or(false)
    }

    fn actions_for_type(&self, node_type: &NodeType) -> Vec<NodeAction> {
        match node_type.type_id.as_str() {
            "tracking:root" => tracking_root_actions(),
            "tracking:entry" => tracking_entry_actions(),
            "tracking:condensed-row" => tracking_condensed_actions(),
            "tracking:tree-item" => tracking_tree_actions(),
            // A group bucket is a read-only aggregate — nothing to act on.
            "tracking:tree-group" => Vec::new(),
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
        // `treegrp:…` addresses a grouped-tree bucket node.
        if id.starts_with(GROUP_ID_PREFIX) {
            return TrackingGroupNode::fetch(&snapshot, &self.handle, id);
        }
        // `tree:…` addresses a duration-tree task node (A2c Tree view),
        // optionally bucket-scoped.
        if id.starts_with(TREE_ID_PREFIX) {
            return TrackingTreeNode::fetch(&snapshot, &self.handle, id);
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
        // Adaptive cadence: as the youngest tracking ages into a slower
        // bracket (5 s → 10 s → 30 s → 60 s, see `live_interval_for`),
        // re-pace the frontend timer. Compared against the last announced
        // value so a steady state sends nothing.
        let target = snapshot.shortest_active_secs(now).map(live_interval_for);
        let target_secs = target.map(|d| d.as_secs()).unwrap_or(0);
        if self
            .last_live_secs
            .swap(target_secs, std::sync::atomic::Ordering::Relaxed)
            != target_secs
        {
            let _ = self.inv_tx.send(Invalidation::RefreshInterval(target));
        }
        snapshot
            .order
            .iter()
            .filter_map(|id| {
                let row = snapshot.by_id.get(id)?;
                row.active.then(|| entry_summary(*id, row, now))
            })
            .collect()
    }

    async fn bucket_for_now(&self, group_by: &GroupSpec) -> Option<String> {
        // The bridge has already reloaded the snapshot for the start/stop that
        // triggered the `NowAnchored`, so this reads the post-toggle state.
        self.snapshot().await.ok()?.bucket_for_now(group_by)
    }

    async fn live_group_rows(
        &self,
        group_by: &GroupSpec,
        query: Option<&str>,
    ) -> Vec<NodeSummary> {
        let Ok(snapshot) = self.snapshot().await else {
            return Vec::new();
        };
        // Honour the pane's saved query so the ticked rows match the filtered
        // tree the user sees; `None`/empty → whole bucket.
        let query = query.map(str::to_string);
        let filter = snapshot.visible_set(&self.handle, &query).await.ok().flatten();
        snapshot.live_group_rows(group_by, filter.as_deref(), chrono::Utc::now())
    }

    async fn revalidate(&self) {
        // Out-of-process changes (CLI, waybar, another instance) write to
        // the same DB but emit no in-process DomainEvent, so the eager
        // snapshot can go stale without the bridge noticing. Diff the
        // running-tracking set against the live DB; on drift drop the
        // snapshot and reload everything (the reload also re-announces the
        // live cadence, so an externally started tracking starts ticking
        // and an externally stopped one stops).
        let snap_active = match self.snapshot.read().await.as_ref() {
            Some(snap) => snap.active_ids(),
            // No snapshot — the next load is fresh anyway.
            None => return,
        };
        let Ok(active) = self.handle.tracking_repo.find_all_active().await else {
            return;
        };
        let db_active: HashSet<Uuid> = active.iter().map(|t| t.id).collect();
        if db_active != snap_active {
            *self.snapshot.write().await = None;
            let _ = self.inv_tx.send(Invalidation::All);
        }
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

    /// A soft-deleted clone of [`tracking`] — its row stays in the snapshot
    /// universe but must never count as running (adapter contract).
    fn deleted_tracking(id: Uuid, task_id: Uuid, started_min_ago: i64, ended: bool) -> tracking::Model {
        tracking::Model {
            deleted: true,
            ..tracking(id, task_id, started_min_ago, ended)
        }
    }

    fn row(model: tracking::Model, desc: &str, path: Vec<&str>) -> TrackingRow {
        // Mirror `TrackingSnapshot::load`: a deleted tracking is never active.
        let active = model.ended_at.is_none() && !model.deleted;
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
        Arc::new(TrackingSnapshot {
            by_id,
            order,
            tree: Arc::new(TreeProjection::default()),
            task_map: HashMap::new(),
            built_at: chrono::Utc::now(),
            visible_cache: Default::default(),
            fold_cache: Default::default(),
        })
    }

    #[test]
    fn tree_summary_id_is_scope_encoded() {
        // An ungrouped duration-tree row carries a `tree:<uuid>` id; the
        // adapter's `get_by_id` routes on that prefix, so the shape is part
        // of the contract. (A bucket-scoped variant prepends the scope.)
        let task = Uuid::from_u128(42);
        let mut by_id = HashMap::new();
        by_id.insert(
            task,
            TreeTaskRow {
                description: "Write report".to_string(),
                own_secs: 600,
                cumulated_secs: 600,
                active: false,
            },
        );
        let mut children = HashMap::new();
        children.insert(None, vec![task]);
        let projection = TreeProjection { by_id, children };

        let row = projection.by_id.get(&task).unwrap();
        let summary = projection.summary(task, row, None);
        assert_eq!(summary.id, format!("tree:{task}"));
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
    fn tree_for_refolds_projection_from_visible_trackings_only() {
        // Task forest A → B; one 30-min tracking on each. Filtering to
        // B's tracking must re-fold the tree: A stays visible as B's
        // ancestor but carries no own time, and the cumulated total is
        // B's alone.
        let task_a = Uuid::from_u128(10);
        let task_b = Uuid::from_u128(20);
        let t1 = Uuid::from_u128(1);
        let t2 = Uuid::from_u128(2);
        let task_map: HashMap<Uuid, (String, Option<Uuid>)> = [
            (task_a, ("A".to_string(), None)),
            (task_b, ("B".to_string(), Some(task_a))),
        ]
        .into_iter()
        .collect();
        let mut by_id = HashMap::new();
        by_id.insert(t1, row(tracking(t1, task_a, 120, true), "A", vec![]));
        by_id.insert(t2, row(tracking(t2, task_b, 90, true), "B", vec!["A"]));
        let snapshot = TrackingSnapshot {
            by_id,
            order: vec![t2, t1],
            tree: Arc::new(TreeProjection::default()),
            task_map,
            built_at: chrono::Utc::now(),
            visible_cache: Default::default(),
            fold_cache: Default::default(),
        };

        // No filter → the prebuilt projection is reused untouched.
        assert!(matches!(
            snapshot.tree_for(None),
            std::borrow::Cow::Borrowed(_)
        ));

        let visible: HashSet<Uuid> = [t2].into_iter().collect();
        let tree = snapshot.tree_for(Some(&visible));
        let roots = tree.child_summaries(None, None);
        assert_eq!(roots.len(), 1, "{roots:?}");
        assert_eq!(roots[0].label, "A");
        let a_row = tree.by_id.get(&task_a).unwrap();
        assert_eq!(a_row.own_secs, 0, "A's own tracking is filtered out");
        assert_eq!(a_row.cumulated_secs, 1800, "subtree total = B's 30 min");
        let kids = tree.child_summaries(Some(task_a), None);
        assert_eq!(kids.len(), 1, "{kids:?}");
        assert_eq!(kids[0].label, "B");
    }

    #[test]
    fn canonical_path_formats_leading_slash() {
        assert_eq!(canonical_path(&[]), "");
        assert_eq!(
            canonical_path(&["a".to_string(), "b".to_string()]),
            "/a/b"
        );
    }

    /// A completed tracking starting at local noon on `date` (timezone-stable
    /// day bucketing) and running `dur_min` minutes.
    fn model_on(id: Uuid, task_id: Uuid, date: (i32, u32, u32), dur_min: i64) -> tracking::Model {
        use chrono::TimeZone;
        let started = chrono::Local
            .with_ymd_and_hms(date.0, date.1, date.2, 12, 0, 0)
            .unwrap()
            .with_timezone(&chrono::Utc);
        tracking::Model {
            id,
            task_id,
            predecessor_id: None,
            started_at: started,
            ended_at: Some(started + chrono::Duration::minutes(dur_min)),
            deleted: false,
            created_at: started,
        }
    }

    #[test]
    fn condensed_collapses_day_task_cells_summing_duration() {
        // Task A worked twice on day 1 (30 + 30 min) and once on day 2
        // (10 min); task B once on day 1 (90 min). The two A-on-day-1
        // intervals must collapse into one row of 3600 s; the other cells
        // stay distinct (different day or different task).
        let a = Uuid::from_u128(10);
        let b = Uuid::from_u128(20);
        let snapshot = snapshot_from(vec![
            (Uuid::from_u128(1), row(model_on(Uuid::from_u128(1), a, (2026, 6, 9), 30), "A", vec![])),
            (Uuid::from_u128(2), row(model_on(Uuid::from_u128(2), a, (2026, 6, 9), 30), "A", vec![])),
            (Uuid::from_u128(3), row(model_on(Uuid::from_u128(3), b, (2026, 6, 9), 90), "B", vec![])),
            (Uuid::from_u128(4), row(model_on(Uuid::from_u128(4), a, (2026, 6, 10), 10), "A", vec![])),
        ]);
        let now = chrono::Utc::now();

        let (items, _) = snapshot.condensed_summaries(None, now, &[]);
        // 4 intervals → 3 (day, task) cells.
        assert_eq!(items.len(), 3, "{items:?}");
        let dur = |s: &NodeSummary| {
            s.metadata
                .fields
                .iter()
                .find(|f| f.key == "duration")
                .unwrap()
                .value
                .clone()
        };
        // Find the day-1 task-A cell: its duration is the sum 30+30 min.
        let day1_a = items
            .iter()
            .find(|s| s.id == format!("{CONDENSED_ID_PREFIX}2026-06-09:{a}"))
            .expect("day-1 task-A cell");
        assert_eq!(dur(day1_a), "3600");
        assert_eq!(day1_a.label, "A");
        assert!(day1_a.has_children == Some(false));
        // Day-2 task-A is a separate cell (10 min), not merged with day 1.
        let day2_a = items
            .iter()
            .find(|s| s.id == format!("{CONDENSED_ID_PREFIX}2026-06-10:{a}"))
            .expect("day-2 task-A cell");
        assert_eq!(dur(day2_a), "600");
    }

    #[test]
    fn condensed_applies_requested_sort_to_the_rows() {
        // With `S duration desc` the adapter returns the condensed rows
        // ordered by their summed duration — this is what the engine's stable
        // single-level day grouping then turns into within-day ordering.
        let a = Uuid::from_u128(10);
        let b = Uuid::from_u128(20);
        let snapshot = snapshot_from(vec![
            (Uuid::from_u128(1), row(model_on(Uuid::from_u128(1), a, (2026, 6, 9), 30), "A", vec![])),
            (Uuid::from_u128(2), row(model_on(Uuid::from_u128(2), b, (2026, 6, 9), 90), "B", vec![])),
        ]);
        let now = chrono::Utc::now();
        let sort = sort_by("duration", SortDirection::Desc);

        let (items, applied) = snapshot.condensed_summaries(None, now, &sort);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "B", "longest first under duration desc");
        assert_eq!(items[1].label, "A");
        assert_eq!(applied, sort, "the adapter reports the sort it applied");
    }

    fn sort_by(column: &str, direction: SortDirection) -> Vec<SortKey> {
        vec![SortKey {
            column: column.to_string(),
            direction,
        }]
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
    fn entry_metadata_emits_deleted_signal() {
        let now = chrono::Utc::now();
        // A live row carries an empty `deleted` field; a soft-deleted row
        // carries "true" — the TUI dims rows whose `deleted` field is "true".
        let live = row(tracking(Uuid::from_u128(1), Uuid::from_u128(9), 60, true), "Live", vec![]);
        let gone = row(
            deleted_tracking(Uuid::from_u128(2), Uuid::from_u128(9), 60, true),
            "Gone",
            vec![],
        );
        let get = |md: &Metadata, k: &str| {
            md.fields.iter().find(|f| f.key == k).map(|f| f.value.clone())
        };
        assert_eq!(get(&entry_metadata(&live, now), "deleted").as_deref(), Some(""));
        assert_eq!(
            get(&entry_metadata(&gone, now), "deleted").as_deref(),
            Some("true")
        );
    }

    #[test]
    fn active_entry_shows_marker_and_running_ended() {
        let now = chrono::Utc::now();
        let m = tracking(Uuid::from_u128(7), Uuid::from_u128(9), 5, false);
        let r = row(m, "Task", vec![]);
        let md = entry_metadata(&r, now);
        let get = |k: &str| md.fields.iter().find(|f| f.key == k).map(|f| f.value.clone());
        assert_eq!(get("marker").as_deref(), Some("⏱"));
        // Literal "running" (native parity): the engine's datetime column
        // passes unparseable values through verbatim.
        assert_eq!(get("ended").as_deref(), Some("running"));
    }

    #[test]
    fn restore_confirm_prompt_names_the_purge_count() {
        // No successors → plain prompt, no purge clause.
        assert_eq!(
            restore_confirm_prompt("Restore this tracking", 0),
            "Restore this tracking? (y/n)"
        );
        // One successor → singular "interval".
        assert_eq!(
            restore_confirm_prompt("Restore this tracking", 1),
            "Restore this tracking? Purges 1 successor interval — irreversible. (y/n)"
        );
        // Many successors → plural, and the leading subject is verbatim.
        assert_eq!(
            restore_confirm_prompt("Restore 3 deleted trackings", 5),
            "Restore 3 deleted trackings? Purges 5 successor intervals — irreversible. (y/n)"
        );
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
    fn flat_list_sort_uses_adapter_column_kinds() {
        // The flat `list()` path sorts entries via the generic
        // `apply_sort` helper using the kinds declared in
        // `tracking_sortable_columns()`. This exercises that integration:
        // `started` must compare as a DateTime and `task` lexically.
        let a = Uuid::from_u128(1); // started 60 min ago, "Banana"
        let b = Uuid::from_u128(2); // started  5 min ago, "apple"
        let snap = snapshot_from(vec![
            (a, row(tracking(a, Uuid::from_u128(9), 60, true), "Banana", vec![])),
            (b, row(tracking(b, Uuid::from_u128(9), 5, true), "apple", vec![])),
        ]);
        let now = chrono::Utc::now();
        let cols = tracking_sortable_columns();

        // started ascending → oldest (a, 60 min ago) before youngest (b).
        let mut items = snap.entries(None, now);
        let applied = apply_sort(
            &mut items,
            &[not_yet_done_content::SortKey {
                column: "started".into(),
                direction: SortDirection::Asc,
            }],
            &cols,
        );
        assert_eq!(items[0].id, a.to_string());
        assert_eq!(items[1].id, b.to_string());
        assert_eq!(applied.len(), 1);

        // task ascending → case-insensitive "apple" (b) before "Banana" (a).
        let mut items = snap.entries(None, now);
        apply_sort(
            &mut items,
            &[not_yet_done_content::SortKey {
                column: "task".into(),
                direction: SortDirection::Asc,
            }],
            &cols,
        );
        assert_eq!(items[0].id, b.to_string());
        assert_eq!(items[1].id, a.to_string());
    }

    #[test]
    fn shortest_active_and_active_ids_track_running_rows() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let c = Uuid::from_u128(3);
        let snap = snapshot_from(vec![
            (a, row(tracking(a, Uuid::from_u128(9), 60, false), "A", vec![])), // active, older
            (b, row(tracking(b, Uuid::from_u128(9), 30, true), "B", vec![])),  // done
            (c, row(tracking(c, Uuid::from_u128(9), 5, false), "C", vec![])),  // active, youngest
        ]);
        let now = chrono::Utc::now();
        // The youngest active tracking (5 min ago) sets the pace; the
        // completed one is ignored.
        let shortest = snap.shortest_active_secs(now).expect("two active rows");
        assert!((290..=310).contains(&shortest), "got {shortest}");
        assert_eq!(snap.active_ids(), HashSet::from([a, c]));
    }

    #[test]
    fn shortest_active_none_without_running_rows() {
        let a = Uuid::from_u128(1);
        let snap = snapshot_from(vec![(
            a,
            row(tracking(a, Uuid::from_u128(9), 30, true), "A", vec![]),
        )]);
        assert_eq!(snap.shortest_active_secs(chrono::Utc::now()), None);
        assert!(snap.active_ids().is_empty());
    }

    #[test]
    fn live_interval_slows_down_as_the_tracking_ages() {
        // Native parity (`App::tick_active_trackings`): <60s → 5s,
        // <10min → 10s, <1h → 30s, else 60s.
        assert_eq!(live_interval_for(0), Duration::from_secs(5));
        assert_eq!(live_interval_for(59), Duration::from_secs(5));
        assert_eq!(live_interval_for(60), Duration::from_secs(10));
        assert_eq!(live_interval_for(599), Duration::from_secs(10));
        assert_eq!(live_interval_for(600), Duration::from_secs(30));
        assert_eq!(live_interval_for(3599), Duration::from_secs(30));
        assert_eq!(live_interval_for(3600), Duration::from_secs(60));
        assert_eq!(live_interval_for(86_400), Duration::from_secs(60));
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
        assert!(has(&a, "split"));
        assert!(has(&a, "move"));
        // `delete` and `toggle-tracking` show in the action bar; `restore`
        // is a recovery shortcut only.
        let key = |id: &str| a.iter().find(|x| x.id == id).and_then(|x| x.default_key);
        assert_eq!(key("delete"), Some('d'));
        assert_eq!(key("toggle-tracking"), Some('s'));
        assert_eq!(key("restore"), Some('R'));
        assert_eq!(key("split"), Some('i'));
        assert_eq!(key("move"), Some('m'));
    }

    #[test]
    fn split_and_move_advertise_their_form_fields() {
        let a = tracking_entry_actions();
        let fields = |id: &str| match &a.iter().find(|x| x.id == id).unwrap().input {
            InputSpec::Form { fields } => fields.clone(),
            other => panic!("{id} should be a Form, got {other:?}"),
        };

        let split = fields("split");
        let split_keys: Vec<&str> = split.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(split_keys, vec!["at", "task"]);
        assert!(split.iter().find(|f| f.key == "at").unwrap().required);
        assert!(!split.iter().find(|f| f.key == "task").unwrap().required);

        let mv = fields("move");
        let mv_keys: Vec<&str> = mv.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(
            mv_keys,
            vec![
                "start",
                "gravity",
                "offset",
                "allow_overlap",
                "allow_same_task_overlap",
                "allow_future",
            ]
        );
        assert!(mv.iter().find(|f| f.key == "start").unwrap().required);
        // The three guards are toggles (never required); gravity/offset optional.
        for k in ["gravity", "offset", "allow_overlap", "allow_future"] {
            assert!(!mv.iter().find(|f| f.key == k).unwrap().required, "{k} optional");
        }
    }

    #[test]
    fn form_helpers_read_required_optional_and_flags() {
        let mut v = HashMap::new();
        v.insert("a".to_string(), "  x  ".to_string());
        v.insert("blank".to_string(), "   ".to_string());
        v.insert("flag_on".to_string(), "true".to_string());
        v.insert("flag_off".to_string(), "false".to_string());

        assert_eq!(form_required(&v, "a").unwrap(), "x");
        assert!(form_required(&v, "blank").is_err());
        assert!(form_required(&v, "missing").is_err());

        assert_eq!(form_opt(&v, "a"), Some("x".to_string()));
        assert_eq!(form_opt(&v, "blank"), None);
        assert_eq!(form_opt(&v, "missing"), None);

        assert!(form_flag(&v, "flag_on"));
        assert!(!form_flag(&v, "flag_off"));
        assert!(!form_flag(&v, "missing"));
    }

    #[test]
    fn root_actions_expose_list_wide_only() {
        let a = tracking_root_actions();
        assert!(has(&a, "restore-all"));
        assert!(has(&a, "backup"));
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

    // ── Adapter contract: query is the single, replaceable filter ─────────

    /// The snapshot universe carries deleted rows (`find_all_including_deleted`),
    /// and `entries` surfaces a row purely on the query-resolved id set — so a
    /// query that selects a deleted tracking shows it (what makes `restore`
    /// reachable). The default `[deleted, =, false]` query, resolved to the
    /// live ids only, hides it. Nothing is pre-dropped before the query runs.
    #[test]
    fn query_set_is_the_only_thing_that_hides_deleted() {
        let live = Uuid::from_u128(1);
        let gone = Uuid::from_u128(2);
        let task = Uuid::from_u128(9);
        let snap = snapshot_from(vec![
            (live, row(tracking(live, task, 60, true), "live", vec![])),
            (gone, row(deleted_tracking(gone, task, 30, true), "gone", vec![])),
        ]);
        let now = chrono::Utc::now();

        // No query → the whole universe, deleted included (no baked filter).
        let all: HashSet<_> = snap.entries(None, now).into_iter().map(|s| s.id).collect();
        assert!(all.contains(&live.to_string()));
        assert!(all.contains(&gone.to_string()));

        // A `deleted = false`-style set hides the deleted row…
        let live_only = HashSet::from([live]);
        let shown: Vec<_> = snap.entries(Some(&live_only), now);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].id, live.to_string());

        // …and a set that selects the deleted row surfaces it, so `restore`
        // has a target. (Before the contract change the snapshot dropped it.)
        let deleted_only = HashSet::from([gone]);
        let restorable: Vec<_> = snap.entries(Some(&deleted_only), now);
        assert_eq!(restorable.len(), 1);
        assert_eq!(restorable[0].id, gone.to_string());
    }

    /// A deleted tracking, even with no `ended_at`, must never count as
    /// running: not in `active_ids` (cross-tab ⏱ marker / revalidate) and not
    /// in `now_scope` (the live-tick bucket anchor).
    #[test]
    fn deleted_tracking_is_never_running() {
        let live = Uuid::from_u128(1);
        let gone = Uuid::from_u128(2);
        let task = Uuid::from_u128(9);
        // Both un-ended; the deleted one is also the youngest (started later).
        let snap = snapshot_from(vec![
            (live, row(tracking(live, task, 60, false), "live", vec![])),
            (gone, row(deleted_tracking(gone, task, 10, false), "gone", vec![])),
        ]);

        let active = snap.active_ids();
        assert!(active.contains(&live));
        assert!(!active.contains(&gone));

        // now_scope picks the youngest *live* tracking, skipping the deleted
        // (younger) one — otherwise the live tick would anchor on a hidden row.
        let spec = GroupSpec {
            column: "started".to_string(),
            bucket: Some(GroupBucket::Day),
            order: SortDirection::Desc,
        };
        let scope = snap.now_scope(&spec).expect("a live tracking exists");
        let expected = grouping::group_key(
            &bucket_raw_value(&snap.by_id[&live], "started"),
            Some(GroupBucket::Day),
        );
        assert_eq!(scope.key, expected);
    }

    // ── A2c Tree projection ──────────────────────────────────────────────

    /// Build a `task_map` (id → (description, parent)) from `(id, desc,
    /// parent)` triples for the tree-projection tests.
    fn task_map(rows: &[(Uuid, &str, Option<Uuid>)]) -> HashMap<Uuid, (String, Option<Uuid>)> {
        rows.iter()
            .map(|(id, desc, parent)| (*id, (desc.to_string(), *parent)))
            .collect()
    }

    #[test]
    fn tree_projection_folds_cumulated_durations_bottom_up() {
        let root = Uuid::from_u128(1);
        let child = Uuid::from_u128(2);
        let grandchild = Uuid::from_u128(3);
        let tm = task_map(&[
            (root, "Root", None),
            (child, "Child", Some(root)),
            (grandchild, "Grandchild", Some(child)),
        ]);
        // Own seconds: root 10, child 20, grandchild 30.
        let own: HashMap<Uuid, i64> = [(root, 10), (child, 20), (grandchild, 30)]
            .into_iter()
            .collect();
        let proj = build_tree_projection(&tm, &own, &HashSet::new());

        // Own values are preserved verbatim…
        assert_eq!(proj.by_id[&grandchild].own_secs, 30);
        // …and cumulated rolls each subtree up: grandchild = 30, child =
        // 20+30 = 50, root = 10+50 = 60.
        assert_eq!(proj.by_id[&grandchild].cumulated_secs, 30);
        assert_eq!(proj.by_id[&child].cumulated_secs, 50);
        assert_eq!(proj.by_id[&root].cumulated_secs, 60);
    }

    #[test]
    fn tree_projection_prunes_untracked_subtrees_keeps_path_to_tracked() {
        let root = Uuid::from_u128(1);
        let tracked_child = Uuid::from_u128(2);
        let empty_branch = Uuid::from_u128(3);
        let tm = task_map(&[
            (root, "Root", None),
            (tracked_child, "Tracked", Some(root)),
            (empty_branch, "Empty", None),
        ]);
        // Only the tracked child has any time; its ancestor (root) inherits a
        // non-zero cumulated and stays, the empty top-level branch is pruned.
        let own: HashMap<Uuid, i64> = [(tracked_child, 42)].into_iter().collect();
        let proj = build_tree_projection(&tm, &own, &HashSet::new());

        let roots = proj.child_summaries(None, None);
        assert_eq!(roots.len(), 1, "the empty branch is pruned");
        assert_eq!(roots[0].label, "Root");
        assert_eq!(roots[0].id, format!("{TREE_ID_PREFIX}{root}"));
        assert_eq!(roots[0].has_children, Some(true));
        // Root's only visible child is the tracked one.
        let kids = proj.child_summaries(Some(root), None);
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0].label, "Tracked");
        assert_eq!(kids[0].has_children, Some(false));
        // The empty branch itself is invisible.
        assert!(!proj.is_visible(empty_branch));
    }

    #[test]
    fn subtree_walk_expands_to_requested_depth() {
        let root = Uuid::from_u128(1);
        let mid = Uuid::from_u128(2);
        let leaf = Uuid::from_u128(3);
        let sibling = Uuid::from_u128(4); // top-level leaf with its own time
        let tm = task_map(&[
            (root, "Root", None),
            (mid, "Mid", Some(root)),
            (leaf, "Leaf", Some(mid)),
            (sibling, "Sibling", None),
        ]);
        // Only the deepest leaf + the sibling carry time; root/mid inherit a
        // non-zero cumulated and stay visible.
        let own: HashMap<Uuid, i64> = [(leaf, 30), (sibling, 10)].into_iter().collect();
        let proj = build_tree_projection(&tm, &own, &HashSet::new());

        // depth 0 ⇔ child_summaries: top level only, nothing expanded. Ids
        // carry the plain `tree:` prefix (no scope).
        let d0 = proj.subtree(None, None, 0);
        let top: Vec<_> = d0.items.iter().map(|n| n.summary.label.clone()).collect();
        assert_eq!(top, vec!["Root", "Sibling"]); // sibling order is by id
        assert_eq!(d0.items[0].summary.id, format!("{TREE_ID_PREFIX}{root}"));
        assert!(d0.items.iter().all(|n| n.children.items.is_empty()));

        // depth 1: Root expands exactly one level (to Mid); Mid stays unexpanded.
        let d1 = proj.subtree(None, None, 1);
        let r1 = d1.items.iter().find(|n| n.summary.label == "Root").unwrap();
        assert_eq!(r1.children.items.len(), 1);
        assert_eq!(r1.children.items[0].summary.label, "Mid");
        assert!(r1.children.items[0].children.items.is_empty());
        // The sibling is a genuine leaf — never expanded.
        let sib = d1.items.iter().find(|n| n.summary.label == "Sibling").unwrap();
        assert_eq!(sib.summary.has_children, Some(false));
        assert!(sib.children.items.is_empty());

        // depth all: the full chain Root → Mid → Leaf, stopping at the leaf.
        let dall = proj.subtree(None, None, u32::MAX);
        let r = dall.items.iter().find(|n| n.summary.label == "Root").unwrap();
        let m = &r.children.items[0];
        assert_eq!(m.summary.label, "Mid");
        assert_eq!(m.children.items.len(), 1);
        let l = &m.children.items[0];
        assert_eq!(l.summary.label, "Leaf");
        assert_eq!(l.summary.has_children, Some(false));
        assert!(l.children.items.is_empty());
    }

    #[test]
    fn tree_projection_reroots_orphans() {
        let orphan = Uuid::from_u128(2);
        let missing = Uuid::from_u128(999);
        let tm = task_map(&[(orphan, "Orphan", Some(missing))]);
        let own: HashMap<Uuid, i64> = [(orphan, 5)].into_iter().collect();
        let proj = build_tree_projection(&tm, &own, &HashSet::new());
        // The orphan (parent gone) re-roots to the forest top, so it shows
        // up as a top-level row rather than vanishing under the missing id.
        let roots = proj.child_summaries(None, None);
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].label, "Orphan");
        assert!(proj.children.get(&Some(missing)).is_none());
    }

    #[test]
    fn tree_metadata_carries_own_and_cumulated_seconds() {
        let id = Uuid::from_u128(7);
        let r = TreeTaskRow {
            description: "Write report".to_string(),
            own_secs: 600,
            cumulated_secs: 1800,
            active: true,
        };
        let md = tree_metadata(id, &r);
        let get = |k: &str| md.fields.iter().find(|f| f.key == k).map(|f| f.value.clone());
        assert_eq!(get("task").as_deref(), Some("Write report"));
        assert_eq!(get("duration").as_deref(), Some("600"));
        assert_eq!(get("duration_cumulated").as_deref(), Some("1800"));
        assert_eq!(get("marker").as_deref(), Some("⏱"));
        assert_eq!(get("id").as_deref(), Some(id.to_string().as_str()));
    }

    #[test]
    fn parse_tree_id_round_trips_and_rejects_bare_uuid() {
        let id = Uuid::from_u128(7);
        assert_eq!(
            parse_tree_id(&format!("{TREE_ID_PREFIX}{id}")).unwrap(),
            (None, id)
        );
        // A bare (unprefixed) uuid is a tracking-entry id, not a tree id.
        assert!(parse_tree_id(&id.to_string()).is_err());
    }

    // ── Grouped tree (group_by_via_adapter) ──────────────────────────────

    fn day_scope(key: &str) -> BucketScope {
        BucketScope {
            column: "started".to_string(),
            bucket: Some(GroupBucket::Day),
            key: key.to_string(),
        }
    }

    #[test]
    fn bucket_scope_and_scoped_tree_id_round_trip() {
        let scope = day_scope("2026-06-09");
        assert_eq!(scope.encode(), "started:day:2026-06-09");
        assert_eq!(BucketScope::parse("started:day:2026-06-09"), Some(scope.clone()));
        // Unknown granularity token → bad id.
        assert_eq!(BucketScope::parse("started:fortnight:x"), None);

        // Scoped tree-item ids embed the scope and parse back out.
        let task = Uuid::from_u128(7);
        let id = tree_item_id(Some(&scope), task);
        assert_eq!(id, format!("tree:started:day:2026-06-09:{task}"));
        assert_eq!(parse_tree_id(&id).unwrap(), (Some(scope.clone()), task));
        // A verbatim bucket key may itself contain `:` — the key is the
        // remainder between the granularity and the trailing uuid.
        let verbatim = BucketScope {
            column: "task".to_string(),
            bucket: None,
            key: "a:b".to_string(),
        };
        assert_eq!(
            parse_tree_id(&tree_item_id(Some(&verbatim), task)).unwrap(),
            (Some(verbatim), task)
        );
    }

    /// A tracking started at **local** noon of `date` (UTC-converted), so
    /// day-bucket boundaries are timezone-stable in tests.
    fn tracking_on(
        id: Uuid,
        task_id: Uuid,
        date: &str,
        minutes: i64,
        ended: bool,
    ) -> tracking::Model {
        use chrono::TimeZone;
        let naive = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        let started = chrono::Local
            .from_local_datetime(&naive)
            .single()
            .unwrap()
            .with_timezone(&chrono::Utc);
        tracking::Model {
            id,
            task_id,
            predecessor_id: None,
            started_at: started,
            ended_at: ended.then(|| started + chrono::Duration::minutes(minutes)),
            deleted: false,
            created_at: started,
        }
    }

    #[test]
    fn group_summaries_bucket_totals_marker_and_order() {
        let task = Uuid::from_u128(9);
        let t1 = Uuid::from_u128(1);
        let t2 = Uuid::from_u128(2);
        let t3 = Uuid::from_u128(3);
        let snap = snapshot_from(vec![
            // Two trackings on the 9th (one still running), one on the 8th.
            (t1, row(tracking_on(t1, task, "2026-06-09", 30, true), "A", vec![])),
            (t2, row(tracking_on(t2, task, "2026-06-09", 0, false), "A", vec![])),
            (t3, row(tracking_on(t3, task, "2026-06-08", 45, true), "B", vec![])),
        ]);
        let spec = GroupSpec {
            column: "started".to_string(),
            bucket: Some(GroupBucket::Day),
            order: SortDirection::Desc,
        };
        let groups = snap.group_summaries(None, &spec);
        assert_eq!(groups.len(), 2);
        // Desc → newest bucket first; ids embed the scope.
        assert_eq!(groups[0].id, "treegrp:started:day:2026-06-09");
        assert_eq!(groups[1].id, "treegrp:started:day:2026-06-08");
        assert_eq!(groups[0].node_type.type_id, "tracking:tree-group");
        assert_eq!(groups[0].has_children, Some(true));
        // Labels go through the shared bucket display mapping.
        assert_eq!(
            groups[0].label,
            grouping::bucket_display_label("2026-06-09", Some(GroupBucket::Day))
        );
        let get = |g: &NodeSummary, k: &str| {
            g.metadata.fields.iter().find(|f| f.key == k).map(|f| f.value.clone())
        };
        // The 9th: 30 min completed + the open tracking's seconds up to the
        // snapshot's `built_at`; marker set. Both duration columns carry the
        // total (so `zt` never blanks the row).
        let total: i64 = get(&groups[0], "duration").unwrap().parse().unwrap();
        assert!(total >= 30 * 60, "got {total}");
        assert_eq!(get(&groups[0], "duration"), get(&groups[0], "duration_cumulated"));
        assert_eq!(get(&groups[0], "marker").as_deref(), Some("⏱"));
        // The 8th: 45 min, nothing running.
        assert_eq!(get(&groups[1], "duration").as_deref(), Some("2700"));
        assert_eq!(get(&groups[1], "marker").as_deref(), Some(""));

        // Asc flips the order.
        let asc = GroupSpec { order: SortDirection::Asc, ..spec };
        assert_eq!(snap.group_summaries(None, &asc)[0].id, "treegrp:started:day:2026-06-08");
    }

    #[test]
    fn bucket_for_now_resolves_youngest_tracking_bucket() {
        let task = Uuid::from_u128(9);
        let older = Uuid::from_u128(1);
        let youngest = Uuid::from_u128(2);
        let snap = snapshot_from(vec![
            (older, row(tracking_on(older, task, "2026-06-08", 45, false), "A", vec![])),
            // Latest `started_at` — the bucket a start/stop just shifted.
            (youngest, row(tracking_on(youngest, task, "2026-06-09", 0, true), "A", vec![])),
        ]);
        let day = GroupSpec {
            column: "started".to_string(),
            bucket: Some(GroupBucket::Day),
            order: SortDirection::Desc,
        };
        // The id matches the bucket `group_summaries` builds for the 9th, so
        // the frontend's splice always finds a real row to graft onto.
        assert_eq!(
            snap.bucket_for_now(&day).as_deref(),
            Some("treegrp:started:day:2026-06-09")
        );
        // Order doesn't affect resolution — it's the youngest item, not the
        // first bucket.
        let asc = GroupSpec { order: SortDirection::Asc, ..day };
        assert_eq!(
            snap.bucket_for_now(&asc).as_deref(),
            Some("treegrp:started:day:2026-06-09")
        );
    }

    #[test]
    fn bucket_for_now_buckets_verbatim_column_by_youngest() {
        let t_old = Uuid::from_u128(1);
        let t_new = Uuid::from_u128(2);
        let snap = snapshot_from(vec![
            (t_old, row(tracking_on(t_old, Uuid::from_u128(8), "2026-06-08", 30, false), "Old task", vec![])),
            (t_new, row(tracking_on(t_new, Uuid::from_u128(9), "2026-06-09", 0, true), "New task", vec![])),
        ]);
        // Grouping verbatim by the task label (no date bucket): the now-bucket
        // is the youngest tracking's task, keyed verbatim.
        let by_task = GroupSpec {
            column: "task".to_string(),
            bucket: None,
            order: SortDirection::Asc,
        };
        assert_eq!(
            snap.bucket_for_now(&by_task).as_deref(),
            Some("treegrp:task:none:New task")
        );
    }

    #[test]
    fn bucket_for_now_empty_snapshot_is_none() {
        let snap = snapshot_from(vec![]);
        let day = GroupSpec {
            column: "started".to_string(),
            bucket: Some(GroupBucket::Day),
            order: SortDirection::Desc,
        };
        assert_eq!(snap.bucket_for_now(&day), None);
    }

    #[test]
    fn live_group_rows_ticks_now_bucket_chain_against_live_now() {
        // Forest A → B, B running since noon the 9th, plus an *idle*
        // (completed) tracking on a sibling task the same day. The live fold
        // returns the now-bucket header + the running chain (B and its
        // ancestor A) keyed to the rendered tree rows, with durations summed
        // to the passed `now` (not the frozen `built_at`); the idle sibling
        // never ticks.
        let task_a = Uuid::from_u128(10);
        let task_b = Uuid::from_u128(20);
        let other = Uuid::from_u128(30);
        let running = Uuid::from_u128(1);
        let idle = Uuid::from_u128(2);
        let tm = task_map(&[
            (task_a, "A", None),
            (task_b, "B", Some(task_a)),
            (other, "Other", None),
        ]);
        let started = tracking_on(running, task_b, "2026-06-09", 0, false).started_at;
        let mut by_id = HashMap::new();
        by_id.insert(running, row(tracking_on(running, task_b, "2026-06-09", 0, false), "B", vec!["A"]));
        by_id.insert(idle, row(tracking_on(idle, other, "2026-06-09", 30, true), "Other", vec![]));
        let snapshot = TrackingSnapshot {
            by_id,
            order: vec![running, idle],
            tree: Arc::new(TreeProjection::default()),
            task_map: tm,
            built_at: chrono::Utc::now(),
            visible_cache: Default::default(),
            fold_cache: Default::default(),
        };
        let spec = GroupSpec {
            column: "started".to_string(),
            bucket: Some(GroupBucket::Day),
            order: SortDirection::Desc,
        };
        let now1 = started + chrono::Duration::hours(1);
        let now2 = started + chrono::Duration::hours(2);
        let rows1 = snapshot.live_group_rows(&spec, None, now1);
        let rows2 = snapshot.live_group_rows(&spec, None, now2);

        let header = "treegrp:started:day:2026-06-09".to_string();
        let a_id = format!("tree:started:day:2026-06-09:{task_a}");
        let b_id = format!("tree:started:day:2026-06-09:{task_b}");
        let other_id = format!("tree:started:day:2026-06-09:{other}");

        // The chain is exactly header + A + B; the idle sibling never ticks.
        let ids1: HashSet<&str> = rows1.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(rows1.len(), 3, "header + running chain (A, B)");
        assert!(ids1.contains(header.as_str()));
        assert!(ids1.contains(a_id.as_str()));
        assert!(ids1.contains(b_id.as_str()));
        assert!(!ids1.contains(other_id.as_str()), "idle sibling isn't returned");

        let dur = |rows: &[NodeSummary], id: &str, key: &str| -> i64 {
            rows.iter()
                .find(|r| r.id == id)
                .and_then(|r| r.metadata.fields.iter().find(|f| f.key == key))
                .map(|f| f.value.parse().unwrap())
                .unwrap()
        };
        // At now1: B has run one hour (own = cumulated = 3600); A's own is 0
        // but its cumulated rolls up B; the header total mirrors it.
        assert_eq!(dur(&rows1, &b_id, "duration"), 3600);
        assert_eq!(dur(&rows1, &a_id, "duration"), 0, "A has no own tracking");
        assert_eq!(dur(&rows1, &a_id, "duration_cumulated"), 3600);
        // The header is the *whole bucket's* total: B's running hour plus the
        // idle sibling's completed 30 min (1800 s) = 5400.
        assert_eq!(dur(&rows1, &header, "duration"), 3600 + 1800);
        // One hour later only the running chain grew by 3600 s — proof the
        // fold ran against the live `now`, not the frozen snapshot. The idle
        // sibling's 1800 s stays put.
        assert_eq!(dur(&rows2, &b_id, "duration"), 7200);
        assert_eq!(dur(&rows2, &a_id, "duration_cumulated"), 7200);
        assert_eq!(dur(&rows2, &header, "duration"), 7200 + 1800);
        // Header marker stays set while something runs.
        let header_marker = rows1
            .iter()
            .find(|r| r.id == header)
            .and_then(|r| r.metadata.fields.iter().find(|f| f.key == "marker"))
            .map(|f| f.value.clone());
        assert_eq!(header_marker.as_deref(), Some("⏱"));
    }

    #[test]
    fn live_group_rows_empty_without_running_tracking() {
        // Only completed trackings → nothing ticks → no live rows (the timer
        // wouldn't even be paced, but the fold is defensive).
        let task = Uuid::from_u128(9);
        let t1 = Uuid::from_u128(1);
        let tm = task_map(&[(task, "A", None)]);
        let mut by_id = HashMap::new();
        by_id.insert(t1, row(tracking_on(t1, task, "2026-06-09", 30, true), "A", vec![]));
        let snapshot = TrackingSnapshot {
            by_id,
            order: vec![t1],
            tree: Arc::new(TreeProjection::default()),
            task_map: tm,
            built_at: chrono::Utc::now(),
            visible_cache: Default::default(),
            fold_cache: Default::default(),
        };
        let spec = GroupSpec {
            column: "started".to_string(),
            bucket: Some(GroupBucket::Day),
            order: SortDirection::Desc,
        };
        assert!(snapshot.live_group_rows(&spec, None, chrono::Utc::now()).is_empty());
    }

    #[test]
    fn bucket_members_intersect_scope_and_query_filter() {
        let task = Uuid::from_u128(9);
        let t1 = Uuid::from_u128(1);
        let t2 = Uuid::from_u128(2);
        let t3 = Uuid::from_u128(3);
        let snap = snapshot_from(vec![
            (t1, row(tracking_on(t1, task, "2026-06-09", 30, true), "A", vec![])),
            (t2, row(tracking_on(t2, task, "2026-06-09", 15, true), "A", vec![])),
            (t3, row(tracking_on(t3, task, "2026-06-08", 45, true), "B", vec![])),
        ]);
        let scope = day_scope("2026-06-09");
        // Scope alone: both trackings of the 9th.
        let members = snap.bucket_members(None, &scope);
        assert_eq!(members, [t1, t2].into_iter().collect());
        // Intersected with a saved-query filter: only the visible one.
        let visible: HashSet<Uuid> = [t2, t3].into_iter().collect();
        let members = snap.bucket_members(Some(&visible), &scope);
        assert_eq!(members, [t2].into_iter().collect());
    }

    #[test]
    fn grouped_subtree_refolds_durations_per_bucket_with_scoped_ids() {
        // Task forest A → B; B tracked 30 min on the 8th and 45 min on the
        // 9th. The 9th's bucket subtree must fold only the 9th's 45 min and
        // every id must carry the bucket scope.
        let task_a = Uuid::from_u128(10);
        let task_b = Uuid::from_u128(20);
        let t1 = Uuid::from_u128(1);
        let t2 = Uuid::from_u128(2);
        let tm = task_map(&[(task_a, "A", None), (task_b, "B", Some(task_a))]);
        let mut by_id = HashMap::new();
        by_id.insert(t1, row(tracking_on(t1, task_b, "2026-06-08", 30, true), "B", vec!["A"]));
        by_id.insert(t2, row(tracking_on(t2, task_b, "2026-06-09", 45, true), "B", vec!["A"]));
        let snapshot = TrackingSnapshot {
            by_id,
            order: vec![t2, t1],
            tree: Arc::new(TreeProjection::default()),
            task_map: tm,
            built_at: chrono::Utc::now(),
            visible_cache: Default::default(),
            fold_cache: Default::default(),
        };

        let scope = day_scope("2026-06-09");
        let members = snapshot.bucket_members(None, &scope);
        let tree = snapshot.tree_for(Some(&members));
        let roots = tree.child_summaries(None, Some(&scope));
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].label, "A");
        assert_eq!(roots[0].id, format!("tree:started:day:2026-06-09:{task_a}"));
        assert_eq!(tree.by_id[&task_a].cumulated_secs, 45 * 60);
        let kids = tree.child_summaries(Some(task_a), Some(&scope));
        assert_eq!(kids[0].id, format!("tree:started:day:2026-06-09:{task_b}"));
        assert_eq!(tree.by_id[&task_b].own_secs, 45 * 60);
    }

    #[test]
    fn grouped_subtree_reports_has_children_three_levels_deep() {
        // A → B → C, only C tracked (45 min on the 9th). The bucket's
        // folded subtree must report `has_children` correctly at every
        // level so the engine's `expand_depth: all` cascade keeps
        // descending past the top two — the "Trackings tree only
        // opens two levels" repro.
        let task_a = Uuid::from_u128(10);
        let task_b = Uuid::from_u128(20);
        let task_c = Uuid::from_u128(30);
        let t1 = Uuid::from_u128(1);
        let tm = task_map(&[
            (task_a, "A", None),
            (task_b, "B", Some(task_a)),
            (task_c, "C", Some(task_b)),
        ]);
        let mut by_id = HashMap::new();
        by_id.insert(t1, row(tracking_on(t1, task_c, "2026-06-09", 45, true), "C", vec!["A", "B"]));
        let snapshot = TrackingSnapshot {
            by_id,
            order: vec![t1],
            tree: Arc::new(TreeProjection::default()),
            task_map: tm,
            built_at: chrono::Utc::now(),
            visible_cache: Default::default(),
            fold_cache: Default::default(),
        };

        let scope = day_scope("2026-06-09");
        let members = snapshot.bucket_members(None, &scope);
        let tree = snapshot.tree_for(Some(&members));

        let roots = tree.child_summaries(None, Some(&scope));
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].label, "A");
        assert_eq!(roots[0].has_children, Some(true), "A has visible child B");

        let kids_a = tree.child_summaries(Some(task_a), Some(&scope));
        assert_eq!(kids_a.len(), 1);
        assert_eq!(kids_a[0].label, "B");
        assert_eq!(kids_a[0].has_children, Some(true), "B has visible child C");

        let kids_b = tree.child_summaries(Some(task_b), Some(&scope));
        assert_eq!(kids_b.len(), 1);
        assert_eq!(kids_b[0].label, "C");
        assert_eq!(kids_b[0].has_children, Some(false), "C is a leaf");
    }

    #[test]
    fn unfiltered_projection_memoizes_per_scope() {
        let task_a = Uuid::from_u128(0xA);
        let t1 = Uuid::from_u128(1);
        let tm = task_map(&[(task_a, "A", None)]);
        let mut by_id = HashMap::new();
        by_id.insert(t1, row(tracking_on(t1, task_a, "2026-06-09", 30, true), "A", vec![]));
        let snapshot = TrackingSnapshot {
            by_id,
            order: vec![t1],
            tree: Arc::new(TreeProjection::default()),
            task_map: tm,
            built_at: chrono::Utc::now(),
            visible_cache: Default::default(),
            fold_cache: Default::default(),
        };

        // Scope-less = the eagerly built projection, no fold at all.
        assert!(Arc::ptr_eq(&snapshot.unfiltered_projection(None), &snapshot.tree));
        // Scoped folds once; the second request is the same Arc (an
        // expand-all cascade lands here once per node — this is the fix
        // for the seconds-long grouped-tree build).
        let scope = day_scope("2026-06-09");
        let first = snapshot.unfiltered_projection(Some(&scope));
        assert_eq!(first.by_id[&task_a].own_secs, 30 * 60);
        assert!(Arc::ptr_eq(&first, &snapshot.unfiltered_projection(Some(&scope))));
    }

    #[test]
    fn bucket_raw_value_matches_entry_metadata_columns() {
        let m = tracking_on(Uuid::from_u128(1), Uuid::from_u128(9), "2026-06-09", 30, true);
        let r = row(m, "Write report", vec!["Work"]);
        assert_eq!(bucket_raw_value(&r, "task"), "Write report");
        assert_eq!(bucket_raw_value(&r, "taskpath"), "/Work");
        assert_eq!(bucket_raw_value(&r, "started"), r.tracking.started_at.to_rfc3339());
        // Unknown column falls back to `started`.
        assert_eq!(bucket_raw_value(&r, "bogus"), bucket_raw_value(&r, "started"));
        // An open tracking's `ended` is the literal "running" (groups
        // verbatim — same as the flat view's engine-side grouping).
        let open = row(
            tracking_on(Uuid::from_u128(2), Uuid::from_u128(9), "2026-06-09", 0, false),
            "X",
            vec![],
        );
        assert_eq!(bucket_raw_value(&open, "ended"), "running");
    }

    #[test]
    fn tree_actions_expose_toggle_only() {
        let a = tracking_tree_actions();
        assert!(has(&a, "toggle-tracking"));
        assert_eq!(a.len(), 1);
        assert_eq!(
            a.iter().find(|x| x.id == "toggle-tracking").and_then(|x| x.default_key),
            Some('s')
        );
    }
}

/// DB-backed regression tests for the query-scoping of `restore-all`: the
/// list-wide restore must act **only** on the pane's active-query set, never
/// on the whole include-deleted universe. Isolated in its own module so the
/// `sea-orm` test harness imports stay out of the pure snapshot tests above.
#[cfg(test)]
mod restore_scope_tests {
    use super::*;

    use chrono::Utc;
    use sea_orm::{
        ActiveModelTrait, ActiveValue::Set, ConnectionTrait, Database, DbBackend, Schema,
    };
    use shaku::HasComponent;

    use not_yet_done_task_core::entity::{
        task::{self, TaskStatus},
        tracking as tracking_entity,
    };
    use not_yet_done_content::InMemoryHostBus;
    use not_yet_done_task_core::module::TaskDomainModule;
    use not_yet_done_task_core::repository::{
        ProjectRepositoryImpl, ProjectRepositoryImplParameters, TagRepositoryImpl,
        TagRepositoryImplParameters, TaskRepositoryImpl, TaskRepositoryImplParameters,
        TrackingRepository, TrackingRepositoryImpl, TrackingRepositoryImplParameters,
    };
    use not_yet_done_task_core::service::TaskService;

    /// In-memory SQLite + a [`CoreHandle`], plus the raw connection (to insert
    /// task rows directly — the handle has no task-insert). Only the `task`
    /// and `tracking` tables are created — that is all `find_filtered` (which
    /// joins task) and the restore path touch.
    async fn setup() -> (CoreHandle, sea_orm::DatabaseConnection) {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("open in-memory SQLite");
        let schema = Schema::new(DbBackend::Sqlite);
        for stmt in [
            schema.create_table_from_entity(task::Entity),
            schema.create_table_from_entity(tracking_entity::Entity),
        ] {
            db.execute(&stmt).await.expect("schema creation");
        }

        let module = TaskDomainModule::builder()
            .with_component_parameters::<TaskRepositoryImpl>(TaskRepositoryImplParameters {
                db: Some(db.clone()),
            })
            .with_component_parameters::<ProjectRepositoryImpl>(ProjectRepositoryImplParameters {
                db: Some(db.clone()),
            })
            .with_component_parameters::<TagRepositoryImpl>(TagRepositoryImplParameters {
                db: Some(db.clone()),
            })
            .with_component_parameters::<TrackingRepositoryImpl>(
                TrackingRepositoryImplParameters { db: Some(db.clone()) },
            )
            .build();

        let task_service: Arc<dyn TaskService> = module.resolve();
        let tracking_repo: Arc<dyn TrackingRepository> = module.resolve();
        let tracking_service: Arc<dyn not_yet_done_task_core::service::TrackingService> =
            module.resolve();
        let tag_service: Arc<dyn not_yet_done_task_core::service::TagService> = module.resolve();
        let project_service: Arc<dyn not_yet_done_task_core::service::ProjectService> =
            module.resolve();
        let bus = Arc::new(InMemoryHostBus::default());
        (
            CoreHandle::new(
                task_service,
                tracking_repo,
                tracking_service,
                tag_service,
                project_service,
                bus,
                "test".to_string(),
                false,
            ),
            db,
        )
    }

    async fn insert_task(db: &sea_orm::DatabaseConnection, desc: &str) -> Uuid {
        let now = Utc::now();
        let model = task::ActiveModel {
            id: Set(Uuid::new_v4()),
            description: Set(desc.to_string()),
            status: Set(TaskStatus::Todo),
            deleted: Set(false),
            deleted_at: Set(None),
            priority: Set(0),
            parent_id: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            last_tracked_at: Set(None),
            path: Set(None),
        };
        model.insert(db).await.expect("insert task").id
    }

    /// A deleted tracking for `task_id`, returning its id.
    async fn insert_deleted_tracking(handle: &CoreHandle, task_id: Uuid) -> Uuid {
        let t = handle
            .tracking_repo
            .insert(task_id, Utc::now(), None)
            .await
            .expect("insert tracking");
        handle
            .tracking_repo
            .soft_delete_keeping_times(t.id)
            .await
            .expect("soft delete");
        t.id
    }

    async fn is_deleted(handle: &CoreHandle, id: Uuid) -> bool {
        handle
            .tracking_repo
            .find_by_id(id)
            .await
            .expect("find_by_id")
            .expect("row exists")
            .deleted
    }

    #[tokio::test]
    async fn restore_all_acts_only_on_the_active_query_set() {
        let (handle, db) = setup().await;
        let alpha = insert_task(&db, "Alpha project").await;
        let beta = insert_task(&db, "Beta project").await;
        let alpha_tr = insert_deleted_tracking(&handle, alpha).await;
        let beta_tr = insert_deleted_tracking(&handle, beta).await;

        // The pane's active query selects only Alpha. (Body is the YAML
        // query format the trackings view uses — the query is the sole
        // filter, so it intentionally does *not* exclude deleted rows.)
        let query = Some("query:\n  [description, like, '%Alpha%']\n".to_string());
        let visible = resolve_visible_set(&handle, &query)
            .await
            .expect("resolve")
            .expect("query present → Some");
        assert!(visible.contains(&alpha_tr), "Alpha is in the visible set");
        assert!(
            !visible.contains(&beta_tr),
            "Beta is filtered out by the query"
        );

        // restore-all over the visible set restores Alpha and leaves the
        // out-of-query Beta untouched — the bug was that it spanned the whole
        // include-deleted universe.
        let candidates: Vec<Uuid> = visible.into_iter().collect();
        let dispatch = invoke_restore_all(&handle, &candidates, true).await;
        assert!(matches!(dispatch, ActionDispatch::Reload));
        assert!(!is_deleted(&handle, alpha_tr).await, "Alpha restored");
        assert!(
            is_deleted(&handle, beta_tr).await,
            "Beta stays deleted — outside the query"
        );
    }

    #[tokio::test]
    async fn delete_on_already_deleted_row_notifies_instead_of_reconfirming() {
        let (handle, db) = setup().await;
        let task = insert_task(&db, "Gamma project").await;
        let live = handle
            .tracking_repo
            .insert(task, Utc::now(), None)
            .await
            .expect("insert live tracking");
        let gone = insert_deleted_tracking(&handle, task).await;

        let snapshot = TrackingSnapshot::load(&handle).await.expect("load snapshot");
        let ctx = ActionContext::default();

        // A live row routes into the generic delete-confirm flow.
        let live_node = TrackingEntryNode::fetch(&snapshot, &handle, &live.id.to_string())
            .expect("fetch live node");
        assert!(matches!(
            live_node
                .invoke_action("delete", &ctx)
                .await
                .expect("invoke"),
            ActionDispatch::DeleteSelf { .. }
        ));

        // An already-deleted row short-circuits with a neutral notice — no
        // confirm, no re-delete.
        let gone_node = TrackingEntryNode::fetch(&snapshot, &handle, &gone.to_string())
            .expect("fetch deleted node");
        match gone_node
            .invoke_action("delete", &ctx)
            .await
            .expect("invoke")
        {
            ActionDispatch::Error(msg) => assert_eq!(msg, "Already deleted"),
            other => panic!("expected an Already-deleted notice, got {other:?}"),
        }
    }

    /// A fixed completed tracking `[08:00, 10:00]` UTC on a fixed day, returning
    /// its id. Absolute UTC instants (`Z`) keep the split/move time-math
    /// independent of the test machine's local timezone. Inserted as a raw
    /// `ActiveModel` because the repo's `insert` only opens *active* (no-end)
    /// trackings, and `move` requires a completed one.
    async fn insert_completed_tracking(db: &sea_orm::DatabaseConnection, task_id: Uuid) -> Uuid {
        use chrono::TimeZone;
        let start = Utc.with_ymd_and_hms(2026, 3, 22, 8, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2026, 3, 22, 10, 0, 0).unwrap();
        let model = tracking_entity::ActiveModel {
            id: Set(Uuid::new_v4()),
            task_id: Set(task_id),
            predecessor_id: Set(None),
            started_at: Set(start),
            ended_at: Set(Some(end)),
            deleted: Set(false),
            created_at: Set(Utc::now()),
        };
        model.insert(db).await.expect("insert completed tracking").id
    }

    #[tokio::test]
    async fn execute_split_cuts_into_two_and_soft_deletes_original() {
        let (handle, db) = setup().await;
        let task = insert_task(&db, "Delta project").await;
        let original = insert_completed_tracking(&db, task).await;

        // Split at the exact UTC midpoint — strictly inside [08:00, 10:00]
        // regardless of the machine's local offset (RFC3339 `Z` is absolute).
        let mut values = HashMap::new();
        values.insert("at".to_string(), "2026-03-22T09:00:00Z".to_string());

        let outcome = execute_split(&handle, original, &values)
            .await
            .expect("split succeeds");
        assert!(matches!(outcome, ActionOutcome::Done { .. }));

        // Original is soft-deleted; exactly two successors reference it.
        assert!(is_deleted(&handle, original).await, "original soft-deleted");
        let successors = handle
            .tracking_repo
            .find_by_predecessor(original)
            .await
            .expect("find successors");
        assert_eq!(successors.len(), 2, "split produced two new intervals");
    }

    #[tokio::test]
    async fn execute_split_missing_at_field_errors() {
        let (handle, db) = setup().await;
        let task = insert_task(&db, "Epsilon project").await;
        let original = insert_completed_tracking(&db, task).await;

        // No `at` → required-field error, original untouched.
        let err = execute_split(&handle, original, &HashMap::new())
            .await
            .err()
            .expect("missing required field");
        assert!(format!("{err}").contains("'at' is required"));
        assert!(!is_deleted(&handle, original).await, "original untouched");
    }

    #[tokio::test]
    async fn execute_move_relocates_and_soft_deletes_original() {
        let (handle, db) = setup().await;
        let task = insert_task(&db, "Zeta project").await;
        let original = insert_completed_tracking(&db, task).await;

        // Move to an earlier absolute start (in the past, no overlap, no
        // future) — no gravity, so granularity stays None.
        let mut values = HashMap::new();
        values.insert("start".to_string(), "2026-03-20T08:00:00Z".to_string());

        let outcome = execute_move(&handle, original, &values)
            .await
            .expect("move succeeds");
        assert!(matches!(outcome, ActionOutcome::Done { .. }));

        // Move soft-deletes the original and creates one successor at the new
        // start.
        assert!(is_deleted(&handle, original).await, "original soft-deleted");
        let successors = handle
            .tracking_repo
            .find_by_predecessor(original)
            .await
            .expect("find successors");
        assert_eq!(successors.len(), 1, "move produced one relocated interval");
    }

    #[tokio::test]
    async fn execute_move_rejects_invalid_gravity() {
        let (handle, db) = setup().await;
        let task = insert_task(&db, "Eta project").await;
        let original = insert_completed_tracking(&db, task).await;

        let mut values = HashMap::new();
        values.insert("start".to_string(), "2026-03-20T08:00:00Z".to_string());
        values.insert("gravity".to_string(), "sideways".to_string());

        let err = execute_move(&handle, original, &values)
            .await
            .err()
            .expect("invalid gravity");
        assert!(format!("{err}").contains("invalid gravity"));
        assert!(!is_deleted(&handle, original).await, "original untouched");
    }
}
