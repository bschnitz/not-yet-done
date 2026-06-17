//! Declarative keymap — single source of truth for both key dispatch
//! and conflict validation.
//!
//! ## Why this exists
//!
//! Before this module, dispatch was scattered across `ContentView::handle_key`,
//! `ContentPane::handle_key`, the App-level action-chain interceptor, plus the
//! tasks/trackings views. Conflicts between YAML-defined actions and built-in
//! keys (subtab switch, common navigation, content actions, global hotkeys)
//! were caught only at runtime — usually by a confused user.
//!
//! The keymap inverts that: every site that wants to react to a key files a
//! [`KeyClaim`]. Both validation and dispatch read the same list. A new key
//! handler that is not represented as a claim simply does not run, so the
//! validator cannot drift out of sync with the dispatcher.
//!
//! ## Scope
//!
//! Phase 1 added the data types and the conflict validator. Phase 2 wired
//! `ContentView::handle_key` / `ContentPane::handle_key` through claims.
//! Phase 3 adds a config-time builder ([`build_view_leaf_maps`]) so loading
//! a YAML view-file can fail loudly on conflicting bindings instead of
//! confusing the user later at runtime.

use crate::config::keybindings::{
    CommonAction, ContentAction, GlobalAction, KeyBinding, KeyBindingConfig, TrackingsAction,
    WindowAction,
};
use crate::config::view_config::{ChildDef, ViewDef, ViewFileConfig};

// ---------------------------------------------------------------------------
// Scope
// ---------------------------------------------------------------------------

/// Where a [`KeyClaim`] is in scope.
///
/// Two claims with overlapping keys conflict only if their scopes overlap
/// (see [`KeyScope::overlaps_with`]). A `Tab(t)` claim conflicts with a
/// `Pane(t, _)` claim, but two `Tab` claims for different tabs do not.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeyScope {
    /// Strictly global — claim is active in every tab and every pane state.
    /// Used for hotkeys like `ctrl+c` that must never be shadowed.
    Global,
    /// Active only when the tab identified by `tab` is foregrounded.
    Tab(TabRef),
    /// Active only in panes of `tab` that are in the matching state profile.
    Pane(TabRef, PaneStateProfile),
}

/// Stable reference to a tab. We use the YAML `tab.name` for adapter tabs
/// and a hard-coded label for built-ins.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TabRef(pub String);

impl TabRef {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
}

