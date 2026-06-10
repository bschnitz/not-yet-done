//! Declarative view configuration loaded from YAML files.
//!
//! Each `.yaml` file in `~/.config/not_yet_done/views/` defines a main tab
//! backed by a ContentAdapter.

use std::collections::HashMap;

use serde::Deserialize;

use crate::action::ActionChains;
use crate::config::keybindings::{ContentAction, KeyBinding, KeyBindingConfig};

// ---------------------------------------------------------------------------
// Top-level config
// ---------------------------------------------------------------------------

/// A complete view configuration loaded from one YAML file.
#[derive(Debug, Clone, Deserialize)]
pub struct ViewFileConfig {
    pub tab: TabConfig,
    pub adapter: AdapterConfig,
    #[serde(default)]
    pub views: Vec<ViewDef>,
}

impl ViewFileConfig {
    /// Check semantic constraints that the deserialiser cannot enforce
    /// (e.g. `id` is required for action types that route through
    /// `Node::execute`). Returns one human-readable error per problem.
    ///
    /// `kb` is the effective keybinding config (defaults merged with
    /// the user's `tui.yaml`); the validator uses it to detect key
    /// conflicts between YAML-defined actions and the runtime's
    /// global / common / content / window bindings.
    pub fn validate(
        &self,
        kb: &KeyBindingConfig,
        editors: &super::editor::EditorsConfig,
    ) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        for view in &self.views {
            for action in &view.actions {
                check_action(view.name.as_str(), None, action, editors, &mut errors);
            }
            check_shortcuts(
                view.name.as_str(),
                None,
                &view.shortcuts,
                &view.actions,
                &mut errors,
            );
            for child in &view.children {
                check_child(view.name.as_str(), child, editors, &mut errors);
            }
            check_tree(view, &mut errors);
            check_row_layout(
                view.name.as_str(),
                None,
                view.row_layout.as_deref(),
                &view.columns,
                &mut errors,
            );
            // `mode: server` without `page_size` is allowed — it tells the
            // adapter to omit the `?page_size=` query param and accept
            // whatever default the server applies (typical for DRF-based
            // APIs like Taiga). The adapter reports the actual page size
            // back via `PageInfo.limit`, which the TUI then uses for
            // `>`/`<` navigation.
        }
        errors.extend(crate::keymap::validate_view_file(self, kb));
        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }
}

fn check_child(
    view: &str,
    child: &ChildDef,
    editors: &super::editor::EditorsConfig,
    errors: &mut Vec<String>,
) {
    for action in &child.actions {
        check_action(view, Some(child.name.as_str()), action, editors, errors);
    }
    check_shortcuts(
        view,
        Some(child.name.as_str()),
        &child.shortcuts,
        &child.actions,
        errors,
    );
    check_row_layout(
        view,
        Some(child.name.as_str()),
        child.row_layout.as_deref(),
        &child.columns,
        errors,
    );
    for nested in &child.children {
        check_child(view, nested, editors, errors);
    }
}

/// Validate a `row_layout:` against the level's `columns`: every column key
/// referenced by a layout line must be a declared column. Empty lines
/// (spacers) reference nothing and are always fine.
fn check_row_layout(
    view: &str,
    child: Option<&str>,
    row_layout: Option<&[LineLayout]>,
    columns: &[ColumnDef],
    errors: &mut Vec<String>,
) {
    let Some(lines) = row_layout else { return };
    let scope = match child {
        Some(c) => format!("views.{view}.children.{c}.row_layout"),
        None => format!("views.{view}.row_layout"),
    };
    for line in lines {
        for key in &line.columns {
            if !columns.iter().any(|c| &c.key == key) {
                errors.push(format!(
                    "{scope}: column '{key}' is not declared in this level's `columns:`"
                ));
            }
        }
        // A `markdown` column expands into N soft-wrapped lines; it can't
        // share a physical line with other columns. Require it to stand
        // alone rather than silently dropping its neighbours at render time.
        let has_markdown = line
            .columns
            .iter()
            .any(|key| columns.iter().any(|c| &c.key == key && c.markdown));
        if has_markdown && line.columns.len() > 1 {
            errors.push(format!(
                "{scope}: a `markdown: true` column must be the only column on its \
                 row_layout line (found {} columns: {:?})",
                line.columns.len(),
                line.columns
            ));
        }
    }
}

/// Validate a `shortcuts:` map against the surrounding `actions:` list.
///
/// We can't check whether the action `id` exists on the adapter at
/// config-load time (the adapter isn't connected yet), so that check
/// happens lazily in the dispatcher. What we *can* check up front:
///
/// 1. The action id is non-empty.
/// 2. The shortcut key isn't already claimed by a YAML-defined
///    `ActionDef` at the same scope — that would be an unresolvable
///    collision at runtime.
fn check_shortcuts(
    view: &str,
    child: Option<&str>,
    shortcuts: &HashMap<char, String>,
    actions: &[ActionDef],
    errors: &mut Vec<String>,
) {
    let scope = match child {
        Some(c) => format!("views.{view}.children.{c}.shortcuts"),
        None => format!("views.{view}.shortcuts"),
    };
    for (key, action_id) in shortcuts {
        // The `parent:` prefix selects the target node and is stripped
        // here before checking emptiness. We don't validate the action
        // name itself — adapters expose actions lazily.
        let body = action_id
            .strip_prefix("parent:")
            .unwrap_or(action_id.as_str());
        if body.trim().is_empty() {
            errors.push(format!(
                "{scope}['{key}']: action id is empty — bind to an adapter action name \
                 (e.g. \"execute\", \"edit\", or \"parent:edit_sql\" to target the parent)"
            ));
        }
        for a in actions {
            // ActionDef.key is a string (may include modifiers like "ctrl+n");
            // a single-char shortcut conflicts only with single-char action keys.
            if a.key.chars().count() == 1
                && a.key.chars().next() == Some(*key)
            {
                errors.push(format!(
                    "{scope}['{key}']: key already bound to view-level action '{}' \
                     (type={}). Remove either the shortcut or the action's key.",
                    a.name, a.action_type
                ));
            }
        }
    }
}

/// Validate `tree_label` references and detect misuse of `fuzzy_filter`
/// / `search` / `tree_find` across a tree chain.
///
/// Rules:
/// 1. Every level whose `tree_label` is set must reference a key
///    present in that level's `columns`.
/// 2. A ChildDef may only set `tree_label` if its enclosing parent
///    (ViewDef or another ChildDef) also has `tree_label` set —
///    otherwise the field is orphaned and silently does nothing.
/// 3. Multiple tree-continuing children at the same level are allowed,
///    but their `node_type` values must be pairwise unique — the
///    walker disambiguates branches by node_type, so duplicates would
///    be ambiguous at re-expand time.
/// 4. Across the entire tree chain, `fuzzy_filter`, `search`, and
///    `tree_find` may only be defined at one level (per plan: defined
///    once at the tree root, applies globally).
/// 5. `tree_find` only makes sense on a tree-enabled level — it
///    drives a tree-aware search that needs a tree to expand into.
///    Defined on a non-tree view or below a non-tree-continuing
///    ChildDef → error.
fn check_tree(view: &ViewDef, errors: &mut Vec<String>) {
    let path = format!("views.{}", view.name);
    let view_has_tree = view.tree_label.is_some();
    if let Some(key) = view.tree_label.as_deref() {
        if !view.columns.iter().any(|c| c.key == key) {
            errors.push(format!(
                "{path}: tree_label '{key}' does not match any column key in this view"
            ));
        }
    }

    // Walk children: warn on orphans (tree_label without active parent
    // chain), count tree-continuing siblings per level. While we walk
    // the active chain, accumulate fuzzy_filter/search/tree_find counts.
    let mut fuzzy_levels: Vec<String> = Vec::new();
    let mut search_levels: Vec<String> = Vec::new();
    let mut tree_find_levels: Vec<String> = Vec::new();
    if view_has_tree {
        collect_input_actions(
            &view.actions,
            &path,
            &mut fuzzy_levels,
            &mut search_levels,
            &mut tree_find_levels,
        );
        check_tree_children_unique(&view.children, &path, errors);
    } else {
        // Outside any tree chain — `tree_find` is meaningless here.
        forbid_tree_find_off_tree(&view.actions, &path, errors);
    }
    for child in &view.children {
        walk_tree_child(
            child,
            &path,
            view_has_tree,
            &mut fuzzy_levels,
            &mut search_levels,
            &mut tree_find_levels,
            errors,
        );
    }

    if fuzzy_levels.len() > 1 {
        errors.push(format!(
            "{path}: fuzzy_filter is defined at multiple tree levels ({}). \
             Define it once at the tree root.",
            fuzzy_levels.join(", ")
        ));
    }
    if search_levels.len() > 1 {
        errors.push(format!(
            "{path}: search is defined at multiple tree levels ({}). \
             Define it once at the tree root.",
            search_levels.join(", ")
        ));
    }
    if tree_find_levels.len() > 1 {
        errors.push(format!(
            "{path}: tree_find is defined at multiple tree levels ({}). \
             Define it once at the tree root — it expands the whole tree \
             to surface hits, so per-level scoping doesn't apply.",
            tree_find_levels.join(", ")
        ));
    }
}

