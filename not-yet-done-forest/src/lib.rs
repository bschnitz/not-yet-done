// not-yet-done-forest/src/lib.rs

//! Generic filterable-forest data structure.
//!
//! This crate is intentionally task-agnostic.  It provides:
//!
//! - [`TreeNode`] / [`HasTreeShape`] — building blocks for any tree
//! - [`ForestItem`] — trait for filter-matching
//! - [`Forest<T, S>`] — an immutable forest
//! - [`GhostNode`] — a borrowed view into a subtree (filtered + sorted)
//! - [`TransformableForest<Q>`] — produce a `GhostNode` forest from a query
//! - [`TreeDisplay`] — optional per-node label for the tree column
//! - [`IntoRow`] — convert an element into non-tree [`Row`] cells
//! - [`FilterableTable<Q>`] — filterable, tree-rendered table
//! - Column sizing strategies ([`MixedColSizer`], [`FixedColSizer`])
//! - [`fit_to_width`] — unicode-aware cell truncation / padding

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;

// =============================================================================
// TreeNode + HasTreeShape + ForestItem
// =============================================================================

/// A single node in the forest holding an element and its children.
#[derive(Debug, Clone)]
pub struct TreeNode<T> {
    pub element: T,
    pub children: Vec<TreeNode<T>>,
}

impl<T> TreeNode<T> {
    pub fn new(element: T) -> Self {
        TreeNode {
            element,
            children: Vec::new(),
        }
    }
}

/// Minimal trait required for building a [`Forest`].
///
/// `S` is the ID type and must be hashable.
pub trait HasTreeShape<S> {
    fn id(&self) -> S;
    fn parent_id(&self) -> Option<S>;
}

/// Trait for items that can be matched against a filter query `Q`.
pub trait ForestItem<Q> {
    fn matches_filter(&self, query: &Q) -> bool;
}

// =============================================================================
// Forest<T, S>
// =============================================================================

/// An immutable forest of elements of type `T` with ID type `S`.
///
/// Built once via [`Forest::from_items`]; all operations are read-only
/// afterwards.
pub struct Forest<T, S> {
    roots: Vec<TreeNode<T>>,
    /// item_id → index in `roots`
    item_to_root: HashMap<S, usize>,
    /// item_id → raw pointer to element (O(1) lookup without tree traversal)
    flat_items: HashMap<S, *const T>,
}

// SAFETY: Forest is immutable after construction. Raw pointers in flat_items
// point into `roots` which is never moved or reallocated afterwards.
unsafe impl<T: Send, S: Send> Send for Forest<T, S> {}
unsafe impl<T: Sync, S: Sync> Sync for Forest<T, S> {}

// Manual Debug: omit flat_items (raw pointers) — just show root count.
impl<T: std::fmt::Debug, S: std::fmt::Debug> std::fmt::Debug for Forest<T, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Forest")
            .field("roots", &self.roots)
            .field("item_count", &self.flat_items.len())
            .finish()
    }
}

// Manual Clone: rebuild flat_items from the cloned roots tree so that the
// raw pointers remain valid for the new allocation.
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
    /// Build the forest in O(n).
    ///
    /// **Pass 1** — Load all items into `HashMap<S, TreeNode<T>>`.
    ///
    /// **Pass 2** — Resolve edges (Kahn's algorithm):
    ///   Roots are items whose `parent_id` is `None` or not present in the map.
    ///   BFS from roots → leaves gives a topological order.
    ///   Children are attached leaf-first so the parent node is still in the
    ///   map when a child is moved into it.
    ///
    /// **Pass 3** — DFS over roots → fill `item_to_root` + `flat_items`.
    pub fn from_items(items: Vec<T>) -> Self {
        let n = items.len();

        // --- Pass 1 ---
        let mut node_map: HashMap<S, TreeNode<T>> = HashMap::with_capacity(n);
        for item in items {
            node_map.insert(item.id(), TreeNode::new(item));
        }

        // --- Pass 2 ---
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

        // Topological order via BFS (roots → leaves)
        let mut topo_order: Vec<S> = Vec::with_capacity(n);
        let mut queue: VecDeque<S> = root_ids.iter().cloned().collect();
        while let Some(id) = queue.pop_front() {
            topo_order.push(id.clone());
            if let Some(kids) = children_of.get(&id) {
                queue.extend(kids.iter().cloned());
            }
        }

        // Attach children leaf-first (reverse topological order)
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

        // --- Pass 3: item_to_root + flat_items via DFS ---
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

    /// All root nodes of the forest.
    pub fn roots(&self) -> &[TreeNode<T>] {
        &self.roots
    }

    /// Total number of items (all levels).
    pub fn len(&self) -> usize {
        self.flat_items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }
}

