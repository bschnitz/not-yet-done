//! Generic filterable-forest data structure.
//!
//! This crate provides tree-specific data structures and rendering:
//!
//! - [`TreeNode`] / [`HasTreeShape`] — building blocks for any tree
//! - [`ForestItem`] — trait for filter-matching
//! - [`Forest<T, S>`] — an immutable forest
//! - [`GhostNode`] — a borrowed, transformed view into a subtree (with highlight ranges)
//! - [`TransformableForest<Q>`] — produce a `GhostNode` forest from a query
//! - [`TreeDisplay`] — optional per-node label for the tree column
//! - [`IntoRow`] — convert an element into non-tree [`Row`] cells
//! - [`RenderableTree<Q>`] — renders the tree column and produces [`TreeCellRow`]s
//! - [`render_table`] — fits all columns to width and returns a [`RenderedTable`]
//!
//! Table-generic types (column sizing, cell fitting) are re-exported from
//! [`not_yet_done_table`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;
use std::ops::Range;

// Re-export table-generic types so existing consumers don't break.
pub use not_yet_done_table::{
    ColSizer, ColStrategy, ColumnId, FixedColSizer, MixedColSizer,
    fit_to_width, fit_to_width_with_highlights,
};

// =============================================================================
// TreeNode + HasTreeShape + ForestItem
// =============================================================================

#[derive(Debug, Clone)]
pub struct TreeNode<T> {
    pub element: T,
    pub children: Vec<TreeNode<T>>,
}

impl<T> TreeNode<T> {
    pub fn new(element: T) -> Self {
        TreeNode { element, children: Vec::new() }
    }
}

pub trait HasTreeShape<S> {
    fn id(&self) -> S;
    fn parent_id(&self) -> Option<S>;
}

pub trait ForestItem<Q> {
    fn matches_filter(&self, query: &Q) -> bool;
}

// =============================================================================
// Forest<T, S>
// =============================================================================

pub struct Forest<T, S> {
    roots: Vec<TreeNode<T>>,
    item_to_root: HashMap<S, usize>,
    flat_items: HashMap<S, *const T>,
}

unsafe impl<T: Send, S: Send> Send for Forest<T, S> {}
unsafe impl<T: Sync, S: Sync> Sync for Forest<T, S> {}

impl<T: std::fmt::Debug, S: std::fmt::Debug> std::fmt::Debug for Forest<T, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Forest")
            .field("roots", &self.roots)
            .field("item_count", &self.flat_items.len())
            .finish()
    }
}

impl<T, S> Clone for Forest<T, S>
where
    T: Clone + HasTreeShape<S>,
    S: Eq + Hash + Clone,
{
    fn clone(&self) -> Self {
        let mut items: Vec<T> = Vec::with_capacity(self.flat_items.len());
        for root in &self.roots {
            let mut stack = vec![root];
            while let Some(node) = stack.pop() {
                items.push(node.element.clone());
                stack.extend(node.children.iter());
            }
        }
        Forest::from_items(items)
    }
}

impl<T, S> Forest<T, S>
where
    S: Eq + Hash + Clone,
    T: HasTreeShape<S>,
{
    pub fn from_items(items: Vec<T>) -> Self {
        let n = items.len();

        let mut node_map: HashMap<S, TreeNode<T>> = HashMap::with_capacity(n);
        for item in items {
            node_map.insert(item.id(), TreeNode::new(item));
        }

        let mut children_of: HashMap<S, Vec<S>> = HashMap::with_capacity(n);
        let mut root_ids: Vec<S> = Vec::new();

        for node in node_map.values() {
            match node.element.parent_id() {
                Some(pid) if node_map.contains_key(&pid) => {
                    children_of.entry(pid).or_default().push(node.element.id());
                }
                _ => root_ids.push(node.element.id()),
            }
        }

        let mut topo_order: Vec<S> = Vec::with_capacity(n);
        let mut queue: VecDeque<S> = root_ids.iter().cloned().collect();
        while let Some(id) = queue.pop_front() {
            topo_order.push(id.clone());
            if let Some(kids) = children_of.get(&id) {
                queue.extend(kids.iter().cloned());
            }
        }

        for id in topo_order.iter().rev() {
            if let Some(kids) = children_of.get(id) {
                let kid_ids: Vec<S> = kids.clone();
                for kid_id in kid_ids {
                    if let Some(child_node) = node_map.remove(&kid_id) {
                        if let Some(parent_node) = node_map.get_mut(id) {
                            parent_node.children.push(child_node);
                        }
                    }
                }
            }
        }

        let roots: Vec<TreeNode<T>> = root_ids
            .iter()
            .filter_map(|id| node_map.remove(id))
            .collect();

        let mut item_to_root: HashMap<S, usize> = HashMap::with_capacity(n);
        let mut flat_items: HashMap<S, *const T> = HashMap::with_capacity(n);

        for (root_idx, root) in roots.iter().enumerate() {
            let mut stack = vec![root];
            while let Some(node) = stack.pop() {
                item_to_root.insert(node.element.id(), root_idx);
                flat_items.insert(node.element.id(), &node.element as *const T);
                stack.extend(node.children.iter());
            }
        }

        Forest { roots, item_to_root, flat_items }
    }

    pub fn roots(&self) -> &[TreeNode<T>] {
        &self.roots
    }

    pub fn len(&self) -> usize {
        self.flat_items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    /// Look up an item by its ID. Returns a reference if found.
    pub fn find_item(&self, id: &S) -> Option<&T> {
        self.flat_items.get(id).map(|&ptr| unsafe { &*ptr })
    }
}

// =============================================================================
// GhostNode + TransformableForest
// =============================================================================

pub struct GhostNode<'a, T> {
    pub node: &'a TreeNode<T>,
    pub highlight_ranges: Vec<Range<usize>>,
    pub children: Vec<GhostNode<'a, T>>,
}

