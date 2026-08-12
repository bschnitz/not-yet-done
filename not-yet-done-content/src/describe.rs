//! Generic, adapter-agnostic documentation of the content tree — the backing
//! for the built-in `help` action.
//!
//! `help` is a *framework-synthesised* action, **not** an adapter-defined one:
//! everything it documents is read straight off the public protocol
//! ([`Node::node_type`], [`Node::actions`], [`Node::children_types`],
//! [`children::Child::columns`](crate::children::Child::columns),
//! [`ContentAdapter::actions_for_type`],
//! [`ContentAdapter::capabilities`]). It therefore exists on **every** node of
//! **every** adapter at zero adapter cost and stays fully decoupled from any
//! frontend — a frontend only asks the content layer to
//!
//! 1. include the synthetic action in a level's action set
//!    ([`level_actions`] / [`level_actions_for_type`]), and
//! 2. render the current level's documentation ([`render_level`]) when the
//!    action fires ([`is_builtin`] / [`run_builtin`]).
//!
//! "Current level" = the node the user is on: its type, the actions available
//! here, the child types reachable from here (by name — descend and run `help`
//! again for their level), and any sortable columns. The adapter's
//! capabilities are documented only at the tree root, since they describe the
//! adapter as a whole rather than one level.

use std::fmt::Write as _;

use crate::{
    ActionOutcome, AdapterCapabilities, ColumnSchema, ContentAdapter, InputSpec, Metadata, Node,
    NodeAction, NodeType,
};

/// A structural stand-in for a node of a given [`NodeType`], carrying no
/// instance data (empty id, no metadata). It lets the framework introspect a
/// *level* — its child types, their sort columns, the type-level action set —
/// purely from the type tree, without fetching a concrete node.
///
/// This is what type-addressed `help` stands on: [`ContentAdapter::childs`]
/// derives a node's child *types* and *sort columns* from the node's
/// `node_type()` alone (the concrete id only ever flows into the lazy `list`
/// closures, which `help` never calls). Handing `childs` a `TypeNode` therefore
/// yields the correct structural description with no I/O and no real instance —
/// so a frontend can document `jira:issue:comment` without naming a comment.
pub struct TypeNode {
    node_type: NodeType,
    metadata: Metadata,
}

impl TypeNode {
    /// Build a prototype node for `node_type`.
    pub fn new(node_type: NodeType) -> Self {
        Self {
            node_type,
            metadata: Metadata::default(),
        }
    }
}

#[async_trait::async_trait]
impl Node for TypeNode {
    fn id(&self) -> &str {
        ""
    }
    fn label(&self) -> &str {
        self.node_type.display_name.as_str()
    }
    fn node_type(&self) -> &NodeType {
        &self.node_type
    }
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}

/// The child *types* reachable from a level, keyed by type alone (no instance).
///
/// Walks [`ContentAdapter::childs`] on a [`TypeNode`] and keeps only the
/// declared child [`NodeType`]s — the instance-free half of the "single source
/// of truth about children". Frontends use this to resolve a type path
/// (`jira → issue → comment`) down the tree without fetching a single node.
pub fn child_types_of_type(adapter: &dyn ContentAdapter, nt: &NodeType) -> Vec<NodeType> {
    let proto = TypeNode::new(nt.clone());
    adapter
        .childs(&proto)
        .into_iter()
        .map(|c| c.node_type)
        .collect()
}

/// Stable id of the built-in `help` action.
pub const HELP_ACTION_ID: &str = "help";

/// The synthetic `help` action every level carries. Menu-only
/// ([`InputSpec::None`]), no default key — frontends expose it however they
/// like; the CLI reaches it via `do help`.
pub fn help_action() -> NodeAction {
    NodeAction::new(
        HELP_ACTION_ID,
        "help (document this level)",
        InputSpec::None,
    )
}

/// Whether `id` names a framework built-in action (handled by the content
/// layer, not routed to the adapter). Currently just [`HELP_ACTION_ID`].
pub fn is_builtin(id: &str) -> bool {
    id == HELP_ACTION_ID
}

