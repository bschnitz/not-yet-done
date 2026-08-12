use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use crate::action::ActionChains;
use crate::active_surface::ActiveSurface;
use not_yet_done_macros::AllVariants;

// ---------------------------------------------------------------------------
// KeyBinding
// ---------------------------------------------------------------------------

/// One or more key strings that trigger the same action.
///
/// Deserializes from either a single string or a YAML list:
/// ```yaml
/// form_add: a           # single key
/// list_next: [j, down]  # multiple keys
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyBinding(pub Vec<String>);

/// Normalise a key string for matching: the YAML-facing alias `"space"`
/// is rewritten to the literal space character `" "` so it matches the
/// raw encoding emitted by [`crate::events::key_event_to_string`] for
/// `KeyCode::Char(' ')`. Only whole-token matches are rewritten — a
/// naïve `replace("space", " ")` would corrupt `"backspace"` into
/// `"back "` and turn it into a fake chord prefix for `b`.
fn canonicalize_key(s: &str) -> String {
    if s == "space" {
        return " ".to_string();
    }
    if let Some(rest) = s.strip_suffix("+space") {
        return format!("{rest}+ ");
    }
    s.to_string()
}

/// Whether `s` looks like a chord sequence of single printable keys
/// (e.g. `"vt"`, `"zr"`, `"gg"`) as opposed to an atomic named key
/// (`"f12"`, `"enter"`, `"backspace"`) or a modifier-prefixed key
/// (`"ctrl+x"`, `"shift+tab"`). Only chord strings are eligible to
/// trigger pending-key chord buildup — without this filter a single
/// `f` would be misread as a prefix of `f12`. Run on canonicalized
/// strings (so `"space"` is already `" "` and gets length-1-rejected).
fn is_chord_string(s: &str) -> bool {
    if s.contains('+') {
        return false;
    }
    if s.chars().count() <= 1 {
        return false;
    }
    if matches!(
        s,
        "enter"
            | "esc"
            | "tab"
            | "backspace"
            | "delete"
            | "up"
            | "down"
            | "left"
            | "right"
            | "home"
            | "end"
            | "pageup"
            | "pagedown"
    ) {
        return false;
    }
    if s.starts_with('f') && s.len() >= 2 && s[1..].chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    true
}

/// Parse one binding string into its ordered list of canonical **step**
/// tokens. A binding is a sequence of steps; each step is one atomic key
/// press, optionally modifier-prefixed (`ctrl+k`). Three surface forms are
/// accepted so both the modern interactive editor and legacy configs parse
/// to the same internal shape:
///
/// * **space-separated** — `"ctrl+k l"` → `["ctrl+k", "l"]`. The modern form
///   the editor writes; the only form that can carry modifiers on a step
///   past the first. A literal space step is written with the `space` alias
///   (`"ctrl+k space"`), never a bare double space, so splitting on ASCII
///   space is unambiguous.
/// * **legacy concatenation** of single printable chars — `"zr"` → `["z",
///   "r"]` (see [`is_chord_string`]).
/// * **single atomic token** — `"a"`, `"ctrl+shift+a"`, `"f12"`, `"enter"`,
///   `"space"` → one step.
///
/// Every step is canonicalized (`space` alias → `" "`).
pub fn binding_steps(s: &str) -> Vec<String> {
    if s.contains(' ') {
        return s
            .split(' ')
            .filter(|p| !p.is_empty())
            .map(canonicalize_key)
            .collect();
    }
    let canon = canonicalize_key(s);
    if is_chord_string(&canon) {
        return canon.chars().map(|c| c.to_string()).collect();
    }
    vec![canon]
}

impl KeyBinding {
    pub fn new(s: impl Into<String>) -> Self {
        Self(vec![s.into()])
    }

    pub fn multi(keys: Vec<impl Into<String>>) -> Self {
        Self(keys.into_iter().map(|s| s.into()).collect())
    }

    /// Step lists of every alternative this binding holds.
    pub fn step_lists(&self) -> Vec<Vec<String>> {
        self.0.iter().map(|s| binding_steps(s)).collect()
    }

    /// Whether a fully-pressed sequence of canonical step tokens exactly
    /// equals one of the bound alternatives. This is the single entry point
    /// the dispatcher uses to decide "does this completed key sequence fire
    /// the action?" — single keys are just a one-element sequence.
    pub fn matches_sequence(&self, pressed: &[String]) -> bool {
        self.0.iter().any(|s| binding_steps(s) == pressed)
    }

    /// Whether `pressed` is a **strict** prefix of some alternative — more
    /// keys are still needed to complete it, so the dispatcher should keep
    /// the sequence pending. A binding that equals `pressed` is *not* a
    /// prefix (nothing left to wait for).
    pub fn is_sequence_prefix(&self, pressed: &[String]) -> bool {
        self.0.iter().any(|s| {
            let steps = binding_steps(s);
            steps.len() > pressed.len() && steps[..pressed.len()] == *pressed
        })
    }

    /// Check whether `key` matches any bound alternative. `key` is the
    /// pressed sequence in surface form — normally a single key, but the
    /// legacy concatenation dispatcher also passes an accumulated chord
    /// string (`"glm"`); both are parsed into steps first, so a multi-step
    /// binding fires only on the fully-pressed sequence, never a lone key.
    pub fn matches(&self, key: &str) -> bool {
        self.matches_sequence(&binding_steps(key))
    }

    /// Check if `pending`+`key` completes a chord binding. `pending` is the
    /// accumulated sequence in either surface form (legacy concatenation or
    /// space-separated); it is parsed into steps before appending `key`.
    pub fn matches_chord(&self, pending: &str, key: &str) -> bool {
        let mut seq = binding_steps(pending);
        seq.push(canonicalize_key(key));
        self.matches_sequence(&seq)
    }

    /// Check if `key` is a **strict** prefix of any multi-step binding.
    /// `key` is a pressed sequence in surface form (a single key, or an
    /// accumulated chord string like `"gl"`). Atomic named keys like
    /// `"f12"` or a lone `"ctrl+x"` are single-step, so they are never a
    /// prefix — a single `f`/`c` isn't misread as the start of a longer
    /// chord.
    pub fn is_prefix(&self, key: &str) -> bool {
        self.is_sequence_prefix(&binding_steps(key))
    }

    /// Display label listing every bound key, e.g. `[backspace/h]`.
    pub fn display_label(&self) -> String {
        if self.0.is_empty() {
            return "[?]".to_string();
        }
        format!("[{}]", self.0.join("/"))
    }

    /// Compact label for status/action bar hints, listing every bound
    /// key joined by `/`. Each key is rendered as its icon glyph if
    /// present in `icons` (e.g. `⌫`), otherwise as the raw key string
    /// (e.g. `<` or `Q`). Result has no brackets — meant for inline
    /// display alongside other hints (e.g. `⌫/h`).
    pub fn hint_label(&self, icons: &KeyIconMap) -> String {
        if self.0.is_empty() {
            return "?".to_string();
        }
        self.0
            .iter()
            .map(|k| icons.get(k).cloned().unwrap_or_else(|| k.clone()))
            .collect::<Vec<_>>()
            .join("/")
    }
}

impl From<&str> for KeyBinding {
    fn from(s: &str) -> Self {
        KeyBinding(vec![s.to_string()])
    }
}

impl From<String> for KeyBinding {
    fn from(s: String) -> Self {
        KeyBinding(vec![s])
    }
}

impl Serialize for KeyBinding {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.0.len() == 1 {
            s.serialize_str(&self.0[0])
        } else {
            self.0.serialize(s)
        }
    }
}

impl<'de> Deserialize<'de> for KeyBinding {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de;

        struct KeyBindingVisitor;

        impl<'de> de::Visitor<'de> for KeyBindingVisitor {
            type Value = KeyBinding;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a string or a list of strings")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<KeyBinding, E> {
                Ok(KeyBinding(vec![v.to_string()]))
            }

