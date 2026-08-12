//! Running an extended query against a real [`ContentAdapter`].
//!
//! The executor deliberately talks to a three-method [`Backend`] instead of an
//! adapter, so the set algebra can be tested against a table of fixed answers.
//! This module is the one place that pays for that decision: it wires the
//! trait to the same `children::list` call the frontends already make, and
//! collects the column types from the same two sources a pane collects them
//! from. Everything a frontend needs to run a document lives here, so neither
//! the TUI nor the CLI has to know how a document becomes a result set.

use std::collections::HashMap;

use async_trait::async_trait;
use not_yet_done_content::{
    ContentAdapter, GroupSpec, ListParams, Node, NodeSummary, NodeType, PageRequest, QueryVariable,
    children,
};

use crate::ast::ExtendedQuery;
use crate::executor::{Backend, ExecError, Execution, Run};
use crate::parse::{ParseError, check_languages, parse};
use crate::rows::ColumnTypes;

/// One adapter, one node, one child type — the coordinates every branch of a
/// document is fetched under.
///
/// All branches share them: an extended query combines *queries*, not places.
/// A branch that could point somewhere else would turn `without:` into a
/// cross-view set difference, which is not what the format expresses.
pub struct AdapterBackend<'a> {
    adapter: &'a dyn ContentAdapter,
    node: &'a dyn Node,
    node_type: NodeType,
    /// The pane's active grouping, passed through for adapters that group
    /// adapter-side. Ignored by everyone else, exactly as in a normal list.
    group_by: Option<GroupSpec>,
}

impl<'a> AdapterBackend<'a> {
    pub fn new(adapter: &'a dyn ContentAdapter, node: &'a dyn Node, node_type: NodeType) -> Self {
        Self {
            adapter,
            node,
            node_type,
            group_by: None,
        }
    }

    pub fn with_group_by(mut self, group_by: Option<GroupSpec>) -> Self {
        self.group_by = group_by;
        self
    }

    /// The query language the document's fences are checked against.
    pub fn language(&self) -> &str {
        self.adapter.query_language()
    }

    /// The typed columns `local_filter` and `order_by` resolve against — the
    /// adapter's declaration unioned with whatever a decorator describes,
    /// which [`children::columns_for`] has already done.
    pub async fn column_types(&self) -> ColumnTypes {
        ColumnTypes::new(&children::columns_for(self.adapter, self.node, &self.node_type).await)
    }
}

#[async_trait]
impl Backend for AdapterBackend<'_> {
    fn query_variables(&self, query: &str) -> Vec<QueryVariable> {
        self.adapter.query_variables(query)
    }

    fn render_query(&self, query: &str, vars: &HashMap<String, String>) -> String {
        self.adapter.render_query(query, vars)
    }

    async fn fetch(&self, query: &str, limit: Option<usize>) -> Result<Vec<NodeSummary>, String> {
        let params = ListParams {
            node_type: self.node_type.clone(),
            query: Some(query.to_string()),
            // The document imposes the order after the merge, so a per-branch
            // sort would only cost the backend work whose result is discarded.
            sort: Vec::new(),
            // No limit means "everything this query selects" — the same thing
            // `PaginationMode::All` asks for, and the only page size under
            // which set algebra between branches is meaningful.
            page: limit.map(|l| PageRequest {
                offset: 0,
                limit: l.min(u32::MAX as usize) as u32,
            }),
            download: false,
            group_by: self.group_by.clone(),
        };
        children::list(self.adapter, self.node, params)
            .await
            .map(|result| result.items)
            .map_err(|e| e.to_string())
    }
}

/// The half of a backend that needs no node: which variables a query text
/// declares, and how they are substituted, are properties of the adapter's
/// query *language* rather than of the place being listed.
///
/// [`document_variables`] runs on that half alone, which is what lets a
/// frontend collect bindings before it has resolved a root node. `fetch` is
/// unreachable from there and says so instead of guessing a node.
struct QueryOnlyBackend<'a> {
    adapter: &'a dyn ContentAdapter,
}

#[async_trait]
impl Backend for QueryOnlyBackend<'_> {
    fn query_variables(&self, query: &str) -> Vec<QueryVariable> {
        self.adapter.query_variables(query)
    }

    fn render_query(&self, query: &str, vars: &HashMap<String, String>) -> String {
        self.adapter.render_query(query, vars)
    }

    async fn fetch(&self, _query: &str, _limit: Option<usize>) -> Result<Vec<NodeSummary>, String> {
        Err("this backend declares variables only; it cannot fetch".to_string())
    }
}

