//! Tree-mode state for a [`ContentPane`].
//!
//! When the active `ViewDef` (or any tree-continuing `ChildDef` in its
//! chain) has `tree_label` set, the pane renders as a tree instead of
//! a flat list: rows expand into their child level inline, indented
//! and prefixed with an expand/collapse glyph in the tree-label
//! column. The legacy `items` / `nav_stack` / `active_child` fields
//! are unused while `tree` is `Some(...)` — the flattened
//! [`TreeEntry`] list is the source of truth for rendering and
//! cursor positioning.
//!
//! This module only defines the data structures. Expand/collapse,
//! lazy loading, pagination placeholders, filter/search and per-level
//! key dispatch are implemented in later phases.

use std::collections::{HashMap, HashSet};

use not_yet_done_content::{NodeSummary, PageRequest};

use crate::config::view_config::{ActionDef, ChildDef, ColumnDef, ViewDef};

/// One visible row in the tree, after flattening the expanded
/// hierarchy. `parent_path` is the chain of node ids from the root
/// down to (and excluding) `node`; root-level entries carry an empty
/// path. `depth == parent_path.len()` — kept as a field so the
/// renderer doesn't have to recompute it for every cell.
#[derive(Debug, Clone)]
pub struct TreeEntry {
    pub depth: usize,
    pub parent_path: Vec<String>,
    /// Chain of node_type ids from root level down to (and including)
    /// this entry's own type. Length == `depth + 1`. Lets walkers
    /// disambiguate which ChildDef produced this entry when its level
    /// has multiple tree-continuing siblings (validator enforces that
    /// node_types are pairwise unique among them).
    pub node_type_chain: Vec<String>,
    pub node: NodeSummary,
    /// Hint for the expand glyph (`▶` vs leaf glyph). Computed at
    /// load time from the level's child-config; cheaper than asking
    /// the adapter on every render.
    pub has_children: bool,
    /// Synthesized "… N more" row at the end of a paginated group.
    /// Activating it (open-key) loads the next page and appends the
    /// new children before this placeholder.
    pub is_more_placeholder: bool,
}

/// Cache of one parent's loaded children. Keyed by the parent path
/// (`Vec<String>` of node ids; the root level uses the empty vec).
///
/// **Single-load mode** (default, used when the parent has exactly
/// one tree-continuing ChildDef): `children` is filled directly,
/// `loaded` flips to true on first arrival, `next_page` may carry a
/// pagination hint. `expected_types` / `pending_types` /
/// `children_by_type` stay empty.
///
/// **Multi-load mode** (parent has N > 1 tree-continuing ChildDefs):
/// `expected_types` is set to those node_types in YAML order at
/// expand-start. Each per-type load populates `children_by_type[type]`
/// and removes its type from `pending_types`. `children` is rebuilt
/// by concatenating buckets in `expected_types` order so the visible
/// order is stable regardless of load-arrival order. `loaded` flips
/// when `pending_types` becomes empty. Multi-load currently ignores
/// per-type pagination (`next_page = None`) — heterogeneous fan-out
/// is reserved for small, bounded child sets like Postgres
/// `Schemas` + `Scripts` groups under a database.
#[derive(Debug, Clone, Default)]
pub struct TreeNodeState {
    pub children: Vec<NodeSummary>,
    /// Next page to request when the user activates the `… N more`
    /// placeholder. `None` once the last page has been loaded (or
    /// when the level isn't paginated, or in multi-load mode).
    pub next_page: Option<PageRequest>,
    /// Whether we've issued a load for this parent at least once
    /// AND every expected per-type load has returned (multi-load).
    /// Distinguishes "not loaded yet" from "loaded, empty children".
    pub loaded: bool,
    /// Multi-load: YAML-ordered list of tree-continuing child
    /// node_types under this parent. None in single-load mode.
    pub expected_types: Option<Vec<String>>,
    /// Multi-load: node_types whose load is still in flight. Empty
    /// (`loaded == true`) once every expected type has arrived.
    pub pending_types: HashSet<String>,
    /// Multi-load: per-type child buckets, merged into `children` in
    /// `expected_types` order whenever a load lands.
    pub children_by_type: HashMap<String, Vec<NodeSummary>>,
}

/// Pane-level tree state. `entries` is the rendered, in-order flat
/// list (rebuilt whenever `expanded` or any cached children change).
/// `expanded` is the set of parent paths the user has opened;
/// `cache` is the loaded children per parent path.
#[derive(Debug, Clone, Default)]
pub struct TreeState {
    pub entries: Vec<TreeEntry>,
    pub expanded: HashSet<Vec<String>>,
    pub cache: HashMap<Vec<String>, TreeNodeState>,
    /// One-shot `expand_depth` cascade armed? Set on a fresh tree (and
    /// re-armed by [`Self::clear_for_new_query`]); cleared by the cascade
    /// once a pass finds nothing left to expand, so later loads landing in
    /// this pane never override the user's manual expand/collapse state.
    pub auto_expand_pending: bool,
    /// One-shot override raised by `content.tree_expand_all` (`zr`): while
    /// set, the auto-expand cascade uses an unbounded depth target so the
    /// whole tree unfolds instead of stopping at `expand_depth`. Cleared by
    /// the cascade once it fully drains, and by `tree_collapse_all` (`zm`)
    /// so a half-finished expand-all can't re-expand what was just folded.
    /// Default `false`; never re-armed by a fresh query (that path uses the
    /// configured `expand_depth`, not expand-all).
    pub expand_all_armed: bool,
}

impl TreeState {
    pub fn new() -> Self {
        Self {
            auto_expand_pending: true,
            ..Self::default()
        }
    }

    /// Drop all expansion and cached children. Called when the active
    /// query changes (a new saved query / search applied): every cached
    /// subtree was loaded under the *old* query and no longer reflects
    /// the new filter, so the tree must re-derive from a fresh root load.
    /// The root cache entry is repopulated immediately afterwards by
    /// [`ContentPane::set_items`]; deeper levels re-fetch (filtered) on
    /// the next expand.
    pub fn clear_for_new_query(&mut self) {
        self.expanded.clear();
        self.cache.clear();
        self.entries.clear();
        // The filtered tree starts from scratch — give it the same
        // `expand_depth` head start as the initial load (not expand-all).
        self.auto_expand_pending = true;
        self.expand_all_armed = false;
    }

    /// Replace the cached children for a parent path. Used by the
    /// adapter-load path to feed depth-0 entries (parent path is
    /// `vec![]`) and, in later phases, the children of an expanded
    /// node. `next_page` arms the pagination placeholder: when `Some`,
    /// the renderer emits a `… N weitere` row at the end of this
    /// parent's children that loads the next slice on activation.
    pub fn set_cached_children(
        &mut self,
        parent_path: Vec<String>,
        children: Vec<NodeSummary>,
        next_page: Option<PageRequest>,
    ) {
        let entry = self.cache.entry(parent_path).or_default();
        entry.children = children;
        entry.next_page = next_page;
        entry.loaded = true;
    }

    /// Append a new slice to an already-loaded parent's children and
    /// re-arm (or clear) the pagination placeholder. Used by the
    /// `… N weitere` activation path. No-op when the parent has not
    /// been loaded yet — the caller should `set_cached_children`
    /// first.
    pub fn extend_cached_children(
        &mut self,
        parent_path: Vec<String>,
        children: Vec<NodeSummary>,
        next_page: Option<PageRequest>,
    ) {
        let entry = self.cache.entry(parent_path).or_default();
        entry.children.extend(children);
        entry.next_page = next_page;
        entry.loaded = true;
    }

    /// Remove `node_id` (and its whole cached subtree) from the tree in
    /// place — the local counterpart of a delete, so a removed row vanishes
    /// without a full reload (reload stays reserved for external changes).
    /// Returns `false` when `node_id` isn't a current entry, so the caller
    /// can fall back to a reload; on `true` the caller rebuilds entries.
    ///
    /// Drops the node from its parent's cached children, prunes every
    /// `expanded`/`cache` path under the deleted node, and — when it was the
    /// parent's last child — flips the *parent's* own `has_children` to
    /// `Some(false)` (it lives in the grandparent's cache) so the parent's
    /// expand glyph disappears. This mirrors the reload path's
    /// leaf-vs-frontier handling without a round-trip.
    pub fn remove_node(&mut self, node_id: &str) -> bool {
        let Some(parent_path) = self
            .entries
            .iter()
            .find(|e| e.node.id == node_id)
            .map(|e| e.parent_path.clone())
        else {
            return false;
        };
        let mut own_path = parent_path.clone();
        own_path.push(node_id.to_string());

        // 1. Drop the node from its parent's cached children.
        if let Some(state) = self.cache.get_mut(&parent_path) {
            state.children.retain(|c| c.id != node_id);
        }
        // 2. Prune the deleted subtree's expansion + cache (its own path and
        //    everything beneath it). A recursive delete removes descendants
        //    in the backend too, so their cache is now stale.
        self.expanded.retain(|p| !p.starts_with(own_path.as_slice()));
        self.cache.retain(|p, _| !p.starts_with(own_path.as_slice()));
        // 3. Last child gone → the parent is now a leaf; clear its glyph in
        //    the grandparent's cache so the `▶` disappears.
        let parent_now_empty = self
            .cache
            .get(&parent_path)
            .map(|s| s.children.is_empty())
            .unwrap_or(false);
        if parent_now_empty {
            if let Some((parent_id, grandparent_path)) = parent_path.split_last() {
                if let Some(gp) = self.cache.get_mut(grandparent_path) {
                    if let Some(p) = gp.children.iter_mut().find(|c| &c.id == parent_id) {
                        p.has_children = Some(false);
                    }
                }
            }
        }
        true
    }