/// State a pane is in. Used to scope claims that only apply, e.g., while
/// the fuzzy-filter input is active or only at the root navigation level.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PaneStateProfile {
    /// A text-input mode is consuming characters. Single-letter handlers
    /// must yield to the input buffer.
    InputMode(InputMode),
    /// Normal table state. `drilldown = Some(false)` is root only,
    /// `Some(true)` is drilled-in only, `None` matches either.
    Normal { drilldown: Option<bool> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputMode {
    Fuzzy,
    Search,
}

impl KeyScope {
    /// Whether two scopes can simultaneously be active for the same key
    /// press. Used by the validator to decide if two claims conflict.
    pub fn overlaps_with(&self, other: &KeyScope) -> bool {
        match (self, other) {
            (KeyScope::Global, _) | (_, KeyScope::Global) => true,
            (KeyScope::Tab(a), KeyScope::Tab(b)) => a == b,
            (KeyScope::Tab(a), KeyScope::Pane(b, _)) | (KeyScope::Pane(b, _), KeyScope::Tab(a)) => {
                a == b
            }
            (KeyScope::Pane(a, pa), KeyScope::Pane(b, pb)) => a == b && profile_overlaps(pa, pb),
        }
    }
}

fn profile_overlaps(a: &PaneStateProfile, b: &PaneStateProfile) -> bool {
    match (a, b) {
        (PaneStateProfile::InputMode(x), PaneStateProfile::InputMode(y)) => x == y,
        (PaneStateProfile::InputMode(_), _) | (_, PaneStateProfile::InputMode(_)) => false,
        (
            PaneStateProfile::Normal { drilldown: dx },
            PaneStateProfile::Normal { drilldown: dy },
        ) => match (dx, dy) {
            (None, _) | (_, None) => true,
            (Some(x), Some(y)) => x == y,
        },
    }
}

// ---------------------------------------------------------------------------
// Source — where the claim came from, for human-readable error messages
// ---------------------------------------------------------------------------

/// Identifies the origin of a [`KeyClaim`]. Surfaced in conflict messages
/// so users can locate the offending YAML entry or built-in handler.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeySource {
    Global(GlobalAction),
    Common(CommonAction),
    Content(ContentAction),
    Window(WindowAction),
    Trackings(TrackingsAction),
    /// `actions:` entry in a YAML view or child.
    YamlAction {
        view: String,
        child_path: Vec<String>,
        name: String,
    },
    /// Shortcut bound to a DB-stored saved query (per `query_shortcut`
    /// row, scoped to a view). Bodies live in adapter-managed storage;
    /// this source only carries enough identity to look the query up
    /// at dispatch time.
    SavedQueryShortcut {
        view: String,
        name: String,
    },
    /// Shortcut bound to a Postgres per-table script (per `query_shortcut`
    /// row scoped to `postgres/<inst>/<db>/schemas/<schema>/tables/<table>`).
    /// Live only while the focused pane has that exact table node in scope
    /// (selected item on the tables list, or parent of a rows pane).
    PostgresTableScriptShortcut {
        table_node_id: String,
        script: String,
    },
    /// `query.menu_key` on a YAML view.
    YamlMenuKey {
        view: String,
    },
    /// `preview.keybinding` on a YAML view or child.
    YamlPreviewKey {
        view: String,
        child_path: Vec<String>,
    },
    /// Top-level `views[*].key` — switches subtab inside an adapter tab.
    YamlSubtab {
        view: String,
    },
    /// `children[*].keybinding` (drilldown override) — currently only the
    /// `back` action is overridable per child.
    YamlChildKeybinding {
        view: String,
        child_path: Vec<String>,
        action: String,
    },
    /// Search-result jump key (next or previous) configured by a `search`
    /// action's `next_key` / `prev_key`. Default n / N. Live only while
    /// the underlying search has matches.
    PaneSearchJump {
        direction: SearchJump,
    },
    /// User-defined `action_chains:` entry. `scope_path` identifies the
    /// scope the chain was declared at:
    /// - `[]` — global `keybindings.action_chains`
    /// - `[view]` — `views[*].action_chains`
    /// - `[view, child, ...]` — `children[*].action_chains` somewhere
    ///   in the drill-down tree of `view`
    AppActionChain {
        scope_path: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SearchJump {
    Next,
    Prev,
}

impl KeySource {
    pub fn human(&self) -> String {
        match self {
            Self::Global(a) => format!("global.{}", a),
            Self::Common(a) => format!("common.{}", a),
            Self::Content(a) => format!("content.{}", a),
            Self::Window(a) => format!("window.{}", a),
            Self::Trackings(a) => format!("trackings.{}", a),
            Self::YamlAction {
                view,
                child_path,
                name,
            } => {
                if child_path.is_empty() {
                    format!("views.{view}.actions[{name}]")
                } else {
                    format!(
                        "views.{view}.children.{}.actions[{name}]",
                        child_path.join(".")
                    )
                }
            }
            Self::SavedQueryShortcut { view, name } => {
                format!("views.{view}.saved_query[{name}]")
            }
            Self::PostgresTableScriptShortcut {
                table_node_id,
                script,
            } => {
                format!("postgres.script[{table_node_id}/{script}]")
            }
            Self::YamlMenuKey { view } => format!("views.{view}.query.menu_key"),
            Self::YamlPreviewKey { view, child_path } => {
                if child_path.is_empty() {
                    format!("views.{view}.preview.keybinding")
                } else {
                    format!(
                        "views.{view}.children.{}.preview.keybinding",
                        child_path.join(".")
                    )
                }
            }
            Self::YamlSubtab { view } => format!("views.{view}.key"),
            Self::YamlChildKeybinding {
                view,
                child_path,
                action,
            } => {
                format!(
                    "views.{view}.children.{}.keybindings.{action}",
                    child_path.join(".")
                )
            }
            Self::PaneSearchJump { direction } => match direction {
                SearchJump::Next => "pane.search_next".into(),
                SearchJump::Prev => "pane.search_prev".into(),
            },
            Self::AppActionChain { scope_path } => {
                if scope_path.is_empty() {
                    "action_chains[global]".into()
                } else {
                    format!("action_chains[{}]", scope_path.join("."))
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Claim
// ---------------------------------------------------------------------------

/// What a claim does when its key fires.
///
/// `Handler` claims actually run code; `Swallow` claims discard the key
/// (e.g. an active fuzzy-input pane swallows everything that the input
/// component itself does not consume). The validator only flags conflicts
/// between two `Handler` claims — a `Swallow` shadowing a `Handler` is
/// the intended dispatch order, not a misconfiguration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyClaimKind {
    Handler,
    Swallow,
}

#[derive(Debug, Clone)]
pub struct KeyClaim {
    pub key: KeyBinding,
    pub scope: KeyScope,
    pub source: KeySource,
    pub kind: KeyClaimKind,
}

impl KeyClaim {
    pub fn handler(key: KeyBinding, scope: KeyScope, source: KeySource) -> Self {
        Self {
            key,
            scope,
            source,
            kind: KeyClaimKind::Handler,
        }
    }

    pub fn swallow(key: KeyBinding, scope: KeyScope, source: KeySource) -> Self {
        Self {
            key,
            scope,
            source,
            kind: KeyClaimKind::Swallow,
        }
    }
}

// ---------------------------------------------------------------------------
// KeyMap
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct KeyMap {
    pub claims: Vec<KeyClaim>,
}

impl KeyMap {
    pub fn new() -> Self {
        Self { claims: Vec::new() }
    }

    pub fn push(&mut self, claim: KeyClaim) {
        self.claims.push(claim);
    }

    /// Find every pair of `Handler` claims that share at least one key
    /// string and whose scopes overlap. Reported once per pair.
    pub fn validate(&self) -> Vec<Conflict> {
        let mut conflicts = Vec::new();
        let handlers: Vec<(usize, &KeyClaim)> = self
            .claims
            .iter()
            .enumerate()
            .filter(|(_, c)| c.kind == KeyClaimKind::Handler)
            .collect();

        for (i, (_, a)) in handlers.iter().enumerate() {
            for (_, b) in handlers.iter().skip(i + 1) {
                let shared: Vec<String> = a
                    .key
                    .0
                    .iter()
                    .filter(|k| b.key.0.iter().any(|kb| kb == *k))
                    .cloned()
                    .collect();
                if shared.is_empty() {
                    continue;
                }
                if !a.scope.overlaps_with(&b.scope) {
                    continue;
                }
                conflicts.push(Conflict {
                    keys: shared,
                    a: (*a).clone(),
                    b: (*b).clone(),
                });
            }
        }
        conflicts
    }
}

#[derive(Debug, Clone)]
pub struct Conflict {
    pub keys: Vec<String>,
    pub a: KeyClaim,
    pub b: KeyClaim,
}

impl Conflict {
    pub fn human(&self) -> String {
        format!(
            "key {:?} is claimed by both {} (scope {:?}) and {} (scope {:?})",
            self.keys,
            self.a.source.human(),
            self.a.scope,
            self.b.source.human(),
            self.b.scope,
        )
    }
}

// ---------------------------------------------------------------------------
// Builder for `ViewFileConfig` — used at config-load time
// ---------------------------------------------------------------------------

/// One entry per active "leaf" (subtab × drilldown path) of an adapter
/// tab. The validator runs `keymap.validate()` on each leaf separately:
/// at runtime only one leaf is active at a time, so claims in different
/// leaves cannot collide. Producing the cross-leaf product of claims
/// would generate false positives.
#[derive(Debug, Clone)]
pub struct ViewLeafMap {
    pub view: String,
    pub child_path: Vec<String>,
    pub keymap: KeyMap,
}

/// Build one [`ViewLeafMap`] per leaf of `config`. A leaf is either a
/// subtab's root list or any of its drill-down levels. Each map carries
/// the union of:
/// - tab-wide claims (Globals, Common nav active in adapter panes,
///   Window-leader, ContentAction defaults),
/// - the leaf-specific claims (YAML actions, preview key, child-level
///   keybinding overrides),
/// - and — only at root — the YamlSubtab keys, query menu_key, and
///   saved-query shortcuts.
///
/// `kb` is the *effective* keybinding config (defaults merged with the
/// user's `tui.yaml`), so the validator catches collisions against the
/// user's actual bindings rather than only the built-ins.
pub fn build_view_leaf_maps(config: &ViewFileConfig, kb: &KeyBindingConfig) -> Vec<ViewLeafMap> {
    build_leaf_maps(&TabRef::new(&config.tab.name), &config.views, kb)
}

/// Internal builder working from the parts a `ContentView` retains at
/// runtime (`tab_name` + `view_defs`), so the saved-query shortcut
/// check below can run without the original [`ViewFileConfig`].
fn build_leaf_maps(tab: &TabRef, views: &[ViewDef], kb: &KeyBindingConfig) -> Vec<ViewLeafMap> {
    let mut leaves = Vec::new();
    for view in views {
        leaves.extend(build_for_view(tab, views, view, kb));
    }
    leaves
}

fn build_for_view(
    tab: &TabRef,
    views: &[ViewDef],
    view: &ViewDef,
    kb: &KeyBindingConfig,
) -> Vec<ViewLeafMap> {
    let mut out = Vec::new();
    // Root leaf for this subtab.
    let mut root_map = KeyMap::new();
    push_tab_wide(&mut root_map, tab, kb);
    push_subtab_keys(&mut root_map, tab, views);
    push_view_query_keys(&mut root_map, tab, view);
    push_leaf_column_cursor(&mut root_map, tab, root_profile(), view.column_cursor);
    push_leaf_content_keys(
        &mut root_map,
        tab,
        root_profile(),
        view,
        &[],
        None,
        view.column_cursor,
        kb,
    );
    push_view_actions(&mut root_map, tab, view, true);
    push_action_chain_claims(&mut root_map, tab, root_profile(), view, &[], None, kb);
    out.push(ViewLeafMap {
        view: view.name.clone(),
        child_path: Vec::new(),
        keymap: root_map,
    });

    // Child leaves: each ChildDef anywhere in the subtree gets its own
    // keymap. At the same depth, multiple siblings are mutually
    // exclusive at runtime, so they get separate leaves.
    for child in &view.children {
        push_child_leaves(&mut out, tab, views, view, &Vec::new(), child, kb);
    }
    out
}

fn push_child_leaves(
    out: &mut Vec<ViewLeafMap>,
    tab: &TabRef,
    views: &[ViewDef],
    view: &ViewDef,
    parent_path: &[String],
    child: &ChildDef,
    kb: &KeyBindingConfig,
) {
    let mut path = parent_path.to_vec();
    path.push(child.name.clone());

    let mut km = KeyMap::new();
    push_tab_wide(&mut km, tab, kb);
    // Subtab keys and view-query keys are active at every drilldown
    // level (Phase 4): pressing the subtab key while drilled in
    // switches the focused subtab. The validator must therefore see
    // them inside every leaf so it can flag collisions with
    // drilldown-level actions.
    push_subtab_keys(&mut km, tab, views);
    push_view_query_keys(&mut km, tab, view);
    push_leaf_column_cursor(&mut km, tab, drilled_profile(), child.column_cursor);
    push_leaf_content_keys(
        &mut km,
        tab,
        drilled_profile(),
        view,
        &path,
        Some(child),
        child.column_cursor,
        kb,
    );
    push_child_actions(&mut km, tab, view, &path, child);
    push_action_chain_claims(
        &mut km,
        tab,
        drilled_profile(),
        view,
        &path,
        Some(child),
        kb,
    );
    out.push(ViewLeafMap {
        view: view.name.clone(),
        child_path: path.clone(),
        keymap: km,
    });

    for nested in &child.children {
        push_child_leaves(out, tab, views, view, &path, nested, kb);
    }
}

fn push_tab_wide(km: &mut KeyMap, tab: &TabRef, kb: &KeyBindingConfig) {
    // Globals — strict overlap with everything.
    for (action, binding) in &kb.global.bindings {
        km.push(KeyClaim::handler(
            binding.clone(),
            KeyScope::Global,
            KeySource::Global(action.clone()),
        ));
    }
    // Common keys actually claimed by adapter panes (not f / / / n / N
    // / q — those are YAML-action keys on adapter tabs). ColumnLeft /
    // ColumnRight are intentionally absent: they live only on leaves
    // that opt in via `column_cursor: true` (see
    // `push_leaf_column_cursor`).
    let adapter_common = [
        CommonAction::ListNext,
        CommonAction::ListPrev,
        CommonAction::ListFirst,
        CommonAction::ListLast,
        CommonAction::ScrollHalfUp,
        CommonAction::ScrollHalfDown,
        CommonAction::ScrollPageUp,
        CommonAction::ScrollPageDown,
    ];
    for action in adapter_common {
        if let Some(binding) = kb.common.get(&action) {
            km.push(KeyClaim::handler(
                binding.clone(),
                KeyScope::Tab(tab.clone()),
                KeySource::Common(action),
            ));
        }
    }
    // ContentAction defaults are NOT pushed tab-wide. They live per
    // leaf so that `child.keybindings: { back: null }`-style disables
    // and `column_cursor: true`-style key reservation can shape the
    // effective binding before the claim is filed
    // (see `push_leaf_content_keys`).

    // Window-leader chord(s).
    for (action, binding) in &kb.window.bindings {
        km.push(KeyClaim::handler(
            binding.clone(),
            KeyScope::Tab(tab.clone()),
            KeySource::Window(action.clone()),
        ));
    }
}

/// Keys reserved by the optional column cursor when `column_cursor:
/// true` is set on a view or child. These keys move the in-pane column
/// cursor at that leaf and are stripped from any ContentAction binding
/// at the same leaf so e.g. `content.back = [backspace, h]` becomes
/// just `[backspace]` while the cursor is live. Hardcoded today; if a
/// per-leaf override ever becomes useful, lift it onto `ViewDef` /
/// `ChildDef`.
pub const COLUMN_CURSOR_LEFT_KEY: &str = "h";
pub const COLUMN_CURSOR_RIGHT_KEY: &str = "l";

fn push_leaf_column_cursor(
    km: &mut KeyMap,
    tab: &TabRef,
    profile: PaneStateProfile,
    column_cursor: bool,
) {
    if !column_cursor {
        return;
    }
    km.push(KeyClaim::handler(
        KeyBinding::new(COLUMN_CURSOR_LEFT_KEY),
        KeyScope::Pane(tab.clone(), profile.clone()),
        KeySource::Common(CommonAction::ColumnLeft),
    ));
    km.push(KeyClaim::handler(
        KeyBinding::new(COLUMN_CURSOR_RIGHT_KEY),
        KeyScope::Pane(tab.clone(), profile),
        KeySource::Common(CommonAction::ColumnRight),
    ));
}

fn push_leaf_content_keys(
    km: &mut KeyMap,
    tab: &TabRef,
    profile: PaneStateProfile,
    view: &ViewDef,
    child_path: &[String],
    child: Option<&ChildDef>,
    column_cursor: bool,
    kb: &KeyBindingConfig,
) {
    // TreeCollapse and TreeCollapseAll live only on the root leaf of a
    // tree-mode view (the runtime gate is `pane.tree.is_some()`, which
    // matches the root pane). Pushed first so the validator catches the
    // same potential `h`-collision the dispatcher would hit at runtime.
    if child.is_none() && view.tree_label.is_some() && !column_cursor {
        if let Some(b) = kb.content.get(&ContentAction::TreeCollapse) {
            km.push(KeyClaim::handler(
                b.clone(),
                KeyScope::Pane(tab.clone(), profile.clone()),
                KeySource::Content(ContentAction::TreeCollapse),
            ));
        }
        if let Some(b) = kb.content.get(&ContentAction::TreeCollapseAll) {
            km.push(KeyClaim::handler(
                b.clone(),
                KeyScope::Pane(tab.clone(), profile.clone()),
                KeySource::Content(ContentAction::TreeCollapseAll),
            ));
        }
        if let Some(b) = kb.content.get(&ContentAction::TreeExpandAll) {
            km.push(KeyClaim::handler(
                b.clone(),
                KeyScope::Pane(tab.clone(), profile.clone()),
                KeySource::Content(ContentAction::TreeExpandAll),
            ));
        }
    }
    for action in [
        ContentAction::Back,
        ContentAction::Open,
        ContentAction::NextPage,
        ContentAction::PrevPage,
        ContentAction::EditQuery,
    ] {
        // `Back` is only meaningful when drilled in (runtime gate:
        // `!nav_stack.is_empty()`, see `ContentView::build_view_claims`).
        // On the root leaf it is a no-op, so it must NOT be claimed there —
        // otherwise its default `backspace` binding would statically collide
        // with `TreeCollapse` (also `backspace`, root-leaf only). Mirroring
        // the runtime gate here keeps the two disjoint by drilldown level.
        if action == ContentAction::Back && child.is_none() {
            continue;
        }
        let (binding, source) = match child.and_then(|c| c.keybindings.get(&action)) {
            // Child sets `action: null` → disabled at this leaf.
            Some(None) => continue,
            // Child sets `action: <binding>` → override.
            Some(Some(b)) => (
                b.clone(),
                KeySource::YamlChildKeybinding {
                    view: view.name.clone(),
                    child_path: child_path.to_vec(),
                    action: action.to_string(),
                },
            ),
            // Otherwise fall back to the global ContentAction binding.
            None => match kb.content.get(&action) {
                Some(b) => (b.clone(), KeySource::Content(action.clone())),
                None => continue,
            },
        };

        let mut keys = binding;
        if column_cursor {
            keys.0
                .retain(|k| k != COLUMN_CURSOR_LEFT_KEY && k != COLUMN_CURSOR_RIGHT_KEY);
        }
        if keys.0.is_empty() {
            continue;
        }
        km.push(KeyClaim::handler(
            keys,
            KeyScope::Pane(tab.clone(), profile.clone()),
            source,
        ));
    }
}

fn push_subtab_keys(km: &mut KeyMap, tab: &TabRef, views: &[ViewDef]) {
    for v in views {
        if let Some(k) = &v.key {
            km.push(KeyClaim::handler(
                KeyBinding::new(k.clone()),
                KeyScope::Tab(tab.clone()),
                KeySource::YamlSubtab {
                    view: v.name.clone(),
                },
            ));
        }
    }
}

fn push_view_query_keys(km: &mut KeyMap, tab: &TabRef, view: &ViewDef) {
    if let Some(query) = &view.query {
        if let Some(menu_key) = &query.menu_key {
            km.push(KeyClaim::handler(
                KeyBinding::new(menu_key.clone()),
                KeyScope::Pane(tab.clone(), PaneStateProfile::Normal { drilldown: None }),
                KeySource::YamlMenuKey {
                    view: view.name.clone(),
                },
            ));
        }
    }
}

fn push_view_actions(km: &mut KeyMap, tab: &TabRef, view: &ViewDef, is_root: bool) {
    let profile = if is_root {
        root_profile()
    } else {
        drilled_profile()
    };
    for action in &view.actions {
        push_action_claims(km, tab, profile.clone(), &view.name, &[], action);
    }
    if let Some(preview) = &view.preview {
        if let Some(k) = &preview.keybinding {
            km.push(KeyClaim::handler(
                KeyBinding::new(k.clone()),
                KeyScope::Pane(tab.clone(), profile),
                KeySource::YamlPreviewKey {
                    view: view.name.clone(),
                    child_path: Vec::new(),
                },
            ));
        }
    }
}

fn push_child_actions(
    km: &mut KeyMap,
    tab: &TabRef,
    view: &ViewDef,
    child_path: &[String],
    child: &ChildDef,
) {
    let profile = drilled_profile();
    for action in &child.actions {
        push_action_claims(km, tab, profile.clone(), &view.name, child_path, action);
    }
    if let Some(preview) = &child.preview {
        if let Some(k) = &preview.keybinding {
            km.push(KeyClaim::handler(
                KeyBinding::new(k.clone()),
                KeyScope::Pane(tab.clone(), profile),
                KeySource::YamlPreviewKey {
                    view: view.name.clone(),
                    child_path: child_path.to_vec(),
                },
            ));
        }
    }
}

fn push_action_claims(
    km: &mut KeyMap,
    tab: &TabRef,
    profile: PaneStateProfile,
    view: &str,
    child_path: &[String],
    action: &crate::config::view_config::ActionDef,
) {
    km.push(KeyClaim::handler(
        KeyBinding::new(action.key.clone()),
        KeyScope::Pane(tab.clone(), profile.clone()),
        KeySource::YamlAction {
            view: view.to_string(),
            child_path: child_path.to_vec(),
            name: action.name.clone(),
        },
    ));
    if let Some(search) = &action.search {
        if let Some(k) = &search.next_key {
            km.push(KeyClaim::handler(
                KeyBinding::new(k.clone()),
                KeyScope::Pane(tab.clone(), profile.clone()),
                KeySource::PaneSearchJump {
                    direction: SearchJump::Next,
                },
            ));
        }
        if let Some(k) = &search.prev_key {
            km.push(KeyClaim::handler(
                KeyBinding::new(k.clone()),
                KeyScope::Pane(tab.clone(), profile),
                KeySource::PaneSearchJump {
                    direction: SearchJump::Prev,
                },
            ));
        }
    }
}

/// Push one claim per effective `action_chains:` entry visible in this
/// leaf. "Effective" walks `child.action_chains → view.action_chains →
/// kb.action_chains` and respects `None` (disabled) at the innermost
/// scope — the same walk that
/// [`crate::action::resolve_chain_in_scopes`] uses at dispatch time.
///
/// Each claim is scoped to the leaf (`Pane(tab, profile)`) so a chain
/// only conflicts with the bindings actually visible at the same
/// drilldown level, and its [`KeySource::AppActionChain`] carries the
/// originating `scope_path` so conflict messages point users at the
/// right YAML location.
fn push_action_chain_claims(
    km: &mut KeyMap,
    tab: &TabRef,
    profile: PaneStateProfile,
    view: &ViewDef,
    child_path: &[String],
    child: Option<&ChildDef>,
    kb: &KeyBindingConfig,
) {
    let mut scopes: Vec<&crate::action::ActionChains> = Vec::new();
    if let Some(c) = child {
        scopes.push(&c.action_chains);
    }
    scopes.push(&view.action_chains);
    scopes.push(&kb.action_chains);
    let effective = crate::action::effective_chains_in_scopes(&scopes);
    for (key_str, (scope_idx, _chain)) in effective {
        // Map scope_idx back to a human-readable scope_path:
        // - innermost (idx 0) is the child if present, otherwise the view
        // - middle (idx 1) is the view when a child is present
        // - last is the global keybindings map
        let scope_path: Vec<String> = if child.is_some() {
            match scope_idx {
                0 => {
                    let mut p = vec![view.name.clone()];
                    p.extend(child_path.iter().cloned());
                    p
                }
                1 => vec![view.name.clone()],
                _ => Vec::new(),
            }
        } else {
            match scope_idx {
                0 => vec![view.name.clone()],
                _ => Vec::new(),
            }
        };
        km.push(KeyClaim::handler(
            KeyBinding::new(key_str),
            KeyScope::Pane(tab.clone(), profile.clone()),
            KeySource::AppActionChain { scope_path },
        ));
    }
}

fn root_profile() -> PaneStateProfile {
    PaneStateProfile::Normal {
        drilldown: Some(false),
    }
}

fn drilled_profile() -> PaneStateProfile {
    PaneStateProfile::Normal {
        drilldown: Some(true),
    }
}

/// Check whether binding `shortcut` to the saved query `query_name`
/// would collide with any other key handler in its tab.
///
/// Saved-query shortcut claims are tab-wide and active at every
/// drill-down leaf (see `ContentView::build_view_claims`), so the
/// shortcut must be free in *every* leaf. This reuses the same leaf
/// maps as the config-load validator (globals, common navigation,
/// window chords, subtab keys, query menu keys, YAML actions, preview
/// keys, action chains) and adds the claim sources that only exist at
/// runtime or outside the static builder:
///
/// - the other saved-query shortcuts already bound in this tab,
/// - the per-node YAML `shortcuts:` maps (dispatched *before* the
///   view-claim layer, so a colliding saved-query shortcut would be
///   dead rather than shadowing — a misconfiguration either way),
/// - the full `ContentAction` section (the pane dispatches chords like
///   `zg`/`zm` directly; only a subset is in the static leaf maps).
///
/// Chord *prefixes* count as conflicts too: claims dispatch before the
/// pane's pending-chord handling, so a shortcut `w` would swallow the
/// window-leader chord `wv` and a shortcut `z` would shadow `zg`.
///
/// Returns a human-readable description of the first conflicting
/// binding, or `None` if the shortcut is free. Callers gate on this
/// both when the user assigns a shortcut (reject + re-prompt) and when
/// shortcuts load from the `query_shortcut` table (warn — rows written
/// externally or predating a config change would otherwise shadow keys
/// silently).
pub fn saved_query_shortcut_conflict(
    tab_name: &str,
    views: &[ViewDef],
    kb: &KeyBindingConfig,
    query_name: &str,
    shortcut: &str,
    bound_saved_queries: &[(String, String)],
) -> Option<String> {
    // Same claim layer first: another saved query already wearing the
    // key. Rebinding the same query to its own key is fine.
    if let Some((name, _)) = bound_saved_queries
        .iter()
        .find(|(name, sc)| name != query_name && KeyBinding::new(sc.clone()).matches(shortcut))
    {
        return Some(format!("saved query '{name}'"));
    }

    let tab = TabRef::new(tab_name);
    for leaf in build_leaf_maps(&tab, views, kb) {
        for claim in &leaf.keymap.claims {
            if claim.kind != KeyClaimKind::Handler {
                continue;
            }
            if claim.key.matches(shortcut) || claim.key.is_prefix(shortcut) {
                return Some(claim.source.human());
            }
        }
    }

    if let Some(hit) = node_shortcut_conflict(views, shortcut) {
        return Some(hit);
    }

    for (action, binding) in &kb.content.bindings {
        if binding.matches(shortcut) || binding.is_prefix(shortcut) {
            return Some(KeySource::Content(action.clone()).human());
        }
    }
    None
}

/// Find a per-node YAML `shortcuts:` entry (view or any drill-down
/// child) claiming `shortcut`. These maps are keyed by single chars,
/// so modifier shortcuts can never collide here.
fn node_shortcut_conflict(views: &[ViewDef], shortcut: &str) -> Option<String> {
    let mut chars = shortcut.chars();
    let ch = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    for view in views {
        let mut path = Vec::new();
        if let Some(hit) =
            find_node_shortcut(&view.name, &mut path, &view.shortcuts, &view.children, ch)
        {
            return Some(hit);
        }
    }
    None
}

fn find_node_shortcut(
    view_name: &str,
    child_path: &mut Vec<String>,
    shortcuts: &std::collections::HashMap<char, crate::config::view_config::ShortcutDef>,
    children: &[ChildDef],
    ch: char,
) -> Option<String> {
    if let Some(action) = shortcuts.get(&ch).map(|sc| sc.action()) {
        return Some(if child_path.is_empty() {
            format!("views.{view_name}.shortcuts[{action}]")
        } else {
            format!(
                "views.{view_name}.children.{}.shortcuts[{action}]",
                child_path.join(".")
            )
        });
    }
    for child in children {
        child_path.push(child.name.clone());
        if let Some(hit) =
            find_node_shortcut(view_name, child_path, &child.shortcuts, &child.children, ch)
        {
            return Some(hit);
        }
        child_path.pop();
    }
    None
}

/// Run the validator on every leaf of `config` and collect a flat list
/// of human-readable error strings. Each error names both colliding
/// sources so the user can locate them in their YAML.
pub fn validate_view_file(config: &ViewFileConfig, kb: &KeyBindingConfig) -> Vec<String> {
    let mut errors = Vec::new();
    for leaf in build_view_leaf_maps(config, kb) {
        let where_ = if leaf.child_path.is_empty() {
            format!("views.{}", leaf.view)
        } else {
            format!("views.{}.children.{}", leaf.view, leaf.child_path.join("."))
        };
        for c in leaf.keymap.validate() {
            errors.push(format!("{where_}: {}", c.human()));
        }
    }
    errors
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(name: &str) -> TabRef {
        TabRef::new(name)
    }

    fn root() -> PaneStateProfile {
        PaneStateProfile::Normal {
            drilldown: Some(false),
        }
    }

    fn drilled() -> PaneStateProfile {
        PaneStateProfile::Normal {
            drilldown: Some(true),
        }
    }

    fn yaml_action(view: &str, name: &str, key: &str) -> KeyClaim {
        KeyClaim::handler(
            KeyBinding::new(key),
            KeyScope::Pane(
                tab("postgres"),
                PaneStateProfile::Normal { drilldown: None },
            ),
            KeySource::YamlAction {
                view: view.into(),
                child_path: Vec::new(),
                name: name.into(),
            },
        )
    }

    #[test]
    fn no_conflict_in_disjoint_tabs() {
        let mut km = KeyMap::new();
        km.push(KeyClaim::handler(
            KeyBinding::new("f"),
            KeyScope::Tab(tab("postgres")),
            KeySource::YamlAction {
                view: "tables".into(),
                child_path: Vec::new(),
                name: "fuzzy".into(),
            },
        ));
        km.push(KeyClaim::handler(
            KeyBinding::new("f"),
            KeyScope::Tab(tab("jira")),
            KeySource::YamlAction {
                view: "tickets".into(),
                child_path: Vec::new(),
                name: "fuzzy".into(),
            },
        ));
        assert!(km.validate().is_empty());
    }

    #[test]
    fn conflict_two_yaml_actions_in_same_view() {
        let mut km = KeyMap::new();
        km.push(yaml_action("tables", "fuzzy", "f"));
        km.push(yaml_action("tables", "find", "f"));
        let cs = km.validate();
        assert_eq!(cs.len(), 1, "expected exactly one conflict, got {cs:?}");
        assert_eq!(cs[0].keys, vec!["f".to_string()]);
    }

    #[test]
    fn conflict_yaml_action_vs_global() {
        let mut km = KeyMap::new();
        km.push(KeyClaim::handler(
            KeyBinding::new("ctrl+c"),
            KeyScope::Global,
            KeySource::Global(GlobalAction::Quit),
        ));
        km.push(yaml_action("tables", "boom", "ctrl+c"));
        let cs = km.validate();
        assert_eq!(cs.len(), 1);
        assert!(cs[0].human().contains("global.quit"));
    }

    #[test]
    fn conflict_yaml_action_vs_content_action() {
        // ContentAction::Open defaults to "enter" — but if a user binds
        // an adapter action to "enter" too, that's a conflict.
        let mut km = KeyMap::new();
        km.push(KeyClaim::handler(
            KeyBinding::new("enter"),
            KeyScope::Pane(
                tab("postgres"),
                PaneStateProfile::Normal { drilldown: None },
            ),
            KeySource::Content(ContentAction::Open),
        ));
        km.push(yaml_action("tables", "open_alt", "enter"));
        assert_eq!(km.validate().len(), 1);
    }

    #[test]
    fn no_conflict_with_swallow() {
        // A fuzzy-input pane swallows "f" while it's active, but that
        // shouldn't prevent another pane state from binding "f" as a
        // handler.
        let mut km = KeyMap::new();
        km.push(KeyClaim::swallow(
            KeyBinding::new("f"),
            KeyScope::Pane(
                tab("postgres"),
                PaneStateProfile::InputMode(InputMode::Fuzzy),
            ),
            KeySource::Common(CommonAction::FuzzyFilterOpen),
        ));
        km.push(KeyClaim::handler(
            KeyBinding::new("f"),
            KeyScope::Pane(
                tab("postgres"),
                PaneStateProfile::Normal { drilldown: None },
            ),
            KeySource::YamlAction {
                view: "tables".into(),
                child_path: Vec::new(),
                name: "fuzzy".into(),
            },
        ));
        assert!(km.validate().is_empty());
    }

    #[test]
    fn conflict_root_only_vs_drilldown_only_disjoint() {
        // Two handlers that are mutually exclusive by drilldown level
        // should not conflict.
        let mut km = KeyMap::new();
        km.push(KeyClaim::handler(
            KeyBinding::new("t"),
            KeyScope::Pane(tab("postgres"), root()),
            KeySource::YamlSubtab {
                view: "tables".into(),
            },
        ));
        km.push(KeyClaim::handler(
            KeyBinding::new("t"),
            KeyScope::Pane(tab("postgres"), drilled()),
            KeySource::YamlAction {
                view: "tickets".into(),
                child_path: vec!["Comments".into()],
                name: "tag".into(),
            },
        ));
        assert!(km.validate().is_empty());
    }

    #[test]
    fn conflict_subtab_vs_action_at_root() {
        // A subtab key (root-level) collides with a YAML root-level action.
        let mut km = KeyMap::new();
        km.push(KeyClaim::handler(
            KeyBinding::new("t"),
            KeyScope::Pane(tab("postgres"), root()),
            KeySource::YamlSubtab {
                view: "tables".into(),
            },
        ));
        km.push(KeyClaim::handler(
            KeyBinding::new("t"),
            KeyScope::Pane(
                tab("postgres"),
                PaneStateProfile::Normal { drilldown: None },
            ),
            KeySource::YamlAction {
                view: "databases".into(),
                child_path: Vec::new(),
                name: "tag".into(),
            },
        ));
        let cs = km.validate();
        assert_eq!(cs.len(), 1);
        // None-drilldown overlaps with Some(false) (root).
        assert!(
            cs[0].human().contains("yamlsubtab")
                || cs[0].human().contains("YamlSubtab")
                || cs[0].human().contains("views.tables.key")
        );
    }

    #[test]
    fn multi_key_binding_conflicts_on_any_overlap() {
        let mut km = KeyMap::new();
        km.push(KeyClaim::handler(
            KeyBinding::multi(vec!["j", "down"]),
            KeyScope::Tab(tab("postgres")),
            KeySource::Common(CommonAction::ListNext),
        ));
        km.push(KeyClaim::handler(
            KeyBinding::new("j"),
            KeyScope::Tab(tab("postgres")),
            KeySource::YamlAction {
                view: "tables".into(),
                child_path: Vec::new(),
                name: "jump".into(),
            },
        ));
        let cs = km.validate();
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].keys, vec!["j".to_string()]);
    }

    #[test]
    fn pane_overlaps_with_tab_scope() {
        // Pane(tab=p, _) ∩ Tab(p) is non-empty.
        let p = KeyScope::Pane(tab("postgres"), root());
        let t = KeyScope::Tab(tab("postgres"));
        assert!(p.overlaps_with(&t));
        assert!(t.overlaps_with(&p));
    }

    #[test]
    fn input_mode_does_not_overlap_with_normal() {
        let im = PaneStateProfile::InputMode(InputMode::Fuzzy);
        let nm = PaneStateProfile::Normal { drilldown: None };
        assert!(!profile_overlaps(&im, &nm));
        assert!(!profile_overlaps(&nm, &im));
    }

    fn yaml_str(s: &str) -> ViewFileConfig {
        serde_yaml::from_str(s).expect("yaml parses")
    }

    /// User-visible regression: `content.back = [backspace, h]` plus
    /// `content.open = [enter, l]` overrides used to be flagged as
    /// conflicts against the default `common.column_left = [left, h]` /
    /// `common.column_right = [right, l]`. With the new model both
    /// defaults are gone and the validator stays quiet on tabs that
    /// don't opt into the column cursor.
    #[test]
    fn no_conflict_for_back_open_overrides_without_column_cursor() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: tickets
    node_type: t
    actions:
      - { name: edit, key: e, type: edit, id: edit_full }
    children:
      - name: Comments
        node_type: c
        actions:
          - { name: add, key: a, type: create, id: create_comment }
"#;
        let mut kb = KeyBindingConfig::default();
        kb.content.bindings.insert(
            ContentAction::Back,
            KeyBinding::multi(vec!["backspace", "h"]),
        );
        kb.content
            .bindings
            .insert(ContentAction::Open, KeyBinding::multi(vec!["enter", "l"]));
        let cfg = yaml_str(yaml);
        let errs = validate_view_file(&cfg, &kb);
        assert!(errs.is_empty(), "unexpected errors: {errs:?}");
    }

    /// `column_cursor: true` reserves h/l for ColumnLeft/Right at that
    /// leaf. The same leaf must still accept `content.back = [backspace,
    /// h]` (with h stripped) without producing a conflict.
    #[test]
    fn column_cursor_reserves_h_l_without_conflict() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: tables
    node_type: t
    children:
      - name: Rows
        node_type: r
        column_cursor: true
"#;
        let mut kb = KeyBindingConfig::default();
        kb.content.bindings.insert(
            ContentAction::Back,
            KeyBinding::multi(vec!["backspace", "h"]),
        );
        kb.content
            .bindings
            .insert(ContentAction::Open, KeyBinding::multi(vec!["enter", "l"]));
        let cfg = yaml_str(yaml);
        let errs = validate_view_file(&cfg, &kb);
        assert!(errs.is_empty(), "unexpected errors: {errs:?}");

        // Drill into the Rows leaf and verify both ColumnLeft and a
        // stripped back claim are present.
        let leaves = build_view_leaf_maps(&cfg, &kb);
        let rows = leaves
            .iter()
            .find(|l| l.child_path == ["Rows".to_string()])
            .expect("Rows leaf present");
        let has_column_left = rows.keymap.claims.iter().any(|c| {
            matches!(c.source, KeySource::Common(CommonAction::ColumnLeft)) && c.key.matches("h")
        });
        let has_column_right = rows.keymap.claims.iter().any(|c| {
            matches!(c.source, KeySource::Common(CommonAction::ColumnRight)) && c.key.matches("l")
        });
        let back_keys: Vec<String> = rows
            .keymap
            .claims
            .iter()
            .find(|c| matches!(c.source, KeySource::Content(ContentAction::Back)))
            .map(|c| c.key.0.clone())
            .unwrap_or_default();
        assert!(has_column_left, "ColumnLeft claim missing on Rows");
        assert!(has_column_right, "ColumnRight claim missing on Rows");
        assert!(
            back_keys.iter().any(|k| k == "backspace") && !back_keys.iter().any(|k| k == "h"),
            "back keys at Rows should keep backspace and drop h, got {back_keys:?}"
        );
    }

    /// A YAML action that uses the column-cursor key at a column-cursor
    /// leaf must still conflict — the reservation protects against
    /// silent shadowing only between ColumnLeft/Right and the standard
    /// content navigation.
    #[test]
    fn column_cursor_still_conflicts_with_explicit_yaml_action() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: tables
    node_type: t
    children:
      - name: Rows
        node_type: r
        column_cursor: true
        actions:
          - { name: shadow, key: h, type: custom, id: shadow }
"#;
        let cfg = yaml_str(yaml);
        let errs = validate_view_file(&cfg, &KeyBindingConfig::default());
        assert!(
            errs.iter()
                .any(|e| e.contains("Rows") && e.contains("\"h\"")),
            "expected a Rows / 'h' conflict, got: {errs:?}"
        );
    }

    /// Phase 4: subtab keys are active at every drilldown level. The
    /// validator must flag a collision between a subtab key and a
    /// drilldown-level action.
    #[test]
    fn validate_view_file_flags_subtab_vs_drilldown_action() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: tables
    node_type: t
    key: t
    children:
      - name: Rows
        node_type: r
        actions:
          - { name: tag, key: t, type: custom, id: tag }
"#;
        let cfg = yaml_str(yaml);
        let errs = validate_view_file(&cfg, &KeyBindingConfig::default());
        assert!(
            errs.iter()
                .any(|e| e.contains("Rows") && e.contains("\"t\"")),
            "expected a Rows / 't' subtab conflict, got: {errs:?}"
        );
    }

    /// Phase 4: view-query `menu_key` is active at every drilldown
    /// level of that view. Collision with a drilldown action must be
    /// flagged.
    #[test]
    fn validate_view_file_flags_menu_key_vs_drilldown_action() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: tables
    node_type: t
    query:
      menu_key: q
    children:
      - name: Rows
        node_type: r
        actions:
          - { name: shadow, key: q, type: custom, id: shadow }
"#;
        let cfg = yaml_str(yaml);
        let errs = validate_view_file(&cfg, &KeyBindingConfig::default());
        assert!(
            errs.iter()
                .any(|e| e.contains("Rows") && e.contains("\"q\"")),
            "expected a Rows / 'q' menu_key conflict, got: {errs:?}"
        );
    }

    /// Phase 5: a global `action_chains:` binding collides with a
    /// YAML-action key declared on a drilldown level. The validator
    /// must flag both so the user knows the chain will shadow the
    /// action (or vice versa, depending on dispatch order).
    #[test]
    fn validate_view_file_flags_global_action_chain_vs_drilldown_action() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: tables
    node_type: t
    children:
      - name: Rows
        node_type: r
        actions:
          - { name: shadow, key: "ctrl+n", type: custom, id: shadow }