/// Every variable a document declares, gathered without running it.
///
/// The prompt for bindings comes *before* the run, at a point where a frontend
/// holds an adapter but not yet the node it will list under — so this path
/// deliberately avoids needing one. Warnings are the same
/// [`ConflictingDefault`](crate::Warning::ConflictingDefault) notes
/// [`crate::variables`] produces.
pub fn document_variables(
    document: &str,
    adapter: &dyn ContentAdapter,
) -> Result<(Vec<QueryVariable>, Vec<crate::executor::Warning>), ParseError> {
    let query = parse(document)?;
    check_languages(&query, adapter.query_language())?;
    Ok(crate::executor::variables(
        &query,
        &QueryOnlyBackend { adapter },
    ))
}

/// Everything that can stop a document short of a result.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error(transparent)]
    Exec(#[from] ExecError),
}

/// Parse a document and check it against the adapter's query language.
///
/// Separate from [`run`] because a frontend has to ask the user for variable
/// bindings *before* the run, and gathering them needs the parsed document:
/// prepare once, call [`crate::variables`], prompt, then execute. [`run`] is
/// for the path where no binding is missing.
pub fn prepare(document: &str, backend: &AdapterBackend<'_>) -> Result<ExtendedQuery, ParseError> {
    let query = parse(document)?;
    check_languages(&query, backend.language())?;
    Ok(query)
}