            // A bare scalar key like `1` parses as a YAML integer, and a
            // `key: yes`/`key: true` as a bool. Coerce them back to their
            // string form so a single unquoted digit/word can never break the
            // owning view. The writer quotes these (see `yaml_edit`), so this
            // is the reader-side safety net for hand-edited or legacy configs.
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<KeyBinding, E> {
                Ok(KeyBinding(vec![v.to_string()]))
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<KeyBinding, E> {
                Ok(KeyBinding(vec![v.to_string()]))
            }

            fn visit_bool<E: de::Error>(self, v: bool) -> Result<KeyBinding, E> {
                Ok(KeyBinding(vec![v.to_string()]))
            }

            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<KeyBinding, A::Error> {
                let mut keys = Vec::new();
                while let Some(s) = seq.next_element::<String>()? {
                    keys.push(s);
                }
                Ok(KeyBinding(keys))
            }
        }

        d.deserialize_any(KeyBindingVisitor)
    }
}

// ---------------------------------------------------------------------------
// Macro: Serialize/Deserialize via Display/FromStr
// ---------------------------------------------------------------------------

macro_rules! impl_string_serde {
    ($t:ty) => {
        impl Serialize for $t {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.to_string())
            }
        }
        impl<'de> Deserialize<'de> for $t {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let raw = String::deserialize(d)?;
                <$t>::from_str(&raw).map_err(serde::de::Error::custom)
            }
        }
    };
}

// ---------------------------------------------------------------------------
// GlobalAction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, AllVariants)]
pub enum GlobalAction {
    Quit,
    TabNext,
    TabPrev,
    /// Cycle to the next subtab (view) within the active tab, wrapping
    /// around. Mirrors [`TabNext`](Self::TabNext) one level down; a no-op on
    /// tabs with a single view.
    SubtabNext,
    /// Cycle to the previous subtab (view) within the active tab, wrapping.
    SubtabPrev,
    DismissNotifications,
    /// Open the notification log — every message both bars have shown this
    /// session, timestamped and merged chronologically — read-only in the
    /// editor. The counterpart to a short bar
    /// (`notifications.max_messages`): messages the bar pushed out, and
    /// messages already dismissed with [`DismissNotifications`](Self::DismissNotifications),
    /// stay readable here.
    ShowNotifications,
    ShowLastError,
    /// Capture the current selection's [`NodeRef`] into the app-wide
    /// link-mark slot. Cleared by Esc or overwritten by another Mark.
    LinkMark,
    /// Write a directed link `current → marked` to the link table.
    /// The mark stays armed so a single mark can be pasted onto
    /// multiple targets in a row.
    LinkPaste,
    /// Open the link popup for the current row: one list with outgoing
    /// links on top and incoming links below. Enter navigates to the
    /// other side; `d` deletes the link; Esc closes.
    LinkOpenPopup,
    /// Vim-style back-jump in the cross-tab link history (Ctrl+O).
    /// Only link-popup activations push entries; regular tab switches
    /// do not.
    LinkJumpBack,
    /// Vim-style forward-jump (Ctrl+I) — reverses a Ctrl+O. Note that
    /// terminals collapse Ctrl+I onto Tab unless kitty's
    /// DISAMBIGUATE_ESCAPE_CODES is active (it is here, when supported).
    LinkJumpForward,
    /// Open the shortcut menu — a list of every configured keyboard
    /// shortcut (name → keys). Opens scoped to the current context by
    /// default; a toggle inside the popup expands it to every tab.
    ShortcutMenu,
    /// Toggle fullscreen mode — hide all chrome bars (tab bar, the view's
    /// action/shortcut bar and the bottom status bar) so the content view
    /// fills the terminal. Message bars (alerts, notifications, inline
    /// query errors) stay visible. Toggling again restores the chrome.
    ToggleFullscreen,
}

impl GlobalAction {
    fn as_str(&self) -> &'static str {
        match self {
            GlobalAction::Quit => "quit",
            GlobalAction::ShortcutMenu => "shortcut_menu",
            GlobalAction::ToggleFullscreen => "toggle_fullscreen",
            GlobalAction::TabNext => "tab_next",
            GlobalAction::TabPrev => "tab_prev",
            GlobalAction::SubtabNext => "subtab_next",
            GlobalAction::SubtabPrev => "subtab_prev",
            GlobalAction::DismissNotifications => "dismiss_notifications",
            GlobalAction::ShowNotifications => "show_notifications",
            GlobalAction::ShowLastError => "show_last_error",
            GlobalAction::LinkMark => "link_mark",
            GlobalAction::LinkPaste => "link_paste",
            GlobalAction::LinkOpenPopup => "link_open_popup",
            GlobalAction::LinkJumpBack => "link_jump_back",
            GlobalAction::LinkJumpForward => "link_jump_forward",
        }
    }
}

impl fmt::Display for GlobalAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for GlobalAction {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "quit" => Ok(GlobalAction::Quit),
            "shortcut_menu" => Ok(GlobalAction::ShortcutMenu),
            "toggle_fullscreen" => Ok(GlobalAction::ToggleFullscreen),
            "tab_next" => Ok(GlobalAction::TabNext),
            "tab_prev" => Ok(GlobalAction::TabPrev),
            "subtab_next" => Ok(GlobalAction::SubtabNext),
            "subtab_prev" => Ok(GlobalAction::SubtabPrev),
            "dismiss_notifications" => Ok(GlobalAction::DismissNotifications),
            "show_notifications" => Ok(GlobalAction::ShowNotifications),
            "show_last_error" => Ok(GlobalAction::ShowLastError),
            "link_mark" => Ok(GlobalAction::LinkMark),
            "link_paste" => Ok(GlobalAction::LinkPaste),
            "link_open_popup" => Ok(GlobalAction::LinkOpenPopup),
            "link_jump_back" => Ok(GlobalAction::LinkJumpBack),
            "link_jump_forward" => Ok(GlobalAction::LinkJumpForward),
            other => Err(format!("unknown global action: {}", other)),
        }
    }
}

impl_string_serde!(GlobalAction);

// ---------------------------------------------------------------------------
// BarPlacement — where a global action surfaces in the permanent chrome
// ---------------------------------------------------------------------------

/// Which permanent bar (if any) a [`GlobalAction`] belongs in.
///
/// This is the compile-time guarantee the shortcut-visibility work is built
/// around: [`GlobalAction::placement`] matches every variant with **no `_`
/// arm**, so adding a new global action without deciding where it surfaces is
/// a compile error — nothing can silently fall through the cracks the way
/// `shortcut_menu` once did (bound, but shown in no permanent bar).
///
/// The bar builders then *iterate* [`GlobalAction::ALL`] and read this, so a
/// newly classified action appears in its bar automatically, gated only on
/// having a binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BarPlacement {
    /// Fire-and-forget: rendered in the bottom status bar under `label`.
    /// Actions sharing a label are grouped (their keys joined with `/`).
    Status { label: &'static str },
    /// Momentarily activatable: rendered in a top bar under `label` and lit
    /// up while `surface` is the active surface.
    Active {
        label: &'static str,
        surface: ActiveSurface,
    },
    /// Reachable only via the shortcut menu — deliberately not in any
    /// permanent bar (too niche or too numerous to earn a slot).
    MenuOnly,
}

impl GlobalAction {
    /// Where this action surfaces in the permanent chrome. Exhaustive by
    /// design — see [`BarPlacement`]. Do **not** add a `_` arm: a new
    /// variant must be classified here explicitly.
    pub fn placement(&self) -> BarPlacement {
        use GlobalAction::*;
        match self {
            Quit => BarPlacement::Status { label: "quit" },
            TabNext | TabPrev => BarPlacement::Status {
                label: "cycle tabs",
            },
            ShortcutMenu => BarPlacement::Active {
                label: "menu",
                surface: ActiveSurface::ShortcutMenu,
            },
            // Reachable from the shortcut menu; no permanent-bar slot.
            SubtabNext | SubtabPrev | DismissNotifications | ShowNotifications | ShowLastError
            | LinkMark | LinkPaste | LinkOpenPopup | LinkJumpBack | LinkJumpForward
            | ToggleFullscreen => BarPlacement::MenuOnly,
        }
    }
}