"#;
        let cfg = yaml_str(yaml);
        let mut kb = KeyBindingConfig::default();
        let chain = vec![crate::action::Action::Common(CommonAction::ListNext)];
        kb.action_chains.0.insert("ctrl+n".into(), Some(chain));
        let errs = validate_view_file(&cfg, &kb);
        assert!(
            errs.iter()
                .any(|e| e.contains("Rows") && e.contains("ctrl+n")),
            "expected a Rows / 'ctrl+n' chain conflict, got: {errs:?}"
        );
    }

    /// Phase 5: a view-scoped `action_chains:` entry collides with a
    /// common-key binding (here `j` for ListNext). The validator must
    /// see both because adapter Common keys are pushed tab-wide and the
    /// chain is pushed per leaf.
    #[test]
    fn validate_view_file_flags_view_action_chain_vs_common_key() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: tables
    node_type: t
    action_chains:
      "j": [common.list_first]
"#;
        let cfg = yaml_str(yaml);
        let errs = validate_view_file(&cfg, &KeyBindingConfig::default());
        assert!(
            errs.iter()
                .any(|e| e.contains("tables") && e.contains("\"j\"")),
            "expected a tables / 'j' chain-vs-common conflict, got: {errs:?}"
        );
    }

    /// Phase 5: a child-level `action_chains: { ctrl+n: null }` must
    /// disable a higher-scope chain at that leaf — no claim is pushed,
    /// so a drilldown action on the same key does NOT conflict.
    #[test]
    fn child_null_action_chain_disables_global_chain_in_leaf() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: tables
    node_type: t
    children:
      - name: Rows
        node_type: r
        action_chains:
          "ctrl+n": ~
        actions:
          - { name: shadow, key: "ctrl+n", type: custom, id: shadow }
