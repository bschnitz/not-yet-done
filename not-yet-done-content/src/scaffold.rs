//! Generic, adapter-agnostic *config scaffolding* — the backing for the CLI's
//! `config generate` verb.
//!
//! This is the sibling of [`crate::describe`]: where `describe` projects the
//! public protocol into human documentation (Markdown), `scaffold` projects the
//! same introspection into a **TUI view-config YAML skeleton**. Both walk the
//! type tree level-by-level via [`ContentAdapter::childs`] on a
//! [`TypeNode`](crate::describe::TypeNode) — no instance data, no I/O beyond the
//! adapter's own (connection-free) type description — so a config can be
//! generated for any adapter the host can construct, without a live connection.
//!
//! What the protocol *can* supply drives what we emit:
//!
//! * **Views / child-views** — the child *types* reachable at each level
//!   ([`child_types_of_type`](crate::describe::child_types_of_type)). The caller
//!   chooses which to include via a [`Selection`]; everything selected becomes a
//!   `views:` entry (top level) or a nested `children:` entry.
//! * **Actions** — *every* type-level action
//!   ([`level_actions_for_type`](crate::describe::level_actions_for_type), minus
//!   the framework built-ins) is emitted **commented out** with a `# TODO key`
//!   (adapters no longer suggest keys — binding is the view config's job), so
//!   nothing silently ships unbound.
//! * **Columns** — a best-effort seed: a `label` column, the child type's
//!   declared columns, and any typed [`describe_columns`](ContentAdapter::describe_columns).
//!   The protocol deliberately does *not* expose a row's full column set (column
//!   layout is a front-end concern), so each `columns:` block ends with a
//!   commented hint on how to add the rest by hand.
//!
//! The result is a *scaffold*: valid, immediately loadable, but meant to be
//! pruned and hand-tuned. It is rendered directly to YAML text (rather than
//! serialising the front-end's config structs, which live in the TUI crate) so
//! this stays a pure protocol projection with no dependency on any front-end;
//! a round-trip test in the TUI crate pins the field-name contract.

use std::collections::HashSet;
use std::fmt::Write as _;

use crate::describe::{TypeNode, is_builtin, level_actions_for_type};
use crate::{ColumnSchema, ContentAdapter, InputSpec, NodeType};

/// Header facts for the generated file that the protocol can't supply — the
/// `tab:` display info and the `adapter:` block. The CLI fills this from the
/// discovered instance (regenerating an existing config) or from `--type` /
/// `--config` flags (bootstrapping a brand-new adapter).
#[derive(Debug, Clone)]
pub struct FileMeta {
    /// `tab.name` — the tab's display name.
    pub tab_name: String,
    /// `tab.order` — sort order in the tab bar.
    pub order: i32,
    /// `adapter.type` — the adapter type id (factory key).
    pub adapter_type: String,
    /// `adapter.id` — explicit instance id, omitted when it equals the type.
    pub adapter_id: Option<String>,
    /// `adapter.config` — path to the adapter's config file.
    pub config: Option<String>,
    /// `adapter.config_inline` — inline adapter config string.
    pub config_inline: Option<String>,
    /// `adapter.manual_connect`.
    pub manual_connect: bool,
}

/// Which child types to include, per level. The caller resolves this however it
/// likes (an interactive walk, an `--all` flag, a fixed list); `scaffold` only
/// reads it.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// The set of type ids to include. `None` = include every reachable type.
    include: Option<HashSet<String>>,
    /// Maximum descent depth (`0` = top-level views only). `None` = unbounded
    /// (still cycle-guarded: a type reappearing on its own ancestor path is
    /// emitted as `recursive: true` rather than descended into).
    max_depth: Option<usize>,
}

impl Selection {
    /// Include every reachable child type, to unlimited depth.
    pub fn all() -> Self {
        Selection {
            include: None,
            max_depth: None,
        }
    }

    /// Include only the given type ids (any depth unless [`Self::with_max_depth`]
    /// is also set).
    pub fn with_types(types: HashSet<String>) -> Self {
        Selection {
            include: Some(types),
            max_depth: None,
        }
    }

    /// Cap the descent depth (`0` = top-level views only).
    pub fn with_max_depth(mut self, depth: usize) -> Self {
        self.max_depth = Some(depth);
        self
    }

    fn includes(&self, type_id: &str) -> bool {
        self.include
            .as_ref()
            .map(|s| s.contains(type_id))
            .unwrap_or(true)
    }

