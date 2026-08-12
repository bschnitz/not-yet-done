//! Declarative view configuration loaded from YAML files.
//!
//! Each `.yaml` file in `~/.config/not_yet_done/views/` defines a main tab
//! backed by a ContentAdapter.

use std::collections::{HashMap, HashSet};

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
    /// Optional per-tab reminder handling. When present and enabled, the
    /// frontend subscribes to the adapter's reminder stream
    /// ([`ContentAdapter::subscribe_reminders`]) and runs `command` for each
    /// reminder that fires. Absent → the adapter may still emit reminders, but
    /// nothing acts on them. The adapter owns *when* a reminder fires; this
    /// block owns *whether* we care and *what* runs.
    ///
    /// [`ContentAdapter::subscribe_reminders`]: not_yet_done_content::ContentAdapter::subscribe_reminders
    #[serde(default)]
    pub reminder: Option<ReminderConfig>,
}

/// Frontend-side reminder handling for one tab (see [`ViewFileConfig::reminder`]).
///
/// ```yaml
/// reminder:
///   enabled: true
///   command: notify-send "$NYD_REMINDER_TITLE" "in $NYD_REMINDER_LEAD_MINUTES min — $NYD_REMINDER_DETAIL"
/// ```
///
/// `command` runs through `sh -c`, detached, with the reminder's fields
/// exported as environment variables (`NYD_REMINDER_ID`, `_TITLE`, `_DETAIL`,
/// `_WHEN`, `_UNTIL`, `_LEAD_MINUTES`) — passing them as env rather than
/// string-splicing keeps event titles from ever being interpreted as shell.
/// `_UNTIL` is the item's end instant (empty when it has none), so a command
/// can keep a notification on screen until the moment has passed.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReminderConfig {
    /// Whether reminders are acted on. Defaults to `true` so that simply
    /// declaring the block with a `command` is enough.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Shell command run for each reminder (via `sh -c`, detached).
    pub command: String,
}

impl ViewFileConfig {
    /// Propagate columns down each tree-continuation chain so a tree need
    /// only declare its columns once, at the root.
    ///
    /// All rows of a tree render into **one** shared column grid, so a
    /// `ChildDef` that continues the tree (`tree_label` set) and omits
    /// `columns:` should show the same columns as the level above it.
    /// Rather than re-declaring the identical set at every depth (which
    /// drifts), such a level inherits the nearest non-empty ancestor's
    /// columns here, once, right after deserialisation — so the validator
    /// and every runtime column lookup see a fully-populated set and need
    /// no inheritance logic of their own.
    ///
    /// Scope is deliberately narrow:
    /// - only tree-continuation levels inherit (gated on `tree_label`); a
    ///   plain drill child with no columns keeps the metadata auto-fallback
    ///   (e.g. the Postgres rows view),
    /// - a level that declares its own `columns:` is untouched and becomes
    ///   the inheritance source for any tree-continuation levels below it,
    /// - separate views (a non-tree sibling `ViewDef`, e.g. a flat list)
    ///   are independent and never inherit across the view boundary.
    pub fn inherit_tree_columns(&mut self) {
        for view in &mut self.views {
            let parent_cols = view.columns.clone();
            for child in &mut view.children {
                inherit_columns_into(child, &parent_cols);
            }
        }
    }