/// Build the bottom status-bar hints for every [`GlobalAction`] classified as
/// [`BarPlacement::Status`], in declaration order, grouping actions that share
/// a label (their keys joined with `/`) and skipping any action with no
/// binding. Iterating [`GlobalAction::ALL`] plus the exhaustive
/// [`GlobalAction::placement`] is what guarantees a newly status-placed global
/// shows up here automatically, with no hand-maintained list.
pub fn global_status_hints(gkb: &KeyBindingSection<GlobalAction>) -> Vec<(String, String)> {
    // (description/label, joined keys) preserving first-seen label order.
    let mut groups: Vec<(&'static str, Vec<String>)> = Vec::new();
    for action in GlobalAction::ALL {
        let BarPlacement::Status { label } = action.placement() else {
            continue;
        };
        let Some(binding) = gkb.get(action) else {
            continue;
        };
        let key = binding.display_label();
        match groups.iter_mut().find(|(l, _)| *l == label) {
            Some((_, keys)) => keys.push(key),
            None => groups.push((label, vec![key])),
        }
    }
    groups
        .into_iter()
        .map(|(label, keys)| (keys.join("/"), label.to_string()))
        .collect()
}

/// Build the top action-bar hints for every [`GlobalAction`] classified as
/// [`BarPlacement::Active`] and currently bound, as `(action, key_label,
/// desc)`. The caller resolves each action's `active` flag — only the App
/// knows whether e.g. the shortcut-menu popup is open — and turns these into
/// `ActionHint`s appended to the content action bar. Like
/// [`global_status_hints`], iterating [`GlobalAction::ALL`] plus the
/// exhaustive [`GlobalAction::placement`] keeps this driftless: a newly
/// `Active`-placed global surfaces here automatically.
pub fn global_active_hints(
    gkb: &KeyBindingSection<GlobalAction>,
) -> Vec<(GlobalAction, String, String)> {
    GlobalAction::ALL
        .iter()
        .filter_map(|action| {
            let BarPlacement::Active { label, .. } = action.placement() else {
                return None;
            };
            let binding = gkb.get(action)?;
            Some((action.clone(), binding.display_label(), label.to_string()))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// CommonAction — shared between Tasks and Trackings tabs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, AllVariants)]
pub enum CommonAction {
    ListNext,
    ListPrev,
    ListFirst,
    ListLast,
    ScrollHalfUp,
    ScrollHalfDown,
    ScrollPageUp,
    ScrollPageDown,
    FuzzyFilterOpen,
    FuzzyFilterAccept,
    FuzzyFilterClear,
    FuzzyFilterCancel,
    SearchOpen,
    SearchNext,
    SearchPrev,
    SavedFilterSelect,
    FormFilter,
    ColumnConfig,
    FormClose,
    FavoriteToggle,
    CommandLineOpen,
    JumpMode,
    SortMode,
    /// Open the sort menu: the whole sort spec as one list (sorted columns
    /// first, in sort order, with their direction). A second UI path onto
    /// the same state [`Self::SortMode`] edits column-by-column — both
    /// end in `App::commit_sort`.
    SortMenu,
    /// Move the optional column cursor one cell to the left. Only takes
    /// effect in views that opt in via `column_cursor: true`.
    ColumnLeft,
    /// Move the optional column cursor one cell to the right.
    ColumnRight,
}

impl CommonAction {
    fn as_str(&self) -> &'static str {
        match self {
            Self::ListNext => "list_next",
            Self::ListPrev => "list_prev",
            Self::ListFirst => "list_first",
            Self::ListLast => "list_last",
            Self::ScrollHalfUp => "scroll_half_up",
            Self::ScrollHalfDown => "scroll_half_down",
            Self::ScrollPageUp => "scroll_page_up",
            Self::ScrollPageDown => "scroll_page_down",
            Self::FuzzyFilterOpen => "fuzzy_filter_open",
            Self::FuzzyFilterAccept => "fuzzy_filter_accept",
            Self::FuzzyFilterClear => "fuzzy_filter_clear",
            Self::FuzzyFilterCancel => "fuzzy_filter_cancel",
            Self::SearchOpen => "search_open",
            Self::SearchNext => "search_next",
            Self::SearchPrev => "search_prev",
            Self::SavedFilterSelect => "saved_filter_select",
            Self::FormFilter => "form_filter",
            Self::ColumnConfig => "column_config",
            Self::FormClose => "form_close",
            Self::FavoriteToggle => "favorite_toggle",
            Self::CommandLineOpen => "command_line_open",
            Self::JumpMode => "jump_mode",
            Self::SortMode => "sort_mode",
            Self::SortMenu => "sort_menu",
            Self::ColumnLeft => "column_left",
            Self::ColumnRight => "column_right",
        }
    }
}

impl fmt::Display for CommonAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for CommonAction {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "list_next" => Ok(Self::ListNext),
            "list_prev" => Ok(Self::ListPrev),
            "list_first" => Ok(Self::ListFirst),
            "list_last" => Ok(Self::ListLast),
            "scroll_half_up" => Ok(Self::ScrollHalfUp),
            "scroll_half_down" => Ok(Self::ScrollHalfDown),
            "scroll_page_up" => Ok(Self::ScrollPageUp),
            "scroll_page_down" => Ok(Self::ScrollPageDown),
            "fuzzy_filter_open" => Ok(Self::FuzzyFilterOpen),
            "fuzzy_filter_accept" => Ok(Self::FuzzyFilterAccept),
            "fuzzy_filter_clear" => Ok(Self::FuzzyFilterClear),
            "fuzzy_filter_cancel" => Ok(Self::FuzzyFilterCancel),
            "search_open" => Ok(Self::SearchOpen),
            "search_next" => Ok(Self::SearchNext),
            "search_prev" => Ok(Self::SearchPrev),
            "saved_filter_select" => Ok(Self::SavedFilterSelect),
            "column_left" => Ok(Self::ColumnLeft),
            "column_right" => Ok(Self::ColumnRight),
            "form_filter" => Ok(Self::FormFilter),
            "column_config" => Ok(Self::ColumnConfig),
            "form_close" => Ok(Self::FormClose),
            "favorite_toggle" => Ok(Self::FavoriteToggle),
            "command_line_open" => Ok(Self::CommandLineOpen),
            "jump_mode" => Ok(Self::JumpMode),
            "sort_mode" => Ok(Self::SortMode),
            "sort_menu" => Ok(Self::SortMenu),
            other => Err(format!("unknown common action: {}", other)),
        }
    }
}

impl_string_serde!(CommonAction);

// ---------------------------------------------------------------------------
// ContentAction — generic ContentView keybindings (Jira/Taiga/…)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, AllVariants)]
pub enum ContentAction {
    Back,
    Open,
    PrevPage,
    NextPage,
    EditQuery,
    /// Open the adapter-native scripts menu for the *selected* item.
    /// Postgres uses this on the `tables` subtab to manage per-table
    /// `.sql` scripts. Distinct from [`Self::EditQuery`] which targets
    /// the parent node when looking *into* a table (Rows view).
    OpenScriptsMenu,
    /// Smart-collapse in tree mode: close the currently selected node
    /// if it is expanded, otherwise close its parent (moving the
    /// cursor up to that parent). No-op at depth 0 on a collapsed
    /// node. Only registered on tree-mode panes (`tree_label` set on
    /// the root view). Bound to `backspace` (a navigation gesture) so it
    /// never competes with the `c` leader — fold is navigation and carries
    /// no action-bar entry, while `c` must stay free to open the table
    /// menus ([`CommonAction::ColumnConfig`] on `c c`,
    /// [`CommonAction::SortMenu`] on `c s`), including on tree panes.
    TreeCollapse,
    /// Collapse every expanded node in a tree-mode pane back to the
    /// root listing. Mirrors the Tasks tab's `zm` chord. Only registered
    /// on tree-mode panes (`tree_label` set on the root view); ignored
    /// elsewhere so `zm` stays free for other uses. Loaded children
    /// remain cached, so a subsequent re-expand reuses them without
    /// a refetch.
    TreeCollapseAll,
    /// Fully expand every node in a tree-mode pane, the mirror of
    /// [`Self::TreeCollapseAll`]. Mirrors the Tasks tab's `zr` chord.
    /// Reuses the per-node auto-expand cascade with an unbounded target
    /// depth, lazily loading unloaded children via `ExpandTreeNode` and
    /// re-pumping as they arrive. Only registered on tree-mode panes
    /// (`tree_label` set on the root view); ignored elsewhere.
    TreeExpandAll,
    /// Rotate the runtime grouping granularity (M3) on a grouped flat view:
    /// `ungrouped → Day → Week → Month → Year → ungrouped`, bucketing the
    /// level's configured `group_by` column. A no-op on a level that
    /// declares no `group_by` (and in tree mode). Lets a user regroup a
    /// worklog by day/week/month without editing the view YAML.
    CycleGrouping,
    /// Open the group-by menu (M3) on a grouped flat view: a small popup
    /// listing the same states `cycle_grouping` walks (No grouping / Day /
    /// Week / Month / Year) for a direct jump instead of cycling. First-
    /// letter hotkeys select immediately — native Trackings `u` parity.
    /// Registered under the same gate as `cycle_grouping` (the level must
    /// declare a `group_by`), so the default `u` stays free elsewhere
    /// (e.g. for a YAML `u: undelete` shortcut on the Tasks tab).
    GroupMenu,
    /// Toggle the **group ordering** on a grouped flat view (`o`): flip the
    /// bucket order between ascending and descending (e.g. day buckets
    /// newest-first ⟷ oldest-first) while preserving the bucket granularity
    /// and the item order *within* each group (that is `S`'s job). Registered
    /// under the same gate as `cycle_grouping` (the level must declare a
    /// `group_by`) and only when no record-detail split is offered, so the
    /// default `o` stays free for [`ToggleRecordDetail`] on wide-row views.
    ToggleGroupOrder,
    /// Toggle `tree_aggregate` columns (M4) between a node's own value and
    /// the adapter's subtree-cumulated value, in tree mode. A no-op on a
    /// level whose columns declare no `tree_aggregate` (and in flat mode).
    /// Lets a user flip a worklog tree between per-node and rolled-up
    /// durations without editing the view YAML.
    ToggleTreeAggregate,
    /// Open vimium-style jump mode on the table (native Tasks-tab parity):
    /// type a character, every visible row containing it gets a label, type
    /// the label to hop the cursor there. Distinct from the native tab's
    /// [`CommonAction::JumpMode`] (`p`) so the adapter tab can keep `p` free
    /// for a `paste`/`paste-move` shortcut; defaults to `J`.
    JumpMode,
    /// Toggle the record-detail split (`o`): split the focused pane and
    /// open a coupled follower to the right that transposes the
    /// *selected* row into a field-name | field-value table, kept live
    /// as the cursor moves. Pressing it again closes the follower. Only
    /// registered on panes whose level sets `record_detail: true`
    /// (wide, schema-rich rows — e.g. Postgres table rows / script
    /// results); a no-op elsewhere. Defaults to `o`.
    ToggleRecordDetail,
    /// Toggle line-wrapping of long values inside the record-detail
    /// follower (`X`): off by default (values clip to the value
    /// column), on splits long values onto continuation rows.
    /// Registered only while a record-detail follower exists; a no-op
    /// otherwise. Defaults to `X`.
    ToggleDetailWrap,
    /// Open link-hop on the table (vimium-style link picker): every URL —
    /// bare URLs and markdown `[text](url)` links — found on a visible line
    /// gets a label; type the label to open that URL in the browser via the
    /// configured opener (`xdg-open` by default). Works on any pane whose
    /// rows carry links (chat messages, comment bodies, …). Defaults to `f`.
    LinkHop,
    /// Toggle long-text mode (`v`): a column that declares `long_source`
    /// stops clipping to a single fitted line and instead renders the full
    /// field as a soft-wrapped block, growing that row vertically. Every
    /// other column, the header, day grouping and totals stay exactly as
    /// they are. Registered only on panes whose active columns declare a
    /// `long_source`; a no-op elsewhere. Defaults to `v`.
    ToggleLongText,
    /// Toggle card mode on a level that declares a `card:` block: every row
    /// re-renders as a framed card whose fields sit in a grid of
    /// `card.columns` slots per line, instead of one table line. The choice
    /// is remembered per level and survives a restart. Not bound by default
    /// — a level names its own key via `card.key`, so no key is stolen from
    /// views without card mode.
    ToggleCardMode,
}

impl ContentAction {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Back => "back",
            Self::Open => "open",
            Self::PrevPage => "prev_page",
            Self::NextPage => "next_page",
            Self::EditQuery => "edit_query",
            Self::OpenScriptsMenu => "open_scripts_menu",
            Self::TreeCollapse => "tree_collapse",
            Self::TreeCollapseAll => "tree_collapse_all",
            Self::TreeExpandAll => "tree_expand_all",
            Self::CycleGrouping => "cycle_grouping",
            Self::GroupMenu => "group_menu",
            Self::ToggleGroupOrder => "toggle_group_order",
            Self::ToggleTreeAggregate => "toggle_tree_aggregate",
            Self::JumpMode => "jump_mode",
            Self::ToggleRecordDetail => "toggle_record_detail",
            Self::ToggleDetailWrap => "toggle_detail_wrap",
            Self::LinkHop => "link_hop",
            Self::ToggleLongText => "toggle_long_text",
            Self::ToggleCardMode => "toggle_card_mode",
        }
    }
}