/// Push an error for each `tree_find` action defined on a level that is
/// *not* part of an active tree chain. Used both at the ViewDef root
/// (when the view has no `tree_label`) and on ChildDefs that don't
/// continue the tree.
fn forbid_tree_find_off_tree(actions: &[ActionDef], scope: &str, errors: &mut Vec<String>) {
    for a in actions {
        if a.action_type == "tree_find" {
            errors.push(format!(
                "{scope}.actions[{}]: type='tree_find' requires the enclosing level \
                 to set `tree_label` — tree_find drives a tree-aware search.",
                a.name
            ));
        }
    }
}

fn walk_tree_child(
    child: &ChildDef,
    parent_path: &str,
    parent_has_tree: bool,
    fuzzy_levels: &mut Vec<String>,
    search_levels: &mut Vec<String>,
    tree_find_levels: &mut Vec<String>,
    errors: &mut Vec<String>,
) {
    let path = format!("{parent_path}.children.{}", child.name);
    let child_has_tree = child.tree_label.is_some();

    if child_has_tree && !parent_has_tree {
        errors.push(format!(
            "{path}: tree_label set but no ancestor has tree_label — \
             the tree chain only continues from a tree-active parent"
        ));
    }
    if let Some(key) = child.tree_label.as_deref() {
        if !child.columns.iter().any(|c| c.key == key) {
            errors.push(format!(
                "{path}: tree_label '{key}' does not match any column key at this level"
            ));
        }
    }

    let on_active_chain = parent_has_tree && child_has_tree;
    if on_active_chain {
        collect_input_actions(
            &child.actions,
            &path,
            fuzzy_levels,
            search_levels,
            tree_find_levels,
        );
        check_tree_children_unique(&child.children, &path, errors);
    } else {
        // Off-chain ChildDef — `tree_find` makes no sense here.
        forbid_tree_find_off_tree(&child.actions, &path, errors);
    }

    // DSF-3: `recursive: true` only makes sense for tree-continuing
    // ChildDefs. Without `tree_label` the recursion never becomes
    // visible as tree expansion — silently dead config, so error out.
    if child.recursive && !child_has_tree {
        errors.push(format!(
            "{path}: recursive: true requires tree_label — \
             a non-tree ChildDef can't self-recurse as a tree branch"
        ));
    }

    for nested in &child.children {
        walk_tree_child(
            nested,
            &path,
            child_has_tree,
            fuzzy_levels,
            search_levels,
            tree_find_levels,
            errors,
        );
    }
}

/// Ensure tree-continuing siblings (those with `tree_label`) at one
/// level have pairwise unique `node_type`. The expand-time walker
/// resolves the producing ChildDef by node_type, so duplicates would
/// be ambiguous at re-expand.
fn check_tree_children_unique(children: &[ChildDef], parent_path: &str, errors: &mut Vec<String>) {
    let mut seen: HashMap<&str, &str> = HashMap::new();
    for c in children.iter().filter(|c| c.tree_label.is_some()) {
        if let Some(prev_name) = seen.insert(c.node_type.as_str(), c.name.as_str()) {
            errors.push(format!(
                "{parent_path}: ambiguous tree continuation — duplicate node_type '{}' \
                 used by both tree-continuing children '{}' and '{}'. \
                 Node-types must be unique among tree-continuing siblings.",
                c.node_type, prev_name, c.name
            ));
        }
    }
}

fn collect_input_actions(
    actions: &[ActionDef],
    scope: &str,
    fuzzy_levels: &mut Vec<String>,
    search_levels: &mut Vec<String>,
    tree_find_levels: &mut Vec<String>,
) {
    for a in actions {
        match a.action_type.as_str() {
            "fuzzy_filter" => fuzzy_levels.push(scope.to_string()),
            "search" => search_levels.push(scope.to_string()),
            "tree_find" => tree_find_levels.push(scope.to_string()),
            _ => {}
        }
    }
}