/// Run a built-in action against the current level, returning its outcome.
/// `None` when `id` is not a built-in — the caller then routes to the adapter
/// as usual. `is_root` toggles the adapter-wide capabilities section (see the
/// module docs).
pub async fn run_builtin(
    id: &str,
    adapter: &dyn ContentAdapter,
    node: &dyn Node,
    is_root: bool,
) -> Option<ActionOutcome> {
    if id == HELP_ACTION_ID {
        Some(ActionOutcome::Done {
            message: Some(render_level(adapter, node, is_root).await),
        })
    } else {
        None
    }
}

/// The set of actions available on `node`'s level — the adapter's type-level
/// action set plus the synthetic built-ins. The single seam a frontend uses to
/// present "what can I do here"; keeps `help` visible everywhere without any
/// adapter declaring it.
///
/// The action *set* is always type-derivable: it is resolved purely from
/// [`ContentAdapter::actions_for_type`] keyed by `node.node_type()`. There is
/// no per-instance action source (the `Node` trait has none), so the
/// instance-bearing and instance-free paths ([`level_actions_for_type`]) can
/// never diverge.
pub fn level_actions(adapter: &dyn ContentAdapter, node: &dyn Node) -> Vec<NodeAction> {
    level_actions_for_type(adapter, node.node_type())
}

/// Type-keyed action set: the adapter's [`ContentAdapter::actions_for_type`]
/// plus the synthetic built-ins. This is the sole action-resolution path,
/// shared by the instance-free surfaces (the CLI's `actions --type`, the TUI's
/// shortcut-hint resolution) and [`level_actions`] alike.
pub fn level_actions_for_type(adapter: &dyn ContentAdapter, nt: &NodeType) -> Vec<NodeAction> {
    with_builtins(adapter.actions_for_type(nt))
}

fn with_builtins(mut actions: Vec<NodeAction>) -> Vec<NodeAction> {
    if !actions.iter().any(|a| a.id == HELP_ACTION_ID) {
        actions.push(help_action());
    }
    actions
}

/// Render the documentation of `node`'s level as Markdown. Introspection over
/// the public protocol only — no adapter downcast. `is_root` adds the
/// adapter-wide capabilities section.
///
/// Async because the column section goes through
/// [`children::columns_for`](crate::children::columns_for), and the described
/// channel a decorator fills is an async read.
pub async fn render_level(adapter: &dyn ContentAdapter, node: &dyn Node, is_root: bool) -> String {
    let nt = node.node_type();
    let mut out = render_header(adapter, nt);

    // --- This node -------------------------------------------------------
    // Instance-specific: the concrete node's id and current label.
    let _ = writeln!(out, "## This level");
    let _ = writeln!(out, "- **id:** `{}`", node.id());
    let _ = writeln!(out, "- **label:** {}", node.label());
    render_type_facts(&mut out, nt);
    let _ = writeln!(out);

    render_capabilities(&mut out, adapter, is_root);
    render_body(&mut out, adapter, node, level_actions(adapter, node)).await;
    out
}

/// Render the documentation of a *type's* level as Markdown, without any
/// instance — the type path (`jira → issue → comment`) alone selects the level.
///
/// Same shape as [`render_level`] minus the instance-specific `id`/`label`
/// lines: actions come from the type-level set ([`level_actions_for_type`]) and
/// the child types / sort columns from [`ContentAdapter::childs`] on a
/// [`TypeNode`]. This is the id-free path a frontend uses for `help` when the
/// user names a level by type rather than by a concrete node.
pub async fn render_level_for_type(
    adapter: &dyn ContentAdapter,
    nt: &NodeType,
    is_root: bool,
) -> String {
    let proto = TypeNode::new(nt.clone());
    let mut out = render_header(adapter, nt);

    let _ = writeln!(out, "## This level");
    render_type_facts(&mut out, nt);
    let _ = writeln!(out);

    render_capabilities(&mut out, adapter, is_root);
    render_body(
        &mut out,
        adapter,
        &proto,
        level_actions_for_type(adapter, nt),
    )
    .await;
    out
}

/// `# help — …` title + the adapter/instance line shared by both render paths.
fn render_header(adapter: &dyn ContentAdapter, nt: &NodeType) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# help — {} (`{}`)", nt.display_name, nt.type_id);
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Adapter `{}` (instance `{}`)",
        adapter.adapter_type(),
        adapter.instance_id()
    );
    let _ = writeln!(out);
    out
}

