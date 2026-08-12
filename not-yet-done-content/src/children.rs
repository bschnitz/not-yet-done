//! The single source of truth about a node's children.
//!
//! Historically three separate `Node` methods each said something about the
//! children of a node — which child *types* exist, how a child type's lists
//! sort, and how to *fetch* a child type's instances. Three
//! independently-maintained surfaces can drift: a `list` match arm without a
//! declared child type, a declared type `list` can't serve, a sortable column
//! for a type that isn't a child.
//!
//! [`ContentAdapter::childs`](crate::ContentAdapter::childs) collapses all
//! three into one function. For each child it yields, in one place: the
//! [`NodeType`], its columns, and a **not-yet-executed** fetch callback
//! (`list` "without the await"). Everything else — [`child_types`],
//! [`columns_for`], [`list`], [`list_subtree`] — is *derived* from it
//! as a free function, so a child type without a fetcher (or vice versa) is no
//! longer expressible.

use std::future::Future;
use std::pin::Pin;

use crate::{
    ColumnSchema, ContentAdapter, ContentError, ListParams, ListResult, Node, NodeSummary,
    NodeType, Result, SortKey, Subtree, SubtreeNode,
};

/// A boxed, `Send` future — the shape `#[async_trait]` methods already lower to,
/// so a [`Child::list`] callback can hand back an adapter's `async fn` result
/// directly.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One child kind reachable under a node: its type, the columns its lists
/// carry, and a lazy fetcher for its instances. The `list` callback is
/// `FnOnce` — it is invoked at most once, when someone actually lists that
/// child type.
pub struct Child<'a> {
    /// The child node type.
    pub node_type: NodeType,
    /// The columns of this child type's lists — see [`ColumnSchema`]. Empty =
    /// the adapter declares nothing, so nothing can be sorted or locally
    /// filtered.
    ///
    /// Every column marked [`ColumnSchema::in_rows`] must appear as a metadata
    /// field on every row this child's `list` returns; [`check_rows`] is that
    /// promise made testable.
    pub columns: Vec<ColumnSchema>,
    /// Fetch this child type's instances — "like `list`, only without the
    /// `await`". Called with the [`ListParams`] whose `node_type` equals
    /// [`Child::node_type`].
    pub list: Box<dyn FnOnce(ListParams) -> BoxFuture<'a, Result<ListResult>> + Send + 'a>,
}

/// The child *types* reachable under `node`, projected from the one truth.
pub fn child_types(adapter: &dyn ContentAdapter, node: &dyn Node) -> Vec<NodeType> {
    adapter
        .childs(node)
        .into_iter()
        .map(|c| c.node_type)
        .collect()
}

/// Every column of `child_type` under `node`: the adapter's own declaration
/// unioned with whatever [`ContentAdapter::describe_columns`] adds for that
/// type. Empty if `child_type` is not a child of `node` and nothing is
/// described for it.
///
/// The union happens **here**, once, so no front-end has to merge two channels
/// itself. A described column wins over a declared one of the same key: it is
/// the more specific statement (a decorator knows its own storage, and the
/// user may have retyped the column).
///
/// Except for a label it does not have. A decorator often knows a column only
/// as a key and a type — a local store has no display name to offer — so an
/// absent label keeps the declared one instead of erasing it. Winning on the
/// fields it states says nothing about the fields it doesn't.
pub async fn columns_for(
    adapter: &dyn ContentAdapter,
    node: &dyn Node,
    child_type: &NodeType,
) -> Vec<ColumnSchema> {
    // Snapshot before the await: `Child` borrows `node`, and the lazy `list`
    // closure isn't `Sync`.
    let declared: Vec<ColumnSchema> = adapter
        .childs(node)
        .into_iter()
        .find(|c| &c.node_type == child_type)
        .map(|c| c.columns)
        .unwrap_or_default();

    let described = adapter.describe_columns(&child_type.type_id).await;
    let mut out = declared;
    for col in described {
        match out.iter_mut().find(|c| c.key == col.key) {
            Some(slot) => {
                let label = col
                    .label
                    .clone()
                    .filter(|l| !l.trim().is_empty())
                    .or_else(|| slot.label.clone());
                *slot = ColumnSchema { label, ..col };
            }
            None => out.push(col),
        }
    }
    out
}