// =============================================================================
// GhostNode — a borrowed, transformed view into a Forest
// =============================================================================

/// A borrowed node in a transformed (filtered + sorted) view of a [`Forest`].
///
/// `GhostNode`s are produced by [`TransformableForest::transform`] and live
/// only for the duration of a single `rows()` or `tree_min_width()` call —
/// they are never stored.
pub struct GhostNode<'a, T> {
    pub node: &'a TreeNode<T>,
    pub children: Vec<GhostNode<'a, T>>,
}

// =============================================================================
// TransformableForest<Q>
// =============================================================================

/// Produce a transformed (filtered, sorted, restructured) view of the forest
/// as a `Vec<GhostNode>` for a given query.
///
/// The default implementation provided for [`Forest<T, S>`] only filters
/// (items where `matches_filter` returns `true`).  Callers who need sorting
/// or other transformations implement this trait on a newtype wrapper around
/// `Forest`.
///
/// # Lifetimes
///
/// `GhostNode<'a, T>` borrows from `&'a self`.  The returned vec must not
/// outlive the forest — in practice this is never a problem since `rows()` and
/// `tree_min_width()` consume the vec immediately within the same call.
pub trait TransformableForest<Q> {
    type Item;
    fn transform<'a>(&'a self, query: &Q) -> Vec<GhostNode<'a, Self::Item>>;
}

/// Default implementation: filter only, no sorting.
impl<T, S, Q> TransformableForest<Q> for Forest<T, S>
where
    S: Eq + Hash + Clone,
    T: HasTreeShape<S> + ForestItem<Q>,
{
    type Item = T;

    fn transform<'a>(&'a self, query: &Q) -> Vec<GhostNode<'a, T>> {
        // Collect matching root indices (O(n) flat scan).
        let mut root_indices: HashSet<usize> = HashSet::new();
        for (id, &ptr) in &self.flat_items {
            // SAFETY: flat_items points into roots which live for &'a self.
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

/// Recursively build a `GhostNode` tree that mirrors a `TreeNode` tree.
fn ghost_from_node<T>(node: &TreeNode<T>) -> GhostNode<'_, T> {
    GhostNode {
        node,
        children: node.children.iter().map(ghost_from_node).collect(),
    }
}

// =============================================================================
// TreeDisplay + IntoRow
// =============================================================================

/// Optional label shown in the tree column next to the connector.
///
/// When `description()` returns `Some(s)`, the cell renders as
/// `<connector><s>`.  When it returns `None`, only the connector is shown
/// (with a `┐` suffix if the node has children).
pub trait TreeDisplay {
    fn description(&self) -> Option<&str>;
}

/// Convert an element into a [`Row`] containing **only the non-tree columns**.
///
/// The tree column (`TREE_COLUMN`) is injected automatically by
/// [`FilterableTable`].
pub trait IntoRow {
    type Id: Eq + Hash + Clone;
    fn into_row(&self) -> Row<Self::Id>;
}

// =============================================================================
// Column / Table types
// =============================================================================

/// Identifies a column by name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ColumnId(pub String);

impl ColumnId {
    pub fn new(s: impl Into<String>) -> Self {
        ColumnId(s.into())
    }
}

/// Per-column sizing strategy.
#[derive(Debug, Clone)]
pub enum ColStrategy {
    /// Always exactly `n` display columns wide.
    Fixed(usize),
    /// As wide as the longest cell content (including the header).
    Max,
    /// Gets a proportional share of remaining space.
    Flex(usize),
}

