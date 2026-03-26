//! Generic filterable-forest data structure.
//!
//! This crate is intentionally task-agnostic.  It provides:
//!
//! - [`TreeNode`] / [`HasTreeShape`] — building blocks for any tree
//! - [`ForestItem`] — trait for filter-matching
//! - [`Forest<T, S>`] — an immutable forest
//! - [`GhostNode`] — a borrowed, transformed view into a subtree (with highlight ranges)
//! - [`TransformableForest<Q>`] — produce a `GhostNode` forest from a query
//! - [`TreeDisplay`] — optional per-node label for the tree column
//! - [`IntoRow`] — convert an element into non-tree [`Row`] cells
//! - [`RenderableTree<Q>`] — renders the tree column and produces [`TableRow`]s
//! - [`render_table`] — fits all columns to width and returns per-cell strings
//! - Column sizing strategies ([`MixedColSizer`], [`FixedColSizer`])
//! - [`fit_to_width`] — unicode-aware cell truncation / padding

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;
use std::ops::Range;

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
        TreeNode { element, children: Vec::new() }
    }
}

/// Minimal trait required for building a [`Forest`].
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

        // --- Pass 3 ---
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
// GhostNode
// =============================================================================

/// A borrowed node in a transformed (filtered + sorted) view of a [`Forest`].
///
/// `highlight_ranges` contains byte ranges **into the description string**
/// (`TreeDisplay::description()`) that matched the query.  They are produced
/// by [`TransformableForest::transform`] and shifted by the connector width
/// inside [`RenderableTree`].
///
/// `GhostNode`s are never stored — they live only for the duration of a
/// single `tree_rows()` or `tree_min_width()` call.
pub struct GhostNode<'a, T> {
    pub node: &'a TreeNode<T>,
    /// Byte ranges into `TreeDisplay::description()` that matched the query.
    /// Empty when there is no active query or no match information available.
    pub highlight_ranges: Vec<Range<usize>>,
    pub children: Vec<GhostNode<'a, T>>,
}

// =============================================================================
// TransformableForest<Q>
// =============================================================================

/// Produce a transformed (filtered, sorted, restructured) view of the forest
/// as a `Vec<GhostNode>` for a given query.
///
/// Implementors fill `GhostNode::highlight_ranges` with byte ranges into
/// `TreeDisplay::description()` that matched the query.  The default impl
/// on `Forest<T, S>` only filters and leaves `highlight_ranges` empty.
pub trait TransformableForest<Q> {
    type Item;
    fn transform<'a>(&'a self, query: &Q) -> Vec<GhostNode<'a, Self::Item>>;
}

/// Default implementation: filter only, no sorting, no highlight ranges.
impl<T, S, Q> TransformableForest<Q> for Forest<T, S>
where
    S: Eq + Hash + Clone,
    T: HasTreeShape<S> + ForestItem<Q>,
{
    type Item = T;

    fn transform<'a>(&'a self, query: &Q) -> Vec<GhostNode<'a, T>> {
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

/// Optional label shown in the tree column next to the connector.
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
pub struct Row<Id: Eq + Hash> {
    pub id: Id,
    pub cells: HashMap<ColumnId, String>,
}

// =============================================================================
// The fixed tree column id
// =============================================================================

/// The fixed [`ColumnId`] used for the tree column.
pub const TREE_COLUMN: &str = "tree";

// =============================================================================
// TreeCellRow — output of RenderableTree
// =============================================================================

/// A single row produced by [`RenderableTree::tree_rows`].
///
/// Contains the rendered tree cell (connector + description, fitted to width),
/// the highlight ranges **shifted to positions within `tree_cell`**, and the
/// original item id.
#[derive(Debug, Clone)]
pub struct TreeCellRow<Id: Eq + Hash + Clone> {
    pub id: Id,
    /// Connector + description, already fitted to `tree_col_width` display columns.
    pub tree_cell: String,
    /// Byte ranges within `tree_cell` that should be highlighted.
    /// These are the original `highlight_ranges` from [`GhostNode`] shifted
    /// right by the byte length of the connector prefix.
    pub highlight_ranges: Vec<Range<usize>>,
}

// =============================================================================
// RenderableTree<Q>
// =============================================================================

