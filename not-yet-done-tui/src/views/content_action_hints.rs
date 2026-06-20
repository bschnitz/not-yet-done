//! Active-state model for the content-tab action bar.
//!
//! The top action bar holds **only** shortcuts that can be *momentarily
//! active* — a mode armed, a popup open, an editor focused (see the contract
//! in [`crate::components::action_bar`]). Everything fire-and-forget belongs
//! in the bottom status bar.
//!
//! Rather than guess a hint's active-ness from its description string at
//! render time (fragile: relabel an action and it silently stops lighting
//! up), every action-bar hint carries an [`ActiveSource`] from the moment it
//! is built. A single resolver — `ContentView::resolve_active` — maps the
//! source against live UI state once per frame.
//!
//! This makes the structural contract enforceable: an action-bar hint with
//! no derivable [`ActiveSource`] is a bug (it belongs in the status bar), and
//! the build paths below `debug_assert!` on that case instead of letting a
//! dead shortcut sit in the top bar forever.

use crate::config::keybindings::{CommonAction, ContentAction};
use crate::keymap::KeySource;

/// Which bar a claim-derived nav hint belongs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintBar {
    Action,
    Status,
}

/// A hint derived directly from a [`crate::keymap::KeySource`] in the pane's
/// `build_claims` set — the typed Content/Common navigation & fold family
/// (back, open, paging, tree collapse/expand, grouping, aggregate toggle).
///
/// Deriving these from the *same* claim builder the dispatcher uses is what
/// makes the bars automatic: a fold chord like `zm`/`zr` shows up the moment
/// its claim is registered, with no per-feature hint wiring, and can never
/// drift from what actually dispatches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavHint {
    pub label: &'static str,
    pub bar: HintBar,
}

/// Map a typed Content/Common claim source to its bar hint, or `None` when the
/// source is either too elementary to surface (list-move, scroll, column
/// cursor — universal keys that would only clutter the bar) or rendered
/// through a richer path that carries its own metadata (YAML `actions:`,
/// preview, search-jump status line, the query/group/column menus and jump
/// mode, all built with their own [`ActiveSource`] in the action-bar builder).
pub fn nav_hint_for_source(source: &KeySource) -> Option<NavHint> {
    match source {
        KeySource::Content(a) => content_nav_hint(a),
        KeySource::Common(a) => common_nav_hint(a),
        _ => None,
    }
}

fn content_nav_hint(action: &ContentAction) -> Option<NavHint> {
    use ContentAction::*;
    let (label, bar) = match action {
        Back => ("back", HintBar::Status),
        Open => ("open", HintBar::Status),
        NextPage => ("next page", HintBar::Status),
        PrevPage => ("prev page", HintBar::Status),
        TreeCollapse => ("collapse", HintBar::Status),
        TreeCollapseAll => ("collapse all", HintBar::Status),
        TreeExpandAll => ("expand all", HintBar::Status),
        CycleGrouping => ("cycle group", HintBar::Status),
        ToggleTreeAggregate => ("aggregate", HintBar::Status),
        // Record-detail split (toggle / value-wrap) and group-order toggle:
        // claimed at the view level (`build_view_claims`), so this
        // pane-claim-driven resolver never sees them — their status-bar
        // hints are emitted directly by `ContentView::status_bar_hints`
        // under the same gate.
        ToggleRecordDetail | ToggleDetailWrap | ToggleGroupOrder => return None,
        // Activatable / richer-path sources: surfaced (with their
        // ActiveSource) by the action-bar builder, not here.
        EditQuery | OpenScriptsMenu | GroupMenu | JumpMode => return None,
    };
    Some(NavHint { label, bar })
}

fn common_nav_hint(action: &CommonAction) -> Option<NavHint> {
    // Every Common source is either universal navigation (list move, scroll,
    // column cursor — deliberately omitted to keep the bar focused) or driven
    // by the action-bar builder (fuzzy, search, column-config, tracking,
    // jump, command line, …). Nothing to surface in the status bar today.
    let _ = action;
    None
}

