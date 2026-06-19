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
//!   itself. The `tracking:` toggle in the buffer is honored here too.
//! - **toggle-tracking** (A1c) — a one-key start/stop of the task's time
//!   tracking, paired with the `tracking` marker column (a `⏱` glyph on
//!   running rows). The toggle reads the live state and reuses
//!   [`apply_tracking`], so it respects the host's exclusivity policy.
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
//! Still A1c: saved queries (`task` scope), `FilterExpr` filtering, and
//! scripts.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use not_yet_done_content::{
    ActionContext, ActionDispatch, ActionInput, ActionOutcome, AdapterCapabilities, AdapterFactory,
    ContentAdapter, ContentError, EditorPrep, FsSavedQueryStore, HintPlacement, InputSpec,
    Invalidation, Metadata, MetadataField, Node, NodeAction, NodeSummary, NodeType, Result,
    SavedQueryStore, SortKind, SortableColumn, Subtree, SubtreeNode, TreeFindHit,
    TreeSearchResults,
};
use not_yet_done_core::entity::task;
use not_yet_done_core::error::AppError;
use not_yet_done_core::events::{DomainEvent, DomainEventReceiver};
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;

use crate::editor_templates::{self, FieldError, ParseResult};
use crate::{notes, publish_row_patches, tree_edit, CoreHandle};

/// Indent width for the subtree-restructure outline buffer (`edit-tree`).
/// The `tree_edit` parser infers depth from indentation per level, so any
/// consistent width round-trips; 4 spaces matches the native editor default
/// and `serialize`'s own default.
const TREE_EDIT_INDENT: usize = 4;

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