    /// Propagate inheritable per-row actions/shortcuts down the tree so a
    /// recursive tree (e.g. the task forest) declares them once at the root
    /// instead of repeating the identical block at every depth.
    ///
    /// Per-entry opt-in: only [`ActionDef::inherit`] actions and
    /// [`ShortcutDef::inherit`] shortcuts propagate; everything else stays
    /// local. The scope mirrors [`Self::inherit_tree_columns`]:
    /// - only tree-continuation levels inherit (gated on `tree_label`); a
    ///   plain drill child is left alone,
    /// - a child that binds the **same key** itself overrides the inherited
    ///   entry (the local binding wins, the inherited one is dropped),
    /// - inherited entries keep their `inherit` flag, so they cascade to
    ///   every depth (the recursive branch is its own deeper level),
    /// - the single-level search family (`tree_find`/`search`/`fuzzy_filter`)
    ///   is never propagated — those are declared once at the tree root and
    ///   already apply tree-wide; copying them down would trip the
    ///   one-level-only validator ([`check_tree`]).
    ///
    /// Runs after parse, **before** [`Self::validate`], in both load paths.
    pub fn inherit_tree_actions(&mut self) {
        for view in &mut self.views {
            let parent_actions: Vec<ActionDef> = view
                .actions
                .iter()
                .filter(|a| a.inherit && is_inheritable_action_type(&a.action_type))
                .cloned()
                .collect();
            let parent_shortcuts: HashMap<char, ShortcutDef> = view
                .shortcuts
                .iter()
                .filter(|(_, sc)| sc.inherit())
                .map(|(k, sc)| (*k, sc.clone()))
                .collect();
            for child in &mut view.children {
                inherit_actions_into(child, &parent_actions, &parent_shortcuts);
            }
        }
    }

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
        // A tab needs at least one view (subtab) — the runtime builds one
        // pane tree per view and always addresses `views[active_subtab]`,
        // so an empty list would panic on the first access. Reject it here
        // as a broken slot with a clear message instead.
        if self.views.is_empty() {
            errors.push(format!(
                "tab '{}': `views` is empty — define at least one view (subtab)",
                self.tab.name
            ));
            return Err(errors);
        }
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
            // `group_headers` renders adapter group buckets as header rows —
            // only meaningful on a tree view that actually groups.
            if view.group_headers.is_some() {
                if view.tree_label.is_none() {
                    errors.push(format!(
                        "views.{}: group_headers requires tree mode (set tree_label)",
                        view.name
                    ));
                }
                if view.group_by.is_none() {
                    errors.push(format!(
                        "views.{}: group_headers requires a group_by (the adapter-grouped tree root)",
                        view.name
                    ));
                }
            }
            // `script_source` must name a sibling view in this file; a
            // typo would silently fall back to the view's own scope
            // (defeating the shared-source intent), so flag it here.
            if let Some(src) = &view.script_source {
                if !self.views.iter().any(|v| &v.name == src) {
                    errors.push(format!(
                        "views.{}: script_source '{}' names no view in this file",
                        view.name, src
                    ));
                }
            }
            check_row_layout(
                view.name.as_str(),
                None,
                view.row_layout.as_deref(),
                &view.columns,
                &mut errors,
            );
            check_card(
                view.name.as_str(),
                None,
                view.card.as_ref(),
                &view.columns,
                &mut errors,
            );
            // `event_actions:` rules must point at a real action in this
            // view — a typo'd `run:` would silently never fire. Topics are
            // free-form (the adapter contract), but must be non-empty.
            for binding in &view.event_actions {
                if binding.on.trim().is_empty() {
                    errors.push(format!(
                        "views.{}: event_actions entry has an empty `on:` topic",
                        view.name
                    ));
                }
                if !view.actions.iter().any(|a| a.name == binding.run) {
                    errors.push(format!(
                        "views.{}: event_actions `run: {}` names no action in this view",
                        view.name, binding.run
                    ));
                }
            }
            // `mode: server` without `page_size` is allowed — it tells the
            // adapter to omit the `?page_size=` query param and accept
            // whatever default the server applies (typical for DRF-based
            // APIs like Taiga). The adapter reports the actual page size
            // back via `PageInfo.limit`, which the TUI then uses for
            // `>`/`<` navigation.
        }
        errors.extend(crate::keymap::validate_view_file(self, kb));
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
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
    check_card(
        view,
        Some(child.name.as_str()),
        child.card.as_ref(),
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

/// Validate a `card:` block against the level's `columns`.
///
/// Card mode reads its values straight out of the level's cells, so every
/// field must name a declared column. The rest catches config that would
/// silently produce an unusable card: `columns: 0`, a `weights:` list that
/// doesn't line up with the grid, or a `markdown:` column — that one expands
/// into N soft-wrapped lines and cannot live in a fixed-height grid slot.
///
/// An omitted `fields:` means "every column of this level", so it is only an
/// error when that leaves nothing to show.
fn check_card(
    view: &str,
    child: Option<&str>,
    card: Option<&CardConfig>,
    columns: &[ColumnDef],
    errors: &mut Vec<String>,
) {
    let Some(card) = card else { return };
    let scope = match child {
        Some(c) => format!("views.{view}.children.{c}.card"),
        None => format!("views.{view}.card"),
    };
    if card.fields.is_empty() && !columns.iter().any(|c| !c.markdown) {
        errors.push(format!(
            "{scope}: `fields:` is omitted (= all columns) but this level has no \
             column the card could show"
        ));
    }
    if card.columns == 0 {
        errors.push(format!(
            "{scope}: `columns: 0` is not a grid — use 1 or more fields per line"
        ));
    }
    if !card.weights.is_empty() && card.weights.len() != card.columns {
        errors.push(format!(
            "{scope}: `weights:` has {} entries but `columns:` is {} — give one \
             weight per grid column or drop `weights:` for equal shares",
            card.weights.len(),
            card.columns
        ));
    }
    if card.weights.iter().all(|w| *w == 0) && !card.weights.is_empty() {
        errors.push(format!(
            "{scope}: all `weights:` are 0 — at least one grid column needs a share"
        ));
    }
    for field in &card.fields {
        match columns.iter().find(|c| c.key == field.column) {
            None => errors.push(format!(
                "{scope}: field '{}' is not declared in this level's `columns:`",
                field.column
            )),
            Some(col) if col.markdown => errors.push(format!(
                "{scope}: field '{}' is a `markdown: true` column — markdown expands \
                 into multiple lines and cannot sit in a card grid slot",
                field.column
            )),
            Some(_) => {}
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
    shortcuts: &HashMap<char, ShortcutDef>,
    actions: &[ActionDef],
    errors: &mut Vec<String>,
) {
    let scope = match child {
        Some(c) => format!("views.{view}.children.{c}.shortcuts"),
        None => format!("views.{view}.shortcuts"),
    };
    for (key, shortcut) in shortcuts {
        let action_id = shortcut.action();
        // The `parent:` prefix selects the target node and is stripped
        // here before checking emptiness. We don't validate the action
        // name itself — adapters expose actions lazily.
        let body = action_id.strip_prefix("parent:").unwrap_or(action_id);
        if body.trim().is_empty() {
            errors.push(format!(
                "{scope}['{key}']: action id is empty — bind to an adapter action name \
                 (e.g. \"execute\", \"edit\", or \"parent:edit_sql\" to target the parent)"
            ));
        }
        for a in actions {
            // ActionDef.key may hold several alternatives (each possibly
            // modifier-prefixed like "ctrl+n"); a single-char shortcut
            // conflicts only with an alternative that is exactly that char.
            // Event-only actions (no key) never collide.
            if a.key_strings()
                .iter()
                .any(|k| k.chars().count() == 1 && k.chars().next() == Some(*key))
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
/// Recursive worker for [`ViewFileConfig::inherit_tree_columns`]. Fills a
/// tree-continuation level's empty `columns` from `parent_cols`, then
/// recurses into its own children carrying the nearest non-empty column set
/// as their inheritance source.
fn inherit_columns_into(child: &mut ChildDef, parent_cols: &[ColumnDef]) {
    if child.tree_label.is_some() && child.columns.is_empty() {
        child.columns = parent_cols.to_vec();
    }
    // Descendants inherit from the closest ancestor that actually has
    // columns — this level if it now has them, else keep looking upward.
    let ctx = if child.columns.is_empty() {
        parent_cols.to_vec()
    } else {
        child.columns.clone()
    };
    for grandchild in &mut child.children {
        inherit_columns_into(grandchild, &ctx);
    }
}

/// Action types that are declared once at the tree root and apply tree-wide;
/// they must never be propagated to child levels or the one-level-only
/// validator ([`check_tree`]) would reject the duplicate. Everything else is
/// eligible for inheritance when marked [`ActionDef::inherit`].
fn is_inheritable_action_type(action_type: &str) -> bool {
    !matches!(action_type, "tree_find" | "search" | "fuzzy_filter")
}

/// Recursive worker for [`ViewFileConfig::inherit_tree_actions`]. Copies the
/// parent level's inheritable actions/shortcuts into a tree-continuation
/// `child` (unless the child binds the same key itself), then recurses
/// carrying the child's *effective* inheritable set so entries cascade to
/// every depth.
fn inherit_actions_into(
    child: &mut ChildDef,
    parent_actions: &[ActionDef],
    parent_shortcuts: &HashMap<char, ShortcutDef>,
) {
    if child.tree_label.is_some() {
        // A child's own binding on the same key overrides the inherited one.
        // Event-only actions (no key) are not deduped by key — they inherit
        // as-is (each carries its own event binding via the view).
        let local_keys: HashSet<String> = child
            .actions
            .iter()
            .flat_map(|a| a.key_strings().iter().cloned())
            .collect();
        for action in parent_actions {
            // A parent action is shadowed only if *every* alternative it binds
            // is already claimed locally; a keyless action always inherits.
            let keys = action.key_strings();
            let keep = keys.is_empty() || keys.iter().any(|k| !local_keys.contains(k));
            if keep {
                child.actions.push(action.clone());
            }
        }
        for (key, sc) in parent_shortcuts {
            child.shortcuts.entry(*key).or_insert_with(|| sc.clone());
        }
    }

    // Carry the child's effective inheritable set further down. Inherited
    // entries kept their `inherit` flag, so they reappear here and cascade.
    let next_actions: Vec<ActionDef> = child
        .actions
        .iter()
        .filter(|a| a.inherit && is_inheritable_action_type(&a.action_type))
        .cloned()
        .collect();
    let next_shortcuts: HashMap<char, ShortcutDef> = child
        .shortcuts
        .iter()
        .filter(|(_, sc)| sc.inherit())
        .map(|(k, sc)| (*k, sc.clone()))
        .collect();
    for grandchild in &mut child.children {
        inherit_actions_into(grandchild, &next_actions, &next_shortcuts);
    }
}

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
            errors.push(format!(
                "{scope}: type='{}' requires `id` (e.g. id: create_comment)",
                a.action_type
            ));
        }
        "navigate" if a.navigate_to.is_none() => {
            errors.push(format!("{scope}: type='navigate' requires `navigate_to`"));
        }
        "text_search" if a.text_search.is_none() => {
            errors.push(format!(
                "{scope}: type='text_search' requires `text_search.query_template`"
            ));
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
    /// Optional override for this tab's switch shortcut. Absent → the
    /// positional autonumber digit (`1`..`9`, then `0`); an empty list
    /// (`key: []`) disables the switch key entirely; a scalar or list sets
    /// explicit binding(s), including chords (`key: "ctrl+k t"`). Editable
    /// from the shortcut menu (Ctrl+Y), which writes it back here.
    #[serde(default)]
    pub key: Option<KeyBinding>,
    /// Leading glyph prefixed to this tab's bar label while the view holds
    /// unread items — the tab-bar counterpart of [`ViewDef::unread_marker`],
    /// rendered as `<marker> <icon> <key> <name>`. `None` falls back to the
    /// view's own `unread_marker` (so tree and tab agree by default), then to
    /// the built-in `🔔` — a bell rather than the rows' `💬`, because the tab
    /// already carries an `icon:` a speech balloon would compete with. The
    /// empty string suppresses the glyph and leaves only `unread_style` to
    /// carry the signal.
    ///
    /// Why separate from the view-level marker at all: the tab label is the
    /// only part of a **background** tab that stays on screen, so it may want
    /// a louder or quieter cue than the rows inside — and the tab already
    /// carries an `icon:`, which the marker must stay distinguishable from.
    #[serde(default)]
    pub unread_marker: Option<String>,
    /// How the tab's own label is emphasised while the view holds unread
    /// items. `None` renders the label **bold** — the conventional
    /// "something new here" weight, and the one emphasis that survives on
    /// both the active (already bold, already colored) and the inactive tab
    /// without fighting the bar's own palette.
    ///
    /// Deliberately its own setting rather than reusing [`ViewDef::unread_style`]:
    /// that one recolors *rows*, where color is free to vary; the tab bar
    /// paints active/inactive tabs from the theme, so an unread tab usually
    /// wants a font change, not a hue. See [`TabUnreadStyle`] for the forms.
    #[serde(default)]
    pub unread_style: Option<TabUnreadStyle>,
    /// Where *this* tab's load banner goes, overriding the global
    /// `notifications.load_banner`. `None` takes the global setting.
    ///
    /// Per-tab because the cost of a load is per-tab: a Postgres query over a
    /// slow tunnel is worth watching from another tab (`global`), while a
    /// local task list finishes before the banner is read (`off`), and the
    /// global default cannot be right for both.
    #[serde(default)]
    pub load_banner: Option<crate::config::tui_config::LoadBannerRoute>,
}

/// Emphasis for an unread tab's bar label ([`TabConfig::unread_style`]).
/// Three surface forms, all optional:
///
/// ```yaml
/// unread_style: unread              # theme color name, no font change
/// unread_style: [bold]              # font modifiers, no recolor
/// unread_style: { fg: unread, modifiers: [bold] }
/// ```
///
/// Whatever it resolves to is layered **on top of** the bar's normal
/// active/inactive style, so an unset part keeps the theme's value.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum TabUnreadStyle {
    /// A bare theme color name (`unread`, `accent`, …) — recolor only.
    Color(String),
    /// A bare modifier list (`[bold]`) — font change only.
    Modifiers(Vec<TextModifier>),
    /// Both, either part omittable.
    Detailed {
        #[serde(default)]
        fg: Option<String>,
        #[serde(default)]
        modifiers: Vec<TextModifier>,
    },
}

impl TabUnreadStyle {
    /// The theme color name this style recolors to, if any.
    pub fn fg(&self) -> Option<&str> {
        match self {
            Self::Color(name) => Some(name.as_str()),
            Self::Modifiers(_) => None,
            Self::Detailed { fg, .. } => fg.as_deref(),
        }
    }

    /// The font modifiers this style adds, folded into one ratatui bitset.
    pub fn modifiers(&self) -> ratatui::style::Modifier {
        let list: &[TextModifier] = match self {
            Self::Color(_) => &[],
            Self::Modifiers(m) => m,
            Self::Detailed { modifiers, .. } => modifiers,
        };
        list.iter()
            .fold(ratatui::style::Modifier::empty(), |acc, m| {
                acc | m.to_ratatui()
            })
    }
}

/// A font attribute a [`TabUnreadStyle`] can add. Named after the ratatui
/// modifiers it maps onto; how much of it a terminal honours is up to the
/// terminal (italic and crossed-out are the usual casualties).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextModifier {
    Bold,
    Dim,
    Italic,
    Underlined,
    Reversed,
    CrossedOut,
}

impl TextModifier {
    pub fn to_ratatui(self) -> ratatui::style::Modifier {
        use ratatui::style::Modifier;
        match self {
            Self::Bold => Modifier::BOLD,
            Self::Dim => Modifier::DIM,
            Self::Italic => Modifier::ITALIC,
            Self::Underlined => Modifier::UNDERLINED,
            Self::Reversed => Modifier::REVERSED,
            Self::CrossedOut => Modifier::CROSSED_OUT,
        }
    }
}

/// The `adapter:` block of a view file. Defined once in `not-yet-done-host`
/// (shared with the CLI/Waybar instance resolver) and re-exported here so the
/// schema can't drift between the TUI's full view parser and the host's
/// lightweight resolver. See [`not_yet_done_host::AdapterInstance`] for the
/// field docs.
pub use not_yet_done_host::AdapterInstance as AdapterConfig;

/// A `shortcuts:` map value. Either a bare action name (`d: delete`) or a
/// detailed form that also marks the shortcut inheritable
/// (`s: { action: toggle-tracking, inherit: true }`). An inheritable shortcut
/// propagates to tree-continuation child levels that don't bind the same key —
/// the per-entry counterpart of [`ActionDef::inherit`] for the fire-and-forget
/// `shortcuts:` map. See [`ViewFileConfig::inherit_tree_actions`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum ShortcutDef {
    /// `key: action-name` — bound here only, not inherited.
    Action(String),
    /// `key: { action: ..., inherit: <bool> }`.
    Detailed {
        action: String,
        #[serde(default)]
        inherit: bool,
    },
}

impl ShortcutDef {
    /// The adapter action id this key invokes.
    pub fn action(&self) -> &str {
        match self {
            ShortcutDef::Action(a) => a,
            ShortcutDef::Detailed { action, .. } => action,
        }
    }

    /// Whether this shortcut propagates to tree-continuation child levels.
    pub fn inherit(&self) -> bool {
        matches!(self, ShortcutDef::Detailed { inherit: true, .. })
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
    /// view (subtab navigation). Only honored when at root level. Absent → no
    /// switch key; an empty list (`key: []`) disables it; a scalar or list sets
    /// explicit binding(s), including chords and alternatives
    /// (`key: [n, ctrl+n]`). Mirrors [`TabConfig::key`] so the shortcut menu
    /// (Ctrl+Y) can write list/chord forms here without breaking the parse.
    #[serde(default)]
    pub key: Option<KeyBinding>,
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
    /// Optional card mode for this level: one row rendered as a framed card
    /// whose fields sit in a grid of `columns:` slots per line. Absent → the
    /// level has no card mode. Present → the mode is *available* and is
    /// entered via `card.key` (persisted per level) or `card.default`.
    /// See [`CardConfig`].
    #[serde(default)]
    pub card: Option<CardConfig>,
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
    /// Opt-in for the record-detail split (`o`). When `true`, a pane
    /// showing this level lets the user split off a coupled detail pane
    /// to the right that transposes the *selected* row into a
    /// field-name | field-value table, kept live as the cursor moves.
    /// Off by default because it only makes sense for wide,
    /// schema-rich rows (e.g. Postgres table rows or script-result
    /// rows) where reading one record across many columns is awkward
    /// in the list. See docs/generic-view-spec.md `record_detail:`.
    #[serde(default)]
    pub record_detail: bool,
    /// Opt-in for per-node scripts: the adapter-native query editor (`Q`)
    /// and the per-node scripts menu (`q`) act on the rows of this level.
    ///
    /// This is what makes those two keys generic. The host used to ask
    /// "is the adapter `postgres` and are these `postgres:table` rows?";
    /// now the view declares which level owns scripts and the host only
    /// checks that the adapter offers a
    /// [`ScriptStore`](not_yet_done_content::ScriptStore) and advertises
    /// `supports_node_query_editor`. Off by default, so no existing view
    /// grows the keys by accident.
    ///
    /// The flag belongs on the level whose *items* own the scripts (e.g.
    /// the tables level), not on the level that shows a script's result.
    /// Drilling one step deeper keeps the keys live and addresses the
    /// parent node — editing a table's query while looking at its rows is
    /// the common case.
    #[serde(default)]
    pub node_scripts: bool,
    /// Opt-in for the window/split operations reachable via the `w`
    /// leader chord (split right/down, close pane, focus parent/child,
    /// pane-tag switch). Off by default so `w` stays a free, ordinary key
    /// on every other view; enable it only where multiple panes actually
    /// make sense — a view with a coupled child `split:` (e.g. the Stoat
    /// chat) or one that offers a `record_detail:` split (e.g. Postgres
    /// rows). When `false`, the `w` chord never engages and the key falls
    /// through to normal handling. See docs/generic-view-spec.md
    /// `window_ops:`.
    #[serde(default)]
    pub window_ops: bool,
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
    /// Name of a sibling view (same tab) whose script source this view
    /// should share. When set, both the script *directory*
    /// (`scripts/<tab>/<node_type…>/`) and the DB shortcut *scope*
    /// (`script:<tab>/<node_type…>`) are derived from the referenced
    /// view's root `node_type` instead of this view's own — so two views
    /// (e.g. Jira `tickets` and `bookmarks`) present the same scripts and
    /// script-shortcuts. Only the root segment is swapped; any drilled
    /// child levels keep their own node_types. An unknown name is a
    /// silent no-op (this view keeps its own scope). Generic across
    /// adapters.
    #[serde(default)]
    pub script_source: Option<String>,
    /// Per-node-type shortcuts. Maps a key (single char) to an adapter-
    /// declared action `id` (returned from `Node::actions`). At runtime
    /// the TUI calls `Node::invoke_action(id)` and dispatches the
    /// returned `ActionDispatch`. Action `id`s are validated lazily —
    /// pressing a key bound to an unknown action surfaces an error in
    /// the status bar.
    #[serde(default)]
    pub shortcuts: HashMap<char, ShortcutDef>,
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
    /// Type glyph shown in the tree-mode label column immediately before
    /// the label, for **every** row of this level — expandable or not.
    /// `None` (default) renders no glyph.
    ///
    /// Why, next to `leaf_glyph`: that one encodes the *expand state*
    /// (this row has nothing below it), this one encodes the row's
    /// *kind*. They are independent questions, and a level whose rows are
    /// expandable never gets a `leaf_glyph` at all. Needed wherever two
    /// different node types share one tree depth and would otherwise be
    /// indistinguishable — e.g. the Stoat server level, which lists
    /// uncategorized channels and categories side by side.
    #[serde(default)]
    pub icon: Option<String>,
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
    /// Foreground color for this tree's connector glyphs — the `├──`/`└──`/`│`
    /// box-drawing prefix and the `▶`/`▼` expand arrows drawn in the
    /// `tree_label` column. A theme color name (`text_dim`, `tree_connector`,
    /// `accent`, …; resolved via the same table as `ColumnDef.style`). `None`
    /// falls back to the global theme `tree_connector` color.
    ///
    /// Why per-view: connectors should read as structural scaffolding, quieter
    /// than the labels — but how much quieter depends on the view's own
    /// palette and density. A deep, busy task tree wants dimmer connectors than
    /// a sparse two-level one; a tree drawn on a colored surface needs a
    /// different hue than one on the base background. Making it per-tree lets
    /// each view tune that contrast independently instead of forcing one global
    /// connector color on every tree. Ignored outside tree mode.
    #[serde(default)]
    pub tree_connector_style: Option<String>,
    /// Foreground color for the unread highlight in chat-style adapters
    /// (Stoat): a channel/category whose `unread` metadata is `"true"` paints
    /// its `tree_label` (and the leading `unread_marker`) in this color, and
    /// an unread message paints its multi-line header line the same way. A
    /// theme color name (`unread`, `accent`, …; resolved via the same table
    /// as `ColumnDef.style`). `None` falls back to the global theme `unread`
    /// color.
    ///
    /// Why per-view: unread emphasis competes with the view's own accents
    /// (selection, fuzzy match, group headers); a dense server tree and a
    /// flat message list want it tuned to different contrast. Per-view keeps
    /// that adjustable without a global override. Ignored where no node
    /// carries an `unread` field.
    #[serde(default)]
    pub unread_style: Option<String>,
    /// Leading glyph prefixed to an unread tree row's label (channel/category)
    /// and the unread message header. `None` falls back to the default `💬`
    /// (speech balloon). Set to the empty string to suppress the marker and
    /// rely on `unread_style` color alone.
    ///
    /// Why configurable: the marker is the at-a-glance "something new" cue;
    /// terminals and fonts vary in how they render emoji vs. Nerd-Font glyphs,
    /// and some users prefer a quiet ASCII dot. Note an emoji marker is two
    /// cells wide — the tree indentation accounts for the rendered width.
    #[serde(default)]
    pub unread_marker: Option<String>,
    /// Draw the `├──`/`└──`/`│` box-drawing line connectors in tree mode.
    /// `false` replaces the lines with plain indentation (two spaces per
    /// depth level); the expand markers (see `tree_markers`) and the
    /// optional `leaf_glyph` are unaffected.
    ///
    /// Why: the lines carry sibling/continuation structure, which earns
    /// its visual weight on deep, irregular trees (tasks) but reads as
    /// noise on shallow, regular drills (database → schema → table).
    /// Default `true`. Ignored outside tree mode.
    #[serde(default)]
    pub tree_lines: Option<bool>,
    /// Expand/collapse markers drawn in front of expandable tree rows,
    /// configured independently of the line connectors (`tree_lines`).
    /// `None` = defaults (`▶` collapsed, `▼` expanded, shown). Ignored
    /// outside tree mode.
    #[serde(default)]
    pub tree_markers: Option<TreeMarkerDef>,
    /// Initial expansion depth for tree mode. Rows at depth `d <
    /// expand_depth` are auto-expanded once after the root list (re)loads
    /// — `2` shows three levels (roots, children, grandchildren), mirroring
    /// the native Tasks tab's `tasks.tree.default_expand_depth`; `all`
    /// keeps expanding until no expandable row is left (the whole tree is
    /// always fully open, like the native Trackings tree). The expansion
    /// is a one-shot cascade: after it completes the user's manual
    /// expand/collapse state is never overridden. A new query
    /// (saved-query apply) re-runs the cascade on the filtered tree.
    ///
    /// Why: lazy-loading trees open fully collapsed by default, which is
    /// right for expensive remote adapters (Postgres, Confluence) but
    /// wrong for cheap in-memory forests (tasks) where the user expects
    /// their working set visible immediately. Each level is fetched
    /// through the normal expand path, so the cost is one round of
    /// adapter calls per level — keep the value small (and avoid `all`)
    /// on remote adapters. `0`/unset = no auto-expansion (default).
    /// Ignored outside tree mode.
    #[serde(default)]
    pub expand_depth: Option<ExpandDepth>,
    /// Render the bucket rows of an adapter-grouped tree
    /// (`group_by_via_adapter`) as flat-style `── label` group-header rows
    /// instead of selectable tree nodes: header style, non-selectable, and
    /// the rows beneath lose the extra indentation level the bucket would
    /// otherwise add — the forest starts at indent 0 under each header,
    /// exactly like the engine's flat-list grouping looks.
    ///
    /// Why: a group bucket is an aggregate, not a thing to navigate to —
    /// rendering it as a tree node makes it selectable and pushes the real
    /// rows a level deeper, which reads as a different (and noisier) layout
    /// than the same grouping on a flat view. Only meaningful on a tree
    /// view whose root level is the adapter's group-bucket type; ignored
    /// while grouping is cycled off (the adapter then returns plain rows at
    /// the root). Combine with `expand_depth` — a collapsed bucket cannot
    /// be expanded by cursor (headers are not selectable).
    #[serde(default)]
    pub group_headers: Option<GroupHeadersDef>,
    /// Event → action rules for this view. Each binding runs a named action
    /// (from this view's `actions:`) when a bus event on its `on:` topic
    /// arrives, passing the event's payload/`correlation_id`/`source` to the
    /// rule engine. This is how the app reacts to adapter-published events
    /// (e.g. MFA number-match / OTC prompts) without the old callback
    /// machinery. Empty (default) → the view ignores all bus events.
    #[serde(default)]
    pub event_actions: Vec<EventActionBinding>,
}

/// Value of [`ViewDef::group_headers`]. Presence alone enables the header
/// rendering (`group_headers: {}`).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct GroupHeadersDef {
    /// Optional extra column appended (as the last column) while grouping
    /// is active: each group's total, shown on the group's **closing** row
    /// — the classic time-sheet layout where a Total column closes each
    /// day (same semantics as the flat grouping's `total_column`). A full
    /// `ColumnDef`: `label`/`kind`/`style`/`sizing` render it; `source`
    /// names the **bucket node's** metadata field carrying the total
    /// (falling back to `key`). The column disappears with grouping off.
    #[serde(default)]
    pub total: Option<ColumnDef>,
}

/// Value of [`ViewDef::expand_depth`]: a fixed number of levels, or the
/// string `all` to expand every level until nothing expandable remains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpandDepth {
    /// Auto-expand rows at depth `d < n` (`expand_depth: 2`).
    Levels(u32),
    /// Auto-expand everything (`expand_depth: all`). The cascade
    /// terminates on its own once no row has unexpanded children.
    All,
}

impl<'de> Deserialize<'de> for ExpandDepth {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ExpandDepthVisitor;
        impl serde::de::Visitor<'_> for ExpandDepthVisitor {
            type Value = ExpandDepth;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a non-negative integer or the string \"all\"")
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<ExpandDepth, E> {
                u32::try_from(v)
                    .map(ExpandDepth::Levels)
                    .map_err(|_| E::custom(format!("expand_depth {v} out of range")))
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<ExpandDepth, E> {
                u32::try_from(v)
                    .map(ExpandDepth::Levels)
                    .map_err(|_| E::custom(format!("expand_depth must be >= 0, got {v}")))
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<ExpandDepth, E> {
                if v.eq_ignore_ascii_case("all") {
                    Ok(ExpandDepth::All)
                } else {
                    Err(E::custom(format!(
                        "unknown expand_depth `{v}` (expected a number or `all`)"
                    )))
                }
            }
        }
        deserializer.deserialize_any(ExpandDepthVisitor)
    }
}