pub trait TransformableForest<Q> {
    type Item;
    fn transform<'a>(&'a self, query: &Q) -> Vec<GhostNode<'a, Self::Item>>;
}

impl<T, S, Q> TransformableForest<Q> for Forest<T, S>
where
    S: Eq + Hash + Clone,
    T: HasTreeShape<S> + ForestItem<Q>,
{
    type Item = T;

    fn transform<'a>(&'a self, query: &Q) -> Vec<GhostNode<'a, T>> {
        let mut root_indices: HashSet<usize> = HashSet::new();
        for (id, &ptr) in &self.flat_items {
            let item = unsafe { &*ptr };
            if item.matches_filter(query) {
                if let Some(&root_idx) = self.item_to_root.get(id) {
                    root_indices.insert(root_idx);
                }
            }
        }

        root_indices
            .into_iter()
            .map(|idx| ghost_from_node(&self.roots[idx]))
            .collect()
    }
}

fn ghost_from_node<T>(node: &TreeNode<T>) -> GhostNode<'_, T> {
    GhostNode {
        node,
        highlight_ranges: vec![],
        children: node.children.iter().map(ghost_from_node).collect(),
    }
}

// =============================================================================
// TreeDisplay + IntoRow
// =============================================================================

pub trait TreeDisplay {
    fn description(&self) -> Option<&str>;
}

/// Convert an element into a [`Row`] containing **only the non-tree columns**.
///
/// The tree column (`TREE_COLUMN`) is populated automatically by
/// [`RenderableTree`].
pub trait IntoRow {
    type Id: Eq + Hash + Clone;
    fn into_row(&self) -> Row<Self::Id>;
}

// =============================================================================
// Row — legacy type for forest consumers
// =============================================================================

/// A raw table row with a typed ID and per-column string cells.
///
/// This is the forest-level row type used by [`IntoRow`]. For the generic
/// table type, see [`not_yet_done_table::Row`].
#[derive(Debug, Clone)]
pub struct Row<Id: Eq + Hash> {
    pub id: Id,
    pub cells: HashMap<ColumnId, String>,
}

// =============================================================================
// The fixed tree column id
// =============================================================================

pub const TREE_COLUMN: &str = "tree";

// =============================================================================
// TreeCellRow — output of RenderableTree
// =============================================================================

#[derive(Debug, Clone)]
pub struct TreeCellRow<Id: Eq + Hash + Clone> {
    pub id: Id,
    pub tree_cell: String,
    pub connector_chars: usize,
    pub(crate) highlight_ranges: Vec<Range<usize>>,
}

// =============================================================================
// TreeRenderOptions — per-render expand-state + child counts
// =============================================================================

/// Visibility options for tree rendering. Lets a caller hide subtrees
/// (collapsed nodes) and surface child counts in the connector.
///
/// `is_expanded(id, depth)` decides whether children of the given node
/// are rendered. When it returns `false`, the node itself is rendered
/// with a `▶` glyph and a trailing `(N)` count, where `N` is looked up
/// in `child_counts`. When it returns `true`, the node gets a `▼`
/// glyph and children are recursed into.
///
/// `child_counts` carries the count of *direct* children of each node
/// in the **unfiltered** source forest. Using the unfiltered count
/// keeps the displayed "(N)" stable when a filter narrows the visible
/// ghost tree.
///
/// Use [`TreeRenderOptions::all_visible`] to render every node with no
/// glyph and no `(N)` suffix — this is the legacy behaviour and is
/// what callers get when they use the no-options entry points
/// (`tree_rows`, `tree_min_width`).
pub struct TreeRenderOptions<Id> {
    pub is_expanded: Box<dyn Fn(&Id, usize) -> bool>,
    pub child_counts: HashMap<Id, usize>,
}