/// The keys of `columns` that a row is required to carry — every column
/// declared [`ColumnSchema::in_rows`].
fn required_keys(columns: &[ColumnSchema]) -> Vec<&str> {
    columns
        .iter()
        .filter(|c| c.in_rows)
        .map(|c| c.key.as_str())
        .collect()
}

/// Check the [`ColumnSchema::in_rows`] promise against actual rows: every
/// column that claims to live in the rows must be present as a metadata field
/// on every one of them. Returns the offending `(row id, column key)` pairs —
/// empty means the rows conform.
///
/// A missing field is not the same as an empty one: a declared column may well
/// be blank for a given row (`cell` reports `None` either way). What is
/// checked here is that the adapter *projects* the column at all, because a
/// silently absent field is what used to make sorting and filtering guess.
///
/// [`list`] runs this as a `debug_assert`, so a violation fails tests and
/// debug builds and costs nothing in release. Adapters can call it directly on
/// their fixtures to catch the same mistake in a unit test.
pub fn check_rows<'a>(
    columns: &'a [ColumnSchema],
    rows: &'a [NodeSummary],
) -> Vec<(&'a str, &'a str)> {
    let required = required_keys(columns);
    let mut out = Vec::new();
    for row in rows {
        for key in &required {
            if !row.metadata.fields.iter().any(|f| &f.key == key) {
                out.push((row.id.as_str(), *key));
            }
        }
    }
    out
}

/// List `node`'s children of the type named by `params.node_type`, by locating
/// the matching [`Child`] and running its fetcher. The dynamic→static dispatch
/// that each adapter used to hand-write as a `match` on `type_id` now happens
/// here, once, over the single declaration.
///
/// Being the one place every list passes through, this is also where the
/// [`ColumnSchema::in_rows`] promise is checked (see [`check_rows`]) — as a
/// `debug_assert`, so a lying adapter fails loudly in tests and debug builds
/// and costs nothing in release — and where a sort the adapter could not
/// finish is finished for it (see [`finish_sort`]).
pub async fn list(
    adapter: &dyn ContentAdapter,
    node: &dyn Node,
    params: ListParams,
) -> Result<ListResult> {
    let target = params.node_type.clone();
    let child = adapter
        .childs(node)
        .into_iter()
        .find(|c| c.node_type == target);
    let Some(c) = child else {
        return Err(ContentError::NotSupported(format!(
            "'{}' has no child type '{}'",
            node.node_type().type_id,
            target.type_id
        )));
    };
    let columns = c.columns.clone();
    let sort = params.sort.clone();
    let mut result = (c.list)(params).await?;
    debug_assert!(
        check_rows(&columns, &result.items).is_empty(),
        "'{}' declares columns it does not carry in its rows: {:?}",
        target.type_id,
        check_rows(&columns, &result.items)
    );
    finish_sort(adapter, node, &target, &sort, &mut result).await;
    Ok(result)
}