    /// Initialise the cache entry for a parent that expects multiple
    /// per-type loads (heterogeneous tree fan-out). Called once at
    /// expand-start. Subsequent per-type completions go through
    /// [`Self::apply_multi_load_result`].
    pub fn begin_multi_load(&mut self, parent_path: Vec<String>, expected_types: Vec<String>) {
        let entry = self.cache.entry(parent_path).or_default();
        entry.children.clear();
        entry.next_page = None;
        entry.loaded = false;
        entry.pending_types = expected_types.iter().cloned().collect();
        entry.children_by_type.clear();
        entry.expected_types = Some(expected_types);
    }

    /// Land the children loaded for one of the expected node_types of
    /// a multi-load parent. Stores them in the per-type bucket and
    /// rebuilds `children` by concatenating buckets in
    /// `expected_types` order. `loaded` flips once every expected
    /// type has arrived. No-op when the parent is not in multi-load
    /// mode — callers should use [`Self::set_cached_children`] for
    /// single-load.
    pub fn apply_multi_load_result(
        &mut self,
        parent_path: Vec<String>,
        node_type: String,
        children: Vec<NodeSummary>,
    ) {
        let entry = self.cache.entry(parent_path).or_default();
        let Some(expected) = entry.expected_types.clone() else {
            return;
        };
        entry.children_by_type.insert(node_type.clone(), children);
        entry.pending_types.remove(&node_type);
        let mut merged: Vec<NodeSummary> = Vec::new();
        for t in &expected {
            if let Some(items) = entry.children_by_type.get(t) {
                merged.extend(items.iter().cloned());
            }
        }
        entry.children = merged;
        if entry.pending_types.is_empty() {
            entry.loaded = true;
        }
    }

    /// Re-flatten `entries` from `cache` + `expanded`, walking the
    /// tree chain top-down. The root is always rendered if loaded;
    /// deeper levels appear when their parent path is in `expanded`.
    /// `has_children` is derived from the config (does the next
    /// tree-chain level exist?), not from the cache — that lets
    /// rendering decide on the expand glyph before children load.
    pub fn rebuild_entries(&mut self, view_def: &ViewDef) {
        self.entries.clear();
        if view_def.tree_label.is_none() {
            return;
        }
        let Some(root) = self.cache.get(&Vec::<String>::new()) else {
            return;
        };
        if !root.loaded {
            return;
        }
        let root_children = root.children.clone();
        for node in root_children {
            self.flatten_into(node, Vec::new(), Vec::new(), 0, view_def);
        }
    }

    fn flatten_into(
        &mut self,
        node: NodeSummary,
        parent_path: Vec<String>,
        parent_type_chain: Vec<String>,
        depth: usize,
        view_def: &ViewDef,
    ) {
        let own_type = node.node_type.type_id.clone();
        let mut own_type_chain = parent_type_chain;
        own_type_chain.push(own_type);
        // The expand glyph means "this row can be unfolded *inline* to
        // reveal child rows in the tree" — NOT "drilling yields items".
        // Those differ: a Stoat channel's `list()` returns messages, but
        // the view config opens them in a split pane (no `tree_label`),
        // so there is nothing to render inline on expand. The config is
        // therefore the gate: only a chain that actually has a tree-
        // continuing ChildDef can ever show the arrow. Only *within* that
        // gate do we consult the adapter's per-row count
        // (`NodeSummary.has_children = Some(_)`) to suppress the glyph on
        // empty parents (e.g. a Confluence leaf page under the
        // `recursive: true` `pages` ChildDef). Adapters that don't plumb
        // per-row counts (Jira, Taiga, Postgres, mock) leave it `None`
        // and default to expandable wherever the config allows it.
        let has_children = has_tree_continuation_for_chain(view_def, &own_type_chain)
            && node.has_children.unwrap_or(true);
        let node_id = node.id.clone();
        self.entries.push(TreeEntry {
            depth,
            parent_path: parent_path.clone(),
            node_type_chain: own_type_chain.clone(),
            node,
            has_children,
            is_more_placeholder: false,
        });

        // Only recurse when this node is expanded AND we have its
        // children cached. Lazy-load path (Phase 3) replaces this
        // gate with an adapter call.
        let mut own_path = parent_path;
        own_path.push(node_id);
        if !self.expanded.contains(&own_path) {
            return;
        }
        let Some(state) = self.cache.get(&own_path) else {
            return;
        };
        let kids = state.children.clone();
        let has_more = state.next_page.is_some();
        for kid in kids {
            self.flatten_into(kid, own_path.clone(), own_type_chain.clone(), depth + 1, view_def);
        }
        if has_more {
            self.entries.push(more_placeholder_entry(own_path, depth + 1));
        }
    }
}

/// Build the synthetic `… N weitere` row for the tail of a paginated
/// parent. `parent_path` is the parent's own path (which becomes the
/// placeholder's `parent_path`, since the placeholder lives among the
/// parent's children). `depth` is the children's depth.
fn more_placeholder_entry(parent_path: Vec<String>, depth: usize) -> TreeEntry {
    use not_yet_done_content::{Metadata, NodeType};
    TreeEntry {
        depth,
        parent_path,
        // Placeholder rows don't correspond to a real ChildDef — the
        // chain is empty, callers gate placeholder rendering on
        // `is_more_placeholder` before consulting `node_type_chain`.
        node_type_chain: Vec::new(),
        node: NodeSummary {
            id: "__tree_more__".into(),
            label: "weitere laden".into(),
            node_type: NodeType {
                type_id: "__tree_more__".into(),
                mime_type: "text/plain".into(),
                syntax: None,
                file_extension: "".into(),
                display_name: "More".into(),
            },
            metadata: Metadata::default(),
            has_children: None,
        },
        has_children: false,
        is_more_placeholder: true,
    }
}

// ---------------------------------------------------------------------------
// Tree-level lookup
// ---------------------------------------------------------------------------

/// Borrowed view of one tree-chain level: enough config to render its
/// columns and dispatch its actions. Carried by `tree_level_at_depth`
/// so callers don't have to enum-match ViewDef vs ChildDef.
pub struct TreeLevel<'a> {
    pub columns: &'a [ColumnDef],
    pub actions: &'a [ActionDef],
    /// The level's own tree_label key. Always `Some` for a level
    /// returned by [`tree_level_at_depth`] (the walk stops at the
    /// first level without one).
    pub tree_label: &'a str,
}

/// Resolve the tree-chain level at `depth` along the *first-chain*
/// walk (the first tree-continuing child at every step). Depth 0 is
/// the [`ViewDef`] itself. Use this only when the call site has no
/// access to an entry's `node_type_chain` (e.g. column lookup for a
/// header at the cursor's depth in a single-branch tree). For
/// multi-branch trees where the chain ambiguates beyond depth 0,
/// prefer [`tree_level_for_chain`] with the entry's chain.
pub fn tree_level_at_depth<'a>(view_def: &'a ViewDef, depth: usize) -> Option<TreeLevel<'a>> {
    let root_label = view_def.tree_label.as_deref()?;
    if depth == 0 {
        return Some(TreeLevel {
            columns: &view_def.columns,
            actions: &view_def.actions,
            tree_label: root_label,
        });
    }
    let mut current: &ChildDef = first_tree_child(&view_def.children)?;
    for _ in 1..depth {
        current = first_tree_child_effective(current)?;
    }
    let label = current.tree_label.as_deref()?;
    Some(TreeLevel {
        columns: &current.columns,
        actions: &current.actions,
        tree_label: label,
    })
}