/// Determines absolute character widths for a set of columns.
pub trait ColSizer {
    fn col_widths<Id: Eq + Hash>(
        &self,
        cols: &[ColumnId],
        rows: &[Row<Id>],
        max_width: usize,
        separator: &str,
    ) -> Vec<usize>;
}

/// Simple sizer: fixed width per column, independent of content.
pub struct FixedColSizer {
    pub widths: HashMap<ColumnId, usize>,
}

impl ColSizer for FixedColSizer {
    fn col_widths<Id: Eq + Hash>(
        &self,
        cols: &[ColumnId],
        _rows: &[Row<Id>],
        _max_width: usize,
        _separator: &str,
    ) -> Vec<usize> {
        cols.iter()
            .map(|col| self.widths.get(col).copied().unwrap_or(0))
            .collect()
    }
}

/// Flexible sizer with three strategies per column.
///
/// **Pass 1 — Fixed**: reserves the exact given width.
/// **Pass 2 — Max**: measures the longest content over all rows (incl. header).
/// **Pass 3 — Flex**: distributes remaining space proportionally by weight.
/// Columns not listed in `strategies` default to `Flex(1)`.
pub struct MixedColSizer {
    pub strategies: HashMap<ColumnId, ColStrategy>,
}

impl ColSizer for MixedColSizer {
    fn col_widths<Id: Eq + Hash>(
        &self,
        cols: &[ColumnId],
        rows: &[Row<Id>],
        max_width: usize,
        separator: &str,
    ) -> Vec<usize> {
        use unicode_width::UnicodeWidthStr;

        let n = cols.len();
        if n == 0 {
            return vec![];
        }

        let sep_total = separator.width() * n.saturating_sub(1);
        let usable = max_width.saturating_sub(sep_total);

        let mut widths = vec![0usize; n];
        let mut used = 0usize;
        let mut flex_total_weight = 0usize;
        let mut flex_indices: Vec<(usize, usize)> = Vec::new();

        for (i, col) in cols.iter().enumerate() {
            let strategy = self
                .strategies
                .get(col)
                .cloned()
                .unwrap_or(ColStrategy::Flex(1));

            match strategy {
                ColStrategy::Fixed(w) => {
                    widths[i] = w.min(usable.saturating_sub(used));
                    used += widths[i];
                }
                ColStrategy::Max => {
                    let max_content = rows
                        .iter()
                        .map(|row| row.cells.get(col).map(|s| s.width()).unwrap_or(0))
                        .max()
                        .unwrap_or(0);
                    let w = max_content.min(usable.saturating_sub(used));
                    widths[i] = w;
                    used += w;
                }
                ColStrategy::Flex(weight) => {
                    flex_indices.push((i, weight));
                    flex_total_weight += weight;
                }
            }
        }

        let remaining = usable.saturating_sub(used);
        if flex_total_weight > 0 {
            let mut distributed = 0usize;
            let flex_count = flex_indices.len();
            for (k, (i, weight)) in flex_indices.iter().enumerate() {
                let w = if k == flex_count - 1 {
                    remaining - distributed
                } else {
                    remaining * weight / flex_total_weight
                };
                widths[*i] = w;
                distributed += w;
            }
        }

        widths
    }
}

pub enum ColSizerEnum {
    Fixed(FixedColSizer),
    Mixed(MixedColSizer),
}

impl ColSizer for ColSizerEnum {
    fn col_widths<Id: Eq + Hash>(
        &self,
        cols: &[ColumnId],
        rows: &[Row<Id>],
        max_width: usize,
        separator: &str,
    ) -> Vec<usize> {
        match self {
            ColSizerEnum::Fixed(s) => s.col_widths(cols, rows, max_width, separator),
            ColSizerEnum::Mixed(s) => s.col_widths(cols, rows, max_width, separator),
        }
    }
}

/// Layout configuration for a table.
pub struct TableLayout {
    pub max_width: usize,
    pub separator: String,
    pub sizer: ColSizerEnum,
}

/// A raw table row with a typed ID and per-column string cells.
#[derive(Debug, Clone)]
pub struct Row<Id>
where
    Id: Eq + Hash,
{
    pub id: Id,
    pub cells: HashMap<ColumnId, String>,
}