/// Why an action-bar hint can light up. Carried on each hint from build time
/// so the resolver — not the renderer — decides active-ness from real state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveSource {
    /// An editor session is focused whose label equals this string.
    Editor(String),
    /// A yes/no confirmation popup is open (delete etc.).
    Confirm,
    /// The saved-query / scripts menu popup is open.
    QueryMenu,
    /// The group-by menu popup is open.
    GroupMenu,
    /// The column-config popup is open.
    ColumnConfig,
    /// Fuzzy-filter input is taking over the bar.
    Fuzzy,
    /// A search / tree-find input or cached result set is active.
    Search,
    /// Jump (vimium-hop) mode is open in the focused pane.
    Jump,
    /// Tracking is running.
    Tracking,
    /// A node is armed on the move-clipboard (cut).
    MarkMove,
    /// A detached script is running.
    Script,
}

/// A built action-bar hint: the key label, the description, and the reason it
/// can light up. The renderer ([`crate::components::action_bar::ActionHint`])
/// only ever sees the resolved `active` bool; the source stays on the build
/// side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionBarHint {
    pub key: String,
    pub label: String,
    pub source: ActiveSource,
}

impl ActionBarHint {
    pub fn new(key: impl Into<String>, label: impl Into<String>, source: ActiveSource) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            source,
        }
    }
}

/// A resolved adapter `shortcuts:` entry: key, label, where it renders, and
/// (for action-bar placement) why it can light up. Built by
/// `ContentPane::collect_shortcut_hints`; consumed by both bar builders.
#[derive(Debug, Clone)]
pub struct ShortcutHint {
    pub key: String,
    pub label: String,
    pub placement: not_yet_done_content::HintPlacement,
    /// Active source, derived for every entry but only meaningful for
    /// action-bar placement. `None` means "no derivable active state" — a
    /// bug if the entry is action-bar placed.
    pub source: Option<ActiveSource>,
}

/// Map a typed `actions:` `action_type` to its active source. Only called for
/// actions where [`crate::config::view_config::ActionDef::shows_in_action_bar`]
/// returned `true`, so every action-bar type must be covered here; an
/// unexpected type means `shows_in_action_bar` admitted something this
/// mapping forgot — a bug, flagged in debug and degraded to an editor source
/// (which simply never lights up for a non-editor label).
pub fn source_for_action_type(action_type: &str, label: &str) -> ActiveSource {
    match action_type {
        "edit" | "create" | "query_edit" => ActiveSource::Editor(label.to_string()),
        "fuzzy_filter" => ActiveSource::Fuzzy,
        "search" | "text_search" | "tree_find" => ActiveSource::Search,
        "script" => ActiveSource::Script,
        // `custom` only reaches the action bar when flagged `on_container`
        // (`shows_in_action_bar`); the only such action today is the
        // trackings `restore all`, which confirms before purging. Map it to
        // the Confirm source so the hint lights up while the `(y/n)` popup
        // is open.
        "custom" => ActiveSource::Confirm,
        other => {
            debug_assert!(
                false,
                "action_type '{other}' shows in the action bar but has no ActiveSource mapping"
            );
            ActiveSource::Editor(label.to_string())
        }
    }
}