fn check_action(
    view: &str,
    child: Option<&str>,
    a: &ActionDef,
    editors: &super::editor::EditorsConfig,
    errors: &mut Vec<String>,
) {
    let scope = match child {
        Some(c) => format!("views.{view}.children.{c}.actions[{}]", a.name),
        None => format!("views.{view}.actions[{}]", a.name),
    };
    // An `editor:` profile must resolve to a defined profile under
    // `editors:`. Caught here so a typo fails loudly at load instead of
    // silently falling back to `default` at edit time.
    if let Some(profile) = &a.editor {
        if !editors.contains(profile) {
            errors.push(format!(
                "{scope}: editor profile '{profile}' is not defined under `editors:` \
                 (available: {})",
                editors.profile_names().join(", ")
            ));
        }
    }
    match a.action_type.as_str() {
        // Adapter-driven actions: must reference a Node-side action by id.
        // `edit` may omit `id` and falls back to `"edit_full"` for legacy
        // configs; `create` and `custom` have no fallback.
        "create" | "custom" if a.id.is_none() => {
            errors.push(format!("{scope}: type='{}' requires `id` (e.g. id: create_comment)", a.action_type));
        }
        "navigate" if a.navigate_to.is_none() => {
            errors.push(format!("{scope}: type='navigate' requires `navigate_to`"));
        }
        "text_search" if a.text_search.is_none() => {
            errors.push(format!("{scope}: type='text_search' requires `text_search.query_template`"));
        }
        _ => {}
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TabConfig {
    pub name: String,
    #[serde(default)]
    pub order: i32,
    #[serde(default)]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AdapterConfig {
    #[serde(rename = "type")]
    pub adapter_type: String,
    /// Stable per-instance identifier — used for the on-disk data
    /// directory (`<data>/not_yet_done/<adapter_type>/<id>/`) and for
    /// scoping things like saved queries. Default = `adapter_type`,
    /// which means a single configured adapter of a given type just
    /// works without further config. Multiple instances of the same
    /// adapter type must each set an explicit `id:` — the loader
    /// errors on collision.
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub config: Option<String>,
    #[serde(default)]
    pub config_inline: Option<String>,
    /// When `true`, no load is spawned automatically for this tab —
    /// neither at startup nor when switching to a still-unloaded
    /// subtab. The user must trigger a `reload` action (e.g. the
    /// `r` key bound to `type: reload`) to make the adapter actually
    /// connect and fetch. While unloaded the view shows a
    /// "Press <key> to connect" banner.
    ///
    /// Use this for adapters whose connection is expensive or
    /// unreliable (Postgres-over-SSH-tunnel via Bastion, slow VPN-
    /// gated APIs) — auto-loading them on TUI startup would either
    /// hang for many seconds or surface confusing timeouts when the
    /// network prerequisite is missing.
    #[serde(default)]
    pub manual_connect: bool,
}

impl AdapterConfig {
    /// Effective instance id — explicit `id:` if given, else
    /// `adapter_type`.
    pub fn effective_instance_id(&self) -> &str {
        self.id.as_deref().unwrap_or(&self.adapter_type)
    }
}

// ---------------------------------------------------------------------------
// View definition (subtab or navigable level)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct ViewDef {
    pub name: String,
    pub node_type: String,
    #[serde(default)]
    pub default: bool,
    /// Optional shortcut key that switches the parent content tab to this
    /// view (subtab navigation). Only honored when at root level.
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub query: Option<QueryConfig>,
    #[serde(default)]
    pub columns: Vec<ColumnDef>,
    /// Optional multi-line row layout. When set, each entry is one physical
    /// line of every row, listing the columns rendered on it (`[]` = blank
    /// spacer). Absent → the classic single-line table. In multi-line mode
    /// the column header is suppressed. Used e.g. for the Stoat chat layout
    /// (meta line + message body + spacer). See [`LineLayout`].
    #[serde(default)]
    pub row_layout: Option<Vec<LineLayout>>,
    /// Smooth (line-wise) scrolling. When `true`, navigation moves the
    /// viewport one physical line at a time over the whole content instead
    /// of jumping entry-to-entry; the content glides continuously and the
    /// active row is the first fully-visible one. Default `false` (classic
    /// row-by-row scrolling). Meant for long, multi-line lists (e.g. chat).
    #[serde(default)]
    pub smooth_scroll: bool,
    #[serde(default)]
    pub preview: Option<PreviewConfig>,
    #[serde(default)]
    pub actions: Vec<ActionDef>,
    #[serde(default)]
    pub children: Vec<ChildDef>,
    /// Pagination strategy for this view. `None` = adapter default
    /// (typically fetch-all + client-side slicing).
    #[serde(default)]
    pub pagination: Option<PaginationConfig>,
    /// Subtab-scoped action chains; resolved before the global chain map
    /// but after any matching ChildDef chain. `None` for a key disables
    /// it at this scope without falling back further.
    #[serde(default)]
    pub action_chains: ActionChains,
    /// Enable the optional column cursor for this view. When true, the
    /// table tracks a per-cell highlight in addition to the row cursor;
    /// `column_left`/`column_right` move it. Default: false.
    #[serde(default)]
    pub column_cursor: bool,
    /// When set, this view renders as a tree: rows of this level expand
    /// into rows of the first ChildDef that itself sets `tree_label`,
    /// and so on down the chain. The referenced key must exist in
    /// `columns`; the tree-glyph + indent are drawn in that column,
    /// other columns hold the level's normal cell value. `None` keeps
    /// the legacy flat list behaviour.
    #[serde(default)]
    pub tree_label: Option<String>,
    /// Number of additional attempts after the first failed load.
    /// `0` (default) = no retries (legacy behaviour, error shown
    /// immediately). `2` means: 1 initial attempt + 2 retries = up to
    /// 3 attempts before the error becomes sticky. Applies to root
    /// loads, drill-down loads, and tree expansions under this view.
    ///
    /// Trade-off: more retries mean longer perceived hangs on a
    /// genuinely broken backend (each attempt pays the adapter's own
    /// timeout). Pick the value to match how transient the failures
    /// you see actually are — a Postgres tunnel with `query_timeout_secs:
    /// 7` and `retries: 2` blocks up to 21s before giving up.
    #[serde(default)]
    pub retries: u32,
    /// Scaffold inserted into a new script created via the `:script`
    /// menu on this view. When `None`, falls back to
    /// `script.template`. Use this for views whose JSON node shape
    /// benefits from a tailored starter (e.g. a Taiga-item template
    /// that pre-references `fields.ref` and `fields.assignee`).
    #[serde(default)]
    pub script_template: Option<String>,
    /// Per-node-type shortcuts. Maps a key (single char) to an adapter-
    /// declared action `id` (returned from `Node::actions`). At runtime
    /// the TUI calls `Node::invoke_action(id)` and dispatches the
    /// returned `ActionDispatch`. Action `id`s are validated lazily —
    /// pressing a key bound to an unknown action surfaces an error in
    /// the status bar.
    #[serde(default)]
    pub shortcuts: HashMap<char, String>,
    /// Glyph shown in the tree-mode label column when a row of this
    /// level is *not* expandable (no children). `None` falls back to
    /// the default `·`. Set this to a semantic glyph (e.g. `📄` on a
    /// pages level) when the leaf state has its own meaning beyond
    /// "no children". Adapters that report
    /// `NodeSummary.has_children = Some(false)` mark leaves that
    /// otherwise would be guessed expandable via the static config
    /// check; this is how Confluence's pages level distinguishes pages
    /// with no sub-pages from pages with sub-pages despite both rows
    /// sitting under the same `recursive: true` ChildDef.
    #[serde(default)]
    pub leaf_glyph: Option<String>,
    /// Default grouping for this view (M3). Partitions the flat list into
    /// groups, each introduced by a header row. Runtime-switchable via
    /// view-state (this is only the startup default). `None` = ungrouped
    /// flat list. Ignored in tree mode (tree-fold is a separate feature).
    #[serde(default)]
    pub group_by: Option<GroupBy>,
    /// Per-column aggregations applied when grouping is active (M3). Each
    /// names a column to total per group; the grand total appears in a
    /// footer row. Empty → groups are pure label headers with no totals.
    #[serde(default)]
    pub aggregates: Vec<AggregateDef>,
    /// Collapse each group to a single summary row (M3 — Trackings
    /// "Condensed"). The per-group header (carrying its totals) is then the
    /// only row shown; individual items are hidden. Only meaningful with
    /// `group_by` set.
    #[serde(default)]
    pub summary_only: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaginationConfig {
    pub mode: PaginationMode,
    /// Optional even when `mode: server`. If omitted, the adapter omits
    /// the `?page_size=` query parameter and lets the server pick its
    /// default; the actual size comes back via `PageInfo.limit` and
    /// drives subsequent `>`/`<` requests. Ignored when `mode: all`.
    #[serde(default)]
    pub page_size: Option<u32>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PaginationMode {
    /// Adapter fetches one API page per `>` / `<` step. The displayed
    /// sort applies only to the rows on the current page (the adapter
    /// pre-sorts server-side where it can; everything else is local
    /// page-only sort).
    Server,
    /// Adapter fetches all rows, then sorts and paginates client-side.
    /// Equivalent to omitting the `pagination` block.
    All,
    /// Server-side cursor pagination. Adapter declares a NO-SCROLL
    /// cursor on the underlying query and returns one page per `>`
    /// step. `<` (page-prev) re-issues the cursor — there is no
    /// backward fetch. Currently honoured by the Postgres adapter for
    /// custom queries; other adapters fall back to `Server`.
    Cursor,
}

// ---------------------------------------------------------------------------
// Query
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct QueryConfig {
    #[serde(default, deserialize_with = "deserialize_optional_query_source")]
    pub default: Option<String>,
    #[serde(default)]
    pub editable: bool,
    /// Key to open the query menu popup (e.g. "q").
    #[serde(default)]
    pub menu_key: Option<String>,
}

/// Accept either a string (for adapters that take a verbatim query, e.g.
/// Jira's JQL) or any YAML structure (re-serialized to a string for the
/// adapter to parse). Lets users write the Taiga query as a native YAML
/// sequence instead of a `|` block scalar.
fn deserialize_query_source<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v: serde_yaml::Value = serde::Deserialize::deserialize(d)?;
    yaml_value_to_query_string(v).map_err(serde::de::Error::custom)
}

fn deserialize_optional_query_source<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v: Option<serde_yaml::Value> = serde::Deserialize::deserialize(d)?;
    match v {
        None | Some(serde_yaml::Value::Null) => Ok(None),
        Some(other) => yaml_value_to_query_string(other)
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

fn yaml_value_to_query_string(v: serde_yaml::Value) -> Result<String, serde_yaml::Error> {
    match v {
        serde_yaml::Value::String(s) => Ok(s),
        other => serde_yaml::to_string(&other),
    }
}

// ---------------------------------------------------------------------------
// Columns
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct ColumnDef {
    pub key: String,
    #[serde(default)]
    pub label: Option<String>,
    /// "label" = use node.label(), otherwise metadata key.
    #[serde(default)]
    pub source: Option<String>,
    /// Theme color reference (e.g. "accent", "text_med", "success").
    #[serde(default)]
    pub style: Option<String>,
    /// "max" or "flex(N)".
    #[serde(default = "default_sizing")]
    pub sizing: String,
    /// When `true`, this column's value is Markdown and is rendered as
    /// multiple soft-wrapped lines (headings, lists, inline emphasis, …)
    /// instead of a single fitted line. Intended for chat / long-text
    /// columns (e.g. the Stoat message body). A `markdown` column must be
    /// the only column on its `row_layout` line (enforced by the validator).
    #[serde(default)]
    pub markdown: bool,
    /// Semantic type of this column's value (M2). Drives how the table
    /// engine parses, formats, aligns and styles the cell. The adapter
    /// emits a *canonical* string for the kind (duration → integer
    /// seconds, datetime → RFC 3339, path → `/`-separated segments,
    /// number → decimal); the engine turns it into the display form.
    /// Defaults to [`ColumnKind::Text`] so every existing (remote) column
    /// is unaffected.
    #[serde(default)]
    pub kind: ColumnKind,
    /// Optional format override for kinds that support one (e.g. a custom
    /// strftime-style pattern for `datetime`). `None` = the kind's default
    /// rendering.
    #[serde(default)]
    pub format: Option<String>,
    /// Segment separator for `kind: path` (default `/`). Ignored by other
    /// kinds.
    #[serde(default)]
    pub separator: Option<String>,
    /// Source field key for `kind: elapsed` (M5 live-elapsed). The cell
    /// renders `now − <this field's datetime>` as a duration, recomputed on
    /// each repaint tick. Defaults to the column's own `key` when omitted.
    /// Ignored by every other kind.
    #[serde(default)]
    pub elapsed_from: Option<String>,
    /// Tree-fold aggregation declaration (M4). When set, this column can show
    /// either its own per-node value (the `key` field) or the adapter-computed
    /// subtree-cumulated value (the `cumulated_field`), toggled at runtime via
    /// `toggle_tree_aggregate`. Only meaningful in tree mode and only when the
    /// adapter advertises `supports_tree_aggregation`. The TUI never folds the
    /// tree itself (the tree is lazy-loaded — collapsed branches are not in
    /// memory); the adapter must supply both values as metadata fields.
    #[serde(default)]
    pub tree_aggregate: Option<TreeAggregate>,
}

/// Tree-fold aggregation for a [`ColumnDef`] (M4). The column reads one of two
/// adapter-supplied metadata fields depending on the level's toggle state: its
/// own `key` field (per-node value) or [`cumulated_field`](Self::cumulated_field)
/// (the adapter's subtree sum). Showing both at once = two columns on two
/// fields; this is for the *switchable* single column.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeAggregate {
    /// Metadata field key carrying the adapter-computed subtree-cumulated
    /// value (canonical for the column's `kind`, e.g. integer seconds for a
    /// `duration` column). The own value still comes from the column's `key`.
    pub cumulated_field: String,
    /// Which value the column shows before the user toggles. Default `own`.
    #[serde(default)]
    pub default: TreeAggregateDefault,
}

/// Initial state for a [`TreeAggregate`] column (M4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TreeAggregateDefault {
    /// Show the per-node value (the column's `key` field). The default.
    #[default]
    Own,
    /// Show the adapter's subtree-cumulated value (the `cumulated_field`).
    Cumulated,
}

/// Semantic type of a [`ColumnDef`] value (M2 — typed column values).
///
/// The value lives only in the view YAML; adapters stay untyped and emit a
/// canonical string per kind (see [`ColumnDef::kind`]). The table engine
/// parses that string and produces the aligned, formatted, styled cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColumnKind {
    /// Plain text, rendered verbatim, left-aligned. The default — every
    /// remote adapter's columns are this without opting in.
    #[default]
    Text,
    /// A decimal number. Right-aligned.
    Number,
    /// A time span. Canonical input: integer seconds. Rendered `Hh Mm`,
    /// right-aligned.
    Duration,
    /// An instant. Canonical input: RFC 3339. Rendered localized.
    Datetime,
    /// A hierarchical path. Canonical input: separator-joined segments.
    /// Rendered with the separator drawn in the theme's
    /// `taskpath_separator` style.
    Path,
    /// A live elapsed duration (M5). The cell holds no value of its own;
    /// it renders `now − <elapsed_from field>` (an RFC 3339 instant) as a
    /// duration, right-aligned, recomputed on every repaint tick driven by
    /// the domain-event bus. See [`ColumnDef::elapsed_from`].
    Elapsed,
}