"#;
        let cfg = yaml_str(yaml);
        let mut kb = KeyBindingConfig::default();
        let chain = vec![crate::action::Action::Common(CommonAction::ListNext)];
        kb.action_chains.0.insert("ctrl+n".into(), Some(chain));
        let errs = validate_view_file(&cfg, &kb);
        // The shadow action keeps living — but no chain claim is pushed
        // at this leaf, so no AppActionChain conflict appears.
        let chain_err = errs
            .iter()
            .find(|e| e.contains("Rows") && e.contains("ctrl+n") && e.contains("action_chains"));
        assert!(
            chain_err.is_none(),
            "child-level `null` must suppress chain claim at the leaf, got: {errs:?}"
        );
    }

    /// Child override `back: null` keeps disabling backspace + h at the
    /// leaf, even without column_cursor on. The validator must not push
    /// a Content::Back claim for that leaf.
    #[test]
    fn back_null_override_disables_back_claim() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: tables
    node_type: t
    children:
      - name: Rows
        node_type: r
        keybindings:
          back: null
"#;
        let cfg = yaml_str(yaml);
        let leaves = build_view_leaf_maps(&cfg, &KeyBindingConfig::default());
        let rows = leaves
            .iter()
            .find(|l| l.child_path == ["Rows".to_string()])
            .expect("Rows leaf present");
        let has_back = rows
            .keymap
            .claims
            .iter()
            .any(|c| matches!(c.source, KeySource::Content(ContentAction::Back)));
        assert!(!has_back, "Back claim should be suppressed by `back: null`");
    }

    // ── saved_query_shortcut_conflict ────────────────────────────────

    /// Fixture tab for the saved-query shortcut checks: two subtabs
    /// (keys a / v), a query menu key, YAML actions, per-node
    /// `shortcuts:` at root and child level.
    fn sq_views() -> Vec<ViewDef> {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: main
    node_type: t
    key: a
    query:
      menu_key: q
    actions:
      - { name: fuzzy, key: f, type: fuzzy_filter }
    shortcuts:
      d: delete
    children:
      - name: Rows
        node_type: r
        shortcuts:
          R: restore
  - name: second
    node_type: t
    key: v
"#;
        yaml_str(yaml).views
    }

    fn sq_conflict(shortcut: &str, bound: &[(String, String)]) -> Option<String> {
        saved_query_shortcut_conflict(
            "T",
            &sq_views(),
            &KeyBindingConfig::default(),
            "candidate",
            shortcut,
            bound,
        )
    }

    #[test]
    fn sq_conflict_flags_subtab_key() {
        let hit = sq_conflict("v", &[]).expect("subtab key must conflict");
        assert!(hit.contains("views.second.key"), "got: {hit}");
    }

    #[test]
    fn sq_conflict_flags_query_menu_key() {
        let hit = sq_conflict("q", &[]).expect("menu key must conflict");
        assert!(hit.contains("query.menu_key"), "got: {hit}");
    }

    #[test]
    fn sq_conflict_flags_yaml_action_key() {
        let hit = sq_conflict("f", &[]).expect("action key must conflict");
        assert!(hit.contains("actions[fuzzy]"), "got: {hit}");
    }

    #[test]
    fn sq_conflict_flags_common_navigation_key() {
        let hit = sq_conflict("j", &[]).expect("nav key must conflict");
        assert!(hit.contains("common.list_next"), "got: {hit}");
    }

    /// Window chords dispatch before the view-claim layer, so a
    /// saved-query shortcut equal to the chord *leader* would be dead
    /// (the leader swallows it). `is_prefix` must catch that.
    #[test]
    fn sq_conflict_flags_window_leader_prefix() {
        let hit = sq_conflict("w", &[]).expect("window leader must conflict");
        assert!(hit.contains("window."), "got: {hit}");
    }

    /// `zg`/`zm`/`zt` are dispatched by the pane outside the static
    /// leaf maps — the full content-section scan must still flag the
    /// chord prefix `z`.
    #[test]
    fn sq_conflict_flags_content_chord_prefix() {
        let hit = sq_conflict("z", &[]).expect("z-chord prefix must conflict");
        assert!(hit.contains("content."), "got: {hit}");
    }

    #[test]
    fn sq_conflict_flags_node_shortcut_at_root_and_child() {
        let root = sq_conflict("d", &[]).expect("root node shortcut must conflict");
        assert!(root.contains("views.main.shortcuts[delete]"), "got: {root}");
        let child = sq_conflict("R", &[]).expect("child node shortcut must conflict");
        assert!(
            child.contains("views.main.children.Rows.shortcuts[restore]"),
            "got: {child}"
        );
    }

    #[test]
    fn sq_conflict_flags_other_saved_query() {
        let bound = vec![("two months".to_string(), "m".to_string())];
        let hit = sq_conflict("m", &bound).expect("other saved query must conflict");
        assert!(hit.contains("saved query 'two months'"), "got: {hit}");
    }

    #[test]
    fn sq_conflict_allows_rebinding_same_query() {
        let bound = vec![("candidate".to_string(), "m".to_string())];
        assert_eq!(sq_conflict("m", &bound), None);
    }

    #[test]
    fn sq_conflict_allows_free_keys() {
        assert_eq!(sq_conflict("m", &[]), None);
        assert_eq!(sq_conflict("ctrl+o", &[]), None);
    }
}
