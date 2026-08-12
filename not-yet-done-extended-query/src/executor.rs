//! Running a parsed document against one adapter.
//!
//! The executor is the only part of the crate that talks to a backend, and it
//! does so through [`Backend`] — three methods the adapter already has
//! (`query_variables`, `render_query`, and a listing round-trip). Depending on
//! `ContentAdapter` itself would drag a node, a parent id and a whole
//! `ListParams` into a crate whose job is set algebra; the caller owns that
//! plumbing and hands down something that can answer "run this query text".
//!
//! # Order of operations per node
//!
//! 1. produce the node's input set (a fetch, or the combination of operands),
//! 2. cut it to `limit`,
//! 3. keep what `local_filter` matches.
//!
//! `limit` before `local_filter` is what lets it bound a *round-trip* at all:
//! a fetch node's limit is passed to the backend, and a filter that ran first
//! could only ever shrink what was already paid for. The same order then
//! applies to set nodes so one rule covers every node.
//!
//! # What is fetched, and how often
//!
//! Every fetch in the document is planned first, deduplicated by its rendered
//! text *and* its fetch budget, and then run concurrently. Two branches that
//! render to the same query text therefore cost one round-trip — a document
//! that subtracts a query from a superset of itself is a normal thing to
//! write, and paying twice for it would be a silent tax.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};

use not_yet_done_content::{
    ColumnSchema, NodeSummary, QueryVariable, SortDirection, SortKey, apply_sort,
};
use not_yet_done_filter::{FilterExpr, eval};

use crate::ast::{Direction, ExtendedQuery, Fetch, FetchSource, Node, NodeKind};
use crate::rows::{ColumnTypes, SummaryRow};

/// What the executor needs from the adapter it runs against.
#[async_trait::async_trait]
pub trait Backend: Send + Sync {
    /// The variables referenced in a raw query text, in the adapter's own
    /// inline syntax. Mirrors `ContentAdapter::query_variables`.
    fn query_variables(&self, query: &str) -> Vec<QueryVariable> {
        let _ = query;
        Vec::new()
    }

    /// Substitute bindings into a raw query text. Mirrors
    /// `ContentAdapter::render_query`.
    fn render_query(&self, query: &str, vars: &HashMap<String, String>) -> String {
        let _ = vars;
        query.to_string()
    }

    /// One backend round-trip: list everything the rendered `query` selects.
    ///
    /// `limit` is a plain "at most this many rows", and the executor asks for
    /// one row more than the node keeps — that extra row is what tells a
    /// complete result from a cut one, so truncation can be reported instead
    /// of silently turning 5000 hits into the first 100. A backend that cannot
    /// bound its fetch may ignore the argument; the executor cuts either way.
    async fn fetch(&self, query: &str, limit: Option<usize>) -> Result<Vec<NodeSummary>, String>;
}

/// Everything one run needs besides the document itself.
pub struct Run<'a> {
    pub backend: &'a dyn Backend,
    /// The union of adapter `SortableColumn`s and described `ColumnSchema`s —
    /// what `local_filter` and `order_by` resolve column references against.
    pub types: &'a ColumnTypes,
    /// Variable bindings, gathered once via [`variables`] and used for every
    /// branch.
    pub bindings: &'a HashMap<String, String>,
    /// Called as `(done, total)` each time a planned fetch completes, so a
    /// frontend can show "3/5 branches" while the run is in flight.
    pub progress: Option<&'a (dyn Fn(usize, usize) + Send + Sync)>,
}

impl<'a> Run<'a> {
    pub fn new(
        backend: &'a dyn Backend,
        types: &'a ColumnTypes,
        bindings: &'a HashMap<String, String>,
    ) -> Self {
        Self {
            backend,
            types,
            bindings,
            progress: None,
        }
    }
}

/// The result of one run.
#[derive(Debug)]
pub struct Execution {
    pub items: Vec<NodeSummary>,
    /// The `order_by` keys that were actually applied — what the pane reports
    /// back so the header arrows stay truthful. Empty means merge order.
    pub applied_sort: Vec<SortKey>,
    /// Things the user should know but that must not fail the run.
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// Several branches were merged without an explicit order.
    NativeOrderIgnored,
    /// A node hit its `limit`, so the set below it is incomplete.
    Truncated { what: String, limit: usize },
    /// The same variable carries different defaults in different branches.
    ConflictingDefault {
        name: String,
        kept: Option<String>,
        ignored: Option<String>,
    },
}