impl fmt::Display for ContentAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ContentAction {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "back" => Ok(Self::Back),
            "open" => Ok(Self::Open),
            "prev_page" => Ok(Self::PrevPage),
            "next_page" => Ok(Self::NextPage),
            "edit_query" => Ok(Self::EditQuery),
            "open_scripts_menu" => Ok(Self::OpenScriptsMenu),
            "tree_collapse" => Ok(Self::TreeCollapse),
            "tree_collapse_all" => Ok(Self::TreeCollapseAll),
            "tree_expand_all" => Ok(Self::TreeExpandAll),
            "cycle_grouping" => Ok(Self::CycleGrouping),
            "group_menu" => Ok(Self::GroupMenu),
            "toggle_group_order" => Ok(Self::ToggleGroupOrder),
            "toggle_tree_aggregate" => Ok(Self::ToggleTreeAggregate),
            "jump_mode" => Ok(Self::JumpMode),
            "toggle_record_detail" => Ok(Self::ToggleRecordDetail),
            "toggle_detail_wrap" => Ok(Self::ToggleDetailWrap),
            "link_hop" => Ok(Self::LinkHop),
            "toggle_long_text" => Ok(Self::ToggleLongText),
            "toggle_card_mode" => Ok(Self::ToggleCardMode),
            other => Err(format!("unknown content action: {}", other)),
        }
    }
}

impl_string_serde!(ContentAction);

// ---------------------------------------------------------------------------
// WindowAction — split-pane window operations (typically Ctrl-w prefixed)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, AllVariants)]
pub enum WindowAction {
    /// Split the focused pane horizontally — new pane on the right.
    SplitRight,
    /// Split the focused pane vertically — new pane below.
    SplitDown,
    /// Close the focused pane (last-pane closes are no-ops).
    Close,
    /// Move focus to the pane that opened the focused split — typically the
    /// source side of the most recent split. Primarily a chain target so a
    /// single key can refocus the parent before navigating in it.
    FocusParent,
    /// Move focus to the pane this one opened — the coupled `linked_child`
    /// if present, otherwise the structural sibling. Symmetric counterpart
    /// to `FocusParent`; mostly used at the tail of an action chain to
    /// return to the just-replaced child after a `content.open`.
    FocusChild,
}

impl WindowAction {
    fn as_str(&self) -> &'static str {
        match self {
            Self::SplitRight => "split_right",
            Self::SplitDown => "split_down",
            Self::Close => "close",
            Self::FocusParent => "focus_parent",
            Self::FocusChild => "focus_child",
        }
    }
}