impl<Id> TreeRenderOptions<Id> {
    /// Render every node fully expanded, with no glyph or count suffix.
    /// This matches the pre-collapse rendering and is the default for
    /// callers that don't pass options.
    pub fn all_visible() -> Self {
        Self {
            is_expanded: Box::new(|_, _| true),
            child_counts: HashMap::new(),
        }
    }
}

impl<Id> Default for TreeRenderOptions<Id> {
    fn default() -> Self {
        Self::all_visible()
    }
}

/// Bundle of inputs for [`forest_connector`]. Grouped so adding a new
/// dimension (e.g. expand state, glyph) doesn't grow the parameter list.
struct ConnectorSpec<'a> {
    depth: usize,
    is_last: bool,
    prefix: &'a str,
    has_description: bool,
    has_children: bool,
    /// `None` = leaf (no glyph). `Some(true)` = expanded (▼).
    /// `Some(false)` = collapsed (▶).
    expanded: Option<bool>,
}

// =============================================================================
// RenderableTree<Q>
// =============================================================================

pub trait RenderableTree<Q>: TransformableForest<Q>
where
    Self::Item: TreeDisplay,
{
    /// Render every node visible (legacy behaviour).
    fn tree_rows<Id>(
        &self,
        query: &Q,
    ) -> Vec<TreeCellRow<Id>>
    where
        Self::Item: IntoRow<Id = Id>,
        Id: Eq + Hash + Clone,
    {
        self.tree_rows_with_options(query, &TreeRenderOptions::all_visible())
    }

    /// Render with caller-supplied expand state. Collapsed parents are
    /// emitted with a `▶` glyph and `(N)` suffix; children of collapsed
    /// parents are not emitted.
    fn tree_rows_with_options<Id>(
        &self,
        query: &Q,
        options: &TreeRenderOptions<Id>,
    ) -> Vec<TreeCellRow<Id>>
    where
        Self::Item: IntoRow<Id = Id>,
        Id: Eq + Hash + Clone,
    {
        let ghost_roots = self.transform(query);
        let mut result = Vec::new();

        for ghost_root in &ghost_roots {
            let mut stack: Vec<(&GhostNode<'_, Self::Item>, usize, bool, String)> =
                vec![(ghost_root, 0, true, String::new())];

            while let Some((ghost, depth, is_last, prefix)) = stack.pop() {
                let elem = &ghost.node.element;
                let desc = elem.description();
                let has_desc = desc.is_some();
                let has_children = !ghost.children.is_empty();

                let id = elem.into_row().id;
                let expanded_flag = if has_children {
                    Some((options.is_expanded)(&id, depth))
                } else {
                    None
                };

                let connector = forest_connector(ConnectorSpec {
                    depth,
                    is_last,
                    prefix: &prefix,
                    has_description: has_desc,
                    has_children,
                    expanded: expanded_flag,
                });
                let connector_char_len = connector.chars().count();

                let shifted_ranges: Vec<Range<usize>> = ghost
                    .highlight_ranges
                    .iter()
                    .map(|r| {
                        (r.start + connector_char_len)..(r.end + connector_char_len)
                    })
                    .collect();

                let mut tree_cell = match desc {
                    Some(d) => format!("{}{}", connector, d),
                    None => connector,
                };
                if expanded_flag == Some(false) {
                    let n = options
                        .child_counts
                        .get(&id)
                        .copied()
                        .unwrap_or(ghost.children.len());
                    tree_cell.push_str(&format!(" ({})", n));
                }

                result.push(TreeCellRow {
                    id,
                    tree_cell,
                    connector_chars: connector_char_len,
                    highlight_ranges: shifted_ranges,
                });

                if expanded_flag == Some(false) {
                    continue;
                }

                let n = ghost.children.len();
                let next_prefix =
                    forest_child_prefix(depth, is_last, has_desc, &prefix);
                for (i, child) in ghost.children.iter().enumerate().rev() {
                    stack.push((child, depth + 1, i == n - 1, next_prefix.clone()));
                }
            }
        }

        result
    }
}

impl<Q, F> RenderableTree<Q> for F
where
    F: TransformableForest<Q>,
    F::Item: TreeDisplay,
{
}

// =============================================================================
// TableRow + RenderedTable — output of render_table
// =============================================================================

#[derive(Debug, Clone)]
pub struct TableRow<Id: Eq + Hash + Clone> {
    pub id: Id,
    pub cells: Vec<String>,
    pub connector_chars: usize,
}

pub struct RenderedTable<Id: Eq + Hash + Clone> {
    pub header: Option<TableRow<Id>>,
    pub rows: Vec<TableRow<Id>>,
    pub highlights: HashMap<Id, Vec<Range<usize>>>,
}

// =============================================================================
// render_table — bridge between tree and table
// =============================================================================

/// Layout configuration for the tree table.
pub struct TableLayout {
    pub max_width: usize,
    pub separator: String,
    pub sizer: Box<dyn ColSizer>,
}

pub fn render_table<Id>(
    tree_rows: Vec<TreeCellRow<Id>>,
    data_rows: Vec<Row<Id>>,
    layout: &TableLayout,
    cols: &[ColumnId],
    header: Option<Row<Id>>,
) -> RenderedTable<Id>
where
    Id: Eq + Hash + Clone,
{
    let tree_col_id = ColumnId::new(TREE_COLUMN);

    // Build merged cell maps for column-width sizing.
    let sizing_cells: Vec<HashMap<ColumnId, String>> = tree_rows
        .iter()
        .zip(data_rows.iter())
        .map(|(tr, dr)| {
            let mut cells = dr.cells.clone();
            cells.insert(tree_col_id.clone(), tr.tree_cell.clone());
            cells
        })
        .collect();
    let sizing_cell_refs: Vec<&HashMap<ColumnId, String>> = sizing_cells.iter().collect();
    let header_map: Option<HashMap<ColumnId, String>> = header.as_ref().map(|h| h.cells.clone());

    let widths = layout.sizer.col_widths(
        cols,
        &sizing_cell_refs,
        header_map.as_ref(),
        layout.max_width,
        &layout.separator,
    );

    let fit_row = |id: Id, tree_cell: Option<&str>, connector_chars: usize, data: &Row<Id>| -> TableRow<Id> {
        let cells: Vec<String> = cols
            .iter()
            .zip(widths.iter())
            .map(|(col_id, &w)| {
                let raw = if col_id == &tree_col_id {
                    tree_cell.unwrap_or("").to_string()
                } else {
                    data.cells.get(col_id).cloned().unwrap_or_default()
                };
                fit_to_width(&raw, w)
            })
            .collect();
        TableRow { id, cells, connector_chars }
    };

    let rendered_header = header.map(|h| {
        let dummy_tree = h.cells.get(&tree_col_id).map(|s| s.as_str()).unwrap_or("");
        fit_row(h.id.clone(), Some(dummy_tree), 0, &h)
    });

    let mut rows = Vec::with_capacity(tree_rows.len());
    let mut highlights: HashMap<Id, Vec<Range<usize>>> =
        HashMap::with_capacity(tree_rows.len());

    for (tr, dr) in tree_rows.into_iter().zip(data_rows.into_iter()) {
        let id = tr.id.clone();
        if !tr.highlight_ranges.is_empty() {
            highlights.insert(id.clone(), tr.highlight_ranges);
        }
        rows.push(fit_row(id, Some(&tr.tree_cell), tr.connector_chars, &dr));
    }

    RenderedTable { header: rendered_header, rows, highlights }
}

// =============================================================================
// Tree min width
// =============================================================================

impl<T, S> Forest<T, S>
where
    S: Eq + Hash + Clone,
    T: HasTreeShape<S> + TreeDisplay,
{
    pub fn tree_min_width<Q>(&self, query: &Q) -> usize
    where
        T: ForestItem<Q>,
    {
        use unicode_width::UnicodeWidthStr;

        let ghost_roots = <Self as TransformableForest<Q>>::transform(self, query);
        let mut max_width = 0usize;

        for ghost_root in &ghost_roots {
            let mut stack: Vec<(&GhostNode<'_, T>, usize, bool, String)> =
                vec![(ghost_root, 0, true, String::new())];

            while let Some((ghost, depth, is_last, prefix)) = stack.pop() {
                let has_desc = ghost.node.element.description().is_some();
                let has_children = !ghost.children.is_empty();

                let connector = forest_connector(ConnectorSpec {
                    depth,
                    is_last,
                    prefix: &prefix,
                    has_description: has_desc,
                    has_children,
                    // tree_min_width is upper-bound — assume expanded
                    // (glyph adds at most 2 cells; assume the wider one)
                    expanded: if has_children { Some(true) } else { None },
                });
                max_width = max_width.max(connector.width());

                let n = ghost.children.len();
                let next_prefix =
                    forest_child_prefix(depth, is_last, has_desc, &prefix);
                for (i, child) in ghost.children.iter().enumerate().rev() {
                    stack.push((child, depth + 1, i == n - 1, next_prefix.clone()));
                }
            }
        }

        max_width
    }
}

// =============================================================================
// Internal tree-rendering helpers
// =============================================================================

fn forest_connector(spec: ConnectorSpec<'_>) -> String {
    let ConnectorSpec {
        depth,
        is_last,
        prefix,
        has_description,
        has_children,
        expanded,
    } = spec;

    let glyph = match expanded {
        Some(true) => "▼ ",
        Some(false) => "▶ ",
        None => "",
    };

    let base = if depth == 0 {
        String::new()
    } else {
        match (has_description || !has_children, is_last) {
            (true, false)  => format!("{}├── ", prefix),
            (true, true)   => format!("{}└── ", prefix),
            (false, false) => format!("{}├───┐", prefix),
            (false, true)  => format!("{}└───┐", prefix),
        }
    };

    format!("{}{}", base, glyph)
}

fn forest_child_prefix(
    depth: usize,
    is_last: bool,
    has_description: bool,
    prefix: &str,
) -> String {
    if depth == 0 {
        if has_description {
            prefix.to_string()
        } else {
            format!("{} ", prefix)
        }
    } else if is_last {
        format!("{}    ", prefix)
    } else {
        format!("{}│   ", prefix)
    }
}

// =============================================================================
// Subtree extraction — minimal subtrees containing a set of interesting IDs
// =============================================================================

impl<T, S> Forest<T, S>
where
    S: Eq + Hash + Clone,
    T: HasTreeShape<S>,
{
    /// Return GhostNode trees pruned to only contain paths leading to nodes
    /// whose IDs are in `interesting`. Ancestors of interesting nodes are
    /// included even if they are not themselves interesting. Branches with
    /// no interesting descendants are removed.
    pub fn extract_subtrees<'a>(&'a self, interesting: &HashSet<S>) -> Vec<GhostNode<'a, T>> {
        let mut relevant_roots: HashSet<usize> = HashSet::new();
        for id in interesting {
            if let Some(&root_idx) = self.item_to_root.get(id) {
                relevant_roots.insert(root_idx);
            }
        }

        let mut result = Vec::new();
        for root_idx in relevant_roots {
            if let Some(pruned) = prune_ghost_node(&self.roots[root_idx], interesting) {
                result.push(pruned);
            }
        }
        result
    }
}