impl std::fmt::Display for Warning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NativeOrderIgnored => write!(
                f,
                "branches were combined without an `order_by`, so no backend's own \
                 ordering survives — rows appear in merge order"
            ),
            Self::Truncated { what, limit } => write!(
                f,
                "{what} hit its limit of {limit} rows; the result is incomplete"
            ),
            Self::ConflictingDefault {
                name,
                kept,
                ignored,
            } => {
                let show = |v: &Option<String>| v.clone().unwrap_or_else(|| "none".to_string());
                write!(
                    f,
                    "variable `{name}` declares different defaults across branches; \
                     keeping `{}`, ignoring `{}`",
                    show(kept),
                    show(ignored)
                )
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExecError {
    #[error("{what} failed: {message}")]
    Fetch { what: String, message: String },

    #[error("{what}: {message}")]
    Filter { what: String, message: String },

    #[error("cannot order by unknown column `{column}`; known columns: {}", known.join(", "))]
    UnknownSortColumn { column: String, known: Vec<String> },
}

/// Every variable the document references, deduplicated across branches.
///
/// A `${key}` used in three branches is prompted once and bound once, so the
/// branches cannot drift apart on the same name. The first declaration wins;
/// a differing default further down is reported rather than silently dropped.
pub fn variables(
    query: &ExtendedQuery,
    backend: &dyn Backend,
) -> (Vec<QueryVariable>, Vec<Warning>) {
    let mut out: Vec<QueryVariable> = Vec::new();
    let mut warnings = Vec::new();
    for fetch in query.fetches() {
        for var in backend.query_variables(&fetch.text) {
            match out.iter().position(|v| v.name == var.name) {
                Some(pos) if out[pos].default != var.default => {
                    warnings.push(Warning::ConflictingDefault {
                        name: var.name.clone(),
                        kept: out[pos].default.clone(),
                        ignored: var.default.clone(),
                    });
                }
                Some(_) => {}
                None => out.push(var),
            }
        }
    }
    (out, warnings)
}

/// Run a document: fetch every branch, apply the set algebra, filter, sort.
pub async fn execute(query: &ExtendedQuery, run: &Run<'_>) -> Result<Execution, ExecError> {
    let plan = plan_fetches(&query.root, run);
    let results = run_plan(&plan, run).await;

    let mut warnings = Vec::new();
    let mut items = evaluate(&query.root, "spec", run, &results, &mut warnings)?;

    if query.order_by.is_empty() {
        if query.fetches().len() > 1 {
            warnings.push(Warning::NativeOrderIgnored);
        }
        return Ok(Execution {
            items,
            applied_sort: Vec::new(),
            warnings,
        });
    }

    let applied_sort = sort(&mut items, query, run)?;
    Ok(Execution {
        items,
        applied_sort,
        warnings,
    })
}

/// A planned round-trip: the rendered query text and the number of rows to ask
/// for. Both belong in the key — the same text fetched under two different
/// limits is two different results.
type FetchKey = (String, Option<usize>);

/// The budget for a node that keeps at most `limit` rows: one more, so a full
/// result can be told from a cut one.
fn budget(limit: Option<usize>) -> Option<usize> {
    limit.map(|l| l.saturating_add(1))
}

fn plan_fetches(root: &Node, run: &Run<'_>) -> Vec<FetchKey> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    collect_plan(root, run, &mut out, &mut seen);
    out
}

fn collect_plan(node: &Node, run: &Run<'_>, out: &mut Vec<FetchKey>, seen: &mut HashSet<FetchKey>) {
    match &node.kind {
        NodeKind::Fetch(fetch) => {
            let key = fetch_key(fetch, node.limit, run);
            if seen.insert(key.clone()) {
                out.push(key);
            }
        }
        NodeKind::And(ops) | NodeKind::Or(ops) | NodeKind::Without(ops) => {
            for op in ops {
                collect_plan(op, run, out, seen);
            }
        }
    }
}

fn fetch_key(fetch: &Fetch, limit: Option<usize>, run: &Run<'_>) -> FetchKey {
    (
        run.backend.render_query(&fetch.text, run.bindings),
        budget(limit),
    )
}

async fn run_plan(
    plan: &[FetchKey],
    run: &Run<'_>,
) -> HashMap<FetchKey, Result<Vec<NodeSummary>, String>> {
    let done = AtomicUsize::new(0);
    let total = plan.len();
    let results = futures::future::join_all(plan.iter().map(|key| {
        let done = &done;
        async move {
            let outcome = run.backend.fetch(&key.0, key.1).await;
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if let Some(progress) = run.progress {
                progress(n, total);
            }
            (key.clone(), outcome)
        }
    }))
    .await;
    results.into_iter().collect()
}

fn evaluate(
    node: &Node,
    path: &str,
    run: &Run<'_>,
    results: &HashMap<FetchKey, Result<Vec<NodeSummary>, String>>,
    warnings: &mut Vec<Warning>,
) -> Result<Vec<NodeSummary>, ExecError> {
    let mut items = match &node.kind {
        NodeKind::Fetch(fetch) => {
            let key = fetch_key(fetch, node.limit, run);
            match results.get(&key) {
                Some(Ok(rows)) => rows.clone(),
                Some(Err(message)) => {
                    return Err(ExecError::Fetch {
                        what: describe(fetch, path),
                        message: message.clone(),
                    });
                }
                // `plan_fetches` walks the very tree evaluated here.
                None => unreachable!("every fetch was planned"),
            }
        }
        NodeKind::Or(ops) => {
            let mut merged: Vec<NodeSummary> = Vec::new();
            let mut seen = HashSet::new();
            for (i, op) in ops.iter().enumerate() {
                for row in evaluate(op, &format!("{path}.or[{i}]"), run, results, warnings)? {
                    // First occurrence wins, hydrated fields and all: a row the
                    // user sees twice in two branches must not change shape
                    // depending on which branch happened to answer first.
                    if seen.insert(row.id.clone()) {
                        merged.push(row);
                    }
                }
            }
            merged
        }
        NodeKind::And(ops) => {
            let mut kept = evaluate(&ops[0], &format!("{path}.and[0]"), run, results, warnings)?;
            for (i, op) in ops.iter().enumerate().skip(1) {
                let other = ids(&evaluate(
                    op,
                    &format!("{path}.and[{i}]"),
                    run,
                    results,
                    warnings,
                )?);
                kept.retain(|row| other.contains(&row.id));
            }
            dedup_by_id(kept)
        }
        NodeKind::Without(ops) => {
            let mut kept = evaluate(
                &ops[0],
                &format!("{path}.without[0]"),
                run,
                results,
                warnings,
            )?;
            for (i, op) in ops.iter().enumerate().skip(1) {
                let other = ids(&evaluate(
                    op,
                    &format!("{path}.without[{i}]"),
                    run,
                    results,
                    warnings,
                )?);
                kept.retain(|row| !other.contains(&row.id));
            }
            dedup_by_id(kept)
        }
    };

    if let Some(limit) = node.limit
        && items.len() > limit
    {
        items.truncate(limit);
        warnings.push(Warning::Truncated {
            what: describe_node(node, path),
            limit,
        });
    }

    if let Some(expr) = &node.local_filter {
        filter(&mut items, expr, path, run.types)?;
    }

    Ok(items)
}

fn ids(items: &[NodeSummary]) -> HashSet<String> {
    items.iter().map(|s| s.id.clone()).collect()
}

/// Keep the first row per id. Only the operand sets can carry duplicates (an
/// adapter returning the same row twice); the combining step above already
/// dedups by construction for `or`.
fn dedup_by_id(items: Vec<NodeSummary>) -> Vec<NodeSummary> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|row| seen.insert(row.id.clone()))
        .collect()
}