/// Renders the tree column for a given query, producing one [`TreeCellRow`]
/// per visible node.
///
/// This is a blanket-implemented supertrait of [`TransformableForest<Q>`] —
/// any type implementing `TransformableForest<Q>` where `Self::Item:
/// TreeDisplay` automatically gets `RenderableTree<Q>`.
pub trait RenderableTree<Q>: TransformableForest<Q>
where
    Self::Item: TreeDisplay,
{
    fn tree_rows<Id>(
        &self,
        query: &Q,
        tree_col_width: usize,
    ) -> Vec<TreeCellRow<Id>>
where
        Self::Item: IntoRow<Id = Id>,
        Id: Eq + Hash + Clone,
    {
        let ghost_roots = self.transform(query);
        let mut result = Vec::new();

        for ghost_root in &ghost_roots {
            // Stack: (ghost, depth, is_last, prefix)
            let mut stack: Vec<(&GhostNode<'_, Self::Item>, usize, bool, String)> =
            vec![(ghost_root, 0, true, String::new())];

            while let Some((ghost, depth, is_last, prefix)) = stack.pop() {
                let elem = &ghost.node.element;
                let desc = elem.description();
                let has_desc = desc.is_some();
                let has_children = !ghost.children.is_empty();

                let connector =
                forest_connector(depth, is_last, &prefix, has_desc, has_children);
                let connector_byte_len = connector.len();

                // Shift highlight ranges by the connector byte length.
                let highlight_ranges: Vec<Range<usize>> = ghost
                    .highlight_ranges
                    .iter()
                    .map(|r| (r.start + connector_byte_len)..(r.end + connector_byte_len))
                    .collect();

                let raw_cell = match desc {
                    Some(d) => format!("{}{}", connector, d),
                    None => connector,
                };

                // Konvertiere Byte-Ranges zu Char-Ranges
                let char_ranges = byte_ranges_to_char_ranges(&raw_cell, &highlight_ranges);

                let (tree_cell, final_highlight_ranges) = 
                fit_to_width_with_char_highlights(&raw_cell, tree_col_width, &char_ranges);

                result.push(TreeCellRow {
                    id: elem.into_row().id,
                    tree_cell,
                    highlight_ranges: final_highlight_ranges,
                });

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

/// Passt den String auf `width` Breite an und projiziert die Char-Ranges
/// (bezogen auf den vollen String) auf den getrimmten String.
/// Gibt (trimmed_string, char_ranges_after_trim) zurück.
fn fit_to_width_with_char_highlights(
    s: &str,
    width: usize,
    char_ranges: &[Range<usize>],  // bereits Char-Ranges, bezogen auf s
) -> (String, Vec<Range<usize>>) {
    use unicode_width::UnicodeWidthChar;
    use unicode_width::UnicodeWidthStr;

    let display_width = s.width();
    if display_width <= width {
        let padding = width - display_width;
        let padded = format!("{}{}", s, " ".repeat(padding));
        return (padded, char_ranges.to_vec());
    }

    // Zielbreite für den sichtbaren Text (ohne Ellipse)
    let target = width.saturating_sub(1);
    let mut trimmed_chars = Vec::new();          // Zeichen im sichtbaren Teil
    let mut full_to_trimmed = Vec::new();        // (full_index, trimmed_index)

    let mut used = 0;
    let mut full_idx = 0;
    for ch in s.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if used + ch_width > target {
            break;
        }
        trimmed_chars.push(ch);
        full_to_trimmed.push((full_idx, trimmed_chars.len() - 1));
        used += ch_width;
        full_idx += 1;
    }

    // Ergebnis-String: sichtbarer Teil + Ellipse
    let mut result = String::with_capacity(trimmed_chars.len() + 1);
    for ch in &trimmed_chars {
        result.push(*ch);
    }
    result.push('…');

    // Projektion der Ranges
    let mut final_ranges = Vec::new();
    for range in char_ranges {
        let start = range.start;
        let end = range.end;

        // Suche den ersten übernommenen Char, dessen full_idx >= start
        let start_trim = full_to_trimmed
            .iter()
            .find_map(|&(f, t)| if f >= start { Some(t) } else { None })
            .unwrap_or(trimmed_chars.len());

        // Suche den letzten übernommenen Char, dessen full_idx < end
        let end_trim = full_to_trimmed
            .iter()
            .rev()
            .find_map(|&(f, t)| if f < end { Some(t + 1) } else { None })
            .unwrap_or(start_trim);

        if start_trim < end_trim {
            final_ranges.push(start_trim..end_trim);
        }
    }

    (result, final_ranges)
}

/// Konvertiert Byte-Ranges in einem String zu Char-Ranges.
fn byte_ranges_to_char_ranges(s: &str, byte_ranges: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut char_starts: Vec<usize> = s.char_indices().map(|(i, _)| i).collect();
    char_starts.push(s.len());
    
    let mut result = Vec::new();
    for byte_range in byte_ranges {
        let start_char = char_starts
            .iter()
            .position(|&byte_idx| byte_idx >= byte_range.start)
            .unwrap_or(char_starts.len() - 1);
        let end_char = char_starts
            .iter()
            .position(|&byte_idx| byte_idx >= byte_range.end)
            .unwrap_or(char_starts.len() - 1);
        
        if start_char < end_char {
            result.push(start_char..end_char);
        }
    }
    result
}

/// Blanket impl: every `TransformableForest<Q>` whose `Item: TreeDisplay`
/// gets `RenderableTree<Q>` for free.
impl<Q, F> RenderableTree<Q> for F
where
    F: TransformableForest<Q>,
    F::Item: TreeDisplay,
{
}

// =============================================================================
// TableRow — output of render_table
// =============================================================================

/// A fully laid-out row ready for the TUI to paint.
///
/// `cells` contains one already-fitted string per column (in the same order
/// as the `cols` slice passed to [`render_table`]).  `highlight_ranges`
/// applies to the tree column only (index 0 when the tree column is first).
#[derive(Debug, Clone)]
pub struct TableRow<Id: Eq + Hash + Clone> {
    pub id: Id,
    /// One fitted string per column, in `cols` order.
    pub cells: Vec<String>,
    /// Byte ranges within `cells[tree_col_index]` to highlight.
    pub highlight_ranges: Vec<Range<usize>>,
}

// =============================================================================
// render_table
// =============================================================================

/// Combine [`TreeCellRow`]s with additional [`Row`] data and fit everything
/// to the given layout, producing one [`TableRow`] per row.
///
/// `tree_col_index` is the position of `ColumnId(TREE_COLUMN)` in `cols` —
/// typically 0.  The tree cell string is taken from `tree_rows`; all other
/// columns are looked up in the corresponding [`Row`] from `data_rows`.
///
/// `data_rows` must be in the same order as `tree_rows` (both come from a
/// single `transform` call on the same forest).
///
/// An optional `header` row is prepended with empty `highlight_ranges`.
pub fn render_table<Id>(
    tree_rows: Vec<TreeCellRow<Id>>,
    data_rows: Vec<Row<Id>>,
    layout: &TableLayout,
    cols: &[ColumnId],
    header: Option<Row<Id>>,
) -> Vec<TableRow<Id>>
where
    Id: Eq + Hash + Clone,
{
    // Build a combined Row<Id> for sizing (tree cell + data cells merged).
    let sizing_rows: Vec<Row<Id>> = tree_rows
        .iter()
        .zip(data_rows.iter())
        .map(|(tr, dr)| {
            let mut cells = dr.cells.clone();
            cells.insert(ColumnId::new(TREE_COLUMN), tr.tree_cell.clone());
            Row { id: tr.id.clone(), cells }
        })
        .collect();

    let widths =
        layout
            .sizer
            .col_widths(cols, &sizing_rows, layout.max_width, &layout.separator);

    let tree_col_id = ColumnId::new(TREE_COLUMN);

    let render_one = |id: Id,
                      tree_cell: Option<&str>,
                      data: &Row<Id>,
                      highlight_ranges: Vec<Range<usize>>|
     -> TableRow<Id> {
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
        TableRow { id, cells, highlight_ranges }
    };

    let mut result = Vec::new();

    // Optional header (no highlights).
    if let Some(h) = header {
        let dummy_tree = h
            .cells
            .get(&tree_col_id)
            .map(|s| s.as_str())
            .unwrap_or("");
        result.push(render_one(h.id.clone(), Some(dummy_tree), &h, vec![]));
    }

    // Data rows.
    for (tr, dr) in tree_rows.into_iter().zip(data_rows.into_iter()) {
        result.push(render_one(
            tr.id,
            Some(&tr.tree_cell),
            &dr,
            tr.highlight_ranges,
        ));
    }

    result
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
    /// (connectors only, without description text) for all nodes matching
    /// `query`.
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

                let connector =
                    forest_connector(depth, is_last, &prefix, has_desc, has_children);
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
        (true, false)  => format!("{}├── ", prefix),
        (true, true)   => format!("{}└── ", prefix),
        (false, false) => format!("{}├───┐", prefix),
        (false, true)  => format!("{}└───┐", prefix),
    }
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
// fit_to_width
// =============================================================================

/// Truncate or pad a string to exactly `width` display columns.
pub fn fit_to_width(s: &str, width: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    use unicode_width::UnicodeWidthStr;

    let display_width = s.width();

    if display_width <= width {
        let padding = width - display_width;
        format!("{}{}", s, " ".repeat(padding))
    } else {
        let target = width.saturating_sub(1);
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
        result.push('…');
        result
    }
}

// =============================================================================
// Original tree-rendering helpers — kept for backwards compatibility
// =============================================================================

/// Build the connector prefix string for a node at a given depth (original helper).
pub fn tree_connector(depth: usize, is_last: bool, prefix: &str) -> String {
    if depth == 0 {
        String::new()
    } else if is_last {
        format!("{}└── ", prefix)
    } else {
        format!("{}├── ", prefix)
    }
}

/// Build the prefix string to pass to children (original helper).
pub fn child_prefix(depth: usize, is_last: bool, prefix: &str) -> String {
    if depth == 0 {
        prefix.to_string()
    } else if is_last {
        format!("{}    ", prefix)
    } else {
        format!("{}│   ", prefix)
    }
}