/// Resolve the [`TreeLevel`] (columns + actions + label) for an
/// entry whose `node_type_chain` is given. Walks the ChildDef tree
/// by matching node_type at each step. Empty chain → root level
/// (the [`ViewDef`] itself). Used by chain-aware callers so that
/// each branch in a multi-branch tree gets its own ChildDef's
/// columns/actions, not the first-chain default.
pub fn tree_level_for_chain<'a>(
    view_def: &'a ViewDef,
    node_type_chain: &[String],
) -> Option<TreeLevel<'a>> {
    let root_label = view_def.tree_label.as_deref()?;
    // Strip only to decide root-vs-deeper. The lookup gets the *original*
    // chain because `child_def_for_type_chain` strips the root type itself
    // — pre-stripping here would double-strip and, in a uniform recursive
    // tree (root type == child type), eat the child segment, wrongly
    // resolving every deeper level to `None`.
    let chain = strip_view_root_type(node_type_chain, view_def);
    if chain.is_empty() {
        return Some(TreeLevel {
            columns: &view_def.columns,
            actions: &view_def.actions,
            tree_label: root_label,
        });
    }
    let child = child_def_for_type_chain(view_def, node_type_chain)?;
    let label = child.tree_label.as_deref()?;
    Some(TreeLevel {
        columns: &child.columns,
        actions: &child.actions,
        tree_label: label,
    })
}

/// Picks the first tree-continuing child from a list (the one with
/// `tree_label` set). Used by the legacy depth-only walkers — for
/// branch disambiguation use [`child_def_for_type_chain`] instead.
fn first_tree_child(children: &[ChildDef]) -> Option<&ChildDef> {
    children.iter().find(|c| c.tree_label.is_some())
}

/// First-chain step from `parent`: the next tree-continuing level when
/// walking down by depth alone. Respects DSF-3 — a `recursive: true`
/// ChildDef with `tree_label` is its own implicit first tree-child, so
/// the walk stays on `parent` instead of descending into the literal
/// `parent.children` (which for a recursive parent typically holds the
/// non-tree-continuing leaf branches like attachments/comments).
///
/// Without this, depth-only walkers ([`tree_level_at_depth`],
/// [`tree_self_at_depth`], [`tree_level_children`]) lose track of the
/// level past the first recursion and return `None` — which the
/// renderer reads as "no tree_label at this depth", blanking the label
/// cell for every deeper row.
fn first_tree_child_effective(parent: &ChildDef) -> Option<&ChildDef> {
    if parent.recursive && parent.tree_label.is_some() {
        return Some(parent);
    }
    first_tree_child(&parent.children)
}

/// All tree-continuing children of `parent_children` (those with
/// `tree_label` set). Used at expand-time to fan out one
/// [`crate::views::ViewRequest::ExpandTreeNode`] per branch when a
/// level has multiple tree-continuing siblings.
pub fn tree_continuing_children<'a>(parent_children: &'a [ChildDef]) -> Vec<&'a ChildDef> {
    parent_children
        .iter()
        .filter(|c| c.tree_label.is_some())
        .collect()
}

/// True if `parent_children` contains at least one tree-continuing
/// child. Quick predicate for the `has_children` glyph hint.
pub fn has_tree_continuation(parent_children: &[ChildDef]) -> bool {
    parent_children.iter().any(|c| c.tree_label.is_some())
}

/// Effective `children:` set of a ChildDef accounting for recursion.
/// A `recursive: true` ChildDef is an implicit member of its own
/// `children:` — that's the DSF-3 rule for arbitrarily deep
/// self-similar trees (e.g. `db_script_dir` under itself). The
/// self-entry is prepended so it ranks first among siblings for the
/// "pick a tree-continuing child" heuristics.
pub fn effective_child_children<'a>(child: &'a ChildDef) -> Vec<&'a ChildDef> {
    let mut out: Vec<&ChildDef> = Vec::with_capacity(child.children.len() + 1);
    if child.recursive {
        out.push(child);
    }
    out.extend(child.children.iter());
    out
}

/// `has_tree_continuation`, chain-aware. Same as
/// [`tree_level_children_for_chain`] + [`has_tree_continuation`] for
/// non-recursive levels; for a recursive ChildDef at the chain tail
/// it additionally returns `true` when the recursive level itself has
/// a `tree_label` (i.e. self counts as a tree-continuing child).
pub fn has_tree_continuation_for_chain(view_def: &ViewDef, node_type_chain: &[String]) -> bool {
    if let Some(kids) = tree_level_children_for_chain(view_def, node_type_chain) {
        if has_tree_continuation(kids) {
            return true;
        }
    }
    // Pass the ORIGINAL chain — `child_def_for_type_chain` strips the root
    // type itself; pre-stripping double-strips and misses the recursive
    // child in a uniform tree (root type == child type). See the note in
    // `tree_level_children_for_chain`.
    if let Some(child) = child_def_for_type_chain(view_def, node_type_chain) {
        if child.recursive && child.tree_label.is_some() {
            return true;
        }
    }
    false
}

/// Find a [`ChildDef`] among a parent's `children:` by exact
/// `node_type` match. Validator guarantees node_types are pairwise
/// unique among tree-continuing siblings, so this is unambiguous.
pub fn child_by_node_type<'a>(
    parent_children: &'a [ChildDef],
    node_type: &str,
) -> Option<&'a ChildDef> {
    parent_children.iter().find(|c| c.node_type == node_type)
}

/// Resolve the [`ChildDef`] that produced an entry whose
/// `node_type_chain` is given. Empty chain → `None` (that's the
/// view root, which has no producing ChildDef). Walks the ChildDef
/// tree top-down, picking the child with matching node_type at each
/// step. Returns `None` if the chain doesn't match any path in the
/// YAML config (stale entry, mis-typed adapter response).
///
/// `flatten_into` builds chains as `[own_type_at_depth_0, …, own_type_at_depth_n]`,
/// where the depth-0 element is the adapter's node_type for the root
/// rows (e.g. `postgres:database`). That root type doesn't correspond
/// to any ChildDef — the ViewDef itself produces the depth-0 rows —
/// so the walker strips it if it matches `view_def.node_type`. Tests
/// that mock root rows with a ChildDef's node_type are unaffected,
/// since their `view_def.node_type` is a sentinel that never matches.
pub fn child_def_for_type_chain<'a>(
    view_def: &'a ViewDef,
    node_type_chain: &[String],
) -> Option<&'a ChildDef> {
    let chain = strip_view_root_type(node_type_chain, view_def);
    let first = chain.first()?;
    let mut current = child_by_node_type(&view_def.children, first)?;
    for ty in &chain[1..] {
        // Recursive ChildDef: same-type segment stays on `current` —
        // that's the "self is implicit member of children:" rule from
        // DSF-3. Without this branch the walker would try to find the
        // type in declared children and miss it, even though the
        // adapter legitimately produced another row of the same type.
        if current.recursive && current.node_type == *ty {
            continue;
        }
        current = child_by_node_type(&current.children, ty)?;
    }
    Some(current)
}

/// Drop the leading view-root node_type from `chain` if present.
/// `flatten_into` includes the entry's own type at the tail of every
/// chain, which means depth-0 entries carry `[view_def.node_type]` at
/// the head — the ViewDef has no producing ChildDef, so this prefix
/// must be skipped before walking `view_def.children`. Empty
/// `view_def.node_type` is treated as "no root type to strip" so
/// hand-built test fixtures keep working.
fn strip_view_root_type<'a>(chain: &'a [String], view_def: &ViewDef) -> &'a [String] {
    match chain.first() {
        Some(first) if !view_def.node_type.is_empty() && first == &view_def.node_type => {
            &chain[1..]
        }
        _ => chain,
    }
}

/// Resolve the [`ChildDef`] that supplies the level at `depth + 1` —
/// i.e. the children of a row at `depth` — along the *first-chain*
/// walk. Returns `None` when the chain ends or the root view is
/// not tree-enabled. Use this only when the call site truly cares
/// about the first-chain default; for branch-aware lookups use
/// [`child_def_for_type_chain`] with the entry's chain.
pub fn tree_child_def_at_depth<'a>(view_def: &'a ViewDef, depth: usize) -> Option<&'a ChildDef> {
    tree_level_children(view_def, depth)
        .and_then(|kids| kids.iter().find(|c| c.tree_label.is_some()))
}

/// Resolve the [`ChildDef`] whose own level is `depth` along the
/// first-chain walk. Depth 0 returns `None` (the ViewDef has no
/// ChildDef). Same first-chain limitation as
/// [`tree_child_def_at_depth`].
pub fn tree_self_at_depth<'a>(view_def: &'a ViewDef, depth: usize) -> Option<&'a ChildDef> {
    if depth == 0 || view_def.tree_label.is_none() {
        return None;
    }
    let mut current: &ChildDef = first_tree_child(&view_def.children)?;
    for _ in 1..depth {
        current = first_tree_child_effective(current)?;
    }
    current.tree_label.as_deref()?;
    Some(current)
}