fn filter(
    items: &mut Vec<NodeSummary>,
    expr: &FilterExpr,
    path: &str,
    types: &ColumnTypes,
) -> Result<(), ExecError> {
    // An empty set has no columns to validate against, and there is nothing
    // left to filter anyway. Rejecting a perfectly good column name just
    // because this run happened to return no rows would be worse than letting
    // a typo through in exactly the case where it changes nothing.
    if items.is_empty() {
        return Ok(());
    }
    let known = known_columns(types, items);
    let borrowed: Vec<&str> = known.iter().map(String::as_str).collect();
    eval::validate_columns(expr, &borrowed, "column").map_err(|message| ExecError::Filter {
        what: format!("{path}.local_filter"),
        message,
    })?;
    items.retain(|row| eval::matches(expr, &SummaryRow::new(row, types)));
    Ok(())
}

/// Every column a query may name: the typed ones plus every metadata key the
/// rows actually carry.
///
/// The typed set alone would be too narrow — an adapter advertises the columns
/// it can *sort* on, while its rows routinely carry more. Those extra columns
/// compare as text (see [`SummaryRow::field`]), which is the same treatment
/// they get everywhere else. What the union still catches is the typo: a
/// column no row has would otherwise fall back to the label and compare
/// against something entirely unrelated.
fn known_columns(types: &ColumnTypes, items: &[NodeSummary]) -> Vec<String> {
    let mut out: Vec<String> = types.keys().into_iter().map(str::to_string).collect();
    let mut seen: HashSet<String> = out.iter().cloned().collect();
    for row in items {
        for field in &row.metadata.fields {
            if seen.insert(field.key.clone()) {
                out.push(field.key.clone());
            }
        }
    }
    out
}