impl fmt::Display for WindowAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for WindowAction {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "split_right" => Ok(Self::SplitRight),
            "split_down" => Ok(Self::SplitDown),
            "close" => Ok(Self::Close),
            "focus_parent" => Ok(Self::FocusParent),
            "focus_child" => Ok(Self::FocusChild),
            other => Err(format!("unknown window action: {}", other)),
        }
    }
}

impl_string_serde!(WindowAction);

// ---------------------------------------------------------------------------
// QueryMenuAction — query menu popup keybindings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, AllVariants)]
pub enum QueryMenuAction {
    Select,
    Next,
    Prev,
    Edit,
    Delete,
    EditShortcut,
    /// Remove the keyboard shortcut bound to the selected entry, leaving
    /// the query itself untouched.
    ClearShortcut,
    /// Toggle the selected entry as the default query — applied
    /// automatically on app start.
    SetDefault,
    Close,
}

impl QueryMenuAction {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::Next => "next",
            Self::Prev => "prev",
            Self::Edit => "edit",
            Self::Delete => "delete",
            Self::EditShortcut => "edit_shortcut",
            Self::ClearShortcut => "clear_shortcut",
            Self::SetDefault => "set_default",
            Self::Close => "close",
        }
    }
}

impl fmt::Display for QueryMenuAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for QueryMenuAction {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "select" => Ok(Self::Select),
            "next" => Ok(Self::Next),
            "prev" => Ok(Self::Prev),
            "edit" => Ok(Self::Edit),
            "delete" => Ok(Self::Delete),
            "edit_shortcut" => Ok(Self::EditShortcut),
            "clear_shortcut" => Ok(Self::ClearShortcut),
            "set_default" => Ok(Self::SetDefault),
            "close" => Ok(Self::Close),
            other => Err(format!("unknown query_menu action: {}", other)),
        }
    }
}

impl_string_serde!(QueryMenuAction);

// ---------------------------------------------------------------------------
// TagMenuAction — tag menu popup keybindings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, AllVariants)]
pub enum TagMenuAction {
    /// Enter — on a selected entry: toggle assignment to the currently
    /// selected task (assign if not assigned, unassign if already
    /// assigned); on typed-name with no match (or `+name` prefix):
    /// create a new tag and auto-assign it to the selected task.
    Toggle,
    /// Edit/rename the selected entry. Default Ctrl+E.
    Edit,
    /// Create a new entry (prompts for a name). Default Ctrl+N.
    Create,
    Next,
    Prev,
    /// Delete the selected entry from the persistent store.
    Delete,
    Close,
}

impl TagMenuAction {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Toggle => "toggle",
            Self::Edit => "edit",
            Self::Create => "create",
            Self::Next => "next",
            Self::Prev => "prev",
            Self::Delete => "delete",
            Self::Close => "close",
        }
    }
}

impl fmt::Display for TagMenuAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TagMenuAction {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "toggle" => Ok(Self::Toggle),
            "edit" => Ok(Self::Edit),
            "create" => Ok(Self::Create),
            "next" => Ok(Self::Next),
            "prev" => Ok(Self::Prev),
            "delete" => Ok(Self::Delete),
            "close" => Ok(Self::Close),
            other => Err(format!("unknown tag_menu action: {}", other)),
        }
    }
}

impl_string_serde!(TagMenuAction);

// ---------------------------------------------------------------------------
// ScriptMenuAction — `:script` fuzzy menu popup keybindings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, AllVariants)]
pub enum ScriptMenuAction {
    /// Enter — on a selected entry: run the script; on a typed name with
    /// no match (or `+name` prefix): open the editor on a new script file
    /// with that filename. Empty input + Enter closes the menu.
    Run,
    /// Edit the selected entry (open the script in the external editor).
    /// Default Ctrl+E.
    Edit,
    /// Bind a keyboard shortcut to the selected script so it can be run
    /// directly from the owning pane without opening the menu. Default
    /// Ctrl+S (mirrors the query menu's [`QueryMenuAction::EditShortcut`]).
    EditShortcut,
    Next,
    Prev,
    /// Delete the selected entry from disk.
    Delete,
    Close,
}

impl ScriptMenuAction {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Edit => "edit",
            Self::EditShortcut => "edit_shortcut",
            Self::Next => "next",
            Self::Prev => "prev",
            Self::Delete => "delete",
            Self::Close => "close",
        }
    }
}

impl fmt::Display for ScriptMenuAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ScriptMenuAction {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "run" => Ok(Self::Run),
            "edit" => Ok(Self::Edit),
            "edit_shortcut" => Ok(Self::EditShortcut),
            "next" => Ok(Self::Next),
            "prev" => Ok(Self::Prev),
            "delete" => Ok(Self::Delete),
            "close" => Ok(Self::Close),
            other => Err(format!("unknown script_menu action: {}", other)),
        }
    }
}

impl_string_serde!(ScriptMenuAction);

// ---------------------------------------------------------------------------
// PopupAction — intrinsic SearchablePopup keybindings shared by every picker
// (transition picker, query menu, tag menu, script menu, …). Embedders still
// own `Select`/`Close` and embedder-specific actions; everything below is
// list-navigation + search-text input that should behave the same way
// everywhere.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, AllVariants)]
pub enum PopupAction {
    /// Move list selection down.
    Next,
    /// Move list selection up.
    Prev,
    /// Delete the character before the search cursor.
    Backspace,
    /// Move the search cursor one character left.
    CursorLeft,
    /// Move the search cursor one character right.
    CursorRight,
}

impl PopupAction {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Next => "next",
            Self::Prev => "prev",
            Self::Backspace => "backspace",
            Self::CursorLeft => "cursor_left",
            Self::CursorRight => "cursor_right",
        }
    }
}

impl fmt::Display for PopupAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PopupAction {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "next" => Ok(Self::Next),
            "prev" => Ok(Self::Prev),
            "backspace" => Ok(Self::Backspace),
            "cursor_left" => Ok(Self::CursorLeft),
            "cursor_right" => Ok(Self::CursorRight),
            other => Err(format!("unknown popup action: {}", other)),
        }
    }
}

impl_string_serde!(PopupAction);

// ---------------------------------------------------------------------------
// FormAction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, AllVariants)]
pub enum FormAction {
    Next,
    Prev,
    MultiselectNext,
    MultiselectPrev,
}

impl FormAction {
    fn as_str(&self) -> &'static str {
        match self {
            FormAction::Next => "next",
            FormAction::Prev => "prev",
            FormAction::MultiselectNext => "multiselect_next",
            FormAction::MultiselectPrev => "multiselect_prev",
        }
    }
}

impl fmt::Display for FormAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for FormAction {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "next" => Ok(FormAction::Next),
            "prev" => Ok(FormAction::Prev),
            "multiselect_next" => Ok(FormAction::MultiselectNext),
            "multiselect_prev" => Ok(FormAction::MultiselectPrev),
            other => Err(format!("unknown form action: {}", other)),
        }
    }
}

impl_string_serde!(FormAction);

// ---------------------------------------------------------------------------
// KeyBindingSection<A>
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct KeyBindingSection<A: Eq + std::hash::Hash> {
    pub bindings: HashMap<A, KeyBinding>,
}

impl<A: Eq + std::hash::Hash> KeyBindingSection<A> {
    pub fn get(&self, action: &A) -> Option<&KeyBinding> {
        self.bindings.get(action)
    }

    pub fn label(&self, action: &A) -> String {
        self.get(action)
            .map(|k| k.display_label())
            .unwrap_or_else(|| "[?]".to_string())
    }

    /// Hint-bar variant of [`label`]: returns the icon glyph (if mapped)
    /// or the raw key string, no brackets.
    pub fn hint_label(&self, action: &A, icons: &KeyIconMap) -> String {
        self.get(action)
            .map(|k| k.hint_label(icons))
            .unwrap_or_else(|| "?".to_string())
    }
}