/// Parse, check and execute a document in one go.
pub async fn run(
    document: &str,
    backend: &AdapterBackend<'_>,
    types: &ColumnTypes,
    bindings: &HashMap<String, String>,
    progress: Option<&(dyn Fn(usize, usize) + Send + Sync)>,
) -> Result<Execution, RunError> {
    let query = prepare(document, backend)?;
    let mut run = Run::new(backend, types, bindings);
    run.progress = progress;
    Ok(crate::executor::execute(&query, &run).await?)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use not_yet_done_content::children::Child;
    use not_yet_done_content::{
        ColumnSchema, ContentError, ListResult, Metadata, MetadataField, Result, SortKind,
    };

    use super::*;
    use crate::executor::Warning;

    fn row_type() -> NodeType {
        NodeType {
            type_id: "row".into(),
            mime_type: "text/plain".into(),
            syntax: None,
            file_extension: ".txt".into(),
            display_name: "Row".into(),
        }
    }

    fn summary(id: &str, fields: &[(&str, &str)]) -> NodeSummary {
        // Every row carries `status`, blank unless the case sets it — the
        // fake adapter declares that column, and `children::list` holds it to
        // the promise just as it does a real one.
        let mut fields = fields.to_vec();
        if !fields.iter().any(|(k, _)| *k == "status") {
            fields.push(("status", ""));
        }
        NodeSummary {
            id: id.to_string(),
            label: format!("label of {id}"),
            node_type: row_type(),
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

    struct TestNode;

    impl Node for TestNode {
        fn id(&self) -> &str {
            "root"
        }
        fn label(&self) -> &str {
            "Root"
        }
        fn node_type(&self) -> &'static NodeType {
            static ROOT: std::sync::OnceLock<NodeType> = std::sync::OnceLock::new();
            ROOT.get_or_init(|| NodeType {
                type_id: "root".into(),
                ..row_type()
            })
        }
        fn metadata(&self) -> &Metadata {
            static EMPTY: std::sync::OnceLock<Metadata> = std::sync::OnceLock::new();
            EMPTY.get_or_init(Metadata::default)
        }
    }

    /// Answers a fixed table of queries and records what it was asked, so a
    /// test can see the `ListParams` the bridge built.
    struct RecordingAdapter {
        rows: Vec<(&'static str, Vec<NodeSummary>)>,
        seen: Arc<Mutex<Vec<ListParams>>>,
    }

    impl RecordingAdapter {
        fn new(rows: Vec<(&'static str, Vec<NodeSummary>)>) -> Self {
            Self {
                rows,
                seen: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait]
    impl ContentAdapter for RecordingAdapter {
        fn adapter_type(&self) -> &str {
            "recording"
        }

        fn query_body_suffix(&self) -> &str {
            ".jql"
        }

        fn query_variables(&self, query: &str) -> Vec<QueryVariable> {
            query
                .contains("{who}")
                .then(|| {
                    vec![QueryVariable {
                        name: "who".into(),
                        default: Some("me".into()),
                    }]
                })
                .unwrap_or_default()
        }

        fn render_query(&self, query: &str, vars: &HashMap<String, String>) -> String {
            match vars.get("who") {
                Some(who) => query.replace("{who}", who),
                None => query.to_string(),
            }
        }

        async fn describe_columns(&self, _node_type: &str) -> Vec<ColumnSchema> {
            vec![ColumnSchema {
                label: None,
                ..ColumnSchema::new("points", "").typed("number")
            }]
        }

        async fn root(&self) -> Result<Box<dyn Node>> {
            Ok(Box::new(TestNode))
        }

        async fn get_by_id(&self, id: &str) -> Result<Box<dyn Node>> {
            Err(ContentError::NotFound(id.to_string()))
        }

        fn childs<'a>(&'a self, _node: &'a dyn Node) -> Vec<Child<'a>> {
            let seen = Arc::clone(&self.seen);
            vec![Child {
                node_type: row_type(),
                columns: vec![ColumnSchema::new("status", "Status")],
                list: Box::new(move |params: ListParams| {
                    let query = params.query.clone().unwrap_or_default();
                    let page = params.page;
                    seen.lock().unwrap().push(params);
                    let items = self
                        .rows
                        .iter()
                        .find(|(q, _)| *q == query)
                        .map(|(_, rows)| rows.clone())
                        .unwrap_or_default();
                    let items = match page {
                        Some(p) => items.into_iter().take(p.limit as usize).collect(),
                        None => items,
                    };
                    Box::pin(async move {
                        Ok(ListResult {
                            items,
                            applied_sort: Vec::new(),
                            page: None,
                            batch_download_available: false,
                            downloaded: Vec::new(),
                        })
                    })
                }),
            }]
        }
    }

    #[tokio::test]
    async fn a_fetch_lists_the_pane_s_child_type_unsorted_and_unpaged() {
        let adapter = RecordingAdapter::new(vec![("open", vec![summary("A", &[])])]);
        let node = TestNode;
        let backend = AdapterBackend::new(&adapter, &node, row_type());

        let items = backend.fetch("open", None).await.unwrap();

        assert_eq!(items.len(), 1);
        let seen = adapter.seen.lock().unwrap();
        assert_eq!(seen[0].node_type.type_id, "row");
        assert_eq!(seen[0].query.as_deref(), Some("open"));
        // The document orders after the merge, so a per-branch sort would buy
        // nothing; no limit means every matching row.
        assert!(seen[0].sort.is_empty());
        assert!(seen[0].page.is_none());
        assert!(!seen[0].download);
    }

    #[tokio::test]
    async fn a_limit_becomes_the_first_page_of_that_size() {
        let adapter = RecordingAdapter::new(vec![("open", vec![])]);
        let node = TestNode;
        let backend = AdapterBackend::new(&adapter, &node, row_type());

        backend.fetch("open", Some(25)).await.unwrap();

        let page = adapter.seen.lock().unwrap()[0].page.unwrap();
        assert_eq!(page.offset, 0);
        assert_eq!(page.limit, 25);
    }

    #[tokio::test]
    async fn an_adapter_error_reaches_the_executor_as_text() {
        let adapter = RecordingAdapter::new(vec![]);
        let node = TestNode;
        // The pane's child type is the one thing `childs` does not offer.
        let backend = AdapterBackend::new(
            &adapter,
            &node,
            NodeType {
                type_id: "nonexistent".into(),
                ..row_type()
            },
        );

        let err = backend.fetch("open", None).await.unwrap_err();

        assert!(err.contains("nonexistent"), "{err}");
    }

    #[tokio::test]
    async fn column_types_union_what_the_adapter_sorts_and_what_it_describes() {
        let adapter = RecordingAdapter::new(vec![]);
        let node = TestNode;
        let backend = AdapterBackend::new(&adapter, &node, row_type());

        let types = backend.column_types().await;

        assert_eq!(types.kind("status"), Some(SortKind::Text));
        assert_eq!(types.kind("points"), Some(SortKind::Number));
    }

    #[test]
    fn the_language_comes_from_the_body_suffix_without_its_dot() {
        let adapter = RecordingAdapter::new(vec![]);
        let node = TestNode;

        assert_eq!(
            AdapterBackend::new(&adapter, &node, row_type()).language(),
            "jql"
        );
    }

    #[test]
    fn a_decorated_adapter_still_reports_the_inner_language() {
        // The decorators sit between the pane and the adapter, so a document's
        // fences are checked against whatever they report. An anonymised Jira
        // view that claimed to speak `yaml` would reject its own queries.
        let adapter = not_yet_done_content::anonymize::AnonymizingAdapter::new(Box::new(
            RecordingAdapter::new(vec![]),
        ));
        let node = TestNode;

        assert_eq!(
            AdapterBackend::new(&adapter, &node, row_type()).language(),
            "jql"
        );
    }

    #[test]
    fn a_fence_in_another_language_is_rejected_before_any_round_trip() {
        let adapter = RecordingAdapter::new(vec![]);
        let node = TestNode;
        let backend = AdapterBackend::new(&adapter, &node, row_type());

        let err = prepare(
            "```yaml\nand:\n  - query-ref: mine\n```\n\n```sql mine\nselect 1\n```\n",
            &backend,
        )
        .unwrap_err();

        assert!(matches!(err, ParseError::Language { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn a_document_runs_end_to_end_against_the_adapter() {
        let adapter = RecordingAdapter::new(vec![
            (
                "assignee = me",
                vec![
                    summary("A", &[("points", "5")]),
                    summary("B", &[("points", "10")]),
                ],
            ),
            ("status = done", vec![summary("A", &[("points", "5")])]),
        ]);
        let node = TestNode;
        let backend = AdapterBackend::new(&adapter, &node, row_type());
        let types = backend.column_types().await;

        let execution = run(
            "```yaml\nwithout:\n  - query: assignee = me\n  - query: status = done\n\
             order_by:\n  - points: desc\n```\n",
            &backend,
            &types,
            &HashMap::new(),
            None,
        )
        .await
        .unwrap();

        assert_eq!(
            execution.items.iter().map(|i| &i.id).collect::<Vec<_>>(),
            ["B"]
        );
        assert!(execution.warnings.is_empty(), "{:?}", execution.warnings);
    }

    #[tokio::test]
    async fn variables_are_gathered_and_rendered_through_the_adapter() {
        let adapter = RecordingAdapter::new(vec![("assignee = vega", vec![summary("A", &[])])]);
        let node = TestNode;
        let backend = AdapterBackend::new(&adapter, &node, row_type());
        let types = backend.column_types().await;
        let document = "```yaml\nquery: assignee = {who}\n```\n";

        let query = prepare(document, &backend).unwrap();
        let (vars, warnings) = crate::variables(&query, &backend);
        assert_eq!(vars.len(), 1);
        assert_eq!(vars[0].name, "who");
        assert!(warnings.is_empty());

        let bindings = HashMap::from([("who".to_string(), "vega".to_string())]);
        let execution = run(document, &backend, &types, &bindings, None)
            .await
            .unwrap();

        assert_eq!(execution.items.len(), 1);
        assert!(!execution.warnings.contains(&Warning::NativeOrderIgnored));
    }

    #[test]
    fn variables_can_be_gathered_with_no_node_in_hand() {
        // The frontend prompts for bindings before it has resolved a root
        // node, so this path takes the adapter alone — and still sees every
        // branch's variables, not just the first one's.
        let adapter = RecordingAdapter::new(vec![]);
        let document =
            "```yaml\nor:\n  - query: assignee = {who}\n  - query: reporter = {who}\n```\n";

        let (vars, warnings) = document_variables(document, &adapter).unwrap();

        let names: Vec<&str> = vars.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(
            names,
            ["who"],
            "the same name across branches is one prompt"
        );
        assert!(warnings.is_empty());
        assert!(
            adapter.seen.lock().unwrap().is_empty(),
            "declaring variables must not list anything"
        );
    }

    #[test]
    fn a_document_in_the_wrong_language_is_rejected_while_gathering_variables() {
        // Same guard as the run path: a document written against another
        // adapter must fail where the user can still fix it, not halfway
        // through a load.
        let adapter = RecordingAdapter::new(vec![]);
        let document = "```yaml\nquery-ref: mine\n```\n\n```cql mine\nassignee = {who}\n```\n";

        let err = document_variables(document, &adapter).unwrap_err();

        assert!(
            err.to_string().contains("cql"),
            "error should name the offending language: {err}"
        );
    }
}