/// A fully rendered row ready to be written into a TUI buffer.
#[derive(Debug, Clone)]
pub struct RenderedRow<Id>
where
    Id: Eq + Hash,
{
    pub id: Id,
    pub rendered: String,
}

// =============================================================================
// The fixed tree column id
// =============================================================================

/// The fixed [`ColumnId`] used for the tree column.
pub const TREE_COLUMN: &str = "tree";

// =============================================================================
// FilterableTable<Q>
// =============================================================================

/// A table whose rows are produced by transforming a forest with a query.
///
/// Implement [`TransformableForest<Q>`] on your type (or a newtype wrapper)
/// to control filtering and sorting.  Then `FilterableTable` uses the result
/// to build rows with tree connectors and optional descriptions.
pub trait FilterableTable<Q>: TransformableForest<Q>
where
    Self::Item: TreeDisplay + IntoRow,
    <Self::Item as IntoRow>::Id: Eq + Hash + Clone,
{
    /// Returns rows for all items matching `query`, with the tree column
    /// already populated (connector + optional description).
    fn rows(&self, query: &Q) -> Vec<Row<<Self::Item as IntoRow>::Id>> {
        let tree_col = ColumnId::new(TREE_COLUMN);
        let ghost_roots = self.transform(query);
        let mut result = Vec::new();

        for ghost_root in &ghost_roots {
            // Stack entries: (ghost_node, depth, is_last, prefix)
            let mut stack: Vec<(&GhostNode<'_, Self::Item>, usize, bool, String)> =
                vec![(ghost_root, 0, true, String::new())];

            while let Some((ghost, depth, is_last, prefix)) = stack.pop() {
                let elem = &ghost.node.element;
                let desc = elem.description();
                let has_desc = desc.is_some();
                let has_children = !ghost.children.is_empty();

                let connector = forest_connector(depth, is_last, &prefix, has_desc, has_children);
                let tree_cell = match desc {
                    Some(d) => format!("{}{}", connector, d),
                    None => connector,
                };

                let mut row = elem.into_row();
                row.cells.insert(tree_col.clone(), tree_cell);
                result.push(row);

                let n = ghost.children.len();
                let next_prefix = forest_child_prefix(depth, is_last, has_desc, &prefix);
                for (i, child) in ghost.children.iter().enumerate().rev() {
                    stack.push((child, depth + 1, i == n - 1, next_prefix.clone()));
                }
            }
        }

        result
    }

    /// Render rows to strings.
    ///
    /// `header` is optional and, when supplied, is rendered as the first row
    /// using the same column widths as the data rows.
    fn rendered_rows(
        &self,
        query: &Q,
        layout: &TableLayout,
        cols: &[ColumnId],
        header: Option<Row<<Self::Item as IntoRow>::Id>>,
    ) -> Vec<RenderedRow<<Self::Item as IntoRow>::Id>> {
        let rows = self.rows(query);
        let widths = layout
            .sizer
            .col_widths(cols, &rows, layout.max_width, &layout.separator);

        let render_one = |row: Row<<Self::Item as IntoRow>::Id>| {
            let fitted: Vec<String> = cols
                .iter()
                .zip(widths.iter())
                .map(|(col_id, &width)| {
                    let value = row.cells.get(col_id).cloned().unwrap_or_default();
                    fit_to_width(&value, width)
                })
                .collect();
            RenderedRow {
                id: row.id,
                rendered: fitted.join(&layout.separator),
            }
        };

        let mut result = Vec::new();
        if let Some(h) = header {
            result.push(render_one(h));
        }
        result.extend(rows.into_iter().map(render_one));
        result
    }
}

/// Blanket impl: every type that implements `TransformableForest<Q>` with
/// the right bounds automatically implements `FilterableTable<Q>`.
impl<Q, F> FilterableTable<Q> for F
where
    F: TransformableForest<Q>,
    F::Item: TreeDisplay + IntoRow,
    <F::Item as IntoRow>::Id: Eq + Hash + Clone,
{
}

// =============================================================================
// Tree-column minimum width helper
// =============================================================================

impl<T, S> Forest<T, S>
where
    S: Eq + Hash + Clone,
    T: HasTreeShape<S> + TreeDisplay,
{
    /// Returns the minimum display width needed to show the tree structure
    /// (connectors only, **without** description text) for all nodes matching
    /// `query`.
    ///
    /// Use this as a baseline when configuring the tree column width, e.g.:
    ///
    /// ```rust,ignore
    /// let min = forest.tree_min_width(&query);
    /// strategies.insert(ColumnId::new(TREE_COLUMN), ColStrategy::Fixed(min + 20));
    /// ```
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

                let connector = forest_connector(depth, is_last, &prefix, has_desc, has_children);
                max_width = max_width.max(connector.width());

                let n = ghost.children.len();
                let next_prefix = forest_child_prefix(depth, is_last, has_desc, &prefix);
                for (i, child) in ghost.children.iter().enumerate().rev() {
                    stack.push((child, depth + 1, i == n - 1, next_prefix.clone()));
                }
            }
        }

        max_width
    }
}

