//! The single discriminator for "which momentary surface is active".
//!
//! A *surface* is anything that can be briefly active and wants to be
//! reflected in the chrome — a mode armed (fuzzy filter, jump), a popup open
//! (query menu, column config, shortcut menu), an editor focused, a
//! confirmation pending. Rather than let every bar builder guess active-ness
//! from a description string, each shortcut hint carries the [`ActiveSurface`]
//! it belongs to, and a single resolver maps that surface against live UI
//! state once per frame.
//!
//! The enum is intentionally *not* fieldless: [`ActiveSurface::Editor`] and
//! [`ActiveSurface::ContentAction`] carry an identity so that two
//! editor-opening (or action-opening) shortcuts do not light up at once — only
//! the one whose label / action id matches the focused surface does.
//!
//! Two families live here side by side:
//! - **content-tab surfaces** — owned by the focused content view / pane
//!   (editor, confirm, the query/group menus, fuzzy, search, jump, tracking,
//!   move-mark, script, content-action). Resolved by
//!   `ContentView::resolve_active`.
//! - **app-native surfaces** — owned directly by `App` (the shortcut menu).
//!   Resolved by `App` and carried by a global action's
//!   [`crate::config::keybindings::BarPlacement`].

/// Why a shortcut hint can light up: the surface it opens or arms. Carried on
/// each hint from build time so the resolver — not the renderer — decides
/// active-ness from real state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveSurface {
    // -- content-tab surfaces (owned by the focused content view / pane) -----
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
    /// A local `/`-search or tree-find input (or its cached result set) is
    /// active. Deliberately *not* the adapter text search — that one keeps
    /// filtering after its input closes and has its own surface
    /// ([`ActiveSurface::TextSearch`]), so the two never light up together.
    Search,
    /// An adapter text search (`text_search`) is active: its input is open,
    /// or the query it produced is still the pane's active query. Unlike
    /// [`ActiveSurface::Search`] this outlives the input — the pane keeps
    /// showing the search result until another query replaces it, and the
    /// hint stays lit for exactly that long.
    TextSearch,
    /// Jump (vimium-hop) mode is open in the focused pane.
    Jump,
    /// Tracking is running.
    Tracking,
    /// A node is armed on the move-clipboard (cut).
    MarkMove,
    /// A detached script is running.
    Script,
    /// A modal `custom` content action (a menu→editor flow such as Taiga
    /// `convert`) is in progress: its target picker popup is open, or the
    /// editor it opened is focused. The string is the action's stable id
    /// (e.g. `"convert"`); it matches the open popup's action id, or an
    /// active content editor whose action id equals it or is prefixed by
    /// `"<id>:"` (so `"convert"` covers `"convert:userstory"`).
    ContentAction(String),

    // -- app-native surfaces (owned directly by `App`) -----------------------
    /// The shortcut menu popup (every configured keybinding) is open.
    ShortcutMenu,
}