impl<A> Serialize for KeyBindingSection<A>
where
    A: Eq + std::hash::Hash + Serialize,
{
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.bindings.serialize(serializer)
    }
}

impl<'de, A> Deserialize<'de> for KeyBindingSection<A>
where
    A: Eq + std::hash::Hash + FromStr,
    KeyBindingSection<A>: Default,
{
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Parse keys as raw strings first, then resolve each to an action
        // via `FromStr`. Unknown action names are skipped rather than
        // failing the whole config — this keeps a user's `tui.yaml` loading
        // even when it still binds an action that has since been removed
        // (e.g. `tracking_toggle`).
        let raw = HashMap::<String, KeyBinding>::deserialize(deserializer)?;
        // Start from defaults, then override with user-provided bindings.
        let mut merged = Self::default();
        for (name, binding) in raw {
            if let Ok(action) = A::from_str(&name) {
                merged.bindings.insert(action, binding);
            }
        }
        Ok(merged)
    }
}

impl Default for KeyBindingSection<GlobalAction> {
    fn default() -> Self {
        let mut m = HashMap::new();
        m.insert(GlobalAction::Quit, KeyBinding::new("ctrl+c"));
        m.insert(GlobalAction::ShortcutMenu, KeyBinding::new("ctrl+y"));
        m.insert(GlobalAction::ToggleFullscreen, KeyBinding::new("f11"));
        m.insert(GlobalAction::TabNext, KeyBinding::new("tab"));
        m.insert(GlobalAction::TabPrev, KeyBinding::new("shift+tab"));
        m.insert(GlobalAction::SubtabNext, KeyBinding::new("]"));
        m.insert(GlobalAction::SubtabPrev, KeyBinding::new("["));
        // `Z` (capital) so the tasks-tree chords `zr`/`zm` are reachable —
        // a single-key binding that's a prefix of a chord shadows the
        // chord at the dispatcher level. Lower-case `z` would also be
        // unambiguous given the prefix-detector, but conflicts with
        // chord-prefix detection. Also exposed via `:dismiss-notifications`.
        m.insert(GlobalAction::DismissNotifications, KeyBinding::new("Z"));
        m.insert(GlobalAction::ShowNotifications, KeyBinding::new("f10"));
        m.insert(GlobalAction::ShowLastError, KeyBinding::new("f12"));
        m.insert(GlobalAction::LinkMark, KeyBinding::new("glm"));
        m.insert(GlobalAction::LinkPaste, KeyBinding::new("glp"));
        m.insert(GlobalAction::LinkOpenPopup, KeyBinding::new("glo"));
        m.insert(GlobalAction::LinkJumpBack, KeyBinding::new("glb"));
        m.insert(GlobalAction::LinkJumpForward, KeyBinding::new("glf"));
        Self { bindings: m }
    }
}

impl Default for KeyBindingSection<CommonAction> {
    fn default() -> Self {
        let mut m = HashMap::new();
        m.insert(CommonAction::ListNext, KeyBinding::multi(vec!["down", "j"]));
        m.insert(CommonAction::ListPrev, KeyBinding::multi(vec!["up", "k"]));
        m.insert(
            CommonAction::ListFirst,
            KeyBinding::multi(vec!["home", "gg"]),
        );
        m.insert(CommonAction::ListLast, KeyBinding::multi(vec!["end", "G"]));
        m.insert(CommonAction::ScrollHalfUp, KeyBinding::new("ctrl+u"));
        m.insert(CommonAction::ScrollHalfDown, KeyBinding::new("ctrl+d"));
        m.insert(CommonAction::ScrollPageUp, KeyBinding::new("ctrl+b"));
        m.insert(CommonAction::ScrollPageDown, KeyBinding::new("ctrl+f"));
        m.insert(CommonAction::FuzzyFilterOpen, KeyBinding::new("f"));
        m.insert(CommonAction::FuzzyFilterAccept, KeyBinding::new("enter"));
        m.insert(CommonAction::FuzzyFilterClear, KeyBinding::new("ctrl+u"));
        m.insert(CommonAction::FuzzyFilterCancel, KeyBinding::new("esc"));
        m.insert(CommonAction::SearchOpen, KeyBinding::new("/"));
        m.insert(CommonAction::SearchNext, KeyBinding::new("n"));
        m.insert(CommonAction::SearchPrev, KeyBinding::new("N"));
        m.insert(CommonAction::SavedFilterSelect, KeyBinding::new("q"));
        // Column config and sort menu share the `c` leader: both configure
        // *how the table reads*, and the chord keeps single-key `c` free on
        // views that want it.
        m.insert(CommonAction::ColumnConfig, KeyBinding::new("c c"));
        m.insert(CommonAction::FormClose, KeyBinding::new("esc"));
        m.insert(CommonAction::CommandLineOpen, KeyBinding::new(":"));
        m.insert(CommonAction::JumpMode, KeyBinding::new("p"));
        m.insert(CommonAction::SortMode, KeyBinding::new("S"));
        m.insert(CommonAction::SortMenu, KeyBinding::new("c s"));
        // ColumnLeft / ColumnRight have no default: they are auto-claimed
        // as `h`/`l` only at leaves with `column_cursor: true` and are
        // suppressed everywhere else. See keymap.rs / content_view.rs.
        Self { bindings: m }
    }
}

impl Default for KeyBindingSection<FormAction> {
    fn default() -> Self {
        let mut m = HashMap::new();
        m.insert(FormAction::Next, KeyBinding::new("ctrl+j"));
        m.insert(FormAction::Prev, KeyBinding::new("ctrl+k"));
        m.insert(FormAction::MultiselectNext, KeyBinding::new("tab"));
        m.insert(FormAction::MultiselectPrev, KeyBinding::new("shift+tab"));
        Self { bindings: m }
    }
}

impl Default for KeyBindingSection<ContentAction> {
    fn default() -> Self {
        let mut m = HashMap::new();
        m.insert(ContentAction::Back, KeyBinding::new("backspace"));
        m.insert(ContentAction::Open, KeyBinding::new("enter"));
        m.insert(ContentAction::PrevPage, KeyBinding::new("<"));
        m.insert(ContentAction::NextPage, KeyBinding::new(">"));
        m.insert(ContentAction::EditQuery, KeyBinding::new("Q"));
        m.insert(ContentAction::OpenScriptsMenu, KeyBinding::new("q"));
        m.insert(ContentAction::TreeCollapse, KeyBinding::new("backspace"));
        m.insert(ContentAction::TreeCollapseAll, KeyBinding::new("zm"));
        m.insert(ContentAction::TreeExpandAll, KeyBinding::new("zr"));
        m.insert(ContentAction::CycleGrouping, KeyBinding::new("zg"));
        m.insert(ContentAction::GroupMenu, KeyBinding::new("u"));
        m.insert(ContentAction::ToggleTreeAggregate, KeyBinding::new("zt"));
        m.insert(ContentAction::JumpMode, KeyBinding::new("J"));
        m.insert(ContentAction::ToggleGroupOrder, KeyBinding::new("o"));
        m.insert(ContentAction::ToggleRecordDetail, KeyBinding::new("o"));
        m.insert(ContentAction::ToggleDetailWrap, KeyBinding::new("X"));
        // `LinkHop` is intentionally NOT bound by default: link-hop is
        // opt-in per view/child. A view enables it by binding the action
        // on itself or a child (`keybindings: { link_hop: f }`); with no
        // binding present the claim is never filed and `f` stays free.
        m.insert(ContentAction::ToggleLongText, KeyBinding::new("v"));
        // `ToggleCardMode` is intentionally NOT bound by default: card mode
        // is opt-in per level and names its own key in the view config
        // (`card: { key: C }`). A global binding here would claim the key on
        // every view, including the ones without a `card:` block.
        Self { bindings: m }
    }
}