    fn depth_ok(&self, depth: usize) -> bool {
        self.max_depth.map(|m| depth <= m).unwrap_or(true)
    }
}

// ---------------------------------------------------------------------------
// In-memory model (rendered to YAML below)
// ---------------------------------------------------------------------------

/// One column line in a `columns:` block.
struct GenColumn {
    key: String,
    label: Option<String>,
    /// Mapped `kind:` (`number`/`datetime`/…), or `None` for the text default.
    kind: Option<&'static str>,
    /// `source:` override (only the label column carries `source: label`).
    source: Option<&'static str>,
    sizing: &'static str,
}

/// One action line in an `actions:` block.
struct GenAction {
    name: String,
    /// Suggested key; `None` → the line is emitted commented out (`# TODO key`).
    key: Option<char>,
    action_type: &'static str,
    id: String,
}

/// One `views:` / `children:` entry.
struct GenLevel {
    /// Local (unqualified) type name — the entry's `name:`.
    name: String,
    type_id: String,
    /// Subtab key (top-level views only); allocated by the renderer's caller.
    key: Option<char>,
    /// `default: true` (first top-level view only).
    default: bool,
    /// A type that reappears on its own ancestor path: emitted minimally with
    /// `recursive: true` and no columns/actions/children (all inherited).
    recursive: bool,
    columns: Vec<GenColumn>,
    actions: Vec<GenAction>,
    children: Vec<GenLevel>,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Generate a view-config YAML scaffold for `adapter`, rooted at its tree root,
/// including the child types picked by `selection`. Pure protocol projection —
/// the only I/O is the adapter's own (connection-free) type description.
pub async fn generate(
    adapter: &dyn ContentAdapter,
    meta: &FileMeta,
    selection: &Selection,
) -> crate::Result<String> {
    let root = adapter.root().await?;
    let root_nt = root.node_type().clone();
    let mut ancestors: Vec<String> = vec![root_nt.type_id.clone()];
    let mut views = build_levels(adapter, &root_nt, &mut ancestors, selection, 0).await?;
    assign_view_keys(&mut views);
    if let Some(first) = views.first_mut() {
        first.default = true;
    }
    Ok(render(meta, &views))
}

/// Build the selected child levels of `parent_nt`. `depth` is 0 for the direct
/// children of the root (which become `views:`), 1+ for deeper (`children:`).
async fn build_levels(
    adapter: &dyn ContentAdapter,
    parent_nt: &NodeType,
    ancestors: &mut Vec<String>,
    selection: &Selection,
    depth: usize,
) -> crate::Result<Vec<GenLevel>> {
    if !selection.depth_ok(depth) {
        return Ok(Vec::new());
    }
    // Snapshot the children (type + columns) before any await: `Child`
    // borrows the prototype node, and we don't need its lazy `list` closure.
    let pairs: Vec<(NodeType, Vec<ColumnSchema>)> = {
        let proto = TypeNode::new(parent_nt.clone());
        adapter
            .childs(&proto)
            .into_iter()
            .map(|c| (c.node_type, c.columns))
            .collect()
    };

    let mut out = Vec::new();
    for (nt, declared) in pairs {
        if !selection.includes(&nt.type_id) {
            continue;
        }
        // Cycle guard: a type already on the ancestor path (e.g. task:item under
        // task:item) becomes a recursive child, not an infinite descent.
        if ancestors.iter().any(|a| a == &nt.type_id) {
            out.push(GenLevel {
                name: type_local_name(&nt.type_id).to_string(),
                type_id: nt.type_id.clone(),
                key: None,
                default: false,
                recursive: true,
                columns: Vec::new(),
                actions: Vec::new(),
                children: Vec::new(),
            });
            continue;
        }

        let columns = build_columns(adapter, &nt, &declared).await;
        let actions = build_actions(adapter, &nt);

        ancestors.push(nt.type_id.clone());
        let children =
            Box::pin(build_levels(adapter, &nt, ancestors, selection, depth + 1)).await?;
        ancestors.pop();

        out.push(GenLevel {
            name: type_local_name(&nt.type_id).to_string(),
            type_id: nt.type_id.clone(),
            key: None,
            default: false,
            recursive: false,
            columns,
            actions,
            children,
        });
    }
    Ok(out)
}

/// The best-effort column seed for a type: a `label` column, the adapter's
/// declared columns, and any dynamically described ones — deduped by key.
async fn build_columns(
    adapter: &dyn ContentAdapter,
    nt: &NodeType,
    declared: &[ColumnSchema],
) -> Vec<GenColumn> {
    let mut cols = vec![GenColumn {
        key: "label".to_string(),
        label: Some(nt.display_name.clone()),
        kind: None,
        source: Some("label"),
        sizing: "flex(1)",
    }];
    for sc in declared {
        if cols.iter().any(|c| c.key == sc.key) {
            continue;
        }
        cols.push(GenColumn {
            key: sc.key.clone(),
            label: Some(sc.display_label().to_string()),
            kind: value_type_to_col_kind(&sc.value_type),
            source: None,
            sizing: "max",
        });
    }
    for cs in adapter.describe_columns(&nt.type_id).await {
        if cols.iter().any(|c| c.key == cs.key) {
            continue;
        }
        cols.push(GenColumn {
            key: cs.key.clone(),
            label: cs.label.clone(),
            kind: value_type_to_col_kind(&cs.value_type),
            source: None,
            sizing: "max",
        });
    }
    cols
}

/// Every type-level action (built-ins excluded), mapped to a config action.
fn build_actions(adapter: &dyn ContentAdapter, nt: &NodeType) -> Vec<GenAction> {
    level_actions_for_type(adapter, nt)
        .into_iter()
        .filter(|a| !is_builtin(&a.id))
        .map(|a| GenAction {
            name: if a.label.is_empty() {
                a.id.clone()
            } else {
                a.label.clone()
            },
            // Adapters no longer suggest keys; the scaffold emits every action
            // keyless (commented-out with a `# TODO key`) for the user to bind.
            key: None,
            action_type: classify_action(&a.id, &a.input),
            id: a.id,
        })
        .collect()
}

/// Infer a view-config `type:` from an action id and its input shape. A scaffold
/// heuristic only — the user retypes where it guesses wrong.
fn classify_action(id: &str, _input: &InputSpec) -> &'static str {
    if id.starts_with("edit") {
        "edit"
    } else if id == "add" || id.starts_with("add") || id.starts_with("create") {
        "create"
    } else if id == "reload" || id == "refresh" {
        "reload"
    } else {
        "custom"
    }
}