/// Expand/collapse marker configuration for tree mode (see
/// [`ViewDef::tree_markers`]). Each field is optional so a config can
/// override just one marker; unset fields keep the defaults.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeMarkerDef {
    /// Show the markers at all. `false` hides them entirely — rows stay
    /// expandable via the usual keys, only the visual cue goes. Default
    /// `true`.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Marker for a collapsed expandable row. Default `▶`.
    #[serde(default)]
    pub collapsed: Option<String>,
    /// Marker for an expanded row. Default `▼`.
    #[serde(default)]
    pub expanded: Option<String>,
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
    /// Scaffold shown in the editor when creating a *new* query, when there
    /// is no active/`default` query to seed from. Unlike `default` this is
    /// never applied as a live filter — it only pre-fills the editor buffer,
    /// so it may be entirely commented-out examples. Falls back to `default`
    /// when absent.
    #[serde(default, deserialize_with = "deserialize_optional_query_source")]
    pub template: Option<String>,
    #[serde(default)]
    pub editable: bool,
    /// Key to open the query menu popup (e.g. "q").
    #[serde(default)]
    pub menu_key: Option<String>,
    /// Apply the tab's user-set default saved query (★ in the query menu)
    /// to this view too. By default the startup apply only stamps the
    /// tab's default view — usually right, because sibling views show
    /// *different* data where the query means something else. Set this on
    /// views that are mere projections of the same rows (e.g. the
    /// Trackings condensed/tree subtabs) so the default filter follows
    /// the user across subtabs.
    #[serde(default)]
    pub inherit_default: bool,
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ColumnDef {
    pub key: String,
    #[serde(default)]
    pub label: Option<String>,
    /// "label" = use node.label(), otherwise metadata key.
    #[serde(default)]
    pub source: Option<String>,
    /// Metadata field this cell reads **instead of** `source`/`key` while the
    /// row is a *collapsed* tree node (has children, not expanded). Lets a
    /// marker roll a hidden-descendant state up onto the parent that hides it
    /// — e.g. the Tasks tree's `tracking` column points `collapsed_source` at
    /// the adapter's subtree-rollup field so a collapsed node shows `⏱` when a
    /// tracking it hides is running, while an expanded node keeps showing only
    /// its own marker. Ignored for expanded nodes, leaves, and the label cell.
    #[serde(default)]
    pub collapsed_source: Option<String>,
    /// Metadata field this cell reads **instead of** `source`/`key` while the
    /// pane's long-text mode is on (`v`). The full field is soft-wrapped to
    /// the column width and rendered as a multi-line block, growing the row
    /// vertically; every other column stays a single fitted line. Unlike
    /// `markdown` (a permanent multi-line chat layout that forbids grouping),
    /// this is a runtime toggle that composes with day grouping, totals and
    /// the header — they render unchanged. `None` (default) opts the column
    /// out, so the `toggle_long_text` key stays free on that view. Typical
    /// use: a `source: label` column (clean first line) with
    /// `long_source: description` (the full body on demand).
    #[serde(default)]
    pub long_source: Option<String>,
    /// Theme color reference (e.g. "accent", "text_med", "success").
    #[serde(default)]
    pub style: Option<String>,
    /// "max" (default), "fixed(N)", "flex(N)", "fit", "auto" or
    /// "auto(min,max)". See docs/generic-view-spec.md `sizing:`.
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
    /// When `true`, this column is omitted from the default rendered set —
    /// it exists in the view's column list (so the `c` column-config popup
    /// can offer it) but is not shown until the user explicitly enables it
    /// there. Use for columns that are occasionally useful but clutter the
    /// default layout (e.g. the Tasks tree's `tag_names`). The tree-label
    /// column is never hidden by this flag. Default `false` (shown).
    #[serde(default)]
    pub hidden: bool,
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

/// Ordering of the groups themselves (not the rows inside a group, which
/// keep the adapter's order). Group labels are built ISO-sortable (see
/// `views::group_aggregate`), so lexical order equals chronological order
/// for date buckets — `desc` therefore puts the newest bucket first, which
/// is what a time-tracking log wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum GroupOrder {
    #[default]
    Asc,
    Desc,
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
    /// Order of the groups in the rendered table. Default ascending.
    #[serde(default)]
    pub order: GroupOrder,
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
    /// When set, the group total moves out of the group-header row into this
    /// dedicated column, written on the *last* data row of each outermost
    /// group (and into the same column on the Σ grand-total footer). The
    /// target column is hidden while grouping is off, since a per-group
    /// total has no meaning in a flat list. This mirrors classic
    /// time-sheet layouts where a running "Total" column closes each day.
    #[serde(default)]
    pub total_column: Option<String>,
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
// Card mode
// ---------------------------------------------------------------------------

/// One field slot of a card layout (see [`CardConfig::fields`]).
///
/// Deserializes from either a bare column key (`author`) — the label then
/// comes from that column's own `label:` — or a map with an explicit
/// override (`{ column: author, label: "By" }`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardFieldDef {
    /// Column `key` this slot reads. Must be declared in the level's
    /// `columns:` (enforced by the validator).
    pub column: String,
    /// Label shown for the value. `None` → the column's `label:`, falling
    /// back to its `key`.
    pub label: Option<String>,
}

impl<'de> Deserialize<'de> for CardFieldDef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            // Shorthand: just the column key.
            Short(String),
            // Full form with an explicit label override.
            Full {
                column: String,
                #[serde(default)]
                label: Option<String>,
            },
        }
        Ok(match Raw::deserialize(deserializer)? {
            Raw::Short(column) => CardFieldDef {
                column,
                label: None,
            },
            Raw::Full { column, label } => CardFieldDef { column, label },
        })
    }
}

/// Where a field's label is drawn relative to its value. Mirrors
/// `not_yet_done_table::CardLabels` on the config side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardLabelMode {
    /// Values only, no labels.
    None,
    /// `Label: value` on one line. The default.
    #[default]
    Inline,
    /// Labels on their own line above the values — each grid row of the card
    /// becomes two physical lines.
    Above,
}

/// Card frame style. Mirrors `not_yet_done_table::CardBorder`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CardBorderMode {
    /// No frame; cards are separated by `gap:` alone.
    None,
    /// Square corners.
    Plain,
    /// Rounded corners. The default.
    #[default]
    Rounded,
}

/// Card mode for a level (see [`ViewDef::card`]).
///
/// Card mode is the table's alternative rendering: one row becomes one
/// framed card whose fields are laid out in a grid of `columns:` slots per
/// line. The number of card lines is **derived** — `fields ÷ columns`,
/// rounded up — so listing six fields at `columns: 3` yields a two-line
/// card. Trailing slots of the last line stay blank so every card keeps the
/// same height.
///
/// Declaring `card:` does not switch the level over; it makes the mode
/// *available*. `key:` toggles it at runtime (and the choice survives a
/// restart), `default: true` opens the level in card mode.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardConfig {
    /// Fields in reading order, filled row-major into the grid. Every entry
    /// must name a column declared in this level's `columns:`.
    ///
    /// Omit the key to show **every** column of the level, in its configured
    /// order — the card then tracks the table instead of having to repeat its
    /// column list (and a column added later shows up on its own).
    /// `markdown: true` columns are skipped, since those cannot sit in a
    /// fixed-height grid slot.
    #[serde(default)]
    pub fields: Vec<CardFieldDef>,
    /// Fields side by side on one card line. Default `1` (one field per
    /// line); `3` with six fields gives a 2×3 card.
    #[serde(default = "default_card_columns")]
    pub columns: usize,
    /// Per-grid-column width weights. Empty → equal shares; otherwise the
    /// length must match `columns:`. `[1, 1, 2]` gives the third slot half
    /// the card's inner width.
    #[serde(default)]
    pub weights: Vec<usize>,
    #[serde(default)]
    pub labels: CardLabelMode,
    #[serde(default)]
    pub border: CardBorderMode,
    /// Theme color for the frame glyphs. `None` → the theme's `card_border`.
    #[serde(default)]
    pub border_style: Option<String>,
    /// Theme color for field labels. `None` → the theme's `card_label`.
    #[serde(default)]
    pub label_style: Option<String>,
    /// Blank columns between frame and content, left and right. Default `1`.
    #[serde(default = "default_card_padding")]
    pub padding: usize,
    /// Blank lines after each card. They never take the selection highlight,
    /// so cards read as separate blocks. Default `0`.
    #[serde(default)]
    pub gap: usize,
    /// Filler between two grid slots. Default two spaces.
    #[serde(default = "default_card_separator")]
    pub separator: String,
    /// Rule drawn *between* two cards, the glyph repeated across the card
    /// width (`divider: "─"`). Empty (the default) → the cards are separated by
    /// `gap:` alone. Takes the place of the last `gap:` line, so `gap: 1` plus
    /// a divider is one ruled line rather than a blank one plus a rule; never
    /// drawn after the last card. Not to be confused with `separator:`, which
    /// fills the space between two slots *within* a card line.
    #[serde(default)]
    pub divider: String,
    /// Key that toggles card mode on this level. Absent → the mode is only
    /// reachable via `default: true` (no key is stolen). Accepts the same
    /// forms as every other binding, including alternatives (`key: [C, ctrl+d]`).
    #[serde(default)]
    pub key: Option<KeyBinding>,
    /// Open this level in card mode. A stored per-level choice (the toggle
    /// key) wins over this default once the user has flipped the mode.
    #[serde(default)]
    pub default: bool,
}

