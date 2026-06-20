//! View components — each tab/sub-view is an autonomous component.

pub mod column_format;
pub mod content_action_hints;
pub mod content_detail;
pub mod content_tree;
pub mod content_view;
pub mod focus_node;
pub mod group_aggregate;
pub mod markdown;

use ratatui::Frame;
use ratatui::layout::Rect;
use uuid::Uuid;

use not_yet_done_content::{CursorIntent, PageInfo, PageRequest, SortKey, SortableColumn};

use crate::app::SavedQuery;
use crate::config::view_config::ChildDef;
use crate::views::content_view::PaneId;

// ---------------------------------------------------------------------------
// Messages from sub-views to their parent view
// ---------------------------------------------------------------------------

/// A hint entry for the action bar or status bar: (key_label, description).
pub type BarHint = (String, String);

/// Messages a sub-view sends to its parent (e.g. TasksListView → TasksView).
#[derive(Debug)]
pub enum SubViewMessage {
    /// Initial or updated action bar hints.
    ActionBarHints(Vec<BarHint>),
    /// Initial or updated status bar hints.
    StatusBarHints(Vec<BarHint>),

    /// Fuzzy filter state changed.
    FuzzyStateChanged {
        active: bool,
        query: String,
        cursor: usize,
    },
    /// The selected item changed.
    SelectionChanged(Option<Uuid>),

    /// An editor was opened (label for action bar highlight).
    EditorOpened(&'static str),
    /// The editor was closed.
    EditorClosed,

    /// Search state changed.
    SearchStateChanged {
        active: bool,
        query: String,
        match_count: usize,
        current: usize,
    },

    /// Request to be forwarded to the app (service calls, editor, etc.).
    Request(ViewRequest),

    /// ContentPane wants to drill into a child. The parent
    /// [`crate::views::content_view::ContentView`] intercepts this
    /// **before** the message escapes to the App layer: depending on
    /// `child_def.split`, it either applies `drill_down_prepare` on the
    /// focused pane (in-place — today's behavior) or allocates a new
    /// pane next to it and applies the prepare there. Either way, the
    /// outcome is a normal [`ViewRequest::DrillDown`] sent on to the App.
    ContentDrill {
        item_id: String,
        item_label: String,
        child_def: Box<ChildDef>,
    },

    /// Key was not handled — parent should try.
    Unhandled,
}

/// Requests from any view to the app (things the view can't do itself).
#[derive(Debug)]
pub enum ViewRequest {
    // Editor
    OpenEditorForSearch {
        entity: String,
        name: String,
    },
    OpenEditorForTrackingSearch {
        name: String,
    },

    // Service calls
    ToggleTracking(Uuid),

    // Popups (App manages these as overlays)
    OpenColumnConfig,
    /// Open the `:script` fuzzy menu seeded with the Trackings-tab
    /// context (filter's tracking ids + date bounds, legacy
    /// `<data_dir>/not_yet_done/tracking/scripts/` directory).
    OpenScriptMenuForTrackings,
    /// Open the `:script` fuzzy menu seeded with the selected content
    /// node's context. App reads the node from the focused pane and
    /// builds the per-`(tab, node_type)` scripts directory.
    OpenScriptMenuForNode {
        view_index: usize,
        pane_id: PaneId,
        /// `true` when the triggering action declared `scope: filtered_set`:
        /// hand the whole filtered row set + date bounds to the script
        /// (batch payload) instead of the single selected node.
        batch: bool,
    },
    /// Open the generic, adapter-driven option menu for the selected content
    /// node. Raised by a `type: option_menu` action. `config` is the menu
    /// recipe (options source, selection-marker field, toggle action) the App
    /// uses to async-fetch `list_values(source)` + the node's marker and open
    /// the popup. See [`crate::app::App::open_option_menu_for_content`].
    OpenOptionMenuForNode {
        view_index: usize,
        pane_id: PaneId,
        config: crate::config::view_config::OptionMenuConfig,
    },
    // Tracking-specific
    DeleteTracking,
    OpenTrackingGroupPopup,
    SaveTrackingGrouping(String),
    RestoreTracking,
    RestoreAllTrackings,