fn value_type_to_col_kind(vt: &str) -> Option<&'static str> {
    match vt {
        "number" => Some("number"),
        "duration" => Some("duration"),
        "datetime" => Some("datetime"),
        _ => None,
    }
}

/// The part of a type id after the last `:` (`jira:issue:comment` → `comment`).
fn type_local_name(type_id: &str) -> &str {
    type_id.rsplit(':').next().unwrap_or(type_id)
}

/// Assign each top-level view a distinct single-char subtab key: the first
/// unused lowercase letter of its name, then any unused `a..z`.
fn assign_view_keys(views: &mut [GenLevel]) {
    let mut used: HashSet<char> = HashSet::new();
    for v in views.iter_mut() {
        let pick = v
            .name
            .chars()
            .map(|c| c.to_ascii_lowercase())
            .find(|c| c.is_ascii_alphabetic() && !used.contains(c))
            .or_else(|| ('a'..='z').find(|c| !used.contains(c)));
        if let Some(k) = pick {
            used.insert(k);
            v.key = Some(k);
        }
    }
}

// ---------------------------------------------------------------------------
// YAML rendering
// ---------------------------------------------------------------------------

const COLUMN_HINT: &[&str] = &[
    "The protocol can't enumerate this type's remaining columns (column layout",
    "is front-end config). Run `nyd adapter <inst>:<path> ls` against a real",
    "instance to see the row metadata keys, then add columns like:",
    "- { key: <field>, label: <Header>, sizing: max }",
];