// ---------------------------------------------------------------------------
// Grouping + aggregation (M3)
// ---------------------------------------------------------------------------

/// Date-bucket granularity for [`GroupBy`] (M3). When set, the group
/// column's value is parsed as an RFC 3339 instant and truncated to this
/// boundary so all items in the same day / week / month / year coalesce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DateBucket {
    Day,
    Week,
    Month,
    Year,
}

/// Grouping declaration (M3). Partitions the flat row list by a column's
/// value — verbatim, or, when `bucket` is set, by the date bucket the
/// column's datetime falls into. This is only the *default*: grouping is
/// switchable at runtime via view-state (see `cycle_grouping`), so a user
/// can regroup or turn it off without an adapter round-trip.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupBy {
    /// Column `key` whose value identifies the group (and labels its header).
    pub column: String,
    /// When set, parse the column value as an RFC 3339 instant and group by
    /// the surrounding day/week/month/year instead of the verbatim value.
    /// A value that fails to parse falls back to verbatim grouping.
    #[serde(default)]
    pub bucket: Option<DateBucket>,
}

/// Aggregation operation for an [`AggregateDef`] (M3). Only `sum` exists
/// today (totals a `duration` column's seconds); kept as an enum so
/// `count` / `avg` / … can be added later without a config break.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AggregateOp {
    #[default]
    Sum,
}

/// Per-column aggregation (M3). When grouping is active, the named column's
/// values are combined per group (and grand-totalled in the footer) using
/// `op`. The total renders in that same column on the group-header / summary
/// row, so it lines up under the data. Currently only `duration` columns
/// (canonical integer seconds) sum meaningfully.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AggregateDef {
    /// Column `key` whose values are aggregated.
    pub column: String,
    /// How to combine the values. Default `sum`.
    #[serde(default)]
    pub op: AggregateOp,
}

fn default_sizing() -> String {
    "max".to_string()
}

/// One physical line of a multi-line row layout (see
/// [`ViewDef::row_layout`]). Lists the columns rendered on that line, in
/// order; an empty list is a blank spacer.
///
/// Deserializes from either a shorthand sequence of column keys
/// (`[author, time]`) — highlighting on select unless empty — or a map
/// (`{ columns: [...], highlight_on_select: false }`) for the escape hatch
/// where a non-empty line should stay outside the selection block.
#[derive(Debug, Clone)]
pub struct LineLayout {
    pub columns: Vec<String>,
    /// Whether this line is painted with the selection style when the row
    /// is selected. Defaults to `true` for non-empty lines, `false` for an
    /// empty (spacer) line.
    pub highlight_on_select: bool,
}

impl<'de> Deserialize<'de> for LineLayout {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            // Shorthand: a bare list of column keys (or `[]` for a spacer).
            Short(Vec<String>),
            // Full form with an explicit highlight override.
            Full {
                #[serde(default)]
                columns: Vec<String>,
                #[serde(default)]
                highlight_on_select: Option<bool>,
            },
        }
        let (columns, explicit) = match Raw::deserialize(deserializer)? {
            Raw::Short(columns) => (columns, None),
            Raw::Full {
                columns,
                highlight_on_select,
            } => (columns, highlight_on_select),
        };
        let highlight_on_select = explicit.unwrap_or(!columns.is_empty());
        Ok(LineLayout {
            columns,
            highlight_on_select,
        })
    }
}

// ---------------------------------------------------------------------------
// Preview
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct PreviewConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// "content" = node.content().read_text() (default).
    /// "action" = run a Node action's `prepare` and show its template
    /// (requires `action: <id>`); useful for previewing the buffer that
    /// `e` would open.
    #[serde(default = "default_content_source")]
    pub source: String,
    /// Adapter-side action id when `source: action`
    /// (e.g. `edit_with_comments`).
    #[serde(default)]
    pub action: Option<String>,
    /// Optional metadata-field key whose value supplies the node_id for
    /// the preview instead of the selected row's own id. Mirrors
    /// `ActionDef.node_id_from`; lets a list of pointers (e.g.
    /// notifications) preview the linked target.
    #[serde(default)]
    pub node_id_from: Option<String>,
    /// "horizontal" (left/right) or "vertical" (top/bottom).
    #[serde(default = "default_split")]
    pub split: String,
    #[serde(default = "default_ratio")]
    pub ratio: u16,
    #[serde(default)]
    pub keybinding: Option<String>,
    /// When `true`, the preview text is rendered as Markdown (soft-wrapped,
    /// inline emphasis, headings, lists) instead of plain wrapped text.
    /// Useful for chat/long-text previews (e.g. the Stoat message body).
    #[serde(default)]
    pub markdown: bool,
}

fn default_true() -> bool { true }
fn default_content_source() -> String { "content".to_string() }
fn default_split() -> String { "horizontal".to_string() }
fn default_ratio() -> u16 { 50 }

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionDef {
    pub name: String,
    pub key: String,
    #[serde(rename = "type")]
    pub action_type: String,
    /// Adapter-side action identifier (e.g. `"edit_full"`, `"transition"`,
    /// `"create_comment"`). Required for `edit`/`create`/`custom` action
    /// types that route through `Node::prepare`/`picker_options`/`execute`.
    #[serde(default)]
    pub id: Option<String>,
    /// Optional metadata-field key whose value supplies the `node_id` for
    /// adapter-routed actions instead of the selected row's own id. Used
    /// when one row points at another node (e.g. a notification linking
    /// to a ticket). Empty/missing values cause the action to be a no-op.
    #[serde(default)]
    pub node_id_from: Option<String>,
    /// For "navigate" actions: target child node-type id.
    #[serde(default)]
    pub navigate_to: Option<String>,
    /// For "fuzzy_filter" actions — optional filter config.
    #[serde(default)]
    pub fuzzy_filter: Option<FuzzyFilterConfig>,
    /// For "search" actions — optional search field config.
    #[serde(default)]
    pub search: Option<SearchConfig>,
    /// For "text_search" actions — required query template (`{q}` is replaced
    /// by the escaped user input).
    #[serde(default)]
    pub text_search: Option<TextSearchConfig>,
    /// For "tree_find" actions — optional UI prompt (CT-10). The
    /// query itself is passed unmodified to
    /// [`not_yet_done_content::ContentAdapter::search_in_tree`], so
    /// there's no `query_template` here — the adapter chooses the
    /// query language (CQL for Confluence, JQL for Jira, …).
    #[serde(default)]
    pub tree_find: Option<TreeFindActionConfig>,
    /// Hide this action from the action bar (top bar).
    /// By default, actions with a persistent/modal state (edit, create,
    /// query_edit, fuzzy_filter) are shown in the action bar. Set this
    /// to true to only show the action in the status bar keybinding hints.
    #[serde(default)]
    pub hide_from_bar: bool,
    /// Editor profile to open for `edit`/`create` actions, by name (a key
    /// under the top-level `editors:` block). `None` → the `default`
    /// profile. Validated at config load: an unknown name is a hard error.
    /// Lets one action open the editor in a different terminal geometry
    /// (e.g. a chat compose in a slim split) than the global default.
    #[serde(default)]
    pub editor: Option<String>,
    /// For `create` actions: take the parent from the **currently selected
    /// row** instead of the drilled-into container. Lets a tree (or a flat
    /// list) add a child *under the highlighted node* without first drilling
    /// into it — e.g. "add subtask under the selected task" in the task tree,
    /// where the default `create` would otherwise add a sibling at the
    /// container level (the forest root in tree mode). Default `false` keeps
    /// the container-parent behaviour every other create relies on. No-op
    /// when nothing is selected.
    #[serde(default)]
    pub under_selection: bool,
    /// Apply the action on every editor save (`:w`) instead of only on
    /// editor close. Requires a detached editor profile (`inline: false`)
    /// so intermediate saves are observable. Built for chat-style compose:
    /// the first `:w` sends the message, each later `:w` edits it in place
    /// (the create action's new node id is captured and subsequent saves
    /// retarget that node's editor action). Saving with no change since the
    /// last apply — including repeated `:w` — is a no-op, so the message is
    /// never sent twice. Default `false` preserves the commit-on-close
    /// behaviour every other action relies on; enabling it for, say, a Jira
    /// ticket edit would push a partial body on every keystroke-save.
    #[serde(default)]
    pub commit_on_save: bool,
}