/// The type-derived facts of a level (type, syntax, content type) — the part of
/// "## This level" that holds whether or not a concrete node is in hand.
fn render_type_facts(out: &mut String, nt: &NodeType) {
    let _ = writeln!(out, "- **type:** {} (`{}`)", nt.display_name, nt.type_id);
    if let Some(syntax) = &nt.syntax {
        let _ = writeln!(out, "- **syntax:** {syntax}");
    }
    if !nt.mime_type.is_empty() {
        let _ = writeln!(out, "- **content type:** {}", nt.mime_type);
    }
}

/// The adapter-wide capabilities block — rendered only at the tree root, since
/// it documents the adapter as a whole rather than one level.
fn render_capabilities(out: &mut String, adapter: &dyn ContentAdapter, is_root: bool) {
    if !is_root {
        return;
    }
    let _ = writeln!(out, "## Adapter capabilities");
    let caps = capability_names(&adapter.capabilities());
    if caps.is_empty() {
        let _ = writeln!(out, "- _read-only (no create / delete / search)_");
    } else {
        for cap in caps {
            let _ = writeln!(out, "- {cap}");
        }
    }
    let _ = writeln!(out);
}

/// The level-agnostic tail shared by both render paths: the actions here, the
/// child types to descend into, and their sort columns. `actions` is passed in
/// so the caller decides between the instance set ([`level_actions`]) and the
/// type-only set ([`level_actions_for_type`]).
async fn render_body(
    out: &mut String,
    adapter: &dyn ContentAdapter,
    node: &dyn Node,
    actions: Vec<NodeAction>,
) {
    // --- Actions here ----------------------------------------------------
    let _ = writeln!(out, "## Actions here");
    for a in &actions {
        let suffix = format!("input: {}", input_kind(&a.input));
        let _ = writeln!(out, "- `{}` — {} _({suffix})_", a.id, a.label);
    }
    let _ = writeln!(out);

    // The child types reachable from here. Snapshot them before the awaits
    // below: a `Child` borrows `node` and carries a non-`Sync` list closure.
    let kid_types: Vec<NodeType> = adapter
        .childs(node)
        .into_iter()
        .map(|c| c.node_type)
        .collect();

    // --- Descend into ----------------------------------------------------
    let _ = writeln!(out, "## Descend into");
    if kid_types.is_empty() {
        let _ = writeln!(out, "- _leaf level — nothing to descend into_");
    } else {
        for nt in &kid_types {
            let _ = writeln!(
                out,
                "- **{}** (`{}`) — descend and run `help` for its level",
                nt.display_name, nt.type_id
            );
        }
    }

    // --- Sort columns ----------------------------------------------------
    // Grouped by the child type whose lists they sort (a level's rows are its
    // children, so sort options belong to a child type, not the node's own).
    //
    // Through `columns_for`, not `Child::columns`: a column a decorator
    // describes — a user's custom column, say — sorts like any other, and help
    // that lists only the declared half of the declaration reports a column the
    // user can plainly use as one that does not exist.
    let mut sortable_by_kid: Vec<(&NodeType, Vec<ColumnSchema>)> = Vec::new();
    for nt in &kid_types {
        let sortable: Vec<ColumnSchema> = crate::children::columns_for(adapter, node, nt)
            .await
            .into_iter()
            .filter(|col| col.sortable)
            .collect();
        if !sortable.is_empty() {
            sortable_by_kid.push((nt, sortable));
        }
    }
    if !sortable_by_kid.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "## Sort columns");
        for (nt, columns) in sortable_by_kid {
            let _ = writeln!(out, "- **{}** (`{}`)", nt.display_name, nt.type_id);
            for col in columns {
                let _ = writeln!(
                    out,
                    "  - `{}` — {} ({})",
                    col.key,
                    col.display_label(),
                    col.value_type
                );
            }
        }
    }
}