fn render(meta: &FileMeta, views: &[GenLevel]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Generated by `nyd config generate` — a scaffold, not a finished config."
    );
    let _ = writeln!(
        out,
        "# All actions are emitted; keyless ones are commented out (assign a key)."
    );
    let _ = writeln!(
        out,
        "# Columns are a best-effort seed — extend them by hand (see each block)."
    );
    let _ = writeln!(
        out,
        "# Prune what you don't need, then save as views/<name>.yaml."
    );
    let _ = writeln!(out);

    // --- tab: ------------------------------------------------------------
    let _ = writeln!(out, "tab:");
    let _ = writeln!(out, "  name: {}", scalar(&meta.tab_name));
    let _ = writeln!(out, "  order: {}", meta.order);
    let _ = writeln!(out, "  # icon: \"\"  # optional nerd-font / emoji glyph");
    let _ = writeln!(out);

    // --- adapter: --------------------------------------------------------
    let _ = writeln!(out, "adapter:");
    let _ = writeln!(out, "  type: {}", scalar(&meta.adapter_type));
    if let Some(id) = &meta.adapter_id {
        let _ = writeln!(out, "  id: {}", scalar(id));
    }
    if let Some(inline) = &meta.config_inline {
        let _ = writeln!(out, "  config_inline: {}", scalar(inline));
    } else if let Some(cfg) = &meta.config {
        let _ = writeln!(out, "  config: {}", scalar(cfg));
    } else {
        let _ = writeln!(
            out,
            "  # config: <adapter-config>.yaml  # or config_inline: '<...>'"
        );
    }
    // `manual_connect` defaults to true, so only the eager choice needs to be
    // written out — spelled explicitly, because "this instance is cheap enough
    // to connect unasked" is a decision worth seeing in the file. The default
    // case still gets a commented hint so the knob is discoverable.
    if meta.manual_connect {
        let _ = writeln!(
            out,
            "  # manual_connect: false  # connect on startup instead of waiting for reload"
        );
    } else {
        let _ = writeln!(out, "  manual_connect: false");
    }
    let _ = writeln!(out);

    // --- views: ----------------------------------------------------------
    let _ = writeln!(out, "views:");
    if views.is_empty() {
        let _ = writeln!(out, "  []  # no views selected");
    }
    for v in views {
        render_level(&mut out, v, 1, true);
    }
    out
}

/// Render one `views:`/`children:` entry at the given indent level (in 2-space
/// units). `is_view` toggles the top-level-only `default:` / `key:` lines.
fn render_level(out: &mut String, lvl: &GenLevel, indent: usize, is_view: bool) {
    let pad = "  ".repeat(indent);
    let ipad = "  ".repeat(indent + 1);

    let _ = writeln!(out, "{pad}- name: {}", scalar(&lvl.name));
    let _ = writeln!(out, "{ipad}node_type: {}", scalar(&lvl.type_id));

    if lvl.recursive {
        // A recursive self-child inherits columns/actions from its ancestor;
        // emit nothing more than the marker.
        let _ = writeln!(out, "{ipad}recursive: true");
        return;
    }

    if is_view {
        if lvl.default {
            let _ = writeln!(out, "{ipad}default: true");
        }
        if let Some(k) = lvl.key {
            let _ = writeln!(out, "{ipad}key: {}", scalar(&k.to_string()));
        }
    }

    // columns:
    if !lvl.columns.is_empty() {
        let _ = writeln!(out, "{ipad}columns:");
        let cpad = "  ".repeat(indent + 2);
        for c in &lvl.columns {
            let _ = writeln!(out, "{cpad}- {}", render_column(c));
        }
        for line in COLUMN_HINT {
            let _ = writeln!(out, "{cpad}# {line}");
        }
    }

    // actions:
    if !lvl.actions.is_empty() {
        let _ = writeln!(out, "{ipad}actions:");
        let apad = "  ".repeat(indent + 2);
        for a in &lvl.actions {
            if a.key.is_some() {
                let _ = writeln!(out, "{apad}- {}", render_action(a));
            } else {
                // Keyless: emit commented so it can't ship unbound.
                let _ = writeln!(out, "{apad}# - {}   # TODO key", render_action(a));
            }
        }
    }

    // children:
    if !lvl.children.is_empty() {
        let _ = writeln!(out, "{ipad}children:");
        for child in &lvl.children {
            render_level(out, child, indent + 2, false);
        }
    }
}

/// A single-line flow map for one column, e.g.
/// `{ key: updated, label: Updated, kind: datetime, sizing: max }`.
fn render_column(c: &GenColumn) -> String {
    let mut parts = vec![format!("key: {}", scalar(&c.key))];
    if let Some(label) = &c.label {
        parts.push(format!("label: {}", scalar(label)));
    }
    if let Some(src) = c.source {
        parts.push(format!("source: {src}"));
    }
    if let Some(kind) = c.kind {
        parts.push(format!("kind: {kind}"));
    }
    parts.push(format!("sizing: {}", scalar(c.sizing)));
    format!("{{ {} }}", parts.join(", "))
}

/// A single-line flow map for one action, e.g.
/// `{ name: "Edit", key: e, type: edit, id: edit }`.
fn render_action(a: &GenAction) -> String {
    let mut parts = vec![format!("name: {}", scalar(&a.name))];
    if let Some(k) = a.key {
        parts.push(format!("key: {}", scalar(&k.to_string())));
    }
    parts.push(format!("type: {}", a.action_type));
    parts.push(format!("id: {}", scalar(&a.id)));
    format!("{{ {} }}", parts.join(", "))
}