impl Default for KeyBindingSection<WindowAction> {
    fn default() -> Self {
        let mut m = HashMap::new();
        // Vim-style chord: `w` as leader. Action keys (v/s/q/h/l) are
        // auto-filtered out of the per-pane tag alphabet so chord keys
        // never collide with pane-switch tags.
        m.insert(WindowAction::SplitRight, KeyBinding::new("wv"));
        m.insert(WindowAction::SplitDown, KeyBinding::new("ws"));
        m.insert(WindowAction::Close, KeyBinding::new("wq"));
        m.insert(WindowAction::FocusParent, KeyBinding::new("wh"));
        m.insert(WindowAction::FocusChild, KeyBinding::new("wl"));
        Self { bindings: m }
    }
}

impl Default for KeyBindingSection<QueryMenuAction> {
    fn default() -> Self {
        let mut m = HashMap::new();
        m.insert(QueryMenuAction::Select, KeyBinding::new("enter"));
        m.insert(QueryMenuAction::Next, KeyBinding::new("ctrl+j"));
        m.insert(QueryMenuAction::Prev, KeyBinding::new("ctrl+k"));
        m.insert(QueryMenuAction::Edit, KeyBinding::new("ctrl+e"));
        m.insert(QueryMenuAction::Delete, KeyBinding::new("ctrl+d"));
        m.insert(QueryMenuAction::EditShortcut, KeyBinding::new("ctrl+s"));
        m.insert(QueryMenuAction::ClearShortcut, KeyBinding::new("ctrl+x"));
        m.insert(QueryMenuAction::SetDefault, KeyBinding::new("ctrl+t"));
        m.insert(QueryMenuAction::Close, KeyBinding::new("esc"));
        Self { bindings: m }
    }
}

impl Default for KeyBindingSection<TagMenuAction> {
    fn default() -> Self {
        let mut m = HashMap::new();
        m.insert(TagMenuAction::Toggle, KeyBinding::new("enter"));
        m.insert(TagMenuAction::Edit, KeyBinding::new("ctrl+e"));
        m.insert(TagMenuAction::Create, KeyBinding::new("ctrl+n"));
        m.insert(TagMenuAction::Next, KeyBinding::new("ctrl+j"));
        m.insert(TagMenuAction::Prev, KeyBinding::new("ctrl+k"));
        m.insert(TagMenuAction::Delete, KeyBinding::new("ctrl+d"));
        m.insert(TagMenuAction::Close, KeyBinding::new("esc"));
        Self { bindings: m }
    }
}

impl Default for KeyBindingSection<ScriptMenuAction> {
    fn default() -> Self {
        let mut m = HashMap::new();
        m.insert(ScriptMenuAction::Run, KeyBinding::new("enter"));
        m.insert(ScriptMenuAction::Edit, KeyBinding::new("ctrl+e"));
        m.insert(ScriptMenuAction::EditShortcut, KeyBinding::new("ctrl+s"));
        m.insert(ScriptMenuAction::Next, KeyBinding::new("ctrl+j"));
        m.insert(ScriptMenuAction::Prev, KeyBinding::new("ctrl+k"));
        m.insert(ScriptMenuAction::Delete, KeyBinding::new("ctrl+d"));
        m.insert(ScriptMenuAction::Close, KeyBinding::new("esc"));
        Self { bindings: m }
    }
}

impl Default for KeyBindingSection<PopupAction> {
    fn default() -> Self {
        let mut m = HashMap::new();
        // Navigation matches the other menus (ctrl+j/ctrl+k); arrow keys
        // are the secondary binding so muscle memory keeps working.
        m.insert(PopupAction::Next, KeyBinding::multi(vec!["ctrl+j", "down"]));
        m.insert(PopupAction::Prev, KeyBinding::multi(vec!["ctrl+k", "up"]));
        m.insert(PopupAction::Backspace, KeyBinding::new("backspace"));
        m.insert(PopupAction::CursorLeft, KeyBinding::new("left"));
        m.insert(PopupAction::CursorRight, KeyBinding::new("right"));
        Self { bindings: m }
    }
}

// ---------------------------------------------------------------------------
// KeyIconMap — display glyphs for known key strings
// ---------------------------------------------------------------------------

/// Maps raw key strings (e.g. `"backspace"`, `"enter"`) to display glyphs
/// (e.g. `⌫`, `⏎`) used in status/action-bar hints. Built-in defaults
/// cover common navigation keys; users can override or extend via the
/// `key_icons` section in `tui.yaml`.
#[derive(Debug, Clone)]
pub struct KeyIconMap(HashMap<String, String>);

impl KeyIconMap {
    pub fn get(&self, key: &str) -> Option<&String> {
        self.0.get(key)
    }
}

impl Default for KeyIconMap {
    fn default() -> Self {
        let mut m = HashMap::new();
        m.insert("backspace".into(), "⌫".into());
        m.insert("enter".into(), "⏎".into());
        m.insert("tab".into(), "⇥".into());
        m.insert("shift+tab".into(), "⇤".into());
        m.insert("esc".into(), "␛".into());
        m.insert("up".into(), "↑".into());
        m.insert("down".into(), "↓".into());
        m.insert("left".into(), "←".into());
        m.insert("right".into(), "→".into());
        m.insert("space".into(), "␣".into());
        Self(m)
    }
}

impl Serialize for KeyIconMap {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> Deserialize<'de> for KeyIconMap {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let user = HashMap::<String, String>::deserialize(d)?;
        // Start from defaults, then layer user overrides on top.
        let mut merged = Self::default();
        for (k, v) in user {
            merged.0.insert(k, v);
        }
        Ok(merged)
    }
}

// ---------------------------------------------------------------------------
// Top-level KeyBindingConfig
// ---------------------------------------------------------------------------

/// Configurable alphabet for per-pane letter tags. Each leaf in a
/// split pane tree is assigned the lowest-still-free letter from this
/// string; pressing `ctrl+w<letter>` switches focus to that pane.
/// Default is the QWERTY home row, ordered for ergonomic reach.
#[derive(Debug, Clone)]
pub struct PaneTagAlphabet(pub String);

fn default_pane_tag_alphabet() -> String {
    "asdfghjkl".to_string()
}

impl Default for PaneTagAlphabet {
    fn default() -> Self {
        Self(default_pane_tag_alphabet())
    }
}

impl Serialize for PaneTagAlphabet {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for PaneTagAlphabet {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Self(s))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KeyBindingConfig {
    #[serde(default)]
    pub global: KeyBindingSection<GlobalAction>,
    #[serde(default)]
    pub common: KeyBindingSection<CommonAction>,
    #[serde(default)]
    pub form: KeyBindingSection<FormAction>,
    #[serde(default)]
    pub query_menu: KeyBindingSection<QueryMenuAction>,
    #[serde(default)]
    pub tag_menu: KeyBindingSection<TagMenuAction>,
    #[serde(default)]
    pub script_menu: KeyBindingSection<ScriptMenuAction>,
    #[serde(default)]
    pub popup: KeyBindingSection<PopupAction>,
    #[serde(default)]
    pub content: KeyBindingSection<ContentAction>,
    #[serde(default)]
    pub window: KeyBindingSection<WindowAction>,
    #[serde(default)]
    pub key_icons: KeyIconMap,
    #[serde(default)]
    pub pane_tags: PaneTagAlphabet,
    /// Globally available action chains. Resolution order is
    /// ChildDef → ViewDef → this map; the most specific scope wins.
    /// `Some(chain)` runs the listed actions in order; `None` disables
    /// the binding at this scope without falling back to a less specific
    /// one.
    #[serde(default)]
    pub action_chains: ActionChains,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(s: &str) -> Vec<String> {
        binding_steps(s)
    }

    #[test]
    fn all_variants_is_complete_and_roundtrips() {
        // `#[derive(AllVariants)]` must list every variant, in declaration
        // order, and each entry must round-trip through Display/FromStr. This
        // guards the compile-time-complete iteration the bar builders rely on:
        // a new variant that forgets its as_str/FromStr arm fails here.
        macro_rules! check {
            ($ty:ty, $count:expr) => {{
                let all = <$ty>::ALL;
                assert_eq!(all.len(), $count, concat!(stringify!($ty), "::ALL length"));
                for v in all {
                    let s = v.to_string();
                    let back: $ty = s.parse().expect("variant must parse back");
                    assert_eq!(&back, v, "round-trip for {s}");
                }
            }};
        }
        check!(GlobalAction, 15);
        check!(CommonAction, 26);
        check!(ContentAction, 19);
        check!(WindowAction, 5);
        check!(QueryMenuAction, 9);
        check!(TagMenuAction, 7);
        check!(ScriptMenuAction, 7);
        check!(PopupAction, 5);
        check!(FormAction, 4);
    }