// =============================================================================
// Internal tree-rendering helpers (broot-style, description-aware)
// =============================================================================

/// Connector string for a single node.
///
/// | has_description | has_children | is_last | depth=0 | result            |
/// |-----------------|--------------|---------|---------|-------------------|
/// | any             | any          | any     | yes     | `""`              |
/// | true            | any          | false   | no      | `"<prefix>├── "`  |
/// | true            | any          | true    | no      | `"<prefix>└── "`  |
/// | false           | true         | false   | no      | `"<prefix>├───┐"` |
/// | false           | true         | true    | no      | `"<prefix>└───┐"` |
/// | false           | false        | false   | no      | `"<prefix>├── "`  |
/// | false           | false        | true    | no      | `"<prefix>└── "`  |
fn forest_connector(
    depth: usize,
    is_last: bool,
    prefix: &str,
    has_description: bool,
    has_children: bool,
) -> String {
    if depth == 0 {
        return String::new();
    }
    match (has_description || !has_children, is_last) {
        (true, false) => format!("{}├── ", prefix),
        (true, true)  => format!("{}└── ", prefix),
        (false, false) => format!("{}├───┐", prefix),
        (false, true)  => format!("{}└───┐", prefix),
    }
}

/// Prefix passed down to the children of a node.
fn forest_child_prefix(depth: usize, is_last: bool, has_description: bool, prefix: &str) -> String {
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
// fit_to_width
// =============================================================================

/// Truncate or pad a string to exactly `width` display columns
/// (measured with unicode-width, not codepoints or bytes).
///
/// On overflow: truncate to `width - 1` display columns and append `…`
/// (U+2026).  On underflow: pad with spaces.
pub fn fit_to_width(s: &str, width: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    use unicode_width::UnicodeWidthStr;

    let display_width = s.width();

    if display_width <= width {
        let padding = width - display_width;
        format!("{}{}", s, " ".repeat(padding))
    } else {
        let ellipsis = "…";
        let ellipsis_width = 1;
        let target = width.saturating_sub(ellipsis_width);

        let mut result = String::new();
        let mut used = 0;
        for ch in s.chars() {
            let ch_width = ch.width().unwrap_or(0);
            if used + ch_width > target {
                break;
            }
            result.push(ch);
            used += ch_width;
        }
        result.push_str(&" ".repeat(target - used));
        result.push_str(ellipsis);
        result
    }
}

// =============================================================================
// Original tree-rendering helpers — kept for backwards compatibility
// =============================================================================

/// Build the connector prefix string for a node at a given depth.
///
/// ```text
/// Root
/// ├── Child A
/// │   ├── Grandchild
/// │   └── Grandchild (last)
/// └── Child B (last)
/// ```
pub fn tree_connector(depth: usize, is_last: bool, prefix: &str) -> String {
    if depth == 0 {
        String::new()
    } else if is_last {
        format!("{}└── ", prefix)
    } else {
        format!("{}├── ", prefix)
    }
}

/// Build the prefix string to pass to children.
pub fn child_prefix(depth: usize, is_last: bool, prefix: &str) -> String {
    if depth == 0 {
        prefix.to_string()
    } else if is_last {
        format!("{}    ", prefix)
    } else {
        format!("{}│   ", prefix)
    }
}