/// Render a string as a YAML scalar: plain when unambiguous, double-quoted
/// (with escaping) otherwise. Conservative — anything with structural or
/// ambiguous characters, or that looks like a bool/null/number, gets quoted.
fn scalar(s: &str) -> String {
    if is_plain_safe(s) {
        s.to_string()
    } else {
        let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    }
}

fn is_plain_safe(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // Leading/trailing whitespace, or a leading indicator char, forces quoting.
    if s.trim() != s {
        return false;
    }
    if let Some(first) = s.chars().next() {
        if "-?:,[]{}#&*!|>'\"%@`".contains(first) {
            return false;
        }
    }
    // Any structural / flow / comment character anywhere forces quoting.
    if s.chars()
        .any(|c| ":#,[]{}\"'".contains(c) || c == '\n' || c == '\t')
    {
        return false;
    }
    // Reserved words / numeric-looking tokens must be quoted to stay strings.
    let lower = s.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "true" | "false" | "null" | "yes" | "no" | "~"
    ) {
        return false;
    }
    if s.parse::<f64>().is_ok() {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{MockAdapterBuilder, MockNodeData, issue_type};
    use crate::{AdapterCapabilities, NodeAction};

    fn meta() -> FileMeta {
        FileMeta {
            tab_name: "Mock".to_string(),
            order: 1,
            adapter_type: "mock".to_string(),
            adapter_id: None,
            config: None,
            config_inline: Some("{}".to_string()),
            manual_connect: false,
        }
    }

    fn adapter() -> crate::mock::MockAdapter {
        MockAdapterBuilder::new("mock")
            .instance_id("mock-1")
            .capabilities(AdapterCapabilities {
                supports_create: true,
                ..Default::default()
            })
            .actions_for(
                "mock:root",
                vec![NodeAction::new("add", "add", InputSpec::Editor)],
            )
            .actions_for(
                "mock:issue",
                vec![
                    NodeAction::new("edit", "edit", InputSpec::Editor),
                    // No default key → must be emitted commented out.
                    NodeAction::new("transition", "transition", InputSpec::Picker),
                ],
            )
            .node(
                MockNodeData::new("root", "Root")
                    .child_type(issue_type())
                    .child(MockNodeData::new("ISS-1", "First").node_type(issue_type())),
            )
            .build()
    }

    #[tokio::test]
    async fn generate_emits_tab_adapter_and_views() {
        let a = adapter();
        let yaml = generate(&a, &meta(), &Selection::all()).await.unwrap();
        assert!(yaml.contains("tab:"));
        assert!(yaml.contains("name: Mock"));
        assert!(yaml.contains("adapter:"));
        assert!(yaml.contains("type: mock"));
        assert!(yaml.contains("config_inline:"));
        assert!(yaml.contains("views:"));
        // The issue child type became a view, addressed by its quoted type id.
        assert!(yaml.contains("node_type: \"mock:issue\""));
    }

    #[tokio::test]
    async fn keyless_action_is_commented_out() {
        let a = adapter();
        let yaml = generate(&a, &meta(), &Selection::all()).await.unwrap();
        // `edit` has a default key → live line.
        assert!(yaml.contains("type: edit, id: edit"));
        // `transition` has none → commented with the TODO marker.
        assert!(yaml.contains("# -"));
        assert!(yaml.contains("# TODO key"));
        assert!(yaml.contains("id: transition"));
    }

    #[tokio::test]
    async fn columns_carry_label_seed_and_hint() {
        let a = adapter();
        let yaml = generate(&a, &meta(), &Selection::all()).await.unwrap();
        assert!(yaml.contains("key: label"));
        assert!(yaml.contains("source: label"));
        assert!(yaml.contains("nyd adapter <inst>:<path> ls"));
    }

    #[tokio::test]
    async fn selection_prunes_unlisted_types() {
        let a = adapter();
        let sel = Selection::with_types(HashSet::new());
        let yaml = generate(&a, &meta(), &sel).await.unwrap();
        assert!(!yaml.contains("mock:issue"));
        assert!(yaml.contains("no views selected"));
    }

    #[test]
    fn scalar_quotes_type_ids_and_reserved_words() {
        assert_eq!(scalar("label"), "label");
        assert_eq!(scalar("mock:issue"), "\"mock:issue\"");
        assert_eq!(scalar("flex(1)"), "flex(1)");
        assert_eq!(scalar("true"), "\"true\"");
        assert_eq!(scalar("42"), "\"42\"");
    }
}