impl ActionDef {
    /// Whether this action should appear in the action bar by default.
    /// Actions with persistent/modal state belong in the action bar:
    /// - edit/create/query_edit: open an editor (shown as active while open)
    /// - fuzzy_filter/search/text_search: takes over the bar with an input field
    /// - script: opens a fuzzy picker over the per-node scripts directory
    /// - tree_find: opens a search input over the adapter's tree-aware
    ///   search (Confluence CQL etc.) and stays "active" while a result
    ///   set is cached
    /// Actions that are fire-and-forget belong in the status bar only:
    /// - reload, navigate, custom, open_url, download
    pub fn shows_in_action_bar(&self) -> bool {
        if self.hide_from_bar {
            return false;
        }
        matches!(
            self.action_type.as_str(),
            "edit"
                | "create"
                | "query_edit"
                | "fuzzy_filter"
                | "search"
                | "text_search"
                | "tree_find"
                | "script"
        )
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FuzzyFilterConfig {
    /// Which metadata fields to search. If empty or absent, searches all fields + label.
    #[serde(default)]
    pub fields: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchConfig {
    /// Which metadata fields to search. If empty or absent, searches label + all fields.
    #[serde(default)]
    pub fields: Vec<String>,
    /// Key to jump to the next match while the search has results. Defaults to `n`.
    #[serde(default)]
    pub next_key: Option<String>,
    /// Key to jump to the previous match while the search has results. Defaults to `N`.
    #[serde(default)]
    pub prev_key: Option<String>,
}

/// CT-10: per-action config for the `tree_find` action type. Today
/// only `prompt` is honoured (mirrors `TextSearchConfig::prompt`);
/// future tuning knobs like `limit:` would land here too.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct TreeFindActionConfig {
    /// UI label shown inside the input bar while typing. Falls back
    /// to "tree search…" when absent. Use it to spell out the
    /// adapter's query language (e.g. `"CQL"`) or the search scope
    /// (`"Search pages"`) so the user knows what the field will do.
    #[serde(default)]
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TextSearchConfig {
    /// Adapter query template. The literal substring `{q}` is replaced by the
    /// user's input (with `\` and `"` escaped). For Jira this is typically
    /// `text ~ "{q}" ORDER BY updated DESC`.
    ///
    /// Also supports `{key_or}` which expands to `issuekey = "{q}" OR ` when
    /// the input matches `PROJECT-123` and to the empty string otherwise.
    ///
    /// Accepts either a string or a YAML structure (re-serialized to a string
    /// for the adapter) — see `deserialize_query_source`.
    #[serde(deserialize_with = "deserialize_query_source")]
    pub query_template: String,
    /// Optional UI prompt shown in the action bar while the search input is
    /// open (e.g. `"Jira-Suche"`). Falls back to `"free-text search…"` if
    /// not set.
    #[serde(default)]
    pub prompt: Option<String>,
}

// ---------------------------------------------------------------------------
// Children (nested navigation)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct ChildDef {
    pub name: String,
    pub node_type: String,
    #[serde(default)]
    pub columns: Vec<ColumnDef>,
    /// Multi-line row layout for this drill level. Same semantics as
    /// [`ViewDef::row_layout`]. Used by the Stoat `messages` level to render
    /// each message as a meta line + body line + spacer.
    #[serde(default)]
    pub row_layout: Option<Vec<LineLayout>>,
    /// Smooth (line-wise) scrolling for this drill level. Same semantics as
    /// [`ViewDef::smooth_scroll`]; default `false`. Set on the Stoat
    /// `messages` level so the chat history glides line-by-line.
    #[serde(default)]
    pub smooth_scroll: bool,
    #[serde(default)]
    pub preview: Option<PreviewConfig>,
    #[serde(default)]
    pub actions: Vec<ActionDef>,
    /// Nested drill-down levels. Each entry mirrors the parent's `children:`
    /// shape, so the tree can be arbitrarily deep (e.g. database → schemas
    /// → tables → table for the Postgres adapter).
    #[serde(default)]
    pub children: Vec<ChildDef>,
    /// If set, drilling into this child opens a new pane in a split next
    /// to the current one instead of replacing it in place.
    #[serde(default)]
    pub split: Option<SplitDef>,
    /// Pagination strategy for this child level. `None` falls back to
    /// the drill-down default (50 rows). Use `mode: server, page_size: 100`
    /// to drive `>`/`<` navigation through `LIMIT … OFFSET …`-style adapters.
    #[serde(default)]
    pub pagination: Option<PaginationConfig>,
    /// Per-level keybinding overrides for [`ContentAction`]s (`back`,
    /// `open`, `next_page`, `prev_page`, `edit_query`). Use `null` as
    /// the value to **disable** an action at this drill level (e.g.
    /// `back: null` to drop `h`/Backspace inside a paginated rows
    /// view). Missing entries fall back to the global content
    /// keybindings.
    #[serde(default)]
    pub keybindings: HashMap<ContentAction, Option<KeyBinding>>,
    /// Drill-level action chains. Most-specific scope: this wins over
    /// `ViewDef::action_chains` and the global map. `None` for a key
    /// disables it at this scope without falling back further.
    #[serde(default)]
    pub action_chains: ActionChains,
    /// Enable the optional column cursor when drilling into this child.
    /// Same semantics as `ViewDef::column_cursor` — applied to the pane
    /// that displays this child's items.
    #[serde(default)]
    pub column_cursor: bool,
    /// Continue the parent's tree chain into this child level. When set
    /// AND the parent (ViewDef or another ChildDef) also has
    /// `tree_label`, drilling into a row of the parent expands its
    /// children inline as the next tree depth instead of pushing a new
    /// drill-down. The referenced key must exist in this child's
    /// `columns`. Leaving it unset terminates the tree chain — drill
    /// behaviour at that point falls back to the configured `split`
    /// (or in-place replace).
    #[serde(default)]
    pub tree_label: Option<String>,
    /// Per-node-type shortcuts at this drill-down level. Same semantics
    /// as `ViewDef::shortcuts` — maps single-char keys to adapter
    /// `Node::actions` ids dispatched through `Node::invoke_action`.
    #[serde(default)]
    pub shortcuts: HashMap<char, String>,
    /// Override Enter (the `content.open` key) on a row of this
    /// ChildDef's node-type so it dispatches a `Node::invoke_action`
    /// instead of the default drill-down. Used for "synthetic" child
    /// levels that exist only to anchor split/pagination config — e.g.
    /// `postgres:db_script_result` is never produced by `Node::list`,
    /// so Enter on a `postgres:db_script` row sets `enter_action:
    /// execute` to fire the same path as the `x` shortcut. When set,
    /// drill semantics (Branch 1 expand or Branch 2 ContentDrill) are
    /// skipped entirely. The value must match a `Node::actions` id.
    #[serde(default)]
    pub enter_action: Option<String>,
    /// Mark this ChildDef as an implicit member of its own `children:`
    /// — used for arbitrarily deep self-similar trees (e.g. Postgres
    /// `db_script_dir` nesting under itself). The effective child set
    /// at any depth becomes `{self, …declared_children}`, and the
    /// chain walker stays on this def whenever the next chain segment
    /// has the same `node_type`. Requires `tree_label` (otherwise the
    /// recursion never becomes visible as tree expansion).
    #[serde(default)]
    pub recursive: bool,
    /// When true, editor temp files for actions opened from this level
    /// (e.g. the Postgres DB-script `e` action) are created in the
    /// real persisted-file directory instead of `$TMPDIR`. Lets
    /// external editors / LSPs discover sibling config files like
    /// `postgres-language-server.jsonc`. The temp file is prefixed
    /// (e.g. `.nyd_tmp_`) so leftovers from a crashed session are
    /// recognisable. Default `false` preserves the legacy `$TMPDIR`
    /// behaviour for all other view-defs.
    #[serde(default)]
    pub editor_in_place: bool,
    /// Per-ChildDef override of the leaf glyph (`·` default). See
    /// [`ViewDef::leaf_glyph`] for the wider semantics; this field
    /// applies when the entry's level is reached via this ChildDef.
    #[serde(default)]
    pub leaf_glyph: Option<String>,
    /// Default grouping for this drill level (M3). Same semantics as
    /// [`ViewDef::group_by`]; applies to the pane that displays this
    /// child's items. Runtime-switchable. Ignored in tree mode.
    #[serde(default)]
    pub group_by: Option<GroupBy>,
    /// Per-column aggregations for this drill level (M3). Same semantics as
    /// [`ViewDef::aggregates`].
    #[serde(default)]
    pub aggregates: Vec<AggregateDef>,
    /// Collapse each group to a single summary row at this drill level
    /// (M3). Same semantics as [`ViewDef::summary_only`].
    #[serde(default)]
    pub summary_only: bool,
}

/// Split direction relative to the source pane: where the new pane lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitDirection {
    Left,
    Right,
    Top,
    Bottom,
}

impl Default for SplitDirection {
    fn default() -> Self {
        SplitDirection::Right
    }
}

fn default_split_ratio() -> f32 {
    0.5
}

/// How a child drill-down opens in a split. `ratio` is the share of the
/// available area allocated to the **new** pane (the drilled-into one).
#[derive(Debug, Clone, Deserialize)]
pub struct SplitDef {
    #[serde(default)]
    pub direction: SplitDirection,
    #[serde(default = "default_split_ratio")]
    pub ratio: f32,
    /// When true, the drilled-into pane is **owned** by the source pane:
    /// re-drilling from the same parent reuses (hot-replaces) the existing
    /// child instead of opening another split, and closing the parent
    /// cascades to the child. The reverse cleanup also applies — closing
    /// the child clears the parent's backlink.
    #[serde(default)]
    pub coupled: bool,
}

impl Default for SplitDef {
    fn default() -> Self {
        Self {
            direction: SplitDirection::default(),
            ratio: default_split_ratio(),
            coupled: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_config() {
        let yaml = r#"
tab:
  name: Jira
  order: 3
  icon: "󰌃"
adapter:
  type: jira
  config: jira-adapter.yaml
views:
  - name: tickets
    node_type: "jira:issue"
    default: true
    query:
      default: "assignee = currentUser()"
      editable: true
    columns:
      - key: key
        label: Key
        style: accent
      - key: summary
        source: label
        sizing: "flex(1)"
    preview:
      keybinding: p
    actions:
      - name: edit
        key: e
        type: edit
        id: edit_full
      - name: refresh
        key: r
        type: reload
      - name: comments
        key: c
        type: navigate
        navigate_to: "jira:comment"
    children:
      - name: Comments
        key: c
        node_type: "jira:comment"
        columns:
          - key: author
          - key: body
            sizing: "flex(1)"
"#;
        let config: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();

        // Tab
        assert_eq!(config.tab.name, "Jira");
        assert_eq!(config.tab.order, 3);
        assert_eq!(config.tab.icon.as_deref(), Some("󰌃"));

        // Adapter
        assert_eq!(config.adapter.adapter_type, "jira");
        assert_eq!(config.adapter.config.as_deref(), Some("jira-adapter.yaml"));

        // Views
        assert_eq!(config.views.len(), 1);
        let view = &config.views[0];
        assert_eq!(view.name, "tickets");
        assert_eq!(view.node_type, "jira:issue");
        assert!(view.default);

        // Query
        let query = view.query.as_ref().unwrap();
        assert_eq!(query.default.as_deref(), Some("assignee = currentUser()"));
        assert!(query.editable);

        // Columns
        assert_eq!(view.columns.len(), 2);
        assert_eq!(view.columns[0].key, "key");
        assert_eq!(view.columns[0].label.as_deref(), Some("Key"));
        assert_eq!(view.columns[0].style.as_deref(), Some("accent"));
        assert_eq!(view.columns[0].sizing, "max"); // default
        assert_eq!(view.columns[1].source.as_deref(), Some("label"));
        assert_eq!(view.columns[1].sizing, "flex(1)");

        // Preview
        let preview = view.preview.as_ref().unwrap();
        assert!(preview.enabled); // default true
        assert_eq!(preview.source, "content"); // default
        assert_eq!(preview.split, "horizontal"); // default
        assert_eq!(preview.ratio, 50); // default
        assert_eq!(preview.keybinding.as_deref(), Some("p"));

        // Actions
        assert_eq!(view.actions.len(), 3);
        assert_eq!(view.actions[0].action_type, "edit");
        assert_eq!(view.actions[0].id.as_deref(), Some("edit_full"));
        assert_eq!(view.actions[1].action_type, "reload");
        assert_eq!(view.actions[2].action_type, "navigate");
        assert_eq!(view.actions[2].navigate_to.as_deref(), Some("jira:comment"));

        // Children
        assert_eq!(view.children.len(), 1);
        assert_eq!(view.children[0].name, "Comments");
        assert_eq!(view.children[0].node_type, "jira:comment");
        assert_eq!(view.children[0].columns.len(), 2);
        assert_eq!(view.children[0].columns[1].sizing, "flex(1)");
    }

    #[test]
    fn parse_minimal_config() {
        let yaml = r#"
tab:
  name: Test
adapter:
  type: mock
"#;
        let config: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.tab.name, "Test");
        assert_eq!(config.tab.order, 0); // default
        assert!(config.tab.icon.is_none());
        assert_eq!(config.adapter.adapter_type, "mock");
        assert!(config.adapter.config.is_none());
        assert!(config.views.is_empty());
    }

    #[test]
    fn parse_preview_defaults() {
        let yaml = r#"
tab:
  name: T
adapter:
  type: x
views:
  - name: v
    node_type: t
    preview: {}
"#;
        let config: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let preview = config.views[0].preview.as_ref().unwrap();
        assert!(preview.enabled);
        assert_eq!(preview.source, "content");
        assert_eq!(preview.split, "horizontal");
        assert_eq!(preview.ratio, 50);
        assert!(preview.keybinding.is_none());
    }

    #[test]
    fn parse_inline_adapter_config() {
        let yaml = r#"
tab:
  name: DB
adapter:
  type: postgres
  config_inline: |
    host: localhost
    port: 5432
    database: mydb
"#;
        let config: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.adapter.adapter_type, "postgres");
        assert!(config.adapter.config.is_none());
        let inline = config.adapter.config_inline.as_deref().unwrap();
        assert!(inline.contains("host: localhost"));
        assert!(inline.contains("port: 5432"));
    }

