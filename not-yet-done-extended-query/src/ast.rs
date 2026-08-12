//! The parsed form of an extended-query document.
//!
//! Deliberately free of any frontend or adapter type: an `ExtendedQuery` is
//! plain data that the planner and executor consume, so parsing can be tested
//! without a backend. Ordering uses this crate's own [`OrderKey`] rather than
//! `not_yet_done_content::SortKey` for the same reason — the executor maps
//! between them.

use not_yet_done_filter::FilterExpr;

/// A whole document: one root node plus the document-level ordering.
///
/// An empty `order_by` means merge order (tree walked left to right, first
/// occurrence fixing the position), which is what makes a single-branch
/// document a true pass-through of the adapter's own ordering.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtendedQuery {
    pub root: Node,
    pub order_by: Vec<OrderKey>,
}

/// One node of the set-algebra tree, with the attributes it may carry.
///
/// `local_filter` and `limit` are attributes rather than sibling operands:
/// the node they hang on brings its own result set, so "filtered against
/// which base set?" cannot arise.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub kind: NodeKind,
    /// Rows produced by this node are kept only when the expression matches.
    pub local_filter: Option<FilterExpr>,
    /// Upper bound on the rows fetched for this node.
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    /// A backend round-trip: one adapter-native query.
    Fetch(Fetch),
    /// Intersection of all operands.
    And(Vec<Node>),
    /// Union of all operands.
    Or(Vec<Node>),
    /// First operand minus every following one.
    Without(Vec<Node>),
}

/// A single adapter-native query, with the fence language it was declared in
/// (`None` for an inline `query:`, which is implicitly the adapter's own).
#[derive(Debug, Clone, PartialEq)]
pub struct Fetch {
    pub text: String,
    pub language: Option<String>,
    pub source: FetchSource,
}

/// Where a fetch's text came from — kept so error messages can name the
/// library entry rather than quoting an anonymous blob of query text.
#[derive(Debug, Clone, PartialEq)]
pub enum FetchSource {
    Inline,
    Ref(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Asc,
    Desc,
}

/// One sort key. Significance comes from the position in
/// [`ExtendedQuery::order_by`], never from a YAML mapping's key order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderKey {
    pub column: String,
    pub direction: Direction,
}

impl Node {
    /// Every fetch in this subtree, in left-to-right walk order — the same
    /// order that defines the default merge ordering.
    pub fn fetches(&self) -> Vec<&Fetch> {
        let mut out = Vec::new();
        self.collect_fetches(&mut out);
        out
    }

    fn collect_fetches<'a>(&'a self, out: &mut Vec<&'a Fetch>) {
        match &self.kind {
            NodeKind::Fetch(f) => out.push(f),
            NodeKind::And(ops) | NodeKind::Or(ops) | NodeKind::Without(ops) => {
                for op in ops {
                    op.collect_fetches(out);
                }
            }
        }
    }
}

impl ExtendedQuery {
    /// Every fetch in the document, in walk order.
    pub fn fetches(&self) -> Vec<&Fetch> {
        self.root.fetches()
    }
}