/// Map an adapter `shortcuts:` action that is placed in the action bar to its
/// active source, from its stable `id` and whether it opens an input. Returns
/// `None` for a fire-and-forget action with no derivable active state — such
/// an action must not live in the action bar, so the caller `debug_assert!`s
/// and demotes it to the status bar.
pub fn source_for_shortcut(id: &str, label: &str, opens_input: bool) -> Option<ActiveSource> {
    match id {
        "delete" => Some(ActiveSource::Confirm),
        "toggle-tracking" => Some(ActiveSource::Tracking),
        "mark-move" => Some(ActiveSource::MarkMove),
        _ if opens_input => Some(ActiveSource::Editor(label.to_string())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_editor_actions_map_to_editor_source() {
        assert_eq!(
            source_for_action_type("edit", "edit"),
            ActiveSource::Editor("edit".into())
        );
        assert_eq!(
            source_for_action_type("create", "add"),
            ActiveSource::Editor("add".into())
        );
        assert_eq!(
            source_for_action_type("query_edit", "edit query"),
            ActiveSource::Editor("edit query".into())
        );
    }

    #[test]
    fn typed_mode_actions_map_to_their_modes() {
        assert_eq!(source_for_action_type("fuzzy_filter", "filter"), ActiveSource::Fuzzy);
        assert_eq!(source_for_action_type("search", "search"), ActiveSource::Search);
        assert_eq!(source_for_action_type("text_search", "find"), ActiveSource::Search);
        assert_eq!(source_for_action_type("tree_find", "tree find"), ActiveSource::Search);
        assert_eq!(source_for_action_type("script", "run"), ActiveSource::Script);
    }

    #[test]
    fn shortcut_ids_map_to_popups_and_modes() {
        assert_eq!(source_for_shortcut("delete", "delete", false), Some(ActiveSource::Confirm));
        assert_eq!(
            source_for_shortcut("toggle-tracking", "track", false),
            Some(ActiveSource::Tracking)
        );
        assert_eq!(source_for_shortcut("mark-move", "cut", false), Some(ActiveSource::MarkMove));
    }

    #[test]
    fn shortcut_with_input_is_an_editor() {
        assert_eq!(
            source_for_shortcut("add", "add", true),
            Some(ActiveSource::Editor("add".into()))
        );
    }

    #[test]
    fn fire_and_forget_shortcut_has_no_source() {
        // An input-less, non-popup action (e.g. open-in-browser) is not
        // activatable → must not sit in the action bar.
        assert_eq!(source_for_shortcut("open-in-browser", "open", false), None);
    }

    #[test]
    fn fold_chords_resolve_to_status_bar_nav_hints() {
        // The bug this whole change fixes: zm/zr (and backspace-collapse)
        // must derive a status-bar hint straight from their claim source.
        for (action, label) in [
            (ContentAction::TreeCollapse, "collapse"),
            (ContentAction::TreeCollapseAll, "collapse all"),
            (ContentAction::TreeExpandAll, "expand all"),
        ] {
            let hint = nav_hint_for_source(&KeySource::Content(action))
                .expect("fold chord must derive a hint");
            assert_eq!(hint.bar, HintBar::Status);
            assert_eq!(hint.label, label);
        }
    }

    #[test]
    fn back_open_paging_grouping_are_status_nav_hints() {
        for action in [
            ContentAction::Back,
            ContentAction::Open,
            ContentAction::NextPage,
            ContentAction::PrevPage,
            ContentAction::CycleGrouping,
            ContentAction::ToggleTreeAggregate,
        ] {
            let hint = nav_hint_for_source(&KeySource::Content(action)).unwrap();
            assert_eq!(hint.bar, HintBar::Status);
        }
    }

    #[test]
    fn activatable_and_richer_path_sources_have_no_nav_hint() {
        // These are surfaced by the action-bar builder with an ActiveSource,
        // so the nav resolver must stay silent to avoid double-display.
        for action in [
            ContentAction::EditQuery,
            ContentAction::OpenScriptsMenu,
            ContentAction::GroupMenu,
            ContentAction::JumpMode,
        ] {
            assert_eq!(nav_hint_for_source(&KeySource::Content(action)), None);
        }
    }

    #[test]
    fn common_and_other_sources_have_no_nav_hint() {
        // Universal navigation keys stay out of the bar.
        assert_eq!(nav_hint_for_source(&KeySource::Common(CommonAction::ListNext)), None);
        assert_eq!(
            nav_hint_for_source(&KeySource::Common(CommonAction::ScrollHalfDown)),
            None
        );
        // A non-typed source (YAML action) is handled by its own builder.
        assert_eq!(
            nav_hint_for_source(&KeySource::YamlAction {
                view: "v".into(),
                child_path: Vec::new(),
                name: "do".into(),
            }),
            None
        );
    }
}