    #[test]
    fn column_sizing_default_is_max() {
        let yaml = r#"
tab:
  name: T
adapter:
  type: x
views:
  - name: v
    node_type: t
    columns:
      - key: col1
      - key: col2
        sizing: "flex(2)"
"#;
        let config: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.views[0].columns[0].sizing, "max");
        assert_eq!(config.views[0].columns[1].sizing, "flex(2)");
    }

    #[test]
    fn shows_in_action_bar_defaults() {
        let make = |action_type: &str| ActionDef {
            name: "test".into(), key: "x".into(), action_type: action_type.into(), id: None,
            node_id_from: None,
            navigate_to: None,
            fuzzy_filter: None, search: None, text_search: None, tree_find: None, hide_from_bar: false,
            editor: None,
            under_selection: false,
            commit_on_save: false,
        };
        // Modal/persistent state actions → action bar
        assert!(make("edit").shows_in_action_bar());
        assert!(make("create").shows_in_action_bar());
        assert!(make("query_edit").shows_in_action_bar());
        assert!(make("fuzzy_filter").shows_in_action_bar());
        assert!(make("search").shows_in_action_bar());
        assert!(make("text_search").shows_in_action_bar());
        assert!(make("tree_find").shows_in_action_bar());
        assert!(make("script").shows_in_action_bar());
        // Fire-and-forget actions → status bar only
        assert!(!make("reload").shows_in_action_bar());
        assert!(!make("navigate").shows_in_action_bar());
        assert!(!make("custom").shows_in_action_bar());
        assert!(!make("open_url").shows_in_action_bar());
        assert!(!make("download").shows_in_action_bar());
    }

    #[test]
    fn hide_from_bar_overrides_default() {
        let action = ActionDef {
            name: "edit".into(), key: "e".into(), action_type: "edit".into(), id: None,
            node_id_from: None,
            navigate_to: None,
            fuzzy_filter: None, search: None, text_search: None, tree_find: None, hide_from_bar: true,
            editor: None,
            under_selection: false,
            commit_on_save: false,
        };
        assert!(!action.shows_in_action_bar());
    }

    #[test]
    fn parse_fuzzy_filter_action() {
        let yaml = r#"
tab:
  name: T
adapter:
  type: x
views:
  - name: v
    node_type: t
    actions:
      - name: fuzzy filter
        key: f
        type: fuzzy_filter
        fuzzy_filter:
          fields: [key, summary]
      - name: edit
        key: e
        type: edit
        hide_from_bar: true
"#;
        let config: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let actions = &config.views[0].actions;
        assert_eq!(actions.len(), 2);

        assert_eq!(actions[0].action_type, "fuzzy_filter");
        assert!(actions[0].shows_in_action_bar());
        let ff = actions[0].fuzzy_filter.as_ref().unwrap();
        assert_eq!(ff.fields, vec!["key", "summary"]);

        assert_eq!(actions[1].action_type, "edit");
        assert!(!actions[1].shows_in_action_bar()); // hide_from_bar overrides
    }

    #[test]
    fn deny_unknown_fields_on_action() {
        // Legacy field `custom_action` is no longer accepted.
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    actions:
      - { name: tr, key: t, type: custom, custom_action: transition }
"#;
        let err = serde_yaml::from_str::<ViewFileConfig>(yaml).unwrap_err();
        assert!(err.to_string().contains("custom_action"), "got: {err}");
    }