    // Data
    SpawnLoad,
    SpawnLoadTrackings,
    ApplyFilter(String),
    ApplyTrackingFilter(String),

    // UI
    Notify(String),
    ModalMessage(String),

    // Cmdline execution (view can't run processes)
    ExecuteCmdline(String),

    // Content views (generic adapter-driven)
    /// Fetch preview content for a node in a content view.
    /// `cache_key` is the pane's preview-key (the selected row's own
    /// id); `node_id` is what the adapter looks up — they differ when
    /// `preview.node_id_from` redirects to a linked node.
    /// `action_id` (when `Some`) routes through `Node::prepare` so the
    /// preview shows the same buffer as that action's editor; `None`
    /// uses the default `content().read_text()` source.
    FetchContentPreview {
        view_index: usize,
        pane_id: PaneId,
        cache_key: String,
        node_id: String,
        action_id: Option<String>,
    },
    /// Open editor for an `InputSpec::Editor` action on a content node.
    /// `action_id` is the node-side action identifier (e.g. `"edit_full"`).
    OpenContentEditor {
        view_index: usize,
        pane_id: PaneId,
        node_id: String,
        action_id: String,
        label: String,
        editor_profile: Option<String>,
        commit_on_save: bool,
    },
    /// Reload items for a content view (async).
    SpawnContentLoad {
        view_index: usize,
        pane_id: PaneId,
    },
    /// Apply a saved query to a pane via the App-side dispatcher that
    /// handles adapter-reported query variables (popup if any). When
    /// the adapter reports no variables, this falls through to a normal
    /// load — same effect as a `set_query` + `SpawnContentLoad` pair.
    ApplyContentSavedQuery {
        view_index: usize,
        pane_id: PaneId,
        query: String,
        name: String,
    },
    /// Toggle a content-view saved query as this tab's default —
    /// applied automatically on app start instead of the view-YAML
    /// `query.default`. Selecting the current default clears it.
    SetDefaultContentQuery {
        view_index: usize,
        name: String,
    },
    /// Drill down into a child node (async).
    DrillDown {
        view_index: usize,
        pane_id: PaneId,
        node_id: String,
        node_label: String,
        child_node_type: String,
    },
    /// Load the children of a tree node into the pane's tree cache
    /// at `parent_path`, then re-flatten its `entries`. Used by the
    /// expand-on-Enter path in tree mode. Mirrors `DrillDown` (calls
    /// `adapter.get_by_id(parent_node_id).list(child_node_type)`),
    /// but the response routes into `ContentPane.tree.cache` instead
    /// of replacing `pane.items` — the tree pane keeps showing its
    /// root list while children appear underneath.
    ExpandTreeNode {
        view_index: usize,
        pane_id: PaneId,
        parent_path: Vec<String>,
        parent_node_id: String,
        child_node_type: String,
        page_size: u32,
        /// Offset/limit for this load. `None` = first page (offset 0).
        /// Pagination placeholder activation passes the cached
        /// `next_page` so `spawn_tree_expand` asks the adapter for the
        /// next slice.
        page: Option<not_yet_done_content::PageRequest>,
        /// `true` when the load should append to the existing
        /// `cache[parent_path].children` (pagination placeholder).
        /// `false` for the initial expand (replace cache).
        append: bool,
    },
    /// Heterogeneous fan-out expand: the parent's ChildDef has N > 1
    /// tree-continuing children (e.g. Postgres `Schemas` + `Scripts`
    /// under a database). The pane records the expected node_types
    /// up front (via `begin_tree_multi_load`); the App fires one
    /// `ExpandTreeNode`-equivalent load per type. Each per-type
    /// response lands in its own bucket and the merged child list
    /// renders in YAML order regardless of arrival order.
    ExpandTreeNodeMulti {
        view_index: usize,
        pane_id: PaneId,
        parent_path: Vec<String>,
        parent_node_id: String,
        /// Node-types of all tree-continuing children of the entry's
        /// ChildDef, in YAML order. Determines both the loads issued
        /// and the merge order in the cache.
        child_node_types: Vec<String>,
        page_size: u32,
    },
    /// Eagerly load and expand the pane's WHOLE subtree in one
    /// `list_subtree(u32::MAX)` call. Fired when a fuzzy filter is opened on an
    /// eager tree (`supports_eager_subtree`) so the filter can match across
    /// collapsed and not-yet-paged branches — the native "filter sees the
    /// entire forest" behaviour. Routes into [`App::spawn_subtree_load`]; the
    /// pre-filter expansion is restored locally when the filter clears.
    EagerExpandSubtree {
        view_index: usize,
        pane_id: PaneId,
    },
    /// Drive the pane's one-shot auto-expand cascade now. Raised by
    /// `content.tree_expand_all` (`zr`) after it arms the pane's
    /// unbounded-depth override: the App calls [`App::drive_tree_auto_expand`]
    /// (the same entry point a fresh tree load uses), which pumps
    /// `pending_auto_expand_requests` and dispatches the resulting
    /// `ExpandTreeNode` loads.
    DriveTreeAutoExpand {
        view_index: usize,
        pane_id: PaneId,
    },
    /// Open editor for the content view's query (JQL etc.).
    /// If `save_name` is set, the query will be saved to DB under that name after editing.
    /// `is_new` triggers the shortcut prompt after save (only on creation, not edit).
    OpenContentQueryEditor {
        view_index: usize,
        pane_id: PaneId,
        save_name: Option<String>,
        is_new: bool,
    },
    /// Open editor for an adapter-native, free-form query (e.g. raw SQL
    /// for Postgres). Triggered at a drill-down level where the active
    /// pane shows rows of a single addressable container (table, sheet,
    /// …). Carries the parent node id so the App can extract the
    /// adapter-specific addressing (database / schema / table).
    OpenAdapterQueryEditor {
        view_index: usize,
        pane_id: PaneId,
        parent_node_id: String,
    },
    /// Open the Postgres per-table scripts menu (the new `q` keybind
    /// on the `tables` subtab). App parses `table_node_id`, lists the
    /// scripts under `<instance_data_dir>/queries/<db>/<schema>/<table>/`
    /// and calls back into the `ContentView` to populate the popup.
    OpenPostgresScriptsMenu {
        view_index: usize,
        pane_id: PaneId,
        table_node_id: String,
    },
    /// Run a Postgres script for `(db, schema, table, script)` against
    /// the adapter and display the result in the focused pane.
    RunPostgresScript {
        view_index: usize,
        pane_id: PaneId,
        database: String,
        schema: String,
        table: String,
        script: String,
    },
    /// Re-execute a free-form Postgres custom query with a different
    /// page offset. Triggered by the pane's next/prev-page keys when
    /// the pane is in custom-query mode (i.e. its items came from the
    /// Q-editor or a script run, not a regular `list()` call). The
    /// `query` is the unwrapped SQL text the user wrote; the adapter
    /// wraps it with `LIMIT/OFFSET` itself.
    ///
    /// `cursor` opts into the cursor-pagination lifecycle (CP-5):
    /// `Some(Open)` opens a fresh server-side cursor and discards
    /// `page`; `Some(Continue { id })` fetches the next chunk from an
    /// existing cursor; `Some(Close { id })` tears the cursor down.
    /// `None` keeps the legacy LIMIT/OFFSET path.
    RunPostgresQuery {
        view_index: usize,
        pane_id: PaneId,
        database: String,
        query: String,
        page: PageRequest,
        cursor: Option<CursorIntent>,
    },
    /// Close an adapter-side cursor by id (CP-6). Emitted when a pane
    /// that was paginating via a server-side cursor is destroyed —
    /// e.g. `wq` close, parent close cascading into a coupled child,
    /// or a coupled split's `linked_child` being hot-replaced.
    /// Fire-and-forget: App spawns a tokio task that calls
    /// `adapter.execute_custom_query("", with_cursor(Close{id}))`; any
    /// error is dropped (the cursor either still exists and we'll leak
    /// one idle TX until connection teardown, or it's already gone).
    CloseAdapterCursor {
        view_index: usize,
        cursor_id: String,
    },
    /// Run a DB-level Postgres script in a freshly-opened result pane
    /// (CP-8). Emitted from `ActionDispatch::ExecuteQuery { paged: true }`
    /// on `postgres:db_script` nodes — the `x` shortcut. The App reads
    /// the pre-extracted SQL, splits the source pane into the
    /// `postgres:db_script_result` ChildDef, and spawns a cursor-paginated
    /// custom-query against the new pane.
    ///
    /// `source_node_id` / `source_label` are used to drill the new pane
    /// into the same script the user selected, so the back-nav stack
    /// shows the right ancestry.
    RunAdapterDbScript {
        view_index: usize,
        pane_id: PaneId,
        source_node_id: String,
        source_label: String,
        database: String,
        sql: String,
    },
    /// Open the SQL editor for a DB-level script (CP-8). Emitted from
    /// `ActionDispatch::OpenEditor { session_kind: "script_editor" }`
    /// — the `e` shortcut on `postgres:db_script` rows. `:w` persists
    /// without re-executing; the user re-runs explicitly via `x` to
    /// avoid coupling the edit path to result-pane lifetime.
    OpenAdapterDbScriptEditor {
        view_index: usize,
        pane_id: PaneId,
        database: String,
        script: String,
        /// EIP — when true, the editor's temp file is created in the
        /// script's real directory (with a `.nyd_tmp_` prefix) so
        /// LSPs / external tools discover sibling config files.
        in_place: bool,
    },
    /// CP-9 / DSF-4: open the cmdline pre-filled so the user types only
    /// the new script name. `parent_rel` (DSF-4) is the rel-path of the
    /// dir under which the script should be created — empty for root.
    /// Emitted from `ActionDispatch::CreateChild { hint: "db_script:<db>[:<parent_rel>]" }`.
    OpenDbScriptNewPrompt {
        view_index: usize,
        pane_id: PaneId,
        database: String,
        parent_rel: String,
    },
    /// DSF-4: open the cmdline pre-filled for creating a new empty
    /// directory under `parent_rel` (empty = root). Emitted from
    /// `ActionDispatch::CreateChild { hint: "db_script_dir:<db>[:<parent_rel>]" }`.
    OpenDbScriptDirNewPrompt {
        view_index: usize,
        pane_id: PaneId,
        database: String,
        parent_rel: String,
    },
    /// CP-9 / DSF-4: confirm + delete a DB-level script file. The
    /// `script` field carries the full rel-path (may contain `/`)
    /// so nested scripts under directories work uniformly.
    ConfirmDeleteAdapterDbScript {
        view_index: usize,
        pane_id: PaneId,
        database: String,
        script: String,
    },
    /// DSF-4: confirm + delete an (empty) DB-script directory. The
    /// adapter rejects non-empty dirs with a "not empty (N)" error,
    /// which the TUI surfaces via `Notify` after the delete spawn.
    ConfirmDeleteAdapterDbScriptDir {
        view_index: usize,
        pane_id: PaneId,
        database: String,
        rel_path: String,
    },
    /// CF-11: generic content-node delete with confirmation. Emitted
    /// from `ActionDispatch::DeleteSelf` for any adapter that hasn't
    /// claimed a typed confirm route (currently: every adapter except
    /// Postgres' `db_script`/`db_script_dir`). On accept the App spawns
    /// `Node::execute("delete", ActionInput::None)` on the same node and
    /// reloads the pane on `ActionOutcome::Done`. The adapter is the
    /// authoritative delete logic — the TUI just stages confirmation +
    /// refresh; no per-adapter handler lives in `app/`.
    ConfirmDeleteContentNode {
        view_index: usize,
        pane_id: PaneId,
        node_id: String,
        /// The action that produced this `DeleteSelf` — re-run verbatim on
        /// confirm via `Node::execute`. Lets one adapter expose several
        /// delete flavours on the same node type (e.g. the tasks tree's
        /// recursive `delete` vs the flat list's single `delete-single`).
        action_name: String,
        /// Adapter-supplied confirmation prompt (e.g. a recursive-delete
        /// warning). `None` → the App builds the generic `Delete '<label>'?`.
        confirm: Option<String>,
    },
    /// Generic confirm-then-invoke: emitted from `ActionDispatch::Confirm`
    /// when an adapter wants a `(y/n)` prompt before doing (often
    /// irreversible) work. On accept the App re-invokes the *same* action
    /// on the *same* node with `ActionContext::confirmed = true`, so the
    /// adapter then performs the work instead of asking again. Used by the
    /// trackings adapter's `restore` / `restore-all` (which purge successor
    /// intervals).
    ConfirmInvokeNodeAction {
        view_index: usize,
        pane_id: PaneId,
        node_id: String,
        action_name: String,
        /// Adapter-authored prompt; the adapter knows what the action will
        /// do (e.g. how many successor intervals a restore purges).
        prompt: String,
    },
    /// Invoke an adapter action on the pane's *container* (the adapter
    /// root), not on the selected row. Emitted for `actions:` entries
    /// flagged `on_container: true` (e.g. trackings `restore all`), which
    /// must fire even at the un-drilled flat root where no row — and no
    /// `parent:` target — is addressable. The App resolves `adapter.root()`
    /// and dispatches through the normal `invoke_action` path.
    InvokeContainerAction {
        view_index: usize,
        pane_id: PaneId,
        action_name: String,
    },
    /// DSF-4: open a rename prompt for an existing script or dir.
    /// `is_dir` lets the App pick the right storage call
    /// (`rename_db_script_entry` is shape-agnostic but the confirm
    /// message wording differs).
    OpenDbScriptRenamePrompt {
        view_index: usize,
        pane_id: PaneId,
        database: String,
        rel_path: String,
        is_dir: bool,
    },
    /// DSF-4: stash the focused node id as the marked source for a
    /// subsequent move. App stores it in `marked_db_script_for_move`
    /// and shows a status-bar indicator until the move completes or
    /// is cleared with Esc.
    MarkDbScriptForMove {
        node_id: String,
    },
    /// DSF-4: paste the previously-marked node into `target_node_id`
    /// (a dir or the db_scripts group). App fetches the marked source,
    /// calls `move_db_script_entry`, reloads, and clears the mark.
    PasteDbScriptMove {
        target_node_id: String,
    },
    /// Open the SQL editor for an existing or brand-new Postgres script
    /// under `(db, schema, table)`. When `is_new` we still create the
    /// file on first save; otherwise it's read from disk.
    EditPostgresScript {
        view_index: usize,
        pane_id: PaneId,
        database: String,
        schema: String,
        table: String,
        script: String,
        is_new: bool,
    },
    /// Remove a Postgres script `.sql` file and its sidecar shortcut.
    DeletePostgresScript {
        view_index: usize,
        pane_id: PaneId,
        database: String,
        schema: String,
        table: String,
        script: String,
    },
    /// Prompt the user for a key chord to bind to a Postgres script;
    /// captured key is written to `.shortcuts.yaml` alongside the file.
    PromptPostgresScriptShortcut {
        view_index: usize,
        pane_id: PaneId,
        database: String,
        schema: String,
        table: String,
        script: String,
    },
    /// Execute a custom action on a content node (e.g. Jira transition).
    /// If the action needs_input, App fetches options and shows a popup first.
    ExecuteContentAction {
        view_index: usize,
        pane_id: PaneId,
        node_id: String,
        action_id: String,
    },
    /// Invoke a per-node shortcut action (Phase CP-1c). The pane
    /// resolved `key → action_name` via `app::node_actions::resolve_shortcut`
    /// and emits this request. App spawns an async task that loads
    /// the node, calls `Node::invoke_action(action_name)`, then turns
    /// the resulting `ActionDispatch` into a follow-up `ViewRequest`
    /// via `app::node_actions::dispatch_to_view_request`.
    InvokeNodeAction {
        view_index: usize,
        pane_id: PaneId,
        node_id: String,
        action_name: String,
    },
    /// Drop the cached auth session for a content view's adapter and
    /// trigger a reload — the next list call drives a fresh login.
    /// Resolver caches (cookie/keyring values) are kept; only the
    /// derived session blob is wiped. Surfaced as the `invalidate_session`
    /// action type and the `:invalidate-session` cmdline command.
    InvalidateContentSession {
        view_index: usize,
    },
    /// Drop session AND every credential resolver / prompt cache so the
    /// next login re-resolves from scratch. Forces a re-prompt for any
    /// `prompt`-provider field. Surfaced as the `invalidate_credentials`
    /// action type and the `:invalidate-credentials` cmdline command.
    InvalidateContentCredentials {
        view_index: usize,
    },
    /// Create a new child node (e.g. add comment to an issue).
    /// Opens an editor for the new content body.
    /// `action_id` is the node-side action identifier on the parent
    /// (e.g. `"create_comment"`).
    CreateContentChild {
        view_index: usize,
        pane_id: PaneId,
        /// Node the create action is invoked on. `None` means the adapter's
        /// root container — used when nothing is selected (empty tree) or a
        /// non-`under_selection` create fires at the un-drilled root level,
        /// where neither a selected row nor a nav-stack parent exists. The
        /// async handler resolves it to `adapter.root().id()`.
        parent_node_id: Option<String>,
        child_node_type: String,
        action_id: String,
        label: String,
        editor_profile: Option<String>,
        commit_on_save: bool,
    },
    /// Save a content query to the DB (upsert by scope+name).
    SaveContentQuery {
        view_index: usize,
        scope: String,
        name: String,
        query: String,
    },
    /// Delete a content query from the DB by scope+name.
    DeleteContentQuery {
        view_index: usize,
        scope: String,
        name: String,
    },
    /// Prompt the user for a shortcut key for a content query.
    PromptContentQueryShortcut {
        view_index: usize,
        scope: String,
        name: String,
        query: String,
    },
    /// Rename a content query in the DB.
    RenameContentQuery {
        view_index: usize,
        scope: String,
        old_name: String,
        new_name: String,
    },