/// Human-readable, user-facing capability names for the ones a person cares
/// about at a glance. The engine-internal flags (tree aggregation, query
/// propagation, adapter-side grouping, eager subtree) are deliberately omitted
/// — they document *how* the adapter integrates, not *what* the user can do.
fn capability_names(caps: &AdapterCapabilities) -> Vec<&'static str> {
    let mut out = Vec::new();
    if caps.supports_create {
        out.push("create");
    }
    if caps.supports_delete {
        out.push("delete");
    }
    if caps.supports_search {
        out.push("search");
    }
    if caps.supports_batch_download {
        out.push("batch download");
    }
    if caps.supports_total_count {
        out.push("total counts");
    }
    out
}

fn input_kind(input: &InputSpec) -> &'static str {
    match input {
        InputSpec::None => "none",
        InputSpec::Editor => "editor",
        InputSpec::Picker => "picker",
        InputSpec::FilePicker { .. } => "file(s)",
        InputSpec::Form { .. } => "form",
        InputSpec::ColumnForm => "form",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{MockAdapterBuilder, MockNodeData, issue_type};

    fn adapter() -> crate::mock::MockAdapter {
        MockAdapterBuilder::new("mock")
            .instance_id("mock-1")
            .capabilities(AdapterCapabilities {
                supports_create: true,
                supports_search: true,
                ..Default::default()
            })
            .actions_for(
                "mock:root",
                vec![NodeAction::new("add", "add", InputSpec::Editor)],
            )
            .actions_for(
                "mock:issue",
                vec![NodeAction::new("edit", "edit", InputSpec::Editor)],
            )
            .node(
                MockNodeData::new("root", "Root")
                    .child_type(issue_type())
                    .child(MockNodeData::new("ISS-1", "First").node_type(issue_type())),
            )
            .build()
    }

    #[tokio::test]
    async fn level_actions_injects_help_everywhere() {
        let a = adapter();
        let root = a.root().await.unwrap();
        let ids: Vec<String> = level_actions(&a, root.as_ref())
            .into_iter()
            .map(|x| x.id)
            .collect();
        assert!(ids.contains(&"add".to_string()));
        assert!(ids.contains(&HELP_ACTION_ID.to_string()));

        let leaf = a.get_by_id("ISS-1").await.unwrap();
        let leaf_ids: Vec<String> = level_actions(&a, leaf.as_ref())
            .into_iter()
            .map(|x| x.id)
            .collect();
        assert!(leaf_ids.contains(&"edit".to_string()));
        assert!(leaf_ids.contains(&HELP_ACTION_ID.to_string()));
    }

    #[tokio::test]
    async fn level_actions_does_not_duplicate_a_declared_help() {
        // An adapter that already declares `help` must not get a second one.
        let a = MockAdapterBuilder::new("mock")
            .actions_for(
                "mock:root",
                vec![NodeAction::new(
                    HELP_ACTION_ID,
                    "custom help",
                    InputSpec::None,
                )],
            )
            .node(MockNodeData::new("root", "Root"))
            .build();
        let root = a.root().await.unwrap();
        let help_count = level_actions(&a, root.as_ref())
            .into_iter()
            .filter(|x| x.id == HELP_ACTION_ID)
            .count();
        assert_eq!(help_count, 1);
    }

    #[tokio::test]
    async fn root_help_documents_capabilities_and_child_types() {
        let a = adapter();
        let root = a.root().await.unwrap();
        let md = render_level(&a, root.as_ref(), true).await;
        assert!(md.contains("Adapter `mock` (instance `mock-1`)"));
        assert!(md.contains("## Adapter capabilities"));
        assert!(md.contains("- create"));
        assert!(md.contains("- search"));
        // Actions of this level, including the synthetic help.
        assert!(md.contains("`add`"));
        assert!(md.contains("`help`"));
        // Child types are listed by name as navigation.
        assert!(md.contains("**Issue** (`mock:issue`)"));
    }

    #[tokio::test]
    async fn non_root_help_omits_capabilities_and_marks_leaf() {
        let a = adapter();
        let leaf = a.get_by_id("ISS-1").await.unwrap();
        let md = render_level(&a, leaf.as_ref(), false).await;
        assert!(!md.contains("## Adapter capabilities"));
        assert!(md.contains("`edit`"));
        assert!(md.contains("leaf level"));
    }

    #[test]
    fn is_builtin_flags_only_help() {
        assert!(is_builtin(HELP_ACTION_ID));
        assert!(!is_builtin("edit"));
    }
}