/// Recursively build a GhostNode that only includes branches leading to
/// an interesting node. Returns None if neither this node nor any
/// descendant is interesting.
fn prune_ghost_node<'a, T, S>(
    node: &'a TreeNode<T>,
    interesting: &HashSet<S>,
) -> Option<GhostNode<'a, T>>
where
    S: Eq + Hash,
    T: HasTreeShape<S>,
{
    let children: Vec<GhostNode<'a, T>> = node.children.iter()
        .filter_map(|child| prune_ghost_node(child, interesting))
        .collect();

    let self_interesting = interesting.contains(&node.element.id());

    if self_interesting || !children.is_empty() {
        Some(GhostNode {
            node,
            highlight_ranges: vec![],
            children,
        })
    } else {
        None
    }
}

// =============================================================================
// Post-order fold on GhostNode trees
// =============================================================================

/// Result of folding a single node: its ID and computed value, plus the
/// tree cell string and connector metadata for rendering.
#[derive(Debug, Clone)]
pub struct FoldedNode<S, R> {
    pub id: S,
    pub result: R,
    pub tree_cell: String,
    pub connector_chars: usize,
}

/// Intermediate result tree from post-order fold.
struct FoldResultNode<S, R> {
    id: S,
    result: R,
    children: Vec<FoldResultNode<S, R>>,
    has_description: bool,
}

