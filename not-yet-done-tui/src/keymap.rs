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
    CommonAction, ContentAction, GlobalAction, KeyBinding, KeyBindingConfig, WindowAction,
    binding_steps,
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
    /// Shortcut bound to a per-node script (per `query_shortcut` row
    /// scoped to `<adapter>/<inst>/<node_id>`, e.g.
    /// `postgres/<inst>/<db>/schemas/<schema>/tables/<table>`). Live only
    /// while the focused pane has that exact node in scope (selected item
    /// on a `node_scripts: true` level, or parent of a drilled pane).
    NodeScriptShortcut {
        node_id: String,
        script: String,
    },
    /// Shortcut bound to a `:script`-menu script (per `query_shortcut` row
    /// scoped to `script:<tab>/<view_path…>`). Live only while the focused
    /// pane's level offers a `type: script` action — i.e. the same level the
    /// menu's scripts directory is derived from. `name` is the script's
    /// filename; the App rebuilds the run context at dispatch time.
    ScriptShortcut {
        scope: String,
        name: String,
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
    /// `card.key` on a YAML view or child — toggles card mode on that level.
    /// Claimed statically like the preview key so a collision with an
    /// `actions:` key (or a chord whose prefix is taken) is reported at
    /// config load instead of silently losing the toggle at runtime.
    YamlCardKey {
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
    /// the underlying search has matches. `view` / `child_path` / `action`
    /// identify the owning search action so the editor can route to its
    /// `search.next_key` / `search.prev_key` in the view file. They are
    /// empty when the owning action is not known (a runtime-only claim that
    /// predates the identity being stashed) — such a claim stays read-only.
    PaneSearchJump {
        view: String,
        child_path: Vec<String>,
        action: String,
        direction: SearchJump,
    },
    /// User-defined `action_chains:` entry. `scope_path` identifies the
    /// scope the chain was declared at:
    /// - `[]` — global `keybindings.action_chains`
    /// - `[view]` — `views[*].action_chains`
    /// - `[view, child, ...]` — `children[*].action_chains` somewhere
    ///   in the drill-down tree of `view`
    ///
    /// `key` is the entry's key chord (the map key) — the binding itself,
    /// so the editor drops the whole entry to free it.
    AppActionChain {
        scope_path: Vec<String>,
        key: String,
    },
    /// Top-level tab-switch shortcut, identified by the tab's display name
    /// (`tab.name`). Its binding lives in that view file's `tab.key`; when
    /// that is absent the tab falls back to its positional autonumber digit
    /// (`1`..`9`, then `0`). Editable from the shortcut menu; Global scope.
    TabSwitch {
        tab: String,
    },
    /// A per-node `shortcuts:` entry (a single key mapped to an adapter
    /// action verb, e.g. `s: toggle-tracking`). Lives in the view file's
    /// `shortcuts:` map at the view (`child_path` empty) or a drill-down
    /// child. `key` is the single-character surface form; `action` is the
    /// verb, kept for display. The map key *is* the binding, so the editor
    /// can only drop the whole entry (not rebind it in place).
    NodeShortcut {
        view: String,
        child_path: Vec<String>,
        key: String,
        action: String,
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
            Self::NodeScriptShortcut { node_id, script } => {
                format!("node.script[{node_id}/{script}]")
            }
            Self::ScriptShortcut { scope, name } => {
                format!("script[{scope}/{name}]")
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
            Self::YamlCardKey { view, child_path } => {
                if child_path.is_empty() {
                    format!("views.{view}.card.key")
                } else {
                    format!("views.{view}.children.{}.card.key", child_path.join("."))
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
            Self::PaneSearchJump { direction, .. } => match direction {
                SearchJump::Next => "pane.search_next".into(),
                SearchJump::Prev => "pane.search_prev".into(),
            },
            Self::AppActionChain { scope_path, key } => {
                if scope_path.is_empty() {
                    format!("action_chains[{key}]")
                } else {
                    format!("action_chains.{}[{key}]", scope_path.join("."))
                }
            }
            Self::TabSwitch { tab } => format!("tab[{tab}].key"),
            Self::NodeShortcut {
                view,
                child_path,
                key,
                ..
            } => {
                if child_path.is_empty() {
                    format!("views.{view}.shortcuts[{key}]")
                } else {
                    format!(
                        "views.{view}.children.{}.shortcuts[{key}]",
                        child_path.join(".")
                    )
                }
            }
        }
    }

    /// True for the four built-in `tui.yaml` sections (global / common /
    /// content / window), whose bindings have a compiled-in default that
    /// [`crate::config::keybinding_edit::remove_binding`] can restore.
    pub fn is_builtin(&self) -> bool {
        matches!(
            self,
            Self::Global(_) | Self::Common(_) | Self::Content(_) | Self::Window(_)
        )
    }

    /// True when "restore default" (Ctrl+R in the shortcut menu) is
    /// meaningful: the built-in sections *plus* tab-switch keys, whose
    /// compiled-in default is the positional autonumber digit — recovered
    /// by removing the `tab.key` override. View actions and DB/script
    /// sources have no default, so restore is suppressed for them.
    pub fn has_compiled_default(&self) -> bool {
        self.is_builtin() || matches!(self, Self::TabSwitch { .. })
    }

    /// The subtab (`views[*]`) this shortcut is *specific to*, if any.
    ///
    /// Only one subtab of an adapter tab is foregrounded at a time, so two
    /// shortcuts that each belong to a *different* subtab can never fire for
    /// the same key press — they don't conflict. The [`KeyScope::Pane`] scope
    /// only tracks the tab (not the subtab), so the conflict check uses this
    /// to tell sibling subtabs apart. Returns `None` for tab-wide/global
    /// sources (built-ins, the subtab-switch keys themselves, tab switches,
    /// DB/script scopes) — those apply across subtabs and must still collide.
    pub fn subtab_view(&self) -> Option<&str> {
        match self {
            Self::YamlAction { view, .. }
            | Self::SavedQueryShortcut { view, .. }
            | Self::YamlMenuKey { view }
            | Self::YamlPreviewKey { view, .. }
            | Self::YamlChildKeybinding { view, .. }
            | Self::PaneSearchJump { view, .. }
            | Self::NodeShortcut { view, .. } => Some(view.as_str()),
            _ => None,
        }
    }

    /// A friendly, human-facing action name for the shortcut menu — as
    /// opposed to [`human`], which yields the diagnostic config path used
    /// in conflict messages. YAML/query/script sources use their declared
    /// name; built-ins are the title-cased action identifier.
    ///
    /// [`human`]: KeySource::human
    pub fn action_name(&self) -> String {
        match self {
            Self::Global(a) => title_case(&a.to_string()),
            Self::Common(a) => title_case(&a.to_string()),
            Self::Content(a) => title_case(&a.to_string()),
            Self::Window(a) => title_case(&a.to_string()),
            Self::YamlAction { name, .. } => name.clone(),
            Self::SavedQueryShortcut { name, .. } => format!("{name} (saved query)"),
            Self::NodeScriptShortcut { script, .. } => format!("{script} (script)"),
            Self::ScriptShortcut { name, .. } => format!("{name} (script)"),
            Self::YamlMenuKey { .. } => "Saved-query menu".into(),
            Self::YamlPreviewKey { .. } => "Toggle preview".into(),
            Self::YamlCardKey { .. } => "Toggle card mode".into(),
            Self::YamlSubtab { view } => format!("Switch to {view}"),
            Self::YamlChildKeybinding { action, .. } => title_case(action),
            Self::PaneSearchJump { direction, .. } => match direction {
                SearchJump::Next => "Search next".into(),
                SearchJump::Prev => "Search prev".into(),
            },
            Self::AppActionChain { .. } => "Action chain".into(),
            Self::TabSwitch { tab } => format!("Switch to {tab}"),
            Self::NodeShortcut { action, .. } => title_case(action),
        }
    }
}

/// Turns a `snake_case` identifier into a `Title case` label: underscores
/// become spaces and the first letter is capitalised (`tab_set_popup` →
/// `Tab set popup`).
fn title_case(snake: &str) -> String {
    let spaced = snake.replace('_', " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
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

    /// Honour `actions: [{ force: true }]`: drop the *built-in* claims
    /// (global hotkeys, common fallbacks, window-leader chords, content
    /// actions) on any key in `keys`, so a YAML action can deliberately
    /// take over that key without the validator reporting a collision.
    ///
    /// Only built-in claims are removed — a key fought over by two YAML
    /// actions (or a YAML action vs. a saved-query/script shortcut) is a
    /// genuine config error and stays flagged. For multi-key built-in
    /// bindings (e.g. `down`/`j`) only the forced key is stripped; the
    /// claim survives with its remaining keys.
    ///
    /// Stripping is prefix-aware, mirroring [`Self::validate`]: forcing `c`
    /// also takes the built-in `c c` chord out of the way, because a YAML
    /// action on the leader key makes the chord unreachable either way.
    pub fn force_override_keys(&mut self, keys: &[String]) {
        if keys.is_empty() {
            return;
        }
        let forced: Vec<Vec<String>> = keys.iter().map(|k| binding_steps(k)).collect();
        self.claims.retain_mut(|c| {
            let is_builtin = matches!(
                c.source,
                KeySource::Global(_)
                    | KeySource::Common(_)
                    | KeySource::Window(_)
                    | KeySource::Content(_)
            );
            if !is_builtin {
                return true;
            }
            c.key.0.retain(|k| {
                let steps = binding_steps(k);
                !forced.iter().any(|f| {
                    let n = f.len().min(steps.len());
                    f[..n] == steps[..n]
                })
            });
            !c.key.0.is_empty()
        });
    }

    /// Find every pair of `Handler` claims that share at least one key
    /// and whose scopes overlap. Reported once per pair.
    ///
    /// "Share" includes chord *prefixes*, not just identical strings: the
    /// dispatcher stashes a pending prefix before it resolves single keys,
    /// so a plain `c` next to a `c c` chord makes one of the two
    /// unreachable — exactly the silent shadowing this validator exists to
    /// prevent.
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
                    .filter(|k| {
                        let ka = binding_steps(k);
                        b.key.0.iter().any(|kb| {
                            let kb = binding_steps(kb);
                            let n = ka.len().min(kb.len());
                            ka[..n] == kb[..n]
                        })
                    })
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

/// Like [`build_view_leaf_maps`], but from the parts a `ContentView`
/// retains at runtime (`tab_name` + `view_defs`) rather than the original
/// [`ViewFileConfig`]. Used by the shortcut menu to enumerate the keys of
/// the currently loaded tabs.
pub fn build_leaf_maps_for(
    tab_name: &str,
    views: &[ViewDef],
    kb: &KeyBindingConfig,
) -> Vec<ViewLeafMap> {
    build_leaf_maps(&TabRef::new(tab_name), views, kb)
}

// ---------------------------------------------------------------------------
// Shortcut inventory — projection of claims for the shortcut menu
// ---------------------------------------------------------------------------

/// One row in the shortcut menu: an action `name`, the `keys` that trigger
/// it, and the `scope` (tab / drilldown level) it is active in.
///
/// `source` / `key_scope` carry the originating claim so the interactive
/// editor can route an edit (rebind / disable / delete) back to the config
/// file that owns the binding. They are `None` for synthetic rows that no
/// config entry backs (e.g. autonumber tab-switch keys) — such rows are
/// read-only in the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutRow {
    pub name: String,
    pub keys: String,
    pub scope: String,
    pub source: Option<KeySource>,
    pub key_scope: Option<KeyScope>,
}

/// Human-readable scope label for a leaf, e.g. `Jira` (root) or
/// `Jira › comments` (drilled in).
pub fn leaf_scope_label(tab: &str, child_path: &[String]) -> String {
    if child_path.is_empty() {
        tab.to_string()
    } else {
        format!("{tab} › {}", child_path.join(" › "))
    }
}

/// Projects a keymap's `Handler` claims into shortcut-menu rows tagged
/// with `scope`. `Swallow` claims are skipped; keyless handlers (actions
/// with no bound key) are kept with an empty `keys` so the menu lists them
/// too. Rows with the same `(name, keys)` are de-duplicated (order
/// preserved).
pub fn shortcut_rows(keymap: &KeyMap, scope: &str) -> Vec<ShortcutRow> {
    shortcut_rows_with(keymap, |_| scope.to_string())
}

/// Like [`shortcut_rows`], but the scope label is derived *per claim* from
/// `scope_of` rather than fixed for the whole keymap. The shortcut menu's
/// "all" view uses this to tag each row with the scope the shortcut is
/// actually active in (`Global` / tab / leaf) instead of the leaf it was
/// enumerated in — so a global or tab-wide shortcut, which `push_tab_wide`
/// files into *every* leaf, collapses to a single row on dedup instead of
/// appearing once per drilldown level.
pub fn shortcut_rows_with<F>(keymap: &KeyMap, scope_of: F) -> Vec<ShortcutRow>
where
    F: Fn(&KeyClaim) -> String,
{
    let mut rows = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for claim in &keymap.claims {
        if claim.kind != KeyClaimKind::Handler {
            continue;
        }
        let keys = claim.key.0.join(" / ");
        let name = claim.source.action_name();
        if seen.insert((name.clone(), keys.clone())) {
            rows.push(ShortcutRow {
                name,
                keys,
                scope: scope_of(claim),
                source: Some(claim.source.clone()),
                key_scope: Some(claim.scope.clone()),
            });
        }
    }
    rows
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
    push_tab_wide(&mut root_map, tab, kb, view.window_ops);
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
    root_map.force_override_keys(&forced_keys(&view.actions));
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
    push_tab_wide(&mut km, tab, kb, view.window_ops);
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
    km.force_override_keys(&forced_keys(&child.actions));
    out.push(ViewLeafMap {
        view: view.name.clone(),
        child_path: path.clone(),
        keymap: km,
    });

    for nested in &child.children {
        push_child_leaves(out, tab, views, view, &path, nested, kb);
    }
}

fn push_tab_wide(km: &mut KeyMap, tab: &TabRef, kb: &KeyBindingConfig, window_ops: bool) {
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
    // App-level fallback keys: dispatched by `App::handle_common_action`
    // only when the focused `ContentView` returns `Unhandled`, so they are
    // effectively live at *every* leaf of an adapter tab (root and every
    // drilldown). They were the validator's blind spot — a YAML `actions:`
    // entry on, say, `c` shadowed column-config at runtime with no
    // conflict reported. Claiming them tab-wide makes that collision a
    // hard error (escape hatch: `force: true` on the action, which strips
    // the matching built-in claim — see `KeyMap::force_override_keys`).
    // `JumpMode` (`p`) is intentionally NOT here: `p` is the usual preview
    // key on content tabs, so claiming it would manufacture false conflicts.
    let app_fallback_common = [
        CommonAction::ColumnConfig,
        CommonAction::SortMode,
        CommonAction::SortMenu,
        CommonAction::CommandLineOpen,
    ];
    for action in app_fallback_common {
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

    // Window-leader chord(s) — only claimed for views that opt into
    // window/split operations (`window_ops: true`). Elsewhere the `w`
    // leader never engages, so filing the claim would manufacture a
    // false tab-wide reservation and hide real collisions on `w`.
    if window_ops {
        for (action, binding) in &kb.window.bindings {
            km.push(KeyClaim::handler(
                binding.clone(),
                KeyScope::Tab(tab.clone()),
                KeySource::Window(action.clone()),
            ));
        }
    }
}

/// Collect a `Handler` claim for every per-node `shortcuts:` entry (a single
/// key → adapter action verb) across `views` and their drill-down children.
///
/// These are real, live bindings but they are deliberately **not** filed into
/// the leaf maps that feed the load-time validator / saved-query check: a
/// node shortcut legitimately overrides a content built-in or subtab key on
/// its key (e.g. `Q` overriding `edit_query`), which the validator would
/// otherwise flag as a hard error. Instead the interactive keybinding editor
/// folds these claims into its own conflict check (via
/// [`crate::app::App`]), so a newly-recorded binding that collides with a
/// node shortcut is caught and offered for resolution.
pub fn node_shortcut_claims(tab: &str, views: &[ViewDef]) -> Vec<KeyClaim> {
    let tref = TabRef::new(tab);
    let mut out = Vec::new();
    for view in views {
        collect_node_shortcuts(
            &tref,
            &view.name,
            &[],
            &view.shortcuts,
            &view.children,
            &mut out,
        );
    }
    out
}

/// The [`KeyScope`] a per-node `shortcuts:` binding lives in, given the
/// drilldown depth of the level that declares it. Mirrors the profile
/// [`collect_node_shortcuts`] stamps on the claims it builds, so a
/// synthesised (still-unbound) adapter-action row shares the exact scope a
/// bound one would — the conflict check and the binding writer then treat
/// the two identically.
pub fn node_shortcut_scope(tab: &str, child_path: &[String]) -> KeyScope {
    let profile = if child_path.is_empty() {
        root_profile()
    } else {
        drilled_profile()
    };
    KeyScope::Pane(TabRef::new(tab), profile)
}

fn collect_node_shortcuts(
    tab: &TabRef,
    view_name: &str,
    child_path: &[String],
    shortcuts: &std::collections::HashMap<char, crate::config::view_config::ShortcutDef>,
    children: &[crate::config::view_config::ChildDef],
    out: &mut Vec<KeyClaim>,
) {
    let profile = if child_path.is_empty() {
        root_profile()
    } else {
        drilled_profile()
    };
    for (ch, def) in shortcuts {
        let key = ch.to_string();
        out.push(KeyClaim::handler(
            KeyBinding::new(&key),
            KeyScope::Pane(tab.clone(), profile.clone()),
            KeySource::NodeShortcut {
                view: view_name.to_string(),
                child_path: child_path.to_vec(),
                key,
                action: def.action().to_string(),
            },
        ));
    }
    for child in children {
        let mut path = child_path.to_vec();
        path.push(child.name.clone());
        collect_node_shortcuts(
            tab,
            view_name,
            &path,
            &child.shortcuts,
            &child.children,
            out,
        );
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
            if k.0.is_empty() {
                continue;
            }
            km.push(KeyClaim::handler(
                k.clone(),
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
                KeyScope::Pane(tab.clone(), profile.clone()),
                KeySource::YamlPreviewKey {
                    view: view.name.clone(),
                    child_path: Vec::new(),
                },
            ));
        }
    }
    if let Some(key) = view.card.as_ref().and_then(|c| c.key.as_ref()) {
        km.push(KeyClaim::handler(
            key.clone(),
            KeyScope::Pane(tab.clone(), profile),
            KeySource::YamlCardKey {
                view: view.name.clone(),
                child_path: Vec::new(),
            },
        ));
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
                KeyScope::Pane(tab.clone(), profile.clone()),
                KeySource::YamlPreviewKey {
                    view: view.name.clone(),
                    child_path: child_path.to_vec(),
                },
            ));
        }
    }
    if let Some(key) = child.card.as_ref().and_then(|c| c.key.as_ref()) {
        km.push(KeyClaim::handler(
            key.clone(),
            KeyScope::Pane(tab.clone(), profile),
            KeySource::YamlCardKey {
                view: view.name.clone(),
                child_path: child_path.to_vec(),
            },
        ));
    }
}

/// Keys of every `force: true` action in `actions` — the built-in claims
/// on these keys are stripped from the leaf (see
/// [`KeyMap::force_override_keys`]).
fn forced_keys(actions: &[crate::config::view_config::ActionDef]) -> Vec<String> {
    actions
        .iter()
        .filter(|a| a.force)
        .flat_map(|a| a.key_strings().iter().cloned())
        .collect()
}

fn push_action_claims(
    km: &mut KeyMap,
    tab: &TabRef,
    profile: PaneStateProfile,
    view: &str,
    child_path: &[String],
    action: &crate::config::view_config::ActionDef,
) {
    // Actions with no key still run via the action menu / rule engine, not
    // a keypress. We record them with an *empty* key binding so the shortcut
    // menu can list them (with a blank keys column) as a complete inventory.
    // An empty binding is inert for the validator (shares no key, so never
    // conflicts) and this keymap is not the dispatch path. Their
    // search/next-prev sub-keys below are irrelevant (those action types
    // always carry a key).
    let binding = action.key.clone().unwrap_or_else(|| KeyBinding(Vec::new()));
    km.push(KeyClaim::handler(
        binding,
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
                    view: view.to_string(),
                    child_path: child_path.to_vec(),
                    action: action.name.clone(),
                    direction: SearchJump::Next,
                },
            ));
        }
        if let Some(k) = &search.prev_key {
            km.push(KeyClaim::handler(
                KeyBinding::new(k.clone()),
                KeyScope::Pane(tab.clone(), profile),
                KeySource::PaneSearchJump {
                    view: view.to_string(),
                    child_path: child_path.to_vec(),
                    action: action.name.clone(),
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
            KeyBinding::new(key_str.clone()),
            KeyScope::Pane(tab.clone(), profile.clone()),
            KeySource::AppActionChain {
                scope_path,
                key: key_str,
            },
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
// Generalised conflict check for a *proposed* binding (interactive editor)
// ---------------------------------------------------------------------------

/// How a proposed binding collides with an existing claim. All three are
/// genuine conflicts — a chord and its prefix cannot coexist because the
/// shorter one dispatches before the longer one can complete.
//
// Wired into the interactive keybinding editor in a later phase; until then
// only the test suite exercises the conflict primitives below.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictKind {
    /// Both bind the exact same completed step sequence.
    Same,
    /// The proposed sequence is a strict prefix of the existing chord, so
    /// pressing it fires the proposal and the longer existing chord becomes
    /// unreachable.
    ProposedShadowsExisting,
    /// An existing (shorter) binding is a strict prefix of the proposed
    /// chord, so the existing key fires first and the proposal is
    /// unreachable.
    ExistingShadowsProposed,
}

/// One collision between a proposed binding and an existing [`KeyClaim`].
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct BindingConflict {
    /// The claim the proposal collides with — locate/rebind/delete via this.
    pub source: KeySource,
    pub scope: KeyScope,
    /// The proposed alternative (step list) that collides.
    pub proposed_seq: Vec<String>,
    /// The existing claim's colliding alternative (step list).
    pub existing_seq: Vec<String>,
    pub kind: ConflictKind,
}

/// Whether two completed step sequences collide, and how. Sequences collide
/// when one is a prefix of (or equal to) the other — they share the shorter
/// sequence's full length. `a` is the proposed side, `b` the existing side,
/// which fixes the direction of the shadowing verdict.
#[allow(dead_code)]
fn sequence_conflict(a: &[String], b: &[String]) -> Option<ConflictKind> {
    let n = a.len().min(b.len());
    if a[..n] != b[..n] {
        return None;
    }
    Some(match a.len().cmp(&b.len()) {
        std::cmp::Ordering::Equal => ConflictKind::Same,
        std::cmp::Ordering::Less => ConflictKind::ProposedShadowsExisting,
        std::cmp::Ordering::Greater => ConflictKind::ExistingShadowsProposed,
    })
}

/// Every way `proposed` (active in `scope`) collides with a `Handler` claim
/// in `claims`. This is the single conflict primitive the interactive
/// keybinding editor calls before writing a binding.
///
// Wired into the interactive keybinding editor in a later phase; until then
// only the test suite exercises it.
#[allow(dead_code)]
///
/// * Multi-step chords and prefixes are handled: `w` collides with `w v`,
///   and `ctrl+k l` collides with `ctrl+k`.
/// * Alternatives are expanded on both sides — a proposed `[a, w v]` is
///   checked as two sequences, and a claim bound to `[j, down]` as two.
/// * Scope gates the check: different tabs never collide (see
///   [`KeyScope::overlaps_with`]).
/// * `own` is the source being (re)bound; a claim from the same source is
///   skipped so rebinding an action onto a key it already holds is not a
///   self-conflict.
/// * An empty `proposed` (the disable form, `key: []`) collides with
///   nothing.
///
/// At most one conflict is reported per distinct existing sequence of a
/// claim; the caller typically dedups further by `source`.
pub fn binding_conflicts(
    proposed: &KeyBinding,
    scope: &KeyScope,
    claims: &[KeyClaim],
    own: Option<&KeySource>,
) -> Vec<BindingConflict> {
    let mut out = Vec::new();
    let proposed_seqs = proposed.step_lists();
    if proposed_seqs.is_empty() {
        return out;
    }
    for claim in claims {
        if claim.kind != KeyClaimKind::Handler {
            continue;
        }
        if own == Some(&claim.source) {
            continue;
        }
        if !scope.overlaps_with(&claim.scope) {
            continue;
        }
        // Sibling subtabs of the same tab are never active at the same time.
        // The `Pane` scope only tracks the tab, so without this two shortcuts
        // that each belong to a *different* subtab would falsely collide.
        if let (Some(a), Some(b)) = (
            own.and_then(KeySource::subtab_view),
            claim.source.subtab_view(),
        ) {
            if a != b {
                continue;
            }
        }
        for existing in claim.key.step_lists() {
            if existing.is_empty() {
                continue;
            }
            if let Some((p, kind)) = proposed_seqs
                .iter()
                .find_map(|p| sequence_conflict(p, &existing).map(|k| (p.clone(), k)))
            {
                out.push(BindingConflict {
                    source: claim.source.clone(),
                    scope: claim.scope.clone(),
                    proposed_seq: p,
                    existing_seq: existing,
                    kind,
                });
            }
        }
    }
    out
}

/// Conflicts for a proposed binding on an action inside one YAML view file,
/// checked against every leaf of that file (each carries the built-in
/// global/common/content/window claims folded in). Deduplicated by
/// conflicting `source`. `own` is the source being rebound. Since a view
/// file is one tab and different tabs never overlap, this is the complete
/// set of collisions for a view-action binding.
#[allow(dead_code)]
pub fn view_file_binding_conflicts(
    config: &ViewFileConfig,
    kb: &KeyBindingConfig,
    proposed: &KeyBinding,
    scope: &KeyScope,
    own: Option<&KeySource>,
) -> Vec<BindingConflict> {
    let mut all = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for leaf in build_view_leaf_maps(config, kb) {
        for c in binding_conflicts(proposed, scope, &leaf.keymap.claims, own) {
            if seen.insert(c.source.clone()) {
                all.push(c);
            }
        }
    }
    all
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
    fn conflict_between_pane_action_and_tab_wide_builtin() {
        // A view action's `Pane` scope overlaps its own tab's tab-wide
        // built-ins (e.g. `common.scroll_page_down`). Binding the action to a
        // key the built-in already holds must be reported — the shortcut menu
        // relies on this to prompt before saving.
        let builtin = KeyClaim::handler(
            KeyBinding::new("ctrl+f"),
            KeyScope::Tab(tab("Jira")),
            KeySource::Common(CommonAction::ScrollPageDown),
        );
        let own = KeySource::YamlAction {
            view: "tickets".into(),
            child_path: Vec::new(),
            name: "free text".into(),
        };
        let scope = KeyScope::Pane(tab("Jira"), root());
        let cs = binding_conflicts(
            &KeyBinding::new("ctrl+f"),
            &scope,
            std::slice::from_ref(&builtin),
            Some(&own),
        );
        assert_eq!(
            cs.len(),
            1,
            "pane action must conflict with a same-tab tab-wide built-in: {cs:?}"
        );

        // Regression precondition: the runtime keymap used to file context
        // claims under a placeholder `Pane(TabRef(""), …)`, whose empty tab
        // fails `overlaps_with` against the real-tab built-in — so the
        // collision slipped past the menu's check and only the reload
        // validator caught it. `add_binding_from_menu` now repairs the tab
        // before checking; this documents why that repair is necessary.
        let placeholder = KeyScope::Pane(TabRef::new(""), root());
        let missed = binding_conflicts(
            &KeyBinding::new("ctrl+f"),
            &placeholder,
            std::slice::from_ref(&builtin),
            Some(&own),
        );
        assert!(
            missed.is_empty(),
            "empty-tab placeholder scope misses the conflict (why the repair is needed): {missed:?}"
        );
    }

    #[test]
    fn no_conflict_between_node_shortcuts_in_different_subtabs() {
        // The Trackings tab has sibling subtabs "trackings" and "condensed".
        // Only one is foregrounded at a time, so binding `s` to
        // toggle-tracking in one must NOT collide with the same key in the
        // other — even though both share the tab-level `Pane` scope.
        let existing = KeyClaim::handler(
            KeyBinding::new("s"),
            node_shortcut_scope("Trackings", &[]),
            KeySource::NodeShortcut {
                view: "condensed".into(),
                child_path: Vec::new(),
                key: "s".into(),
                action: "toggle-tracking".into(),
            },
        );
        let own = KeySource::NodeShortcut {
            view: "trackings".into(),
            child_path: Vec::new(),
            key: String::new(),
            action: "toggle-tracking".into(),
        };
        let cs = binding_conflicts(
            &KeyBinding::new("s"),
            &node_shortcut_scope("Trackings", &[]),
            std::slice::from_ref(&existing),
            Some(&own),
        );
        assert!(cs.is_empty(), "sibling subtabs must not conflict: {cs:?}");
    }

    #[test]
    fn conflict_between_node_shortcuts_in_the_same_subtab() {
        // Same subtab, same key, different action → a genuine conflict.
        let existing = KeyClaim::handler(
            KeyBinding::new("s"),
            node_shortcut_scope("Trackings", &[]),
            KeySource::NodeShortcut {
                view: "trackings".into(),
                child_path: Vec::new(),
                key: "s".into(),
                action: "start".into(),
            },
        );
        let own = KeySource::NodeShortcut {
            view: "trackings".into(),
            child_path: Vec::new(),
            key: String::new(),
            action: "toggle-tracking".into(),
        };
        let cs = binding_conflicts(
            &KeyBinding::new("s"),
            &node_shortcut_scope("Trackings", &[]),
            std::slice::from_ref(&existing),
            Some(&own),
        );
        assert_eq!(cs.len(), 1, "same subtab must still conflict");
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

    /// An `actions:` entry on `c` collides with the App-level column-config
    /// fallback (`common.column_config`, default `c c`), which is claimed
    /// tab-wide. Since the default moved to a chord the collision is a
    /// *prefix* one: taking the leader key leaves the chord unreachable.
    /// Binding it without `force` is a hard error — exactly the case that
    /// previously slipped through (jira's `c` = comments shadowed
    /// column-config silently).
    #[test]
    fn action_c_conflicts_with_column_config_fallback() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: tickets
    node_type: t
    actions:
      - { name: comments, key: c, type: navigate, navigate_to: cc }
"#;
        let cfg = yaml_str(yaml);
        let errs = validate_view_file(&cfg, &KeyBindingConfig::default());
        assert!(
            errs.iter()
                .any(|e| e.contains("\"c c\"") && e.contains("column_config")),
            "expected a 'c' / column_config conflict, got: {errs:?}"
        );
    }

    /// `force: true` is the escape hatch: it strips the built-in
    /// column-config claim for `c` at that leaf, so the action takes the
    /// key cleanly and the validator stays quiet.
    #[test]
    fn force_action_c_suppresses_column_config_conflict() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: tickets
    node_type: t
    actions:
      - { name: comments, key: c, type: navigate, navigate_to: cc, force: true }
"#;
        let cfg = yaml_str(yaml);
        let errs = validate_view_file(&cfg, &KeyBindingConfig::default());
        assert!(
            errs.is_empty(),
            "force should suppress conflict, got: {errs:?}"
        );
    }

    /// `force` only overrides *built-in* claims — two YAML actions on the
    /// same key are still a genuine conflict even if one sets `force`.
    #[test]
    fn force_does_not_hide_two_yaml_actions_on_same_key() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: tickets
    node_type: t
    actions:
      - { name: comments, key: c, type: navigate, navigate_to: cc, force: true }
      - { name: other, key: c, type: custom, id: other }
"#;
        let cfg = yaml_str(yaml);
        let errs = validate_view_file(&cfg, &KeyBindingConfig::default());
        assert!(
            errs.iter().any(|e| e.contains("\"c\"")),
            "two YAML actions on 'c' must still conflict, got: {errs:?}"
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

    /// `card.key` is claimed statically like the preview key, so a collision
    /// with an `actions:` key on the same level is reported at config load
    /// instead of silently losing the toggle at runtime.
    #[test]
    fn validate_view_file_flags_card_key_vs_action_key() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: tickets
    node_type: t
    columns: [{ key: a }, { key: b }]
    card:
      key: C
      fields: [a, b]
      columns: 2
    actions:
      - { name: comments, key: C, type: custom, id: comments }
"#;
        let cfg = yaml_str(yaml);
        let errs = validate_view_file(&cfg, &KeyBindingConfig::default());
        assert!(
            errs.iter()
                .any(|e| e.contains("card.key") && e.contains("\"C\"")),
            "expected a card.key / 'C' conflict, got: {errs:?}"
        );
    }

    /// A card chord whose *prefix* is a free key stays clean — that is what
    /// makes `v c` usable next to an action on `v`-less levels.
    #[test]
    fn validate_view_file_accepts_card_chord_with_free_prefix() {
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: tickets
    node_type: t
    columns: [{ key: a }, { key: b }]
    card:
      key: 'v c'
      fields: [a, b]
      columns: 2
    actions:
      - { name: comments, key: C, type: custom, id: comments }
"#;
        let cfg = yaml_str(yaml);
        let errs = validate_view_file(&cfg, &KeyBindingConfig::default());
        assert!(errs.is_empty(), "chord should not collide, got: {errs:?}");
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
    # Window-leader chords are opt-in per view; enable them here so the
    # window-leader-prefix conflict check below has a claim to hit.
    window_ops: true
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

    // --- shortcut inventory ------------------------------------------------

    #[test]
    fn title_case_snake_to_label() {
        assert_eq!(title_case("tab_set_popup"), "Tab set popup");
        assert_eq!(title_case("quit"), "Quit");
        assert_eq!(title_case(""), "");
    }

    #[test]
    fn action_name_prefers_yaml_name_over_diagnostic_path() {
        let src = KeySource::YamlAction {
            view: "tables".into(),
            child_path: vec!["Rows".into()],
            name: "edit sql".into(),
        };
        assert_eq!(src.action_name(), "edit sql");
        // `human` still returns the diagnostic path for conflict messages.
        assert!(src.human().contains("actions[edit sql]"));
    }

    #[test]
    fn leaf_scope_label_root_and_drilled() {
        assert_eq!(leaf_scope_label("Jira", &[]), "Jira");
        assert_eq!(
            leaf_scope_label("Jira", &["comments".to_string()]),
            "Jira › comments"
        );
    }

    #[test]
    fn shortcut_rows_names_builtins_and_yaml_actions() {
        let yaml = r#"
tab: { name: Postgres }
adapter: { type: x }
views:
  - name: tables
    node_type: t
    actions:
      - name: run query
        key: r
        type: custom
        id: run
"#;
        let cfg = yaml_str(yaml);
        let kb = KeyBindingConfig::default();
        let leaves = build_view_leaf_maps(&cfg, &kb);
        let root = leaves
            .iter()
            .find(|l| l.child_path.is_empty())
            .expect("root leaf");
        let rows = shortcut_rows(&root.keymap, &leaf_scope_label("Postgres", &[]));

        // The YAML action carries its declared name and key.
        let run = rows
            .iter()
            .find(|r| r.name == "run query")
            .expect("YAML action row present");
        assert_eq!(run.keys, "r");
        assert_eq!(run.scope, "Postgres");

        // A built-in global (quit / ctrl+c) is title-cased.
        assert!(
            rows.iter()
                .any(|r| r.name == "Quit" && r.keys.contains("ctrl+c")),
            "expected a title-cased Quit row, got: {rows:?}"
        );
    }

    #[test]
    fn tab_switch_source_names_and_restore_default() {
        let s = KeySource::TabSwitch {
            tab: "Tasks".to_string(),
        };
        assert_eq!(s.action_name(), "Switch to Tasks");
        assert_eq!(s.human(), "tab[Tasks].key");
        // Not one of the four built-in sections …
        assert!(!s.is_builtin());
        // … but it does have a compiled default (the autonumber digit).
        assert!(s.has_compiled_default());
    }

    #[test]
    fn shortcut_rows_dedup_keeps_keyless_skips_swallow() {
        let mut km = KeyMap::new();
        // Two identical (name, keys) claims → one row.
        for _ in 0..2 {
            km.push(KeyClaim::handler(
                KeyBinding::new("g"),
                KeyScope::Global,
                KeySource::Global(GlobalAction::Quit),
            ));
        }
        // A keyless handler is kept, with an empty keys column.
        km.push(KeyClaim::handler(
            KeyBinding(vec![]),
            KeyScope::Global,
            KeySource::Global(GlobalAction::TabNext),
        ));
        // A Swallow claim is skipped.
        km.push(KeyClaim::swallow(
            KeyBinding::new("x"),
            KeyScope::Global,
            KeySource::Global(GlobalAction::TabPrev),
        ));
        let rows = shortcut_rows(&km, "S");
        assert_eq!(
            rows.len(),
            2,
            "dedup + keyless kept + swallow skipped, got: {rows:?}"
        );
        assert_eq!(rows[0].name, "Quit");
        assert_eq!(rows[0].keys, "g");
        // The keyless handler survives with an empty keys string.
        let keyless = rows.iter().find(|r| r.keys.is_empty());
        assert!(keyless.is_some(), "keyless row should be present: {rows:?}");
    }

    // --- generalised proposed-binding conflict check --------------------

    fn pane_scope() -> KeyScope {
        KeyScope::Pane(
            tab("postgres"),
            PaneStateProfile::Normal { drilldown: None },
        )
    }

    #[test]
    fn proposed_binding_flags_exact_and_prefix_and_shadow() {
        let claims = vec![
            yaml_action("t", "edit", "e"),
            yaml_action("t", "window", "w v"), // a two-step chord
        ];

        // Exact clash with `e`.
        let same = binding_conflicts(&KeyBinding::new("e"), &pane_scope(), &claims, None);
        assert_eq!(same.len(), 1);
        assert_eq!(same[0].kind, ConflictKind::Same);
        assert!(matches!(&same[0].source, KeySource::YamlAction { name, .. } if name == "edit"));

        // Proposing the leader `w` shadows the existing `w v` chord.
        let shadow = binding_conflicts(&KeyBinding::new("w"), &pane_scope(), &claims, None);
        assert_eq!(shadow.len(), 1);
        assert_eq!(shadow[0].kind, ConflictKind::ProposedShadowsExisting);

        // Proposing the longer `e x` is shadowed by the existing `e`.
        let shadowed = binding_conflicts(&KeyBinding::new("e x"), &pane_scope(), &claims, None);
        assert_eq!(shadowed.len(), 1);
        assert_eq!(shadowed[0].kind, ConflictKind::ExistingShadowsProposed);
    }

    #[test]
    fn proposed_binding_skips_own_source_and_disjoint_scope() {
        let own = KeySource::YamlAction {
            view: "t".into(),
            child_path: Vec::new(),
            name: "edit".into(),
        };
        let claims = vec![yaml_action("t", "edit", "e")];
        // Rebinding the same action onto its own key is not a conflict.
        assert!(
            binding_conflicts(&KeyBinding::new("e"), &pane_scope(), &claims, Some(&own)).is_empty()
        );
        // A claim on a different tab never collides.
        let other_tab = KeyScope::Pane(tab("jira"), PaneStateProfile::Normal { drilldown: None });
        assert!(binding_conflicts(&KeyBinding::new("e"), &other_tab, &claims, None).is_empty());
    }

    #[test]
    fn proposed_disable_and_alternatives() {
        let claims = vec![yaml_action("t", "edit", "e")];
        // The disable form (empty list) collides with nothing.
        assert!(
            binding_conflicts(&KeyBinding(Vec::new()), &pane_scope(), &claims, None).is_empty()
        );
        // A list of alternatives is expanded — the `e` alternative clashes.
        let alts = KeyBinding::multi(vec!["x", "e"]);
        let hits = binding_conflicts(&alts, &pane_scope(), &claims, None);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].proposed_seq, vec!["e".to_string()]);
    }
}