/// All child definitions available at tree-`depth` — i.e. the
/// `children:` of the ChildDef at that depth (first-chain walk),
/// or `view_def.children` when `depth == 0`. Returns `None` if the
/// chain doesn't reach `depth`. Used both for the tree-continuing
/// child (for expand) and for leaf-drill siblings (e.g. `Rows` with
/// `split:`). Branch-aware variants take a chain instead — see
/// [`tree_level_children_for_chain`].
pub fn tree_level_children<'a>(view_def: &'a ViewDef, depth: usize) -> Option<&'a [ChildDef]> {
    if view_def.tree_label.is_none() {
        return None;
    }
    if depth == 0 {
        return Some(&view_def.children);
    }
    let mut current: &ChildDef = first_tree_child(&view_def.children)?;
    for _ in 1..depth {
        current = first_tree_child_effective(current)?;
    }
    Some(&current.children)
}

/// Children of the ChildDef at `node_type_chain`. Empty chain →
/// `view_def.children` (the depth-0 entries are produced by the
/// view root's children). Returns `None` when the chain doesn't
/// match any path in the YAML config.
pub fn tree_level_children_for_chain<'a>(
    view_def: &'a ViewDef,
    node_type_chain: &[String],
) -> Option<&'a [ChildDef]> {
    if view_def.tree_label.is_none() {
        return None;
    }
    let chain = strip_view_root_type(node_type_chain, view_def);
    if chain.is_empty() {
        return Some(&view_def.children);
    }
    // Pass the ORIGINAL chain: `child_def_for_type_chain` strips the root
    // type itself, so handing it the pre-stripped `chain` would double-strip
    // and, in a uniform recursive tree (root type == child type, e.g. the
    // tasks adapter's `task:item`/`task:item`), eat the child segment —
    // resolving every level past depth 0 to `None` and blanking its
    // expand glyph. Same fix as `tree_level_for_chain` / `leaf_glyph_opt_for_chain`.
    Some(&child_def_for_type_chain(view_def, node_type_chain)?.children)
}

/// Pick the expand-state glyph for a tree entry. Placeholders show
/// an ellipsis marker; entries with no children show the level's
/// configured `leaf_glyph` (falling back to `·`); expandable entries
/// show `▶`/`▼` depending on whether their own path is in `expanded`.
///
/// The leaf glyph comes from the producing ChildDef's `leaf_glyph`
/// (or the ViewDef's at depth 0) so semantically-distinct level
/// types can use semantically-distinct glyphs (e.g. `📄` for pages
/// vs. `·` for generic rows).
pub fn tree_row_glyph<'a>(
    entry: &'a TreeEntry,
    tree: &TreeState,
    view_def: &'a ViewDef,
) -> &'a str {
    if entry.is_more_placeholder {
        return "…";
    }
    if !entry.has_children {
        return leaf_glyph_for_chain(view_def, &entry.node_type_chain);
    }
    let mut own_path = entry.parent_path.clone();
    own_path.push(entry.node.id.clone());
    if tree.expanded.contains(&own_path) {
        "▼"
    } else {
        "▶"
    }
}

/// Resolve the configured leaf glyph for an entry whose
/// `node_type_chain` is given. Walks the ChildDef tree by node_type
/// to find the producing level; returns its `leaf_glyph` if set,
/// otherwise falls back to the ViewDef's `leaf_glyph`, otherwise to
/// the universal default `·`.
fn leaf_glyph_for_chain<'a>(view_def: &'a ViewDef, node_type_chain: &[String]) -> &'a str {
    leaf_glyph_opt_for_chain(view_def, node_type_chain).unwrap_or("·")
}