/// Post-order fold over GhostNode trees. Calls `f(element, child_results)`
/// bottom-up, returns nodes in pre-order (display order) with tree
/// connector strings.
pub fn fold_ghost_trees<'a, T, S, R>(
    ghosts: &[GhostNode<'a, T>],
    f: &impl Fn(&T, Vec<&R>) -> R,
) -> Vec<FoldedNode<S, R>>
where
    S: Eq + Hash + Clone,
    T: HasTreeShape<S> + TreeDisplay,
    R: Clone,
{
    // Phase 1: compute results bottom-up.
    let result_trees: Vec<FoldResultNode<S, R>> = ghosts.iter()
        .map(|g| fold_compute(g, f))
        .collect();

    // Phase 2: flatten to pre-order with tree connectors.
    let mut output = Vec::new();
    for tree in &result_trees {
        flatten_fold_result(tree, 0, true, &String::new(), &mut output);
    }
    output
}

fn fold_compute<'a, T, S, R>(
    ghost: &GhostNode<'a, T>,
    f: &impl Fn(&T, Vec<&R>) -> R,
) -> FoldResultNode<S, R>
where
    S: Eq + Hash + Clone,
    T: HasTreeShape<S> + TreeDisplay,
    R: Clone,
{
    let children: Vec<FoldResultNode<S, R>> = ghost.children.iter()
        .map(|child| fold_compute(child, f))
        .collect();

    let child_refs: Vec<&R> = children.iter().map(|c| &c.result).collect();
    let result = f(&ghost.node.element, child_refs);

    FoldResultNode {
        id: ghost.node.element.id(),
        result,
        children,
        has_description: ghost.node.element.description().is_some(),
    }
}