    #[test]
    fn validate_requires_id_for_create() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    children:
      - { name: c, key: c, node_type: t2, actions: [{ name: add, key: a, type: create }] }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let errs = cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap_err();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("create") && errs[0].contains("id"));
    }

    #[test]
    fn example_tasks_yaml_parses_and_validates() {
        // The shipped `docs/examples/views/tasks.yaml` (plan phase A1) must
        // stay loadable: it parses into a ViewFileConfig and passes the
        // semantic validator (tree_label chain, recursive branch, typed
        // columns, tree_find/fuzzy_filter only at the tree root).
        let yaml = include_str!("../../../docs/examples/views/tasks.yaml");
        let cfg: ViewFileConfig =
            serde_yaml::from_str(yaml).expect("tasks.yaml should deserialize");
        assert_eq!(cfg.adapter.adapter_type, "tasks");
        assert_eq!(cfg.views[0].tree_label.as_deref(), Some("description"));
        assert!(
            cfg.views[0].columns.iter().any(|c| c.key == "priority"
                && matches!(c.kind, ColumnKind::Number)),
            "priority column should be kind: number"
        );
        // A1b mutation bindings: edit/add as typed actions, the
        // delete/undelete/reparent quartet as generic shortcuts, on both
        // the root view and the recursive subtask branch.
        let root = &cfg.views[0];
        let edit = root.actions.iter().find(|a| a.action_type == "edit").unwrap();
        assert_eq!(edit.id.as_deref(), Some("edit"));
        let add = root.actions.iter().find(|a| a.action_type == "create").unwrap();
        assert_eq!(add.id.as_deref(), Some("add"));
        for (k, name) in [
            ('d', "delete"),
            ('u', "undelete"),
            ('t', "toggle-tracking"),
            ('m', "mark-move"),
            ('p', "paste-move"),
        ] {
            assert_eq!(root.shortcuts.get(&k), Some(&name.to_string()));
        }
        // A1c-1: the tracking marker column is declared on both levels.
        assert!(
            root.columns.iter().any(|c| c.key == "tracking"),
            "root view should declare the tracking marker column"
        );
        let child = &root.children[0];
        assert!(child.actions.iter().any(|a| a.action_type == "edit"));
        assert!(child.actions.iter().any(|a| a.action_type == "create"));
        assert_eq!(child.shortcuts.get(&'d'), Some(&"delete".to_string()));
        assert_eq!(child.shortcuts.get(&'t'), Some(&"toggle-tracking".to_string()));
        assert!(child.columns.iter().any(|c| c.key == "tracking"));
        // A1c-2: the root view declares a saved-query block — editable, with
        // a `q` menu key and a default whose body re-serializes to the YAML
        // document the tasks adapter parses (a `name` + a `query` FilterExpr).
        let query = root.query.as_ref().expect("root view should declare a query block");
        assert!(query.editable, "tasks query should be editable");
        assert_eq!(query.menu_key.as_deref(), Some("q"));
        let body = query.default.as_ref().expect("tasks query should ship a default body");
        let parsed = not_yet_done_core::filter::query_filter::parse(body)
            .expect("default tasks query body should parse as a FilterExpr document");
        assert_eq!(parsed.name, "open tasks");
        // A1c (scripts): a `type: script` action on both levels reaches the
        // generic script menu (key `x`). The validator does not restrict
        // `script` to the tree root (unlike search/fuzzy_filter/tree_find).
        let root_script = root.actions.iter().find(|a| a.action_type == "script").unwrap();
        assert_eq!(root_script.key, "x");
        assert!(child.actions.iter().any(|a| a.action_type == "script"));
        // A1c comfort extras: `A` adds a child under the selected node
        // (`under_selection`), `U` un-nests to the top level. Both on both
        // levels so they work in tree mode (root view) and when drilled.
        let add_child = root.actions.iter().find(|a| a.key == "A").unwrap();
        assert_eq!(add_child.action_type, "create");
        assert_eq!(add_child.id.as_deref(), Some("add"));
        assert!(add_child.under_selection);
        assert_eq!(root.shortcuts.get(&'U'), Some(&"unnest".to_string()));
        let child_add = child.actions.iter().find(|a| a.key == "A").unwrap();
        assert!(child_add.under_selection);
        assert_eq!(child.shortcuts.get(&'U'), Some(&"unnest".to_string()));

        cfg.validate(
            &KeyBindingConfig::default(),
            &crate::config::editor::EditorsConfig::default(),
        )
        .expect("tasks.yaml should pass the validator");
    }

    #[test]
    fn validate_rejects_unknown_editor_profile() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    actions:
      - { name: edit, key: e, type: edit, id: edit_full, editor: nope }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        // Default editors only defines `default`, so `nope` is unknown.
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .unwrap_err();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("nope") && errs[0].contains("editors"), "got: {}", errs[0]);
    }

    #[test]
    fn validate_accepts_known_editor_profile() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    actions:
      - { name: edit, key: e, type: edit, id: edit_full, editor: compose-below }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let editors: crate::config::editor::EditorsConfig =
            serde_yaml::from_str("default: {}\ncompose-below: {}").unwrap();
        cfg.validate(&KeyBindingConfig::default(), &editors).unwrap();
    }

    #[test]
    fn action_parses_editor_profile_field() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    actions:
      - { name: send, key: a, type: create, id: send_message, editor: compose-below }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            cfg.views[0].actions[0].editor.as_deref(),
            Some("compose-below")
        );
    }

    #[test]
    fn validate_requires_id_for_custom() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    actions:
      - { name: tr, key: t, type: custom }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let errs = cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap_err();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("custom") && errs[0].contains("id"));
    }

    #[test]
    fn validate_text_search_requires_template() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    actions:
      - { name: free, key: s, type: text_search }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let errs = cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap_err();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("text_search") && errs[0].contains("query_template"));
    }

    #[test]
    fn pagination_server_without_page_size_validates() {
        // Omitting `page_size` is the documented way to defer to the
        // server's own default (e.g. DRF's `PAGE_SIZE = 30`).
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: notifications
    node_type: t
    pagination: { mode: server }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap();
        let p = cfg.views[0].pagination.as_ref().unwrap();
        assert_eq!(p.mode, PaginationMode::Server);
        assert_eq!(p.page_size, None);
    }

    #[test]
    fn pagination_server_with_page_size_validates() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: notifications
    node_type: t
    pagination: { mode: server, page_size: 30 }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap();
        let p = cfg.views[0].pagination.as_ref().unwrap();
        assert_eq!(p.mode, PaginationMode::Server);
        assert_eq!(p.page_size, Some(30));
    }

    #[test]
    fn pagination_all_validates_without_page_size() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: items
    node_type: t
    pagination: { mode: all }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap();
        let p = cfg.views[0].pagination.as_ref().unwrap();
        assert_eq!(p.mode, PaginationMode::All);
    }

    #[test]
    fn parse_text_search_action() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    actions:
      - name: free
        key: s
        type: text_search
        text_search:
          query_template: 'text ~ "{q}"'
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap();
        let action = &cfg.views[0].actions[0];
        assert_eq!(action.action_type, "text_search");
        assert!(action.shows_in_action_bar());
        assert_eq!(
            action.text_search.as_ref().unwrap().query_template,
            r#"text ~ "{q}""#,
        );
    }

    #[test]
    fn validate_navigate_requires_target() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    actions:
      - { name: nav, key: n, type: navigate }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let errs = cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap_err();
        assert!(errs[0].contains("navigate_to"));
    }

    #[test]
    fn query_default_accepts_yaml_sequence() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    query:
      default:
        - { type: task, project: 2 }
        - { type: task, project: 3 }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let q = cfg.views[0].query.as_ref().unwrap();
        let default = q.default.as_deref().unwrap();
        // Re-serialized YAML — must parse back as a sequence with both projects.
        let parsed: serde_yaml::Value = serde_yaml::from_str(default).unwrap();
        let seq = parsed.as_sequence().expect("default should be a YAML sequence");
        assert_eq!(seq.len(), 2);
        assert_eq!(seq[0].get("project").unwrap().as_u64(), Some(2));
        assert_eq!(seq[1].get("project").unwrap().as_u64(), Some(3));
    }

    #[test]
    fn query_default_still_accepts_string() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    query:
      default: "assignee = currentUser()"
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let q = cfg.views[0].query.as_ref().unwrap();
        assert_eq!(q.default.as_deref(), Some("assignee = currentUser()"));
    }

    #[test]
    fn validate_accepts_valid_config() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    actions:
      - { name: edit, key: e, type: edit, id: edit_full }
      - { name: tr, key: t, type: custom, id: transition }
      - { name: nav, key: c, type: navigate, navigate_to: t2 }
      - { name: refresh, key: r, type: reload }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap();
    }

    #[test]
    fn tree_label_defaults_to_none() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    children:
      - { name: c, node_type: t2 }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.views[0].tree_label.is_none());
        assert!(cfg.views[0].children[0].tree_label.is_none());
    }

    #[test]
    fn tree_label_parses_when_set() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: db
    tree_label: name
    columns:
      - { key: name }
    children:
      - name: schema
        node_type: schema
        tree_label: name
        columns: [{ key: name }]
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap();
        assert_eq!(cfg.views[0].tree_label.as_deref(), Some("name"));
        assert_eq!(cfg.views[0].children[0].tree_label.as_deref(), Some("name"));
    }

    #[test]
    fn validate_tree_label_requires_existing_column() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    tree_label: missing
    columns: [{ key: name }]
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let errs = cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("tree_label 'missing'")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn validate_row_layout_rejects_unknown_column() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    columns: [{ key: author }, { key: content }]
    row_layout:
      - [author, nope]
      - [content]
      - []
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("row_layout") && e.contains("'nope'")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn validate_row_layout_rejects_markdown_column_sharing_a_line() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    columns:
      - { key: author }
      - { key: content, markdown: true }
    row_layout:
      - [author, content]
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("row_layout") && e.contains("only column")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn validate_row_layout_accepts_markdown_column_alone() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    columns:
      - { key: author }
      - { key: content, markdown: true }
    row_layout:
      - [author]
      - [content]
      - []
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        // A markdown column on its own line is fine.
        assert!(
            cfg.validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .is_ok()
        );
    }

    #[test]
    fn validate_row_layout_accepts_known_columns_and_spacer() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    columns: [{ key: author }, { key: time }, { key: content }]
    row_layout:
      - [author, time]
      - [content]
      - []
      - { columns: [content], highlight_on_select: false }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        // The spacer (`[]`) defaults to highlight_on_select=false; the
        // explicit-map line overrides it on a non-empty line.
        let view = &cfg.views[0];
        let layout = view.row_layout.as_ref().unwrap();
        assert!(!layout[2].highlight_on_select, "empty spacer defaults off");
        assert!(layout[0].highlight_on_select, "shorthand non-empty defaults on");
        assert!(!layout[3].highlight_on_select, "explicit override off");
        // Validation passes (no unknown-column errors).
        let res = cfg.validate(
            &KeyBindingConfig::default(),
            &crate::config::editor::EditorsConfig::default(),
        );
        assert!(
            res.is_ok() || !res.unwrap_err().iter().any(|e| e.contains("row_layout")),
            "row_layout must validate clean",
        );
    }

    #[test]
    fn smooth_scroll_defaults_off_and_parses_per_level() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    columns: [{ key: name }]
    children:
      - name: msgs
        node_type: m
        smooth_scroll: true
        columns: [{ key: body }]
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let view = &cfg.views[0];
        // Absent on the root view → defaults to false.
        assert!(!view.smooth_scroll);
        // Explicit on the child level → true.
        assert!(view.children[0].smooth_scroll);
    }

    #[test]
    fn validate_tree_label_on_child_requires_existing_column() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: db
    tree_label: name
    columns: [{ key: name }]
    children:
      - name: schema
        node_type: schema
        tree_label: oops
        columns: [{ key: label }]
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let errs = cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("tree_label 'oops'") && e.contains("schema")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn validate_tree_label_child_without_parent_is_orphan() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: db
    columns: [{ key: name }]
    children:
      - name: schema
        node_type: schema
        tree_label: name
        columns: [{ key: name }]
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let errs = cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("no ancestor has tree_label")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn validate_tree_chain_accepts_multiple_tree_children_with_unique_types() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: db
    tree_label: name
    columns: [{ key: name }]
    children:
      - { name: a, node_type: t1, tree_label: name, columns: [{ key: name }] }
      - { name: b, node_type: t2, tree_label: name, columns: [{ key: name }] }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        // Unique node_types among tree-continuing children → OK.
        assert!(cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).is_ok());
    }

    /// The Stoat tree shape: a heterogeneous server level (category +
    /// uncategorized channel as tree-continuing siblings) where the same
    /// `stoat:channel` type appears at two different depths (under the
    /// server and under a category), each drilling into a non-tree
    /// `stoat:message` leaf. The duplicate-node_type rule is per-level
    /// (siblings only), so reuse across depths must validate cleanly.
    #[test]
    fn validate_accepts_heterogeneous_category_channel_tree() {
        let yaml = r#"
tab: { name: Stoat }
adapter: { type: stoat }
views:
  - name: chats
    node_type: "stoat:server"
    tree_label: name
    columns: [{ key: name, source: label }]
    actions:
      - { name: find, key: /, type: tree_find, tree_find: { prompt: x } }
    children:
      - name: channels
        node_type: "stoat:channel"
        tree_label: name
        columns: [{ key: name, source: label }]
        children:
          - name: messages
            node_type: "stoat:message"
            columns: [{ key: content, source: label }]
      - name: categories
        node_type: "stoat:category"
        tree_label: name
        columns: [{ key: name, source: label }]
        children:
          - name: channels
            node_type: "stoat:channel"
            tree_label: name
            columns: [{ key: name, source: label }]
            children:
              - name: messages
                node_type: "stoat:message"
                columns: [{ key: content, source: label }]
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(
            cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).is_ok(),
            "got: {:?}",
            cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap_err()
        );
    }

    #[test]
    fn validate_tree_chain_rejects_duplicate_tree_node_types_at_root() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: db
    tree_label: name
    columns: [{ key: name }]
    children:
      - name: schema_a
        node_type: schema
        tree_label: name
        columns: [{ key: name }]
      - name: schema_b
        node_type: schema
        tree_label: name
        columns: [{ key: name }]
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let errs = cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("duplicate node_type")),
            "got: {errs:?}"
        );
    }

    /// DSF-3: `recursive: true` without `tree_label` is silently dead
    /// config — the recursion would never become visible as tree
    /// expansion. Validator must surface this as an error.
    #[test]
    fn validate_recursive_requires_tree_label() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: db
    tree_label: name
    columns: [{ key: name }]
    children:
      - name: dir
        node_type: dir
        columns: [{ key: name }]
        recursive: true
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let errs = cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("recursive: true requires tree_label")),
            "got: {errs:?}"
        );
    }

    /// DSF-3: `recursive: true` together with `tree_label` is valid —
    /// this is the canonical configuration for a self-recursive
    /// directory branch (e.g. `db_script_dir`).
    #[test]
    fn validate_recursive_with_tree_label_is_ok() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: db
    tree_label: name
    columns: [{ key: name }]
    children:
      - name: dir
        node_type: dir
        tree_label: name
        columns: [{ key: name }]
        recursive: true
        children:
          - { name: leaf, node_type: leaf, columns: [{ key: name }] }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap();
    }

    #[test]
    fn validate_tree_chain_rejects_duplicate_tree_node_types_at_inner_level() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: db
    tree_label: name
    columns: [{ key: name }]
    children:
      - name: schema
        node_type: schema
        tree_label: name
        columns: [{ key: name }]
        children:
          - { name: a, node_type: same, tree_label: name, columns: [{ key: name }] }
          - { name: b, node_type: same, tree_label: name, columns: [{ key: name }] }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let errs = cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("duplicate node_type")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn validate_tree_rejects_fuzzy_filter_on_multiple_levels() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: db
    tree_label: name
    columns: [{ key: name }]
    actions:
      - { name: ff, key: f, type: fuzzy_filter }
    children:
      - name: schema
        node_type: schema
        tree_label: name
        columns: [{ key: name }]
        actions:
          - { name: ff, key: g, type: fuzzy_filter }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let errs = cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("fuzzy_filter is defined at multiple tree levels")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn validate_tree_accepts_fuzzy_filter_on_one_level() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: db
    tree_label: name
    columns: [{ key: name }]
    actions:
      - { name: ff, key: f, type: fuzzy_filter }
    children:
      - name: schema
        node_type: schema
        tree_label: name
        columns: [{ key: name }]
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap();
    }

    /// CT-4: `tree_find` only makes sense on tree-enabled levels — it
    /// drives the adapter's tree-aware search, then expands the tree to
    /// surface hits. On a non-tree view it has no tree to expand into.
    #[test]
    fn validate_tree_find_requires_tree_label_on_view() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    columns: [{ key: name }]
    actions:
      - { name: tf, key: '/', type: tree_find }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let errs = cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("type='tree_find'")
                && e.contains("tree_label")),
            "got: {errs:?}"
        );
    }

    /// CT-4: same check for ChildDefs that are NOT on an active tree
    /// chain. Defining `tree_find` on a flat drill-down child level
    /// would silently do nothing — surface it as an error.
    #[test]
    fn validate_tree_find_requires_tree_chain_on_child() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    columns: [{ key: name }]
    children:
      - name: c
        node_type: t2
        columns: [{ key: name }]
        actions:
          - { name: tf, key: '/', type: tree_find }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let errs = cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("type='tree_find'")
                && e.contains("tree_label")),
            "got: {errs:?}"
        );
    }

    /// CT-4: `tree_find` on a tree-enabled view validates successfully
    /// and lifts the action into the action bar (analogous to
    /// `fuzzy_filter` / `search`).
    #[test]
    fn validate_tree_find_on_tree_view_is_ok() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: spaces
    node_type: space
    tree_label: name
    columns: [{ key: name }]
    actions:
      - { name: tree-find, key: '/', type: tree_find }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap();
        let action = &cfg.views[0].actions[0];
        assert_eq!(action.action_type, "tree_find");
        assert!(action.shows_in_action_bar());
    }

    /// CT-4: like fuzzy_filter / search, `tree_find` must be defined at
    /// at most one tree level — otherwise it's ambiguous which level
    /// owns the search input + result cache.
    #[test]
    fn validate_tree_rejects_tree_find_on_multiple_levels() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: db
    tree_label: name
    columns: [{ key: name }]
    actions:
      - { name: tf1, key: '/', type: tree_find }
    children:
      - name: schema
        node_type: schema
        tree_label: name
        columns: [{ key: name }]
        actions:
          - { name: tf2, key: '?', type: tree_find }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let errs = cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("tree_find is defined at multiple tree levels")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn validate_rejects_duplicate_action_keys_via_keymap() {
        // End-to-end smoke: two actions in the same subtab share a key.
        // The keymap validator runs as part of `ViewFileConfig::validate`
        // and surfaces the conflict with the `views.<name>` path prefix.
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: tables
    node_type: t
    actions:
      - { name: fuzzy, key: f, type: fuzzy_filter }
      - { name: find,  key: f, type: custom, id: find }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let errs = cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("views.tables") && e.contains("\"f\"")),
            "expected a tables/'f' conflict, got: {errs:?}"
        );
    }

    #[test]
    fn parse_shortcuts_field_on_view_and_child() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: scripts
    node_type: s
    shortcuts:
      x: execute
      e: edit
    children:
      - name: result
        node_type: s_row
        shortcuts:
          d: delete
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let view = &cfg.views[0];
        assert_eq!(view.shortcuts.get(&'x'), Some(&"execute".to_string()));
        assert_eq!(view.shortcuts.get(&'e'), Some(&"edit".to_string()));
        assert_eq!(
            view.children[0].shortcuts.get(&'d'),
            Some(&"delete".to_string())
        );
    }

    #[test]
    fn parse_shortcuts_field_defaults_to_empty() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.views[0].shortcuts.is_empty());
    }

    #[test]
    fn validate_rejects_empty_shortcut_action_id() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    shortcuts:
      x: ""
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let errs = cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("views.v.shortcuts['x']")
                && e.contains("action id is empty")),
            "expected an empty-id error for views.v shortcut 'x', got: {errs:?}"
        );
    }

    #[test]
    fn validate_rejects_shortcut_colliding_with_action_key() {
        // Action 'r' is reload; the shortcut also binds 'r'. The static
        // validator must catch this — at runtime there'd be no way to
        // dispatch to both.
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    actions:
      - { name: refresh, key: r, type: reload }
    shortcuts:
      r: execute
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let errs = cfg.validate(&KeyBindingConfig::default(), &crate::config::editor::EditorsConfig::default()).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("views.v.shortcuts['r']")
                && e.contains("already bound")),
            "expected a collision error for shortcut 'r' vs action 'refresh', got: {errs:?}"
        );
    }
}