fn default_card_columns() -> usize {
    1
}

fn default_card_padding() -> usize {
    1
}

fn default_card_separator() -> String {
    "  ".to_string()
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

fn default_true() -> bool {
    true
}
fn default_content_source() -> String {
    "content".to_string()
}
fn default_split() -> String {
    "horizontal".to_string()
}
fn default_ratio() -> u16 {
    50
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// What a `type: script` action hands to the chosen script.
///
/// The default ([`ScriptScope::Node`]) passes the single selected row as the
/// uniform `{"node": …}` payload. [`ScriptScope::FilteredSet`] instead passes
/// the **whole currently-filtered row set plus the active query's date bounds**
/// as the legacy batch payload `{"tracking_ids": […], "filter_min_date": …,
/// "filter_max_date": …}` — for aggregate scripts (daily reports, period
/// equalizers) that operate over the entire filtered list, not one row. The
/// `tracking_ids` key is retained verbatim so the historical scripts run
/// unchanged; the mechanism itself is adapter-agnostic.
/// [`ScriptScope::Table`] passes the **whole currently-displayed table** as a
/// richer payload — `{"rows": [{id, label, fields}…], "query": …,
/// "selected_index": …, "selected_field": …}` — so a script can act on the
/// visible rows *with their cursor context* (which row, which column). It works
/// on any content table, including the transposed record-detail split (where
/// each "row" is a field/value pair). `selected_field` is the key under the
/// column cursor, or the action's `default_field` when the column cursor is off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptScope {
    #[default]
    Node,
    FilteredSet,
    Table,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionDef {
    pub name: String,
    /// Keyboard binding(s). `None` for an **event-only** action: one that is
    /// never bound to a key and only runs when the rule engine matches a
    /// bus event to it via the view's `event_actions:`. A keyed action may
    /// additionally be event-triggered — the two triggers are independent.
    ///
    /// Accepts a scalar or a list in YAML (same convenience as the `tui.yaml`
    /// [`KeyBinding`] sections): `key: e` or `key: [e, ctrl+k l]`. A list is
    /// alternatives (any one triggers); a space inside a single string is a
    /// step separator for a chord sequence.
    #[serde(default)]
    pub key: Option<KeyBinding>,
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
    /// Force this action into the action bar (top bar) even though its
    /// `type` is otherwise status-bar-only (`custom`). Use for a `custom`
    /// action that opens a *modal flow* — a picker followed by an editor,
    /// e.g. Taiga `convert` — so it reads as an ongoing operation and lights
    /// up while its picker popup or resulting editor is open. Requires an
    /// `id` (the active-state resolver keys on it). Default `false`.
    #[serde(default)]
    pub in_action_bar: bool,
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
    /// Propagate this action down to tree-continuation child levels
    /// (`tree_label` set) that don't bind the same key themselves. Lets a
    /// recursive tree (e.g. the task forest) declare its per-row actions once
    /// at the tree root instead of repeating the identical block at every
    /// depth — the inheritance pass ([`ViewFileConfig::inherit_tree_actions`])
    /// copies inheritable actions into each child before validation. An
    /// inherited action keeps its `inherit` flag, so it cascades to every
    /// depth. A child that declares its own action on the same key overrides
    /// the inherited one. The single-level search family (`tree_find`,
    /// `search`, `fuzzy_filter`) is never inherited — those are declared once
    /// at the tree root and already apply tree-wide. Default `false`.
    #[serde(default)]
    pub inherit: bool,
    /// For `type: script` actions: which payload the script receives.
    /// `node` (default) hands over the single selected row; `filtered_set`
    /// hands over the whole filtered row set + the active query's date
    /// bounds (see [`ScriptScope`]). Ignored by non-script actions.
    #[serde(default, rename = "scope")]
    pub script_scope: ScriptScope,
    /// For `type: script` with `scope: table`: the field key reported as
    /// `selected_field` when the column cursor is off (no column highlighted).
    /// `None` reports `selected_field: null` in that case. Ignored otherwise.
    #[serde(default)]
    pub script_default_field: Option<String>,
    /// Invoke this action on the pane's *container* (the adapter `root()`)
    /// instead of the selected row. Built for actions that operate on the
    /// whole list/level and must stay reachable even at the un-drilled flat
    /// root, where there is no selected row and no `parent:` shortcut target
    /// to address — e.g. trackings `restore all`. Unlike a `shortcuts:`
    /// `parent:`-prefixed entry (which resolves its target from the nav
    /// stack and so disappears at the empty root), an `on_container` action
    /// builds its hint statically and dispatches through `adapter.root()`.
    /// Today only `type: custom` honours this flag. Default `false`.
    #[serde(default)]
    pub on_container: bool,
    /// For `type: option_menu` actions — the menu definition (options
    /// source, the node's selection-marker field, and the adapter action
    /// invoked on toggle). The whole menu is host-side and config-driven;
    /// the adapter only sees `list_values(source)` and an `invoke_action`
    /// with the chosen value. Required for `option_menu`, ignored otherwise.
    #[serde(default)]
    pub option_menu: Option<OptionMenuConfig>,
    /// Deliberately take over a key that a built-in handler (a global
    /// hotkey, a common fallback like `c` column-config / `S` sort / `:`
    /// command-line, a window-leader chord, or a content action) would
    /// otherwise claim. Binding such a key without `force` is a hard
    /// config error (the keymap validator reports the collision at
    /// startup), because the YAML action silently shadows the built-in at
    /// runtime and the built-in becomes unreachable on that leaf. Setting
    /// `force: true` is the explicit opt-in: the validator drops the
    /// built-in's claim for this key at this leaf so the action wins
    /// cleanly. Only suppresses conflicts against *built-in* claims —
    /// two YAML actions fighting over one key is still an error. Default
    /// `false`.
    #[serde(default)]
    pub force: bool,
    /// Publish a bus event when this action completes. Built for the
    /// request/response half of the MFA flow: an OTC-input action answers
    /// the backend's `otc-required` request by emitting the typed code. The
    /// emitted event's `correlation_id` is copied from the event that
    /// triggered this action (when it ran in response to one), so a
    /// request and its reply match up even with two connections in flight.
    #[serde(default)]
    pub emit: Option<EmitConfig>,
    /// React to bus events while this action's UI (popup / input field) is
    /// open. Maps a bus topic to a reaction; for now only `close` exists
    /// (dismiss this action's popup when the topic fires, scoped to the
    /// same `source`/`correlation_id` as the event that opened it). E.g. a
    /// number-match notification closes itself when the backend publishes
    /// `…:mfa:resolved`.
    #[serde(default)]
    pub on_event: Option<HashMap<String, OnEventReaction>>,
    /// For `type: notify` actions: the message to show in the notification
    /// bar. Placeholders `{field}` are replaced with the matching key from the
    /// triggering bus event's JSON payload (e.g. `"Authenticator: tap number
    /// {number}"`). A notify action is normally event-only (no `key`) and, if
    /// it declares `on_event: { <topic>: close }`, its message is retracted
    /// when that topic fires for the same `source`.
    #[serde(default)]
    pub message: Option<String>,
    /// For `type: notify` actions: render the message in the prominent top
    /// alert bar (loud `alert_fg`/`alert_bg` colours) instead of the ordinary
    /// bottom notification bar. Use for messages that must not be missed — e.g.
    /// the Microsoft Authenticator number to tap during an interactive sign-in.
    /// Honoured only while `notifications.alert_enabled` is set; otherwise the
    /// message falls back to the bottom bar. Default `false`.
    #[serde(default)]
    pub prominent: bool,
    /// For `edit`/`create`/`custom` actions that open an
    /// [`InputSpec::Form`](not_yet_done_content::InputSpec) popup: per-action
    /// overrides for the form's **layout and behaviour** (column count,
    /// explicit column assignment, whether the focused field draws a filled
    /// bar, inline-radio vs dropdown selects). Colours live in the theme's
    /// `form:` block, not here. Absent → the global `form_defaults` (else the
    /// classic single-column, no-bar, dropdown look). Ignored by actions that
    /// don't open a form.
    #[serde(default)]
    pub form: Option<ActionFormConfig>,
}

/// Per-action layout/behaviour overrides for an [`InputSpec::Form`] popup.
/// Every field is optional; an absent field falls back to the global
/// `form_defaults` and finally to the driver's classic defaults (1 column,
/// no field bar, dropdown selects). See [`ActionDef::form`].
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionFormConfig {
    /// Number of columns to lay the fields out in (`1` or `2`; other values
    /// are clamped by the driver). `None` → default.
    #[serde(default)]
    pub columns: Option<u8>,
    /// Explicit per-column field assignment as lists of field **keys**
    /// (matching [`FormFieldSpec::key`](not_yet_done_content::FormFieldSpec)).
    /// The outer vec is the columns (left-to-right), each inner vec the keys
    /// placed in that column, top-to-bottom. Keys not listed keep spec order
    /// in the first column. `None` → the driver auto-balances by height.
    #[serde(default)]
    pub column_assignment: Option<Vec<Vec<String>>>,
    /// Draw a filled bar behind the focused field (needs `form.field_bg` set
    /// in the theme to be visible). `None` → default (off).
    #[serde(default)]
    pub field_bar: Option<bool>,
    /// Render selects as an inline radio list (`inline`) or a collapsed
    /// dropdown (`dropdown`). `None` → default (dropdown).
    #[serde(default)]
    pub select_style: Option<SelectStyleConfig>,
}

/// How a form's select fields are rendered. Mirrors
/// [`not_yet_done_ratatui::SelectStyle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SelectStyleConfig {
    /// Inline radio list, every option always visible.
    Inline,
    /// Collapsed dropdown showing only the current value.
    Dropdown,
}

impl ActionDef {
    /// Every key string bound to this action (empty for an event-only
    /// action). Flattens the alternatives of the [`KeyBinding`] so callers
    /// that reason per-key (conflict/inheritance dedup, forced-key stripping)
    /// don't have to know the scalar-or-list shape.
    pub fn key_strings(&self) -> &[String] {
        match &self.key {
            Some(b) => &b.0,
            None => &[],
        }
    }

    /// Whether this action is bound to at least one key.
    pub fn has_key(&self) -> bool {
        self.key.as_ref().is_some_and(|b| !b.0.is_empty())
    }

    /// The representative key for single-slot displays (action/status bar
    /// hints show one key). First alternative, or `None` when unbound.
    pub fn primary_key(&self) -> Option<&str> {
        self.key
            .as_ref()
            .and_then(|b| b.0.first())
            .map(|s| s.as_str())
    }

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
        // A container action (e.g. `restore all`) is shown even though its
        // type (`custom`) is otherwise status-bar-only: it operates on the
        // whole level and would have no other visible affordance at the
        // flat root.
        if self.on_container {
            return true;
        }
        // A `custom` action can opt into the bar when it opens a modal flow
        // (picker → editor), e.g. Taiga `convert`.
        if self.in_action_bar {
            return true;
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
                | "option_menu"
        )
    }
}

/// Value of [`ActionDef::emit`] — a bus event published when the action
/// completes. The `correlation_id` is not configured here; the rule engine
/// copies it from the triggering event so a reply matches its request.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmitConfig {
    /// Topic of the emitted event (e.g. `office365-web:mfa:otc-provided`).
    pub topic: String,
    /// Static payload for the event. Values may use `{…}` templates
    /// resolved from the action's input at emit time (e.g. `{code}` /
    /// `{form.code}`), mirroring the `text_search` `{q}` convention. An
    /// empty payload (`{}`) is valid — a bare acknowledgement.
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// Value of an [`ActionDef::on_event`] entry — what happens when the mapped
/// bus topic fires while the action's UI is open. Deliberately a closed enum
/// so unknown reactions are a config error; today only `close` is wired up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnEventReaction {
    /// Dismiss this action's popup / input field.
    Close,
}