fn sort(
    items: &mut [NodeSummary],
    query: &ExtendedQuery,
    run: &Run<'_>,
) -> Result<Vec<SortKey>, ExecError> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let known = known_columns(run.types, items);
    let mut keys = Vec::with_capacity(query.order_by.len());
    for order in &query.order_by {
        // `apply_sort` silently drops what it cannot resolve, which is right
        // for the interactive `S` sort (the user picks from a list) and wrong
        // here: an `order_by` key was typed deliberately, and a document that
        // quietly ignores it looks like a broken sort.
        if !known.iter().any(|k| k == &order.column) {
            return Err(ExecError::UnknownSortColumn {
                column: order.column.clone(),
                known,
            });
        }
        keys.push(SortKey {
            column: order.column.clone(),
            direction: match order.direction {
                Direction::Asc => SortDirection::Asc,
                Direction::Desc => SortDirection::Desc,
            },
        });
    }
    // Every known column is sortable here and present in the rows by
    // construction: `known_columns` is the declared list plus every field the
    // fetched rows actually carry, and an extended query sorts the whole set
    // it holds rather than one server-side page.
    let columns: Vec<ColumnSchema> = known
        .iter()
        .map(|key| {
            let mut col = ColumnSchema::new(key.clone(), key.clone());
            if let Some(kind) = run.types.kind(key) {
                col = col.typed(match kind {
                    not_yet_done_content::SortKind::Number => "number",
                    not_yet_done_content::SortKind::DateTime => "datetime",
                    not_yet_done_content::SortKind::Text => "text",
                });
            }
            col
        })
        .collect();
    Ok(apply_sort(items, &keys, &columns))
}

fn describe(fetch: &Fetch, path: &str) -> String {
    match &fetch.source {
        FetchSource::Ref(name) => format!("fence `{name}`"),
        FetchSource::Inline => format!("the query at `{path}`"),
    }
}