fn flatten_fold_result<S, R>(
    node: &FoldResultNode<S, R>,
    depth: usize,
    is_last: bool,
    prefix: &str,
    output: &mut Vec<FoldedNode<S, R>>,
) where
    S: Clone,
    R: Clone,
{
    let has_children = !node.children.is_empty();
    let connector = forest_connector(ConnectorSpec {
        depth,
        is_last,
        prefix,
        has_description: node.has_description,
        has_children,
        expanded: None,
    });
    let connector_chars = connector.chars().count();

    output.push(FoldedNode {
        id: node.id.clone(),
        result: node.result.clone(),
        tree_cell: connector,
        connector_chars,
    });

    let n = node.children.len();
    let next_prefix = forest_child_prefix(depth, is_last, node.has_description, prefix);
    for (i, child) in node.children.iter().enumerate() {
        flatten_fold_result(child, depth + 1, i == n - 1, &next_prefix, output);
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Simple test item: id, parent_id, description.
    #[derive(Debug, Clone)]
    struct Item {
        id: u32,
        parent_id: Option<u32>,
        desc: String,
    }

    impl Item {
        fn new(id: u32, parent: Option<u32>, desc: &str) -> Self {
            Self { id, parent_id: parent, desc: desc.to_string() }
        }
    }

    impl HasTreeShape<u32> for Item {
        fn id(&self) -> u32 { self.id }
        fn parent_id(&self) -> Option<u32> { self.parent_id }
    }

    impl TreeDisplay for Item {
        fn description(&self) -> Option<&str> { Some(&self.desc) }
    }

    impl ForestItem<()> for Item {
        fn matches_filter(&self, _query: &()) -> bool { true }
    }

    impl IntoRow for Item {
        type Id = u32;
        fn into_row(&self) -> Row<u32> {
            Row { id: self.id, cells: HashMap::new() }
        }
    }

    /// Build a forest from items for testing.
    fn test_forest() -> Forest<Item, u32> {
        // Tree structure:
        //   1 "A"
        //   ├── 2 "B"
        //   │   ├── 4 "D"
        //   │   └── 5 "E"
        //   └── 3 "C"
        //       └── 6 "F"
        //   7 "G"  (separate root)
        //   └── 8 "H"
        Forest::from_items(vec![
            Item::new(1, None, "A"),
            Item::new(2, Some(1), "B"),
            Item::new(3, Some(1), "C"),
            Item::new(4, Some(2), "D"),
            Item::new(5, Some(2), "E"),
            Item::new(6, Some(3), "F"),
            Item::new(7, None, "G"),
            Item::new(8, Some(7), "H"),
        ])
    }

    fn ghost_ids<T: HasTreeShape<u32>>(ghosts: &[GhostNode<'_, T>]) -> Vec<u32> {
        let mut ids = Vec::new();
        for g in ghosts {
            collect_ghost_ids(g, &mut ids);
        }
        ids
    }

    fn collect_ghost_ids<T: HasTreeShape<u32>>(ghost: &GhostNode<'_, T>, ids: &mut Vec<u32>) {
        ids.push(ghost.node.element.id());
        for child in &ghost.children {
            collect_ghost_ids(child, ids);
        }
    }

    // ── extract_subtrees tests ──────────────────────────────────────

    #[test]
    fn extract_leaf_only() {
        let forest = test_forest();
        let interesting: HashSet<u32> = [5].into_iter().collect();
        let ghosts = forest.extract_subtrees(&interesting);

        // Should include path: A → B → E (but not D, C, F)
        let ids = ghost_ids(&ghosts);
        assert!(ids.contains(&1), "root A included");
        assert!(ids.contains(&2), "ancestor B included");
        assert!(ids.contains(&5), "interesting E included");
        assert!(!ids.contains(&4), "sibling D pruned");
        assert!(!ids.contains(&3), "unrelated C pruned");
        assert!(!ids.contains(&6), "unrelated F pruned");
        assert!(!ids.contains(&7), "other root G pruned");
    }

    #[test]
    fn extract_multiple_in_same_tree() {
        let forest = test_forest();
        let interesting: HashSet<u32> = [4, 6].into_iter().collect();
        let ghosts = forest.extract_subtrees(&interesting);

        let ids = ghost_ids(&ghosts);
        // Both branches needed: A → B → D and A → C → F
        assert!(ids.contains(&1));
        assert!(ids.contains(&2));
        assert!(ids.contains(&4));
        assert!(ids.contains(&3));
        assert!(ids.contains(&6));
        // E is not interesting and has no interesting descendants
        assert!(!ids.contains(&5), "sibling E pruned");
    }

    #[test]
    fn extract_across_roots() {
        let forest = test_forest();
        let interesting: HashSet<u32> = [5, 8].into_iter().collect();
        let ghosts = forest.extract_subtrees(&interesting);

        let ids = ghost_ids(&ghosts);
        assert!(ids.contains(&1)); // root of tree 1
        assert!(ids.contains(&2)); // ancestor of 5
        assert!(ids.contains(&5));
        assert!(ids.contains(&7)); // root of tree 2
        assert!(ids.contains(&8));
        assert!(!ids.contains(&3), "unrelated branch pruned");
    }

    #[test]
    fn extract_root_node() {
        let forest = test_forest();
        let interesting: HashSet<u32> = [1].into_iter().collect();
        let ghosts = forest.extract_subtrees(&interesting);

        // Root itself is interesting — no children needed
        assert_eq!(ghosts.len(), 1);
        let ids = ghost_ids(&ghosts);
        assert_eq!(ids, vec![1]);
    }

    #[test]
    fn extract_empty_set() {
        let forest = test_forest();
        let interesting: HashSet<u32> = HashSet::new();
        let ghosts = forest.extract_subtrees(&interesting);
        assert!(ghosts.is_empty());
    }

    // ── fold_ghost_trees tests ──────────────────────────────────────

    #[test]
    fn fold_sum_bottom_up() {
        let forest = test_forest();
        // All nodes interesting — full tree
        let interesting: HashSet<u32> = [1, 2, 3, 4, 5, 6].into_iter().collect();
        let ghosts = forest.extract_subtrees(&interesting);

        // Each leaf has value 1, parent accumulates children.
        let folded = fold_ghost_trees(&ghosts, &|_item: &Item, children: Vec<&u32>| -> u32 {
            1 + children.iter().copied().sum::<u32>()
        });

        let by_id: HashMap<u32, u32> = folded.iter().map(|f| (f.id, f.result)).collect();
        // Leaves: D=1, E=1, F=1
        assert_eq!(by_id[&4], 1);
        assert_eq!(by_id[&5], 1);
        assert_eq!(by_id[&6], 1);
        // B = 1 + D + E = 3
        assert_eq!(by_id[&2], 3);
        // C = 1 + F = 2
        assert_eq!(by_id[&3], 2);
        // A = 1 + B + C = 6
        assert_eq!(by_id[&1], 6);
    }

    #[test]
    fn fold_display_order_is_preorder() {
        let forest = test_forest();
        let interesting: HashSet<u32> = [4, 6].into_iter().collect();
        let ghosts = forest.extract_subtrees(&interesting);

        let folded = fold_ghost_trees(&ghosts, &|_item: &Item, _children: Vec<&u32>| -> u32 { 0 });

        let ids: Vec<u32> = folded.iter().map(|f| f.id).collect();
        // Pre-order: root A first, then its children (B→D and C→F in some order).
        assert_eq!(ids[0], 1, "root A comes first");
        assert_eq!(ids.len(), 5);
        // B must come before D, C must come before F.
        let b_pos = ids.iter().position(|&id| id == 2).unwrap();
        let d_pos = ids.iter().position(|&id| id == 4).unwrap();
        let c_pos = ids.iter().position(|&id| id == 3).unwrap();
        let f_pos = ids.iter().position(|&id| id == 6).unwrap();
        assert!(b_pos < d_pos, "B before D");
        assert!(c_pos < f_pos, "C before F");
    }

    #[test]
    fn fold_connectors_generated() {
        let forest = test_forest();
        let interesting: HashSet<u32> = [4, 5].into_iter().collect();
        let ghosts = forest.extract_subtrees(&interesting);

        let folded = fold_ghost_trees(&ghosts, &|_item: &Item, _children: Vec<&u32>| -> u32 { 0 });

        // Root has no connector
        assert_eq!(folded[0].tree_cell, ""); // A (depth 0)
        assert!(folded[0].connector_chars == 0);
        // Children have connectors
        assert!(folded[1].connector_chars > 0); // B
        assert!(folded[2].connector_chars > 0); // D
    }

    // ── tree_rows_with_options tests ────────────────────────────────

    /// Empty options (all expanded) — same row count as `tree_rows`.
    #[test]
    fn tree_rows_with_options_all_visible_renders_full_tree() {
        let forest = test_forest();
        let rows: Vec<TreeCellRow<u32>> =
            forest.tree_rows_with_options(&(), &TreeRenderOptions::all_visible());

        let ids: Vec<u32> = rows.iter().map(|r| r.id).collect();
        assert_eq!(ids.len(), 8, "all 8 nodes visible");
        // No `(N)` suffix anywhere because no node is collapsed.
        for row in &rows {
            assert!(
                !row.tree_cell.contains('('),
                "no collapsed-count suffix in fully expanded render: {:?}",
                row.tree_cell
            );
        }
        // No glyph either (all_visible uses Some(true) → ▼ for parents).
        // Parents A, B, C, G should have ▼; leaves D, E, F, H should not.
        let cell_for = |id: u32| -> &str {
            rows.iter().find(|r| r.id == id).unwrap().tree_cell.as_str()
        };
        // all_visible passes is_expanded=true for parents; tree_rows_with_options
        // emits ▼ for parents and no glyph for leaves.
        assert!(cell_for(1).contains('\u{25BC}'), "A is parent → ▼ glyph");
        assert!(cell_for(2).contains('\u{25BC}'), "B is parent → ▼ glyph");
        assert!(!cell_for(4).contains('\u{25BC}'), "D is leaf → no glyph");
        assert!(!cell_for(4).contains('\u{25B6}'), "D is leaf → no glyph");
    }

    /// Collapse-all closure: parents collapse to `(N)`, children skipped.
    #[test]
    fn tree_rows_with_options_collapsed_hides_children_and_shows_count() {
        let forest = test_forest();
        let child_counts: HashMap<u32, usize> =
            [(1, 2), (2, 2), (3, 1), (7, 1)].into_iter().collect();
        let options = TreeRenderOptions {
            is_expanded: Box::new(|_id: &u32, _depth: usize| false),
            child_counts,
        };

        let rows: Vec<TreeCellRow<u32>> = forest.tree_rows_with_options(&(), &options);
        let ids: Vec<u32> = rows.iter().map(|r| r.id).collect();
        // Only root nodes (A=1, G=7) are emitted; their children are hidden.
        assert!(ids.contains(&1), "root A emitted");
        assert!(ids.contains(&7), "root G emitted");
        assert!(!ids.contains(&2), "B hidden under collapsed A");
        assert!(!ids.contains(&3), "C hidden under collapsed A");
        assert!(!ids.contains(&4), "D hidden under collapsed A");
        assert_eq!(ids.len(), 2, "only the two roots remain");

        // `(N)` suffix uses unfiltered child count from the map.
        let cell_a = &rows.iter().find(|r| r.id == 1).unwrap().tree_cell;
        assert!(cell_a.contains("(2)"), "A shows (2) for its 2 children: {cell_a:?}");
        assert!(cell_a.contains('\u{25B6}'), "A shows ▶ glyph when collapsed: {cell_a:?}");
        let cell_g = &rows.iter().find(|r| r.id == 7).unwrap().tree_cell;
        assert!(cell_g.contains("(1)"), "G shows (1) for its 1 child: {cell_g:?}");
    }

    /// Depth-based expansion mimicking `default_expand_depth`. With
    /// depth-cap=1, root expands (▼), depth-1 children collapse (▶).
    #[test]
    fn tree_rows_with_options_depth_cap_expansion() {
        let forest = test_forest();
        let child_counts: HashMap<u32, usize> =
            [(1, 2), (2, 2), (3, 1), (7, 1)].into_iter().collect();
        let options = TreeRenderOptions {
            // depth < 1  → expanded (depth 0 only = roots)
            is_expanded: Box::new(|_id: &u32, depth: usize| depth < 1),
            child_counts,
        };

        let rows: Vec<TreeCellRow<u32>> = forest.tree_rows_with_options(&(), &options);
        let ids: Vec<u32> = rows.iter().map(|r| r.id).collect();
        // Roots + their direct children visible; grandchildren hidden.
        assert!(ids.contains(&1), "A visible");
        assert!(ids.contains(&2), "B (child of A) visible");
        assert!(ids.contains(&3), "C (child of A) visible");
        assert!(ids.contains(&7), "G visible");
        assert!(ids.contains(&8), "H (child of G) visible");
        assert!(!ids.contains(&4), "D (depth-2 grandchild) hidden");
        assert!(!ids.contains(&5), "E (depth-2 grandchild) hidden");
        assert!(!ids.contains(&6), "F (depth-2 grandchild) hidden");

        // Depth-1 parents (B, C) should be collapsed → `(N)` + ▶.
        let cell_b = &rows.iter().find(|r| r.id == 2).unwrap().tree_cell;
        assert!(cell_b.contains("(2)"), "B collapsed → (2): {cell_b:?}");
        assert!(cell_b.contains('\u{25B6}'), "B collapsed → ▶ glyph");
        let cell_c = &rows.iter().find(|r| r.id == 3).unwrap().tree_cell;
        assert!(cell_c.contains("(1)"), "C collapsed → (1): {cell_c:?}");

        // Root A is expanded → ▼, no count.
        let cell_a = &rows.iter().find(|r| r.id == 1).unwrap().tree_cell;
        assert!(cell_a.contains('\u{25BC}'), "A expanded → ▼");
        assert!(!cell_a.contains('('), "A expanded → no (N) suffix");

        // Leaf H (no children) → no glyph, no count.
        let cell_h = &rows.iter().find(|r| r.id == 8).unwrap().tree_cell;
        assert!(!cell_h.contains('\u{25BC}'));
        assert!(!cell_h.contains('\u{25B6}'));
        assert!(!cell_h.contains('('));
    }
}