/// One entry of a view's `event_actions:` list — a rule that runs an action
/// in response to a bus event. See [`ViewDef::event_actions`].
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventActionBinding {
    /// Bus topic to match (e.g. `office365-web:mfa:number-match`).
    pub on: String,
    /// Name of the action (in this view's `actions:`) to run when the topic
    /// fires. The event's payload is exposed to the action as `{…}` template
    /// values and its `correlation_id`/`source` are carried through.
    pub run: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FuzzyFilterConfig {
    /// Which metadata fields to search. If empty or absent, searches all fields + label.
    #[serde(default)]
    pub fields: Vec<String>,
}

/// Config for a `type: option_menu` action — a host-side, adapter-agnostic
/// popup that toggles values on the selected node (e.g. tags).
///
/// Why it exists: an action coupled to a GUI shape (a picker, a form) forces
/// the adapter to know about the host. Instead the adapter exposes a flat list
/// of selectable values via `list_values(source)` and accepts a chosen value
/// through a normal `invoke_action` (`ActionContext.value`). This config is the
/// host's recipe for wiring those two adapter calls into one searchable menu —
/// the adapter knows nothing about it.
#[derive(Debug, Clone, Deserialize)]
pub struct OptionMenuConfig {
    /// Source key passed to [`not_yet_done_content::ContentAdapter::list_values`]
    /// to fetch the selectable options (e.g. `"tags"`). The adapter maps it to
    /// a `Vec<ValueOption>` (stable value id + display label).
    pub source: String,
    /// Hidden node metadata field holding the currently-selected option values
    /// (comma-separated stable ids, e.g. `tag_ids`). Read to pre-mark the
    /// assigned options when the menu opens.
    pub marker: String,
    /// Adapter action id invoked on toggle (e.g. `"toggle-tag"`). The chosen
    /// option's value is handed over in `ActionContext.value`; the adapter
    /// decides assign-vs-unassign from the node's current membership.
    pub toggle: String,
    /// Adapter action id invoked to *create* a new option (e.g. `"create-tag"`).
    /// The menu prompts for a line of text and hands it over in
    /// `ActionContext.text`. Absent → the create binding (and its key) is
    /// disabled. After a successful create the host re-fetches the option list.
    #[serde(default)]
    pub create: Option<String>,
    /// Adapter action id invoked to *rename* the focused option (e.g.
    /// `"rename-tag"`). The menu prompts for the new name (pre-filled with the
    /// current label); the focused option's id goes in `ActionContext.value`
    /// and the typed name in `ActionContext.text`. Absent → disabled.
    #[serde(default)]
    pub rename: Option<String>,
    /// Adapter action id invoked to *delete* the focused option (e.g.
    /// `"delete-tag"`). The menu confirms first, then hands the focused option's
    /// id over in `ActionContext.value`. Absent → disabled.
    #[serde(default)]
    pub delete: Option<String>,
    /// Popup title. Falls back to the action's name when absent.
    #[serde(default)]
    pub title: Option<String>,
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
    /// Card mode for this drill level. Same semantics as [`ViewDef::card`].
    #[serde(default)]
    pub card: Option<CardConfig>,
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
    /// Opt-in for the record-detail split (`o`) when drilling into this
    /// child. Same semantics as [`ViewDef::record_detail`] — applied to
    /// the pane that displays this child's items (e.g. the Postgres
    /// `Rows` / `DB Script Result` levels, which are reached via drill).
    #[serde(default)]
    pub record_detail: bool,
    /// Opt-in for per-node scripts on this child's items. Same semantics
    /// as [`ViewDef::node_scripts`] — set it on the level whose rows own
    /// the scripts (e.g. the Postgres `Tables` child under a schema).
    #[serde(default)]
    pub node_scripts: bool,
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
    pub shortcuts: HashMap<char, ShortcutDef>,
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
    /// Per-ChildDef type glyph, drawn before the label on every row of
    /// this level. See [`ViewDef::icon`] for the semantics; this field
    /// applies when the entry's level is reached via this ChildDef —
    /// which is what makes two sibling branches at the same tree depth
    /// (Stoat: channels vs. categories) tell themselves apart.
    #[serde(default)]
    pub icon: Option<String>,
    /// Default grouping for this drill level (M3). Same semantics as
    /// [`ViewDef::group_by`]; applies to the pane that displays this
    /// child's items. Runtime-switchable. Ignored in tree mode.
    #[serde(default)]
    pub group_by: Option<GroupBy>,
    /// Per-column aggregations for this drill level (M3). Same semantics as
    /// [`ViewDef::aggregates`].
    #[serde(default)]
    pub aggregates: Vec<AggregateDef>,
    /// When the selection moves onto the **last** row of this (flat) drill
    /// level and that row is still unread (its `unread` metadata is
    /// `"true"`), the engine invokes the named `Node::invoke_action` on the
    /// selected row exactly once. Used by the Stoat `messages` level
    /// (`mark_read_on_reach_end: mark-read`) so scrolling to the newest
    /// message acknowledges the channel — clearing the unread highlight
    /// without an explicit keypress.
    ///
    /// Why a generic config hook rather than Stoat-specific wiring: "reached
    /// the end of the list" is a view-level event, and acking is just an
    /// adapter action keyed by id — keeping the trigger in the engine lets
    /// any chat-/feed-style adapter opt in by naming an action, with no
    /// frontend code change. The unread gate makes it idempotent: after the
    /// ack the row reloads as read, so the hook does not re-fire (and it
    /// never fires for an already-read list). `None` disables it. Ignored in
    /// tree mode.
    #[serde(default)]
    pub mark_read_on_reach_end: Option<String>,
    /// Where the cursor lands when this drill level is **opened** — a drill
    /// into a row, or a coupled pane's hot-replace — as soon as its items
    /// arrive. `None` keeps the historical behaviour (the first row).
    ///
    /// Only the *opening* applies it: a reload of an already-open level
    /// (`r`, a live invalidation, a page change) leaves the cursor where the
    /// user put it. Chat levels set `first_unread` so opening a channel lands
    /// on the oldest message not yet seen. Like [`Self::mark_read_on_reach_end`]
    /// this is a generic engine hook reading the same `unread` metadata
    /// field, so any feed-style adapter opts in without frontend code.
    /// Ignored in tree mode (an expansion is not a fresh list).
    #[serde(default)]
    pub cursor_on_open: Option<CursorOnOpen>,
}

/// Initial cursor placement for a freshly-opened drill level
/// ([`ChildDef::cursor_on_open`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorOnOpen {
    /// The first (top) row — the engine's default, spelled out.
    First,
    /// The last (bottom) row. On an oldest-first chat page that is the
    /// newest message.
    Last,
    /// The first row whose `unread` metadata field is `"true"`, anchored at
    /// the **top** edge so the whole unread run reads downward from the
    /// cursor. Falls back to [`Last`](Self::Last) when nothing is unread —
    /// with no news to catch up on, the newest row is what the user came for.
    FirstUnread,
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

    /// Parse just a `tab:` block (the rest of the file is the smallest
    /// thing that still deserializes).
    fn parse_tab(tab_yaml: &str) -> TabConfig {
        let yaml = format!("{tab_yaml}\nadapter:\n  type: mock\n");
        serde_yaml::from_str::<ViewFileConfig>(&yaml)
            .expect("view file parses")
            .tab
    }

    #[test]
    fn tab_unread_defaults_to_absent() {
        let tab = parse_tab("tab:\n  name: Stoat\n");
        assert!(tab.unread_marker.is_none());
        assert!(tab.unread_style.is_none());
    }

    #[test]
    fn tab_unread_style_accepts_color_modifiers_and_both() {
        // A bare theme color name.
        let tab = parse_tab("tab:\n  name: Stoat\n  unread_style: unread\n");
        let style = tab.unread_style.unwrap();
        assert_eq!(style.fg(), Some("unread"));
        assert!(style.modifiers().is_empty());

        // A bare modifier list — the "only the font changes" case.
        let tab = parse_tab("tab:\n  name: Stoat\n  unread_style: [bold]\n");
        let style = tab.unread_style.unwrap();
        assert_eq!(style.fg(), None);
        assert!(style.modifiers().contains(ratatui::style::Modifier::BOLD));

        // Both, and more than one modifier folds into the bitset.
        let tab = parse_tab(
            "tab:\n  name: Stoat\n  unread_marker: \"🔔\"\n\
             \x20 unread_style:\n    fg: accent\n    modifiers: [bold, underlined]\n",
        );
        assert_eq!(tab.unread_marker.as_deref(), Some("🔔"));
        let style = tab.unread_style.unwrap();
        assert_eq!(style.fg(), Some("accent"));
        let m = style.modifiers();
        assert!(m.contains(ratatui::style::Modifier::BOLD));
        assert!(m.contains(ratatui::style::Modifier::UNDERLINED));
    }

    #[test]
    fn child_cursor_on_open_parses_snake_case_and_defaults_to_none() {
        let parse = |line: &str| -> Option<CursorOnOpen> {
            let yaml = format!(
                "tab:\n  name: T\nadapter:\n  type: mock\nviews:\n  - name: v\n    \
                 node_type: \"mock:a\"\n    columns:\n      - {{ key: name }}\n    \
                 children:\n      - name: messages\n        node_type: \"mock:msg\"\n        \
                 columns:\n          - {{ key: name }}\n{line}"
            );
            serde_yaml::from_str::<ViewFileConfig>(&yaml)
                .expect("view file parses")
                .views[0]
                .children[0]
                .cursor_on_open
        };
        assert_eq!(parse(""), None, "unset → the engine's own default");
        assert_eq!(
            parse("        cursor_on_open: first_unread\n"),
            Some(CursorOnOpen::FirstUnread)
        );
        assert_eq!(
            parse("        cursor_on_open: last\n"),
            Some(CursorOnOpen::Last)
        );
        assert_eq!(
            parse("        cursor_on_open: first\n"),
            Some(CursorOnOpen::First)
        );
    }

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
        assert_eq!(view.actions[0].primary_key(), Some("e")); // scalar key form
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

    /// `ActionDef.key` accepts the same scalar-or-list convenience as the
    /// `tui.yaml` keybinding sections: a bare scalar is one binding; a list
    /// is a set of alternatives (any one triggers). A space inside a single
    /// string stays a chord-step separator (Phase 1 dispatch), not a split
    /// into alternatives.
    #[test]
    fn action_key_accepts_scalar_and_list() {
        let yaml = r#"
tab:
  name: T
adapter:
  type: demo
views:
  - name: v
    node_type: "demo:x"
    columns:
      - key: a
    actions:
      - name: scalar
        key: e
        type: reload
      - name: alternatives
        key: [a, "ctrl+k l"]
        type: reload
      - name: eventonly
        type: reload
"#;
        let config: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let actions = &config.views[0].actions;

        // Scalar → single binding.
        assert_eq!(actions[0].key_strings(), &["e".to_string()]);
        assert_eq!(actions[0].primary_key(), Some("e"));
        assert!(actions[0].has_key());

        // List → two alternatives, the chord string kept whole.
        assert_eq!(
            actions[1].key_strings(),
            &["a".to_string(), "ctrl+k l".to_string()]
        );
        assert_eq!(actions[1].primary_key(), Some("a"));

        // No key → event-only, empty inventory.
        assert!(actions[2].key_strings().is_empty());
        assert!(!actions[2].has_key());
        assert_eq!(actions[2].primary_key(), None);
    }

    /// The committed calendar example ships a `create` action carrying the
    /// new per-action `form:` block (2 columns, field bar, inline selects).
    /// Parse the real file so a typo or schema drift is caught here rather
    /// than only at runtime when the user copies it.
    #[test]
    fn committed_calendar_example_form_config_parses() {
        let yaml = include_str!("../../../docs/examples/views/calendar.yaml");
        let config: ViewFileConfig =
            serde_yaml::from_str(yaml).expect("committed calendar.yaml must parse");
        let create = config
            .views
            .iter()
            .flat_map(|v| &v.actions)
            .find(|a| a.id.as_deref() == Some("create"))
            .expect("calendar create action present");
        let form = create
            .form
            .as_ref()
            .expect("create action carries form config");
        assert_eq!(form.columns, Some(2));
        assert_eq!(form.field_bar, Some(true));
        assert_eq!(form.select_style, Some(SelectStyleConfig::Inline));
        let cols = form.column_assignment.as_ref().expect("explicit columns");
        assert_eq!(cols.len(), 2);
    }

    /// The shipped `stoat.yaml` is what a user copies, so a typo or schema
    /// drift in it must fail here rather than in their terminal. Parse and
    /// fully validate the real file, then pin the two levels that are easy
    /// to get wrong: the channel's `attach` (a FilePicker action, hence
    /// `custom`) and the `stoat:attachment` list below a message.
    #[test]
    fn committed_stoat_example_parses_and_validates() {
        let yaml = include_str!("../../../docs/examples/views/stoat.yaml");
        let cfg: ViewFileConfig =
            serde_yaml::from_str(yaml).expect("committed stoat.yaml must parse");
        // The example binds the `compose-below` editor profile, which the
        // validator only accepts when `tui.yaml` defines it.
        let editors: crate::config::editor::EditorsConfig =
            serde_yaml::from_str("default: {}\ncompose-below: {}").unwrap();
        cfg.validate(&KeyBindingConfig::default(), &editors)
            .expect("committed stoat.yaml must validate");

        // Both channel branches (uncategorized and inside a category) offer
        // the upload; a file picker routes through the custom pipeline.
        let channels: Vec<&ChildDef> = collect_children(&cfg.views[0].children)
            .into_iter()
            .filter(|c| c.node_type == "stoat:channel")
            .collect();
        assert_eq!(channels.len(), 2, "uncategorized + categorized channels");
        for channel in &channels {
            let attach = channel
                .actions
                .iter()
                .find(|a| a.id.as_deref() == Some("attach"))
                .expect("channel offers attach");
            assert_eq!(attach.action_type, "custom");
            assert!(
                attach
                    .key
                    .as_ref()
                    .is_some_and(|k| k.matches_sequence(&["A".to_string()])),
                "attach is bound to Shift+A"
            );
        }

        // Every message level drills into its files.
        let messages: Vec<&ChildDef> = collect_children(&cfg.views[0].children)
            .into_iter()
            .filter(|c| c.node_type == "stoat:message")
            .collect();
        assert_eq!(messages.len(), 2);
        for message in &messages {
            let files = message
                .children
                .iter()
                .find(|c| c.node_type == "stoat:attachment")
                .expect("message drills into its attachments");
            // Files are leaves: no tree_label, so Enter drills instead of
            // expanding, and the two actions the adapter exposes are bound.
            assert!(files.tree_label.is_none());
            for id in ["open", "download_all"] {
                assert!(
                    files.actions.iter().any(|a| a.id.as_deref() == Some(id)),
                    "attachment level binds `{id}`"
                );
            }
            assert!(files.columns.iter().any(|c| c.key == "filename"));
        }
    }

    /// Flatten a child tree into one list (depth-first), so a test can look
    /// for a node type without walking the nesting by hand.
    fn collect_children(children: &[ChildDef]) -> Vec<&ChildDef> {
        let mut out = Vec::new();
        for child in children {
            out.push(child);
            out.extend(collect_children(&child.children));
        }
        out
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
    fn parse_expand_depth_number_and_all() {
        let yaml = r#"
tab:
  name: T
adapter:
  type: x
views:
  - name: levels
    node_type: t
    expand_depth: 2
  - name: full
    node_type: t
    expand_depth: all
"#;
        let config: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.views[0].expand_depth, Some(ExpandDepth::Levels(2)));
        assert_eq!(config.views[1].expand_depth, Some(ExpandDepth::All));

        let bad = yaml.replace("expand_depth: all", "expand_depth: everything");
        let err = serde_yaml::from_str::<ViewFileConfig>(&bad).unwrap_err();
        assert!(err.to_string().contains("expand_depth"), "{err}");
    }

    #[test]
    fn parse_single_level_grouping() {
        // Engine grouping is single-level: a `group_by` + aggregates, no
        // nested `then_by`/`summary_only` (an adapter that wants finer
        // condensing pre-condenses its own rows).
        let yaml = r#"
tab:
  name: T
adapter:
  type: x
views:
  - name: grouped
    node_type: tracking:entry
    group_by:
      column: started
      bucket: day
    aggregates:
      - column: duration
"#;
        let config: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let grouped = &config.views[0];
        assert_eq!(grouped.group_by.as_ref().unwrap().column, "started");
        assert_eq!(
            grouped.group_by.as_ref().unwrap().bucket,
            Some(DateBucket::Day)
        );
        assert_eq!(grouped.aggregates.len(), 1);
        assert_eq!(grouped.aggregates[0].column, "duration");
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
    fn subtab_key_accepts_scalar_list_and_empty() {
        // Regression: the shortcut menu writes the subtab switch key as a
        // list of alternatives (`key: [n, ctrl+n]`) just like a tab key.
        // `ViewDef.key` used to be scalar-only `Option<String>`, so a list
        // made the whole view file unparseable and its tab vanished on the
        // next startup. It is now `Option<KeyBinding>` — scalar, list and
        // the empty-list "disabled" form must all deserialize.
        let yaml = r#"
tab:
  name: T
adapter:
  type: x
views:
  - name: scalar
    node_type: t
    key: i
  - name: alternatives
    node_type: t
    key: [n, ctrl+n]
  - name: disabled
    node_type: t
    key: []
  - name: absent
    node_type: t
"#;
        let config: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.views[0].key, Some("i".into()));
        assert_eq!(
            config.views[1].key.as_ref().unwrap().0,
            vec!["n".to_string(), "ctrl+n".to_string()]
        );
        assert!(config.views[2].key.as_ref().unwrap().0.is_empty());
        assert_eq!(config.views[3].key, None);
    }

    #[test]
    fn shows_in_action_bar_defaults() {
        let make = |action_type: &str| ActionDef {
            name: "test".into(),
            key: Some("x".into()),
            action_type: action_type.into(),
            id: None,
            node_id_from: None,
            navigate_to: None,
            fuzzy_filter: None,
            search: None,
            text_search: None,
            tree_find: None,
            hide_from_bar: false,
            in_action_bar: false,
            editor: None,
            under_selection: false,
            commit_on_save: false,
            inherit: false,
            script_scope: Default::default(),
            script_default_field: None,
            on_container: false,
            option_menu: None,
            force: false,
            message: None,
            prominent: false,
            form: None,
            emit: None,
            on_event: None,
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
            name: "edit".into(),
            key: Some("e".into()),
            action_type: "edit".into(),
            id: None,
            node_id_from: None,
            navigate_to: None,
            fuzzy_filter: None,
            search: None,
            text_search: None,
            tree_find: None,
            hide_from_bar: true,
            in_action_bar: false,
            editor: None,
            under_selection: false,
            commit_on_save: false,
            inherit: false,
            script_scope: Default::default(),
            script_default_field: None,
            on_container: false,
            option_menu: None,
            force: false,
            message: None,
            prominent: false,
            form: None,
            emit: None,
            on_event: None,
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
        hide_from_bar: true"#;
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
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .unwrap_err();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("create") && errs[0].contains("id"));
    }

    #[test]
    fn validate_rejects_empty_views() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views: []
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .unwrap_err();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("views") && errs[0].contains("empty"));
    }

    #[test]
    fn example_tasks_yaml_parses_and_validates() {
        // The shipped `docs/examples/views/tasks.yaml` (plan phase A1) must
        // stay loadable: it parses into a ViewFileConfig and passes the
        // semantic validator (tree_label chain, recursive branch, typed
        // columns, tree_find/fuzzy_filter only at the tree root).
        let yaml = include_str!("../../../docs/examples/views/tasks.yaml");
        let mut cfg: ViewFileConfig =
            serde_yaml::from_str(yaml).expect("tasks.yaml should deserialize");
        // The loader fills tree-continuation columns AND inheritable
        // actions/shortcuts right after parse, so the recursive subtask
        // branch (which ships neither `columns:` nor `actions:`/`shortcuts:`)
        // inherits the root's set. Mirror both passes here before asserting.
        cfg.inherit_tree_columns();
        cfg.inherit_tree_actions();
        assert_eq!(cfg.adapter.adapter_type, "tasks");
        assert_eq!(cfg.views[0].tree_label.as_deref(), Some("description"));
        assert!(
            cfg.views[0]
                .columns
                .iter()
                .any(|c| c.key == "priority" && matches!(c.kind, ColumnKind::Number)),
            "priority column should be kind: number"
        );
        // A1b mutation bindings: edit/add as typed actions, the
        // delete/undelete/reparent quartet as generic shortcuts, on both
        // the root view and the recursive subtask branch.
        let root = &cfg.views[0];
        let edit = root
            .actions
            .iter()
            .find(|a| a.action_type == "edit")
            .unwrap();
        assert_eq!(edit.id.as_deref(), Some("edit"));
        let add = root
            .actions
            .iter()
            .find(|a| a.action_type == "create")
            .unwrap();
        assert_eq!(add.id.as_deref(), Some("add"));
        for (k, name) in [
            ('d', "delete"),
            ('u', "undelete"),
            ('s', "toggle-tracking"),
            ('m', "mark-move"),
            ('p', "paste-move"),
        ] {
            assert_eq!(root.shortcuts.get(&k).map(|s| s.action()), Some(name));
        }
        // A1c-1: the tracking marker column is declared on both levels.
        assert!(
            root.columns.iter().any(|c| c.key == "tracking"),
            "root view should declare the tracking marker column"
        );
        // The subtask branch ships no actions/shortcuts of its own; the
        // inheritable entries from the root view cascade in via
        // `inherit_tree_actions` (called above), so edit/create + the
        // d/s shortcuts are present at this depth too.
        let child = &root.children[0];
        assert!(child.actions.iter().any(|a| a.action_type == "edit"));
        assert!(child.actions.iter().any(|a| a.action_type == "create"));
        assert_eq!(
            child.shortcuts.get(&'d').map(|s| s.action()),
            Some("delete")
        );
        assert_eq!(
            child.shortcuts.get(&'s').map(|s| s.action()),
            Some("toggle-tracking")
        );
        // The subtask branch ships no `columns:` of its own — it inherits the
        // root's set (incl. the tracking marker) via `inherit_tree_columns`.
        assert!(child.columns.iter().any(|c| c.key == "tracking"));
        // A1c-2 / #34: the root view declares a saved-query block — editable,
        // with a `q` menu key. Since the snapshot now loads the *full* task
        // universe (deleted included), the query is the single replaceable
        // filter, so the view ships a `default` body of `[deleted, =, false]`
        // — that clause, not a baked-in snapshot filter, is what hides deleted
        // tasks by default (full non-deleted forest, including done = parity).
        let query = root
            .query
            .as_ref()
            .expect("root view should declare a query block");
        assert!(query.editable, "tasks query should be editable");
        assert_eq!(query.menu_key.as_deref(), Some("q"));
        let default = query
            .default
            .as_deref()
            .expect("tasks view ships a default query that hides deleted tasks");
        assert!(
            default.contains("deleted"),
            "the default query must filter on `deleted` (the sole filter that \
             hides deleted tasks now the universe is loaded whole): {default:?}"
        );
        // Column parity with the native tab: the default visible set + order
        // (St / Pri / Tr / T / Task / Created / Updated / Tracked / N). The
        // `🔗` links column is intentionally absent (app-level link store not
        // wired to the in-process adapter yet). `tag_names` is declared but
        // ships `hidden: true` — like the native tab it is offered in the `c`
        // column-config popup rather than shown by default, so it sits in the
        // column list (between `tag_symbols` and `description`) without
        // appearing in the rendered default set.
        let col_keys: Vec<&str> = root.columns.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(
            col_keys,
            vec![
                "status",
                "priority",
                "tracking",
                "tag_symbols",
                "tag_names",
                "description",
                "created",
                "updated",
                "last_tracked",
                "notes",
            ],
            "tasks columns must mirror the native default order"
        );
        // `tag_names` is the only hidden column; everything else renders by
        // default. This guards the new `hidden:` flag against silently
        // flipping on the wrong column.
        for col in &root.columns {
            assert_eq!(
                col.hidden,
                col.key == "tag_names",
                "only tag_names ships hidden by default (got hidden={} for {})",
                col.hidden,
                col.key
            );
        }
        // The date columns render date-only (`%Y-%m-%d`) like the native tab.
        for key in ["created", "updated", "last_tracked"] {
            let col = root.columns.iter().find(|c| c.key == key).unwrap();
            assert!(matches!(col.kind, ColumnKind::Datetime));
            assert_eq!(col.format.as_deref(), Some("%Y-%m-%d"));
        }
        // A1c (scripts): a `type: script` action on both levels reaches the
        // generic script menu (key `x`). The validator does not restrict
        // `script` to the tree root (unlike search/fuzzy_filter/tree_find).
        let root_script = root
            .actions
            .iter()
            .find(|a| a.action_type == "script")
            .unwrap();
        assert_eq!(root_script.primary_key(), Some("x"));
        assert!(child.actions.iter().any(|a| a.action_type == "script"));
        // Task-1 semantics: `a` adds a child of the selected node in the
        // tree (`under_selection`, adapter id `add`); `A` adds a *sibling*
        // (adapter id `add-sibling`). `U` un-nests to the top level. All
        // three inherit down to the recursive branch.
        let add = root
            .actions
            .iter()
            .find(|a| a.primary_key() == Some("a"))
            .unwrap();
        assert_eq!(add.id.as_deref(), Some("add"));
        assert!(
            add.under_selection,
            "tree `a` re-targets onto the selection"
        );
        let add_sibling = root
            .actions
            .iter()
            .find(|a| a.primary_key() == Some("A"))
            .unwrap();
        assert_eq!(add_sibling.action_type, "create");
        assert_eq!(add_sibling.id.as_deref(), Some("add-sibling"));
        assert!(add_sibling.under_selection);
        assert_eq!(root.shortcuts.get(&'U').map(|s| s.action()), Some("unnest"));
        let child_sibling = child
            .actions
            .iter()
            .find(|a| a.primary_key() == Some("A"))
            .unwrap();
        assert_eq!(child_sibling.id.as_deref(), Some("add-sibling"));
        assert!(child_sibling.under_selection);
        assert_eq!(
            child.shortcuts.get(&'U').map(|s| s.action()),
            Some("unnest")
        );

        cfg.validate(
            &KeyBindingConfig::default(),
            &crate::config::editor::EditorsConfig::default(),
        )
        .expect("tasks.yaml should pass the validator");
    }

    #[test]
    fn example_trackings_yaml_parses_and_validates() {
        // The shipped `docs/examples/views/trackings.yaml` (plan phase A2)
        // must stay loadable and pass the validator: the flat list, the
        // Condensed nested-grouping view, and the A2c Tree view all coexist
        // in one tab, so their subtab keys must not collide with any per-node
        // shortcut — notably the Tree's `T` switch key vs. its `t`
        // toggle-tracking shortcut.
        let yaml = include_str!("../../../docs/examples/views/trackings.yaml");
        let cfg: ViewFileConfig =
            serde_yaml::from_str(yaml).expect("trackings.yaml should deserialize");
        assert_eq!(cfg.adapter.adapter_type, "trackings");

        // Three views: flat (key a), condensed (key v), tree (key t).
        let flat = cfg.views.iter().find(|v| v.name == "trackings").unwrap();
        assert_eq!(flat.key, Some("a".into()));
        let condensed = cfg.views.iter().find(|v| v.name == "condensed").unwrap();
        assert_eq!(condensed.key, Some("v".into()));
        // Condensed + Tree are projections of the same trackings, so the
        // user-set default saved query follows across them.
        assert!(condensed.query.as_ref().unwrap().inherit_default);

        // A2c Tree: an adapter-grouped tree (`group_by_via_adapter`) —
        // root level = `tracking:tree-group` day buckets, switched to with
        // `t` (native parity — track itself sits on `s`), with own and
        // subtree-cumulated tracked time as two side-by-side columns
        // (native column parity).
        let tree = cfg.views.iter().find(|v| v.name == "tree").unwrap();
        assert_eq!(tree.key, Some("t".into()));
        assert_eq!(tree.node_type, "tracking:tree-group");
        assert_eq!(tree.tree_label.as_deref(), Some("task"));
        assert!(tree.query.as_ref().unwrap().inherit_default);
        // The grouping the adapter applies (day buckets, newest first).
        let gb = tree.group_by.as_ref().expect("tree view declares group_by");
        assert_eq!(gb.column, "started");
        assert_eq!(gb.bucket, Some(DateBucket::Day));
        assert_eq!(gb.order, GroupOrder::Desc);
        assert!(tree.columns.iter().any(|c| c.key == "duration"));
        assert!(tree.columns.iter().any(|c| c.key == "duration_cumulated"));
        // Buckets render as `── label` header rows; the appended Total
        // column reads the bucket's `duration` metadata field.
        let gh = tree
            .group_headers
            .as_ref()
            .expect("tree view declares group_headers");
        let total = gh
            .total
            .as_ref()
            .expect("group_headers carries a total column");
        assert_eq!(total.key, "total");
        assert_eq!(total.source.as_deref(), Some("duration"));
        // Group buckets are read-only aggregates — no shortcuts on the root
        // level; `s: toggle-tracking` lives on the task (item) level, which
        // also serves the root rows when grouping is cycled off.
        assert!(tree.shortcuts.is_empty());
        // The recursive subtask branch carries the same duration columns and
        // the track toggle.
        let sub = &tree.children[0];
        assert!(sub.recursive);
        assert_eq!(sub.node_type, "tracking:tree-item");
        assert!(sub.columns.iter().any(|c| c.key == "duration"));
        assert!(sub.columns.iter().any(|c| c.key == "duration_cumulated"));
        assert_eq!(
            sub.shortcuts.get(&'s').map(|s| s.action()),
            Some("toggle-tracking")
        );

        cfg.validate(
            &KeyBindingConfig::default(),
            &crate::config::editor::EditorsConfig::default(),
        )
        .expect("trackings.yaml should pass the validator");
    }

    #[test]
    fn example_calendar_yaml_parses_and_validates() {
        // The shipped `docs/examples/views/calendar.yaml` documents the MFA
        // number-match rule-engine wiring (event_actions + a keyless `notify`
        // action with `on_event: close`). Keep it loadable and valid so the
        // example never drifts from the schema.
        let yaml = include_str!("../../../docs/examples/views/calendar.yaml");
        let cfg: ViewFileConfig =
            serde_yaml::from_str(yaml).expect("calendar.yaml should deserialize");
        assert_eq!(cfg.adapter.adapter_type, "calendar");

        let events = cfg.views.iter().find(|v| v.name == "events").unwrap();
        // The bus binding routes the number-match topic to the notify action.
        let binding = events
            .event_actions
            .iter()
            .find(|b| b.on == "office365-web:mfa:number-match")
            .expect("calendar.yaml binds the number-match topic");
        assert_eq!(binding.run, "show auth number");
        // The bound action is event-only (no key), type notify, and closes on
        // resolve.
        let notify = events
            .actions
            .iter()
            .find(|a| a.name == "show auth number")
            .expect("calendar.yaml declares the show-auth-number action");
        assert_eq!(notify.action_type, "notify");
        assert!(notify.key.is_none());
        assert!(notify.message.as_deref().unwrap().contains("{number}"));
        assert_eq!(
            notify
                .on_event
                .as_ref()
                .unwrap()
                .get("office365-web:mfa:resolved"),
            Some(&OnEventReaction::Close)
        );

        cfg.validate(
            &KeyBindingConfig::default(),
            &crate::config::editor::EditorsConfig::default(),
        )
        .expect("calendar.yaml should pass the validator");
    }

    #[test]
    fn example_postgres_yaml_parses_and_validates() {
        // The shipped `docs/examples/views/postgres.yaml` documents the
        // Postgres tree/flat views, the coupled record-detail split
        // (`record_detail: true`) and — the point of the example — the
        // `w` window-leader opt-in (`window_ops: true`) on every
        // top-level view. Keep it loadable and valid so it never drifts.
        let yaml = include_str!("../../../docs/examples/views/postgres.yaml");
        let mut cfg: ViewFileConfig =
            serde_yaml::from_str(yaml).expect("postgres.yaml should deserialize");
        // The recursive db-script branch ships neither columns nor
        // actions of its own; mirror the loader's inherit passes.
        cfg.inherit_tree_columns();
        cfg.inherit_tree_actions();
        assert_eq!(cfg.adapter.adapter_type, "postgres");

        // Every top-level view opts into the window-leader chords — the
        // regression this example guards against (window_ops defaults off).
        for name in ["databases", "tables", "scripts"] {
            let v = cfg
                .views
                .iter()
                .find(|v| v.name == name)
                .unwrap_or_else(|| panic!("postgres.yaml should declare a `{name}` view"));
            assert!(v.window_ops, "`{name}` view should set window_ops: true");
        }

        // The flat `tables` view offers the record-detail split on its Rows.
        let tables = cfg.views.iter().find(|v| v.name == "tables").unwrap();
        let rows = tables
            .children
            .iter()
            .find(|c| c.name == "Rows")
            .expect("tables view should have a Rows child");
        assert!(rows.record_detail, "Rows should set record_detail: true");

        cfg.validate(
            &KeyBindingConfig::default(),
            &crate::config::editor::EditorsConfig::default(),
        )
        .expect("postgres.yaml should pass the validator");
    }

    #[test]
    fn example_sqlite_yaml_parses_and_validates() {
        // The shipped `docs/examples/views/sqlite.yaml` documents the
        // SQLite tree — one level flatter than Postgres, since SQLite has
        // no schema namespace: database → Tables → table → rows. Keep it
        // loadable and valid so it never drifts.
        let yaml = include_str!("../../../docs/examples/views/sqlite.yaml");
        let mut cfg: ViewFileConfig =
            serde_yaml::from_str(yaml).expect("sqlite.yaml should deserialize");
        cfg.inherit_tree_columns();
        cfg.inherit_tree_actions();
        assert_eq!(cfg.adapter.adapter_type, "sqlite");

        for name in ["databases", "tables"] {
            let v = cfg
                .views
                .iter()
                .find(|v| v.name == name)
                .unwrap_or_else(|| panic!("sqlite.yaml should declare a `{name}` view"));
            assert!(v.window_ops, "`{name}` view should set window_ops: true");
        }

        // The drill has no schema level: Tables sits directly under a
        // database. That's the structural difference the example exists
        // to document.
        let databases = cfg.views.iter().find(|v| v.name == "databases").unwrap();
        let tables_group = databases
            .children
            .iter()
            .find(|c| c.node_type == "sqlite:tables")
            .expect("databases view should have a Tables group child");
        let table = tables_group
            .children
            .iter()
            .find(|c| c.node_type == "sqlite:table")
            .expect("Tables group should list sqlite:table directly");

        // Per-table SQL scripts are opt-in via `node_scripts:` — the host
        // knows nothing about SQLite, so without the flag `q`/`Q` stay
        // ordinary keys. It has to sit on the table level in both views.
        assert!(table.node_scripts, "Table child should own node scripts");
        let flat_tables = cfg.views.iter().find(|v| v.name == "tables").unwrap();
        assert!(
            flat_tables.node_scripts,
            "flat tables view should own node scripts"
        );

        // The db-script branch hangs beside Tables, with the same recursive
        // folder shape the Postgres example has.
        let scripts = databases
            .children
            .iter()
            .find(|c| c.node_type == "sqlite:db_scripts")
            .expect("databases view should have a Scripts group child");
        let dir = scripts
            .children
            .iter()
            .find(|c| c.node_type == "sqlite:db_script_dir")
            .expect("Scripts group should list folders");
        assert!(dir.recursive, "folders have to nest arbitrarily deep");

        // The one place this branch must NOT copy Postgres: SQLite has no
        // server-side cursor, so every result pane pages via LIMIT/OFFSET.
        // A `mode: cursor` here would make `x` fail at runtime.
        for script in scripts
            .children
            .iter()
            .chain(dir.children.iter())
            .filter(|c| c.node_type == "sqlite:db_script")
        {
            let result = script
                .children
                .iter()
                .find(|c| c.node_type == "sqlite:db_script_result")
                .expect("a script should open a result pane");
            let pagination = result
                .pagination
                .as_ref()
                .expect("result pane should configure pagination");
            assert_eq!(
                pagination.mode,
                PaginationMode::Server,
                "sqlite result panes cannot use cursor pagination"
            );
        }

        cfg.validate(
            &KeyBindingConfig::default(),
            &crate::config::editor::EditorsConfig::default(),
        )
        .expect("sqlite.yaml should pass the validator");
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
        assert!(
            errs[0].contains("nope") && errs[0].contains("editors"),
            "got: {}",
            errs[0]
        );
    }

    #[test]
    fn validate_group_headers_requires_tree_and_group_by() {
        // `group_headers` renders adapter group buckets as `── label` header
        // rows — meaningless without tree mode and a root `group_by`.
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    group_headers: {}
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
                .any(|e| e.contains("group_headers") && e.contains("tree_label")),
            "got: {errs:?}"
        );
        assert!(
            errs.iter()
                .any(|e| e.contains("group_headers") && e.contains("group_by")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn validate_rejects_unknown_script_source() {
        // A `script_source` naming no sibling view is flagged (typo would
        // otherwise silently fall back to the view's own script scope).
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: tickets
    node_type: t
  - name: bookmarks
    node_type: b
    script_source: typo
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
                .any(|e| e.contains("script_source") && e.contains("typo")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn validate_accepts_known_script_source() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: tickets
    node_type: t
  - name: bookmarks
    node_type: b
    script_source: tickets
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .err()
            .unwrap_or_default();
        assert!(
            !errs.iter().any(|e| e.contains("script_source")),
            "unexpected script_source error: {errs:?}"
        );
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
        cfg.validate(&KeyBindingConfig::default(), &editors)
            .unwrap();
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
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .unwrap_err();
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
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .unwrap_err();
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
        cfg.validate(
            &KeyBindingConfig::default(),
            &crate::config::editor::EditorsConfig::default(),
        )
        .unwrap();
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
        cfg.validate(
            &KeyBindingConfig::default(),
            &crate::config::editor::EditorsConfig::default(),
        )
        .unwrap();
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
        cfg.validate(
            &KeyBindingConfig::default(),
            &crate::config::editor::EditorsConfig::default(),
        )
        .unwrap();
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
        cfg.validate(
            &KeyBindingConfig::default(),
            &crate::config::editor::EditorsConfig::default(),
        )
        .unwrap();
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
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .unwrap_err();
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
        let seq = parsed
            .as_sequence()
            .expect("default should be a YAML sequence");
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
      - { name: nav, key: C, type: navigate, navigate_to: t2 }
      - { name: refresh, key: r, type: reload }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        cfg.validate(
            &KeyBindingConfig::default(),
            &crate::config::editor::EditorsConfig::default(),
        )
        .unwrap();
    }

    #[test]
    fn event_only_action_and_event_actions_parse_and_validate() {
        // An event-only action has no `key`; it is reached solely via the
        // view's `event_actions:` rule engine. `notify`, `emit`, and
        // `on_event` are the new MFA-flow building blocks.
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    actions:
      - name: show-auth-number
        type: notify
        on_event: { "office365-web:mfa:resolved": close }
      - name: enter-otc
        key: o
        type: custom
        id: noop
        emit:
          topic: office365-web:mfa:otc-provided
          payload: { code: "{code}" }
    event_actions:
      - { on: "office365-web:mfa:number-match", run: show-auth-number }
      - { on: "office365-web:mfa:otc-required", run: enter-otc }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        cfg.validate(
            &KeyBindingConfig::default(),
            &crate::config::editor::EditorsConfig::default(),
        )
        .unwrap();
        let view = &cfg.views[0];
        // Event-only action: no key, no keymap presence.
        assert!(view.actions[0].key.is_none());
        assert_eq!(
            view.actions[0].on_event.as_ref().unwrap()["office365-web:mfa:resolved"],
            OnEventReaction::Close
        );
        // Emitting action carries the reply topic + templated payload.
        assert_eq!(
            view.actions[1].emit.as_ref().unwrap().topic,
            "office365-web:mfa:otc-provided"
        );
        assert_eq!(view.event_actions.len(), 2);
        assert_eq!(view.event_actions[0].run, "show-auth-number");
    }

    #[test]
    fn event_action_run_must_name_an_existing_action() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    actions:
      - { name: edit, key: e, type: edit, id: edit_full }
    event_actions:
      - { on: "some:topic", run: nonexistent }
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("names no action")),
            "expected a run-target error, got {errs:?}"
        );
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
        cfg.validate(
            &KeyBindingConfig::default(),
            &crate::config::editor::EditorsConfig::default(),
        )
        .unwrap();
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
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .unwrap_err();
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
            errs.iter()
                .any(|e| e.contains("row_layout") && e.contains("'nope'")),
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
        assert!(
            layout[0].highlight_on_select,
            "shorthand non-empty defaults on"
        );
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

    /// The 2×3 shape from the spec: six fields at `columns: 3`. Also pins the
    /// defaults and both field forms (bare key vs. explicit label).
    #[test]
    fn card_parses_a_two_by_three_grid_with_defaults() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    columns:
      - { key: key }
      - { key: summary }
      - { key: status }
      - { key: assignee }
      - { key: updated }
      - { key: creator }
    card:
      key: C
      fields:
        - key
        - { column: summary, label: "Title" }
        - status
        - assignee
        - updated
        - creator
      columns: 3
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        cfg.validate(
            &KeyBindingConfig::default(),
            &crate::config::editor::EditorsConfig::default(),
        )
        .unwrap();
        let card = cfg.views[0].card.as_ref().unwrap();
        assert_eq!(card.columns, 3);
        // Six fields over three grid columns → two card lines (derived).
        assert_eq!(card.fields.len(), 6);
        assert_eq!(card.fields[0].column, "key");
        assert_eq!(
            card.fields[0].label, None,
            "bare key inherits the column label"
        );
        assert_eq!(card.fields[1].label.as_deref(), Some("Title"));
        assert_eq!(card.labels, CardLabelMode::Inline);
        assert_eq!(card.border, CardBorderMode::Rounded);
        assert_eq!(card.padding, 1);
        assert_eq!(card.gap, 0);
        assert_eq!(card.separator, "  ");
        assert!(
            !card.default,
            "declaring card: only makes the mode available"
        );
        assert!(card.key.is_some());
        assert!(card.weights.is_empty(), "no weights → equal shares");
    }

    #[test]
    fn example_cards_yaml_parses_and_validates() {
        // The shipped `docs/examples/views/cards.yaml` documents the 2×3 card
        // and must stay loadable: it parses into a ViewFileConfig, passes the
        // semantic validator, and keeps the shape the doc claims.
        let yaml = include_str!("../../../docs/examples/views/cards.yaml");
        let cfg: ViewFileConfig =
            serde_yaml::from_str(yaml).expect("cards.yaml should deserialize");
        cfg.validate(
            &KeyBindingConfig::default(),
            &crate::config::editor::EditorsConfig::default(),
        )
        .expect("cards.yaml should validate");
        let card = cfg.views[0].card.as_ref().unwrap();
        assert_eq!(card.columns, 3);
        assert_eq!(
            card.fields.len(),
            6,
            "six fields over three slots → two lines"
        );
        assert_eq!(card.weights, vec![1, 1, 2]);
        assert!(card.key.is_some(), "the example ships a toggle key");
        assert!(!card.default, "the example opens as a table");
    }

    #[test]
    fn card_defaults_to_one_field_per_line_and_is_absent_by_default() {
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
        columns: [{ key: body }]
        card:
          fields: [body]
          labels: none
          border: plain
          gap: 1
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        cfg.validate(
            &KeyBindingConfig::default(),
            &crate::config::editor::EditorsConfig::default(),
        )
        .unwrap();
        assert!(cfg.views[0].card.is_none(), "no card: → no card mode");
        let card = cfg.views[0].children[0].card.as_ref().unwrap();
        assert_eq!(card.columns, 1);
        assert_eq!(card.labels, CardLabelMode::None);
        assert_eq!(card.border, CardBorderMode::Plain);
        assert_eq!(card.gap, 1);
        assert!(card.key.is_none(), "no key → reachable via default: only");
    }

    /// `border: none` plus a `divider:` is the "no frame, ruled between cards"
    /// look: both are plain config, no extra switch.
    #[test]
    fn card_parses_borderless_with_a_divider_rule() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    columns: [{ key: a }, { key: b }]
    card:
      fields: [a, b]
      columns: 2
      border: none
      divider: "─"
      gap: 1
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        cfg.validate(
            &KeyBindingConfig::default(),
            &crate::config::editor::EditorsConfig::default(),
        )
        .unwrap();
        let card = cfg.views[0].card.as_ref().unwrap();
        assert_eq!(card.border, CardBorderMode::None);
        assert_eq!(card.divider, "─");
        assert_eq!(
            card.separator, "  ",
            "`separator:` (within a line) is untouched by `divider:`"
        );
    }

    #[test]
    fn card_divider_defaults_to_empty() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    columns: [{ key: a }]
    card:
      fields: [a]
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.views[0].card.as_ref().unwrap().divider.is_empty());
    }

    #[test]
    fn validate_card_rejects_unknown_field() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    columns: [{ key: name }]
    card:
      fields: [name, nope]
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
                .any(|e| e.contains("card") && e.contains("'nope'")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn validate_card_rejects_markdown_field() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    columns:
      - { key: author }
      - { key: body, markdown: true }
    card:
      fields: [author, body]
      columns: 2
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
                .any(|e| e.contains("card") && e.contains("markdown")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn validate_card_rejects_weights_not_matching_columns() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    columns: [{ key: a }, { key: b }, { key: c }]
    card:
      fields: [a, b, c]
      columns: 3
      weights: [1, 2]
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .unwrap_err();
        assert!(errs.iter().any(|e| e.contains("weights")), "got: {errs:?}");
    }

    #[test]
    fn validate_card_rejects_zero_columns() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    columns: [{ key: a }]
    card:
      fields: [a]
      columns: 0
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("columns: 0")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn card_without_fields_is_valid_and_means_all_columns() {
        // No `fields:` — the card shows the level's whole column list, so a
        // column added later needs no second edit.
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    columns: [{ key: a }, { key: b }, { key: c }]
    card:
      columns: 2
"#;
        let cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        cfg.validate(
            &KeyBindingConfig::default(),
            &crate::config::editor::EditorsConfig::default(),
        )
        .expect("an omitted `fields:` is not an error");
        assert!(cfg.views[0].card.as_ref().unwrap().fields.is_empty());
    }

    #[test]
    fn validate_card_rejects_omitted_fields_with_nothing_to_show() {
        // "All columns" minus the markdown ones leaves an empty card — that
        // one is worth reporting instead of rendering blank frames.
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    columns: [{ key: a, markdown: true }]
    card:
      columns: 1
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
                .any(|e| e.contains("`fields:` is omitted") && e.contains("no column")),
            "got: {errs:?}"
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
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("tree_label 'oops'") && e.contains("schema")),
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
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("no ancestor has tree_label")),
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
        assert!(
            cfg.validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default()
            )
            .is_ok()
        );
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
            cfg.validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default()
            )
            .is_ok(),
            "got: {:?}",
            cfg.validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default()
            )
            .unwrap_err()
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
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .unwrap_err();
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
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .unwrap_err();
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
        cfg.validate(
            &KeyBindingConfig::default(),
            &crate::config::editor::EditorsConfig::default(),
        )
        .unwrap();
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
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .unwrap_err();
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
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("fuzzy_filter is defined at multiple tree levels")),
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
        cfg.validate(
            &KeyBindingConfig::default(),
            &crate::config::editor::EditorsConfig::default(),
        )
        .unwrap();
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
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("type='tree_find'") && e.contains("tree_label")),
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
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("type='tree_find'") && e.contains("tree_label")),
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
        cfg.validate(
            &KeyBindingConfig::default(),
            &crate::config::editor::EditorsConfig::default(),
        )
        .unwrap();
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
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("tree_find is defined at multiple tree levels")),
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
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("views.tables") && e.contains("\"f\"")),
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
        assert_eq!(
            view.shortcuts.get(&'x').map(|s| s.action()),
            Some("execute")
        );
        assert_eq!(view.shortcuts.get(&'e').map(|s| s.action()), Some("edit"));
        assert_eq!(
            view.children[0].shortcuts.get(&'d').map(|s| s.action()),
            Some("delete")
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
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("views.v.shortcuts['x']") && e.contains("action id is empty")),
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
        let errs = cfg
            .validate(
                &KeyBindingConfig::default(),
                &crate::config::editor::EditorsConfig::default(),
            )
            .unwrap_err();
        assert!(
            errs.iter()
                .any(|e| e.contains("views.v.shortcuts['r']") && e.contains("already bound")),
            "expected a collision error for shortcut 'r' vs action 'refresh', got: {errs:?}"
        );
    }

    #[test]
    fn inherit_tree_columns_fills_continuation_levels() {
        // Root declares columns; the recursive tree-continuation child omits
        // them and must inherit. A separate non-tree view stays independent,
        // and an explicit column set is left untouched.
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: tree
    node_type: t
    tree_label: title
    columns:
      - { key: title, source: label }
      - { key: status }
    children:
      - name: kids
        node_type: t
        tree_label: title
        recursive: true
      - name: own
        node_type: u
        tree_label: title
        columns:
          - { key: title, source: label }
  - name: flat
    node_type: f
    columns:
      - { key: only }
"#;
        let mut cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        cfg.inherit_tree_columns();

        let tree = &cfg.views[0];
        // The recursive continuation child inherited the root's two columns.
        let kids = tree.children.iter().find(|c| c.name == "kids").unwrap();
        let kid_keys: Vec<&str> = kids.columns.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(kid_keys, vec!["title", "status"]);
        // A child that declares its own columns keeps exactly those.
        let own = tree.children.iter().find(|c| c.name == "own").unwrap();
        assert_eq!(own.columns.len(), 1);
        assert_eq!(own.columns[0].key, "title");
        // A separate non-tree view does not inherit across the view boundary.
        let flat = &cfg.views[1];
        let flat_keys: Vec<&str> = flat.columns.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(flat_keys, vec!["only"]);
    }

    #[test]
    fn inherit_tree_columns_leaves_non_tree_drill_child_empty() {
        // A drill child WITHOUT tree_label (a metadata-fallback level, e.g.
        // postgres rows) must stay empty — it does not join the tree's grid.
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: v
    node_type: t
    tree_label: title
    columns:
      - { key: title, source: label }
    children:
      - name: rows
        node_type: r
"#;
        let mut cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        cfg.inherit_tree_columns();
        let rows = &cfg.views[0].children[0];
        assert!(rows.columns.is_empty());
    }

    // -----------------------------------------------------------------------
    // Scaffold round-trip contract
    //
    // `content::scaffold::generate` hand-renders view-config YAML from the
    // adapter protocol (it lives in the content crate and can't `Serialize`
    // *these* structs, which are `Deserialize`-only and downstream of it).
    // That means the field names it emits are an untyped contract with this
    // schema. This test pins that contract: whatever the scaffolder emits must
    // still deserialize into `ViewFileConfig` — if a field here is renamed or
    // gains `deny_unknown_fields` coverage the scaffolder doesn't know about,
    // this fails instead of silently shipping a config the TUI rejects.
    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn scaffold_output_deserializes_into_view_file_config() {
        use not_yet_done_content::mock::{MockAdapterBuilder, MockNodeData, issue_type};
        use not_yet_done_content::{
            AdapterCapabilities, InputSpec, NodeAction, ScaffoldFileMeta, ScaffoldSelection,
            generate_scaffold,
        };

        let adapter = MockAdapterBuilder::new("mock")
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
                    // Adapters no longer suggest keys, so the scaffolder emits
                    // *every* action commented out with a `# TODO key`; neither
                    // may break the round trip.
                    NodeAction::new("edit", "edit", InputSpec::Editor),
                    NodeAction::new("transition", "transition", InputSpec::Picker),
                ],
            )
            .node(
                MockNodeData::new("root", "Root")
                    .child_type(issue_type())
                    .child(MockNodeData::new("ISS-1", "First").node_type(issue_type())),
            )
            .build();

        let meta = ScaffoldFileMeta {
            tab_name: "Mock".to_string(),
            order: 3,
            adapter_type: "mock".to_string(),
            adapter_id: Some("mock-1".to_string()),
            config: None,
            config_inline: Some("{}".to_string()),
            manual_connect: false,
        };

        let yaml = generate_scaffold(&adapter, &meta, &ScaffoldSelection::all())
            .await
            .expect("scaffold generation failed");

        // The contract: it deserializes into the real schema without loss.
        let cfg: ViewFileConfig = serde_yaml::from_str(&yaml)
            .unwrap_or_else(|e| panic!("scaffold did not round-trip: {e}\n---\n{yaml}"));

        // And the header + one view the protocol described survived intact,
        // proving the field names (not just the syntax) line up.
        assert_eq!(cfg.tab.name, "Mock");
        assert_eq!(cfg.tab.order, 3);
        assert_eq!(cfg.adapter.adapter_type, "mock");
        assert_eq!(cfg.adapter.id.as_deref(), Some("mock-1"));
        let view = cfg
            .views
            .iter()
            .find(|v| v.node_type == "mock:issue")
            .expect("issue view missing");
        assert!(view.default, "first view should be the default");
        assert!(view.key.is_some(), "top-level view should get a subtab key");
        assert!(
            view.columns.iter().any(|c| c.key == "label"),
            "label column seed missing"
        );
        // Every action is emitted commented out (adapters no longer suggest
        // keys), so none of them materialize in the parsed set…
        assert!(
            view.actions.is_empty(),
            "actions should all be commented out, got {:?}",
            view.actions
        );
        // …but they are present-but-commented in the raw text with a `# TODO
        // key`, so nothing is silently dropped.
        assert!(
            yaml.contains("# TODO key") && yaml.contains("id: edit"),
            "edit action should be present but commented out:\n{yaml}"
        );
        assert!(
            yaml.contains("id: transition"),
            "transition action should be present but commented out:\n{yaml}"
        );
    }
}
