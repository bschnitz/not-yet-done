//! Generic filterable-forest data structure.
//!
//! This crate is intentionally task-agnostic.  It provides:
//!
//! - [`TreeNode`] / [`HasTreeShape`] — building blocks for any tree
//! - [`ForestItem`] — trait for filter-matching
//! - [`Forest<T, S>`] — an immutable, O(1)-filterable forest
//! - [`Table`] / [`ForestTable`] traits — rendering helpers
//! - Column sizing strategies ([`MixedColSizer`], [`FixedColSizer`])
//! - [`fit_to_width`] — unicode-aware cell truncation / padding

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;

// =============================================================================
// TreeNode + traits
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

/// A filterable forest — knows how to return matching root nodes.
pub trait FilterableForest<Q> {
    type Item;
    fn filter(&self, query: &Q) -> Vec<&TreeNode<Self::Item>>;
}

// =============================================================================
// Forest<T, S>
// =============================================================================

/// An immutable forest of elements of type `T` with ID type `S`.
///
/// Built once via [`Forest::from_items`]; all filter operations are
/// read-only afterwards.
pub struct Forest<T, S> {
    roots: Vec<TreeNode<T>>,
    /// item_id → index in `roots`
    item_to_root: HashMap<S, usize>,
    /// item_id → raw pointer to element (O(1) filter without tree traversal)
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
        
        // Iterative traversal
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
    /// # Algorithm
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

        Forest {
            roots,
            item_to_root,
            flat_items,
        }
    }

    /// All root nodes of the forest.
    pub fn roots(&self) -> Vec<&TreeNode<T>> {
        self.roots.iter().collect()
    }

    /// Total number of items (all levels).
    pub fn len(&self) -> usize {
        self.flat_items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }
}

impl<T, S, Q> FilterableForest<Q> for Forest<T, S>
where
    S: Eq + Hash + Clone,
    T: HasTreeShape<S> + ForestItem<Q>,
{
    type Item = T;

    /// Filter in O(n): walk `flat_items`, call `matches_filter`, look up the
    /// root index, deduplicate via HashSet.
    fn filter(&self, query: &Q) -> Vec<&TreeNode<T>> {
        let mut root_indices: HashSet<usize> = HashSet::new();

        for (id, &task_ptr) in &self.flat_items {
            // SAFETY: flat_items points into roots which live for &self.
            let item = unsafe { &*task_ptr };
            if item.matches_filter(query) {
                if let Some(&root_idx) = self.item_to_root.get(id) {
                    root_indices.insert(root_idx);
                }
            }
        }

        root_indices
            .into_iter()
            .map(|idx| &self.roots[idx])
            .collect()
    }
}

// =============================================================================
// Column / Table traits
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

/// Generic table trait.
///
/// Implementors supply raw rows and an optional header.
/// `rendered_rows` is a provided default and normally need not be overridden.
pub trait Table {
    type Id: Eq + Hash + Clone;

    fn rows(&self) -> Vec<Row<Self::Id>>;
    fn header(&self) -> Option<Row<Self::Id>>;

    /// Render rows according to a `TableLayout` and an ordered column slice.
    fn rendered_rows(&self, layout: &TableLayout, cols: &[ColumnId]) -> Vec<RenderedRow<Self::Id>> {
        let rows = self.rows();
        let widths = layout
            .sizer
            .col_widths(cols, &rows, layout.max_width, &layout.separator);
        rows.into_iter()
            .map(|row| {
                let fitted: Vec<String> = cols
                    .iter()
                    .zip(widths.iter())
                    .map(|(col_id, &width)| {
                        let value = row.cells.get(col_id).cloned().unwrap_or_default();
                        fit_to_width(&value, width)
                    })
                    .collect();
                let rendered = fitted.join(&layout.separator);
                RenderedRow {
                    id: row.id,
                    rendered,
                }
            })
            .collect()
    }
}

/// Extension of `Table` for forests — the first column is always the tree
/// representation.
pub trait ForestTable: Table {
    fn available_columns(&self) -> Vec<ColumnId>;
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
        let ellipsis_width = 1; // U+2026 is always 1 display column wide
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
// Tree rendering helper (broot-style connectors)
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