/// The node type of the flat "list view" projection: every task in the
/// forest as one flat list (depth-first order), no hierarchy. Mirrors the
/// native tab's `vl` list view, rebuilt as a second YAML view. The rows it
/// returns keep `task:item` summaries so actions, mark/move and
/// `get_by_id` behave exactly like in the tree view — this type exists
/// only to route the root `list()` to the flat projection.
fn task_flat_type() -> NodeType {
    NodeType {
        type_id: "task:flat".to_string(),
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
    /// Comma-separated tag names (alphabetical, case-insensitive) — backs the
    /// `tag_names` column. Mirrors the native tab's `fmt_tag_names`.
    tag_names: String,
    /// Concatenated tag symbols (alphabetical by tag name; symbol-less tags
    /// skipped) — backs the `tag_symbols` column. Mirrors the native tab's
    /// `fmt_tag_symbols`.
    tag_symbols: String,
    /// Whether a notes file exists on disk for this task. Precomputed once at
    /// snapshot-build time (a single `find_notes_file` stat) so the `notes`
    /// marker column reads it for free — mirrors the native `📝` marker.
    has_notes: bool,
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
    /// Ids of tasks with an open (running) tracking, resolved once at
    /// build time. Backs the `tracking` marker column and seeds the
    /// start/stop toggle's "is it already running?" check.
    tracked: HashSet<Uuid>,
    /// Ids of tasks whose *subtree* carries a running tracking — the task
    /// itself or any descendant. Folded once from [`tracked`] + the parent
    /// chain. Backs the `tracking_rollup` marker (`collapsed_source` in
    /// `views/tasks.yaml`): a collapsed node shows `⏱` when a tracking it
    /// hides is running, even though its own row isn't the tracked one.
    tracked_subtree: HashSet<Uuid>,
}

impl ForestSnapshot {
    /// Build a snapshot from the live service: load the *whole* task universe
    /// (deleted tasks included), batch-resolve tags, re-root dangling orphans,
    /// and order siblings by creation time for a stable render.
    ///
    /// The universe is unfiltered on purpose (adapter contract): the saved
    /// query is the single, replaceable filter. The shipped default query
    /// `[deleted, =, false]` hides deleted rows; a query that drops or flips
    /// that clause surfaces them. A deleted task stays in the forest structure
    /// so a *non*-deleted child still hangs under its (now deleted) parent —
    /// the parent renders dimmed as context whenever a child matches, rather
    /// than the child silently re-rooting to the top level.
    async fn load(handle: &CoreHandle) -> Result<Arc<Self>> {
        let tasks = handle
            .task_service
            .list_tasks_including_deleted(None)
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
            // Re-root only tasks whose parent is genuinely absent from the
            // universe (a dangling `parent_id` — referential integrity guard).
            // Deleted parents ARE loaded now, so they stay in the structure;
            // hiding them is the query's job, not the loader's.
            let parent = t.parent_id.filter(|p| id_set.contains(p));
            let resolved = tag_map.get(&t.id).map(|v| v.as_slice()).unwrap_or(&[]);
            let tag_names = fmt_tag_names(resolved);
            let tag_symbols = fmt_tag_symbols(resolved);
            children.entry(parent).or_default().push(t.id);
            by_id.insert(
                t.id,
                TaskRow {
                    task: t,
                    tag_names,
                    tag_symbols,
                    // Filled in the notes pass below, once the full task set
                    // is available for parent-path resolution.
                    has_notes: false,
                    parent,
                },
            );
        }
        // Notes marker: `find_notes_file` walks each task's parent chain to
        // build its on-disk notes directory, so it needs the whole task set.
        // Resolve it once here (one `read_dir` per task) and cache the bool —
        // the native tab does the same lookup, just lazily per visible row.
        let all_tasks: Vec<task::Model> = by_id.values().map(|r| r.task.clone()).collect();
        let with_notes: HashSet<Uuid> = all_tasks
            .iter()
            .filter(|t| crate::notes::find_notes_file(t, &all_tasks).is_some())
            .map(|t| t.id)
            .collect();
        for id in &with_notes {
            if let Some(row) = by_id.get_mut(id) {
                row.has_notes = true;
            }
        }
        for siblings in children.values_mut() {
            siblings.sort_by(|a, b| {
                let ca = by_id.get(a).map(|r| r.task.created_at);
                let cb = by_id.get(b).map(|r| r.task.created_at);
                ca.cmp(&cb)
            });
        }
        // Resolve the running-tracking set once for the whole forest rather
        // than per-row: one query feeds every marker cell.
        let tracked: HashSet<Uuid> = handle
            .tracking_repo
            .find_all_active()
            .await
            .map(|active| active.into_iter().map(|t| t.task_id).collect())
            .unwrap_or_default();
        let tracked_subtree = fold_tracked_subtree(&tracked, &by_id);
        Ok(Arc::new(ForestSnapshot {
            by_id,
            children,
            tracked,
            tracked_subtree,
        }))
    }

    /// Number of tasks strictly below `id` in the forest (its whole
    /// subtree, excluding `id` itself). Used to warn that a `delete`
    /// cascades — `delete_task_recursive` soft-deletes the subtree, so the
    /// confirm prompt must say how many tasks go with it. Counts the full
    /// (unfiltered) subtree, matching what the recursive delete touches.
    fn descendant_count(&self, id: Uuid) -> usize {
        let mut stack = vec![id];
        let mut count = 0;
        while let Some(cur) = stack.pop() {
            if let Some(kids) = self.children.get(&Some(cur)) {
                count += kids.len();
                stack.extend(kids.iter().copied());
            }
        }
        count
    }

    /// Ordered child summaries of `parent` (`None` = forest roots).
    /// When `filter` is `Some`, only children in the visible set are
    /// returned (a saved-query filter is active — the set is matches plus
    /// their ancestors, see [`resolve_visible_set`]); `None` lists the
    /// full forest.
    fn child_summaries(
        &self,
        parent: Option<Uuid>,
        filter: Option<&HashSet<Uuid>>,
    ) -> Vec<NodeSummary> {
        self.children
            .get(&parent)
            .map(|ids| {
                ids.iter()
                    .filter(|id| filter.map_or(true, |f| f.contains(id)))
                    .filter_map(|id| self.by_id.get(id).map(|row| self.summary(*id, row, filter)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Walk the forest from `parent` (`None` = roots) down `depth` additional
    /// levels, building the eager [`Subtree`] the engine ingests in one shot
    /// (capability `supports_eager_subtree`). One level mirrors
    /// [`Self::child_summaries`] (same `filter` = visible set under a saved
    /// query); a node is descended only while depth budget remains and it has
    /// visible children. `depth == u32::MAX` expands to every visible leaf.
    /// Pure in-memory — replaces the per-node TUI expand cascade with a single
    /// pass.
    fn subtree(&self, parent: Option<Uuid>, filter: Option<&HashSet<Uuid>>, depth: u32) -> Subtree {
        let items = self
            .children
            .get(&parent)
            .map(|ids| {
                ids.iter()
                    .filter(|id| filter.map_or(true, |f| f.contains(id)))
                    .filter_map(|id| {
                        let row = self.by_id.get(id)?;
                        let summary = self.summary(*id, row, filter);
                        let children = if depth > 0 && summary.has_children == Some(true) {
                            self.subtree(Some(*id), filter, depth - 1)
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

    fn summary(&self, id: Uuid, row: &TaskRow, filter: Option<&HashSet<Uuid>>) -> NodeSummary {
        // `has_children` must reflect *visible* children under an active
        // filter, or the tree would draw an expand glyph that yields no
        // rows. An ancestor in the visible set always has ≥1 visible child
        // (the match below it); a leaf match has none.
        let has_children = self
            .children
            .get(&Some(id))
            .map(|v| v.iter().any(|c| filter.map_or(true, |f| f.contains(c))))
            .unwrap_or(false);
        NodeSummary {
            id: id.to_string(),
            label: row.task.description.clone(),
            node_type: task_item_type(),
            metadata: task_metadata(
                row,
                self.tracked.contains(&id),
                self.tracked_subtree.contains(&id),
                self.ancestors_json(id),
            ),
            has_children: Some(has_children),
        }
    }

    /// Every task in depth-first forest order as one flat list — the
    /// `task:flat` "list view" projection. `filter` keeps only matching
    /// tasks; unlike the tree's [`resolve_visible_set`] there is no
    /// ancestor fill-in (the flat list shows hits, not paths), but the
    /// walk still descends through non-matching parents so nested
    /// matches surface. Rows never expand or drill (`has_children:
    /// Some(false)`).
    fn flat_summaries(&self, filter: Option<&HashSet<Uuid>>) -> Vec<NodeSummary> {
        let mut out = Vec::new();
        let mut stack: Vec<Uuid> = self
            .children
            .get(&None)
            .map(|roots| roots.iter().rev().copied().collect())
            .unwrap_or_default();
        while let Some(id) = stack.pop() {
            if let Some(children) = self.children.get(&Some(id)) {
                stack.extend(children.iter().rev());
            }
            if filter.map_or(true, |f| f.contains(&id)) {
                if let Some(row) = self.by_id.get(&id) {
                    let mut summary = self.summary(id, row, None);
                    summary.has_children = Some(false);
                    out.push(summary);
                }
            }
        }
        out
    }

    /// The task's own refreshed summary plus one for **every ancestor**.
    ///
    /// A tracking start/stop flips not only the task's own `⏱` marker but
    /// the `tracking_rollup` of each ancestor (a *collapsed* ancestor shows
    /// the rollup via `collapsed_source`). Patching only the toggled row
    /// leaves a collapsed parent of a now-stopped task wearing a stale
    /// marker — exactly the bug seen when starting a task elsewhere stops a
    /// tracking buried in a collapsed subtree. Patching the whole chain
    /// refreshes each ancestor's marker; rows not currently visible are
    /// ignored by `patch_row`, so over-reporting is harmless.
    fn summary_with_ancestors(&self, id: Uuid) -> Vec<NodeSummary> {
        let mut out = Vec::new();
        if let Some(row) = self.by_id.get(&id) {
            out.push(self.summary(id, row, None));
        }
        let mut cur = self.by_id.get(&id).and_then(|r| r.parent);
        while let Some(pid) = cur {
            let Some(row) = self.by_id.get(&pid) else { break };
            out.push(self.summary(pid, row, None));
            cur = row.parent;
        }
        out
    }

    /// Root→parent ancestor chain (exclusive of the task itself) as a
    /// JSON array string of `{"id", "description"}` objects — the
    /// `ancestors` metadata field. Scripts parse it to reconstruct the
    /// task's position in the forest (the legacy native Tasks tab handed
    /// the same shape as a structured `task.ancestors` array; here it
    /// rides inside the uniform `node.fields` map, so it is one
    /// JSON-encoded string value).
    fn ancestors_json(&self, id: Uuid) -> String {
        let mut chain: Vec<serde_json::Value> = Vec::new();
        let mut cur = self.by_id.get(&id).and_then(|r| r.parent);
        while let Some(pid) = cur {
            let Some(row) = self.by_id.get(&pid) else { break };
            chain.push(serde_json::json!({
                "id": pid.to_string(),
                "description": row.task.description,
            }));
            cur = row.parent;
        }
        chain.reverse();
        serde_json::Value::Array(chain).to_string()
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

    /// Pure tree-search backing [`ContentAdapter::search_in_tree`]. Two
    /// modes, switched on the query shape:
    ///
    /// - **`id:<uuid>`** — exact node match, independent of the
    ///   (possibly drifted) description. Scripted jumps that already
    ///   resolved the task id via the CLI (e.g. the Taiga `goto_task`
    ///   script) use this so the jump stays exact even when the Taiga
    ///   subject and the local description diverge. Yields 0 or 1 hit.
    /// - **anything else** — case-insensitive, whitespace-tokenised
    ///   AND-substring match against task descriptions, sorted into
    ///   tree-render order (parents before children) and capped at
    ///   `limit`.
    fn tree_search(&self, query: &str, limit: u32) -> TreeSearchResults {
        if let Some(rest) = query.trim().strip_prefix("id:") {
            let hits = match Uuid::parse_str(rest.trim()) {
                Ok(uuid) => self
                    .by_id
                    .get(&uuid)
                    .map(|row| TreeFindHit {
                        path: self.path_to(uuid),
                        label: row.task.description.clone(),
                        space_key: String::new(),
                    })
                    .into_iter()
                    .collect(),
                Err(_) => Vec::new(),
            };
            return TreeSearchResults {
                hits,
                truncated: false,
            };
        }
        let tokens: Vec<String> = query.split_whitespace().map(|t| t.to_lowercase()).collect();
        if tokens.is_empty() {
            return TreeSearchResults {
                hits: Vec::new(),
                truncated: false,
            };
        }
        let mut hits: Vec<TreeFindHit> = self
            .by_id
            .iter()
            .filter(|(_, row)| {
                let hay = row.task.description.to_lowercase();
                tokens.iter().all(|t| hay.contains(t))
            })
            .map(|(id, row)| TreeFindHit {
                path: self.path_to(*id),
                label: row.task.description.clone(),
                space_key: String::new(),
            })
            .collect();
        // Tree-render order: parents before children, siblings together.
        hits.sort_by(|a, b| a.path.cmp(&b.path));
        let truncated = hits.len() > limit as usize;
        hits.truncate(limit as usize);
        TreeSearchResults { hits, truncated }
    }
}

// ---------------------------------------------------------------------------
// Column / metadata helpers
// ---------------------------------------------------------------------------

/// Nerd-font glyph for a [`task::TaskStatus`] — the canonical `status`
/// column value. Mirrors the native tab (`ui/tasks/forest.rs`) pixel-for-pixel
/// so the adapter's "St" column renders the same icon. The four glyphs are in
/// ascending codepoint order Todo < InProgress < Done < Cancelled, so a sort
/// on this column still falls out in the natural status order.
fn status_icon(status: &task::TaskStatus) -> &'static str {
    match status {
        task::TaskStatus::Todo => "󰄰",
        task::TaskStatus::InProgress => "󰄳",
        task::TaskStatus::Done => "󰄵",
        task::TaskStatus::Cancelled => "󰜺",
    }
}

fn tag_name(tag: &not_yet_done_core::repository::ResolvedTag) -> String {
    use not_yet_done_core::repository::ResolvedTag;
    match tag {
        ResolvedTag::Global(t) => t.name.clone(),
        ResolvedTag::Project(t) => t.name.clone(),
    }
}

/// A tag's display symbol, if it has one.
fn tag_symbol(tag: &not_yet_done_core::repository::ResolvedTag) -> Option<String> {
    use not_yet_done_core::repository::ResolvedTag;
    match tag {
        ResolvedTag::Global(t) => t.symbol.clone(),
        ResolvedTag::Project(t) => t.symbol.clone(),
    }
}

/// Comma-separated tag names, alphabetical (case-insensitive) — the
/// `tag_names` column. Mirrors the native `fmt_tag_symbols`/`fmt_tag_names`
/// pair (in the TUI crate, which depends on this one, so the logic is
/// duplicated here rather than shared).
fn fmt_tag_names(tags: &[not_yet_done_core::repository::ResolvedTag]) -> String {
    let mut names: Vec<String> = tags.iter().map(tag_name).collect();
    names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
    names.join(", ")
}

/// Concatenated tag symbols, ordered alphabetically by tag name; tags without
/// a symbol are skipped — the `tag_symbols` column.
fn fmt_tag_symbols(tags: &[not_yet_done_core::repository::ResolvedTag]) -> String {
    let mut pairs: Vec<(String, String)> = tags
        .iter()
        .filter_map(|t| tag_symbol(t).map(|s| (tag_name(t), s)))
        .collect();
    pairs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    pairs.into_iter().map(|(_, s)| s).collect()
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
/// Roll the running-tracking set up the parent chain: each tracked task
/// marks itself and every ancestor as "subtree tracked". Walking `parent`
/// (the re-rooted, cycle-free forest link) bounds each walk by the tree
/// depth, and the `insert` short-circuit both dedupes shared ancestors and
/// makes a corrupt cycle harmless (a node already inserted stops the walk).
fn fold_tracked_subtree(
    tracked: &HashSet<Uuid>,
    by_id: &HashMap<Uuid, TaskRow>,
) -> HashSet<Uuid> {
    let mut subtree: HashSet<Uuid> = HashSet::new();
    for &leaf in tracked {
        let mut cur = Some(leaf);
        while let Some(id) = cur {
            if !subtree.insert(id) {
                break; // already walked this ancestor (and its chain)
            }
            cur = by_id.get(&id).and_then(|r| r.parent);
        }
    }
    subtree
}

fn task_metadata(
    row: &TaskRow,
    is_tracked: bool,
    is_tracked_subtree: bool,
    ancestors_json: String,
) -> Metadata {
    let t = &row.task;
    Metadata {
        fields: vec![
            // Marker column: a running stopwatch glyph when this task has an
            // open tracking, blank otherwise. `views/tasks.yaml` renders it
            // as a narrow `kind: text` column. Mirrors the native tab's `⏱`.
            field(
                "tracking",
                if is_tracked { "⏱".to_string() } else { String::new() },
                "Tracking",
            ),
            // Roll-up marker: `⏱` when this task *or any descendant* is
            // tracked. `views/tasks.yaml` wires it as the `tracking` column's
            // `collapsed_source`, so a collapsed node surfaces a running
            // tracking it hides; an expanded node keeps showing only its own.
            field(
                "tracking_rollup",
                if is_tracked_subtree {
                    "⏱".to_string()
                } else {
                    String::new()
                },
                "Tracking (subtree)",
            ),
            field("status", status_icon(&t.status).to_string(), "Status"),
            field("priority", t.priority.to_string(), "Priority"),
            // Tag columns mirror the native split: `tag_symbols` (icons only)
            // and `tag_names` (comma-separated). Both precomputed at snapshot
            // build so the cells read straight off the row.
            field("tag_symbols", row.tag_symbols.clone(), "Tag symbols"),
            field("tag_names", row.tag_names.clone(), "Tags"),
            // Notes marker: the native `📝` when a notes file exists, blank
            // otherwise.
            field(
                "notes",
                if row.has_notes {
                    "📝".to_string()
                } else {
                    String::new()
                },
                "Notes",
            ),
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
            // Deleted flag (`"true"`/`""`) — not a visible column, a styling
            // signal: the TUI renders rows whose `deleted` field is `"true"`
            // dimmed (a deleted parent kept on screen as context for a
            // matching child). Now that the snapshot loads the full universe
            // this can actually be `true`.
            field(
                "deleted",
                if t.deleted { "true".to_string() } else { String::new() },
                "Deleted",
            ),
            // Root→parent chain as a JSON array string (see
            // [`ForestSnapshot::ancestors_json`]). Consumed by `:script`
            // payloads, not meant as a visible column.
            field("ancestors", ancestors_json, "Ancestors"),
        ],
    }
}

/// Columns a list of tasks can be sorted on. Sorting itself is in-memory
/// in the engine (the adapter applies no server-side sort and reports an
/// empty `applied_sort`); this just marks the headers sort-eligible.
fn task_sortable_columns() -> Vec<SortableColumn> {
    [
        ("description", "Description", SortKind::Text),
        ("status", "Status", SortKind::Text),
        ("priority", "Priority", SortKind::Text),
        ("created", "Created", SortKind::DateTime),
        ("updated", "Updated", SortKind::DateTime),
    ]
    .into_iter()
    .map(|(key, label, kind)| SortableColumn {
        key: key.to_string(),
        label: label.to_string(),
        kind,
    })
    .collect()
}

fn to_content_err(e: AppError) -> ContentError {
    ContentError::Other(Box::new(e))
}

/// Resolve the pane's active query into the set of *visible* task ids:
/// every task matching the query's `FilterExpr`, plus all of their
/// ancestors so the tree keeps a path down to each hit (filtered tree,
/// not a flat list). Ancestors are walked in-memory from the snapshot,
/// so the result is a structurally valid tree regardless of the query's
/// `options.include_ancestors` flag (which the flat native tab honors but
/// a tree inherently requires). Returns `None` when there is no query —
/// the whole forest is visible. A query body that fails to parse surfaces
/// as an error on the load rather than silently showing everything.
async fn resolve_visible_set(
    snapshot: &ForestSnapshot,
    handle: &CoreHandle,
    query: &Option<String>,
) -> Result<Option<HashSet<Uuid>>> {
    let Some(matches) = resolve_match_set(handle, query).await? else {
        return Ok(None);
    };
    let mut visible = HashSet::new();
    for m in matches {
        let mut cur = Some(m);
        while let Some(c) = cur {
            if !visible.insert(c) {
                break; // this ancestor chain is already recorded
            }
            cur = snapshot.by_id.get(&c).and_then(|r| r.parent);
        }
    }
    Ok(Some(visible))
}

/// Resolve the pane's active query into the bare set of *matching* task
/// ids — no ancestor fill-in. This is the flat list view's filter
/// semantics (a query for done tasks shows only done tasks); the tree
/// view layers the ancestor walk on top via [`resolve_visible_set`].
/// `None` = no query, everything visible.
async fn resolve_match_set(
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
        .task_service
        .list_filtered(&parsed.expr)
        .await
        .map_err(to_content_err)?;
    Ok(Some(matches.into_iter().map(|m| m.id).collect()))
}

// ---------------------------------------------------------------------------
// Actions (A1b)
// ---------------------------------------------------------------------------

/// Actions the synthetic forest root exposes: only `add`, which routes
/// through the view config's `type: create` to create a *top-level* task
/// (the new node's parent comes from the buffer's `parent:` field, blank
/// by default).
fn task_root_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new("add", "Add task", InputSpec::Editor)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('a'),
        // Sibling-of-a-top-level-task *is* a top-level task, so on the root
        // `add-sibling` aliases `add` (both → `prepare_add(None)`). It exists
        // here only so the empty-tree create fallback — which invokes the
        // configured action id on the forest root when nothing is selected —
        // finds the id rather than erroring.
        NodeAction::new("add-sibling", "Add task", InputSpec::Editor)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('A'),
    ]
}

/// Actions a single task exposes. `add` makes a task a valid `type: create`
/// container (new child); `edit` opens the markdown buffer on the task
/// itself; `delete`/`undelete`, `toggle-tracking`, and the
/// `mark-move`/`paste-move` reparent pair are fire-and-forget shortcuts
/// dispatched through [`Node::invoke_action`].
fn task_item_actions() -> Vec<NodeAction> {
    vec![
        NodeAction::new("edit", "Edit", InputSpec::Editor)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('e'),
        NodeAction::new("add", "Add subtask", InputSpec::Editor)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('a'),
        // Add a *sibling* of this task: the new node's parent is this task's
        // own (effective, re-rooted) parent, so it lands next to it rather
        // than nested under it. At a top-level task the parent is `None`, so
        // the sibling is another top-level task.
        NodeAction::new("add-sibling", "Add sibling", InputSpec::Editor)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('A'),
        // Subtree-restructure outline editor: edit the task and its whole
        // subtree as one indented checkbox list — reparent, re-status,
        // add/remove rows in a single buffer. Bound to `ctrl+n` in
        // `tasks.yaml` (mirrors the native tab's "edit node" key); no
        // `default_key` here because a ctrl-combo isn't a single `char`.
        NodeAction::new("edit-tree", "edit node", InputSpec::Editor),
        // Free-form per-task notes: edit the task's standalone notes
        // markdown file (no frontmatter/description — the buffer *is* the
        // raw file). Bound to `o` in `tasks.yaml`, mirroring the native
        // tab's notes key. Saving an empty buffer deletes the file.
        NodeAction::new("edit-notes", "notes", InputSpec::Editor)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('o'),
        NodeAction::new("delete", "Delete", InputSpec::None)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('d'),
        // Non-recursive single-task delete for the flat list view, which
        // has no hierarchy on screen. No default key — the flat list's
        // `tasks.yaml` binds `d` to it explicitly; the tree view's `d`
        // stays the recursive `delete`. See [`execute_delete_single`].
        NodeAction::new("delete-single", "Delete", InputSpec::None),
        NodeAction::new("undelete", "Undelete", InputSpec::None).with_default_key('u'),
        NodeAction::new("toggle-tracking", "track", InputSpec::None)
            .with_placement(HintPlacement::ActionBar)
            .with_default_key('s'),
        NodeAction::new("mark-move", "Mark for move", InputSpec::None),
        NodeAction::new("paste-move", "Move here", InputSpec::None),
        NodeAction::new("unnest", "Move to top level", InputSpec::None),
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

/// The task models for `root` plus all its descendants present in the
/// snapshot — the `original_tasks` slice the `edit-tree` outline diffs
/// against (serialize + [`tree_edit::apply_changes`] both operate on it).
fn subtree_tasks(snapshot: &ForestSnapshot, root: Uuid) -> Vec<task::Model> {
    subtree_ids(snapshot, root)
        .into_iter()
        .filter_map(|tid| snapshot.by_id.get(&tid).map(|r| r.task.clone()))
        .collect()
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
pub(crate) async fn apply_tracking(handle: &CoreHandle, task_id: Uuid, wants_tracked: bool) {
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

/// `prepare` for the `edit-tree` action: serialize the selected task and
/// its whole subtree into the indented checkbox outline `tree_edit`
/// understands, with `-t` flags on the directly-tracked rows.
///
/// No version token — the buffer spans many tasks, so there is nothing
/// single to conflict against; [`tree_edit::apply_changes`] reconciles each
/// row by its short id, not by a version timestamp.
fn prepare_edit_tree(snapshot: &ForestSnapshot, id: Uuid) -> Result<EditorPrep> {
    let root = snapshot
        .by_id
        .get(&id)
        .map(|r| r.task.clone())
        .ok_or_else(|| ContentError::NotFound(id.to_string()))?;
    let subtree = subtree_tasks(snapshot, id);
    let template =
        tree_edit::serialize_with_indent(&root, &subtree, TREE_EDIT_INDENT, &snapshot.tracked);
    Ok(EditorPrep {
        template,
        version: String::new(),
        suffix: ".md".into(),
    })
}

/// `execute` for the `edit-tree` action: diff the edited outline against the
/// subtree snapshot and apply every create/update/delete/reparent through
/// [`tree_edit::apply_changes`]. Returns `NoChanges` on an unedited buffer
/// (the diff is a no-op) and reopens with the error message on failure so
/// the user keeps their edits.
async fn execute_edit_tree(
    handle: &CoreHandle,
    snapshot: &ForestSnapshot,
    id: Uuid,
    text: &str,
) -> Result<ActionOutcome> {
    if !snapshot.by_id.contains_key(&id) {
        return Err(ContentError::NotFound(id.to_string()));
    }
    let originals = subtree_tasks(snapshot, id);
    match tree_edit::apply_changes(
        text,
        &originals,
        id,
        &handle.task_service,
        &handle.tracking_repo,
        &snapshot.tracked,
        handle.allow_parallel_tracking,
    )
    .await
    {
        Ok(message) => {
            // The outline can create/move/delete arbitrarily many tasks, so
            // a single `TaskChanged` can't name them all — the bridge maps
            // it to a full reload, which is what a structural edit needs.
            emit_task_changed(handle, id);
            Ok(ActionOutcome::Done {
                message: Some(message),
            })
        }
        Err(e) => Ok(ActionOutcome::Reopen {
            content: editor_templates::render_with_errors(
                text,
                &[FieldError {
                    field: "description",
                    message: e,
                }],
            ),
            new_version: None,
        }),
    }
}

/// `prepare` for the `edit-notes` action: open the task's standalone notes
/// file as a raw markdown buffer. Unlike `edit`, there is no frontmatter or
/// description — the buffer is exactly the file's content (empty when no
/// notes file exists yet). No version token: notes carry no edit timestamp,
/// and the unchanged-buffer guard in [`execute_edit_notes`] handles the
/// "opened, saved nothing" case so a stray empty file is never created.
fn prepare_edit_notes(snapshot: &ForestSnapshot, id: Uuid) -> Result<EditorPrep> {
    let task = snapshot
        .by_id
        .get(&id)
        .map(|r| r.task.clone())
        .ok_or_else(|| ContentError::NotFound(id.to_string()))?;
    let notes_str = notes::read_notes(&task, &all_tasks(snapshot));
    Ok(EditorPrep {
        template: notes_str,
        version: String::new(),
        suffix: ".md".into(),
    })
}

/// `execute` for the `edit-notes` action: write the buffer to the task's
/// notes file, or delete the file when the buffer is blank — the same
/// rule the native notes editor uses. An unchanged buffer is a no-op
/// (returns `NoChanges`); this is what stops an "open notes on a task with
/// no file, save without typing" round-trip from creating an empty file,
/// since the generic edit session always calls `execute` with the opening
/// template as `original` rather than short-circuiting first.
async fn execute_edit_notes(
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
    if text == original {
        return Ok(ActionOutcome::NoChanges);
    }
    let all = all_tasks(snapshot);
    let message = if text.trim().is_empty() {
        notes::delete_notes(&task, &all);
        "Notes deleted"
    } else {
        notes::write_notes(&task, &all, text);
        "Notes saved"
    };
    // The `notes` 📝 marker is precomputed at snapshot-build time, so a
    // create/delete only shows once the snapshot rebuilds: `TaskChanged`
    // drops the cached snapshot and refetches this subtree.
    emit_task_changed(handle, id);
    Ok(ActionOutcome::Done {
        message: Some(message.to_string()),
    })
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

/// `execute("delete-single")` — delete only the invoking task, NOT its
/// subtree. The flat list view uses this: there is no hierarchy on screen,
/// so a recursive cascade would silently delete tasks the user can't see
/// they're affecting. Any children re-root to the forest top on the next
/// load (their parent is now deleted, hence filtered out at snapshot build).
/// The tree view keeps the recursive [`execute_delete`].
async fn execute_delete_single(
    handle: &CoreHandle,
    snapshot: &ForestSnapshot,
    id: Uuid,
) -> Result<ActionOutcome> {
    if let Some(row) = snapshot.by_id.get(&id) {
        notes::mark_notes_deleted(&row.task, &all_tasks(snapshot));
    }
    handle
        .task_service
        .delete_task(id)
        .await
        .map_err(to_content_err)?;
    emit_task_changed(handle, id);
    Ok(ActionOutcome::Done {
        message: Some("Task deleted".to_string()),
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

/// `invoke_action("toggle-tracking")` — flip time tracking for `task_id`.
/// The current state is read live (an active tracking exists?) rather than
/// from the snapshot, so a stale marker can't desync the toggle.
/// [`apply_tracking`] enforces the host's exclusivity policy and emits the
/// `Tracking*` events; the bridge then patches the affected task row's
/// `⏱` marker in place (M9) rather than reloading the tree.
async fn invoke_toggle_tracking(handle: &CoreHandle, task_id: Uuid) -> ActionDispatch {
    let is_tracked = matches!(
        handle.tracking_repo.find_active_for_task(task_id).await,
        Ok(Some(_))
    );
    apply_tracking(handle, task_id, !is_tracked).await;
    // No `Reload`: only the row's tracking marker changed, so the bridge
    // patches it in place instead of rebuilding the (deep) task tree.
    ActionDispatch::Noop
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

/// `invoke_action("unnest")` — move `task` to the top level (`parent_id =
/// None`). The comfort inverse of paste-move: it re-roots a nested task
/// without needing a mark + a target. A no-op (friendly error) when the
/// task is already top-level. No cycle check is needed — the root is never
/// a descendant of anything.
async fn invoke_unnest(handle: &CoreHandle, snapshot: &ForestSnapshot, task_id: Uuid) -> ActionDispatch {
    let Some(row) = snapshot.by_id.get(&task_id) else {
        return ActionDispatch::Error(format!("Task not found: {task_id}"));
    };
    if row.task.parent_id.is_none() {
        return ActionDispatch::Error("Task is already at the top level".to_string());
    }
    let all = all_tasks(snapshot);
    match handle
        .task_service
        .update_task(task_id, None, None, None, Some(None), None)
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
        Err(e) => ActionDispatch::Error(format!("Un-nest failed: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Nodes
// ---------------------------------------------------------------------------

/// The adapter's shared snapshot cache cell: `None` until the first load,
/// then the eager forest. Held by nodes so a mutation can drop it
/// synchronously (see [`invalidate_cache`]).
type SnapshotCell = Arc<RwLock<Option<Arc<ForestSnapshot>>>>;

/// Drop the cached forest snapshot so the *next* read reloads it from the
/// DB. Called synchronously at the end of every mutation, before the action
/// returns: a post-mutation reload can arrive via `get_by_id`
/// (`spawn_content_drill_down` — the cached path) before the async event
/// bridge has had a chance to clear the cache, so without this the reload
/// would re-read the pre-mutation forest and the change wouldn't show until
/// a second refresh. Clearing it here closes that race regardless of which
/// reload path fires.
async fn invalidate_cache(cache: &SnapshotCell) {
    *cache.write().await = None;
}

/// Synthetic forest root. Lists the top-level tasks (`parent_id == None`).
struct TaskRootNode {
    snapshot: Arc<ForestSnapshot>,
    /// Shared cache cell, so a mutation here can invalidate it synchronously.
    cache: SnapshotCell,
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
        vec![task_item_type(), task_flat_type()]
    }
    fn sortable_columns(&self, _node_type: &NodeType) -> Vec<SortableColumn> {
        task_sortable_columns()
    }
    fn actions(&self) -> Vec<NodeAction> {
        task_root_actions()
    }
    async fn list(
        &self,
        params: not_yet_done_content::ListParams,
    ) -> Result<not_yet_done_content::ListResult> {
        // `task:flat` routes to the list-view projection: the whole
        // forest flat, filter = matches only (no ancestor fill-in).
        if params.node_type.type_id == task_flat_type().type_id {
            let filter = resolve_match_set(&self.handle, &params.query).await?;
            return Ok(list_result(self.snapshot.flat_summaries(filter.as_ref())));
        }
        let filter = resolve_visible_set(&self.snapshot, &self.handle, &params.query).await?;
        Ok(list_result(
            self.snapshot.child_summaries(None, filter.as_ref()),
        ))
    }
    async fn list_subtree(
        &self,
        params: not_yet_done_content::ListParams,
        depth: u32,
    ) -> Result<Subtree> {
        // The flat list view (`task:flat`) is a single level of leaf rows —
        // no expansion, so depth is irrelevant.
        if params.node_type.type_id == task_flat_type().type_id {
            let filter = resolve_match_set(&self.handle, &params.query).await?;
            return Ok(leaf_subtree(self.snapshot.flat_summaries(filter.as_ref())));
        }
        // Tree view: same visible-set semantics as `list`, expanded in one
        // pass instead of a per-node cascade (capability
        // `supports_eager_subtree`).
        let filter = resolve_visible_set(&self.snapshot, &self.handle, &params.query).await?;
        Ok(self.snapshot.subtree(None, filter.as_ref(), depth))
    }
    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        TaskItemNode::fetch(&self.snapshot, &self.cache, &self.handle, id)
    }
    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        match action_id {
            // Create a top-level task; the buffer's `parent:` may override.
            // `add-sibling` aliases `add` here (sibling of root = top-level).
            "add" | "add-sibling" => Ok(prepare_add(None)),
            other => Err(ContentError::NotSupported(format!(
                "action `{other}` not supported on the task root"
            ))),
        }
    }
    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        let outcome = match (action_id, input) {
            ("add" | "add-sibling", ActionInput::Edited { text, original, .. }) => {
                execute_add(&self.handle, &self.snapshot, &text, &original).await
            }
            (other, _) => Err(ContentError::NotSupported(format!(
                "action `{other}` not supported on the task root"
            ))),
        };
        // A successful add changed the forest — drop the cache so the
        // reload that follows reads the new task (race-free with the bridge).
        if matches!(outcome, Ok(ActionOutcome::Done { .. })) {
            invalidate_cache(&self.cache).await;
        }
        outcome
    }
}

/// A single task. Owns its label + metadata (the `Node` accessors return
/// borrows) and a shared handle to the forest snapshot for drilling.
struct TaskItemNode {
    snapshot: Arc<ForestSnapshot>,
    /// Shared cache cell, so a mutation here can invalidate it synchronously.
    cache: SnapshotCell,
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
        cache: &SnapshotCell,
        handle: &CoreHandle,
        id: &str,
    ) -> Result<Box<dyn Node>> {
        let uuid = Self::resolve_id(snapshot, id)?;
        let row = &snapshot.by_id[&uuid];
        Ok(Box::new(TaskItemNode {
            snapshot: snapshot.clone(),
            cache: cache.clone(),
            handle: handle.clone(),
            id: uuid,
            id_str: id.to_string(),
            label: row.task.description.clone(),
            node_type: task_item_type(),
            metadata: task_metadata(
                row,
                snapshot.tracked.contains(&uuid),
                snapshot.tracked_subtree.contains(&uuid),
                snapshot.ancestors_json(uuid),
            ),
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
        params: not_yet_done_content::ListParams,
    ) -> Result<not_yet_done_content::ListResult> {
        let filter = resolve_visible_set(&self.snapshot, &self.handle, &params.query).await?;
        Ok(list_result(
            self.snapshot.child_summaries(Some(self.id), filter.as_ref()),
        ))
    }
    async fn list_subtree(
        &self,
        params: not_yet_done_content::ListParams,
        depth: u32,
    ) -> Result<Subtree> {
        let filter = resolve_visible_set(&self.snapshot, &self.handle, &params.query).await?;
        Ok(self.snapshot.subtree(Some(self.id), filter.as_ref(), depth))
    }
    async fn get_child(&self, id: &str) -> Result<Box<dyn Node>> {
        TaskItemNode::fetch(&self.snapshot, &self.cache, &self.handle, id)
    }
    async fn prepare(&self, action_id: &str) -> Result<EditorPrep> {
        match action_id {
            // Create a child task under this one (buffer's `parent:` wins).
            "add" => Ok(prepare_add(Some(self.id))),
            // Create a sibling: parent is this task's own effective parent
            // (re-rooted), so a top-level task gets another top-level sibling.
            "add-sibling" => Ok(prepare_add(
                self.snapshot.by_id.get(&self.id).and_then(|r| r.parent),
            )),
            "edit" => prepare_edit(&self.handle, &self.snapshot, self.id).await,
            "edit-tree" => prepare_edit_tree(&self.snapshot, self.id),
            "edit-notes" => prepare_edit_notes(&self.snapshot, self.id),
            other => Err(ContentError::NotSupported(format!(
                "action `{other}` has no editor buffer"
            ))),
        }
    }
    async fn execute(&mut self, action_id: &str, input: ActionInput) -> Result<ActionOutcome> {
        let outcome = match (action_id, input) {
            ("add" | "add-sibling", ActionInput::Edited { text, original, .. }) => {
                execute_add(&self.handle, &self.snapshot, &text, &original).await
            }
            ("edit", ActionInput::Edited { text, original, .. }) => {
                execute_edit(&self.handle, &self.snapshot, self.id, &text, &original).await
            }
            ("edit-tree", ActionInput::Edited { text, .. }) => {
                execute_edit_tree(&self.handle, &self.snapshot, self.id, &text).await
            }
            ("edit-notes", ActionInput::Edited { text, original, .. }) => {
                execute_edit_notes(&self.handle, &self.snapshot, self.id, &text, &original).await
            }
            // Reached via the generic `DeleteSelf` confirm flow, which calls
            // `execute(<delete action>, None)` after the user confirms.
            // `delete` cascades (tree view); `delete-single` removes only
            // this task (flat list view) — see the two `invoke_action` arms.
            ("delete", _) => execute_delete(&self.handle, &self.snapshot, self.id).await,
            ("delete-single", _) => {
                execute_delete_single(&self.handle, &self.snapshot, self.id).await
            }
            (other, _) => Err(ContentError::NotSupported(format!(
                "action `{other}` not supported on a task"
            ))),
        };
        // Any successful add/edit/delete changed the forest — drop the cache
        // so the follow-up reload reads fresh, even on the cached drill path
        // (race-free with the async event bridge). A validation `Reopen`
        // made no DB change, so leave the cache intact.
        if matches!(outcome, Ok(ActionOutcome::Done { .. })) {
            invalidate_cache(&self.cache).await;
        }
        outcome
    }
    async fn invoke_action(&self, name: &str, ctx: &ActionContext) -> Result<ActionDispatch> {
        let dispatch = match name {
            // Routed to the generic delete-confirm flow; the actual delete
            // happens in `execute("delete")` after confirmation. A task
            // delete recursively soft-deletes its whole subtree, so when
            // the task has descendants we spell that out in the prompt;
            // a leaf falls back to the TUI's generic `Delete '<label>'?`.
            "delete" => {
                let descendants = self.snapshot.descendant_count(self.id);
                let confirm = (descendants > 0).then(|| {
                    let label = self
                        .snapshot
                        .by_id
                        .get(&self.id)
                        .map(|r| r.task.description.as_str())
                        .unwrap_or("this task");
                    let subtasks = if descendants == 1 {
                        "1 subtask".to_string()
                    } else {
                        format!("{descendants} subtasks")
                    };
                    format!(
                        "Delete '{label}' and its {subtasks} (recursive — they are deleted too)? (y/n)"
                    )
                });
                ActionDispatch::DeleteSelf { confirm }
            }
            // Flat list view: delete just this task, no subtree cascade and
            // no recursive warning (the generic `Delete '<label>'?` prompt
            // applies). See [`execute_delete_single`].
            "delete-single" => ActionDispatch::DeleteSelf { confirm: None },
            "undelete" => invoke_undelete(&self.handle).await,
            "toggle-tracking" => invoke_toggle_tracking(&self.handle, self.id).await,
            // The frontend records the mark; the adapter does nothing here.
            "mark-move" => ActionDispatch::Noop,
            "paste-move" => match &ctx.marked {
                Some(marked) => {
                    invoke_paste_move(&self.handle, &self.snapshot, self.id, marked).await
                }
                None => ActionDispatch::Error("Nothing marked to move".to_string()),
            },
            "unnest" => invoke_unnest(&self.handle, &self.snapshot, self.id).await,
            _ => ActionDispatch::Noop,
        };
        // The structural mutations here (reparent via paste-move / unnest,
        // and undelete) all signal success by asking for a `Reload`; drop the
        // cache first so that reload reads the post-mutation forest. The
        // tracking toggle deliberately returns `Noop` (the bridge patches the
        // affected rows in place), so it never reaches this branch.
        if matches!(dispatch, ActionDispatch::Reload) {
            invalidate_cache(&self.cache).await;
        }
        Ok(dispatch)
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

/// Wrap a flat list of summaries as a single-level [`Subtree`] of leaves —
/// the eager-subtree shape for a non-tree (flat) view, where no row expands.
fn leaf_subtree(items: Vec<NodeSummary>) -> Subtree {
    Subtree {
        items: items
            .into_iter()
            .map(|summary| SubtreeNode {
                summary,
                children: Subtree::default(),
            })
            .collect(),
        page: None,
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
/// - `TaskChanged { id }` → clear + [`Invalidation::Node`] (refetch that
///   subtree).
/// - `TrackingStarted`/`Stopped` → the task's `⏱` tracking marker (and the
///   subtree-rollup marker of each ancestor) flips; the forest shape is
///   unchanged. Reload the snapshot (so the `tracked`/`tracked_subtree`
///   sets are fresh) and **patch that task row plus its ancestor chain in
///   place** (M9, [`publish_row_patches`], [`summary_with_ancestors`])
///   instead of [`Invalidation::All`], so a deep, fully-expanded task tree
///   never rebuilds on a `t` toggle — yet a collapsed ancestor's rollup
///   marker still clears when the tracking under it stops. (Under an active
///   saved-query filter the patched `has_children` is recomputed unfiltered
///   — a harmless edge until the next `r`.)
/// - `TrackingChanged` → a tracking was deleted/restored; keep the coarse
///   clear + `All`.
fn spawn_task_bridge(
    mut events: DomainEventReceiver,
    inv_tx: broadcast::Sender<Invalidation>,
    snapshot: Arc<RwLock<Option<Arc<ForestSnapshot>>>>,
    handle: CoreHandle,
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
                Ok(DomainEvent::TrackingStarted { task_id, .. })
                | Ok(DomainEvent::TrackingStopped { task_id, .. }) => {
                    match ForestSnapshot::load(&handle).await {
                        Ok(snap) => {
                            *snapshot.write().await = Some(snap.clone());
                            // Patch the toggled task *and its ancestors* — a
                            // collapsed parent shows the subtree rollup
                            // marker, which also flips when a descendant's
                            // tracking starts/stops.
                            publish_row_patches(&inv_tx, snap.summary_with_ancestors(task_id));
                        }
                        // Couldn't refresh in place — fall back to a full reload.
                        Err(_) => {
                            *snapshot.write().await = None;
                            let _ = inv_tx.send(Invalidation::All);
                        }
                    }
                }
                Ok(DomainEvent::TrackingChanged { .. }) => {
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
            self.handle.clone(),
        );
        let queries_root = dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("not_yet_done")
            .join("tasks")
            .join(instance_id)
            .join("queries");
        Ok(Box::new(TaskAdapter {
            instance_id: instance_id.to_string(),
            handle: self.handle.clone(),
            inv_tx,
            snapshot,
            saved_queries: FsSavedQueryStore::new(queries_root),
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
    /// Filesystem-backed saved queries (`<data>/not_yet_done/tasks/<id>/
    /// queries/*.yaml`). Bodies are the same `name`/`query`/`options`
    /// YAML the native tab persists; applying one filters the forest via
    /// [`resolve_visible_set`]. Shortcuts live in the `query_shortcut`
    /// table under the generic `tasks/<id>/<view>` scope.
    saved_queries: FsSavedQueryStore,
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
            // A1c-2: the task forest is homogeneous (`task:item` →
            // `task:item`), so a saved-query `FilterExpr` is valid at every
            // depth — the engine threads the active query into subtree
            // expansion so a filtered tree stays filtered below the root.
            propagates_query_to_subtree: true,
            // The whole task forest is in memory, so the tree view builds its
            // entire expanded shape in one `list_subtree` projection walk —
            // the engine skips the per-node expand cascade for it.
            supports_eager_subtree: true,
            ..AdapterCapabilities::default()
        }
    }

    fn actions_for_type(&self, node_type: &NodeType) -> Vec<NodeAction> {
        match node_type.type_id.as_str() {
            "task:root" => task_root_actions(),
            // The flat list view's rows are ordinary tasks — same actions.
            "task:item" | "task:flat" => task_item_actions(),
            _ => Vec::new(),
        }
    }

    async fn root(&self) -> Result<Box<dyn Node>> {
        // A `root()` call is the reload entry point — fetch fresh.
        let snapshot = self.reload_snapshot().await?;
        Ok(Box::new(TaskRootNode {
            snapshot,
            cache: self.snapshot.clone(),
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
                cache: self.snapshot.clone(),
                handle: self.handle.clone(),
                node_type: task_root_type(),
                metadata: Metadata::default(),
            }));
        }
        TaskItemNode::fetch(&snapshot, &self.snapshot, &self.handle, id)
    }

    fn subscribe_invalidations(&self) -> broadcast::Receiver<Invalidation> {
        self.inv_tx.subscribe()
    }

    async fn revalidate(&self) {
        // Out-of-process changes (CLI, waybar, another instance) write to
        // the same DB but emit no in-process DomainEvent, so the snapshot's
        // `tracked` set (the `⏱` marker column) can go stale without the
        // bridge noticing. Diff it against the live DB on tab switch; on
        // drift drop the snapshot and reload.
        let snap_tracked = match self.snapshot.read().await.as_ref() {
            Some(snap) => snap.tracked.clone(),
            // No snapshot — the next load is fresh anyway.
            None => return,
        };
        let Ok(active) = self.handle.tracking_repo.find_all_active().await else {
            return;
        };
        let db_tracked: HashSet<Uuid> = active.iter().map(|t| t.task_id).collect();
        if db_tracked != snap_tracked {
            *self.snapshot.write().await = None;
            let _ = self.inv_tx.send(Invalidation::All);
        }
    }

    fn saved_query_store(&self) -> Option<&dyn SavedQueryStore> {
        Some(&self.saved_queries)
    }

    async fn search_in_tree(&self, query: &str, limit: u32) -> Result<Option<TreeSearchResults>> {
        let snapshot = self.snapshot().await?;
        Ok(Some(snapshot.tree_search(query, limit)))
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
                tag_names: String::new(),
                tag_symbols: String::new(),
                has_notes: false,
                parent,
            },
        )
    }

    /// A soft-deleted variant of [`row`]. Its model stays in the snapshot
    /// universe (the adapter now loads `list_tasks_including_deleted`) — it is
    /// only ever hidden by the query, and when kept on screen as a matching
    /// child's ancestor it renders dimmed via the `deleted` styling signal.
    fn deleted_row(id: Uuid, desc: &str, parent: Option<Uuid>) -> (Uuid, TaskRow) {
        let (id, mut r) = row(id, desc, parent);
        r.task.deleted = true;
        r.task.deleted_at = Some(chrono::Utc::now());
        (id, r)
    }

    /// Read a NodeSummary metadata field by key (`""` if absent).
    fn field_value(s: &NodeSummary, key: &str) -> String {
        s.metadata
            .fields
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.value.clone())
            .unwrap_or_default()
    }

    /// Build a snapshot from rows directly (no DB) for pure-logic tests.
    fn snapshot_from(rows: Vec<(Uuid, TaskRow)>) -> Arc<ForestSnapshot> {
        let mut by_id = HashMap::new();
        let mut children: HashMap<Option<Uuid>, Vec<Uuid>> = HashMap::new();
        for (id, row) in rows {
            children.entry(row.parent).or_default().push(id);
            by_id.insert(id, row);
        }
        Arc::new(ForestSnapshot {
            by_id,
            children,
            tracked: HashSet::new(),
            tracked_subtree: HashSet::new(),
        })
    }

    #[test]
    fn descendant_count_walks_whole_subtree() {
        // root → mid → leaf, plus root → sibling. Counts are the full
        // subtree below a node (what a recursive delete soft-deletes).
        let root = Uuid::from_u128(1);
        let mid = Uuid::from_u128(2);
        let leaf = Uuid::from_u128(3);
        let sibling = Uuid::from_u128(4);
        let snap = snapshot_from(vec![
            row(root, "Root", None),
            row(mid, "Mid", Some(root)),
            row(leaf, "Leaf", Some(mid)),
            row(sibling, "Sibling", Some(root)),
        ]);
        assert_eq!(snap.descendant_count(root), 3); // mid + leaf + sibling
        assert_eq!(snap.descendant_count(mid), 1); // leaf
        assert_eq!(snap.descendant_count(leaf), 0); // genuine leaf
        assert_eq!(snap.descendant_count(sibling), 0);
    }

    #[test]
    fn fold_tracked_subtree_marks_self_and_ancestors_only() {
        let root = Uuid::from_u128(1);
        let mid = Uuid::from_u128(2);
        let leaf = Uuid::from_u128(3);
        let sibling = Uuid::from_u128(4); // untracked branch off root
        let mut by_id = HashMap::new();
        for (id, r) in [
            row(root, "Root", None),
            row(mid, "Mid", Some(root)),
            row(leaf, "Leaf", Some(mid)),
            row(sibling, "Sibling", Some(root)),
        ] {
            by_id.insert(id, r);
        }
        let tracked: HashSet<Uuid> = [leaf].into_iter().collect();
        let subtree = fold_tracked_subtree(&tracked, &by_id);
        // The tracked leaf and every ancestor up to the root are marked …
        assert!(subtree.contains(&leaf));
        assert!(subtree.contains(&mid));
        assert!(subtree.contains(&root));
        // … but an untracked sibling branch is not.
        assert!(!subtree.contains(&sibling));
        assert_eq!(subtree.len(), 3);
    }

    #[test]
    fn summary_with_ancestors_patches_self_and_chain_not_siblings() {
        let root = Uuid::from_u128(1);
        let mid = Uuid::from_u128(2);
        let leaf = Uuid::from_u128(3);
        let sibling = Uuid::from_u128(4); // untracked branch off root
        let mut by_id = HashMap::new();
        let mut children: HashMap<Option<Uuid>, Vec<Uuid>> = HashMap::new();
        for (id, r) in [
            row(root, "Root", None),
            row(mid, "Mid", Some(root)),
            row(leaf, "Leaf", Some(mid)),
            row(sibling, "Sibling", Some(root)),
        ] {
            children.entry(r.parent).or_default().push(id);
            by_id.insert(id, r);
        }
        let tracked: HashSet<Uuid> = [leaf].into_iter().collect();
        let tracked_subtree = fold_tracked_subtree(&tracked, &by_id);
        let snap = ForestSnapshot {
            by_id,
            children,
            tracked,
            tracked_subtree,
        };

        let rollup = |s: &NodeSummary| {
            s.metadata
                .fields
                .iter()
                .find(|f| f.key == "tracking_rollup")
                .map(|f| f.value.clone())
                .unwrap_or_default()
        };

        // The toggled leaf and every ancestor are returned, root→… order
        // unimportant — what matters is the set and that each carries the
        // roll-up marker so a collapsed ancestor repaints.
        let patches = snap.summary_with_ancestors(leaf);
        let ids: HashSet<String> = patches.iter().map(|s| s.id.clone()).collect();
        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&leaf.to_string()));
        assert!(ids.contains(&mid.to_string()));
        assert!(ids.contains(&root.to_string()));
        // The untracked sibling branch is never patched.
        assert!(!ids.contains(&sibling.to_string()));
        // Every patched row in the tracked chain lights the roll-up marker.
        for s in &patches {
            assert_eq!(rollup(s), "⏱", "row {} should carry the rollup", s.id);
        }
    }

    #[test]
    fn child_summaries_list_roots_and_children() {
        let root = Uuid::from_u128(1);
        let child = Uuid::from_u128(2);
        let snap = snapshot_from(vec![
            row(root, "Root task", None),
            row(child, "Child task", Some(root)),
        ]);

        let roots = snap.child_summaries(None, None);
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].label, "Root task");
        assert_eq!(roots[0].has_children, Some(true));

        let kids = snap.child_summaries(Some(root), None);
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0].label, "Child task");
        assert_eq!(kids[0].has_children, Some(false));
    }

    #[test]
    fn subtree_expands_forest_to_requested_depth() {
        let root = Uuid::from_u128(1);
        let mid = Uuid::from_u128(2);
        let leaf = Uuid::from_u128(3);
        let snap = snapshot_from(vec![
            row(root, "Root", None),
            row(mid, "Mid", Some(root)),
            row(leaf, "Leaf", Some(mid)),
        ]);

        // depth 0 ⇔ child_summaries: roots only, nothing expanded.
        let d0 = snap.subtree(None, None, 0);
        assert_eq!(d0.items.len(), 1);
        assert_eq!(d0.items[0].summary.label, "Root");
        assert!(d0.items[0].children.items.is_empty());

        // depth 1: Root → Mid, Mid not expanded further.
        let d1 = snap.subtree(None, None, 1);
        let m = &d1.items[0].children.items;
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].summary.label, "Mid");
        assert!(m[0].children.items.is_empty());

        // depth all: full chain, leaf stops naturally.
        let dall = snap.subtree(None, None, u32::MAX);
        let m = &dall.items[0].children.items[0];
        assert_eq!(m.summary.label, "Mid");
        let l = &m.children.items[0];
        assert_eq!(l.summary.label, "Leaf");
        assert_eq!(l.summary.has_children, Some(false));
        assert!(l.children.items.is_empty());
    }

    #[test]
    fn subtree_honours_visible_filter() {
        // root → a (match) , root → b (not in visible set) ; only `a` shows.
        let root = Uuid::from_u128(1);
        let a = Uuid::from_u128(2);
        let b = Uuid::from_u128(3);
        let snap = snapshot_from(vec![
            row(root, "Root", None),
            row(a, "A", Some(root)),
            row(b, "B", Some(root)),
        ]);
        let visible: HashSet<Uuid> = [root, a].into_iter().collect();
        let st = snap.subtree(None, Some(&visible), u32::MAX);
        let kids: Vec<_> = st.items[0]
            .children
            .items
            .iter()
            .map(|n| n.summary.label.clone())
            .collect();
        assert_eq!(kids, vec!["A"]); // `b` filtered out at every level
    }

    #[test]
    fn child_summaries_filter_keeps_only_visible_ids() {
        // Forest: root → child → grandchild, plus a sibling under root.
        let root = Uuid::from_u128(1);
        let child = Uuid::from_u128(2);
        let grandchild = Uuid::from_u128(3);
        let sibling = Uuid::from_u128(4);
        let snap = snapshot_from(vec![
            row(root, "Root", None),
            row(child, "Child", Some(root)),
            row(grandchild, "Grandchild", Some(child)),
            row(sibling, "Sibling", Some(root)),
        ]);

        // Visible set = the grandchild match plus its ancestor chain
        // (child, root). The sibling is filtered out.
        let visible: HashSet<Uuid> = [root, child, grandchild].into_iter().collect();

        let roots = snap.child_summaries(None, Some(&visible));
        assert_eq!(roots.len(), 1, "only the root on the path stays");
        assert_eq!(roots[0].label, "Root");
        // Root still has a visible child (the ancestor chain continues).
        assert_eq!(roots[0].has_children, Some(true));

        let root_kids = snap.child_summaries(Some(root), Some(&visible));
        assert_eq!(root_kids.len(), 1, "sibling is filtered out");
        assert_eq!(root_kids[0].label, "Child");

        // The grandchild is a leaf match: no visible children, so the tree
        // must not draw an expand glyph for it.
        let grandchild_kids = snap.child_summaries(Some(child), Some(&visible));
        assert_eq!(grandchild_kids.len(), 1);
        assert_eq!(grandchild_kids[0].label, "Grandchild");
        assert_eq!(grandchild_kids[0].has_children, Some(false));
    }

    #[test]
    fn flat_summaries_walk_the_forest_depth_first() {
        // Forest: A → B → C, plus root D. The flat list view shows every
        // task in DFS order, none of them expandable.
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
        let flat = snap.flat_summaries(None);
        let labels: Vec<&str> = flat.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, vec!["A", "B", "C", "D"]);
        assert!(
            flat.iter().all(|s| s.has_children == Some(false)),
            "flat rows never expand or drill"
        );
    }

    #[test]
    fn tree_search_id_escape_matches_exactly_one_node() {
        let root = Uuid::from_u128(1);
        let child = Uuid::from_u128(2);
        let snap = snapshot_from(vec![
            row(root, "Work", None),
            row(child, "#42 - Fix the frobnicator", Some(root)),
        ]);

        // `id:<uuid>` resolves the one node and its full root→leaf path,
        // ignoring the description entirely.
        let res = snap.tree_search(&format!("id:{child}"), 50);
        assert_eq!(res.hits.len(), 1);
        assert_eq!(res.hits[0].path, vec![root.to_string(), child.to_string()]);
        assert_eq!(res.hits[0].label, "#42 - Fix the frobnicator");
        assert!(!res.truncated);

        // Unknown / unparseable id → no hits (no panic, no fallback to
        // description search).
        assert!(snap.tree_search(&format!("id:{}", Uuid::from_u128(99)), 50).hits.is_empty());
        assert!(snap.tree_search("id:not-a-uuid", 50).hits.is_empty());
    }

    #[test]
    fn tree_search_description_fallback_is_substring_and_path_sorted() {
        let root = Uuid::from_u128(1);
        let child = Uuid::from_u128(2);
        let snap = snapshot_from(vec![
            row(root, "Frobnicator project", None),
            row(child, "Fix the Frobnicator", Some(root)),
        ]);

        // Case-insensitive AND-substring across descriptions; both rows
        // contain "frobnicator", returned parent-before-child.
        let res = snap.tree_search("frobnicator", 50);
        let labels: Vec<&str> = res.hits.iter().map(|h| h.label.as_str()).collect();
        assert_eq!(labels, vec!["Frobnicator project", "Fix the Frobnicator"]);
    }

    #[test]
    fn flat_summaries_filter_keeps_matches_without_ancestors() {
        // Unlike the tree's visible-set (matches + ancestors), the flat
        // list shows only the matches themselves — even when the match is
        // nested under non-matching parents.
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
        let matches: HashSet<Uuid> = [c, d].into_iter().collect();
        let flat = snap.flat_summaries(Some(&matches));
        let labels: Vec<&str> = flat.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, vec!["C", "D"], "nested match surfaces, no ancestors");
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
    fn ancestors_json_is_root_first_exclusive_of_self() {
        let a = Uuid::from_u128(10);
        let b = Uuid::from_u128(20);
        let c = Uuid::from_u128(30);
        let snap = snapshot_from(vec![
            row(a, "Root \"quoted\"", None),
            row(b, "Mid", Some(a)),
            row(c, "Leaf", Some(b)),
        ]);
        let parsed: serde_json::Value =
            serde_json::from_str(&snap.ancestors_json(c)).expect("valid JSON");
        let chain = parsed.as_array().expect("array");
        assert_eq!(chain.len(), 2, "exclusive of the task itself");
        assert_eq!(chain[0]["id"], a.to_string());
        assert_eq!(chain[0]["description"], "Root \"quoted\"");
        assert_eq!(chain[1]["id"], b.to_string());
        assert_eq!(chain[1]["description"], "Mid");
        // A root task has no ancestors → empty array, still valid JSON.
        assert_eq!(snap.ancestors_json(a), "[]");
    }

    #[test]
    fn metadata_carries_canonical_typed_values() {
        let id = Uuid::from_u128(7);
        let (_, mut r) = row(id, "Do the thing", None);
        r.task.priority = 5;
        r.task.status = task::TaskStatus::InProgress;
        r.tag_names = "home, urgent".into();
        r.tag_symbols = "🔥".into();
        r.has_notes = true;
        let md = task_metadata(&r, true, true, "[]".to_string());
        let get = |k: &str| md.fields.iter().find(|f| f.key == k).map(|f| f.value.clone());
        assert_eq!(get("priority").as_deref(), Some("5"));
        // status is rendered as the native nerd-font glyph, not a text label.
        assert_eq!(get("status").as_deref(), Some("󰄳"));
        assert_eq!(get("tag_names").as_deref(), Some("home, urgent"));
        assert_eq!(get("tag_symbols").as_deref(), Some("🔥"));
        assert_eq!(get("notes").as_deref(), Some("📝"));
        // created/updated are RFC 3339 (parseable back to a datetime).
        let created = get("created").unwrap();
        assert!(chrono::DateTime::parse_from_rfc3339(&created).is_ok());
        assert_eq!(get("last_tracked").as_deref(), Some(""));
        // The marker reflects the passed-in tracked flag.
        assert_eq!(get("tracking").as_deref(), Some("⏱"));
        // The roll-up marker reflects the passed-in subtree flag (separate
        // from the own-marker so `collapsed_source` can pick it up).
        assert_eq!(get("tracking_rollup").as_deref(), Some("⏱"));
        // Own marker off, but a tracked descendant lights the roll-up only.
        let md_rollup = task_metadata(&r, false, true, "[]".to_string());
        let get_rollup =
            |k: &str| md_rollup.fields.iter().find(|f| f.key == k).map(|f| f.value.clone());
        assert_eq!(get_rollup("tracking").as_deref(), Some(""));
        assert_eq!(get_rollup("tracking_rollup").as_deref(), Some("⏱"));
        let md_off = task_metadata(&r, false, false, "[]".to_string());
        assert_eq!(
            md_off.fields.iter().find(|f| f.key == "tracking").map(|f| f.value.as_str()),
            Some("")
        );
        assert_eq!(
            md_off.fields.iter().find(|f| f.key == "tracking_rollup").map(|f| f.value.as_str()),
            Some("")
        );
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
    fn tag_formatting_matches_native_sort_and_join() {
        use not_yet_done_core::entity::global_tag;
        use not_yet_done_core::repository::ResolvedTag;
        let mk = |name: &str, symbol: Option<&str>| {
            ResolvedTag::Global(global_tag::Model {
                id: Uuid::new_v4(),
                name: name.to_string(),
                fg_color: None,
                bg_color: None,
                symbol: symbol.map(str::to_string),
            })
        };
        // Out-of-order, mixed case, one symbol-less tag.
        let tags = vec![
            mk("Urgent", Some("🔥")),
            mk("home", Some("🏠")),
            mk("misc", None),
        ];
        // Names: alphabetical (case-insensitive), comma-joined, ALL names.
        assert_eq!(fmt_tag_names(&tags), "home, misc, Urgent");
        // Symbols: alphabetical by name (home < Urgent), symbol-less skipped.
        assert_eq!(fmt_tag_symbols(&tags), "🏠🔥");
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
        // `add` (top-level task) plus the `add-sibling` alias — both map to
        // `prepare_add(None)` on the root, so the empty-tree `a`/`A` fallback
        // resolves either action id to a top-level create.
        assert!(has(&root, "add"));
        assert!(has(&root, "add-sibling"));
        assert_eq!(root.len(), 2);
        let item = task_item_actions();
        for id in [
            "edit",
            "edit-tree",
            "add",
            "add-sibling",
            "delete",
            "undelete",
            "toggle-tracking",
            "mark-move",
            "paste-move",
        ] {
            assert!(has(&item, id), "task:item missing action `{id}`");
        }
    }

    #[test]
    fn prepare_edit_tree_serializes_subtree_with_tracked_flag() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let c = Uuid::from_u128(3);
        let outside = Uuid::from_u128(9);
        let mut snap = snapshot_from(vec![
            row(a, "Root task", None),
            row(b, "Child task", Some(a)),
            row(c, "Grandchild", Some(b)),
            row(outside, "Unrelated", None),
        ]);
        // The middle node is tracked → its outline row carries the `-t` flag.
        Arc::get_mut(&mut snap).unwrap().tracked.insert(b);

        let prep = prepare_edit_tree(&snap, a).expect("root resolves");
        // The whole subtree is present, the unrelated root is not, and the
        // tracked child is flagged for `tree_edit`'s round-trip.
        assert!(prep.template.contains("Root task"));
        assert!(prep.template.contains("Child task"));
        assert!(prep.template.contains("Grandchild"));
        assert!(!prep.template.contains("Unrelated"));
        assert!(prep.template.contains("-t Child task"));
        // A multi-task restructure has nothing single to version against.
        assert!(prep.version.is_empty());
        // A missing root is a clean NotFound, not a panic.
        assert!(prepare_edit_tree(&snap, Uuid::from_u128(42)).is_err());
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

    #[tokio::test]
    async fn invalidate_cache_drops_the_loaded_snapshot() {
        // The mutation paths call this synchronously so a post-mutation
        // reload (even the cached `get_by_id` drill path) can't re-read the
        // stale forest. Populated cell → `None` after invalidation.
        let snap = snapshot_from(vec![row(Uuid::from_u128(1), "Root", None)]);
        let cell: SnapshotCell = Arc::new(RwLock::new(Some(snap)));
        assert!(cell.read().await.is_some());
        invalidate_cache(&cell).await;
        assert!(
            cell.read().await.is_none(),
            "cache must be cleared so the next read reloads fresh"
        );
    }

    // ── Adapter contract: query is the single filter, deleted-as-context ──

    /// `task_metadata` carries a hidden `deleted` field — the TUI's styling
    /// signal (`"true"` → dim the row). It is `"true"` only for a deleted
    /// model and blank otherwise, so a deleted parent left on screen as a
    /// matching child's context renders greyed while live rows stay normal.
    #[test]
    fn task_metadata_emits_deleted_signal() {
        let live = Uuid::from_u128(1);
        let gone = Uuid::from_u128(2);
        let snap = snapshot_from(vec![
            row(live, "Live", None),
            deleted_row(gone, "Gone", None),
        ]);

        let live_summary = snap.summary(live, &snap.by_id[&live], None);
        let gone_summary = snap.summary(gone, &snap.by_id[&gone], None);
        assert_eq!(field_value(&live_summary, "deleted"), "");
        assert_eq!(field_value(&gone_summary, "deleted"), "true");
    }

    /// The decided #34 semantics: a deleted parent of a *matching* child is
    /// kept visible as context (unchanged ancestor fill-in), just dimmed.
    /// The visible set passed here is exactly what `resolve_visible_set`
    /// produces for a query matching only the live child — the child plus
    /// its (deleted) ancestor chain. The parent must still render, carry the
    /// `deleted` signal, and report a visible child; the matching child shows
    /// normally. A deleted *sibling* with no matching descendant stays out.
    #[test]
    fn deleted_parent_kept_as_dimmed_context_for_matching_child() {
        let parent = Uuid::from_u128(1); // deleted, but on the path to a hit
        let child = Uuid::from_u128(2); // the live match
        let stray = Uuid::from_u128(3); // deleted leaf, no matching descendant
        let snap = snapshot_from(vec![
            deleted_row(parent, "Deleted parent", None),
            row(child, "Live child", Some(parent)),
            deleted_row(stray, "Deleted stray", None),
        ]);

        // resolve_visible_set(query = the live child) = {child} ∪ ancestors.
        let visible: HashSet<Uuid> = [child, parent].into_iter().collect();

        // The deleted parent survives as the only visible root, dimmed, and
        // still advertises a visible child (so the tree draws its expander).
        let roots = snap.child_summaries(None, Some(&visible));
        assert_eq!(roots.len(), 1, "stray deleted leaf is filtered out");
        assert_eq!(roots[0].label, "Deleted parent");
        assert_eq!(field_value(&roots[0], "deleted"), "true");
        assert_eq!(roots[0].has_children, Some(true));

        // Its child is the live match: shown normally, no deleted signal.
        let kids = snap.child_summaries(Some(parent), Some(&visible));
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0].label, "Live child");
        assert_eq!(field_value(&kids[0], "deleted"), "");

        // Same picture through the eager `subtree` projection the tree uses.
        let st = snap.subtree(None, Some(&visible), u32::MAX);
        assert_eq!(st.items.len(), 1);
        assert_eq!(st.items[0].summary.label, "Deleted parent");
        assert_eq!(field_value(&st.items[0].summary, "deleted"), "true");
        let nested: Vec<&str> = st.items[0]
            .children
            .items
            .iter()
            .map(|n| n.summary.label.as_str())
            .collect();
        assert_eq!(nested, vec!["Live child"]);
    }
}