fn describe_node(node: &Node, path: &str) -> String {
    match &node.kind {
        NodeKind::Fetch(fetch) => describe(fetch, path),
        _ => format!("the branch at `{path}`"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use not_yet_done_content::{Metadata, MetadataField, NodeType};
    use std::sync::Mutex;

    /// A backend answering from a fixed table, counting its round-trips.
    #[derive(Default)]
    struct Fake {
        answers: HashMap<String, Vec<NodeSummary>>,
        failures: HashMap<String, String>,
        calls: Mutex<Vec<String>>,
        variables: HashMap<String, Vec<QueryVariable>>,
    }

    impl Fake {
        fn answering(pairs: &[(&str, &[(&str, &[(&str, &str)])])]) -> Self {
            let mut answers = HashMap::new();
            for (query, rows) in pairs {
                answers.insert(
                    (*query).to_string(),
                    rows.iter()
                        .map(|(id, fields)| summary(id, fields))
                        .collect(),
                );
            }
            Self {
                answers,
                ..Self::default()
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl Backend for Fake {
        fn query_variables(&self, query: &str) -> Vec<QueryVariable> {
            self.variables.get(query).cloned().unwrap_or_default()
        }

        fn render_query(&self, query: &str, vars: &HashMap<String, String>) -> String {
            let mut out = query.to_string();
            for (k, v) in vars {
                out = out.replace(&format!("${{{k}}}"), v);
            }
            out
        }

        async fn fetch(
            &self,
            query: &str,
            limit: Option<usize>,
        ) -> Result<Vec<NodeSummary>, String> {
            self.calls.lock().unwrap().push(query.to_string());
            if let Some(message) = self.failures.get(query) {
                return Err(message.clone());
            }
            let mut rows = self.answers.get(query).cloned().unwrap_or_default();
            if let Some(limit) = limit {
                rows.truncate(limit);
            }
            Ok(rows)
        }
    }

    fn summary(id: &str, fields: &[(&str, &str)]) -> NodeSummary {
        NodeSummary {
            id: id.to_string(),
            label: format!("label of {id}"),
            node_type: NodeType {
                type_id: "row".into(),
                mime_type: "text/plain".into(),
                syntax: None,
                file_extension: ".txt".into(),
                display_name: "Row".into(),
            },
            metadata: Metadata {
                fields: fields
                    .iter()
                    .map(|(k, v)| MetadataField {
                        key: (*k).into(),
                        value: (*v).into(),
                        display_label: (*k).into(),
                        editable: false,
                        allowed_values: None,
                    })
                    .collect(),
            },
            has_children: None,
        }
    }

    fn types() -> ColumnTypes {
        ColumnTypes::new(&[ColumnSchema::new("effort", "Effort").typed("number")])
    }

    async fn run_spec(yaml: &str, backend: &Fake) -> Result<Execution, ExecError> {
        let query = crate::parse(&format!("```yaml\n{yaml}\n```\n")).unwrap();
        let types = types();
        let bindings = HashMap::new();
        super::execute(&query, &Run::new(backend, &types, &bindings)).await
    }

    fn ids_of(execution: &Execution) -> Vec<&str> {
        execution.items.iter().map(|s| s.id.as_str()).collect()
    }

    #[tokio::test]
    async fn a_union_keeps_the_first_occurrence_of_a_row() {
        let backend = Fake::answering(&[
            ("mine", &[("a", &[]), ("b", &[])]),
            ("theirs", &[("b", &[]), ("c", &[])]),
        ]);
        let out = run_spec("or:\n  - query: mine\n  - query: theirs", &backend)
            .await
            .unwrap();
        assert_eq!(ids_of(&out), vec!["a", "b", "c"]);
    }

    #[tokio::test]
    async fn an_intersection_keeps_the_first_operands_order() {
        let backend = Fake::answering(&[
            ("mine", &[("c", &[]), ("a", &[]), ("b", &[])]),
            ("open", &[("a", &[]), ("c", &[])]),
        ]);
        let out = run_spec("and:\n  - query: mine\n  - query: open", &backend)
            .await
            .unwrap();
        assert_eq!(ids_of(&out), vec!["c", "a"]);
    }

    #[tokio::test]
    async fn without_subtracts_every_later_operand() {
        let backend = Fake::answering(&[
            ("all", &[("a", &[]), ("b", &[]), ("c", &[])]),
            ("done", &[("b", &[])]),
            ("mine", &[("c", &[])]),
        ]);
        let out = run_spec(
            "without:\n  - query: all\n  - query: done\n  - query: mine",
            &backend,
        )
        .await
        .unwrap();
        assert_eq!(ids_of(&out), vec!["a"]);
    }

    #[tokio::test]
    async fn the_same_rendered_text_is_fetched_once() {
        let backend =
            Fake::answering(&[("all", &[("a", &[]), ("b", &[])]), ("done", &[("b", &[])])]);
        let out = run_spec(
            "or:\n  - without:\n      - query: all\n      - query: done\n  - query: all",
            &backend,
        )
        .await
        .unwrap();
        assert_eq!(ids_of(&out), vec!["a", "b"]);
        assert_eq!(
            backend.calls().iter().filter(|q| *q == "all").count(),
            1,
            "the branch shared by both operands must cost one round-trip"
        );
    }

    #[tokio::test]
    async fn a_local_filter_runs_against_the_rows_that_came_back() {
        let backend = Fake::answering(&[(
            "all",
            &[
                ("a", &[("effort", "3")] as &[(&str, &str)]),
                ("b", &[("effort", "20")]),
            ],
        )]);
        let out = run_spec("query: all\nlocal_filter:\n  - [effort, '>', 5]", &backend)
            .await
            .unwrap();
        assert_eq!(ids_of(&out), vec!["b"], "compared as a number, not as text");
    }

    #[tokio::test]
    async fn a_filter_on_an_untyped_metadata_column_compares_as_text() {
        let backend = Fake::answering(&[(
            "all",
            &[
                ("a", &[("status", "open")] as &[(&str, &str)]),
                ("b", &[("status", "done")]),
            ],
        )]);
        let out = run_spec(
            "query: all\nlocal_filter:\n  - [status, '=', open]",
            &backend,
        )
        .await
        .unwrap();
        assert_eq!(ids_of(&out), vec!["a"]);
    }

    #[tokio::test]
    async fn a_filter_on_a_column_no_row_carries_is_an_error() {
        let backend =
            Fake::answering(&[("all", &[("a", &[("status", "open")] as &[(&str, &str)])])]);
        let err = run_spec(
            "query: all\nlocal_filter:\n  - [statsu, '=', open]",
            &backend,
        )
        .await
        .unwrap_err();
        // Without the check this would silently compare against the label.
        assert!(
            matches!(&err, ExecError::Filter { what, .. } if what == "spec.local_filter"),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn a_limit_cuts_the_set_and_says_so() {
        let backend = Fake::answering(&[("all", &[("a", &[]), ("b", &[]), ("c", &[])])]);
        let out = run_spec("query: all\nlimit: 2", &backend).await.unwrap();
        assert_eq!(ids_of(&out), vec!["a", "b"]);
        assert_eq!(
            out.warnings,
            vec![Warning::Truncated {
                what: "the query at `spec`".into(),
                limit: 2
            }]
        );
    }

    #[tokio::test]
    async fn a_limit_that_the_result_stays_under_is_not_reported() {
        let backend = Fake::answering(&[("all", &[("a", &[]), ("b", &[])])]);
        let out = run_spec("query: all\nlimit: 2", &backend).await.unwrap();
        assert_eq!(ids_of(&out), vec!["a", "b"]);
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    }

    #[tokio::test]
    async fn the_limit_bounds_the_fetch_by_one_extra_row() {
        // Asking for exactly the limit could never tell "two rows exist" from
        // "two of many were kept".
        let backend =
            Fake::answering(&[("all", &[("a", &[]), ("b", &[]), ("c", &[]), ("d", &[])])]);
        let out = run_spec("query: all\nlimit: 2", &backend).await.unwrap();
        assert_eq!(ids_of(&out), vec!["a", "b"]);
        assert_eq!(out.warnings.len(), 1);
    }

    #[tokio::test]
    async fn merge_order_is_the_default_and_is_flagged_once_branches_combine() {
        let backend = Fake::answering(&[("mine", &[("b", &[])]), ("theirs", &[("a", &[])])]);
        let out = run_spec("or:\n  - query: mine\n  - query: theirs", &backend)
            .await
            .unwrap();
        assert_eq!(ids_of(&out), vec!["b", "a"], "no reordering happened");
        assert_eq!(out.warnings, vec![Warning::NativeOrderIgnored]);
        assert!(out.applied_sort.is_empty());
    }

    #[tokio::test]
    async fn a_single_branch_document_is_a_pass_through() {
        let backend = Fake::answering(&[("all", &[("c", &[]), ("a", &[]), ("b", &[])])]);
        let out = run_spec("and:\n  - query: all", &backend).await.unwrap();
        assert_eq!(ids_of(&out), vec!["c", "a", "b"], "the adapter's own order");
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    }

    #[tokio::test]
    async fn order_by_sorts_through_the_column_types() {
        let backend = Fake::answering(&[(
            "all",
            &[
                ("a", &[("effort", "10")] as &[(&str, &str)]),
                ("b", &[("effort", "9")]),
            ],
        )]);
        let out = run_spec("query: all\norder_by:\n  - effort: asc", &backend)
            .await
            .unwrap();
        assert_eq!(ids_of(&out), vec!["b", "a"], "9 < 10 numerically");
        assert_eq!(out.applied_sort.len(), 1);
    }

    #[tokio::test]
    async fn an_unknown_order_by_column_is_an_error_not_a_silent_drop() {
        let backend = Fake::answering(&[("all", &[("a", &[("effort", "1")] as &[(&str, &str)])])]);
        let err = run_spec("query: all\norder_by:\n  - nonsense: asc", &backend)
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ExecError::UnknownSortColumn { column, .. } if column == "nonsense"),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn a_failing_branch_names_the_fence_it_came_from() {
        let mut backend = Fake::answering(&[("ok", &[("a", &[])])]);
        backend
            .failures
            .insert("broken".to_string(), "400 Bad Request".to_string());
        let source = "```yaml\nor:\n  - query: ok\n  - query-ref: theirs\n```\n\n\
                      ```jql theirs\nbroken\n```\n";
        let query = crate::parse(source).unwrap();
        let types = types();
        let bindings = HashMap::new();
        let err = super::execute(&query, &Run::new(&backend, &types, &bindings))
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "fence `theirs` failed: 400 Bad Request",
            "an anonymous blob of query text would be useless in a banner"
        );
    }

    #[tokio::test]
    async fn bindings_are_rendered_into_every_branch() {
        let backend = Fake::answering(&[("issues in PROJ", &[("a", &[])])]);
        let query = crate::parse("```yaml\nquery: issues in ${project}\n```\n").unwrap();
        let types = types();
        let bindings = HashMap::from([("project".to_string(), "PROJ".to_string())]);
        let out = super::execute(&query, &Run::new(&backend, &types, &bindings))
            .await
            .unwrap();
        assert_eq!(ids_of(&out), vec!["a"]);
    }

    #[test]
    fn variables_are_gathered_once_across_branches() {
        let mut backend = Fake::default();
        backend.variables.insert(
            "a ${who}".to_string(),
            vec![QueryVariable {
                name: "who".into(),
                default: Some("me".into()),
            }],
        );
        backend.variables.insert(
            "b ${who}".to_string(),
            vec![QueryVariable {
                name: "who".into(),
                default: Some("you".into()),
            }],
        );
        let query =
            crate::parse("```yaml\nor:\n  - query: a ${who}\n  - query: b ${who}\n```\n").unwrap();
        let (vars, warnings) = variables(&query, &backend);
        assert_eq!(vars.len(), 1, "prompted once, bound once");
        assert_eq!(vars[0].default, Some("me".into()), "first declaration wins");
        assert_eq!(
            warnings,
            vec![Warning::ConflictingDefault {
                name: "who".into(),
                kept: Some("me".into()),
                ignored: Some("you".into()),
            }]
        );
    }

    #[tokio::test]
    async fn progress_counts_the_branches_as_they_land() {
        let backend = Fake::answering(&[
            ("a", &[("a", &[])]),
            ("b", &[("b", &[])]),
            ("c", &[("c", &[])]),
        ]);
        let query =
            crate::parse("```yaml\nor:\n  - query: a\n  - query: b\n  - query: c\n```\n").unwrap();
        let types = types();
        let bindings = HashMap::new();
        let seen: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());
        let report = |done: usize, total: usize| seen.lock().unwrap().push((done, total));
        let mut run = Run::new(&backend, &types, &bindings);
        run.progress = Some(&report);
        super::execute(&query, &run).await.unwrap();
        assert_eq!(
            seen.lock().unwrap().clone(),
            vec![(1, 3), (2, 3), (3, 3)],
            "a frontend needs the fraction, not just the end"
        );
    }
}