/// Like [`leaf_glyph_for_chain`] but without the universal `·` default:
/// returns the configured leaf glyph or `None`. The connector-style
/// renderer wants no default so native-looking leaves render as just
/// the connector (`└── Label`), while adapters that *do* configure a
/// glyph (e.g. Confluence pages `📄`) still get it.
pub fn leaf_glyph_opt_for_chain<'a>(
    view_def: &'a ViewDef,
    node_type_chain: &[String],
) -> Option<&'a str> {
    // Pass the ORIGINAL chain: `child_def_for_type_chain` strips the root
    // type itself, so pre-stripping here would double-strip and miss the
    // child in a uniform recursive tree (root type == child type).
    if let Some(child) = child_def_for_type_chain(view_def, node_type_chain) {
        if let Some(g) = child.leaf_glyph.as_deref() {
            return Some(g);
        }
    }
    view_def.leaf_glyph.as_deref()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use not_yet_done_content::{Metadata, NodeType};

    use crate::action::ActionChains;
    use crate::config::view_config::{ColumnDef, ColumnKind, PreviewConfig};

    use super::*;

    #[test]
    fn defaults_are_empty() {
        let t = TreeState::new();
        assert!(t.entries.is_empty());
        assert!(t.expanded.is_empty());
        assert!(t.cache.is_empty());
    }

    #[test]
    fn tree_node_state_default() {
        let n = TreeNodeState::default();
        assert!(!n.loaded);
        assert!(n.children.is_empty());
        assert!(n.next_page.is_none());
    }

    fn col(key: &str) -> ColumnDef {
        ColumnDef {
            key: key.into(),
            label: None,
            source: None,
            collapsed_source: None,
            long_source: None,
            style: None,
            sizing: "max".into(),
            markdown: false,
            kind: ColumnKind::Text,
            format: None,
            separator: None,
            elapsed_from: None,
            tree_aggregate: None,
            hidden: false,
        }
    }

    fn child(name: &str, tree_label: Option<&str>, columns: Vec<ColumnDef>, children: Vec<ChildDef>) -> ChildDef {
        ChildDef {
            row_layout: None,
            smooth_scroll: false,
            name: name.into(),
            node_type: format!("mock:{name}"),
            columns,
            preview: None as Option<PreviewConfig>,
            actions: Vec::new(),
            children,
            split: None,
            pagination: None,
            keybindings: HashMap::new(),
            action_chains: ActionChains::default(),
            column_cursor: false,
            record_detail: false,
            tree_label: tree_label.map(String::from),
            shortcuts: HashMap::new(),
            enter_action: None,
            recursive: false,
            editor_in_place: false,
            leaf_glyph: None,
            group_by: None,
            aggregates: Vec::new(),
            mark_read_on_reach_end: None,
        }
    }

    fn view(tree_label: Option<&str>, columns: Vec<ColumnDef>, children: Vec<ChildDef>) -> ViewDef {
        ViewDef {
            row_layout: None,
            smooth_scroll: false,
            name: "root".into(),
            node_type: "mock:root".into(),
            default: true,
            window_ops: false,
            key: None,
            query: None,
            columns,
            preview: None,
            actions: Vec::new(),
            children,
            pagination: None,
            action_chains: ActionChains::default(),
            column_cursor: false,
            record_detail: false,
            tree_label: tree_label.map(String::from),
            retries: 0,
            script_template: None,
            shortcuts: HashMap::new(),
            leaf_glyph: None,
            group_by: None,
            aggregates: Vec::new(),
            tree_connector_style: None,
            unread_style: None,
            unread_marker: None,
            tree_lines: None,
            tree_markers: None,
            expand_depth: None,
            group_headers: None,
        }
    }

    fn typed_node(id: &str, label: &str, type_id: &str) -> NodeSummary {
        NodeSummary {
            id: id.into(),
            label: label.into(),
            node_type: NodeType {
                type_id: type_id.into(),
                mime_type: "text/plain".into(),
                syntax: None,
                file_extension: ".txt".into(),
                display_name: "Mock".into(),
            },
            metadata: Metadata::default(),
            has_children: None,
        }
    }

    /// Legacy helper used by tests that don't exercise multi-branch
    /// lookups. The `"mock"` placeholder type means
    /// `child_def_for_type_chain` returns `None`, so chain-aware
    /// callers treat the entry as a leaf — fine for tests that only
    /// check structural flatten behaviour.
    fn node(id: &str, label: &str) -> NodeSummary {
        typed_node(id, label, "mock")
    }

    #[test]
    fn tree_level_at_depth_walks_chain() {
        let leaf = child("table", None, vec![col("name")], Vec::new());
        let schema = child("schema", Some("name"), vec![col("name")], vec![leaf]);
        let v = view(Some("name"), vec![col("name")], vec![schema]);
        let l0 = tree_level_at_depth(&v, 0).unwrap();
        assert_eq!(l0.tree_label, "name");
        assert_eq!(l0.columns.len(), 1);
        let l1 = tree_level_at_depth(&v, 1).unwrap();
        assert_eq!(l1.tree_label, "name");
        // Depth 2: schema's first child has no tree_label → chain ends.
        assert!(tree_level_at_depth(&v, 2).is_none());
    }

    #[test]
    fn tree_level_at_depth_returns_none_without_tree_label_root() {
        let v = view(None, vec![col("name")], Vec::new());
        assert!(tree_level_at_depth(&v, 0).is_none());
    }

    #[test]
    fn rebuild_entries_flattens_root_only_when_collapsed() {
        // Two-level tree: view root → schema (depth-0 entries) → table.
        // Depth-0 entries must carry the schema's node_type so the
        // chain walker resolves them; "table" gives schema a
        // tree-continuing child, so has_children is true.
        let table = child("table", Some("name"), vec![col("name")], Vec::new());
        let schema = child("schema", Some("name"), vec![col("name")], vec![table]);
        let v = view(Some("name"), vec![col("name")], vec![schema]);
        let mut t = TreeState::new();
        t.set_cached_children(
            Vec::new(),
            vec![
                typed_node("db1", "db1", "mock:schema"),
                typed_node("db2", "db2", "mock:schema"),
            ],
            None,
        );
        t.rebuild_entries(&v);
        assert_eq!(t.entries.len(), 2);
        assert_eq!(t.entries[0].depth, 0);
        assert_eq!(t.entries[0].node_type_chain, vec!["mock:schema".to_string()]);
        assert!(t.entries[0].has_children);
        assert_eq!(t.entries[0].node.id, "db1");
    }

    #[test]
    fn rebuild_entries_descends_only_into_expanded() {
        let table = child("table", Some("name"), vec![col("name")], Vec::new());
        let schema = child("schema", Some("name"), vec![col("name")], vec![table]);
        let v = view(Some("name"), vec![col("name")], vec![schema]);
        let mut t = TreeState::new();
        t.set_cached_children(
            Vec::new(),
            vec![
                typed_node("db1", "db1", "mock:schema"),
                typed_node("db2", "db2", "mock:schema"),
            ],
            None,
        );
        t.set_cached_children(
            vec!["db1".into()],
            vec![typed_node("public", "public", "mock:table")],
            None,
        );
        t.expanded.insert(vec!["db1".into()]);
        t.rebuild_entries(&v);
        // db1, then its child public, then db2.
        assert_eq!(t.entries.len(), 3);
        assert_eq!(t.entries[0].node.id, "db1");
        assert_eq!(t.entries[0].depth, 0);
        assert_eq!(t.entries[1].node.id, "public");
        assert_eq!(t.entries[1].depth, 1);
        assert_eq!(t.entries[1].parent_path, vec!["db1".to_string()]);
        assert_eq!(
            t.entries[1].node_type_chain,
            vec!["mock:schema".to_string(), "mock:table".to_string()]
        );
        assert_eq!(t.entries[2].node.id, "db2");
    }

    /// A two-level tree where `db1` is expanded with two children;
    /// removing one drops only that row and keeps the sibling, the
    /// parent's expansion, and the parent's glyph.
    #[test]
    fn remove_node_drops_row_and_keeps_siblings() {
        let table = child("table", Some("name"), vec![col("name")], Vec::new());
        let schema = child("schema", Some("name"), vec![col("name")], vec![table]);
        let v = view(Some("name"), vec![col("name")], vec![schema]);
        let mut t = TreeState::new();
        t.set_cached_children(
            Vec::new(),
            vec![typed_node("db1", "db1", "mock:schema")],
            None,
        );
        t.set_cached_children(
            vec!["db1".into()],
            vec![
                typed_node("public", "public", "mock:table"),
                typed_node("private", "private", "mock:table"),
            ],
            None,
        );
        t.expanded.insert(vec!["db1".into()]);
        t.rebuild_entries(&v);

        assert!(t.remove_node("public"));
        t.rebuild_entries(&v);

        let ids: Vec<&str> = t.entries.iter().map(|e| e.node.id.as_str()).collect();
        assert_eq!(ids, vec!["db1", "private"]);
        // Parent stays expanded; its glyph (has_children) stays on.
        assert!(t.expanded.contains(&vec!["db1".to_string()]));
        assert!(t.entries[0].has_children);
    }

    /// Removing the parent's *last* child flips the parent's own
    /// `has_children` to `false` so the expand glyph disappears — the
    /// local counterpart of the reload path's leaf-vs-frontier fix.
    #[test]
    fn remove_last_child_clears_parent_glyph() {
        let table = child("table", Some("name"), vec![col("name")], Vec::new());
        let schema = child("schema", Some("name"), vec![col("name")], vec![table]);
        let v = view(Some("name"), vec![col("name")], vec![schema]);
        let mut t = TreeState::new();
        let mut db1 = typed_node("db1", "db1", "mock:schema");
        db1.has_children = Some(true);
        t.set_cached_children(Vec::new(), vec![db1], None);
        t.set_cached_children(
            vec!["db1".into()],
            vec![typed_node("public", "public", "mock:table")],
            None,
        );
        t.expanded.insert(vec!["db1".into()]);
        t.rebuild_entries(&v);

        assert!(t.remove_node("public"));
        t.rebuild_entries(&v);

        let ids: Vec<&str> = t.entries.iter().map(|e| e.node.id.as_str()).collect();
        assert_eq!(ids, vec!["db1"]);
        // db1 is now a leaf: glyph cleared.
        assert!(!t.entries[0].has_children);
    }

    /// Removing a node prunes its whole cached subtree (deeper
    /// `expanded` paths + `cache` entries beneath it).
    #[test]
    fn remove_node_prunes_subtree() {
        let table = child("table", Some("name"), vec![col("name")], Vec::new());
        let schema = child("schema", Some("name"), vec![col("name")], vec![table]);
        let v = view(Some("name"), vec![col("name")], vec![schema]);
        let mut t = TreeState::new();
        t.set_cached_children(
            Vec::new(),
            vec![typed_node("db1", "db1", "mock:schema")],
            None,
        );
        t.set_cached_children(
            vec!["db1".into()],
            vec![typed_node("public", "public", "mock:table")],
            None,
        );
        // A deeper level cached + expanded under db1/public.
        t.set_cached_children(
            vec!["db1".into(), "public".into()],
            vec![typed_node("t1", "t1", "mock:leaf")],
            None,
        );
        t.expanded.insert(vec!["db1".into()]);
        t.expanded.insert(vec!["db1".into(), "public".into()]);
        t.rebuild_entries(&v);

        assert!(t.remove_node("public"));

        assert!(!t.cache.contains_key(&vec!["db1".to_string(), "public".to_string()]));
        assert!(!t
            .expanded
            .contains(&vec!["db1".to_string(), "public".to_string()]));
    }

    #[test]
    fn remove_node_returns_false_for_unknown() {
        let v = view(Some("name"), vec![col("name")], Vec::new());
        let mut t = TreeState::new();
        t.set_cached_children(
            Vec::new(),
            vec![typed_node("db1", "db1", "mock:schema")],
            None,
        );
        t.rebuild_entries(&v);
        assert!(!t.remove_node("ghost"));
    }

    #[test]
    fn tree_child_def_at_depth_resolves_continuing_child() {
        let leaf_split = child("table", None, vec![col("name")], Vec::new());
        let tree_table = child("table_tree", Some("name"), vec![col("name")], Vec::new());
        let schema = child(
            "schema",
            Some("name"),
            vec![col("name")],
            vec![leaf_split, tree_table],
        );
        let v = view(Some("name"), vec![col("name")], vec![schema]);
        // Depth 0 → schema (the tree-continuing child of view root).
        let c0 = tree_child_def_at_depth(&v, 0).unwrap();
        assert_eq!(c0.name, "schema");
        // Depth 1 → table_tree (the only tree-continuing sibling).
        let c1 = tree_child_def_at_depth(&v, 1).unwrap();
        assert_eq!(c1.name, "table_tree");
        // Depth 2 → no tree continuation.
        assert!(tree_child_def_at_depth(&v, 2).is_none());
    }

    #[test]
    fn tree_level_children_includes_leaf_siblings() {
        let leaf_split = child("rows", None, vec![col("name")], Vec::new());
        let tree_table = child("table_tree", Some("name"), vec![col("name")], Vec::new());
        let schema = child(
            "schema",
            Some("name"),
            vec![col("name")],
            vec![leaf_split, tree_table],
        );
        let v = view(Some("name"), vec![col("name")], vec![schema]);
        let kids_at_1 = tree_level_children(&v, 1).unwrap();
        assert_eq!(kids_at_1.len(), 2);
        assert!(kids_at_1.iter().any(|c| c.name == "rows" && c.tree_label.is_none()));
    }

    #[test]
    fn tree_level_children_none_for_non_tree_view() {
        let v = view(None, vec![col("name")], Vec::new());
        assert!(tree_level_children(&v, 0).is_none());
    }

    /// Live `NodeSummary.has_children` overrides the static config check
    /// (DSF-3 `recursive: true` would otherwise force `▶` on every row
    /// of a self-recursive ChildDef). The Confluence adapter sets this
    /// from `childTypes.page.value`; the renderer must respect it.
    #[test]
    fn flatten_uses_node_summary_has_children_override() {
        let mut pages = child("pages", Some("name"), vec![col("name")], Vec::new());
        pages.node_type = "mock:page".into();
        pages.recursive = true;
        let v = view(Some("name"), vec![col("name")], vec![pages]);
        let mut t = TreeState::new();
        let mut leaf_summary = typed_node("p1", "Leaf page", "mock:page");
        leaf_summary.has_children = Some(false);
        let mut expandable_summary = typed_node("p2", "Has subpages", "mock:page");
        expandable_summary.has_children = Some(true);
        t.set_cached_children(
            Vec::new(),
            vec![leaf_summary, expandable_summary],
            None,
        );
        t.rebuild_entries(&v);
        assert_eq!(t.entries.len(), 2);
        assert!(!t.entries[0].has_children, "explicit Some(false) wins over recursive");
        assert!(t.entries[1].has_children, "explicit Some(true) keeps it expandable");
    }

    /// A node whose only child opens via drill/split (no `tree_label`)
    /// must NOT get an expand arrow, even when the adapter reports
    /// `has_children = Some(true)`. This is the Stoat channel case: the
    /// channel's `list()` yields messages (so the adapter says "true"),
    /// but `stoat:message` has no `tree_label` — it drills into a split
    /// pane. There is nothing to unfold inline, so the config gate wins.
    #[test]
    fn drill_only_child_gets_no_expand_arrow_despite_adapter_has_children() {
        // channels (tree_label) → messages (no tree_label = drill/split)
        let messages = child("messages", None, vec![col("body")], Vec::new());
        let channels = child("channels", Some("name"), vec![col("name")], vec![messages]);
        let v = view(Some("name"), vec![col("name")], vec![channels]);
        let mut t = TreeState::new();
        let mut channel = typed_node("c1", "general", "mock:channels");
        channel.has_children = Some(true); // adapter: "drillable to messages"
        t.set_cached_children(Vec::new(), vec![channel], None);
        t.rebuild_entries(&v);
        assert_eq!(t.entries.len(), 1);
        assert!(
            !t.entries[0].has_children,
            "messages have no tree_label → channel is not inline-expandable"
        );
        assert_ne!(
            tree_row_glyph(&t.entries[0], &t, &v),
            "▶",
            "a drill-only node must not show the expand arrow"
        );
    }

    /// Configured `leaf_glyph` is picked up via the producing ChildDef
    /// (depth>0) or the ViewDef root (depth 0). Falls back to `·` when
    /// neither level configures one.
    #[test]
    fn leaf_glyph_resolves_from_view_and_child_config() {
        let mut pages = child("pages", Some("name"), vec![col("name")], Vec::new());
        pages.node_type = "mock:page".into();
        pages.recursive = true;
        pages.leaf_glyph = Some("📄".into());
        let mut v = view(Some("name"), vec![col("name")], vec![pages]);
        v.leaf_glyph = Some("∅".into());

        let t = TreeState::new();
        // Root-level entry → ViewDef.leaf_glyph wins.
        let root_leaf = TreeEntry {
            depth: 0,
            parent_path: Vec::new(),
            node_type_chain: vec!["mock:root".into()],
            node: typed_node("r", "r", "mock:root"),
            has_children: false,
            is_more_placeholder: false,
        };
        assert_eq!(tree_row_glyph(&root_leaf, &t, &v), "∅");

        // Page-level entry → ChildDef.leaf_glyph wins.
        let page_leaf = TreeEntry {
            depth: 1,
            parent_path: vec!["r".into()],
            node_type_chain: vec!["mock:root".into(), "mock:page".into()],
            node: typed_node("p", "p", "mock:page"),
            has_children: false,
            is_more_placeholder: false,
        };
        assert_eq!(tree_row_glyph(&page_leaf, &t, &v), "📄");

        // Stripping both → universal `·` fallback.
        v.leaf_glyph = None;
        v.children[0].leaf_glyph = None;
        assert_eq!(tree_row_glyph(&page_leaf, &t, &v), "·");
    }

    #[test]
    fn glyph_reflects_state() {
        let mut t = TreeState::new();
        let v = view(Some("name"), vec![col("name")], Vec::new());
        let leaf_entry = TreeEntry {
            depth: 0,
            parent_path: Vec::new(),
            node_type_chain: vec!["mock".into()],
            node: node("x", "x"),
            has_children: false,
            is_more_placeholder: false,
        };
        assert_eq!(tree_row_glyph(&leaf_entry, &t, &v), "·");

        let collapsed = TreeEntry { has_children: true, ..leaf_entry.clone() };
        assert_eq!(tree_row_glyph(&collapsed, &t, &v), "▶");

        t.expanded.insert(vec!["x".into()]);
        assert_eq!(tree_row_glyph(&collapsed, &t, &v), "▼");

        let placeholder = TreeEntry { is_more_placeholder: true, ..leaf_entry };
        assert_eq!(tree_row_glyph(&placeholder, &t, &v), "…");
    }

    #[test]
    fn flatten_emits_more_placeholder_for_paginated_parent() {
        let table = child("table", Some("name"), vec![col("name")], Vec::new());
        let schema = child("schema", Some("name"), vec![col("name")], vec![table]);
        let v = view(Some("name"), vec![col("name")], vec![schema]);
        let mut t = TreeState::new();
        t.set_cached_children(
            Vec::new(),
            vec![typed_node("db1", "db1", "mock:schema")],
            None,
        );
        // db1's children are paginated: arm next_page.
        let next = PageRequest { offset: 2, limit: 2 };
        t.set_cached_children(
            vec!["db1".into()],
            vec![
                typed_node("public", "public", "mock:table"),
                typed_node("audit", "audit", "mock:table"),
            ],
            Some(next),
        );
        t.expanded.insert(vec!["db1".into()]);
        t.rebuild_entries(&v);
        // Entries: db1, public, audit, <placeholder>
        assert_eq!(t.entries.len(), 4);
        assert!(t.entries[3].is_more_placeholder);
        assert_eq!(t.entries[3].depth, 1);
        assert_eq!(t.entries[3].parent_path, vec!["db1".to_string()]);
        assert_eq!(tree_row_glyph(&t.entries[3], &t, &v), "…");
    }

    #[test]
    fn extend_cached_children_appends_and_rebuilds_placeholder() {
        let table = child("table", Some("name"), vec![col("name")], Vec::new());
        let schema = child("schema", Some("name"), vec![col("name")], vec![table]);
        let v = view(Some("name"), vec![col("name")], vec![schema]);
        let mut t = TreeState::new();
        t.set_cached_children(
            Vec::new(),
            vec![typed_node("db1", "db1", "mock:schema")],
            None,
        );
        t.set_cached_children(
            vec!["db1".into()],
            vec![typed_node("a", "a", "mock:table")],
            Some(PageRequest { offset: 1, limit: 1 }),
        );
        t.expanded.insert(vec!["db1".into()]);
        t.rebuild_entries(&v);
        assert_eq!(t.entries.len(), 3); // db1, a, <more>
        assert!(t.entries[2].is_more_placeholder);

        // Activate placeholder: caller appends next slice. Final page —
        // no further next_page, so the placeholder should disappear.
        t.extend_cached_children(
            vec!["db1".into()],
            vec![typed_node("b", "b", "mock:table")],
            None,
        );
        t.rebuild_entries(&v);
        assert_eq!(t.entries.len(), 3); // db1, a, b
        assert!(!t.entries[2].is_more_placeholder);
        assert_eq!(t.entries[2].node.id, "b");
    }

    // ---- Multi-branch (MT-1) helpers --------------------------------

    #[test]
    fn child_def_for_type_chain_walks_multi_branch() {
        // view → database → {schemas → schema, db_scripts → db_script}
        let schema = child("schema", Some("name"), vec![col("name")], Vec::new());
        let db_script = child("db_script", None, vec![col("name")], Vec::new());
        let schemas = child("schemas", Some("name"), vec![col("name")], vec![schema]);
        let db_scripts = child(
            "db_scripts",
            Some("name"),
            vec![col("name")],
            vec![db_script],
        );
        let database = child(
            "database",
            Some("name"),
            vec![col("name")],
            vec![schemas, db_scripts],
        );
        let v = view(Some("name"), vec![col("name")], vec![database]);

        // Walk to the schemas branch by node_type chain.
        let schemas_def =
            child_def_for_type_chain(&v, &["mock:database".into(), "mock:schemas".into()]).unwrap();
        assert_eq!(schemas_def.name, "schemas");

        // And the db_scripts branch.
        let scripts_def = child_def_for_type_chain(
            &v,
            &["mock:database".into(), "mock:db_scripts".into()],
        )
        .unwrap();
        assert_eq!(scripts_def.name, "db_scripts");

        // Unknown type at depth 1 returns None.
        assert!(child_def_for_type_chain(&v, &["mock:database".into(), "mock:nope".into()]).is_none());
    }

    #[test]
    fn flatten_uses_per_entry_chain_for_heterogeneous_children() {
        // Multi-branch under database: a Schemas-group entry and a
        // DB-Scripts-group entry sit at the same depth as siblings.
        let schema = child("schema", Some("name"), vec![col("name")], Vec::new());
        let db_script = child("db_script", None, vec![col("name")], Vec::new());
        let schemas = child("schemas", Some("name"), vec![col("name")], vec![schema]);
        let db_scripts = child(
            "db_scripts",
            Some("name"),
            vec![col("name")],
            vec![db_script],
        );
        let database = child(
            "database",
            Some("name"),
            vec![col("name")],
            vec![schemas, db_scripts],
        );
        let v = view(Some("name"), vec![col("name")], vec![database]);
        let mut t = TreeState::new();
        // depth-0: one database
        t.set_cached_children(
            Vec::new(),
            vec![typed_node("db1", "db1", "mock:database")],
            None,
        );
        // depth-1 under db1: two siblings with different node_types
        t.set_cached_children(
            vec!["db1".into()],
            vec![
                typed_node("db1:schemas", "Schemas", "mock:schemas"),
                typed_node("db1:scripts", "Scripts", "mock:db_scripts"),
            ],
            None,
        );
        t.expanded.insert(vec!["db1".into()]);
        t.rebuild_entries(&v);
        // Entries: db1, Schemas, Scripts.
        assert_eq!(t.entries.len(), 3);
        assert_eq!(t.entries[1].node_type_chain.last().unwrap(), "mock:schemas");
        // Schemas group has tree-continuing child (Schema) → has_children = true.
        assert!(t.entries[1].has_children);
        assert_eq!(t.entries[2].node_type_chain.last().unwrap(), "mock:db_scripts");
        // DB-Scripts group has tree-continuing child (db_script with no tree_label) → false here,
        // because db_script has no tree_label, so has_tree_continuation returns false.
        assert!(!t.entries[2].has_children);
    }

    #[test]
    fn multi_load_merges_buckets_in_expected_order() {
        let schemas = child("schemas", Some("name"), vec![col("name")], Vec::new());
        let db_scripts = child(
            "db_scripts",
            Some("name"),
            vec![col("name")],
            Vec::new(),
        );
        let database = child(
            "database",
            Some("name"),
            vec![col("name")],
            vec![schemas, db_scripts],
        );
        let v = view(Some("name"), vec![col("name")], vec![database]);
        let mut t = TreeState::new();
        t.set_cached_children(
            Vec::new(),
            vec![typed_node("db1", "db1", "mock:database")],
            None,
        );
        // Begin multi-load with two expected types in YAML order.
        t.begin_multi_load(
            vec!["db1".into()],
            vec!["mock:schemas".into(), "mock:db_scripts".into()],
        );
        // Land the SECOND type first — merge order must still follow
        // `expected_types`.
        t.apply_multi_load_result(
            vec!["db1".into()],
            "mock:db_scripts".into(),
            vec![typed_node("scripts_group", "Scripts", "mock:db_scripts")],
        );
        // Still pending the first type → not yet loaded.
        let state = t.cache.get(&vec!["db1".to_string()]).unwrap();
        assert!(!state.loaded);
        assert_eq!(state.children.len(), 1);
        assert_eq!(state.children[0].label, "Scripts");
        // Land the first type → buckets re-merged in YAML order.
        t.apply_multi_load_result(
            vec!["db1".into()],
            "mock:schemas".into(),
            vec![typed_node("schemas_group", "Schemas", "mock:schemas")],
        );
        let state = t.cache.get(&vec!["db1".to_string()]).unwrap();
        assert!(state.loaded);
        assert_eq!(state.children.len(), 2);
        assert_eq!(state.children[0].label, "Schemas");
        assert_eq!(state.children[1].label, "Scripts");
    }

    #[test]
    fn tree_level_for_chain_resolves_branch_columns() {
        // Two branches with different columns to ensure the right
        // TreeLevel comes back per chain.
        let schemas = child("schemas", Some("name"), vec![col("name")], Vec::new());
        let db_scripts = child(
            "db_scripts",
            Some("name"),
            vec![col("name"), col("size")],
            Vec::new(),
        );
        let database = child(
            "database",
            Some("name"),
            vec![col("name")],
            vec![schemas, db_scripts],
        );
        let v = view(Some("name"), vec![col("name")], vec![database]);
        let schemas_level = tree_level_for_chain(
            &v,
            &["mock:database".into(), "mock:schemas".into()],
        )
        .unwrap();
        assert_eq!(schemas_level.columns.len(), 1);
        let scripts_level = tree_level_for_chain(
            &v,
            &["mock:database".into(), "mock:db_scripts".into()],
        )
        .unwrap();
        assert_eq!(scripts_level.columns.len(), 2);
    }

    // ---- View-root prefix in chains (production-shape YAML) ----------
    // YAML without a wrapper ChildDef under the ViewDef looks like:
    //
    //   views:
    //     - node_type: postgres:database
    //       tree_label: name
    //       children:
    //         - node_type: postgres:schema       # depth-1 tree continuation
    //           tree_label: name
    //           children:
    //             - node_type: postgres:table    # depth-2 tree continuation
    //               tree_label: name
    //               children:
    //                 - node_type: postgres:row  # leaf-drill (no tree_label)
    //         - node_type: postgres:db_script    # depth-1 second branch
    //           tree_label: script
    //
    // The adapter returns depth-0 rows with node_type ==
    // view_def.node_type. The chain walker must strip that root prefix
    // so depth-1 entries (schemas) and depth-2 entries (tables) still
    // resolve their producing ChildDef and get the right glyph.

    fn view_with_node_type(
        node_type: &str,
        tree_label: Option<&str>,
        columns: Vec<ColumnDef>,
        children: Vec<ChildDef>,
    ) -> ViewDef {
        let mut v = view(tree_label, columns, children);
        v.node_type = node_type.into();
        v
    }

    fn child_with_type(
        name: &str,
        node_type: &str,
        tree_label: Option<&str>,
        columns: Vec<ColumnDef>,
        children: Vec<ChildDef>,
    ) -> ChildDef {
        let mut c = child(name, tree_label, columns, children);
        c.node_type = node_type.into();
        c
    }

    #[test]
    fn child_def_for_type_chain_strips_view_root_prefix() {
        // ViewDef = postgres:database; children = [Schema(postgres:schema)].
        let table = child_with_type(
            "table",
            "postgres:table",
            Some("name"),
            vec![col("name")],
            Vec::new(),
        );
        let schema = child_with_type(
            "schema",
            "postgres:schema",
            Some("name"),
            vec![col("name")],
            vec![table],
        );
        let v = view_with_node_type(
            "postgres:database",
            Some("name"),
            vec![col("name")],
            vec![schema],
        );
        // depth-0 chain (root row, type == view root) → no producing ChildDef.
        assert!(child_def_for_type_chain(&v, &["postgres:database".into()]).is_none());
        // depth-1 chain (schema) → resolves Schema ChildDef even though
        // the chain starts with the view-root type.
        let resolved = child_def_for_type_chain(
            &v,
            &["postgres:database".into(), "postgres:schema".into()],
        )
        .unwrap();
        assert_eq!(resolved.name, "schema");
        // depth-2 chain (table) → resolves Table ChildDef.
        let resolved = child_def_for_type_chain(
            &v,
            &[
                "postgres:database".into(),
                "postgres:schema".into(),
                "postgres:table".into(),
            ],
        )
        .unwrap();
        assert_eq!(resolved.name, "table");
    }

    #[test]
    fn flatten_has_children_through_view_root_prefix() {
        // Real-world shape: chain starts with view-root type, no
        // wrapper ChildDef. Glyph must be ▶/▼ on depth-0 and depth-1
        // expandable rows, `·` only on the true leaf (table, since its
        // child Rows has no tree_label).
        let rows = child_with_type(
            "rows",
            "postgres:row",
            None,
            vec![col("name")],
            Vec::new(),
        );
        let table = child_with_type(
            "table",
            "postgres:table",
            Some("name"),
            vec![col("name")],
            vec![rows],
        );
        let schema = child_with_type(
            "schema",
            "postgres:schema",
            Some("name"),
            vec![col("name")],
            vec![table],
        );
        let v = view_with_node_type(
            "postgres:database",
            Some("name"),
            vec![col("name")],
            vec![schema],
        );

        let mut t = TreeState::new();
        // depth-0: one database with the view-root type.
        t.set_cached_children(
            Vec::new(),
            vec![typed_node("db1", "db1", "postgres:database")],
            None,
        );
        // Expand db1 → load schemas.
        t.set_cached_children(
            vec!["db1".into()],
            vec![typed_node("public", "public", "postgres:schema")],
            None,
        );
        t.expanded.insert(vec!["db1".into()]);
        // Expand public → load tables.
        t.set_cached_children(
            vec!["db1".into(), "public".into()],
            vec![typed_node("users", "users", "postgres:table")],
            None,
        );
        t.expanded.insert(vec!["db1".into(), "public".into()]);
        t.rebuild_entries(&v);

        // db1 (depth 0, has tree-continuing child Schema) → ▶/▼.
        assert!(t.entries[0].has_children, "db1 should be expandable");
        // public (depth 1, has tree-continuing child Table) → ▶/▼.
        assert!(t.entries[1].has_children, "public schema should be expandable");
        // users (depth 2, child Rows has no tree_label) → leaf glyph.
        assert!(
            !t.entries[2].has_children,
            "table is a tree-leaf (drill-down, not tree-continuation)"
        );
    }

    /// DSF-3: walker on a recursive ChildDef stays on the same def for
    /// each same-type segment in the chain. Without the recursive
    /// branch the walker would try (and fail) to find the type under
    /// `children:` after the first hop.
    #[test]
    fn walker_recursive_stays_on_current_for_same_type_chain() {
        let mut leaf = child("leaf", None, vec![col("name")], Vec::new());
        leaf.node_type = "mock:leaf".into();
        let mut dir = child("dir", Some("name"), vec![col("name")], vec![leaf]);
        dir.node_type = "mock:dir".into();
        dir.recursive = true;
        let v = view(Some("name"), vec![col("name")], vec![dir]);
        // Chain `[dir, dir, dir]` — 3 deep, all same type. Walker
        // must resolve to the `dir` ChildDef without descending into
        // its declared `children:`.
        let chain = vec!["mock:dir".to_string(); 3];
        let resolved = child_def_for_type_chain(&v, &chain).expect("dir def");
        assert_eq!(resolved.node_type, "mock:dir");
        // Now mix: `[dir, dir, leaf]` — the last segment leaves
        // recursion and finds `leaf` under declared children.
        let chain = vec![
            "mock:dir".to_string(),
            "mock:dir".to_string(),
            "mock:leaf".to_string(),
        ];
        let resolved = child_def_for_type_chain(&v, &chain).expect("leaf def");
        assert_eq!(resolved.node_type, "mock:leaf");
    }

    /// DSF-3: `effective_child_children` prepends self for recursive
    /// defs and leaves non-recursive defs untouched.
    #[test]
    fn effective_child_children_prepends_self_for_recursive() {
        let leaf = child("leaf", None, vec![col("name")], Vec::new());
        let mut dir = child(
            "dir",
            Some("name"),
            vec![col("name")],
            vec![leaf.clone()],
        );
        dir.recursive = true;
        let effective = effective_child_children(&dir);
        assert_eq!(effective.len(), 2);
        assert_eq!(effective[0].name, "dir");
        assert_eq!(effective[1].name, "leaf");
        // Non-recursive: returns only declared children.
        let mut dir2 = dir.clone();
        dir2.recursive = false;
        let effective = effective_child_children(&dir2);
        assert_eq!(effective.len(), 1);
        assert_eq!(effective[0].name, "leaf");
    }

    /// DSF-3: `has_tree_continuation_for_chain` treats a recursive
    /// ChildDef as its own tree-continuing child — required for the
    /// `▶`/`▼` glyph to appear on a directory whose only declared
    /// children are leaves.
    #[test]
    fn has_tree_continuation_for_chain_honors_recursive_self() {
        let mut leaf = child("leaf", None, vec![col("name")], Vec::new());
        leaf.node_type = "mock:leaf".into();
        let mut dir = child("dir", Some("name"), vec![col("name")], vec![leaf]);
        dir.node_type = "mock:dir".into();
        dir.recursive = true;
        let v = view(Some("name"), vec![col("name")], vec![dir]);
        // Chain `[dir]` — the recursive def itself. Without DSF-3 the
        // check would consult only declared children (which has no
        // tree-continuing entry) and return false. With DSF-3 self
        // counts as tree-continuing → true.
        let chain = vec!["mock:dir".to_string()];
        assert!(has_tree_continuation_for_chain(&v, &chain));
    }

    /// Regression (tasks adapter, "sub-levels past the 2nd don't unfold"):
    /// in a *uniform* recursive tree the view-root node_type equals the
    /// recursive child's node_type (here `task:item`/`task:item`). A depth
    /// ≥1 entry's `node_type_chain` then carries the root type twice. The
    /// chain-aware helpers must strip the root prefix exactly once —
    /// `child_def_for_type_chain` already strips it, so a caller that
    /// pre-strips double-strips and eats the child segment, resolving every
    /// level below the root to `None`. That blanked the `▶`/`▼` expand glyph
    /// for every node below the root, so sub-levels couldn't be unfolded.
    /// Distinct from
    /// `has_tree_continuation_for_chain_honors_recursive_self`, which uses
    /// distinct root/child types and a single-element chain → never triggers
    /// the double-strip.
    #[test]
    fn uniform_recursive_tree_keeps_expand_glyph_below_root() {
        let mut item = child("item", Some("name"), vec![col("name")], Vec::new());
        item.node_type = "task:item".into();
        item.recursive = true;
        let mut v = view(Some("name"), vec![col("name")], vec![item]);
        v.node_type = "task:item".into();

        // Depth 0 (chain == [root]) always worked; depths ≥1 are the regression.
        let d0 = vec!["task:item".to_string()];
        let d1 = vec!["task:item".to_string(), "task:item".to_string()];
        let d2 = vec!["task:item".to_string(); 3];

        assert!(has_tree_continuation_for_chain(&v, &d0));
        assert!(
            has_tree_continuation_for_chain(&v, &d1),
            "depth-1 node in a uniform recursive tree must stay expandable"
        );
        assert!(
            has_tree_continuation_for_chain(&v, &d2),
            "self-similar tree stays expandable at arbitrary depth"
        );

        // The children-resolver (drives the glyph hint and the expand
        // fan-out) must resolve the recursive child, not `None`.
        assert!(tree_level_children_for_chain(&v, &d1).is_some());
        assert!(tree_level_children_for_chain(&v, &d2).is_some());
    }

    /// Regression: the first-chain depth walkers (`tree_level_at_depth`,
    /// `tree_self_at_depth`, `tree_level_children` — still used by the
    /// tree-find dispatch and the children fallback) must follow DSF-3
    /// self-recursion too, not just the chain-aware variants.
    ///
    /// Reproduces the Confluence "blank rows under expanded page"
    /// symptom: a recursive `pages` ChildDef with two non-tree-
    /// continuing siblings (`attachments`, `comments`). At depth 2 the
    /// walker used to descend into `pages.children` and pick the first
    /// `tree_label`-bearing entry there — but `attachments`/`comments`
    /// have no `tree_label`, so it returned `None`. (The renderer itself
    /// no longer resolves the label cell by depth — it keys off the
    /// entry's `node_type_chain` — but these walkers still feed other
    /// chain-blind paths, so the recursion fix stays load-bearing.)
    #[test]
    fn first_chain_walkers_honor_recursive_through_leaf_siblings() {
        let attachments = child("attachments", None, vec![col("filename")], Vec::new());
        let comments = child("comments", None, vec![col("author")], Vec::new());
        let mut pages = child(
            "pages",
            Some("name"),
            vec![col("name"), col("id")],
            vec![attachments, comments],
        );
        pages.node_type = "mock:page".into();
        pages.recursive = true;
        let v = view(Some("name"), vec![col("name")], vec![pages]);

        // Depth 1: pages level. Already worked pre-fix.
        let l1 = tree_level_at_depth(&v, 1).expect("depth 1 resolves");
        assert_eq!(l1.tree_label, "name");

        // Depth 2: same pages level via recursion. Pre-fix this
        // returned `None` because the walker stepped into
        // `pages.children` = [attachments, comments] and found no
        // tree_label there.
        let l2 = tree_level_at_depth(&v, 2).expect("depth 2 resolves via recursion");
        assert_eq!(l2.tree_label, "name");
        let key = l2.columns.first().map(|c| c.key.as_str());
        assert_eq!(key, Some("name"), "columns carry through to depth 2");

        // Depth 3 too — recursion is unbounded.
        let l3 = tree_level_at_depth(&v, 3).expect("depth 3 resolves via recursion");
        assert_eq!(l3.tree_label, "name");

        // `tree_self_at_depth` shares the walk — same regression.
        let self2 = tree_self_at_depth(&v, 2).expect("self at depth 2");
        assert_eq!(self2.node_type, "mock:page");

        // `tree_level_children` shares the walk too. Children of the
        // recursive `pages` level are its declared children (attach /
        // comments) — DSF-3 only affects which level we're *on*, not
        // what its declared children are.
        let kids = tree_level_children(&v, 2).expect("children at depth 2");
        let names: Vec<_> = kids.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["attachments", "comments"]);
    }
}