    /// CT-6: kick off an adapter-side tree search for the active
    /// pane. The App spawns `adapter.search_in_tree(query, limit)` and
    /// lands the result on `pane.tree_find` via `tree_find_complete`
    /// / `tree_find_fail`. `query` is the raw user input; the adapter
    /// is responsible for any escaping or whitelist scoping (e.g.
    /// Confluence's `space in (...)` injection). Emitted by the
    /// `tree_find`-type action on Enter in the search bar.
    TreeFindStart {
        view_index: usize,
        pane_id: PaneId,
        query: String,
    },

    // ── Saved-query menu (tasks / trackings) ─────────────────────────
    /// Apply a saved query to the active filter (scope routes to the right view).
    ApplySavedQuery {
        scope: String,
        content: String,
    },
    /// Open the YAML editor for a saved query, either to edit (`current_query=Some`)
    /// an existing one or create a new one (`is_new=true`, `current_query=None`).
    OpenSavedQueryEditor {
        scope: String,
        name: String,
        current_query: Option<String>,
        is_new: bool,
    },
    /// Delete a saved query by scope+name.
    DeleteSavedQuery {
        scope: String,
        name: String,
    },
    /// Prompt the user for a shortcut key for a saved query.
    PromptSavedQueryShortcut {
        scope: String,
        name: String,
        query: String,
    },
    /// Toggle a saved query as the per-scope default (applied on app
    /// start). Selecting the current default clears it.
    SetDefaultSavedQuery {
        scope: String,
        name: String,
    },
}

// ---------------------------------------------------------------------------
// Capability traits — views opt in to features they support
// ---------------------------------------------------------------------------

/// Snapshot of the search component state for sync_components.
#[derive(Debug, Clone, Default)]
pub struct SearchState {
    pub active: bool,
    pub query: String,
    pub cursor: usize,
    pub match_count: usize,
    pub current: usize,
}

/// Snapshot of the cmdline component state for sync_components.
#[derive(Debug, Clone, Default)]
pub struct CmdlineState {
    pub active: bool,
    pub query: String,
    pub cursor: usize,
}

/// Result of cmdline key handling.
#[derive(Debug)]
pub enum CmdlineKeyResult {
    /// User pressed Enter with a non-empty command string.
    Execute(String),
    /// Cmdline was closed (Esc or Enter on empty).
    Closed,
    /// Key was handled (char input, cursor move, etc.).
    Handled,
}

/// Result of search key handling.
#[derive(Debug)]
pub enum SearchKeyResult {
    /// User pressed Enter — close search, keep matches.
    Accepted,
    /// User pressed Esc on empty query — close and clear.
    Cancelled,
    /// Query changed — caller should update matches and possibly jump.
    QueryChanged,
    /// Key was handled but query didn't change (cursor move, etc.).
    Handled,
}

/// Info returned when a favorite is activated.
#[derive(Debug, Clone)]
pub struct FilterActivation {
    pub filter_json: String,
    pub filter_name: String,
}

/// Capability: view supports text search (/).
pub trait Searchable {
    fn search_active(&self) -> bool;
    fn search_state(&self) -> SearchState;
    fn search_open(&mut self);
    fn search_close(&mut self);
    fn search_clear(&mut self);
    fn search_handle_key(&mut self, key: &str) -> SearchKeyResult;
    fn search_jump(&mut self, direction: isize);
}

/// Capability: view supports the command line (:).
pub trait HasCmdline {
    fn cmdline_active(&self) -> bool;
    fn cmdline_state(&self) -> CmdlineState;
    fn cmdline_open(&mut self);
    /// Open the cmdline with `prefill` pre-typed (cursor at end). Used
    /// by adapter actions that want the user to finish a partially-
    /// typed command (e.g. CP-9 `:db-script-new <db> ` prompt).
    fn cmdline_open_with(&mut self, prefill: &str);
    fn cmdline_close(&mut self);
    fn cmdline_handle_key(&mut self, key: &str) -> CmdlineKeyResult;
}

/// Capability: view has filters and favorites.
pub trait Filterable {
    fn active_filter_name(&self) -> Option<&str>;
    fn favorites(&self) -> &[SavedQuery];
    fn try_activate_favorite(&mut self, key: &str) -> Option<FilterActivation>;
}

/// Capability: view manages its own popups.
pub trait HasPopups {
    fn has_popup(&self) -> bool;
    fn handle_popup_key(&mut self, key: &str) -> Vec<SubViewMessage>;
    fn render_popups(&self, frame: &mut Frame, area: Rect);
}

/// Capability: view supports column sort. Implementors decide whether
/// the sort is applied server-side (adapter) or in-memory (Tasks).
pub trait SortableView {
    /// Columns the view can sort on. Empty = no sort UI offered.
    fn sortable_columns(&self) -> Vec<SortableColumn>;

    /// User's current desired sort. Empty = view's natural default.
    fn current_sort(&self) -> &[SortKey];

    /// Replace the current sort spec. Returns `true` if the value
    /// changed and the caller should trigger a reload / re-sort.
    fn set_current_sort(&mut self, sort: Vec<SortKey>) -> bool;

    /// Sort actually applied to the visible data on the last refresh.
    /// Empty if the view hasn't been refreshed yet.
    fn last_applied_sort(&self) -> &[SortKey];
}

/// Capability: view supports server-side pagination. Views that always
/// load the full dataset (Tasks/Trackings) do not implement this.
pub trait PaginatedView {
    fn current_page(&self) -> Option<PageRequest>;
    fn set_current_page(&mut self, page: Option<PageRequest>) -> bool;
    fn last_page_info(&self) -> Option<PageInfo>;
    fn next_page_request(&self) -> Option<PageRequest>;
    fn prev_page_request(&self) -> Option<PageRequest>;
}