/// Apply the part of `sort` the adapter left undone, when doing so is both
/// possible and an improvement.
///
/// An adapter can only sort by what it knows. A column a *decorator*
/// describes — a user's custom column, say — is invisible to it: the Jira
/// adapter turns the sort into a JQL `ORDER BY` and Jira has never heard of
/// `local_rank`, so the key is silently dropped and the list comes back in
/// some other order. The column was offered in the sort menu and did nothing,
/// which is the one outcome worse than not offering it.
///
/// Here the described columns are in hand ([`columns_for`]) and so are the
/// rows, so the sort can simply be finished. Three conditions, each of which
/// would otherwise trade one wrong order for another:
///
/// 1. **Only a result held in full.** One page of a server-side result is a
///    sample, not the query: reordering it locally would sort the page and
///    present it as the whole. Left alone, and [`ListResult::applied_sort`]
///    reports the key as unapplied rather than lying about it.
/// 2. **Only if nothing already honoured is lost.** A key the adapter served
///    server-side may name a column that carries no cell in the rows
///    ([`ColumnSchema::in_rows`] is `false`), which no local sort can compare.
///    Rescuing one key at the cost of another is not a fix.
/// 3. **Only if something is gained.** Otherwise every list would pay for a
///    re-sort that reproduces the order it already has.
///
/// When it does take over it re-sorts by the *whole* spec, not just the
/// missing keys: the requested key order is the meaning of the request, and a
/// stable pass over the leftovers alone would silently promote a secondary key
/// to primary.
async fn finish_sort(
    adapter: &dyn ContentAdapter,
    node: &dyn Node,
    child_type: &NodeType,
    sort: &[SortKey],
    result: &mut ListResult,
) {
    if sort.is_empty() || sort.iter().all(|k| result.applied_sort.contains(k)) {
        return;
    }
    let complete = result
        .page
        .as_ref()
        .is_none_or(|p| !p.has_next && !p.has_prev);
    if !complete {
        return;
    }
    let columns = columns_for(adapter, node, child_type).await;
    let honoured = crate::honoured_sort_keys(sort, &columns);
    let keeps_everything = result.applied_sort.iter().all(|k| honoured.contains(k));
    if !keeps_everything || honoured.len() <= result.applied_sort.len() {
        return;
    }
    result.applied_sort = crate::apply_sort(&mut result.items, sort, &columns);
}

/// List `node`'s children and eagerly expand their descendants up to `depth`
/// additional levels. Generic over any adapter; the free-function twin of the
/// former `Node::list_subtree`. Descends by resolving each child via
/// [`ContentAdapter::get_by_id`] and recursing through [`child_types`].
///
/// Depth semantics match the old contract (total visible levels = `depth + 1`);
/// see [`crate::Subtree`]. `params.query` is threaded into every level; child
/// levels are requested unpaginated.
pub async fn list_subtree(
    adapter: &dyn ContentAdapter,
    node: &dyn Node,
    params: ListParams,
    depth: u32,
) -> Result<Subtree> {
    // Adapters that own their whole tree (local Tasks/Trackings) build the
    // expanded, per-level-sorted subtree in one pass. The generic recursion
    // below can't carry `params.sort` past the first level, so it is only the
    // fallback for adapters without an eager path.
    if let Some(result) = adapter.eager_subtree(node, &params, depth).await {
        return result;
    }
    let query = params.query.clone();
    let result = list(adapter, node, params).await?;
    let page = result.page;
    let mut items = Vec::with_capacity(result.items.len());
    for summary in result.items {
        // Only descend when we have budget AND the node isn't a known leaf. A
        // node that claims children but can't be re-resolved is treated as a
        // leaf here (graceful) rather than failing the whole subtree.
        let children = if depth > 0 && summary.has_children != Some(false) {
            match adapter.get_by_id(&summary.id).await {
                Ok(child) => {
                    let mut merged = Subtree::default();
                    for child_type in child_types(adapter, child.as_ref()) {
                        let child_params = ListParams {
                            node_type: child_type,
                            query: query.clone(),
                            sort: Vec::new(),
                            page: None,
                            download: false,
                            group_by: None,
                        };
                        let mut sub = Box::pin(list_subtree(
                            adapter,
                            child.as_ref(),
                            child_params,
                            depth - 1,
                        ))
                        .await?;
                        merged.items.append(&mut sub.items);
                        // Single child-type is the common case; keep the first
                        // level's page. Multi-type local adapters load
                        // all-or-nothing (page stays None).
                        if merged.page.is_none() {
                            merged.page = sub.page;
                        }
                    }
                    merged
                }
                Err(_) => Subtree::default(),
            }
        } else {
            Subtree::default()
        };
        items.push(SubtreeNode { summary, children });
    }
    Ok(Subtree { items, page })
}
