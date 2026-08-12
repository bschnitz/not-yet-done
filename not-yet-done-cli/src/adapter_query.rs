//! Stored queries on the CLI: listing them, resolving `--query-name` to a
//! body, binding its variables from `--var`, and running an extended document
//! through the executor.
//!
//! Two stores hold queries for an adapter instance — adapter-native bodies in
//! [`SavedQueryStore`](not_yet_done_content::SavedQueryStore) and Markdown
//! documents in
//! [`ExtendedQueryStore`](not_yet_done_content::ExtendedQueryStore) — and a
//! name is unique across both. The TUI reads the kind off the merged menu
//! list the user picked the entry from; the CLI has no menu, so it asks the
//! stores. Either way the *store* decides what a body is: nothing in the text
//! says which one it came from, and a `yaml` adapter's own query would be
//! indistinguishable from a document's spec fence.
//!
//! Everything here is about a query's *identity and bindings*. How a document
//! becomes rows is `not_yet_done_extended_query`'s job, and how rows are
//! printed is `adapter_cli`'s.

use std::collections::HashMap;

use anyhow::{Result, anyhow};

use not_yet_done_content::{
    ContentAdapter, GroupSpec, ListResult, Node, NodeType, QueryKind, QueryVariable, apply_sort,
    children,
};

/// A query body together with the store it came from.
pub struct StoredQuery {
    pub text: String,
    pub kind: QueryKind,
}

/// Every stored query of this adapter instance, both kinds, sorted by name.
///
/// The kind is printed alongside because the CLI is where a query gets
/// scripted and debugged: it says which language the body is in and which
/// directory holds the file. The TUI's menu still shows no difference — there
/// the two are interchangeable by design.
pub async fn list(adapter: &dyn ContentAdapter) -> Result<Vec<(String, QueryKind)>> {
    let mut out: Vec<(String, QueryKind)> = Vec::new();
    if let Some(store) = adapter.saved_query_store() {
        for name in store.list().await? {
            out.push((name, QueryKind::Saved));
        }
    }
    if let Some(store) = adapter.extended_query_store() {
        for name in store.list().await? {
            out.push((name, QueryKind::Extended));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Load the query named `name` from whichever store holds it.
///
/// A store that cannot be listed fails the whole lookup rather than reading as
/// "not there" — the same rule
/// [`existing_query_kind`](not_yet_done_content::existing_query_kind) applies
/// when a name is being created.
pub async fn load(adapter: &dyn ContentAdapter, name: &str) -> Result<StoredQuery> {
    let kind = not_yet_done_content::existing_query_kind(adapter, name)
        .await?
        .ok_or_else(|| {
            anyhow!("no stored query named '{name}' — `queries` lists the names of this level")
        })?;
    let text = match kind {
        QueryKind::Saved => {
            adapter
                .saved_query_store()
                .ok_or_else(|| anyhow!("this adapter has no saved-query store"))?
                .load(name)
                .await?
        }
        QueryKind::Extended => {
            adapter
                .extended_query_store()
                .ok_or_else(|| anyhow!("this adapter has no extended-query store"))?
                .load(name)
                .await?
        }
    };
    Ok(StoredQuery { text, kind })
}

/// Turn the repeated `--var k=v` pairs into the binding map both kinds render
/// with.
pub fn bindings(pairs: &[(String, String)]) -> HashMap<String, String> {
    pairs.iter().cloned().collect()
}

/// Refuse a run whose query declares variables the command line did not bind.
///
/// A variable that carries its own default is fine — rendering falls back to
/// it, exactly as it does in the TUI when the user confirms the prompt
/// unchanged. One without a default has no answer here: there is no prompt on
/// a CLI, and sending the placeholder to the adapter verbatim would run a
/// different query than the one that was asked for.
fn check_bound(vars: &[QueryVariable], given: &HashMap<String, String>) -> Result<()> {
    let missing: Vec<&str> = vars
        .iter()
        .filter(|v| v.default.is_none() && !given.contains_key(&v.name))
        .map(|v| v.name.as_str())
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(anyhow!(
        "query variable{} without a value: {} — bind {} with --var name=value",
        if missing.len() == 1 { "" } else { "s" },
        missing.join(", "),
        if missing.len() == 1 { "it" } else { "them" },
    ))
}

/// The adapter-native query text to hand to `list()`: variables substituted,
/// unbound ones rejected.
///
/// Applies to a body typed at `--query` as much as to a stored one. A typed
/// body used to go to the adapter verbatim, which sent any `{{variable}}` in
/// it downstream as literal text.
pub fn render_native(
    adapter: &dyn ContentAdapter,
    body: &str,
    bindings: &HashMap<String, String>,
) -> Result<String> {
    check_bound(&adapter.query_variables(body), bindings)?;
    Ok(adapter.render_query(body, bindings))
}

/// Run an extended document under `parent` and return its rows.
///
/// Mirrors the TUI's path: the document's own `order_by` stands unless
/// `--sort` was given, in which case the explicit sort wins — the same
/// precedence a pane's `s` has over a document. Warnings (truncation, a lost
/// native order) go to stderr: they are notes on a *successful* run, and
/// stdout carries the result a script reads.
pub async fn run_extended(
    adapter: &dyn ContentAdapter,
    parent: &dyn Node,
    node_type: NodeType,
    document: &str,
    bindings: &HashMap<String, String>,
    sort: &[not_yet_done_content::SortKey],
    group_by: Option<GroupSpec>,
) -> Result<ListResult> {
    let (vars, warnings) = not_yet_done_extended_query::document_variables(document, adapter)?;
    check_bound(&vars, bindings)?;
    for warning in &warnings {
        eprintln!("nyd: warning: {warning}");
    }

    let backend =
        not_yet_done_extended_query::AdapterBackend::new(adapter, parent, node_type.clone())
            .with_group_by(group_by);
    let types = backend.column_types().await;
    let execution =
        not_yet_done_extended_query::run(document, &backend, &types, bindings, None).await?;
    for warning in &execution.warnings {
        eprintln!("nyd: warning: {warning}");
    }

    let mut items = execution.items;
    let applied_sort = if sort.is_empty() {
        execution.applied_sort
    } else {
        let columns = children::columns_for(adapter, parent, &node_type).await;
        apply_sort(&mut items, sort, &columns)
    };
    Ok(ListResult {
        items,
        applied_sort,
        // The merge is complete by construction — there is no server page left
        // to continue from.
        page: None,
        batch_download_available: false,
        downloaded: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(name: &str, default: Option<&str>) -> QueryVariable {
        QueryVariable {
            name: name.to_string(),
            default: default.map(str::to_string),
        }
    }

    #[test]
    fn a_variable_with_a_default_needs_no_binding() {
        assert!(check_bound(&[var("who", Some("me"))], &HashMap::new()).is_ok());
    }

    #[test]
    fn an_unbound_variable_without_a_default_is_refused_by_name() {
        let err = check_bound(&[var("who", None), var("when", None)], &HashMap::new())
            .unwrap_err()
            .to_string();
        assert!(err.contains("who, when"), "{err}");
        assert!(err.contains("--var name=value"), "{err}");
    }

    #[test]
    fn binding_it_on_the_command_line_satisfies_the_check() {
        let given = bindings(&[("who".to_string(), "someone".to_string())]);
        assert!(check_bound(&[var("who", None)], &given).is_ok());
    }
}