    #[test]
    fn binding_steps_parses_all_three_surface_forms() {
        // Single atomic tokens → one step.
        assert_eq!(steps("a"), vec!["a"]);
        assert_eq!(steps("ctrl+shift+a"), vec!["ctrl+shift+a"]);
        assert_eq!(steps("f12"), vec!["f12"]);
        assert_eq!(steps("enter"), vec!["enter"]);
        // Legacy concatenation of single printable chars → per-char steps.
        assert_eq!(steps("zr"), vec!["z", "r"]);
        assert_eq!(steps("glm"), vec!["g", "l", "m"]);
        // Modern space-separated form → per-token steps, modifiers preserved.
        assert_eq!(steps("ctrl+k l"), vec!["ctrl+k", "l"]);
        assert_eq!(steps("g ctrl+d"), vec!["g", "ctrl+d"]);
        // The `space` alias canonicalizes to a literal space, in any position.
        assert_eq!(steps("space"), vec![" "]);
        assert_eq!(steps("ctrl+k space"), vec!["ctrl+k", " "]);
    }

    #[test]
    fn deserialize_coerces_scalar_int_and_bool_to_string() {
        // A bare digit (a tab-switch positional key written unquoted) must not
        // break the owning view — it deserializes as the string key "1".
        let kb: KeyBinding = serde_yaml::from_str("1").unwrap();
        assert!(kb.matches("1"));
        // YAML 1.1 bool-like scalars coerce too.
        let kb: KeyBinding = serde_yaml::from_str("true").unwrap();
        assert!(kb.matches("true"));
        // Ordinary string and list forms are unaffected.
        let kb: KeyBinding = serde_yaml::from_str("ctrl+k").unwrap();
        assert!(kb.matches("ctrl+k"));
        let kb: KeyBinding = serde_yaml::from_str("[a, \"ctrl+k l\"]").unwrap();
        assert!(kb.matches("a"));
        assert!(kb.matches_chord("ctrl+k", "l"));
    }

    #[test]
    fn section_deserialize_skips_unknown_action_names() {
        // A `tui.yaml` that still binds a since-removed action (here the
        // retired `tracking_toggle`) must load anyway: the unknown key is
        // dropped, known keys still override the defaults.
        let section: KeyBindingSection<CommonAction> =
            serde_yaml::from_str("tracking_toggle: s\ncolumn_config: x\n").unwrap();
        // Unknown action never materializes (there is no variant for it).
        // Known override took effect over the default `c`.
        assert!(
            section
                .get(&CommonAction::ColumnConfig)
                .unwrap()
                .matches("x")
        );
        // Untouched defaults remain intact.
        assert!(
            section
                .get(&CommonAction::FormClose)
                .unwrap()
                .matches("esc")
        );
    }

    #[test]
    fn matches_sequence_needs_exact_completed_steps() {
        let kb = KeyBinding::new("ctrl+k l");
        assert!(kb.matches_sequence(&["ctrl+k".into(), "l".into()]));
        // Partial sequence does not match — it is only a prefix.
        assert!(!kb.matches_sequence(&["ctrl+k".into()]));
        assert!(kb.is_sequence_prefix(&["ctrl+k".into()]));
        // Wrong first step neither matches nor is a prefix.
        assert!(!kb.is_sequence_prefix(&["ctrl+j".into()]));
    }

    #[test]
    fn modifier_bearing_chord_completes_via_matches_chord() {
        let kb = KeyBinding::new("ctrl+k l");
        // pending is the leader step, key is the final step.
        assert!(kb.matches_chord("ctrl+k", "l"));
        assert!(!kb.matches_chord("ctrl+k", "j"));
    }

    #[test]
    fn list_form_is_alternatives_space_form_is_sequence() {
        // A YAML list: any single alternative fires.
        let alts = KeyBinding::multi(vec!["a", "ctrl+k l"]);
        assert!(alts.matches("a"));
        assert!(alts.matches_chord("ctrl+k", "l"));
        assert!(alts.is_prefix("ctrl+k"));
        // `a` alone is not a prefix (it is a complete alternative).
        assert!(!alts.is_prefix("a"));
    }

    #[test]
    fn is_prefix_treats_zr_as_chord_prefix_for_z() {
        let kb = KeyBinding::new("zR");
        assert!(kb.is_prefix("z"));
    }

    #[test]
    fn is_prefix_rejects_named_function_key_f12_as_prefix_for_f() {
        let kb = KeyBinding::new("f12");
        assert!(!kb.is_prefix("f"));
    }

    #[test]
    fn is_prefix_rejects_modifier_prefixed_binding() {
        let kb = KeyBinding::new("ctrl+x");
        assert!(!kb.is_prefix("c"));
    }

    #[test]
    fn is_prefix_treats_gg_as_chord_prefix_alongside_named_home() {
        let kb = KeyBinding::multi(vec!["home", "gg"]);
        assert!(kb.is_prefix("g"));
    }

    #[test]
    fn is_prefix_treats_glm_as_chord_prefix_for_g_and_gl() {
        let kb = KeyBinding::new("glm");
        assert!(kb.is_prefix("g"));
        assert!(kb.is_prefix("gl"));
    }

    #[test]
    fn is_prefix_rejects_named_keys_as_chord_prefixes() {
        for name in [
            "enter",
            "esc",
            "tab",
            "backspace",
            "delete",
            "up",
            "down",
            "left",
            "right",
            "home",
            "end",
            "pageup",
            "pagedown",
        ] {
            let kb = KeyBinding::new(name);
            // First char of the name must not be treated as a chord
            // prefix — that would break single-key bindings like `e`/`u`.
            let first: String = name.chars().take(1).collect();
            assert!(
                !kb.is_prefix(&first),
                "named key `{name}` was mis-detected as chord prefix for `{first}`"
            );
        }
    }

    #[test]
    fn is_prefix_does_not_match_length_one_binding_against_itself() {
        let kb = KeyBinding::new("z");
        assert!(!kb.is_prefix("z"));
    }

    #[test]
    fn empty_binding_never_matches_and_is_no_prefix() {
        // The interactive editor writes a deliberately-disabled binding as
        // an empty list (`quit: []`). An empty binding must be inert at
        // every dispatch entry point — it fires nothing and starts no chord.
        let kb = KeyBinding(Vec::new());
        assert!(!kb.matches("ctrl+c"));
        assert!(!kb.matches("a"));
        assert!(!kb.matches_sequence(&[]));
        assert!(!kb.matches_chord("ctrl+k", "l"));
        assert!(!kb.is_prefix("g"));
        assert!(!kb.is_sequence_prefix(&["g".to_string()]));
        assert!(kb.step_lists().is_empty());
    }

    #[test]
    fn empty_list_overrides_a_builtin_default_but_leaves_others_intact() {
        // `quit: []` in tui.yaml must *override* the built-in `ctrl+c`
        // default with an empty (disabled) binding — not be ignored, and
        // not wipe the sibling defaults in the same section.
        let cfg: KeyBindingConfig = serde_yaml::from_str("global:\n  quit: []\n").unwrap();

        // The override is present and empty (so dispatch is dead).
        let quit = cfg
            .global
            .get(&GlobalAction::Quit)
            .expect("quit key survives the merge, present-but-empty");
        assert!(quit.0.is_empty(), "quit must be disabled, got {quit:?}");
        assert!(!quit.matches("ctrl+c"));

        // Untouched sibling defaults are preserved by the merge.
        assert_eq!(
            cfg.global
                .get(&GlobalAction::ShortcutMenu)
                .map(|b| b.matches("ctrl+y")),
            Some(true),
            "non-overridden defaults must remain"
        );
    }
}
