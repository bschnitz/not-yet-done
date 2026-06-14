use std::sync::Arc;
use std::time::Instant;

use not_yet_done_core::entity::task::Model as Task;
use std::collections::HashSet;

use not_yet_done_core::repository::{
    LinkRepository, QueryShortcutRepository, SavedQueryRepository, SettingsRepository,
    TrackingRepository,
};
use not_yet_done_core::service::TaskService;
use not_yet_done_ratatui::{DetachedEditor, FilePicker, FilePickerEvent};

use uuid::Uuid;

use crate::action::{self, Action};
use crate::components::data_table::DataTable;
use crate::views::content_view::ContentView;
use crate::views::tasks_view::TasksView;
use crate::views::trackings_view::TrackingsView;
use crate::views::{SubViewMessage, ViewRequest};
use crate::components::form_pane::FormPaneComponent;
use crate::components::view_pane::ViewPaneComponent;
use crate::components::notification_bar::NotificationBarComponent;
use crate::components::query_error_bar::QueryErrorBarComponent;
use crate::components::searchable_popup::{SearchablePopup, PopupItem};
use crate::components::content_form_popup::{ContentFormPopup, ContentFormEvent};
use crate::components::status_bar::{StatusBarComponent, StatusMode};
use crate::components::tab_bar::TabBarComponent;
use crate::config::{
    CommonAction, FormAction, GlobalAction, KeyBindingConfig, TasksAction, TrackingsAction,
    TuiConfig,
};
use crate::filter_builder;
use crate::tabs::{FilterField, LoadState, Tab, TabLayout, TasksSubView, TrackingsSubView, TasksForm};
use crate::ui::theme::Theme;

// ---------------------------------------------------------------------------
// Messages from the async loader back to the main thread
// ---------------------------------------------------------------------------

/// Which adapter-level invalidation `spawn_invalidate_auth` should perform.
#[derive(Copy, Clone, Debug)]
enum AuthInvalidate {
    /// Drop only the cached session blob.
    Session,
    /// Drop the session AND every resolver / prompt cache.
    Credentials,
}

pub enum LoadMsg {
    Tasks(Vec<Task>),
    /// Tag-by-task map, sent after [`LoadMsg::Tasks`] once the
    /// task-tag junction queries finish.
    TaskTags(std::collections::HashMap<Uuid, Vec<not_yet_done_core::repository::ResolvedTag>>),
    Trackings(Vec<crate::tabs::TrackingRow>),
    Error(String),
    TrackingError(String),
    /// Async-loaded items for a content view.
    ContentItems {
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        items: Vec<not_yet_done_content::NodeSummary>,
        applied_sort: Vec<not_yet_done_content::SortKey>,
        page: Option<not_yet_done_content::PageInfo>,
        sortable_columns: Vec<not_yet_done_content::SortableColumn>,
        error: Option<String>,
    },
    /// Async-loaded preview for a content view node. `cache_key` is the
    /// pane's `preview_key` (the selected row's own id) — must match
    /// regardless of whether the fetch was redirected via
    /// `preview.node_id_from`.
    ContentPreview {
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        cache_key: String,
        text: String,
    },
    /// A `NodeActionEditSession` finished its off-thread `prepare` and is
    /// ready to open in `$EDITOR`. Built off-thread (see
    /// `ViewRequest::OpenContentEditor`) so the network-heavy prepare —
    /// metadata fetches, comment loads — never blocks the render thread.
    /// `token` is the generation stamp captured when the load was
    /// spawned; a mismatch means a newer open (or a cancel) superseded
    /// this one and the stale session is dropped. `node_id` is echoed
    /// back only for the error notification.
    EditorSessionReady {
        node_id: String,
        token: u64,
        result: Result<Box<dyn crate::edit_session::EditSession>, String>,
    },
    /// Custom action completed on a content node. `result` is `Ok(msg)`
    /// when the adapter call succeeded, `Err(msg)` when it failed —
    /// failures are surfaced in the inline error bar AND remembered as
    /// `last_error` so the user can reopen the message in `$EDITOR`.
    ContentActionDone {
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        result: Result<String, String>,
    },
    /// Result of an async per-node shortcut invocation (Phase CP-1c).
    /// Carries the `ActionDispatch` returned by `Node::invoke_action`
    /// (or a load/invoke error) so the main loop can translate it via
    /// `app::node_actions::dispatch_to_view_request` into the next
    /// follow-up request. The `node_id` + `action_name` are echoed
    /// back so the dispatcher can include them in user-facing
    /// notifications (e.g. "node-action 'edit' not implemented").
    NodeActionDispatched {
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        node_id: String,
        action_name: String,
        result: Result<not_yet_done_content::ActionDispatch, String>,
        /// Label + type of the resolved node, captured while it was
        /// fetched in the spawn task (M7/E6). `None` when the fetch
        /// itself failed. Used to build the [`MarkedNode`] for a
        /// `mark-move` without a second `get_by_id` roundtrip.
        node_label: Option<String>,
        node_type: Option<not_yet_done_content::NodeType>,
    },
    /// Live connection-status update from a content adapter (e.g.
    /// `Connecting`, `Ready`, `Failed`). Pushed by a background task
    /// that watches the adapter's status channel.
    ContentAdapterStatus {
        view_index: usize,
        status: not_yet_done_content::AdapterStatus,
    },
    /// Out-of-band content-change signal from a streaming adapter (the
    /// Stoat gateway). Pushed by `spawn_content_invalidation_watcher`;
    /// `poll_load` reloads the affected pane(s)' current level. See
    /// [`not_yet_done_content::Invalidation`].
    AdapterInvalidation {
        view_index: usize,
        inv: not_yet_done_content::Invalidation,
    },
    /// Result of an interactive credential submission. `Ok` keeps the
    /// popup in submitting state until the status flips to `Ready` (the
    /// flip closes the popup); `Err` re-opens the form with the message.
    CredentialSubmitResult {
        view_index: usize,
        error: Option<String>,
    },
    /// Async-loaded result of a custom adapter query (e.g. SQL via the
    /// Postgres Q-editor or a page-flip on a SELECT result). Routed to
    /// `cv.apply_custom_query_result` so the pane stays in custom-query
    /// mode and the next/prev-page keys can re-execute. `Err` is
    /// surfaced as a notification and the pane is left untouched.
    CustomQueryItems {
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        result: Result<CustomQueryItemsPayload, String>,
    },
    /// Children of an expanded tree node, loaded by `spawn_tree_expand`
    /// in response to a `ViewRequest::ExpandTreeNode`. Routes into the
    /// pane's `tree.cache[parent_path]`; an `Err` is surfaced as a
    /// notification and the row is left in the collapsed state.
    TreeChildren {
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        parent_path: Vec<String>,
        result: Result<TreeChildrenPayload, String>,
        append: bool,
    },
    /// A whole eagerly-expanded subtree, loaded by `spawn_subtree_load`
    /// for an adapter that advertises `supports_eager_subtree`. Lands via
    /// [`ContentView::apply_subtree`], which fills every tree level and marks
    /// the expanded nodes in one pass — the eager replacement for the
    /// per-node [`Self::TreeChildren`] cascade. `parent_path` is the path the
    /// subtree hangs under (`vec![]` for a root load).
    Subtree {
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        parent_path: Vec<String>,
        result: Result<not_yet_done_content::Subtree, String>,
    },
    /// In-flight retry progress for a failed content/drill/tree load on
    /// `view_index` / `pane_id`. Updates the pane's `retry_state` so
    /// the auth-status banner reads `"Retrying (n/total): {err}"`
    /// while the next attempt is in progress. The final attempt
    /// (success or last failure) arrives as `ContentItems` /
    /// `TreeChildren` instead, which clears `retry_state` again.
    ContentLoadProgress {
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        attempt: u32,
        max_attempts: u32,
        last_error: String,
    },
    /// CT-6: result of an adapter-side tree search spawned in
    /// response to [`ViewRequest::TreeFindStart`]. `query` round-trips
    /// for late-arrival sanity checks (compare against the pane's
    /// current `tree_find.query` and drop the result when they no
    /// longer match — the user typed a new query before this one
    /// returned). `Ok(None)` means the adapter doesn't support tree
    /// search at all; surfaced to the user as an explicit notice.
    TreeFindResult {
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        query: String,
        result: Result<Option<not_yet_done_content::TreeSearchResults>, String>,
    },
}

/// Run an async fallible operation up to `1 + retries` times, emitting
/// a [`LoadMsg::ContentLoadProgress`] between attempts so the active
/// pane's banner can show `"Retrying (n/total): {err}"`. Used by every
/// `list()`-style spawn function on a content view (root load,
/// drill-down, tree expand). The factory closure is called per attempt
/// to rebuild the future from scratch — adapter calls cannot be
/// retried by polling the same future twice.
async fn run_with_retries<F, Fut, T>(
    retries: u32,
    tx: &tokio::sync::mpsc::UnboundedSender<LoadMsg>,
    view_index: usize,
    pane_id: crate::views::content_view::PaneId,
    mut op: F,
) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let max_attempts = retries.saturating_add(1);
    let mut last_error = String::new();
    for attempt in 1..=max_attempts {
        match op().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                last_error = e;
                if attempt < max_attempts {
                    let _ = tx.send(LoadMsg::ContentLoadProgress {
                        view_index,
                        pane_id,
                        attempt: attempt + 1,
                        max_attempts,
                        last_error: last_error.clone(),
                    });
                }
            }
        }
    }
    Err(last_error)
}

/// Successful payload for [`LoadMsg::TreeChildren`]. The pane uses
/// `page_info` (when `Some`) to derive a `next_page` cache hint so the
/// tree renderer can emit a `… N weitere` placeholder under the
/// expanded parent. `child_node_type` lets the receiver route the
/// items into the right per-type bucket when the parent is in
/// multi-load mode (heterogeneous fan-out).
pub struct TreeChildrenPayload {
    pub items: Vec<not_yet_done_content::NodeSummary>,
    pub page_info: Option<not_yet_done_content::PageInfo>,
    pub child_node_type: String,
}

/// Successful payload for [`LoadMsg::CustomQueryItems`]. Carries the
/// rows plus the state needed to remember the query for page-flips.
pub struct CustomQueryItemsPayload {
    pub items: Vec<not_yet_done_content::NodeSummary>,
    pub page: Option<not_yet_done_content::PageInfo>,
    pub custom_query: crate::views::content_view::CustomQueryRunState,
    pub status: Option<String>,
}

mod config_edit;
pub mod editor;
mod filter_persist;
mod link;
pub mod node_actions;
pub mod script;
mod tag_menu;

pub use editor::EditorRequest;

/// Serialize a sort spec to the compact `col:dir,col:dir` form persisted
/// in the `settings` table.
fn serialize_sort_state(sort: &[not_yet_done_content::SortKey]) -> String {
    use not_yet_done_content::SortDirection;
    sort.iter()
        .map(|k| {
            let dir = match k.direction {
                SortDirection::Asc => "asc",
                SortDirection::Desc => "desc",
            };
            format!("{}:{}", k.column, dir)
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Generate one label per sortable column. Single letters first, then
/// two-letter combos. Capacity 26 + 26·26 = 702 — far more than any
/// realistic column count.
fn generate_sort_labels(count: usize) -> Vec<String> {
    let alphabet: Vec<char> = ('a'..='z').collect();
    let n = alphabet.len();
    if count <= n {
        return alphabet.iter().take(count).map(|c| c.to_string()).collect();
    }
    let mut out = Vec::with_capacity(count);
    'outer: for first in &alphabet {
        for second in &alphabet {
            if out.len() >= count { break 'outer; }
            out.push(format!("{}{}", first, second));
        }
    }
    out
}

/// Parse the format produced by [`serialize_sort_state`]. Unknown
/// directions and malformed entries are dropped silently — a corrupt
/// settings row should degrade to "no sort," not crash.
fn parse_sort_state(s: &str) -> Vec<not_yet_done_content::SortKey> {
    use not_yet_done_content::{SortDirection, SortKey};
    s.split(',')
        .filter_map(|part| {
            let (col, dir) = part.trim().split_once(':')?;
            let col = col.trim();
            if col.is_empty() {
                return None;
            }
            let direction = match dir.trim() {
                "asc"  => SortDirection::Asc,
                "desc" => SortDirection::Desc,
                _      => return None,
            };
            Some(SortKey { column: col.to_string(), direction })
        })
        .collect()
}

/// A pending confirmation dialog: shows a message, executes on y/Enter, cancels on n/Esc.
pub enum PendingConfirmation {
    DeleteTask(Uuid),
    DeleteTaskRecursive(Uuid),
    DeleteTracking(Uuid),
    /// Drop a stale link row whose target ref can no longer be resolved
    /// (Stale / UnknownRoute / parse failure from [`crate::app::link`]).
    DeleteStaleLink(Uuid),
    /// Bulk-delete every link row whose source or target ref no longer
    /// resolves. Triggered by `:linkprune` after the user accepts the
    /// preview count. The Vec holds the link table IDs to remove.
    BulkDeleteStaleLinks(Vec<Uuid>),
    /// CP-9: delete a Postgres DB-level script after the user confirms.
    /// On accept, the App calls `delete_db_script` (idempotent unlink)
    /// and reloads the source pane so the row disappears.
    DeleteAdapterDbScript {
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        database: String,
        script: String,
    },
    /// DSF-4: delete an empty Postgres DB-script directory. The
    /// storage layer rejects non-empty dirs; we surface that error
    /// via Notify after the spawn fails.
    DeleteAdapterDbScriptDir {
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        database: String,
        rel_path: String,
    },
    /// CF-11: generic content-node delete. On accept the App spawns
    /// `Node::execute("delete", ActionInput::None)` via the adapter
    /// and reloads the pane on `ActionOutcome::Done`. No per-adapter
    /// coupling lives in the App for this path — every adapter that
    /// opts in by returning `ActionDispatch::DeleteSelf` from
    /// `invoke_action` gets confirmation + refresh for free.
    DeleteContentNode {
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        node_id: String,
    },
}

/// A saved query/filter with an optional keyboard shortcut.
/// Persisted in the `saved_query` DB table.
#[derive(Debug, Clone)]
pub struct SavedQuery {
    pub id: Option<Uuid>,
    pub name: String,
    pub query: String,
    pub shortcut: Option<String>,
}

impl SavedQuery {
    pub fn from_db(m: not_yet_done_core::entity::saved_query::Model) -> Self {
        Self {
            id: Some(m.id),
            name: m.name,
            query: m.query,
            shortcut: m.shortcut,
        }
    }
}

/// Legacy alias.
pub type Favorite = SavedQuery;

/// Handle to a script running in an external terminal.
/// The TUI polls for the output file to detect completion.
pub struct DetachedScript {
    pub output_path: std::path::PathBuf,
    pub capture: bool,
    /// True when the output file should be parsed as JSON
    /// `{"commands": [...]}` and dispatched through `execute_cmdline`,
    /// rather than displayed as text. Mutually exclusive with
    /// `capture` in practice (modes are disjoint), but kept as a
    /// separate flag so the two flows stay independent.
    pub emits_commands: bool,
}

impl DetachedScript {
    pub fn is_done(&self) -> bool {
        self.output_path.exists()
    }

    pub fn read_output(&self) -> Option<String> {
        std::fs::read_to_string(&self.output_path).ok()
    }

    pub fn cleanup(&self) {
        let _ = std::fs::remove_file(&self.output_path);
    }
}

// ---------------------------------------------------------------------------
// Content action popup state
// ---------------------------------------------------------------------------

/// State for the content action selection popup (e.g. Jira transitions).
pub struct ContentActionPopupState {
    pub popup: SearchablePopup,
    pub view_index: usize,
    pub pane_id: crate::views::content_view::PaneId,
    pub node_id: String,
    pub action_id: String,
}

/// State for the file-picker popup used by `InputSpec::FilePicker` actions
/// (e.g. Taiga attachment upload).
pub struct ContentFilePickerPopupState {
    pub picker: FilePicker,
    pub view_index: usize,
    pub pane_id: crate::views::content_view::PaneId,
    pub node_id: String,
    pub action_id: String,
}

/// State for the generic form popup used by `InputSpec::Form` actions (M6/E5).
pub struct ContentFormPopupState {
    pub popup: ContentFormPopup,
    pub view_index: usize,
    pub pane_id: crate::views::content_view::PaneId,
    pub node_id: String,
    pub action_id: String,
}

// ---------------------------------------------------------------------------
// Sort-hint mode
// ---------------------------------------------------------------------------

/// Where a sort change should land. `Tasks` updates `tasks_view` + persists
/// via the core `settings` table. `Content(idx)` updates the indexed
/// `ContentView` + persists in the adapter's own DB.
#[derive(Debug, Clone, Copy)]
pub enum SortTarget {
    Tasks,
    Content(usize),
}

/// Sort-hint mode is a two-phase modal: pick a column via letter label,
/// then pick a direction. Inactive when `Off`.
pub enum SortHintPhase {
    Off,
    /// Phase 1: action bar shows column → label mapping.
    WaitingForColumn {
        target: SortTarget,
        labels: Vec<(usize, String)>,
        columns: Vec<not_yet_done_content::SortableColumn>,
        input: String,
    },
    /// Phase 2: a column is picked, awaiting direction key.
    WaitingForDirection {
        target: SortTarget,
        column_id: String,
        column_name: String,
    },
}

impl SortHintPhase {
    pub fn is_active(&self) -> bool {
        !matches!(self, SortHintPhase::Off)
    }
}

/// Direction the user picked in the sort-hint direction phase. Translated
/// into an additive mutation on the view's current sort vector.
#[derive(Debug, Clone, Copy)]
enum SortAction {
    Asc,
    Desc,
    Clear,
}

// ---------------------------------------------------------------------------
// Content slots — Working ContentView vs. broken YAML
// ---------------------------------------------------------------------------

/// One entry per `Tab::Content(idx)` slot. A slot is `Working` when the
/// YAML loaded cleanly and an adapter (or fallback) is bound; it is
/// `Broken` when the YAML failed to parse or `validate()` reported one
/// or more errors. Broken slots still claim a tab so the user sees the
/// error in-app rather than the process exiting at startup.
/// Addressing tuple for a single Postgres per-table script. Carried in
/// the shortcut-capture state so the captured key chord can be written
/// to the right `<table_dir>/.shortcuts.yaml`.
#[derive(Debug, Clone)]
pub struct PostgresScriptCoords {
    pub view_index: usize,
    pub database: String,
    pub schema: String,
    pub table: String,
    pub script: String,
}

pub enum ContentSlot {
    Working(ContentView),
    Broken {
        name: String,
        path: std::path::PathBuf,
        errors: Vec<String>,
    },
}

impl ContentSlot {
    pub fn as_view(&self) -> Option<&ContentView> {
        match self {
            ContentSlot::Working(cv) => Some(cv),
            ContentSlot::Broken { .. } => None,
        }
    }
    pub fn as_view_mut(&mut self) -> Option<&mut ContentView> {
        match self {
            ContentSlot::Working(cv) => Some(cv),
            ContentSlot::Broken { .. } => None,
        }
    }
    pub fn tab_name(&self) -> &str {
        match self {
            ContentSlot::Working(cv) => cv.tab_name.as_str(),
            ContentSlot::Broken { name, .. } => name.as_str(),
        }
    }
    pub fn tab_icon(&self) -> Option<&str> {
        match self {
            ContentSlot::Working(cv) if !cv.tab_icon.is_empty() => Some(cv.tab_icon.as_str()),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

pub struct App {
    pub active_tab: Tab,
    /// Visible, ordered tabs + autonumber state (from the active
    /// `tabs:` constellation, or the legacy all-tabs order). Drives tab
    /// switching, `Tab`/`Shift+Tab` cycling, the digit keys and which
    /// tabs the bar renders. Rebuilt on config reload.
    pub tab_layout: TabLayout,
    // TasksState now lives in tasks_view.state, sub_view in tasks_view.sub_view()
    pub keybindings: KeyBindingConfig,
    pub theme:       Theme,
    pub shared_theme: Arc<Theme>,
    pub config:      TuiConfig,
    pub should_quit: bool,

    task_service: Arc<dyn TaskService>,
    pub tag_service: Arc<dyn not_yet_done_core::service::TagService>,
    pub saved_query_repo: Arc<dyn SavedQueryRepository>,
    pub query_shortcut_repo: Arc<dyn QueryShortcutRepository>,
    settings_repo: Arc<dyn SettingsRepository>,
    tracking_repo: Arc<dyn TrackingRepository>,
    pub link_repo: Arc<dyn LinkRepository>,

    pub load_rx: tokio::sync::mpsc::UnboundedReceiver<LoadMsg>,
    load_tx:     tokio::sync::mpsc::UnboundedSender<LoadMsg>,

    /// Per-view live-refresh timers (M9 — adapter-driven live rows). Key =
    /// `view_index`. Each handle drives a `tokio::time::interval` that pulls
    /// the view's adapter `live_rows()` and republishes each as a
    /// `LoadMsg::AdapterInvalidation { Invalidation::Row }` patch. (Re)paced
    /// by `Invalidation::RefreshInterval`; at most one timer per view (a
    /// respawn aborts the previous handle, `None` stops it).
    live_refresh_timers: std::collections::HashMap<usize, tokio::task::JoinHandle<()>>,

    /// Channel for results of background commit tasks (see `app::editor`).
    /// The receiver is selected on by the main loop and each message is
    /// applied via `handle_commit_msg`.
    pub commit_rx: tokio::sync::mpsc::UnboundedReceiver<crate::app::editor::CommitMsg>,
    pub commit_tx: tokio::sync::mpsc::UnboundedSender<crate::app::editor::CommitMsg>,

    /// `true` while a session commit is running on a background task. Keeps
    /// the editor "busy" so a second editor open is rejected with a clear
    /// "Saving previous edit, please wait…" message instead of opening on
    /// top of an in-flight commit.
    pub commit_in_flight: bool,

    /// Active detached editor process (non-inline mode).
    pub detached_editor: Option<DetachedEditor>,
    pub detached_script: Option<DetachedScript>,

    /// Active edit session — drives all `$EDITOR` round-trips.
    pub pending_session: Option<Box<dyn crate::edit_session::EditSession>>,

    /// Exact notification-bar text shown while a `NodeActionEditSession`
    /// is being prepared off-thread (the network-heavy fetch behind
    /// `OpenContentEditor`). `Some` ⇒ a load is in flight: it counts
    /// toward [`Self::editor_busy`] so a second open is rejected, and the
    /// stored string lets the completion handler remove *exactly* this
    /// notification without clearing unrelated ones.
    editor_loading_msg: Option<String>,
    /// Generation stamp bumped on every editor-open spawn (and on cancel).
    /// The off-thread result carries the stamp it was spawned with; a
    /// mismatch on arrival means a newer open superseded it, so the stale
    /// session is discarded instead of popping an unexpected editor.
    editor_load_token: u64,

    /// EditorRequest produced inside an async `LoadMsg` drain (e.g. a
    /// `NodeActionDispatched` carrying `ActionDispatch::OpenEditor`).
    /// `main.rs` drains this after every `poll_load` and runs it through
    /// the same dispatch as a keypress-time EditorRequest. Without this
    /// stash, `Inline`/`Launch` requests would silently drop on the
    /// async path because `poll_load` returns `()`.
    pub pending_editor_request: Option<EditorRequest>,

    /// Snapshot of the buffer most recently handed to `$EDITOR` (initial
    /// open or post-error reopen). When the editor closes and returns a
    /// buffer that's byte-identical to this snapshot, the user closed
    /// without saving (`:q` / `:q!`) and the App treats it as a cancel —
    /// crucial for breaking out of validation-error reopen loops.
    pub last_editor_buffer: Option<String>,

    /// Notification bar: (message, expiry time).
    pub notification: Option<(String, Instant)>,

    /// Query error shown below the sub-tab bar (persists until next :w).
    pub query_error: Option<String>,

    /// Most recent error message captured anywhere in the app
    /// (`set_query_error(Some(_))` or `notify_error`). Read on demand by
    /// `GlobalAction::ShowLastError` to open the message in $EDITOR so
    /// the user can scroll/copy long error text. `None` until the first
    /// error of the session.
    pub last_error: Option<String>,

    /// Last time active tracking durations were refreshed.
    last_tracking_tick: Instant,

    /// Last time a live `Busy` banner was nudged to repaint (~1 Hz).
    last_anim_tick: Instant,

    /// Column configuration popup.
    pub column_config_popup: Option<crate::components::column_config_popup::ColumnConfigPopup>,

    /// App-level tag-management menu (`:tag`). Stays alive across
    /// opens; the inner `popup` toggles per session.
    pub tag_menu: crate::components::tag_menu::TagMenuComponent,

    /// App-level script management menu (`:script`, also bound to `x`
    /// in the Trackings tab and to per-view `type: script` actions in
    /// content tabs). One menu, multiple contexts — the per-context
    /// JSON shape and scripts directory live on
    /// [`crate::app::script::ScriptContext`].
    pub script_menu: crate::components::script_menu::ScriptMenuComponent,
    /// Context for the currently open script menu. Drives the script
    /// dir and JSON construction when the user picks an entry. `None`
    /// whenever the menu is closed.
    pub script_menu_ctx: Option<crate::app::script::ScriptContext>,

    /// Tab-set switch popup (`ctrl+x`). Lists the configured
    /// constellations; switching rebuilds [`Self::tab_layout`].
    pub tab_set_popup: crate::components::tab_set_popup::TabSetPopup,

    /// Adapter credentials popup (login form for adapters that surface
    /// `AdapterStatus::NeedsCreds`).
    pub adapter_creds_popup:
        Option<crate::components::adapter_creds_popup::AdapterCredsPopup>,

    /// Query-variable input popup. Set when applying a saved query that
    /// the adapter reports as having `${var}` placeholders; cleared on
    /// submit (after the load) or cancel.
    pub query_var_popup:
        Option<crate::components::query_var_popup::QueryVarPopup>,

    /// `:config` picker popup — lists YAML files under the config dir.
    /// Activating a row opens it in a [`crate::edit_session::FileEditSession`].
    pub config_picker_popup: Option<SearchablePopup>,

    /// Cached set of actively tracked task IDs (refreshed on tracking changes).
    pub tracked_ids: HashSet<Uuid>,

    /// Cached set of every `source_ref` + `target_ref` string in the link
    /// table. Drives the "has-links" indicator column without hitting the
    /// DB per row. Refreshed on link create/delete and on startup.
    pub link_refs: HashSet<String>,

    /// Pending key for chord sequences (e.g. "g" waiting for "g" to form "gg").
    pub pending_key: Option<String>,


    /// When true, the next keypress is captured as a shortcut for a new favorite.
    pub awaiting_favorite_shortcut: Option<(String, String, String)>, // (scope, name, query)
    /// Saved-query shortcut conflicts already surfaced as notifications
    /// this session. Saved queries reload on every tab switch and
    /// q-menu mutation, so without this an unresolved conflict would
    /// re-notify dozens of times per session instead of once.
    warned_saved_query_conflicts: std::collections::HashSet<String>,
    /// Pending shortcut capture for a Postgres per-table script. Carries
    /// the addressing tuple so the captured key chord lands in the right
    /// `<table_dir>/.shortcuts.yaml`. Reset on capture or Esc.
    pub awaiting_postgres_script_shortcut: Option<PostgresScriptCoords>,
    /// Modal message popup — blocks input until dismissed.
    pub modal_message: Option<String>,

    /// Pending confirmation dialog — blocks input until y/n.
    pub pending_confirmation: Option<(String, PendingConfirmation)>,


    /// Trackings tab state.
    // TrackingsState now lives in trackings_view.state

    /// tuirealm components.
    pub tab_bar: TabBarComponent,
    pub status_bar: StatusBarComponent,
    pub notification_bar: NotificationBarComponent,
    pub query_error_bar: QueryErrorBarComponent,
    pub tasks_view: TasksView,
    pub trackings_view: TrackingsView,
    pub content_views: Vec<ContentSlot>,
    pub view_pane: ViewPaneComponent,

    /// Content action popup (e.g. Jira transitions).
    pub content_action_popup: Option<ContentActionPopupState>,
    pub content_file_picker_popup: Option<ContentFilePickerPopupState>,
    pub content_form_popup: Option<ContentFormPopupState>,
    pub form_pane: FormPaneComponent,

    /// App-wide link-mark slot. Set by `GlobalAction::LinkMark`, cleared
    /// by Esc (via [`dispatch_escape`]) or overwritten by another mark.
    /// Surfaced as a persistent indicator in the status bar so the user
    /// always knows whether a paste target is armed.
    pub marked_link: Option<not_yet_done_content::NodeRef>,

    /// DSF-4: node id of the DB-script entry (script or dir) currently
    /// marked for move. Set by `m` (`mark-move`), consumed by `p`
    /// (`paste-move`). Same-database only — the paste handler validates.
    /// Surfaced via status-bar indicator (mirrors [`Self::marked_link`]).
    pub marked_db_script_for_move: Option<String>,

    /// M7/E6: generic move clipboard for content nodes. Set when a
    /// `mark-move` action fires on any non-db-script content node, read
    /// back into [`not_yet_done_content::ActionContext::marked`] on the
    /// next `paste-move` invocation so the adapter performs the move.
    /// Cleared on successful paste, on Esc, or by another `mark-move`.
    /// DB-script keeps its own [`Self::marked_db_script_for_move`] slot
    /// until the consolidation follow-up (see plan A2). Surfaced via the
    /// status-bar indicator.
    pub content_marked_node: Option<not_yet_done_content::MarkedNode>,

    /// Task marked for moving via `:cut-node` (`mc`). The task is only
    /// actually reparented on `:paste-node` (`mp`); until then the tree
    /// is untouched. Cleared on successful paste, on Esc, or by another
    /// `:cut-node`.
    pub cut_node_id: Option<Uuid>,

    /// Open link popup (`gl` chord). When `Some`, it intercepts every
    /// key. Built lazily from the LinkRepository against the current
    /// row's [`NodeRef`].
    pub link_popup: Option<crate::app::link::LinkPopupState>,

    /// Vim-style cross-tab jump history driven by link-popup activation
    /// (Ctrl+O = back, Ctrl+I = forward). Only link jumps push entries —
    /// regular tab switches or selection changes do not.
    pub jump_history: crate::app::link::JumpHistory,

    /// Factory builder, stored as a boxed closure (not a bare `fn`
    /// pointer) so it can capture the in-process
    /// [`CoreHandle`](not_yet_done_local_adapter::CoreHandle) the local
    /// adapter needs. Called once in [`App::new`] and again on every
    /// [`App::reload_config`] — adapter factories are stateless to build,
    /// so re-running this is safe.
    pub adapter_factory_builder: Box<
        dyn Fn() -> std::collections::HashMap<String, Box<dyn not_yet_done_content::AdapterFactory>>
            + Send
            + Sync,
    >,

    /// Active sort-hint mode (column-pick → direction-pick). `Off` when idle.
    pub sort_hint_phase: SortHintPhase,
}

impl App {
    /// Borrow the working `ContentView` for slot `idx`, or `None` if the
    /// slot is broken or out of range. Most callers want this — only
    /// the render and key-dispatch paths inspect the broken variant.
    pub fn content_view(&self, idx: usize) -> Option<&ContentView> {
        self.content_views.get(idx).and_then(|s| s.as_view())
    }
    pub fn content_view_mut(&mut self, idx: usize) -> Option<&mut ContentView> {
        self.content_views.get_mut(idx).and_then(|s| s.as_view_mut())
    }
    /// Iterate working content views (skips broken slots).
    pub fn content_views_iter(&self) -> impl Iterator<Item = &ContentView> {
        self.content_views.iter().filter_map(|s| s.as_view())
    }
    pub fn content_views_iter_mut(&mut self) -> impl Iterator<Item = &mut ContentView> {
        self.content_views.iter_mut().filter_map(|s| s.as_view_mut())
    }
    /// Iterate working views with their slot index — skips broken slots
    /// while preserving the global slot numbering used by `Tab::Content`.
    pub fn content_views_indexed(&self) -> impl Iterator<Item = (usize, &ContentView)> {
        self.content_views
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_view().map(|v| (i, v)))
    }

    pub fn new(
        config: TuiConfig,
        theme: Theme,
        task_service: Arc<dyn TaskService>,
        tag_service: Arc<dyn not_yet_done_core::service::TagService>,
        saved_query_repo: Arc<dyn SavedQueryRepository>,
        query_shortcut_repo: Arc<dyn QueryShortcutRepository>,
        settings_repo: Arc<dyn SettingsRepository>,
        tracking_repo: Arc<dyn TrackingRepository>,
        link_repo: Arc<dyn LinkRepository>,
        adapter_factory_builder: Box<
            dyn Fn() -> std::collections::HashMap<String, Box<dyn not_yet_done_content::AdapterFactory>>
                + Send
                + Sync,
        >,
    ) -> Self {
        let keybindings = config.keybindings.clone();
        let shared_theme = Arc::new(Theme::new(config.theme.clone()));
        // Load content views from YAML config files (must happen before tab_bar).
        let content_views = load_content_views(&shared_theme, &config.keybindings, &config.editors, adapter_factory_builder());
        let content_tab_infos: Vec<crate::components::tab_bar::ContentTabInfo> = content_views.iter().map(|slot| {
            crate::components::tab_bar::ContentTabInfo {
                name: slot.tab_name().to_string(),
                icon: slot.tab_icon().unwrap_or_default().to_string(),
            }
        }).collect();
        let (tab_layout, tab_layout_error) = build_tab_layout(&config.tabs, &content_views);
        let initial_tab = tab_layout.first();
        let tab_bar = TabBarComponent::new(Arc::clone(&shared_theme), &config.keybindings, &content_tab_infos);
        let status_bar = StatusBarComponent::new(Arc::clone(&shared_theme), &config.keybindings);
        let tasks_view = TasksView::new(
            Arc::clone(&shared_theme),
            config.keybindings.clone(),
            Arc::clone(&task_service),
            config.tasks.tree.default_expand_depth,
        );
        let mut trackings_view = TrackingsView::new(Arc::clone(&shared_theme), config.keybindings.clone(), Arc::clone(&tracking_repo), Arc::clone(&saved_query_repo), Arc::clone(&settings_repo));
        trackings_view.set_taskpath_separator(config.tracking.taskpath_separator.clone());

        let view_pane = ViewPaneComponent::new(Arc::clone(&shared_theme));
        let form_pane = FormPaneComponent::new(Arc::clone(&shared_theme));
        let mut notification_bar = NotificationBarComponent::new(Arc::clone(&shared_theme));
        notification_bar.set_max_lines(config.notifications.max_lines);
        let query_error_bar = QueryErrorBarComponent::new(Arc::clone(&shared_theme));
        let (load_tx, load_rx) = tokio::sync::mpsc::unbounded_channel();
        let (commit_tx, commit_rx) = tokio::sync::mpsc::unbounded_channel();

        // Pre-clone the popup-intrinsic kb + icons so they can be passed
        // into TagMenu/ScriptMenu without colliding with the move of
        // `keybindings` into the struct literal below.
        let popup_kb = keybindings.popup.clone();
        let popup_icons = keybindings.key_icons.clone();

        let mut app = Self {
            active_tab:  initial_tab,
            tab_layout,
            keybindings,
            theme,
            shared_theme: Arc::clone(&shared_theme),
            config,
            should_quit: false,
            task_service,
            tag_service,
            saved_query_repo,
            query_shortcut_repo,
            settings_repo,
            tracking_repo,
            link_repo,
            load_rx,
            load_tx,
            live_refresh_timers: std::collections::HashMap::new(),
            commit_rx,
            commit_tx,
            commit_in_flight: false,
            detached_editor: None,
            detached_script: None,
            pending_session: None,
            editor_loading_msg: None,
            editor_load_token: 0,
            pending_editor_request: None,
            last_editor_buffer: None,
            notification: None,
            query_error: None,
            last_error: None,
            last_tracking_tick: Instant::now(),
            last_anim_tick: Instant::now(),
            column_config_popup: None,
            tag_menu: crate::components::tag_menu::TagMenuComponent::new(
                Arc::clone(&shared_theme),
                "Tags",
            )
            .with_popup_kb(popup_kb.clone(), popup_icons.clone()),
            script_menu: crate::components::script_menu::ScriptMenuComponent::new(
                Arc::clone(&shared_theme),
                "Scripts",
            )
            .with_popup_kb(popup_kb, popup_icons),
            script_menu_ctx: None,
            tab_set_popup: crate::components::tab_set_popup::TabSetPopup::new(Arc::clone(
                &shared_theme,
            )),
            adapter_creds_popup: None,
            query_var_popup: None,
            config_picker_popup: None,
            tracked_ids: HashSet::new(),
            link_refs: HashSet::new(),
            pending_key: None,
            awaiting_favorite_shortcut: None,
            warned_saved_query_conflicts: std::collections::HashSet::new(),
            awaiting_postgres_script_shortcut: None,
            modal_message: None,
            pending_confirmation: None,
            trackings_view,
            tasks_view,
            content_views,
            content_form_popup: None,
            content_action_popup: None,
            content_file_picker_popup: None,
            tab_bar,
            status_bar,
            notification_bar,
            query_error_bar,
            view_pane,
            form_pane,
            sort_hint_phase: SortHintPhase::Off,
            marked_link: None,
            marked_db_script_for_move: None,
            content_marked_node: None,
            cut_node_id: None,
            link_popup: None,
            jump_history: crate::app::link::JumpHistory::new(),
            adapter_factory_builder,
        };
        // A duplicate tab name is a hard config error — show it up front
        // (the layout already fell back to legacy so the app still runs).
        if let Some(err) = tab_layout_error {
            app.modal_message = Some(format!("Tab configuration error:\n\n{err}"));
        }

        // Configure nav chars on all tables.
        let nav_chars: Vec<char> = app.config.navigation.jump_chars.chars().collect();
        app.tasks_view.set_nav_chars(&nav_chars);
        app.trackings_view.table.set_nav_chars(&nav_chars);

        app.reload_link_refs();
        app.spawn_load();

        app
    }

    /// Auto-load content views that already have an adapter from YAML config.
    /// The watcher is always spawned (it only subscribes to a watch
    /// channel, no I/O); the load itself is skipped for tabs flagged
    /// `adapter.manual_connect: true` so they wait for an explicit
    /// user-triggered `reload` action.
    ///
    /// Lives outside [`App::new`] so main can apply DB-persisted state
    /// (default saved queries) before the first fetch — otherwise every
    /// tab with a default query would load twice on startup.
    pub fn start_content_loads(&mut self) {
        let to_watch: Vec<(usize, crate::views::content_view::PaneId, bool)> = self
            .content_views_indexed()
            .filter(|(_, cv)| cv.adapter.is_some())
            .map(|(i, cv)| (i, cv.active_pane_id(), cv.manual_connect))
            .collect();
        for (i, pane_id, manual) in to_watch {
            self.spawn_content_status_watcher(i);
            self.spawn_content_invalidation_watcher(i);
            if !manual {
                self.spawn_content_load(i, pane_id);
            }
        }
    }

    /// Stamp each content view's default saved query (if any) onto its
    /// active pane — plus every subtab pane opting in via
    /// `query.inherit_default` — so the initial loads already use it.
    /// Runs once at startup after [`Self::load_content_saved_queries`];
    /// a default whose name no longer exists in the store is skipped
    /// silently (the view falls back to its YAML `query.default`).
    pub fn apply_default_content_queries(&mut self) {
        for idx in 0..self.content_views.len() {
            let Some(cv) = self.content_view_mut(idx) else { continue };
            let Some(name) = cv.default_saved_query.clone() else { continue };
            let Some(body) = cv
                .db_saved_queries
                .iter()
                .find(|q| q.name == name)
                .map(|q| q.query.clone())
            else {
                continue;
            };
            cv.apply_default_query(body, Some(name));
        }
    }

    // -----------------------------------------------------------------------
    // Async task loading
    // -----------------------------------------------------------------------

    pub fn spawn_load(&mut self) {
        self.tasks_view.state.load_state = LoadState::Loading;

        // Use active_filter if set, otherwise fall back to form-based filter.
        let expr = if let Some(ref filter) = self.tasks_view.active_filter {
            filter.clone()
        } else {
            let build_result = filter_builder::build(&self.tasks_view.state.filter);

            self.tasks_view.state.filter.created_after_err  = None;
            self.tasks_view.state.filter.created_before_err = None;
            self.tasks_view.state.filter.priority_err       = None;
            for e in &build_result.errors {
                match e.field {
                    "Created after"  => self.tasks_view.state.filter.created_after_err  = Some(e.message.clone()),
                    "Created before" => self.tasks_view.state.filter.created_before_err = Some(e.message.clone()),
                    "Priority \u{2265}" => self.tasks_view.state.filter.priority_err    = Some(e.message.clone()),
                    _ => {}
                }
            }
            build_result.expr
        };

        let service = Arc::clone(&self.task_service);
        let tx      = self.load_tx.clone();
        let options = self.tasks_view.active_filter_options.clone();

        tokio::spawn(async move {
            let msg = match service.list_filtered_with_options(&expr, &options).await {
                Ok(tasks) => LoadMsg::Tasks(tasks),
                Err(e)    => LoadMsg::Error(e.to_string()),
            };
            let _ = tx.send(msg);
        });
    }

    /// Batch-fetch tags for every task id and feed the result back as
    /// [`LoadMsg::TaskTags`]. Errors are silently swallowed — the tag
    /// columns simply stay blank until the next reload tries again.
    fn spawn_load_task_tags(&self, ids: Vec<Uuid>) {
        let service = Arc::clone(&self.task_service);
        let tx      = self.load_tx.clone();
        tokio::spawn(async move {
            if let Ok(map) = service.load_tags_for_tasks(&ids).await {
                let _ = tx.send(LoadMsg::TaskTags(map));
            }
        });
    }

    pub fn spawn_load_trackings(&mut self) {
        self.trackings_view.state.load_state = crate::tabs::LoadState::Loading;
        let tracking_repo = Arc::clone(&self.tracking_repo);
        let task_service = Arc::clone(&self.task_service);
        let tx = self.load_tx.clone();
        let tracking_filter = self.trackings_view.active_filter.clone();

        tokio::spawn(async move {
            // Load all tasks for description + parent lookup.
            let tasks = task_service.list_tasks(None).await.unwrap_or_default();
            let task_map: std::collections::HashMap<Uuid, (String, Option<Uuid>)> = tasks.into_iter()
                .map(|t| (t.id, (t.description, t.parent_id)))
                .collect();

            // Build root → parent chain of task descriptions (excluding the
            // task itself — that is rendered separately in the `task` column).
            // The view prepends a leading separator and joins with a
            // configurable separator at render time. Cycles are guarded
            // against (defensive); a missing parent simply truncates the
            // chain at the last known ancestor.
            let path_for = |task_id: Uuid| -> Vec<String> {
                let mut chain: Vec<String> = Vec::new();
                let mut seen = std::collections::HashSet::new();
                seen.insert(task_id);
                let mut current = task_map.get(&task_id).and_then(|(_, p)| *p);
                while let Some(id) = current {
                    if !seen.insert(id) { break; }
                    match task_map.get(&id) {
                        Some((desc, parent)) => {
                            chain.push(desc.clone());
                            current = *parent;
                        }
                        None => break,
                    }
                }
                chain.reverse();
                chain
            };

            // Load trackings — filtered if a tracking query filter is active.
            let trackings = if let Some(ref expr) = tracking_filter {
                match tracking_repo.find_filtered(expr).await {
                    Ok(t) => t,
                    Err(e) => {
                        let _ = tx.send(LoadMsg::TrackingError(e.to_string()));
                        return;
                    }
                }
            } else {
                match tracking_repo.find_all().await {
                    Ok(t) => t,
                    Err(e) => {
                        let _ = tx.send(LoadMsg::TrackingError(e.to_string()));
                        return;
                    }
                }
            };

            let now = chrono::Utc::now();
            let rows: Vec<crate::tabs::TrackingRow> = trackings.into_iter()
                .map(|t| {
                    let desc = task_map.get(&t.task_id)
                        .map(|(d, _)| d.clone())
                        .unwrap_or_else(|| format!("(deleted {})", &t.task_id.to_string()[..8]));
                    let path = path_for(t.task_id);
                    let ended = t.ended_at;
                    let duration = ended.unwrap_or(now) - t.started_at;
                    crate::tabs::TrackingRow {
                        id: t.id,
                        task_id: t.task_id,
                        task_description: desc,
                        task_path: path,
                        started_at: t.started_at,
                        ended_at: ended,
                        duration,
                        active: ended.is_none(),
                    }
                })
                .collect();

            let _ = tx.send(LoadMsg::Trackings(rows));
        });
    }

    /// Spawn async item load for a content view (root level).
    /// Subscribe to the adapter's auth/connection status and forward each
    /// transition to `poll_load` as a [`LoadMsg::ContentAdapterStatus`].
    /// The first push happens immediately so the view sees the current
    /// state without waiting for a transition.
    /// Spawn `submit_credentials` on the adapter behind `view_index` and
    /// route the result back to the popup via `LoadMsg::CredentialSubmitResult`.
    pub fn spawn_submit_credentials(
        &self,
        view_index: usize,
        values: std::collections::HashMap<String, String>,
    ) {
        let Some(cv) = self.content_view(view_index) else { return; };
        let Some(adapter) = cv.adapter.as_ref() else { return; };
        let adapter = Arc::clone(adapter);
        let tx = self.load_tx.clone();
        tokio::spawn(async move {
            let result = adapter.submit_credentials(values).await;
            let error = result.err().map(|e| e.to_string());
            let _ = tx.send(LoadMsg::CredentialSubmitResult { view_index, error });
        });
    }

    pub fn spawn_content_status_watcher(&self, view_index: usize) {
        let Some(cv) = self.content_view(view_index) else { return; };
        let Some(adapter) = cv.adapter.as_ref() else { return; };
        let mut rx = adapter.subscribe_status();
        let tx = self.load_tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(LoadMsg::ContentAdapterStatus {
                view_index,
                status: rx.borrow().clone(),
            });
            while rx.changed().await.is_ok() {
                let status = rx.borrow().clone();
                let _ = tx.send(LoadMsg::ContentAdapterStatus { view_index, status });
            }
        });
    }

    /// Forward a streaming adapter's out-of-band [`Invalidation`] events
    /// into `poll_load` as [`LoadMsg::AdapterInvalidation`]. Mirrors
    /// `spawn_content_status_watcher`; harmless for pull-only adapters
    /// (their default subscription never sends, so the task just parks).
    /// On `Lagged` we resync the conservative way — a full reload — so a
    /// momentarily-slow frontend never silently drops a change.
    pub fn spawn_content_invalidation_watcher(&self, view_index: usize) {
        let Some(cv) = self.content_view(view_index) else { return; };
        let Some(adapter) = cv.adapter.as_ref() else { return; };
        let mut rx = adapter.subscribe_invalidations();
        let tx = self.load_tx.clone();
        tokio::spawn(async move {
            use tokio::sync::broadcast::error::RecvError;
            loop {
                let msg = match rx.recv().await {
                    Ok(inv) => LoadMsg::AdapterInvalidation { view_index, inv },
                    Err(RecvError::Lagged(_)) => LoadMsg::AdapterInvalidation {
                        view_index,
                        inv: not_yet_done_content::Invalidation::All,
                    },
                    Err(RecvError::Closed) => break,
                };
                if tx.send(msg).is_err() {
                    break;
                }
            }
        });
    }

    /// Start, re-pace, or stop the live-refresh timer for `view_index`
    /// (M9 — adapter-driven live rows). `Some(interval)` (re)spawns a
    /// `tokio::time::interval` that, on each tick, pulls the view adapter's
    /// [`live_rows`](not_yet_done_content::ContentAdapter::live_rows) and
    /// forwards each refreshed row as an
    /// [`Invalidation::Row`](not_yet_done_content::Invalidation::Row) patch
    /// through the load channel; `None` stops it. A respawn aborts the
    /// existing handle first, so the cadence the adapter last declared
    /// always wins and timers never accumulate across re-pacings.
    fn set_live_refresh_timer(
        &mut self,
        view_index: usize,
        interval: Option<std::time::Duration>,
    ) {
        // Re-pacing replaces the running timer; `None` leaves it stopped.
        if let Some(handle) = self.live_refresh_timers.remove(&view_index) {
            handle.abort();
        }
        let Some(interval) = interval else { return };
        if interval.is_zero() {
            return; // a zero interval would busy-loop
        }
        let Some(cv) = self.content_view(view_index) else { return };
        let Some(adapter) = cv.adapter.as_ref() else { return };
        let adapter = Arc::clone(adapter);
        let tx = self.load_tx.clone();
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // `interval()` fires immediately at t=0; skip that tick so the
            // first refresh lands one interval out, not on the same frame
            // as the load that declared the cadence.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                for summary in adapter.live_rows().await {
                    let msg = LoadMsg::AdapterInvalidation {
                        view_index,
                        inv: not_yet_done_content::Invalidation::Row(summary),
                    };
                    if tx.send(msg).is_err() {
                        return; // app gone
                    }
                }
            }
        });
        self.live_refresh_timers.insert(view_index, handle);
    }

    /// React to a streaming adapter's [`Invalidation`]. Reloads the
    /// current level of each pane in the view that the invalidation
    /// affects:
    /// - [`Invalidation::All`] → every pane (reconnect / first bootstrap).
    /// - [`Invalidation::Node`] → only panes whose current level is that
    ///   node's children (a message in the open channel reloads; one in
    ///   any other channel costs nothing).
    /// - [`Invalidation::Repaint`] → no pane reloads (no refetch), but the
    ///   live (`kind: elapsed`) panes are rebuilt in place against a fresh
    ///   `now` so a time-derived cell (e.g. a running "elapsed" duration)
    ///   advances; the rebuild marks the frame dirty.
    fn handle_adapter_invalidation(
        &mut self,
        view_index: usize,
        inv: not_yet_done_content::Invalidation,
    ) {
        use not_yet_done_content::Invalidation;
        // Repaint is redraw-only: no refetch. But the table rows are
        // pre-built and cached, so a dirty frame alone would redraw a stale
        // string for a time-derived cell. Recompute the live (`kind:
        // elapsed`) panes in place against a fresh `now`, then fall through
        // — the rebuild marks the frame dirty so the new value is drawn.
        if matches!(inv, Invalidation::Repaint) {
            if let Some(cv) = self.content_view_mut(view_index) {
                cv.repaint_live_columns();
            }
            return;
        }
        // M9 — a single row's refreshed state: patch it in place (no
        // refetch). The adapter already computed the new cell values.
        if let Invalidation::Row(summary) = &inv {
            if let Some(cv) = self.content_view_mut(view_index) {
                cv.patch_row(summary);
            }
            return;
        }
        // M9 — the adapter (re)paces this view's live-refresh timer: start
        // it at the given interval, or stop it on `None`.
        if let Invalidation::RefreshInterval(interval) = inv {
            self.set_live_refresh_timer(view_index, interval);
            return;
        }
        // Collect the affected pane ids first so the immutable borrow of
        // the view ends before `reload_content_pane_current_level`
        // re-borrows `self`.
        let targets: Vec<crate::views::content_view::PaneId> = {
            let Some(cv) = self.content_view(view_index) else {
                return;
            };
            cv.all_pane_ids()
                .into_iter()
                .filter(|&pid| match &inv {
                    Invalidation::All => true,
                    Invalidation::Node { id } => cv
                        .find_pane(pid)
                        .and_then(|p| p.parent_node_id())
                        .is_some_and(|parent| parent == id),
                    // Redraw-only: select no panes (no refetch). The
                    // repaint itself happens because `handle_load_msg`
                    // always returns dirty=true for any message it drains.
                    // Row / RefreshInterval are handled by the early returns
                    // above and never reach this filter.
                    Invalidation::Repaint
                    | Invalidation::Row(_)
                    | Invalidation::RefreshInterval(_) => false,
                })
                .collect()
        };
        for pid in targets {
            self.reload_content_pane_current_level(view_index, pid);
        }
    }

    /// Reload the content pane at its **current** drill level. At root,
    /// re-runs the ViewDef query. Inside a drill-down, re-fetches the
    /// active child level under the current parent — without this the
    /// pane would silently jump back to root after an action completes.
    pub fn reload_content_pane_current_level(
        &self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
    ) {
        let drill = self
            .content_view(view_index)
            .and_then(|cv| cv.find_pane(pane_id))
            .and_then(|pane| {
                let parent = pane.parent_node_id()?.to_string();
                let child = pane.current_child_node_type()?.to_string();
                Some((parent, child))
            });
        match drill {
            Some((parent, child)) => self.spawn_content_drill_down(view_index, pane_id, parent, child),
            None => self.spawn_content_load(view_index, pane_id),
        }
    }

    pub fn spawn_content_load(&self, view_index: usize, pane_id: crate::views::content_view::PaneId) {
        let cv = match self.content_view(view_index) {
            Some(cv) => cv,
            None => return,
        };
        let adapter = match cv.adapter.as_ref() {
            Some(a) => Arc::clone(a),
            None => return,
        };
        let pane = match cv.find_pane(pane_id) {
            Some(p) => p,
            None => return,
        };
        let Some(req) = pane.root_load_request(&cv.view_defs) else {
            return;
        };
        let crate::views::content_view::LoadRequest {
            node_type_id,
            query,
            sort,
            page,
            vars,
        } = req;
        let query = query.map(|raw| adapter.render_query(&raw, &vars));
        // Adapter-grouped tree (capability `group_by_via_adapter`): the
        // pane's effective grouping rides along so the adapter buckets the
        // root level itself. `None` everywhere else.
        let group_by = pane.adapter_group_spec(&cv.view_defs);
        let retries = cv
            .view_defs
            .get(pane.view_def_index())
            .map(|v| v.retries)
            .unwrap_or(0);
        let tx = self.load_tx.clone();
        tokio::spawn(async move {
            let result = run_with_retries(retries, &tx, view_index, pane_id, || {
                let adapter = Arc::clone(&adapter);
                let node_type_id = node_type_id.clone();
                let query = query.clone();
                let sort = sort.clone();
                let group_by = group_by.clone();
                async move {
                    let root = adapter.root().await.map_err(|e| e.to_string())?;
                    let node_type = root.children_types()
                        .into_iter()
                        .find(|t| t.type_id == node_type_id)
                        .ok_or_else(|| format!("Node type '{node_type_id}' not found"))?;
                    let sortable_columns = root.sortable_columns(&node_type);
                    let params = not_yet_done_content::ListParams {
                        node_type,
                        query,
                        sort,
                        page,
                        download: false,
                        group_by,
                    };
                    let list = root.list(params).await.map_err(|e| e.to_string())?;
                    Ok((list, sortable_columns))
                }
            })
            .await;
            match result {
                Ok((list, sortable_columns)) => {
                    let _ = tx.send(LoadMsg::ContentItems {
                        view_index,
                        pane_id,
                        items: list.items,
                        applied_sort: list.applied_sort,
                        page: list.page,
                        sortable_columns,
                        error: None,
                    });
                }
                Err(e) => {
                    let _ = tx.send(LoadMsg::ContentItems {
                        view_index,
                        pane_id,
                        items: vec![],
                        applied_sort: Vec::new(),
                        page: None,
                        sortable_columns: Vec::new(),
                        error: Some(e),
                    });
                }
            }
        });
    }

    /// Eager tree load: ask the adapter (capability `supports_eager_subtree`)
    /// for the whole expanded subtree under the root in ONE `list_subtree`
    /// call, landing it via [`LoadMsg::Subtree`] → [`ContentView::apply_subtree`].
    /// The root level itself is still configured by the ordinary
    /// [`Self::spawn_content_load`] (`ContentItems` sets columns / sort /
    /// selection); this fires alongside it to expand the descendants in place
    /// of the per-node cascade. `depth` is the view's `expand_depth` mapped to
    /// a level count (`all` → `u32::MAX`).
    pub fn spawn_subtree_load(
        &self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        depth: u32,
    ) {
        let cv = match self.content_view(view_index) {
            Some(cv) => cv,
            None => return,
        };
        let adapter = match cv.adapter.as_ref() {
            Some(a) => Arc::clone(a),
            None => return,
        };
        let pane = match cv.find_pane(pane_id) {
            Some(p) => p,
            None => return,
        };
        let Some(req) = pane.root_load_request(&cv.view_defs) else {
            return;
        };
        let crate::views::content_view::LoadRequest {
            node_type_id,
            query,
            sort,
            page,
            vars,
        } = req;
        let query = query.map(|raw| adapter.render_query(&raw, &vars));
        let group_by = pane.adapter_group_spec(&cv.view_defs);
        let retries = cv
            .view_defs
            .get(pane.view_def_index())
            .map(|v| v.retries)
            .unwrap_or(0);
        let tx = self.load_tx.clone();
        tokio::spawn(async move {
            let result = run_with_retries(retries, &tx, view_index, pane_id, || {
                let adapter = Arc::clone(&adapter);
                let node_type_id = node_type_id.clone();
                let query = query.clone();
                let sort = sort.clone();
                let group_by = group_by.clone();
                async move {
                    let root = adapter.root().await.map_err(|e| e.to_string())?;
                    let node_type = root
                        .children_types()
                        .into_iter()
                        .find(|t| t.type_id == node_type_id)
                        .ok_or_else(|| format!("Node type '{node_type_id}' not found"))?;
                    let params = not_yet_done_content::ListParams {
                        node_type,
                        query,
                        sort,
                        page,
                        download: false,
                        group_by,
                    };
                    root.list_subtree(params, depth).await.map_err(|e| e.to_string())
                }
            })
            .await;
            let _ = tx.send(LoadMsg::Subtree {
                view_index,
                pane_id,
                parent_path: Vec::new(),
                result,
            });
        });
    }

    /// Spawn an async re-execution of a Postgres custom query. Used by
    /// the editor session for the initial run and by the pane's
    /// next/prev-page keys for subsequent pages. Result lands back via
    /// [`LoadMsg::CustomQueryItems`] so the main loop applies it the
    /// same way for both entry points.
    ///
    /// `cursor` opts into cursor pagination (CP-5). When `Some` the
    /// adapter takes the cursor lifecycle path and ignores `page`; the
    /// returned [`CustomQueryItemsPayload`] carries the adapter's
    /// opaque `cursor_id` so the pane can chain a `Continue` on the
    /// next `>` press. `None` keeps the legacy LIMIT/OFFSET path.
    pub fn spawn_postgres_query(
        &self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        database: String,
        query: String,
        page: Option<not_yet_done_content::PageRequest>,
        cursor: Option<not_yet_done_content::CursorIntent>,
    ) {
        let cv = match self.content_view(view_index) {
            Some(cv) => cv,
            None => return,
        };
        let adapter = match cv.adapter.as_ref() {
            Some(a) => Arc::clone(a),
            None => return,
        };
        let tx = self.load_tx.clone();
        tokio::spawn(async move {
            let mut ctx = not_yet_done_content::CustomQueryContext::new()
                .with("database", database.clone());
            if let Some(p) = page {
                ctx = ctx.with_page(p);
            }
            if let Some(c) = cursor {
                ctx = ctx.with_cursor(c);
            }
            let outcome = adapter
                .execute_custom_query(&query, &ctx)
                .await
                .map_err(|e| e.to_string());
            let result = outcome.map(|res| {
                let status = if res.items.is_empty() && res.status.is_some() {
                    res.status.clone()
                } else if res.page.is_none() {
                    // Non-paginated SELECT (multi-statement, etc.): mention
                    // the row count so the user knows the result size.
                    Some(format!("{} row(s)", res.items.len()))
                } else {
                    None
                };
                crate::app::CustomQueryItemsPayload {
                    items: res.items,
                    page: res.page,
                    custom_query: crate::views::content_view::CustomQueryRunState {
                        query: query.clone(),
                        database: database.clone(),
                        // Placeholder — the pane overrides this with its
                        // own view-config-derived mode in
                        // `apply_custom_query_result`.
                        mode: crate::config::view_config::PaginationMode::Server,
                        cursor_id: res.cursor_id.clone(),
                    },
                    status,
                }
            });
            let _ = tx.send(LoadMsg::CustomQueryItems { view_index, pane_id, result });
        });
    }

    /// Drain the focused content view's pending cursor-close queue
    /// (CP-6) and spawn one fire-and-forget close per id. Called after
    /// every interaction with a content view so panes destroyed by
    /// `wq` / cascade / hot-replace have their server-side cursors
    /// torn down promptly.
    fn drain_content_cursor_closes(&mut self, view_index: usize) {
        let ids = self
            .content_view_mut(view_index)
            .map(|cv| cv.take_pending_cursor_closes())
            .unwrap_or_default();
        for id in ids {
            self.spawn_close_adapter_cursor(view_index, id);
        }
    }

    /// Fire-and-forget cursor close for CP-6 pane-close cleanup. The
    /// adapter's `execute_custom_query` "Close" branch ignores the
    /// query string and the database; we send empty placeholders. Any
    /// error (already-closed cursor, connection gone, etc.) is dropped
    /// — the worst case is one idle TX leaked until the connection is
    /// recycled at process exit.
    pub fn spawn_close_adapter_cursor(&self, view_index: usize, cursor_id: String) {
        let cv = match self.content_view(view_index) {
            Some(cv) => cv,
            None => return,
        };
        let adapter = match cv.adapter.as_ref() {
            Some(a) => Arc::clone(a),
            None => return,
        };
        tokio::spawn(async move {
            let ctx = not_yet_done_content::CustomQueryContext::new()
                .with_cursor(not_yet_done_content::CursorIntent::Close { cursor_id });
            let _ = adapter.execute_custom_query("", &ctx).await;
        });
    }

    /// CP-8 entry point: a `postgres:db_script` row's `x` shortcut
    /// dispatched `ActionDispatch::ExecuteQuery { paged: true }`. Allocates
    /// (or reuses) the result pane child via the active level's first
    /// `ChildDef` (typically `postgres:db_script_result` with
    /// `split: right` + `pagination: { mode: cursor }`), then spawns a
    /// cursor-paginated custom query against it.
    ///
    /// `sql` is the script body already stripped of scratch/marker by the
    /// adapter side — see `DbScriptNode::invoke_action("execute")`.
    fn run_adapter_db_script(
        &mut self,
        view_index: usize,
        _pane_id: crate::views::content_view::PaneId,
        source_node_id: String,
        source_label: String,
        database: String,
        sql: String,
    ) {
        let Some(cv) = self.content_view_mut(view_index) else {
            self.notify("No content view available".to_string());
            return;
        };
        let target_pane_id = cv.open_db_script_result_pane(&source_node_id, &source_label);
        self.spawn_postgres_query(
            view_index,
            target_pane_id,
            database,
            sql,
            Some(not_yet_done_content::PageRequest {
                offset: 0,
                limit: crate::edit_session::POSTGRES_QUERY_DEFAULT_PAGE_SIZE,
            }),
            Some(not_yet_done_content::CursorIntent::Open),
        );
    }

    /// CP-8 entry point: a `postgres:db_script` row's `e` shortcut
    /// dispatched `ActionDispatch::OpenEditor { session_kind: "postgres_db_script" }`.
    /// Opens [`PostgresDbScriptSession`] which writes the buffer back to
    /// `<instance_data_dir>/db_scripts/<database>/<script>.sql` on `:w`
    /// and does NOT re-execute — the user re-runs explicitly with `x`.
    fn open_adapter_db_script_editor(
        &mut self,
        view_index: usize,
        _pane_id: crate::views::content_view::PaneId,
        database: String,
        script: String,
        in_place: bool,
    ) -> EditorRequest {
        let Some(adapter) = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone)
        else {
            self.notify("No adapter for this view".to_string());
            return EditorRequest::None;
        };
        let session = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                crate::edit_session::PostgresDbScriptSession::open(
                    adapter, database, script, in_place,
                )
                .await
            })
        });
        self.open_session(Box::new(session))
    }

    /// CP-9 / DSF-4 / DSF-5: open the active content tab's cmdline
    /// pre-typed so the user only enters the new script name. Uses
    /// the DSF-5 namespace `:db-script new <name>` — the selected row
    /// already pins the current dir, so we don't need to thread
    /// `parent_rel` through the cmdline.
    fn open_db_script_new_prompt(
        &mut self,
        view_index: usize,
        _pane_id: crate::views::content_view::PaneId,
        _database: String,
        _parent_rel: String,
    ) {
        use crate::views::HasCmdline;
        if !matches!(self.active_tab, Tab::Content(idx) if idx == view_index) {
            self.notify("Switch back to the Postgres tab to add a script".to_string());
            return;
        }
        let Some(cv) = self.content_view_mut(view_index) else {
            return;
        };
        cv.cmdline_open_with("db-script new ");
    }

    /// DSF-4 / DSF-5: pre-fill `:db-script new-dir <name>`.
    fn open_db_script_dir_new_prompt(
        &mut self,
        view_index: usize,
        _pane_id: crate::views::content_view::PaneId,
        _database: String,
        _parent_rel: String,
    ) {
        use crate::views::HasCmdline;
        if !matches!(self.active_tab, Tab::Content(idx) if idx == view_index) {
            self.notify("Switch back to the Postgres tab to add a folder".to_string());
            return;
        }
        let Some(cv) = self.content_view_mut(view_index) else {
            return;
        };
        cv.cmdline_open_with("db-script new-dir ");
    }

    /// DSF-4 / DSF-5: pre-fill `:db-script rename <name>` (the user
    /// just types the new name and presses Enter).
    fn open_db_script_rename_prompt(
        &mut self,
        view_index: usize,
        _pane_id: crate::views::content_view::PaneId,
        _database: String,
        _rel_path: String,
        _is_dir: bool,
    ) {
        use crate::views::HasCmdline;
        if !matches!(self.active_tab, Tab::Content(idx) if idx == view_index) {
            self.notify("Switch back to the Postgres tab to rename".to_string());
            return;
        }
        let Some(cv) = self.content_view_mut(view_index) else {
            return;
        };
        cv.cmdline_open_with("db-script rename ");
    }

    /// DSF-4: stash the marked source for a subsequent move. Mirrors
    /// [`Self::marked_link`] UX — the status bar shows the indicator
    /// until paste or Esc clears it.
    fn mark_db_script_for_move(&mut self, node_id: String) {
        self.notify(format!("Marked '{node_id}' for move — paste with `p` on the target dir"));
        self.marked_db_script_for_move = Some(node_id);
    }

    /// DSF-4: paste the marked source into the target dir (or root
    /// group). Validates same-database, calls `move_db_script_entry`,
    /// reloads, and clears the mark.
    fn paste_db_script_move(
        &mut self,
        target_node_id: String,
    ) {
        use crate::app::node_actions::{db_script_rel_path_str, parse_db_script_node_id};
        let Some(source_node_id) = self.marked_db_script_for_move.clone() else {
            self.notify("No DB-script marked for move (use `m` first)".to_string());
            return;
        };
        let Some((src_db, src_segs)) = parse_db_script_node_id(&source_node_id) else {
            self.notify_error(format!("Marked source '{source_node_id}' is not a DB-script id"));
            self.marked_db_script_for_move = None;
            return;
        };
        let Some((dst_db, dst_segs)) = parse_db_script_node_id(&target_node_id) else {
            self.notify_error(format!("Target '{target_node_id}' is not a DB-script id"));
            return;
        };
        if src_db != dst_db {
            self.notify_error(format!(
                "Cross-database move not supported ({src_db} → {dst_db})"
            ));
            return;
        }
        let src_rel = db_script_rel_path_str(&src_segs);
        // Target rel_path: drop the dir's own name from src and prepend
        // dst's rel_path. Source name is the last segment; the file
        // keeps its name in the destination.
        let Some(src_name) = src_segs.last().cloned() else {
            self.notify_error(format!("Marked source '{source_node_id}' has no name segment"));
            return;
        };
        let dst_rel = if dst_segs.is_empty() {
            src_name
        } else {
            format!("{}/{}", db_script_rel_path_str(&dst_segs), src_name)
        };
        // Find the source pane's view + adapter. Use the active content
        // tab — paste-move was triggered by `p` on the focused row.
        let view_index = match self.current_content_view_index_or_modal("paste-move") {
            Some(idx) => idx,
            None => return,
        };
        let Some(adapter) = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone)
        else {
            self.notify("No adapter for this view".to_string());
            return;
        };
        let pane_id = self
            .content_view(view_index)
            .map(|cv| cv.active_pane_id())
            .unwrap_or(0);
        let instance_dir = adapter.instance_data_dir();
        let result: std::io::Result<()> = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                not_yet_done_postgres_adapter::query::move_db_script_entry(
                    &instance_dir,
                    &src_db,
                    std::path::Path::new(&src_rel),
                    std::path::Path::new(&dst_rel),
                )
                .await
            })
        });
        match result {
            Ok(()) => {
                self.notify(format!("Moved '{src_rel}' → '{dst_rel}' in {src_db}"));
                self.marked_db_script_for_move = None;
                self.spawn_content_load(view_index, pane_id);
                self.refresh_db_scripts_tree_children(view_index, pane_id, &src_db);
            }
            Err(e) => self.notify_error(format!("Move failed: {e}")),
        }
    }

    /// DSF-4: confirm + delete an empty DB-script directory. The
    /// storage layer rejects non-empty dirs ("not empty (N entries)");
    /// we surface that via Notify after the spawn so the user sees the
    /// actual count.
    fn confirm_delete_adapter_db_script_dir(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        database: String,
        rel_path: String,
    ) {
        let msg = format!(
            "Delete empty DB-script folder '{rel_path}' in '{database}'? (y/n)"
        );
        self.modal_message = Some(msg.clone());
        self.pending_confirmation = Some((
            msg,
            PendingConfirmation::DeleteAdapterDbScriptDir {
                view_index,
                pane_id,
                database,
                rel_path,
            },
        ));
    }

    /// DSF-4: do-the-actual-delete handler invoked from the confirm
    /// accept path. Mirrors [`Self::delete_adapter_db_script_now`].
    fn delete_adapter_db_script_dir_now(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        database: String,
        rel_path: String,
    ) {
        let Some(adapter) = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone)
        else {
            self.notify("No adapter for this view".to_string());
            return;
        };
        let instance_dir = adapter.instance_data_dir();
        let rel_path_pb = std::path::PathBuf::from(&rel_path);
        let result: std::io::Result<()> = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                not_yet_done_postgres_adapter::query::delete_db_script_dir(
                    &instance_dir,
                    &database,
                    &rel_path_pb,
                )
                .await
            })
        });
        match result {
            Ok(()) => {
                self.notify(format!("Deleted DB-script folder '{rel_path}'"));
                self.spawn_content_load(view_index, pane_id);
                self.refresh_db_scripts_tree_children(view_index, pane_id, &database);
            }
            Err(e) => self.notify_error(format!("Delete folder failed: {e}")),
        }
    }

    /// DSF-5: top-level dispatch for the `:db-script <sub>` namespace.
    /// All subcommands operate on the focused content pane's selected
    /// row; if no row is selected (e.g. user is on the Tasks tab) we
    /// surface a modal so the user sees the path requirement.
    ///
    /// Subcommands:
    /// - `new <name>` — create script in current dir (file ext `.sql`)
    /// - `new-dir <name>` — create empty dir in current dir
    /// - `rename <name>` — rename selected entry
    /// - `move <dest>` — move selected (or marked) into `<dest>`
    /// - `delete` — delete selected entry (script or empty dir)
    fn db_script_command(&mut self, sub: &str, rest: &str) {
        match sub {
            "new" => {
                if rest.is_empty() {
                    self.modal_message =
                        Some(":db-script new expects <name>".to_string());
                    return;
                }
                self.db_script_new_in_current_dir(rest);
            }
            "new-dir" => {
                if rest.is_empty() {
                    self.modal_message =
                        Some(":db-script new-dir expects <name>".to_string());
                    return;
                }
                self.db_script_new_dir_in_current_dir(rest);
            }
            "rename" => {
                if rest.is_empty() {
                    self.modal_message =
                        Some(":db-script rename expects <new-name>".to_string());
                    return;
                }
                self.db_script_rename_selected(rest);
            }
            "move" => {
                if rest.is_empty() {
                    self.modal_message = Some(
                        ":db-script move expects <dest-dir> (use '/' for root)"
                            .to_string(),
                    );
                    return;
                }
                self.db_script_move_selected_or_marked(rest);
            }
            "delete" => {
                if !rest.is_empty() {
                    self.modal_message =
                        Some(":db-script delete takes no arguments".to_string());
                    return;
                }
                self.db_script_delete_selected();
            }
            "" => {
                self.modal_message = Some(
                    ":db-script expects a subcommand (new | new-dir | rename | move | delete)"
                        .to_string(),
                );
            }
            other => {
                self.modal_message = Some(format!(
                    ":db-script — unknown subcommand '{other}' (expected new | new-dir | rename | move | delete)"
                ));
            }
        }
    }

    /// DSF-5: resolve the focused row to a DB-script context. Returns
    /// `(view_index, pane_id, adapter, database, current_dir_rel, selected)`
    /// where `selected` is `Some((rel_path, is_dir))` if the row is a
    /// dir or script, or `None` if it's the db_scripts group node
    /// itself. `current_dir_rel` is the rel-path of the dir the user
    /// is *inside* — empty for root.
    #[allow(clippy::type_complexity)]
    fn resolve_db_script_context(
        &mut self,
        sub: &str,
    ) -> Option<(
        usize,
        crate::views::content_view::PaneId,
        Arc<dyn not_yet_done_content::ContentAdapter>,
        String,
        String,
        Option<(String, bool)>,
    )> {
        let view_index = self.current_content_view_index_or_modal(&format!("db-script {sub}"))?;
        let (selected_id, pane_id) = {
            let cv = self.content_view(view_index)?;
            let id = cv.selected_item_id().map(str::to_string);
            (id, cv.active_pane_id())
        };
        let Some(selected_id) = selected_id else {
            self.modal_message =
                Some(format!(":db-script {sub} — no row selected"));
            return None;
        };
        // Parse the id. If it doesn't look like a db-script id, bail.
        let Some((database, segments)) = crate::app::node_actions::parse_db_script_node_id(&selected_id) else {
            // Maybe the user is on the db_scripts group itself
            // (`<db>/db_scripts` with no trailing segment). Detect:
            let mut parts = selected_id.split('/');
            let db = parts.next().unwrap_or("").to_string();
            let group = parts.next();
            if group == Some("db_scripts") && parts.next().is_none() && !db.is_empty() {
                let adapter = self
                    .content_view(view_index)
                    .and_then(|cv| cv.adapter.as_ref())
                    .map(Arc::clone)?;
                return Some((view_index, pane_id, adapter, db, String::new(), None));
            }
            self.modal_message = Some(format!(
                ":db-script {sub} — selected row '{selected_id}' is not a DB-script entry"
            ));
            return None;
        };
        let adapter = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone)?;
        let instance_dir = adapter.instance_data_dir();
        let rel_path = crate::app::node_actions::db_script_rel_path_str(&segments);
        // Filesystem probe to disambiguate dir vs script.
        let dir_path = not_yet_done_postgres_adapter::query::db_script_dir_path(
            &instance_dir,
            &database,
            std::path::Path::new(&rel_path),
        );
        let is_dir = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { tokio::fs::metadata(&dir_path).await })
        })
        .map(|m| m.is_dir())
        .unwrap_or(false);
        // current_dir_rel: if the selected row is a dir, the dir
        // itself; otherwise the script's parent.
        let current_dir_rel = if is_dir {
            rel_path.clone()
        } else if let Some((parent, _)) = rel_path.rsplit_once('/') {
            parent.to_string()
        } else {
            String::new()
        };
        Some((
            view_index,
            pane_id,
            adapter,
            database,
            current_dir_rel,
            Some((rel_path, is_dir)),
        ))
    }

    /// Reject names that contain path separators or start with `.` —
    /// the underlying filesystem layer applies the same validation in
    /// `rename_db_script_entry`, but we surface early so the user
    /// gets a clean error instead of a generic io::Error.
    fn validate_db_script_name(&mut self, sub: &str, name: &str) -> bool {
        if name.is_empty()
            || name.contains('/')
            || name.contains('\\')
            || name.starts_with('.')
        {
            self.modal_message = Some(format!(
                ":db-script {sub} — invalid name '{name}' (no slashes or leading dot)"
            ));
            return false;
        }
        true
    }

    fn db_script_new_in_current_dir(&mut self, name: &str) {
        if !self.validate_db_script_name("new", name) {
            return;
        }
        let Some((view_index, pane_id, adapter, database, current_dir_rel, _selected)) =
            self.resolve_db_script_context("new")
        else {
            return;
        };
        let instance_dir = adapter.instance_data_dir();
        // Default extension: if the user did not include a dot in the
        // filename, append `.sql`. Anything containing a `.` is taken
        // as-is so `migrate.py`, `notes.md`, `helper.psql` etc. all work.
        // The check is on the final segment only — a path like
        // `util/audit` (no dot anywhere) still becomes `util/audit.sql`.
        let file_name = if name.contains('.') {
            name.to_string()
        } else {
            format!("{name}.sql")
        };
        let rel_path = if current_dir_rel.is_empty() {
            file_name.clone()
        } else {
            format!("{current_dir_rel}/{file_name}")
        };
        let database_for_write = database.clone();
        let rel_for_write = rel_path.clone();
        let file_name_for_template = file_name.clone();
        let result: std::io::Result<bool> = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let path = not_yet_done_postgres_adapter::query::db_script_path(
                    &instance_dir,
                    &database_for_write,
                    std::path::Path::new(&rel_for_write),
                );
                if tokio::fs::try_exists(&path).await.unwrap_or(false) {
                    return Ok(false);
                }
                // Ensure parent dir exists for nested scripts. The
                // storage `write_db_script` helper only handles flat
                // root, so we call out to mkdir then write directly.
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::write(
                    &path,
                    not_yet_done_postgres_adapter::query::default_db_script_file(
                        &database_for_write,
                        &file_name_for_template,
                    )
                    .as_bytes(),
                )
                .await?;
                Ok(true)
            })
        });
        match result {
            Ok(true) => {
                self.notify(format!("Created DB script '{rel_path}'"));
                self.spawn_content_load(view_index, pane_id);
                self.refresh_db_scripts_tree_children(view_index, pane_id, &database);
            }
            Ok(false) => {
                self.notify_error(format!("DB script '{rel_path}' already exists"));
            }
            Err(e) => self.notify_error(format!("Create script failed: {e}")),
        }
    }

    fn db_script_new_dir_in_current_dir(&mut self, name: &str) {
        if !self.validate_db_script_name("new-dir", name) {
            return;
        }
        let Some((view_index, pane_id, adapter, database, current_dir_rel, _selected)) =
            self.resolve_db_script_context("new-dir")
        else {
            return;
        };
        let instance_dir = adapter.instance_data_dir();
        let rel_path = if current_dir_rel.is_empty() {
            name.to_string()
        } else {
            format!("{current_dir_rel}/{name}")
        };
        let rel_for_write = rel_path.clone();
        let database_for_write = database.clone();
        let result: std::io::Result<()> = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                not_yet_done_postgres_adapter::query::create_db_script_dir(
                    &instance_dir,
                    &database_for_write,
                    std::path::Path::new(&rel_for_write),
                )
                .await
            })
        });
        match result {
            Ok(()) => {
                self.notify(format!("Created DB-script folder '{rel_path}'"));
                self.spawn_content_load(view_index, pane_id);
                self.refresh_db_scripts_tree_children(view_index, pane_id, &database);
            }
            Err(e) => self.notify_error(format!("Create folder failed: {e}")),
        }
    }

    fn db_script_rename_selected(&mut self, new_name: &str) {
        if !self.validate_db_script_name("rename", new_name) {
            return;
        }
        let Some((view_index, pane_id, adapter, database, _current_dir, selected)) =
            self.resolve_db_script_context("rename")
        else {
            return;
        };
        let Some((rel_path, _is_dir)) = selected else {
            self.modal_message =
                Some(":db-script rename — selected row is the group node, not an entry".to_string());
            return;
        };
        let instance_dir = adapter.instance_data_dir();
        let database_for_write = database.clone();
        let rel_for_write = rel_path.clone();
        let new_name_owned = new_name.to_string();
        let result: std::io::Result<()> = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                not_yet_done_postgres_adapter::query::rename_db_script_entry(
                    &instance_dir,
                    &database_for_write,
                    std::path::Path::new(&rel_for_write),
                    &new_name_owned,
                )
                .await
            })
        });
        match result {
            Ok(()) => {
                self.notify(format!("Renamed '{rel_path}' → '{new_name}'"));
                self.spawn_content_load(view_index, pane_id);
                self.refresh_db_scripts_tree_children(view_index, pane_id, &database);
            }
            Err(e) => self.notify_error(format!("Rename failed: {e}")),
        }
    }

    fn db_script_move_selected_or_marked(&mut self, dest: &str) {
        let Some((view_index, pane_id, adapter, database, _current_dir, selected)) =
            self.resolve_db_script_context("move")
        else {
            return;
        };
        // Prefer the marked source if set; otherwise the currently
        // selected row. The marked source can be cross-pane — but
        // same-database is enforced below.
        let (src_db, src_rel) = if let Some(marked) = self.marked_db_script_for_move.clone() {
            let Some((db, segs)) = crate::app::node_actions::parse_db_script_node_id(&marked) else {
                self.notify_error(format!("Marked source '{marked}' is not a DB-script id"));
                self.marked_db_script_for_move = None;
                return;
            };
            (db, crate::app::node_actions::db_script_rel_path_str(&segs))
        } else {
            let Some((rel, _)) = selected else {
                self.modal_message = Some(
                    ":db-script move — no marked source and no entry selected".to_string(),
                );
                return;
            };
            (database.clone(), rel)
        };
        if src_db != database {
            self.notify_error(format!(
                "Cross-database move not supported ({src_db} → {database})"
            ));
            return;
        }
        // Destination rel: `dest` may be absolute-from-root (`/foo/bar`)
        // or relative to the selected row's current dir.
        let dest_dir_rel = if let Some(stripped) = dest.strip_prefix('/') {
            stripped.trim_end_matches('/').to_string()
        } else {
            // Resolve against current_dir_rel from context.
            let current_dir_rel = match self.resolve_db_script_context("move") {
                Some((_, _, _, _, dir, _)) => dir,
                None => return,
            };
            if current_dir_rel.is_empty() {
                dest.trim_end_matches('/').to_string()
            } else {
                format!("{current_dir_rel}/{}", dest.trim_end_matches('/'))
            }
        };
        let src_name = std::path::Path::new(&src_rel)
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_string);
        let Some(src_name) = src_name else {
            self.notify_error(format!("Source '{src_rel}' has no file name"));
            return;
        };
        let dst_rel = if dest_dir_rel.is_empty() {
            src_name
        } else {
            format!("{dest_dir_rel}/{src_name}")
        };
        let instance_dir = adapter.instance_data_dir();
        let src_rel_clone = src_rel.clone();
        let dst_rel_clone = dst_rel.clone();
        let database_for_write = database.clone();
        let result: std::io::Result<()> = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                not_yet_done_postgres_adapter::query::move_db_script_entry(
                    &instance_dir,
                    &database_for_write,
                    std::path::Path::new(&src_rel_clone),
                    std::path::Path::new(&dst_rel_clone),
                )
                .await
            })
        });
        match result {
            Ok(()) => {
                self.notify(format!("Moved '{src_rel}' → '{dst_rel}'"));
                self.marked_db_script_for_move = None;
                self.spawn_content_load(view_index, pane_id);
                self.refresh_db_scripts_tree_children(view_index, pane_id, &database);
            }
            Err(e) => self.notify_error(format!("Move failed: {e}")),
        }
    }

    fn db_script_delete_selected(&mut self) {
        let Some((view_index, pane_id, _adapter, database, _current_dir, selected)) =
            self.resolve_db_script_context("delete")
        else {
            return;
        };
        let Some((rel_path, is_dir)) = selected else {
            self.modal_message = Some(
                ":db-script delete — selected row is the group node, not an entry".to_string(),
            );
            return;
        };
        if is_dir {
            self.confirm_delete_adapter_db_script_dir(view_index, pane_id, database, rel_path);
        } else {
            self.confirm_delete_adapter_db_script(view_index, pane_id, database, rel_path);
        }
    }

    /// CP-9: stage the confirm popup for unlinking a DB-level script.
    /// On accept the App calls `delete_adapter_db_script_now`, which
    /// removes the file (idempotent) and reloads the source pane.
    fn confirm_delete_adapter_db_script(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        database: String,
        script: String,
    ) {
        let msg = format!(
            "Delete DB script '{script}' in database '{database}'? (y/n)"
        );
        self.modal_message = Some(msg.clone());
        self.pending_confirmation = Some((
            msg,
            PendingConfirmation::DeleteAdapterDbScript {
                view_index,
                pane_id,
                database,
                script,
            },
        ));
    }

    /// CF-11: stage the confirm popup for a generic content-node delete.
    /// `node_id` is the adapter's authoritative id (we don't try to
    /// shorten it — the user sees the full path because the row label
    /// alone can be ambiguous on numeric ids).
    fn confirm_delete_content_node(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        node_id: String,
    ) {
        // Pull a user-friendly label from the selected row when we can
        // — falls back to the raw id if the pane / cursor moved. The
        // pane label is set from the adapter's `NodeSummary.label`, so
        // confluence pages show as the page title; postgres rows show
        // their last segment; etc.
        let label = self
            .content_view(view_index)
            .and_then(|cv| cv.find_pane(pane_id))
            .and_then(|pane| pane.selected_item_label().map(str::to_string))
            .unwrap_or_else(|| node_id.clone());
        let msg = format!("Delete '{label}'? (y/n)");
        self.modal_message = Some(msg.clone());
        self.pending_confirmation = Some((
            msg,
            PendingConfirmation::DeleteContentNode {
                view_index,
                pane_id,
                node_id,
            },
        ));
    }

    /// CF-11: spawn the actual `Node::execute("delete", ActionInput::None)`
    /// roundtrip on the current pane's adapter. On `ActionOutcome::Done`
    /// the result lands in `ContentActionDone`, which already notifies
    /// + reloads the pane.
    fn delete_content_node_now(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        node_id: String,
    ) {
        let Some(adapter) = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone)
        else {
            self.notify("No adapter for this view".to_string());
            return;
        };
        let tx = self.load_tx.clone();
        let action_id = "delete".to_string();
        tokio::spawn(async move {
            let outcome = async {
                let mut node = adapter.get_by_id(&node_id).await?;
                node.execute(&action_id, not_yet_done_content::ActionInput::None)
                    .await
            }
            .await;
            let result = match outcome {
                Ok(not_yet_done_content::ActionOutcome::Done { message }) => {
                    Ok(message.unwrap_or_else(|| "Deleted".to_string()))
                }
                Ok(_) => Ok("Deleted".to_string()),
                Err(e) => Err(format!("Delete failed: {e}")),
            };
            let _ = tx.send(LoadMsg::ContentActionDone {
                view_index,
                pane_id,
                result,
            });
        });
    }

    /// CP-9: unlink the script file and reload the source pane so the
    /// row disappears. `delete_db_script` is idempotent (NotFound is
    /// treated as success), so re-runs are safe.
    fn delete_adapter_db_script_now(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        database: String,
        script: String,
    ) {
        let Some(adapter) = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone)
        else {
            self.notify("No adapter for this view".to_string());
            return;
        };
        let instance_dir = adapter.instance_data_dir();
        let result: std::io::Result<()> = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                not_yet_done_postgres_adapter::query::delete_db_script(
                    &instance_dir,
                    &database,
                    &script,
                )
                .await
            })
        });
        match result {
            Ok(()) => {
                self.notify(format!("Deleted DB script '{script}'"));
                // Same dual-refresh pattern as `db_script_new_command`:
                // root reload for drill-down panes, tree-expand re-fire
                // for tree-mode panes with the DB-Scripts row expanded.
                self.spawn_content_load(view_index, pane_id);
                self.refresh_db_scripts_tree_children(view_index, pane_id, &database);
            }
            Err(e) => self.notify_error(format!("Delete failed: {e}")),
        }
    }

    /// Re-issue a tree-expand for every cached subtree whose immediate
    /// parent is either the `<database>/db_scripts` group node OR any
    /// `postgres:db_script_dir` under it. Used by the create/delete/
    /// rename/move paths so a tree-mode pane that currently shows the
    /// scripts/folders under an expanded DB-Scripts row picks up the
    /// new on-disk state without a full reload.
    ///
    /// Multi-tree-continuation (MT-1, DSF-3): both `postgres:db_script_dir`
    /// AND `postgres:db_script` are fanned out per parent so newly
    /// created folders show up alongside scripts — refreshing only the
    /// script bucket would leave new folders invisible until restart.
    fn refresh_db_scripts_tree_children(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        database: &str,
    ) {
        let group_id = format!("{database}/db_scripts");
        let sub_prefix = format!("{group_id}/");
        let paths: Vec<(Vec<String>, String)> = {
            let Some(cv) = self.content_view(view_index) else { return; };
            let Some(pane) = cv.find_pane(pane_id) else { return; };
            let Some(tree) = pane.tree.as_ref() else { return; };
            tree.cache
                .keys()
                .filter_map(|p| {
                    p.last().and_then(|last| {
                        if last == &group_id || last.starts_with(&sub_prefix) {
                            Some((p.clone(), last.clone()))
                        } else {
                            None
                        }
                    })
                })
                .collect()
        };
        let types: Vec<String> = vec![
            "postgres:db_script_dir".to_string(),
            "postgres:db_script".to_string(),
        ];
        for (path, parent_node_id) in paths {
            if let Some(cv) = self.content_view_mut(view_index) {
                cv.begin_tree_multi_load(pane_id, path.clone(), types.clone());
            }
            for ty in &types {
                self.spawn_tree_expand(
                    view_index,
                    pane_id,
                    path.clone(),
                    parent_node_id.clone(),
                    ty.clone(),
                    50,
                    None,
                    false,
                );
            }
        }
    }

    /// CP-9: `:db-script-new <database> <name>` — create an empty
    /// script file (default template) and open the editor on it. The
    /// caller passes everything after the command verb, so we parse
    /// the two whitespace-separated tokens here.
    fn db_script_new_command(&mut self, rest: &str) {
        let mut tokens = rest.split_whitespace();
        let database = match tokens.next() {
            Some(s) => s.to_string(),
            None => {
                self.modal_message = Some(
                    ":db-script-new expects <database> <script>".to_string(),
                );
                return;
            }
        };
        let script = match tokens.next() {
            Some(s) => s.to_string(),
            None => {
                self.modal_message = Some(
                    ":db-script-new expects a script name after the database".to_string(),
                );
                return;
            }
        };
        if tokens.next().is_some() {
            self.modal_message = Some(
                ":db-script-new — extra arguments after <script> (use names without spaces)"
                    .to_string(),
            );
            return;
        }
        if script.contains('/')
            || script.contains('\\')
            || script.starts_with('.')
            || script.is_empty()
        {
            self.modal_message = Some(format!(
                ":db-script-new — invalid script name '{script}' (no slashes or leading dot)"
            ));
            return;
        }
        // Same default-extension policy as the in-tree create flow: a
        // bare name gets `.sql`; anything containing a dot is taken
        // as-is (so the user can opt into `migrate.py`, `notes.md` …).
        let script = if script.contains('.') {
            script
        } else {
            format!("{script}.sql")
        };
        let view_index = match self.current_content_view_index_or_modal("db-script-new") {
            Some(idx) => idx,
            None => return,
        };
        let Some(adapter) = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone)
        else {
            self.modal_message =
                Some(":db-script-new — active tab has no adapter".to_string());
            return;
        };
        let instance_dir = adapter.instance_data_dir();
        let database_for_write = database.clone();
        let script_for_write = script.clone();
        let result: std::io::Result<bool> = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let path = not_yet_done_postgres_adapter::query::db_script_file_path(
                    &instance_dir,
                    &database_for_write,
                    &script_for_write,
                );
                if tokio::fs::try_exists(&path).await.unwrap_or(false) {
                    return Ok(false);
                }
                not_yet_done_postgres_adapter::query::write_db_script(
                    &instance_dir,
                    &database_for_write,
                    &script_for_write,
                    &not_yet_done_postgres_adapter::query::default_db_script_file(
                        &database_for_write,
                        &script_for_write,
                    ),
                )
                .await?;
                Ok(true)
            })
        });
        match result {
            Ok(true) => {
                self.notify(format!("Created DB script '{script}'"));
                // Use the active pane as the editor's source — we don't
                // have the dispatch pane_id here (cmdline route), and
                // `_pane_id` is unused by the editor session.
                let pane_id = self
                    .content_view(view_index)
                    .map(|cv| cv.active_pane_id())
                    .unwrap_or(0);
                // Refresh listings so the new script appears immediately:
                // root reload covers drilled-into-DB-Scripts panes; the
                // tree-expand re-fire covers tree-mode panes whose
                // DB-Scripts row is expanded (root reload alone doesn't
                // touch the cached children of expanded subtrees).
                self.spawn_content_load(view_index, pane_id);
                self.refresh_db_scripts_tree_children(view_index, pane_id, &database);
                // Cmdline create-flow: use the view-config flag for the
                // db_script ChildDef in case the user opts into in-place
                // editing globally for this view.
                let in_place = self
                    .content_view(view_index)
                    .and_then(|cv| cv.active_view_def())
                    .map(|v| crate::app::node_actions::editor_in_place_for_node_id(v, ""))
                    .unwrap_or(false);
                let _ = self.open_adapter_db_script_editor(view_index, pane_id, database, script, in_place);
            }
            Ok(false) => {
                self.modal_message = Some(format!(
                    ":db-script-new — script '{script}' already exists (use :w to edit)"
                ));
            }
            Err(e) => self.notify_error(format!("Create failed: {e}")),
        }
    }

    /// Spawn async drill-down load for a content view child level.
    /// Resolve the active (rendered) query a child/subtree `list()` should
    /// carry. Returns `None` unless the adapter opts into
    /// `propagates_query_to_subtree` — flat adapters keep child loads
    /// query-free (their child node types don't share the parent's query
    /// semantics). For filtered-tree adapters (the task forest) it mirrors
    /// [`ContentPane::root_load_request`]'s query resolution so the subtree
    /// filters by the same query as the root.
    fn subtree_query_for_pane(
        cv: &crate::views::content_view::ContentView,
        pane: &crate::views::content_view::ContentPane,
        adapter: &Arc<dyn not_yet_done_content::ContentAdapter>,
    ) -> Option<String> {
        if !adapter.capabilities().propagates_query_to_subtree {
            return None;
        }
        pane.root_load_request(&cv.view_defs)
            .and_then(|req| req.query.map(|raw| adapter.render_query(&raw, &req.vars)))
    }

    pub fn spawn_content_drill_down(
        &self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        node_id: String,
        child_node_type: String,
    ) {
        let cv = match self.content_view(view_index) {
            Some(cv) => cv,
            None => return,
        };
        let adapter = match cv.adapter.as_ref() {
            Some(a) => Arc::clone(a),
            None => return,
        };
        let pane = match cv.find_pane(pane_id) {
            Some(p) => p,
            None => return,
        };
        // Honor the active child's pagination config if any; otherwise
        // re-use whatever page the pane already has (e.g. after `>`/`<`),
        // and fall back to the historical hard-coded first page of 50.
        let page = pane
            .drill_load_page()
            .unwrap_or(not_yet_done_content::PageRequest { offset: 0, limit: 50 });
        // Filtered-tree adapters (capability `propagates_query_to_subtree`)
        // want the pane's active query honored at every depth, so the
        // drilled child list stays filtered. Flat adapters leave the
        // capability `false` and the child load keeps `query: None`.
        let subtree_query = Self::subtree_query_for_pane(cv, pane, &adapter);
        let retries = cv
            .view_defs
            .get(pane.view_def_index())
            .map(|v| v.retries)
            .unwrap_or(0);
        let tx = self.load_tx.clone();
        tokio::spawn(async move {
            let result = run_with_retries(retries, &tx, view_index, pane_id, || {
                let adapter = Arc::clone(&adapter);
                let node_id = node_id.clone();
                let child_node_type = child_node_type.clone();
                let subtree_query = subtree_query.clone();
                async move {
                    let parent = adapter.get_by_id(&node_id).await.map_err(|e| e.to_string())?;
                    let node_type = parent.children_types()
                        .into_iter()
                        .find(|t| t.type_id == child_node_type)
                        .ok_or_else(|| format!(
                            "Node type '{child_node_type}' not available on '{node_id}'"
                        ))?;
                    let sortable_columns = parent.sortable_columns(&node_type);
                    let params = not_yet_done_content::ListParams {
                        node_type,
                        query: subtree_query,
                        sort: Vec::new(),
                        page: Some(page),
                        download: false,
                        group_by: None,
                    };
                    let list = parent.list(params).await.map_err(|e| e.to_string())?;
                    Ok((list, sortable_columns))
                }
            })
            .await;
            match result {
                Ok((list, sortable_columns)) => {
                    let _ = tx.send(LoadMsg::ContentItems {
                        view_index,
                        pane_id,
                        items: list.items,
                        applied_sort: list.applied_sort,
                        page: list.page,
                        sortable_columns,
                        error: None,
                    });
                }
                Err(e) => {
                    let _ = tx.send(LoadMsg::ContentItems {
                        view_index,
                        pane_id,
                        items: vec![],
                        applied_sort: Vec::new(),
                        page: None,
                        sortable_columns: Vec::new(),
                        error: Some(e),
                    });
                }
            }
        });
    }

    /// Async-load the children of a tree-mode parent. Mirrors
    /// [`spawn_content_drill_down`] but the result lands in the
    /// pane's `tree.cache[parent_path]` via [`LoadMsg::TreeChildren`]
    /// instead of replacing `pane.items`.
    pub fn spawn_tree_expand(
        &self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        parent_path: Vec<String>,
        parent_node_id: String,
        child_node_type: String,
        page_size: u32,
        page: Option<not_yet_done_content::PageRequest>,
        append: bool,
    ) {
        let cv = match self.content_view(view_index) {
            Some(cv) => cv,
            None => return,
        };
        let adapter = match cv.adapter.as_ref() {
            Some(a) => Arc::clone(a),
            None => return,
        };
        let retries = cv
            .find_pane(pane_id)
            .and_then(|p| cv.view_defs.get(p.view_def_index()))
            .map(|v| v.retries)
            .unwrap_or(0);
        // Carry the pane's active query into the expansion for filtered-tree
        // adapters (see `subtree_query_for_pane`); flat adapters get `None`.
        let subtree_query = cv
            .find_pane(pane_id)
            .and_then(|p| Self::subtree_query_for_pane(cv, p, &adapter));
        let tx = self.load_tx.clone();
        tokio::spawn(async move {
            let payload = run_with_retries(retries, &tx, view_index, pane_id, || {
                let adapter = Arc::clone(&adapter);
                let parent_node_id = parent_node_id.clone();
                let child_node_type = child_node_type.clone();
                let subtree_query = subtree_query.clone();
                async move {
                    let parent = adapter.get_by_id(&parent_node_id).await.map_err(|e| e.to_string())?;
                    let node_type = parent.children_types()
                        .into_iter()
                        .find(|t| t.type_id == child_node_type)
                        .ok_or_else(|| format!(
                            "Node type '{child_node_type}' not available on '{parent_node_id}'"
                        ))?;
                    let page_request = page.unwrap_or(not_yet_done_content::PageRequest {
                        offset: 0,
                        limit: page_size,
                    });
                    let params = not_yet_done_content::ListParams {
                        node_type,
                        query: subtree_query,
                        sort: Vec::new(),
                        page: Some(page_request),
                        download: false,
                        group_by: None,
                    };
                    let list = parent.list(params).await.map_err(|e| e.to_string())?;
                    Ok(TreeChildrenPayload {
                        items: list.items,
                        page_info: list.page,
                        child_node_type: child_node_type.clone(),
                    })
                }
            })
            .await;
            let _ = tx.send(LoadMsg::TreeChildren {
                view_index,
                pane_id,
                parent_path,
                result: payload,
                append,
            });
        });
    }

    /// CT-6: default per-call cap on tree-find hits. Picked low
    /// enough that a single popup doesn't drown the user (refining
    /// the query is cheaper than scrolling 500 hits), high enough to
    /// cover most realistic results. Surfaced as `truncated = true`
    /// when the server reports more.
    pub const TREE_FIND_DEFAULT_LIMIT: u32 = 100;

    /// CT-6: spawn an adapter-side tree search.
    ///
    /// Mirrors [`spawn_tree_expand`] for the search-in-tree call: the
    /// pane's `tree_find_begin(query)` is the caller's job (so the
    /// loading hint shows up immediately on the keystroke), and this
    /// helper drives the asynchronous round-trip. The response lands
    /// as [`LoadMsg::TreeFindResult`] regardless of success/failure;
    /// `poll_load` then routes it through `tree_find_complete` /
    /// `tree_find_fail` / `tree_find_clear` per outcome.
    ///
    /// `limit` caps the hit count the adapter returns. Picked at the
    /// call site so future per-view tuning (e.g. a `tree_find.limit`
    /// YAML knob) lands here without touching the trait. The default
    /// caller in CT-7 uses [`TREE_FIND_DEFAULT_LIMIT`].
    pub fn spawn_tree_find(
        &self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        query: String,
        limit: u32,
    ) {
        let cv = match self.content_view(view_index) {
            Some(cv) => cv,
            None => return,
        };
        let adapter = match cv.adapter.as_ref() {
            Some(a) => Arc::clone(a),
            None => return,
        };
        let tx = self.load_tx.clone();
        let query_for_call = query.clone();
        tokio::spawn(async move {
            let result = adapter
                .search_in_tree(&query_for_call, limit)
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(LoadMsg::TreeFindResult {
                view_index,
                pane_id,
                query,
                result,
            });
        });
    }

    /// CT-7: pump one step of the lazy-expand chain for a pane's
    /// active tree-find. Called after `TreeFindResult` lands the
    /// initial hits, and after every `TreeChildren` so the walk
    /// continues until the current hit's leaf is on screen (or the
    /// walker reports `NotInTree`).
    ///
    /// No-op when the pane isn't mid-tree-find. Multi-step walks
    /// re-enter via the next `TreeChildren` LoadMsg: each
    /// `NeedTreeExpand` dispatch fires an `ExpandTreeNode` request
    /// whose response routes back here.
    pub fn drive_tree_find_chain(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
    ) {
        let Some(cv) = self.content_view_mut(view_index) else { return; };
        let Some(msg) = cv.drive_tree_find(view_index, pane_id) else { return; };
        let _ = self.process_sub_view_message(msg);
    }

    /// Drive the `expand_depth` auto-expansion cascade after tree data
    /// landed in a pane: collect the pane's pending expand requests and
    /// dispatch them through the normal request path, exactly as a
    /// manual Enter on each row would.
    fn drive_tree_auto_expand(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
    ) {
        let reqs = match self.content_view_mut(view_index) {
            Some(cv) => cv.pending_auto_expand_requests(view_index, pane_id),
            None => return,
        };
        for req in reqs {
            let _ = self.process_view_request(req);
        }
    }

    /// After a root reload landed in a tree pane, re-fetch the children of
    /// every expanded node so the whole visible tree reflects the reload —
    /// not just depth 0 (see
    /// [`ContentPane::pending_expanded_refresh_requests`](crate::views::content_view::ContentPane)).
    fn drive_tree_expanded_refresh(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
    ) {
        let reqs = match self.content_view(view_index) {
            Some(cv) => cv.pending_expanded_refresh_requests(view_index, pane_id),
            None => return,
        };
        for req in reqs {
            let _ = self.process_view_request(req);
        }
    }

    /// Drain all pending async results. Returns `true` if at least one
    /// message was processed (i.e. visible state may have changed and the
    /// frame should be redrawn).
    pub fn poll_load(&mut self) -> bool {
        let mut changed = false;
        while let Ok(msg) = self.load_rx.try_recv() {
            changed |= self.handle_load_msg(msg);
        }
        changed
    }

    /// Apply a single [`LoadMsg`] to App state, returning `true` when
    /// visible state may have changed. Split out from [`Self::poll_load`]
    /// so the event-driven (1b) `select!` loop can handle the one message
    /// its `load_rx.recv()` consumed before draining the rest with
    /// `poll_load`. See docs/decisions/0001-render-loop-dirty-gating.md.
    pub fn handle_load_msg(&mut self, msg: LoadMsg) -> bool {
        {
            match msg {
                LoadMsg::Tasks(tasks) => {
                    let ids: Vec<Uuid> = tasks.iter().map(|t| t.id).collect();
                    self.tasks_view.state.set_tasks(tasks);
                    self.refresh_task_table();
                    self.spawn_load_task_tags(ids);
                }
                LoadMsg::TaskTags(map) => {
                    self.tasks_view.state.task_tags = map;
                    self.refresh_task_table();
                }
                LoadMsg::Trackings(rows) => {
                    let focus_idx = self.trackings_view.state.set_rows(rows);
                    self.rebuild_trackings_table();
                    if let Some(idx) = focus_idx {
                        self.trackings_view.table.set_selected(idx);
                    }
                }
                LoadMsg::Error(e) => {
                    not_yet_done_content::http_log::log_error("tasks_load", &e);
                    self.last_error = Some(e.clone());
                    self.tasks_view.state.set_load_error(e);
                }
                LoadMsg::TrackingError(e) => {
                    self.trackings_view.state.set_load_error(e.clone());
                    self.notify_error(format!("Tracking filter error: {e}"));
                }
                LoadMsg::ContentItems { view_index, pane_id, items, applied_sort, page, sortable_columns, error } => {
                    if let Some(err) = error.as_ref() {
                        not_yet_done_content::http_log::log_error("content_load", err);
                        self.last_error = Some(err.clone());
                    }
                    if let Some(cv) = self.content_view_mut(view_index) {
                        cv.set_items_for_pane(pane_id, items, applied_sort, page, sortable_columns, error);
                    }
                    // Eager tree (capability `supports_eager_subtree`): the
                    // root rows are in; pull the WHOLE expanded subtree in one
                    // `list_subtree` call instead of running the per-node
                    // cascade. This covers reload (r / Invalidation::All) too —
                    // the single eager load renews every level.
                    let eager_depth = self.content_view(view_index).and_then(|cv| {
                        cv.find_pane(pane_id)
                            .and_then(|p| p.eager_subtree_depth(&cv.view_defs))
                    });
                    if let Some(depth) = eager_depth {
                        self.spawn_subtree_load(view_index, pane_id, depth);
                    } else {
                        // Tree mode: kick off the `expand_depth` cascade now
                        // that the depth-0 rows are in.
                        self.drive_tree_auto_expand(view_index, pane_id);
                        // …and refresh what's already expanded, so a reload
                        // (r / Invalidation::All) renews the whole visible
                        // tree, not just the depth-0 rows. Disjoint from the
                        // cascade: it only touches loaded expanded paths.
                        self.drive_tree_expanded_refresh(view_index, pane_id);
                    }
                    // `:tree-find` queued a search to run against the
                    // freshly-reloaded snapshot — fire it now that the
                    // root rows are in. The lazy expand-to-hit walk then
                    // proceeds via the normal `TreeFindResult` /
                    // `TreeChildren` drivers.
                    let pending = self
                        .content_view_mut(view_index)
                        .and_then(|cv| cv.find_pane_mut(pane_id))
                        .and_then(|pane| pane.take_pending_tree_find());
                    if let Some(query) = pending {
                        if let Some(pane) = self
                            .content_view_mut(view_index)
                            .and_then(|cv| cv.find_pane_mut(pane_id))
                        {
                            pane.tree_find_begin(query.clone());
                        }
                        self.spawn_tree_find(
                            view_index,
                            pane_id,
                            query,
                            Self::TREE_FIND_DEFAULT_LIMIT,
                        );
                    }
                    // Reload may have shifted the row under the cursor onto a
                    // different item (e.g. mark_as_read sorts the read entry
                    // away). Refresh preview when the row's id no longer
                    // matches `preview_key`.
                    let preview_req = self
                        .content_view_mut(view_index)
                        .and_then(|cv| cv.pending_preview_request(view_index, pane_id));
                    if let Some(req) = preview_req {
                        let _ = self.process_view_request(req);
                    }
                }
                LoadMsg::TreeChildren { view_index, pane_id, parent_path, result, append } => {
                    if let Some(cv) = self.content_view_mut(view_index) {
                        if let Some(pane) = cv.find_pane_mut(pane_id) {
                            pane.retry_state = None;
                        }
                    }
                    match result {
                        Ok(payload) => {
                            if let Some(cv) = self.content_view_mut(view_index) {
                                cv.apply_tree_children(
                                    pane_id,
                                    parent_path,
                                    payload.items,
                                    payload.page_info,
                                    append,
                                    payload.child_node_type,
                                );
                            }
                            // Continue the `expand_depth` cascade one
                            // level deeper (no-op once disarmed).
                            self.drive_tree_auto_expand(view_index, pane_id);
                            // CT-7: if this pane is mid-tree-find,
                            // continue the lazy-expand walk now that
                            // a new level has landed.
                            self.drive_tree_find_chain(view_index, pane_id);
                        }
                        Err(e) => {
                            not_yet_done_content::http_log::log_error("tree_expand", &e);
                            self.last_error = Some(e.clone());
                            self.notify_error(format!("Tree expand error: {e}"));
                            if let Some(cv) = self.content_view_mut(view_index) {
                                cv.cancel_tree_expand(pane_id, parent_path);
                            }
                        }
                    }
                }
                LoadMsg::Subtree { view_index, pane_id, parent_path, result } => {
                    if let Some(cv) = self.content_view_mut(view_index) {
                        if let Some(pane) = cv.find_pane_mut(pane_id) {
                            pane.retry_state = None;
                        }
                    }
                    match result {
                        Ok(subtree) => {
                            if let Some(cv) = self.content_view_mut(view_index) {
                                cv.apply_subtree(pane_id, parent_path, subtree);
                            }
                            // The eager load already laid down the whole
                            // expanded shape; nothing to cascade. A pending
                            // tree-find walk may still want to advance.
                            self.drive_tree_find_chain(view_index, pane_id);
                        }
                        Err(e) => {
                            not_yet_done_content::http_log::log_error("subtree_load", &e);
                            self.last_error = Some(e.clone());
                            self.notify_error(format!("Tree load error: {e}"));
                            // Fall back to the per-node cascade so the tree
                            // still expands progressively despite the eager
                            // load failing.
                            self.drive_tree_auto_expand(view_index, pane_id);
                        }
                    }
                }
                LoadMsg::CustomQueryItems { view_index, pane_id, result } => {
                    match result {
                        Ok(payload) => {
                            if let Some(cv) = self.content_view_mut(view_index) {
                                cv.apply_custom_query_result(
                                    pane_id,
                                    payload.items,
                                    payload.page,
                                    Some(payload.custom_query),
                                );
                            }
                            self.set_query_error(None);
                            if let Some(s) = payload.status {
                                self.notify(s);
                            }
                        }
                        Err(e) => {
                            not_yet_done_content::http_log::log_error("custom_query", &e);
                            self.last_error = Some(e.clone());
                            self.notify_error(format!("Query error: {e}"));
                        }
                    }
                }
                LoadMsg::ContentPreview { view_index, pane_id, cache_key, text } => {
                    if let Some(cv) = self.content_view_mut(view_index) {
                        cv.set_preview_description_for_pane(pane_id, &cache_key, text);
                    }
                }
                LoadMsg::EditorSessionReady { node_id, token, result } => {
                    // A newer open (higher token) or a cancel superseded this
                    // one — it owns the loading state now, so drop this stale
                    // session without touching the indicator.
                    if token != self.editor_load_token {
                        return true;
                    }
                    if let Some(msg) = self.editor_loading_msg.take() {
                        self.notification_bar.remove(&msg);
                    }
                    match result {
                        Ok(session) => match self.open_session(session) {
                            // Detached editors launch inside `open_session`
                            // and return `None`; `Inline`/`Launch` must bubble
                            // out to main.rs via the post-`poll_load` drain.
                            EditorRequest::None => {}
                            other => self.pending_editor_request = Some(other),
                        },
                        Err(e) => self.notify_error(format!("Failed to load {node_id}: {e}")),
                    }
                }
                LoadMsg::ContentActionDone { view_index, pane_id, result } => {
                    match result {
                        Ok(msg) => self.notify(msg),
                        Err(msg) => {
                            self.set_query_error(Some(msg.clone()));
                            self.notification_bar.push(msg);
                        }
                    }
                    self.reload_content_pane_current_level(view_index, pane_id);
                }
                LoadMsg::NodeActionDispatched { view_index, pane_id, node_id, action_name, result, node_label, node_type } => {
                    self.handle_node_action_dispatched(view_index, pane_id, node_id, action_name, result, node_label, node_type);
                }
                LoadMsg::ContentAdapterStatus { view_index, status } => {
                    if let Some(cv) = self.content_view_mut(view_index) {
                        cv.set_auth_status(status.clone());
                    }
                    self.react_to_adapter_status(view_index, &status);
                }
                LoadMsg::AdapterInvalidation { view_index, inv } => {
                    self.handle_adapter_invalidation(view_index, inv);
                }
                LoadMsg::ContentLoadProgress {
                    view_index,
                    pane_id,
                    attempt,
                    max_attempts,
                    last_error,
                } => {
                    if let Some(cv) = self.content_view_mut(view_index) {
                        if let Some(pane) = cv.find_pane_mut(pane_id) {
                            pane.retry_state = Some(crate::views::content_view::RetryState {
                                attempt,
                                max_attempts,
                                last_error,
                            });
                        }
                    }
                }
                LoadMsg::TreeFindResult { view_index, pane_id, query, result } => {
                    // Late-arrival sanity check (CT-6): the user may
                    // have re-typed (clearing + restarting via CT-9)
                    // before this in-flight call returned. If the
                    // pane's active query no longer matches, drop the
                    // payload silently — the matching response will
                    // arrive (or already has). Done in a scoped
                    // borrow so the later notify_* / last_error
                    // assignments don't conflict.
                    enum Outcome {
                        Stale,
                        Landed { count: usize, truncated: bool },
                        Unsupported,
                        Failed(String),
                    }
                    let outcome = {
                        let Some(cv) = self.content_view_mut(view_index) else { return true; };
                        let Some(pane) = cv.find_pane_mut(pane_id) else { return true; };
                        let stale = pane
                            .tree_find
                            .as_ref()
                            .map(|s| s.query != query)
                            .unwrap_or(true);
                        if stale {
                            Outcome::Stale
                        } else {
                            match result {
                                Ok(Some(res)) => {
                                    let count = res.hits.len();
                                    let truncated = res.truncated;
                                    pane.tree_find_complete(res.hits, truncated);
                                    Outcome::Landed { count, truncated }
                                }
                                Ok(None) => {
                                    // Adapter doesn't support tree search.
                                    // Drop the state (so n/N revert to local
                                    // /-search) and notify outside the borrow.
                                    pane.tree_find_clear();
                                    Outcome::Unsupported
                                }
                                Err(e) => {
                                    pane.tree_find_fail();
                                    Outcome::Failed(e)
                                }
                            }
                        }
                    };
                    match outcome {
                        Outcome::Stale => {}
                        Outcome::Landed { count, truncated } => {
                            let suffix = if truncated { ", truncated" } else { "" };
                            if count == 0 {
                                self.notify(format!("Tree find \"{query}\": no matches"));
                            } else {
                                self.notify(format!(
                                    "Tree find \"{query}\": {count} hit{}{suffix} — n/N to navigate",
                                    if count == 1 { "" } else { "s" },
                                ));
                                // Kick off the lazy-expand walk so
                                // the first hit becomes visible
                                // without the user having to press
                                // `n` once just to start.
                                self.drive_tree_find_chain(view_index, pane_id);
                            }
                        }
                        Outcome::Unsupported => {
                            self.notify_error(
                                "Adapter doesn't support tree search.".to_string(),
                            );
                        }
                        Outcome::Failed(e) => {
                            not_yet_done_content::http_log::log_error("tree_find", &e);
                            self.last_error = Some(e.clone());
                            self.notify_error(format!("Tree find error: {e}"));
                        }
                    }
                }
                LoadMsg::CredentialSubmitResult { view_index, error } => {
                    if let Some(popup) = self.adapter_creds_popup.as_mut() {
                        if popup.view_index() == view_index {
                            match error {
                                Some(reason) => popup.set_error(reason),
                                None => popup.close(),
                            }
                        }
                    }
                    if self
                        .adapter_creds_popup
                        .as_ref()
                        .is_some_and(|p| !p.is_open())
                    {
                        self.adapter_creds_popup = None;
                    }
                }
            }
        }
        true
    }

    // -----------------------------------------------------------------------
    // Key routing: resolve (pure) → dispatch (mutates)
    // -----------------------------------------------------------------------

    pub fn handle_key(&mut self, key: &str) -> EditorRequest {
        // Quit always works, regardless of mode/popups.
        if self.keybindings.global.bindings.get(&GlobalAction::Quit)
            .map_or(false, |b| b.matches(key))
        {
            self.should_quit = true;
            return EditorRequest::None;
        }

        // Modal message: dismiss on any key (but not when awaiting shortcut/confirm).
        if self.modal_message.is_some()
            && self.awaiting_favorite_shortcut.is_none()
            && self.pending_confirmation.is_none()
        {
            self.modal_message = None;
            self.sync_components();
            return EditorRequest::None;
        }

        // Confirmation dialog: y/Enter confirms, anything else cancels.
        if let Some((_, confirmation)) = self.pending_confirmation.take() {
            self.modal_message = None;
            if key == "y" || key == "Y" || key == "enter" {
                self.execute_confirmation(confirmation);
            } else {
                self.notify("Cancelled".to_string());
            }
            self.sync_components();
            return EditorRequest::None;
        }

        // Postgres script shortcut capture mode.
        if let Some(coords) = self.awaiting_postgres_script_shortcut.take() {
            self.modal_message = None;
            if key == "esc" {
                // Cancelled.
            } else if self.is_shortcut_taken(key) {
                self.modal_message = Some(format!(
                    "Shortcut '{}' is already taken!\n\nPress another key for '{}'\nEsc to cancel",
                    key, coords.script
                ));
                self.awaiting_postgres_script_shortcut = Some(coords);
            } else {
                let chord = key.to_string();
                let script_label = coords.script.clone();
                self.bind_postgres_script_shortcut(coords, &chord);
                self.modal_message = Some(format!(
                    "Script '{}' bound to [{}]",
                    script_label, chord
                ));
            }
            self.sync_components();
            return EditorRequest::None;
        }

        // Favorite shortcut capture mode.
        if let Some((scope, name, query)) = self.awaiting_favorite_shortcut.take() {
            self.modal_message = None;
            if key == "esc" {
                // Cancelled — no modal needed.
            } else if let Some(conflict) = self.favorite_shortcut_conflict(&scope, &name, key) {
                // Show error and re-prompt.
                self.modal_message = Some(format!(
                    "Shortcut '{}' is already taken by {}!\n\nPress another key for '{}'\nEsc to cancel",
                    key, conflict, name
                ));
                self.awaiting_favorite_shortcut = Some((scope, name, query));
            } else {
                self.add_favorite(&scope, name.clone(), key.to_string(), query);
                self.modal_message = Some(format!("Favorite '{}' added with shortcut [{}]", name, key));
            }
            self.sync_components();
            return EditorRequest::None;
        }

        // Sort-hint mode: intercept all keys while active.
        if self.sort_hint_phase.is_active() {
            self.sort_hint_handle_key(key);
            self.sync_components();
            return EditorRequest::None;
        }

        // Jump mode: intercept all keys when jump is active. Broken
        // content tabs have no table, so jump mode can't be active and
        // we fall through to global key dispatch.
        if self.active_table_mut().is_some_and(|t| t.jump_active()) {
            if key == "esc" {
                if let Some(table) = self.active_table_mut() {
                    table.jump_mode_close();
                }
            } else if key.chars().count() == 1 && !key.chars().next().unwrap().is_control() {
                let ch = key.chars().next().unwrap();
                if let Some(table) = self.active_table_mut() {
                    if table.jump_waiting_for_char() {
                        table.jump_mode_search(ch);
                    } else {
                        table.jump_mode_label_input(ch);
                    }
                }
            }
            self.sync_components();
            return EditorRequest::None;
        }

        // Command line mode: delegate to active view's CmdlineComponent.
        {
            use crate::views::{HasCmdline, CmdlineKeyResult};
            let cmdline_active = match self.active_tab {
                Tab::Tasks => self.tasks_view.cmdline_active(),
                Tab::Trackings => self.trackings_view.cmdline_active(),
                Tab::Content(idx) => self.content_view(idx)
                    .map(|cv| cv.cmdline_active()).unwrap_or(false),
            };
            if cmdline_active {
                let result = match self.active_tab {
                    Tab::Tasks => self.tasks_view.cmdline_handle_key(key),
                    Tab::Trackings => self.trackings_view.cmdline_handle_key(key),
                    Tab::Content(idx) => self.content_view_mut(idx)
                        .map(|cv| cv.cmdline_handle_key(key))
                        .unwrap_or(CmdlineKeyResult::Closed),
                };
                match result {
                    CmdlineKeyResult::Execute(cmd) => {
                        self.execute_cmdline(&cmd);
                    }
                    CmdlineKeyResult::Closed | CmdlineKeyResult::Handled => {}
                }
                self.sync_components();
                return EditorRequest::None;
            }
        }

        // Search input mode: delegate to active view's SearchComponent.
        {
            use crate::views::{Searchable, SearchKeyResult};
            let search_active = match self.active_tab {
                Tab::Tasks => self.tasks_view.search_active(),
                Tab::Trackings => self.trackings_view.search_active(),
                Tab::Content(_) => false,
            };
            if search_active {
                let result = match self.active_tab {
                    Tab::Tasks => self.tasks_view.search_handle_key(key),
                    Tab::Trackings => self.trackings_view.search_handle_key(key),
                    Tab::Content(_) => SearchKeyResult::Cancelled,
                };
                match result {
                    SearchKeyResult::Accepted | SearchKeyResult::Cancelled
                    | SearchKeyResult::QueryChanged | SearchKeyResult::Handled => {}
                }
                self.sync_components();
                return EditorRequest::None;
            }
        }

        // Chord handling: if a pending key exists, try to complete the chord.
        if let Some(pending) = self.pending_key.take() {
            let chord = format!("{pending}{key}");
            // Chords are never SearchNext/SearchPrev (single-char `n`/`N`),
            // so any chord that fires here is an "other key" that should
            // lock in the `/`-search auto-expansion before running. The
            // chord branch bypasses `dispatch()` and therefore the commit
            // hook there — call it explicitly.
            self.tasks_view.commit_search_transient();
            // Check if the chord matches any binding.
            // Global comes first so chords like `gl` land here regardless
            // of the active tab.
            let global_chord = self.keybindings.global.bindings.iter()
                .find(|(_, b)| b.matches(&chord))
                .map(|(a, _)| a.clone());
            if let Some(action) = global_chord {
                let _ = self.handle_global_action(action);
                self.sync_components();
                return EditorRequest::None;
            }
            let common_chord = self.keybindings.common.bindings.iter()
                .find(|(_, b)| b.matches(&chord))
                .map(|(a, _)| a.clone());
            if let Some(action) = common_chord {
                let _ = self.handle_common_action(action);
                self.sync_components();
                return EditorRequest::None;
            }
            // Tab-specific chord sections are checked only on their own
            // tab so a cross-tab name collision (e.g. `tasks.zm` ==
            // `content.zm` for TreeCollapseAll) doesn't swallow the chord
            // on the wrong tab. Each branch must guard its lookup with
            // `active_tab == …` and is allowed to early-return only when
            // we are on its tab — otherwise fall through to the next
            // section so the right one can pick the chord up.
            if self.active_tab == Tab::Tasks {
                let tasks_chord = self.keybindings.tasks.bindings.iter()
                    .find(|(_, b)| b.matches(&chord))
                    .map(|(a, _)| a.clone());
                if let Some(action) = tasks_chord {
                    // Route the chord through TasksView first so sub-view
                    // switches (`vt`/`vl`) and tree chords (`zr`/`zm`)
                    // reach the right handler. Only fall back to
                    // `handle_tasks_action` for actions the view leaves
                    // for the App (currently none of the chord-bound ones).
                    let msg = self.tasks_view.handle_key(&chord);
                    match msg {
                        SubViewMessage::Unhandled => {
                            let _ = self.handle_tasks_action(action);
                        }
                        other => {
                            let _ = self.process_sub_view_message(other);
                        }
                    }
                    self.sync_components();
                    return EditorRequest::None;
                }
            }
            if self.active_tab == Tab::Trackings {
                let trackings_chord = self.keybindings.trackings.bindings.iter()
                    .find(|(_, b)| b.matches(&chord))
                    .map(|(a, _)| a.clone());
                if let Some(action) = trackings_chord {
                    let _ = self.handle_trackings_action(action);
                    self.sync_components();
                    return EditorRequest::None;
                }
            }
            // Content-tab chords (e.g. `zm` → TreeCollapseAll). Route
            // through the active ContentView's central dispatcher so the
            // tree-mode guard + drill post-processing match the single-
            // key path.
            let content_chord = self.keybindings.content.bindings.iter()
                .find(|(_, b)| b.matches(&chord))
                .map(|(a, _)| a.clone());
            if let Some(action) = content_chord {
                if let Tab::Content(idx) = self.active_tab {
                    if let Some(cv) = self.content_view_mut(idx) {
                        let msg = cv.dispatch_content_action(action);
                        match msg {
                            SubViewMessage::Unhandled => {}
                            other => {
                                let _ = self.process_sub_view_message(other);
                            }
                        }
                        self.drain_content_cursor_closes(idx);
                    }
                }
                self.sync_components();
                return EditorRequest::None;
            }
            // Chord matches a user-defined cmdline shortcut?
            // (`cmdline_shortcuts:` in tui.yaml; the default ships
            // `mc`/`mp` for cut/paste-node.)
            if let Some(cmd) = self.config.cmdline_shortcuts.get(&chord).cloned() {
                self.execute_cmdline(&cmd);
                self.sync_components();
                return EditorRequest::None;
            }
            // Chord didn't match — but if the accumulated chord is itself
            // a prefix of an even longer binding (e.g. `gl` → `glm`/`glp`),
            // keep stashing so the next key can complete it. Without this
            // branch the dispatcher would top out at 2-char chords.
            if self.keybindings.global.bindings.values().any(|b| b.is_prefix(&chord))
                || self.keybindings.common.bindings.values().any(|b| b.is_prefix(&chord))
                || self.keybindings.tasks.bindings.values().any(|b| b.is_prefix(&chord))
                || self.keybindings.trackings.bindings.values().any(|b| b.is_prefix(&chord))
                || self.keybindings.content.bindings.values().any(|b| b.is_prefix(&chord))
                || self.cmdline_shortcut_chord_prefix(&chord)
            {
                self.pending_key = Some(chord);
                self.sync_components();
                return EditorRequest::None;
            }
            // Truly no match — drop pending, process `key` normally.
        }

        // Link popup intercepts all keys while open.
        if self.link_popup.is_some() {
            self.handle_link_popup_key(key);
            self.sync_components();
            return EditorRequest::None;
        }

        // Config picker popup intercepts all keys while open.
        if self.config_picker_popup.is_some() {
            self.handle_config_picker_key(key);
            self.sync_components();
            return EditorRequest::None;
        }

        // Tab-set switch popup intercepts all keys while open.
        if self.tab_set_popup.is_open() {
            self.handle_tab_set_popup_key(key);
            self.sync_components();
            return EditorRequest::None;
        }

        // Tag-management menu (:tag) intercepts keys while open.
        if self.tag_menu.is_open() {
            let req = self.handle_tag_menu_key(key);
            self.sync_components();
            return req;
        }

        // Script management menu (:script / `x` / per-view) intercepts
        // keys while open.
        if self.script_menu.is_open() {
            let req = self.handle_script_menu_key(key);
            self.sync_components();
            return req;
        }

        // Adapter credentials popup intercepts all keys.
        if self.adapter_creds_popup.is_some() {
            use crate::components::adapter_creds_popup::CredsKeyOutcome;
            let popup = self.adapter_creds_popup.as_mut().unwrap();
            let outcome = popup.handle_key(key);
            match outcome {
                CredsKeyOutcome::Cancel => {
                    self.adapter_creds_popup = None;
                }
                CredsKeyOutcome::Submit { values } => {
                    let view_index = popup.view_index();
                    self.spawn_submit_credentials(view_index, values);
                }
                CredsKeyOutcome::Consumed | CredsKeyOutcome::Pass => {}
            }
            self.sync_components();
            return EditorRequest::None;
        }

        // Query-variable popup intercepts all keys.
        if self.query_var_popup.is_some() {
            use crate::components::query_var_popup::QueryVarKeyOutcome;
            let outcome = self.query_var_popup.as_mut().unwrap().handle_key(key);
            match outcome {
                QueryVarKeyOutcome::Cancel => {
                    self.query_var_popup = None;
                }
                QueryVarKeyOutcome::Submit { values } => {
                    let target = self
                        .query_var_popup
                        .as_ref()
                        .map(|p| p.target().clone());
                    self.query_var_popup = None;
                    if let Some(target) = target {
                        self.apply_query_with_vars(target, values);
                    }
                }
                QueryVarKeyOutcome::Consumed | QueryVarKeyOutcome::Pass => {}
            }
            self.sync_components();
            return EditorRequest::None;
        }

        // Column config popup intercepts all keys.
        if let Some(popup) = &mut self.column_config_popup {
            popup.handle_key(key, &self.keybindings);
            if !popup.is_open() {
                let result = popup.result();
                self.column_config_popup = None;
                self.apply_column_config(result);
            }
            self.sync_components();
            return EditorRequest::None;
        }

        // Tasks query menu — delegated to TasksView.
        if self.active_tab == Tab::Tasks && self.tasks_view.has_query_menu() {
            let result = self.tasks_view.handle_query_menu_key(key)
                .map(|msg| self.process_sub_view_message(msg))
                .unwrap_or(EditorRequest::None);
            self.sync_components();
            return result;
        }
        // Trackings query menu — delegated to TrackingsView.
        if self.active_tab == Tab::Trackings && self.trackings_view.has_query_menu() {
            let result = self.trackings_view.handle_query_menu_key(key)
                .map(|msg| self.process_trackings_message(msg))
                .unwrap_or(EditorRequest::None);
            self.sync_components();
            return result;
        }

        // Tracking grouping popup — delegated to TrackingsView.
        if self.trackings_view.has_group_popup() {
            let msgs = self.trackings_view.handle_group_popup_key(key);
            for msg in msgs {
                self.process_trackings_message(msg);
            }
            self.sync_components();
            return EditorRequest::None;
        }

        // Trackings tab: delegate to TrackingsView component.
        let trackings_has_popup = self.script_menu.is_open()
            || self.column_config_popup.is_some();
        if self.active_tab == Tab::Trackings && !trackings_has_popup {
            if self.trackings_view.state.fuzzy_active {
                let handled = self.handle_trackings_fuzzy_key(key);
                if handled {
                    self.sync_components();
                    return EditorRequest::None;
                }
            }

            // Normal mode: delegate to TrackingsView.
            let forest = self.tasks_view.state.forest.as_ref();
            let msg = self.trackings_view.handle_key(key, forest);
            match msg {
                SubViewMessage::Unhandled => {
                    // Fall through to global/favorites/chords.
                }
                other => {
                    let result = self.process_trackings_message(other);
                    self.sync_components();
                    return result;
                }
            }
        }

        // Content file-picker popup (e.g. Taiga attachment upload) —
        // intercepts every key while open. The picker handles its own
        // Esc/submit via `FilePickerEvent`.
        if matches!(self.active_tab, Tab::Content(_))
            && self.content_file_picker_popup.is_some()
        {
            if let Some(ev) = crate::events::key_string_to_tuirealm(key) {
                let popup = self.content_file_picker_popup.as_mut().unwrap();
                let outcome = tuirealm::component::AppComponent::on(
                    &mut popup.picker,
                    &tuirealm::event::Event::Keyboard(ev),
                );
                match outcome {
                    Some(FilePickerEvent::Confirmed(paths)) => {
                        let popup = self.content_file_picker_popup.take().unwrap();
                        if paths.is_empty() {
                            self.notify("No files selected".to_string());
                        } else {
                            self.execute_content_action_files(
                                popup.view_index,
                                popup.pane_id,
                                popup.node_id,
                                popup.action_id,
                                paths,
                            );
                        }
                    }
                    Some(FilePickerEvent::Cancelled) => {
                        self.content_file_picker_popup = None;
                    }
                    _ => {}
                }
            }
            self.sync_components();
            return EditorRequest::None;
        }

        // Content form popup (generic `InputSpec::Form` actions) — intercepts
        // every key while open. The popup owns its field focus + in-field
        // editing; we only act on Submitted/Cancelled.
        if matches!(self.active_tab, Tab::Content(_)) && self.content_form_popup.is_some() {
            let popup_state = self.content_form_popup.as_mut().unwrap();
            match popup_state.popup.handle_key(key) {
                ContentFormEvent::Submitted(values) => {
                    let popup = self.content_form_popup.take().unwrap();
                    self.execute_content_action_form(
                        popup.view_index,
                        popup.pane_id,
                        popup.node_id,
                        popup.action_id,
                        values,
                    );
                }
                ContentFormEvent::Cancelled => {
                    self.content_form_popup = None;
                }
                ContentFormEvent::Consumed => {}
            }
            self.sync_components();
            return EditorRequest::None;
        }

        // Content action popup (transitions, etc.) — applies to any content tab.
        // The popup handles Next/Prev/Backspace/Cursor/Typing intrinsically
        // via its own PopupAction bindings; we only dispatch Enter (apply)
        // and Esc (close) here.
        if matches!(self.active_tab, Tab::Content(_)) && self.content_action_popup.is_some() {
            let popup_state = self.content_action_popup.as_mut().unwrap();
            match key {
                "enter" => {
                    if let Some(item) = popup_state.popup.selected_item() {
                        let value = item.value.clone();
                        let vi = popup_state.view_index;
                        let pid = popup_state.pane_id;
                        let nid = popup_state.node_id.clone();
                        let aid = popup_state.action_id.clone();
                        self.content_action_popup = None;
                        self.execute_content_action(vi, pid, nid, aid, value);
                    }
                }
                "esc" => {
                    self.content_action_popup = None;
                }
                _ => {
                    popup_state.popup.handle_key(key);
                }
            }
            self.sync_components();
            return EditorRequest::None;
        }

        // Phase-2 action-chain interceptor. Resolution order is the
        // active ChildDef → active ViewDef → global `action_chains`. The
        // most-specific scope wins; a `None` value at any scope disables
        // the binding without falling through. Skipped when a chord is
        // pending (the second char belongs to the chord), when a popup
        // is consuming keys, or when the focused content pane is in a
        // text-input mode (fuzzy / search) — those keys belong in the
        // input buffer, not in a chain.
        let content_text_input = matches!(self.active_tab, Tab::Content(idx)
            if self.content_view(idx).is_some_and(|cv| cv.is_text_input_active()));
        if self.pending_key.is_none() && !self.has_input_popup() && !content_text_input {
            if let Some(entry) = self.resolve_action_chain(key) {
                match entry {
                    Some(chain) => {
                        self.run_action_chain(key, chain);
                    }
                    None => {
                        // Explicitly disabled at scope — consume key.
                    }
                }
                self.sync_components();
                return EditorRequest::None;
            }
        }

        // Content tab: delegate to ContentView.
        if let Tab::Content(idx) = self.active_tab {
            // SQ-8d: pre-fill the Postgres-table shortcut cache for the
            // currently-focused table so `build_view_claims` can register
            // global apply-on-chord handlers for them. No-op when the
            // focus isn't on a Postgres table or the cache is already
            // populated for that table.
            self.ensure_postgres_table_shortcuts_loaded(idx);
            if let Some(cv) = self.content_view_mut(idx) {
                let msg = cv.handle_key(key);
                match msg {
                    SubViewMessage::Unhandled => {
                        // Fall through to global/chords. Still drain any
                        // cursor closes the view queued during dispatch.
                        self.drain_content_cursor_closes(idx);
                    }
                    other => {
                        let result = self.process_sub_view_message(other);
                        self.drain_content_cursor_closes(idx);
                        self.sync_components();
                        return result;
                    }
                }
            }
        }

        // Tasks tab: delegate to TasksView component.
        let has_popup = self.script_menu.is_open()
            || self.column_config_popup.is_some();

        if self.active_tab == Tab::Tasks && !has_popup && !self.tasks_view.state.form_visible() {
            // Fuzzy mode: delegate to view's fuzzy handler.
            if self.tasks_view.fuzzy_active() {
                if let Some(msg) = self.tasks_view.handle_fuzzy_key(key) {
                    self.process_sub_view_message(msg);
                    self.sync_components();
                    return EditorRequest::None;
                }
                // Not handled by fuzzy — fall through.
            }

            // Normal mode: delegate to view.
            let msg = self.tasks_view.handle_key(key);
            match msg {
                SubViewMessage::Unhandled => {
                    // Fall through to global/favorites/chords.
                }
                other => {
                    let result = self.process_sub_view_message(other);
                    self.sync_components();
                    return result;
                }
            }
        }

        let mode = action::input_mode(
            self.script_menu.is_open(),
            false, // fuzzy handled above for tasks; trackings uses its own path
            self.active_tab == Tab::Tasks && self.tasks_view.state.active_form == Some(TasksForm::Filter),
        );

        // Chord-prefix detection runs BEFORE single-key resolution: when
        // `key` is a prefix of any chord binding active in this tab,
        // stash it as `pending_key` and wait for the next char. Without
        // this, a single-key binding that shadows a chord prefix (e.g.
        // global `z` → DismissNotifications shadowing tasks `zr`/`zm`)
        // would always win, making the chord unreachable. Tab-specific
        // sections only count for their tab so e.g. trackings `v` isn't
        // suppressed by tasks `vt`/`vl` when the trackings tab is active.
        if mode == action::InputMode::Normal && self.pending_key.is_none() {
            let prefix_global = self.keybindings.global.bindings.values().any(|b| b.is_prefix(key));
            let prefix_common = self.keybindings.common.bindings.values().any(|b| b.is_prefix(key));
            let prefix_tab = match self.active_tab {
                Tab::Tasks => self.keybindings.tasks.bindings.values().any(|b| b.is_prefix(key)),
                Tab::Trackings => self.keybindings.trackings.bindings.values().any(|b| b.is_prefix(key)),
                // Content chords (e.g. `zm` → TreeCollapseAll) live in
                // `content.bindings`. Without this, `z` would never be
                // stashed as a pending key on a Content tab and the chord
                // would silently break into two single-key dispatches.
                Tab::Content(_) => self.keybindings.content.bindings.values().any(|b| b.is_prefix(key)),
            };
            let prefix_cmdline = self.cmdline_shortcut_chord_prefix(key);
            if prefix_global || prefix_common || prefix_tab || prefix_cmdline {
                self.pending_key = Some(key.to_string());
                return EditorRequest::None;
            }
        }

        // Autonumber tab switch: in constellation mode the visible tabs
        // own *every* digit key (`1`..`9`, then `0`). Resolved here at
        // global priority — after view delegation, so a view that binds a
        // digit on its own tab still wins. A mapped digit switches tabs; an
        // unmapped digit (more digits than visible tabs) is swallowed so
        // the legacy fixed `GlobalAction` keys can't switch to a hidden
        // tab. In legacy mode (no constellation) this block is inert and
        // the fixed keys below take over.
        let is_plain_digit =
            key.len() == 1 && key.chars().next().is_some_and(|c| c.is_ascii_digit());
        if mode == action::InputMode::Normal && self.tab_layout.autonumber() && is_plain_digit {
            if let Some(tab) = self.tab_layout.tab_for_key(key) {
                self.set_active_tab(tab);
            }
            self.sync_components();
            return EditorRequest::None;
        }

        let action = action::resolve_key(
            key,
            mode,
            &self.keybindings,
            false, // tasks bindings handled by TasksView above
            self.active_tab == Tab::Trackings,
            self.tasks_view.state.form_visible(),
        );

        // If the action is Noop, try favorites then cmdline shortcuts.
        // Chord-prefix detection has already run above.
        if action == Action::Noop && mode == action::InputMode::Normal {
            // Try activating a favorite shortcut.
            if self.try_activate_favorite(key) {
                self.sync_components();
                return EditorRequest::None;
            }
            // User-defined cmdline shortcut (`cmdline_shortcuts:` in
            // tui.yaml). Runs the bound command exactly as if the user
            // had typed it after `:` — useful for one-key access to
            // `:config`, `:linkprune`, custom CLI commands, etc.
            if let Some(cmd) = self.config.cmdline_shortcuts.get(key).cloned() {
                self.execute_cmdline(&cmd);
                self.sync_components();
                return EditorRequest::None;
            }
        }

        let result = self.dispatch(action);
        self.sync_components();
        result
    }

    fn dispatch(&mut self, action: Action) -> EditorRequest {
        // Lock in the `/`-search auto-expansion as soon as the user
        // touches anything other than n/N. SearchNext/Prev are the only
        // actions that keep the transient "replace-on-jump" mode alive;
        // everything else (j, k, space, tab switch, …) promotes the
        // current ancestor path into `flipped` so it stays visible.
        let preserves_search_transient = matches!(
            action,
            Action::Common(CommonAction::SearchNext) | Action::Common(CommonAction::SearchPrev)
        );
        if !preserves_search_transient {
            self.tasks_view.commit_search_transient();
        }
        match action {
            Action::Global(g) => self.handle_global_action(g),
            Action::Common(c) => self.handle_common_action(c),
            Action::Tasks(t) => self.handle_tasks_action(t),
            Action::Trackings(t) => self.handle_trackings_action(t),
            Action::Form(f) => { self.handle_form_action(f); EditorRequest::None }
            Action::Content(_) | Action::Window(_) | Action::QueryMenu(_) => {
                // Not produced by `resolve_key` (these reach the App via the
                // chain interceptor in Phase 2). Routed centrally through
                // `dispatch_chained_action` once that path lands; until then
                // they're a no-op so the match stays exhaustive.
                let _ = self.dispatch_chained_action(action.clone());
                EditorRequest::None
            }
            Action::InsertChar(c) => { self.dispatch_insert(c); EditorRequest::None }
            Action::Backspace => { self.dispatch_backspace(); EditorRequest::None }
            Action::CursorLeft => { self.dispatch_cursor_left(); EditorRequest::None }
            Action::CursorRight => { self.dispatch_cursor_right(); EditorRequest::None }
            Action::Escape => { self.dispatch_escape(); EditorRequest::None }
            Action::Submit => self.dispatch_submit(),
            Action::Toggle => { self.dispatch_toggle(); EditorRequest::None }
            Action::Reset => { self.dispatch_reset(); EditorRequest::None }
            Action::Blocked | Action::Noop => EditorRequest::None,
        }
    }

    /// Look up an action chain for `key`, walking ChildDef → ViewDef →
    /// global. Returns `Some(Some(chain))` to run, `Some(None)` when
    /// disabled at a scope (consume key without running anything), and
    /// `None` when no scope defines the binding (caller should fall
    /// through to ordinary key handling).
    fn resolve_action_chain(&self, key: &str) -> Option<Option<Vec<Action>>> {
        let mut scopes: Vec<&crate::action::ActionChains> = Vec::new();
        if let Tab::Content(idx) = self.active_tab {
            if let Some(cv) = self.content_view(idx) {
                scopes.extend(cv.action_chain_scopes());
            }
        }
        scopes.push(&self.keybindings.action_chains);
        crate::action::resolve_chain_in_scopes(&scopes, key).cloned()
    }

    /// Execute a chain in order. On the first step that returns `Err`,
    /// stop and surface a notification — partial chains are visible in
    /// the UI rather than silently swallowed. Successful no-ops (e.g.
    /// `content.next_page` at the last page) keep the chain going.
    fn run_action_chain(&mut self, key: &str, chain: Vec<Action>) {
        for (i, action) in chain.into_iter().enumerate() {
            if let Err(e) = self.dispatch_chained_action(action) {
                self.notify_error(format!("chain `{key}`: step {i} aborted: {e}"));
                return;
            }
        }
    }

    /// Whether some popup or sticky modal is currently consuming keys.
    /// Used by the chain interceptor to make sure user-defined bindings
    /// don't pre-empt critical popup interaction.
    fn has_input_popup(&self) -> bool {
        self.script_menu.is_open()
            || self.column_config_popup.is_some()
            || self.adapter_creds_popup.is_some()
            || self.query_var_popup.is_some()
            || self.content_action_popup.is_some()
            || self.content_file_picker_popup.is_some()
            || self.content_form_popup.is_some()
            || self.link_popup.is_some()
            || self.config_picker_popup.is_some()
            || self.tab_set_popup.is_open()
    }

    /// Execute a single chainable action through the Phase-2 dispatch
    /// path. Used both as a fallback inside `dispatch` (when chain
    /// actions reach the App via the standard Action match) and as the
    /// per-step entry point of the chain interceptor in
    /// [`run_action_chain`]. Returns `Err` (chain-aborting) for actions
    /// outside the V1 whitelist or when a tab/mode mismatch makes the
    /// action a no-op (e.g. window.* outside a Content tab).
    fn dispatch_chained_action(&mut self, action: Action) -> Result<(), String> {
        if !action.is_chainable() {
            return Err(format!("action `{action}` is not chainable in V1"));
        }
        match action {
            Action::Common(c) => {
                self.handle_common_action(c);
                Ok(())
            }
            Action::Window(w) => {
                let Tab::Content(idx) = self.active_tab else {
                    return Err(format!("window.{w} requires a Content tab"));
                };
                let Some(cv) = self.content_view_mut(idx) else {
                    return Err(format!("window.{w} on broken content tab"));
                };
                let msg = cv.dispatch_window_action(w);
                self.process_sub_view_message(msg);
                self.drain_content_cursor_closes(idx);
                Ok(())
            }
            Action::Content(c) => {
                let Tab::Content(idx) = self.active_tab else {
                    return Err(format!("content.{c} requires a Content tab"));
                };
                let Some(cv) = self.content_view_mut(idx) else {
                    return Err(format!("content.{c} on broken content tab"));
                };
                let msg = cv.dispatch_content_action(c);
                self.process_sub_view_message(msg);
                Ok(())
            }
            other => Err(format!("dispatch for `{other}` not implemented")),
        }
    }

    // -----------------------------------------------------------------------
    // Text input dispatch — routes to popup, fuzzy, or filter form
    // -----------------------------------------------------------------------

    fn dispatch_insert(&mut self, c: char) {
        if self.task_table().fuzzy_active {
            self.task_table_mut().fuzzy_insert(c);
        } else if self.tasks_view.state.active_form == Some(TasksForm::Filter) {
            let focused = self.tasks_view.state.filter.focused_field;
            if focused != FilterField::Status && focused != FilterField::ShowDeleted {
                self.tasks_view.state.filter.insert_char(c);
                self.spawn_load();
            }
        }
    }

    fn dispatch_backspace(&mut self) {
        if self.task_table().fuzzy_active {
            self.task_table_mut().fuzzy_backspace();
        } else if self.tasks_view.state.active_form == Some(TasksForm::Filter) {
            self.tasks_view.state.filter.backspace();
            self.spawn_load();
        }
    }

    fn dispatch_cursor_left(&mut self) {
        if self.task_table().fuzzy_active {
            self.task_table_mut().fuzzy_cursor_left();
        } else if self.tasks_view.state.active_form == Some(TasksForm::Filter) {
            let focused = self.tasks_view.state.filter.focused_field;
            if focused == FilterField::Status {
                self.tasks_view.state.filter.focus_prev();
            } else {
                self.tasks_view.state.filter.cursor_left();
            }
        }
    }

    fn dispatch_cursor_right(&mut self) {
        if self.task_table().fuzzy_active {
            self.task_table_mut().fuzzy_cursor_right();
        } else if self.tasks_view.state.active_form == Some(TasksForm::Filter) {
            let focused = self.tasks_view.state.filter.focused_field;
            if focused == FilterField::Status {
                self.tasks_view.state.filter.focus_next();
            } else {
                self.tasks_view.state.filter.cursor_right();
            }
        }
    }

    fn dispatch_escape(&mut self) {
        if self.link_popup.take().is_some() {
            return;
        }
        // Tail-end Esc consumer: clear the link mark when nothing else
        // claimed the key. Keeps Esc semantically "cancel pending state"
        // without competing with per-view modal handlers above.
        if self.marked_link.is_some() {
            self.link_clear_mark();
            return;
        }
        if self.cut_node_id.is_some() {
            self.cut_node_id = None;
            self.notify("Cut cancelled".to_string());
            return;
        }
        if self.marked_db_script_for_move.take().is_some() {
            self.notify("DB-script move cancelled".to_string());
            return;
        }
        if self.content_marked_node.take().is_some() {
            self.notify("Move cancelled".to_string());
            return;
        }
        // Fuzzy cancel is handled via FuzzyFilterCancel action.
    }

    fn dispatch_submit(&mut self) -> EditorRequest {
        if self.tasks_view.state.active_form == Some(TasksForm::Filter) {
            self.spawn_load();
        }
        EditorRequest::None
    }

    fn dispatch_toggle(&mut self) {
        if self.tasks_view.state.active_form == Some(TasksForm::Filter) {
            let focused = self.tasks_view.state.filter.focused_field;
            if focused == FilterField::Status {
                self.tasks_view.state.filter.toggle_status_cursor();
                self.spawn_load();
            } else if focused == FilterField::ShowDeleted {
                self.tasks_view.state.filter.toggle_show_deleted();
                self.spawn_load();
            }
        }
    }

    fn dispatch_reset(&mut self) {
        if self.tasks_view.state.active_form == Some(TasksForm::Filter) {
            self.tasks_view.state.filter.reset();
            self.spawn_load();
        }
    }

    fn handle_form_action(&mut self, action: FormAction) {
        if self.tasks_view.state.active_form == Some(TasksForm::Filter) {
            let focused = self.tasks_view.state.filter.focused_field;
            match action {
                FormAction::Next => self.tasks_view.state.filter.focus_next(),
                FormAction::Prev => self.tasks_view.state.filter.focus_prev(),
                FormAction::MultiselectNext if focused == FilterField::Status => {
                    self.tasks_view.state.filter.status_cursor_next();
                }
                FormAction::MultiselectPrev if focused == FilterField::Status => {
                    self.tasks_view.state.filter.status_cursor_prev();
                }
                _ => {}
            }
        }
    }

    // -----------------------------------------------------------------------
    // Action handlers
    // -----------------------------------------------------------------------

    fn handle_global_action(&mut self, action: GlobalAction) -> EditorRequest {
        match action {
            GlobalAction::Quit => self.should_quit = true,
            GlobalAction::TabTasks => {
                self.set_active_tab(Tab::Tasks);
                if self.tasks_view.state.load_state == LoadState::Idle {
                    self.spawn_load();
                }
            }
            GlobalAction::TabTrackings => {
                self.set_active_tab(Tab::Trackings);
                if self.trackings_view.state.load_state == crate::tabs::LoadState::Idle {
                    self.spawn_load_trackings();
                }
            }
            GlobalAction::TabJira => {
                self.set_active_tab(Tab::Content(0));
            }
            GlobalAction::TabTaiga => {
                if self.content_views.len() > 1 {
                    self.set_active_tab(Tab::Content(1));
                }
            }
            GlobalAction::TabPostgres => {
                if self.content_views.len() > 2 {
                    self.set_active_tab(Tab::Content(2));
                }
            }
            GlobalAction::TabConfluence => {
                if self.content_views.len() > 3 {
                    self.set_active_tab(Tab::Content(3));
                }
            }
            GlobalAction::TabNext => {
                self.set_active_tab(self.tab_layout.next(self.active_tab));
            }
            GlobalAction::TabPrev => {
                self.set_active_tab(self.tab_layout.prev(self.active_tab));
            }
            GlobalAction::DismissNotifications => self.dismiss_notifications(),
            GlobalAction::ShowLastError => return self.open_last_error_editor(),
            GlobalAction::TabSetPopup => self.open_tab_set_popup(),
            GlobalAction::LinkMark => self.link_mark_current(),
            GlobalAction::LinkPaste => self.link_paste_current(),
            GlobalAction::LinkOpenPopup => self.link_open_popup(),
            GlobalAction::LinkJumpBack => self.link_jump_back(),
            GlobalAction::LinkJumpForward => self.link_jump_forward(),
        }
        EditorRequest::None
    }

    /// Open the tab-set switch popup, populated from `tabs.sets` in their
    /// deterministic display order. A no-op (with a notification) when no
    /// constellation is configured — there is nothing to switch between.
    fn open_tab_set_popup(&mut self) {
        use crate::components::tab_set_popup::TabSetEntry;
        let active = self.config.tabs.active.clone();
        let entries: Vec<TabSetEntry> = self
            .config
            .tabs
            .sets_sorted()
            .into_iter()
            .map(|(name, set)| TabSetEntry {
                name: name.clone(),
                label: set.label.clone().unwrap_or_else(|| name.clone()),
                icon: set.icon.clone(),
                shortcut: set.shortcut.clone(),
                active: *name == active,
            })
            .collect();
        if entries.is_empty() {
            self.notify("No tab sets configured (tabs.sets in tui.yaml)".to_string());
            return;
        }
        self.tab_set_popup.open(entries);
    }

    /// Dispatch a key while the tab-set popup is open. Switching mutates
    /// the in-memory active constellation and rebuilds the tab layout;
    /// the change is session-only (not written back to `tui.yaml`).
    fn handle_tab_set_popup_key(&mut self, key: &str) {
        use crate::components::tab_set_popup::TabSetPopupMessage;
        match self.tab_set_popup.handle_key(key) {
            TabSetPopupMessage::Switch(name) => {
                if self.config.tabs.active != name {
                    self.config.tabs.active = name.clone();
                    self.rebuild_tab_layout();
                    self.notify(format!("Tab set: {name}"));
                }
            }
            TabSetPopupMessage::Unhandled
            | TabSetPopupMessage::Handled
            | TabSetPopupMessage::Closed => {}
        }
    }

    /// True iff some `cmdline_shortcuts:` key is strictly longer than
    /// `key` and starts with it — i.e. `key` is a chord-prefix that
    /// should be stashed and waited on (e.g. `m` for `mc`/`mp`).
    fn cmdline_shortcut_chord_prefix(&self, key: &str) -> bool {
        self.config.cmdline_shortcuts.keys().any(|k| k.len() > key.len() && k.starts_with(key))
    }

    /// Clear notification bar, sticky notification, and the most recent
    /// query-error banner. Shared by `GlobalAction::DismissNotifications`
    /// and the `:dismiss-notifications` cmdline command.
    fn dismiss_notifications(&mut self) {
        self.notification_bar.clear();
        self.notification = None;
        self.set_query_error(None);
    }

    /// `:cut-node` — mark the currently selected task for moving. Pure
    /// state change; the tree is not touched until `:paste-node` runs.
    /// Re-running silently overwrites the previous mark.
    fn cut_node_command(&mut self) {
        if self.active_tab != Tab::Tasks {
            self.modal_message = Some(":cut-node only works on the Tasks tab".to_string());
            return;
        }
        let Some(id) = self.selected_task_id() else {
            self.modal_message = Some(":cut-node — no task selected".to_string());
            return;
        };
        let desc = self
            .tasks_view
            .state
            .task_rows
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.description.clone())
            .unwrap_or_else(|| id.to_string());
        self.cut_node_id = Some(id);
        self.notify(format!("Cut: {desc} — paste with :paste-node (mp)"));
    }

    /// `:paste-node` — reparent the previously cut task so the currently
    /// selected task becomes its new parent. Refuses any move that would
    /// create a cycle (target == cut node, or target inside cut node's
    /// subtree); on refusal the tree is left untouched and a modal error
    /// is shown.
    fn paste_node_command(&mut self) {
        if self.active_tab != Tab::Tasks {
            self.modal_message = Some(":paste-node only works on the Tasks tab".to_string());
            return;
        }
        let Some(cut_id) = self.cut_node_id else {
            self.modal_message = Some(":paste-node — nothing cut (use :cut-node / mc first)".to_string());
            return;
        };
        let Some(target_id) = self.selected_task_id() else {
            self.modal_message = Some(":paste-node — no target task selected".to_string());
            return;
        };
        if target_id == cut_id {
            self.modal_message = Some(":paste-node — cannot paste a task onto itself".to_string());
            return;
        }
        // Cycle check: walk the target's ancestor chain. If `cut_id`
        // appears anywhere on it, the paste would put the cut node
        // beneath one of its own descendants.
        let parent_of: std::collections::HashMap<Uuid, Uuid> = self
            .tasks_view
            .state
            .task_rows
            .iter()
            .filter_map(|t| t.parent_id.map(|p| (t.id, p)))
            .collect();
        let mut cur = Some(target_id);
        while let Some(node) = cur {
            if node == cut_id {
                self.modal_message = Some(
                    ":paste-node — cannot move a task into its own subtree".to_string(),
                );
                return;
            }
            cur = parent_of.get(&node).copied();
        }
        // No-op short-circuit: already a child of the target.
        if self
            .tasks_view
            .state
            .task_rows
            .iter()
            .find(|t| t.id == cut_id)
            .and_then(|t| t.parent_id)
            == Some(target_id)
        {
            self.cut_node_id = None;
            self.notify(":paste-node — task is already a child of the target".to_string());
            return;
        }

        let service = Arc::clone(&self.task_service);
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                service
                    .update_task(cut_id, None, None, None, Some(Some(target_id)), None)
                    .await
            })
        });
        match result {
            Ok(_) => {
                let desc = self
                    .tasks_view
                    .state
                    .task_rows
                    .iter()
                    .find(|t| t.id == cut_id)
                    .map(|t| t.description.clone())
                    .unwrap_or_else(|| cut_id.to_string());
                self.cut_node_id = None;
                self.notify(format!("Moved: {desc}"));
                // Cursor stays on the target (the new parent). DataTable
                // restores by selected_id on rebuild; setting focus to
                // `cut_id` would silently fail when the new parent is
                // still collapsed.
                self.spawn_load();
            }
            Err(e) => {
                self.modal_message = Some(format!(":paste-node failed: {e}"));
            }
        }
    }

    /// `:jump <Tab>[:<sub>]` — programmatic tab + sub-tab switch.
    ///
    /// Recognised forms (case-insensitive head, sub-token compared
    /// case-insensitively against `TasksSubView::title` /
    /// `TrackingsSubView::title`):
    ///   - `Tasks`, `Tasks:list`, `Tasks:tree`
    ///   - `Trackings`, `Trackings:normal`, `Trackings:condensed`,
    ///     `Trackings:tree`
    ///   - any content tab — matched against `tab_name`
    ///
    /// Used by scripts (via the script-output relay) and by users from
    /// the `:` cmdline. Unknown tab or sub-tab => modal error, no
    /// state change.
    fn jump_command(&mut self, target: &str) {
        let (head, sub) = match target.split_once(':') {
            Some((h, s)) => (h.trim(), Some(s.trim())),
            None => (target.trim(), None),
        };
        if head.is_empty() {
            self.modal_message = Some(":jump — empty tab name".to_string());
            return;
        }

        if head.eq_ignore_ascii_case("tasks") {
            self.set_active_tab(Tab::Tasks);
            if let Some(s) = sub {
                let sv = match s.to_ascii_lowercase().as_str() {
                    "list" => Some(TasksSubView::List),
                    "tree" => Some(TasksSubView::Tree),
                    _ => None,
                };
                let Some(sv) = sv else {
                    self.modal_message =
                        Some(format!(":jump — unknown Tasks sub-view '{s}' (list|tree)"));
                    return;
                };
                if self.tasks_view.set_sub_view(sv) {
                    self.spawn_load();
                }
            }
            return;
        }

        if head.eq_ignore_ascii_case("trackings") {
            self.set_active_tab(Tab::Trackings);
            if let Some(s) = sub {
                use crate::tabs::TrackingsSubView;
                let sv = match s.to_ascii_lowercase().as_str() {
                    "normal" => Some(TrackingsSubView::Normal),
                    "condensed" => Some(TrackingsSubView::Condensed),
                    "tree" => Some(TrackingsSubView::Tree),
                    _ => None,
                };
                let Some(sv) = sv else {
                    self.modal_message = Some(format!(
                        ":jump — unknown Trackings sub-view '{s}' (normal|condensed|tree)"
                    ));
                    return;
                };
                self.trackings_view.state.sub_view = sv;
                self.rebuild_trackings_table();
            }
            return;
        }

        // Content tab — match on tab_name (case-insensitive).
        let idx = self
            .content_views
            .iter()
            .position(|slot| slot.tab_name().eq_ignore_ascii_case(head));
        match idx {
            Some(i) => {
                self.set_active_tab(Tab::Content(i));
                if let Some(s) = sub {
                    self.modal_message = Some(format!(
                        ":jump — content tabs don't take a sub-view (got ':{s}')"
                    ));
                }
            }
            None => {
                self.modal_message =
                    Some(format!(":jump — unknown tab '{head}'"));
            }
        }
    }

    /// `:focus-task [-i] /seg/seg/...` — walk the task hierarchy from
    /// the roots down through children, matching each segment against
    /// task descriptions. Default is case-sensitive substring matching;
    /// `-i` switches the whole match to case-insensitive. Each segment
    /// may opt into regex matching with the `re:` prefix (e.g.
    /// `re:\b151\b`). On success, auto-expands the ancestor path and
    /// parks the cursor on the matched node.
    ///
    /// Modal error (tree unchanged) when:
    ///   - the active tab is not Tasks
    ///   - the active sub-view is not Tree (where the expand makes sense)
    ///   - an unknown flag appears before the path
    ///   - the path doesn't start with `/`
    ///   - any segment has a malformed `re:` regex
    ///   - any segment matches no child of the previous match
    ///   - any segment matches more than one child (ambiguous)
    fn focus_task_command(&mut self, raw_args: &str) {
        if self.active_tab != Tab::Tasks {
            self.modal_message = Some(":focus-task only works on the Tasks tab".to_string());
            return;
        }
        if self.tasks_view.sub_view() != TasksSubView::Tree {
            self.modal_message =
                Some(":focus-task only works in the Tasks:tree sub-view".to_string());
            return;
        }

        // Parse leading flags. Currently only `-i` is supported.
        let mut case_insensitive = false;
        let mut rest = raw_args.trim_start();
        loop {
            let Some(tok) = rest.split_whitespace().next() else {
                break;
            };
            if !tok.starts_with('-') {
                break;
            }
            match tok {
                "-i" => {
                    case_insensitive = true;
                    rest = rest[tok.len()..].trim_start();
                }
                other => {
                    self.modal_message = Some(format!(
                        ":focus-task — unknown flag '{other}' (only -i is supported)"
                    ));
                    return;
                }
            }
        }
        let path = rest;

        let rows = &self.tasks_view.state.task_rows;
        let target = match not_yet_done_core::task_path::walk_task_path(rows, path, case_insensitive) {
            not_yet_done_core::task_path::WalkOutcome::Found(id) => id,
            not_yet_done_core::task_path::WalkOutcome::MissingLeadingSlash => {
                self.modal_message = Some(
                    ":focus-task expects a /-rooted path (e.g. /work/clients/acme)".to_string(),
                );
                return;
            }
            not_yet_done_core::task_path::WalkOutcome::EmptyPath => {
                self.modal_message = Some(":focus-task — path is empty".to_string());
                return;
            }
            not_yet_done_core::task_path::WalkOutcome::BadRegex { msg, .. } => {
                self.modal_message = Some(format!(":focus-task — {msg}"));
                return;
            }
            not_yet_done_core::task_path::WalkOutcome::NotFound { depth, seg, .. } => {
                let segments: Vec<&str> = path
                    .strip_prefix('/')
                    .unwrap_or("")
                    .split('/')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect();
                let scope = if depth == 0 {
                    "root level".to_string()
                } else {
                    format!("under '{}'", segments[depth - 1])
                };
                self.modal_message = Some(format!(
                    ":focus-task — no task matching '{seg}' at {scope}"
                ));
                return;
            }
            not_yet_done_core::task_path::WalkOutcome::Ambiguous { seg, candidates, .. } => {
                let descs: Vec<String> = candidates
                    .iter()
                    .take(5)
                    .filter_map(|id| rows.iter().find(|t| t.id == *id))
                    .map(|t| format!("'{}'", t.description))
                    .collect();
                let more = if candidates.len() > 5 {
                    format!(", … (+{})", candidates.len() - 5)
                } else {
                    String::new()
                };
                self.modal_message = Some(format!(
                    ":focus-task — '{seg}' is ambiguous: {}{}",
                    descs.join(", "),
                    more
                ));
                return;
            }
        };

        // Use the same transient-open mechanism as `/`-search, then
        // commit it so the path stays open after the focus.
        let rows_clone: Vec<Task> = self.tasks_view.state.task_rows.clone();
        self.tasks_view.tree_set_transient_open_for(target, &rows_clone);
        self.tasks_view.tree_commit_transient_open(&rows_clone);
        self.tasks_view.set_pending_focus(target);
        // Force a rebuild so the newly-opened path is visible and the
        // cursor moves before the user does anything else.
        self.refresh_task_table();
    }

    /// `:reload-tasks` — synchronously refetch task rows so subsequent
    /// commands in the same `execute_cmdline` chain (e.g. `:focus-task`)
    /// see external CLI mutations. Works from any tab; the user might be
    /// in Taiga and `:jump`+`:focus-task` is queued behind this command.
    /// Silent on success; modal-error on failure.
    fn reload_tasks_command(&mut self) {
        let expr = if let Some(ref filter) = self.tasks_view.active_filter {
            filter.clone()
        } else {
            filter_builder::build(&self.tasks_view.state.filter).expr
        };
        let options = self.tasks_view.active_filter_options.clone();
        let service = Arc::clone(&self.task_service);
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                service.list_filtered_with_options(&expr, &options).await
            })
        });
        match result {
            Ok(tasks) => {
                let ids: Vec<Uuid> = tasks.iter().map(|t| t.id).collect();
                self.tasks_view.state.set_tasks(tasks);
                self.refresh_task_table();
                self.spawn_load_task_tags(ids);
            }
            Err(e) => {
                let msg = e.to_string();
                not_yet_done_content::http_log::log_error("tasks_reload", &msg);
                self.last_error = Some(msg.clone());
                self.modal_message = Some(format!(":reload-tasks — {msg}"));
            }
        }
    }

    /// `:focus-node [-i] <Tab>[:<view>] /<col>|<pattern>[/...]` — switch
    /// to the named content tab/sub-view and park the cursor on the first
    /// row whose `col` matches `pattern`. Without an explicit column
    /// hint, the pattern is matched against `label` plus all metadata
    /// values. Use `re:` to opt into regex (e.g. `re:\b151\b`); `-i`
    /// switches both substring and regex matching to case-insensitive.
    ///
    /// Modal error when:
    ///   - the target tab is unknown or is not a content tab
    ///   - the named view is unknown for that tab
    ///   - the path is empty / not `/`-rooted
    ///   - any segment has a malformed `re:` regex
    ///   - the requested column doesn't exist for any visible row
    ///   - no row matches, or more than one row matches
    ///   - the path has more than one segment (drill-down not yet supported)
    fn focus_node_command(&mut self, raw_args: &str) {
        let mut case_insensitive = false;
        let mut rest = raw_args.trim_start();
        loop {
            let Some(tok) = rest.split_whitespace().next() else {
                break;
            };
            if !tok.starts_with('-') {
                break;
            }
            match tok {
                "-i" => {
                    case_insensitive = true;
                    rest = rest[tok.len()..].trim_start();
                }
                other => {
                    self.modal_message = Some(format!(
                        ":focus-node — unknown flag '{other}' (only -i is supported)"
                    ));
                    return;
                }
            }
        }

        let mut parts = rest.splitn(2, char::is_whitespace);
        let target = parts.next().unwrap_or("").trim();
        let path = parts.next().unwrap_or("").trim();
        if target.is_empty() || path.is_empty() {
            self.modal_message = Some(
                ":focus-node expects <Tab>[:<view>] /col|pattern, e.g. \
                :focus-node Taiga:items /ref|acme#42"
                    .to_string(),
            );
            return;
        }

        let (tab_name, view_name) = match target.split_once(':') {
            Some((t, v)) => (t.trim(), Some(v.trim())),
            None => (target, None),
        };
        if tab_name.is_empty() {
            self.modal_message = Some(":focus-node — empty tab name".to_string());
            return;
        }

        let tab_idx = self
            .content_views
            .iter()
            .position(|slot| slot.tab_name().eq_ignore_ascii_case(tab_name));
        let Some(tab_idx) = tab_idx else {
            self.modal_message = Some(format!(
                ":focus-node — '{tab_name}' is not a content tab (Taiga/Jira/Postgres/…)"
            ));
            return;
        };

        self.set_active_tab(Tab::Content(tab_idx));

        // After set_active_tab the active tab is Content(tab_idx); resolve
        // the working ContentView slot and switch its subtab if asked.
        let cv = match &mut self.content_views[tab_idx] {
            ContentSlot::Working(cv) => cv,
            ContentSlot::Broken { name, errors, .. } => {
                self.modal_message = Some(format!(
                    ":focus-node — tab '{name}' is in an error state: {}",
                    errors.first().cloned().unwrap_or_default()
                ));
                return;
            }
        };
        if let Some(v) = view_name {
            match cv.switch_to_view_by_name(v) {
                Ok(_load_needed) => {}
                Err(available) => {
                    self.modal_message = Some(format!(
                        ":focus-node — unknown view '{v}' for tab '{tab_name}' (available: {})",
                        available.join(", ")
                    ));
                    return;
                }
            }
        }

        let segments = match crate::views::focus_node::parse_path(path, case_insensitive) {
            Ok(s) => s,
            Err(e) => {
                self.modal_message = Some(format_focus_error(&e));
                return;
            }
        };

        if let Err(e) = cv.focus_node_in_active_pane(&segments) {
            self.modal_message = Some(format_focus_error(&e));
        }
    }

    /// `:tree-find <Tab>[:<view>] <query>` — the tree-mode sibling of
    /// `:focus-node`. Switches to the named content tab/sub-view, forces
    /// a fresh reload (so out-of-process CLI mutations are in the
    /// adapter's snapshot before the search runs), then drives a
    /// server-side tree search and lazily expands to the first hit,
    /// parking the cursor on it. Unlike `:focus-node` (synchronous, flat,
    /// single-segment) this is asynchronous and walks the lazy-loaded
    /// tree — the natural target for jumping into the adapterized Tasks
    /// tab, whose ticket nodes sit several levels deep.
    ///
    /// The tab name may be double-quoted to allow spaces, e.g.
    /// `:tree-find "Tasks (A)" id:<uuid>`. The query is adapter-defined;
    /// the local task adapter additionally accepts an exact-id escape
    /// `id:<uuid>` (used by scripted jumps that already resolved the
    /// node id via the CLI).
    ///
    /// Modal error when:
    ///   - the target tab is unknown or not a content tab
    ///   - the named view is unknown for that tab
    ///   - the active view isn't a tree (use `:focus-node` for flat views)
    fn tree_find_command(&mut self, raw_args: &str) {
        let (target, query) = split_leading_token(raw_args.trim());
        let query = query.trim().to_string();
        if target.is_empty() || query.is_empty() {
            self.modal_message = Some(
                ":tree-find expects <Tab>[:<view>] <query>, e.g. \
                 :tree-find \"Tasks (A)\" id:<uuid>"
                    .to_string(),
            );
            return;
        }
        let (tab_name, view_name) = match target.split_once(':') {
            Some((t, v)) => (t.trim().to_string(), Some(v.trim().to_string())),
            None => (target, None),
        };
        if tab_name.is_empty() {
            self.modal_message = Some(":tree-find — empty tab name".to_string());
            return;
        }

        let Some(tab_idx) = self
            .content_views
            .iter()
            .position(|slot| slot.tab_name().eq_ignore_ascii_case(&tab_name))
        else {
            self.modal_message = Some(format!(
                ":tree-find — '{tab_name}' is not a content tab (Taiga/Jira/Tasks/…)"
            ));
            return;
        };

        self.set_active_tab(Tab::Content(tab_idx));

        let pane_id = {
            let cv = match &mut self.content_views[tab_idx] {
                ContentSlot::Working(cv) => cv,
                ContentSlot::Broken { name, errors, .. } => {
                    self.modal_message = Some(format!(
                        ":tree-find — tab '{name}' is in an error state: {}",
                        errors.first().cloned().unwrap_or_default()
                    ));
                    return;
                }
            };
            if let Some(v) = view_name {
                if let Err(available) = cv.switch_to_view_by_name(&v) {
                    self.modal_message = Some(format!(
                        ":tree-find — unknown view '{v}' for tab '{tab_name}' (available: {})",
                        available.join(", ")
                    ));
                    return;
                }
            }
            if !cv.active_view_is_tree() {
                self.modal_message = Some(
                    ":tree-find — the active view isn't a tree \
                     (use :focus-node for flat views)"
                        .to_string(),
                );
                return;
            }
            let pane_id = cv.active_pane_id();
            cv.active_pane_mut().queue_pending_tree_find(query);
            pane_id
        };

        // Force a fresh reload; the queued query fires when the load
        // lands (see the `LoadMsg::ContentItems` handler), so the search
        // runs against an up-to-date snapshot — parity with the legacy
        // `:reload-tasks` that preceded `:focus-task`.
        self.spawn_content_load(tab_idx, pane_id);
    }

    /// `:query apply [-t <Tab>[:<view>]] <name>` — activate the saved
    /// query `<name>` on a content tab, synchronously reload so a
    /// subsequent command (e.g. `:focus-node`) in the same command list
    /// sees the new rows. `-t` is optional: without it the currently
    /// active content tab is used; with it the named tab/sub-view is
    /// switched to first.
    ///
    /// `<name>` may contain whitespace and is matched case-insensitively
    /// against the merged YAML+DB saved-query list of the active view.
    ///
    /// Modal error when:
    ///   - `-t` is missing and the active tab is not a content tab
    ///   - the named tab is unknown or not a content tab
    ///   - the named view is unknown for that tab
    ///   - no saved query matches `<name>` in the active view
    ///   - the synchronous reload returns an adapter error
    fn query_apply_command(&mut self, raw_args: &str) {
        // ── 1. Parse `[--var k=v]* [-t <Tab>[:<view>]] <name>` ──────────
        let (vars_prefilled, target, name_str) =
            match parse_query_apply_args(raw_args) {
                Ok(parsed) => parsed,
                Err(msg) => {
                    self.modal_message = Some(format!(":query apply — {msg}"));
                    return;
                }
            };
        if name_str.is_empty() {
            self.modal_message = Some(
                ":query apply expects [--var k=v]* [-t <Tab>[:<view>]] <name>".to_string(),
            );
            return;
        }

        // ── 2. Resolve target tab + view ─────────────────────────────────
        if let Some((tab_name, view_name)) = target {
            let tab_idx = self
                .content_views
                .iter()
                .position(|slot| slot.tab_name().eq_ignore_ascii_case(&tab_name));
            let Some(tab_idx) = tab_idx else {
                self.modal_message = Some(format!(
                    ":query apply — '{tab_name}' is not a content tab"
                ));
                return;
            };
            self.set_active_tab(Tab::Content(tab_idx));
            if let Some(v) = view_name {
                let cv = match &mut self.content_views[tab_idx] {
                    ContentSlot::Working(cv) => cv,
                    ContentSlot::Broken { name, errors, .. } => {
                        self.modal_message = Some(format!(
                            ":query apply — tab '{name}' is in an error state: {}",
                            errors.first().cloned().unwrap_or_default()
                        ));
                        return;
                    }
                };
                if let Err(available) = cv.switch_to_view_by_name(&v) {
                    self.modal_message = Some(format!(
                        ":query apply — unknown view '{v}' for tab '{tab_name}' (available: {})",
                        available.join(", ")
                    ));
                    return;
                }
            }
        }
        let Tab::Content(tab_idx) = self.active_tab else {
            self.modal_message = Some(
                ":query apply — not on a content tab (use -t <Tab>[:<view>] to target one)"
                    .to_string(),
            );
            return;
        };

        // ── 3. Pull fresh DB saved queries before lookup ─────────────────
        self.reload_content_saved_queries(tab_idx);

        // ── 4. Look up saved query + pane, hand off to dispatcher ───────
        let (raw_query, saved_name, pane_id) = {
            let cv = match &mut self.content_views[tab_idx] {
                ContentSlot::Working(cv) => cv,
                ContentSlot::Broken { name, errors, .. } => {
                    self.modal_message = Some(format!(
                        ":query apply — tab '{name}' is in an error state: {}",
                        errors.first().cloned().unwrap_or_default()
                    ));
                    return;
                }
            };
            let Some(sq) = cv
                .db_saved_queries
                .iter()
                .find(|s| s.name.eq_ignore_ascii_case(&name_str))
                .cloned()
            else {
                let available: Vec<String> =
                    cv.db_saved_queries.iter().map(|s| s.name.clone()).collect();
                let hint = if available.is_empty() {
                    "no saved queries on this view".to_string()
                } else {
                    format!("available: {}", available.join(", "))
                };
                self.modal_message = Some(format!(
                    ":query apply — no saved query named '{name_str}' ({hint})"
                ));
                return;
            };
            (sq.query, sq.name, cv.active_pane_id())
        };

        let target = crate::components::query_var_popup::QueryVarPopupTarget {
            tab_idx,
            pane_id,
            raw_query,
            saved_name: Some(saved_name),
        };
        // CLI path: only popup when required vars are missing. Scripts
        // that pre-fill all `--var` flags get a popup-free apply.
        self.start_query_apply(target, vars_prefilled, false);
    }

    /// `:query edit <name>` — open the body file for `<name>` in the
    /// adapter's saved-query store in the external editor. Operates
    /// on the currently active content tab. Modal-errors when the
    /// active tab isn't a content tab, the adapter doesn't expose a
    /// filesystem-backed store, or the query doesn't exist.
    fn query_edit_command(&mut self, name: &str) {
        let view_index = match self.current_content_view_index_or_modal("edit") {
            Some(idx) => idx,
            None => return,
        };
        let Some(path) = self.saved_query_path_or_modal("edit", view_index, name) else {
            return;
        };
        if !path.exists() {
            self.modal_message = Some(format!(
                ":query edit — no saved query named '{name}' (use :query new to create)"
            ));
            return;
        }
        match crate::edit_session::SavedQueryEditSession::open(
            path.clone(),
            view_index,
            name.to_string(),
        ) {
            Ok(session) => {
                let _ = self.open_session(Box::new(session));
            }
            Err(e) => {
                self.notify_error(format!("Cannot open {}: {e}", path.display()));
            }
        }
    }

    /// `:query new <name>` — open the external editor on an empty
    /// buffer; first commit creates the body file in the adapter's
    /// saved-query store. Operates on the active content tab.
    fn query_new_command(&mut self, name: &str) {
        let view_index = match self.current_content_view_index_or_modal("new") {
            Some(idx) => idx,
            None => return,
        };
        let Some(path) = self.saved_query_path_or_modal("new", view_index, name) else {
            return;
        };
        if path.exists() {
            self.modal_message = Some(format!(
                ":query new — '{name}' already exists (use :query edit to modify)"
            ));
            return;
        }
        let session = crate::edit_session::SavedQueryEditSession::new(
            path,
            view_index,
            name.to_string(),
        );
        let _ = self.open_session(Box::new(session));
    }

    /// `:query delete <name>` — remove the body from the adapter's
    /// store and the shortcut row from the DB. Idempotent: silently
    /// no-ops when the entry is already gone. Operates on the active
    /// content tab.
    fn query_delete_command(&mut self, name: &str) {
        let view_index = match self.current_content_view_index_or_modal("delete") {
            Some(idx) => idx,
            None => return,
        };
        let scope = match self.content_view(view_index).map(|cv| cv.query_scope.clone()) {
            Some(s) => s,
            None => {
                self.modal_message =
                    Some(":query delete — active tab has no scope".to_string());
                return;
            }
        };
        self.delete_content_query(view_index, &scope, name);
        self.reload_content_saved_queries(view_index);
        self.notify(format!("Deleted saved query '{name}'"));
    }

    /// Return the active content tab's slot index, or set
    /// `modal_message` and return `None` when the active tab isn't a
    /// content tab / is in an error state.
    fn current_content_view_index_or_modal(&mut self, sub: &str) -> Option<usize> {
        let Tab::Content(tab_idx) = self.active_tab else {
            self.modal_message = Some(format!(
                ":query {sub} — not on a content tab"
            ));
            return None;
        };
        match &self.content_views[tab_idx] {
            ContentSlot::Working(_) => Some(tab_idx),
            ContentSlot::Broken { name, errors, .. } => {
                self.modal_message = Some(format!(
                    ":query {sub} — tab '{name}' is in an error state: {}",
                    errors.first().cloned().unwrap_or_default()
                ));
                None
            }
        }
    }

    /// Look up the on-disk path for saved-query `<name>` in the active
    /// content view's adapter store. Returns `None` (and sets
    /// `modal_message`) when the adapter exposes no store, or its store
    /// returns `None` from `path()` (opaque storage).
    fn saved_query_path_or_modal(
        &mut self,
        sub: &str,
        view_index: usize,
        name: &str,
    ) -> Option<std::path::PathBuf> {
        let cv = self.content_view(view_index)?;
        let Some(adapter) = cv.adapter.as_ref() else {
            self.modal_message =
                Some(format!(":query {sub} — this tab has no adapter"));
            return None;
        };
        let Some(store) = adapter.saved_query_store() else {
            self.modal_message = Some(format!(
                ":query {sub} — adapter '{}' has no saved-query store",
                adapter.adapter_type()
            ));
            return None;
        };
        let Some(path) = store.path(name) else {
            self.modal_message = Some(format!(
                ":query {sub} — adapter '{}' stores queries opaquely (no file path)",
                adapter.adapter_type()
            ));
            return None;
        };
        Some(path)
    }

    /// Decide whether a saved-query apply needs the variable input
    /// popup or can run directly. `force_popup` is set by interactive
    /// entry points (YAML shortcut, query menu Apply) per the
    /// architecture decision "Shortcut → immer Popup".
    pub fn start_query_apply(
        &mut self,
        target: crate::components::query_var_popup::QueryVarPopupTarget,
        prefilled: std::collections::HashMap<String, String>,
        force_popup: bool,
    ) {
        let cv = match self.content_view(target.tab_idx) {
            Some(cv) => cv,
            None => {
                self.modal_message = Some(":query apply — invalid tab".to_string());
                return;
            }
        };
        let adapter = match cv.adapter.as_ref() {
            Some(a) => Arc::clone(a),
            None => {
                self.modal_message = Some(":query apply — this tab has no adapter".to_string());
                return;
            }
        };
        let vars = adapter.query_variables(&target.raw_query);
        let any_required_missing = vars.iter().any(|v| {
            v.default.is_none() && !prefilled.contains_key(&v.name)
        });
        let needs_popup =
            !vars.is_empty() && (force_popup || any_required_missing);
        if !needs_popup {
            self.apply_query_with_vars(target, prefilled);
            return;
        }
        let title = match &target.saved_name {
            Some(n) => format!("Query: {n}"),
            None => "Query variables".to_string(),
        };
        self.query_var_popup = Some(
            crate::components::query_var_popup::QueryVarPopup::new(
                Arc::clone(&self.shared_theme),
                title,
                target,
                vars,
                prefilled,
            ),
        );
    }

    /// Apply a saved query with the given variable bindings: set the
    /// pane's query+vars, then synchronously run the load and update
    /// the pane (same body as the legacy `:query apply` path).
    pub fn apply_query_with_vars(
        &mut self,
        target: crate::components::query_var_popup::QueryVarPopupTarget,
        vars: std::collections::HashMap<String, String>,
    ) {
        let tab_idx = target.tab_idx;
        let pane_id = target.pane_id;

        let (adapter, load_req, group_by) = {
            let cv = match &mut self.content_views[tab_idx] {
                ContentSlot::Working(cv) => cv,
                ContentSlot::Broken { name, errors, .. } => {
                    self.modal_message = Some(format!(
                        ":query apply — tab '{name}' is in an error state: {}",
                        errors.first().cloned().unwrap_or_default()
                    ));
                    return;
                }
            };
            cv.set_query_for_pane_with_vars(
                pane_id,
                target.raw_query.clone(),
                target.saved_name.clone(),
                vars.clone(),
            );
            let adapter = match cv.adapter.as_ref() {
                Some(a) => Arc::clone(a),
                None => {
                    self.modal_message =
                        Some(":query apply — this tab has no adapter".to_string());
                    return;
                }
            };
            let Some(pane) = cv.find_pane(pane_id) else {
                self.modal_message =
                    Some(":query apply — could not build a load request".to_string());
                return;
            };
            let Some(req) = pane.root_load_request(&cv.view_defs) else {
                self.modal_message =
                    Some(":query apply — could not build a load request".to_string());
                return;
            };
            // Adapter-grouped tree: keep the pane's grouping across a
            // query apply, same as `spawn_content_load`.
            (adapter, req, pane.adapter_group_spec(&cv.view_defs))
        };

        let crate::views::content_view::LoadRequest {
            node_type_id,
            query,
            sort,
            page,
            vars: req_vars,
        } = load_req;
        let query = query.map(|raw| adapter.render_query(&raw, &req_vars));
        let result: Result<_, String> = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let root = adapter.root().await.map_err(|e| e.to_string())?;
                let node_type = root
                    .children_types()
                    .into_iter()
                    .find(|t| t.type_id == node_type_id)
                    .ok_or_else(|| format!("Node type '{node_type_id}' not found"))?;
                let sortable_columns = root.sortable_columns(&node_type);
                let params = not_yet_done_content::ListParams {
                    node_type,
                    query,
                    sort,
                    page,
                    download: false,
                    group_by,
                };
                let list = root.list(params).await.map_err(|e| e.to_string())?;
                Ok((list, sortable_columns))
            })
        });

        match result {
            Ok((list, sortable_columns)) => {
                if let Some(cv) = self.content_view_mut(tab_idx) {
                    cv.set_items_for_pane(
                        pane_id,
                        list.items,
                        list.applied_sort,
                        list.page,
                        sortable_columns,
                        None,
                    );
                }
                // `set_query_*` re-armed the pane's `expand_depth`
                // cascade; the async load path pumps it from
                // `LoadMsg::ContentItems`, but this synchronous apply
                // bypasses that — drive it here so the filtered tree
                // unfolds just like a fresh load.
                self.drive_tree_auto_expand(tab_idx, pane_id);
            }
            Err(e) => {
                not_yet_done_content::http_log::log_error("query_apply", &e);
                self.last_error = Some(e.clone());
                if let Some(cv) = self.content_view_mut(tab_idx) {
                    cv.set_items_for_pane(
                        pane_id,
                        vec![],
                        Vec::new(),
                        None,
                        Vec::new(),
                        Some(e.clone()),
                    );
                }
                self.modal_message = Some(format!(":query apply — {e}"));
            }
        }
    }
}

/// Parse the arguments to `:query apply`. Returns the prefilled vars
/// map, the optional `-t <Tab>[:<view>]` target, and the saved-query
/// name. Flags can appear in any order before the name; the name is
/// the remainder after the last flag.
fn parse_query_apply_args(
    raw: &str,
) -> Result<
    (
        std::collections::HashMap<String, String>,
        Option<(String, Option<String>)>,
        String,
    ),
    String,
> {
    let mut vars: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut target: Option<(String, Option<String>)> = None;
    let mut rest = raw.trim();
    loop {
        if let Some(after) = rest.strip_prefix("--var") {
            let after = after.trim_start();
            let mut parts = after.splitn(2, char::is_whitespace);
            let pair = parts.next().unwrap_or("").trim();
            let tail = parts.next().unwrap_or("").trim_start();
            if pair.is_empty() {
                return Err("--var expects k=v".into());
            }
            let (k, v) = pair
                .split_once('=')
                .ok_or_else(|| "--var expects k=v".to_string())?;
            let k = k.trim();
            if k.is_empty() {
                return Err("--var key is empty".into());
            }
            vars.insert(k.to_string(), v.to_string());
            rest = tail;
            continue;
        }
        if let Some(after) = rest.strip_prefix("-t") {
            let after = after.trim_start();
            let mut parts = after.splitn(2, char::is_whitespace);
            let tgt = parts.next().unwrap_or("").trim();
            let tail = parts.next().unwrap_or("").trim_start();
            if tgt.is_empty() {
                return Err("-t expects <Tab>[:<view>]".into());
            }
            let (tab, view) = match tgt.split_once(':') {
                Some((t, v)) => (t.trim().to_string(), Some(v.trim().to_string())),
                None => (tgt.to_string(), None),
            };
            target = Some((tab, view));
            rest = tail;
            continue;
        }
        break;
    }
    Ok((vars, target, rest.to_string()))
}

/// Split off a leading token from `s`, honouring double quotes so a
/// token may itself contain spaces — e.g. `"Tasks (A)" id:42` →
/// (`Tasks (A)`, `id:42`). Unquoted tokens split on the first
/// whitespace run. Used by `:tree-find` to address a tab whose display
/// name contains spaces. Returns (token, remainder).
fn split_leading_token(s: &str) -> (String, &str) {
    let s = s.trim_start();
    if let Some(rest) = s.strip_prefix('"') {
        return match rest.find('"') {
            Some(end) => (rest[..end].to_string(), rest[end + 1..].trim_start()),
            // Unterminated quote: take the rest as the whole token.
            None => (rest.to_string(), ""),
        };
    }
    match s.split_once(char::is_whitespace) {
        Some((tok, rest)) => (tok.to_string(), rest.trim_start()),
        None => (s.to_string(), ""),
    }
}

fn format_focus_error(e: &crate::views::focus_node::FocusError) -> String {
    use crate::views::focus_node::FocusError::*;
    match e {
        MissingLeadingSlash => {
            ":focus-node expects a /-rooted path (e.g. /ref|acme#42)".to_string()
        }
        EmptyPath => ":focus-node — path is empty".to_string(),
        BadRegex { seg, msg } => format!(":focus-node — bad regex in '{seg}': {msg}"),
        NotFound { seg } => format!(":focus-node — no row matching '{seg}'"),
        Ambiguous { seg, preview } => {
            let ids = preview
                .iter()
                .map(|i| format!("'{i}'"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(":focus-node — '{seg}' is ambiguous: {ids}")
        }
        UnknownColumn { col, available } => format!(
            ":focus-node — unknown column '{col}' (available: {})",
            available.join(", ")
        ),
        MultiSegmentUnsupported => {
            ":focus-node — multi-segment drill-down paths are not yet supported".to_string()
        }
    }
}

impl App {
    /// Open the most recently captured error in `$EDITOR` (read-only).
    /// Falls back to a notification when no error has been recorded yet.
    fn open_last_error_editor(&mut self) -> EditorRequest {
        let Some(text) = self.last_error.clone() else {
            self.notify("No error has occurred yet".to_string());
            return EditorRequest::None;
        };
        let scope = match self.active_tab {
            Tab::Tasks => crate::edit_session::SessionScope::Tasks,
            Tab::Trackings => crate::edit_session::SessionScope::Trackings,
            Tab::Content(_) => crate::edit_session::SessionScope::Content,
        };
        let session = crate::edit_session::ErrorViewSession::new(text, scope);
        self.open_session(Box::new(session))
    }

    // ── Trackings tab key handling ──────────────────────────────────

    fn handle_trackings_fuzzy_key(&mut self, key: &str) -> bool {
        let needs_rebuild;
        if self.keybindings.common.bindings.get(&CommonAction::FuzzyFilterAccept).map_or(false, |b| b.matches(key)) {
            self.trackings_view.state.fuzzy_close();
            needs_rebuild = true;
        } else if self.keybindings.common.bindings.get(&CommonAction::FuzzyFilterCancel).map_or(false, |b| b.matches(key)) {
            if self.trackings_view.state.fuzzy_query.is_empty() {
                self.trackings_view.state.fuzzy_close();
            } else {
                self.trackings_view.state.fuzzy_query.clear();
                self.trackings_view.state.fuzzy_cursor = 0;
                self.trackings_view.state.refilter();
            }
            needs_rebuild = true;
        } else if self.keybindings.common.bindings.get(&CommonAction::FuzzyFilterClear).map_or(false, |b| b.matches(key)) {
            self.trackings_view.state.fuzzy_query.clear();
            self.trackings_view.state.fuzzy_cursor = 0;
            self.trackings_view.state.refilter();
            needs_rebuild = true;
        } else {
            needs_rebuild = match key {
                "backspace" => { self.trackings_view.state.fuzzy_backspace(); true }
                "left" => { self.trackings_view.state.fuzzy_cursor_left(); false }
                "right" => { self.trackings_view.state.fuzzy_cursor_right(); false }
                ch if ch.chars().count() == 1 && !ch.chars().next().unwrap().is_control() => {
                    self.trackings_view.state.fuzzy_insert(ch.chars().next().unwrap());
                    true
                }
                _ => return false,
            };
        }
        if needs_rebuild {
            if self.trackings_view.state.sub_view == crate::tabs::TrackingsSubView::Tree {
                if let Some(ref forest) = self.tasks_view.state.forest {
                    self.trackings_view.state.rebuild_tree_rows(forest);
                }
            }
            self.rebuild_trackings_table();
        }
        true
    }

    fn set_active_tab(&mut self, tab: Tab) {
        // Sort-hint mode is bound to the previously active view; cancel
        // on tab switch so we don't strand the user in a tab-mismatched
        // popup.
        if self.sort_hint_phase.is_active() {
            self.cancel_sort_hint_mode();
        }
        self.active_tab = tab;
        if tab == Tab::Trackings {
            self.spawn_load_trackings();
        }
        if tab == Tab::Tasks && self.tasks_view.state.load_state == LoadState::Idle {
            self.spawn_load();
        }
        if let Tab::Content(idx) = tab {
            if let Some(cv) = self.content_view(idx) {
                // Cheap staleness probe: adapters over stores that change
                // outside the process (local task/tracking DB written by
                // the CLI or waybar) diff their cache against the backend
                // and emit `Invalidation::All` on drift, so the tab shows
                // e.g. an externally started tracking on switch. No-op
                // for everyone else.
                if let Some(adapter) = cv.adapter.as_ref() {
                    let adapter = Arc::clone(adapter);
                    tokio::spawn(async move { adapter.revalidate().await });
                }
                let status = cv.auth_status.clone();
                self.react_to_adapter_status(idx, &status);
            }
        }
    }

    /// Recompute the tab layout from the current config + content views.
    /// Call after any reload that can change the tab set (tui.yaml, view
    /// add/remove, rename). Snaps the active tab to the first visible one
    /// if the current tab dropped out of the layout, and surfaces a
    /// duplicate-name hard error as a modal.
    pub(crate) fn rebuild_tab_layout(&mut self) {
        let (layout, err) = build_tab_layout(&self.config.tabs, &self.content_views);
        self.tab_layout = layout;
        if !self.tab_layout.contains(self.active_tab) {
            self.active_tab = self.tab_layout.first();
        }
        if let Some(e) = err {
            self.modal_message = Some(format!("Tab configuration error:\n\n{e}"));
        }
    }

    /// Build the main-tab label list from the active [`TabLayout`]:
    /// visible tabs in order, each as `icon name key`. The key hint is
    /// the autonumber digit in constellation mode, or the legacy fixed
    /// `GlobalAction` key otherwise (so pre-constellation labels render
    /// byte-for-byte as before).
    fn build_main_tab_labels(&self) -> Vec<(Tab, String)> {
        let gkb = &self.keybindings.global;
        let autonumber = self.tab_layout.autonumber();
        self.tab_layout
            .tabs()
            .iter()
            .map(|&tab| {
                let key = if autonumber {
                    self.tab_layout
                        .digit_for(tab)
                        .map(|c| c.to_string())
                        .unwrap_or_default()
                } else {
                    match tab {
                        Tab::Tasks => gkb.label(&GlobalAction::TabTasks),
                        Tab::Trackings => gkb.label(&GlobalAction::TabTrackings),
                        Tab::Content(0) => gkb.label(&GlobalAction::TabJira),
                        Tab::Content(1) => gkb.label(&GlobalAction::TabTaiga),
                        Tab::Content(2) => gkb.label(&GlobalAction::TabPostgres),
                        Tab::Content(3) => gkb.label(&GlobalAction::TabConfluence),
                        Tab::Content(_) => String::new(),
                    }
                };
                let label = match tab {
                    Tab::Tasks => format!("󰝖 Tasks {key}").trim_end().to_string(),
                    Tab::Trackings => format!("󱦗 Trackings {key}").trim_end().to_string(),
                    Tab::Content(idx) => {
                        let (name, icon) = self
                            .content_views
                            .get(idx)
                            .map(|s| {
                                (
                                    s.tab_name().to_string(),
                                    s.tab_icon().unwrap_or_default().to_string(),
                                )
                            })
                            .unwrap_or_default();
                        format!("{icon} {name} {key}").trim().to_string()
                    }
                };
                (tab, label)
            })
            .collect()
    }

    /// Open / close the adapter credentials popup based on a fresh status.
    /// Only acts when the status is for the currently active content tab.
    fn react_to_adapter_status(
        &mut self,
        view_index: usize,
        status: &not_yet_done_content::AdapterStatus,
    ) {
        let Tab::Content(active) = self.active_tab else {
            return;
        };
        if active != view_index {
            return;
        }
        match status {
            not_yet_done_content::AdapterStatus::NeedsCreds { fields } => {
                let already_open = self
                    .adapter_creds_popup
                    .as_ref()
                    .is_some_and(|p| p.view_index() == view_index && p.is_open());
                if !already_open {
                    let title = self
                        .content_view(view_index)
                        .map(|cv| format!("Login: {}", cv.tab_name))
                        .unwrap_or_else(|| "Login".into());
                    self.adapter_creds_popup = Some(
                        crate::components::adapter_creds_popup::AdapterCredsPopup::new(
                            Arc::clone(&self.shared_theme),
                            title,
                            view_index,
                            fields.clone(),
                        ),
                    );
                }
            }
            not_yet_done_content::AdapterStatus::Ready => {
                if let Some(popup) = self.adapter_creds_popup.as_mut() {
                    if popup.view_index() == view_index {
                        popup.close();
                    }
                }
                if self
                    .adapter_creds_popup
                    .as_ref()
                    .is_some_and(|p| !p.is_open())
                {
                    self.adapter_creds_popup = None;
                }
            }
            _ => {}
        }
    }

    pub fn set_query_error(&mut self, err: Option<String>) {
        if let Some(msg) = err.as_ref() {
            not_yet_done_content::http_log::log_error("query_error", msg);
            self.last_error = Some(msg.clone());
        }
        self.query_error_bar.set_error(err.clone());
        self.query_error = err;
    }

    /// Push an error to the notification bar and remember it as the
    /// "last error" so `GlobalAction::ShowLastError` can reopen it in
    /// `$EDITOR`. Use for any failure surfaced to the user — DB writes,
    /// adapter calls, script launches, etc. Plain informational messages
    /// should keep using `notify`.
    pub fn notify_error(&mut self, message: String) {
        not_yet_done_content::http_log::log_error("notify", &message);
        self.last_error = Some(message.clone());
        self.notification_bar.push(message);
    }

    /// Sync all component state from App. Called once after each dispatch.
    pub fn sync_components(&mut self) {
        // Push the current sort-hint state onto the per-view header overlay
        // BEFORE the table refreshes — refresh reads `header_overlay`.
        self.update_header_overlays();

        // Refresh task table if data or filter may have changed.
        if self.active_tab == Tab::Tasks {
            self.refresh_task_table();
        }
        // Content tabs need their table rebuilt so the header reflects
        // the current overlay (Tasks rebuilds above).
        if let Tab::Content(idx) = self.active_tab {
            if let Some(cv) = self.content_view_mut(idx) {
                cv.rebuild_table();
            }
        }

        self.tab_bar.set_active_tab(self.active_tab);
        let main_tab_labels = self.build_main_tab_labels();
        self.tab_bar.set_main_tab_labels(main_tab_labels);
        self.tab_bar.set_tasks_sub_view(self.tasks_view.sub_view());
        self.tab_bar.set_trackings_sub_view(self.trackings_view.state.sub_view);
        let subtab_labels: Vec<(usize, Vec<(String, bool)>)> = self
            .content_views_indexed()
            .map(|(idx, cv)| (idx, cv.subtab_labels()))
            .collect();
        for (idx, labels) in subtab_labels {
            self.tab_bar.set_content_sub_tabs(idx, labels);
        }

        // Action bar lives on each view; push state in.
        let tracking_active = !self.tracked_ids.is_empty();
        let session = self.pending_session.as_ref();
        let session_label = |scope: crate::edit_session::SessionScope| -> Option<&str> {
            session.filter(|s| s.scope() == scope).map(|s| s.label())
        };
        let tasks_active_editor = session_label(crate::edit_session::SessionScope::Tasks);
        let trackings_active_editor = session_label(crate::edit_session::SessionScope::Trackings)
            .or_else(|| if self.detached_script.is_some() { Some("run") } else { None });
        let content_active_editor = session_label(crate::edit_session::SessionScope::Content)
            .map(|s| s.to_string());
        self.tasks_view.sync_action_bar(tasks_active_editor, tracking_active);
        self.trackings_view.sync_action_bar(trackings_active_editor, tracking_active);
        if let Tab::Content(idx) = self.active_tab {
            if let Some(cv) = self.content_view_mut(idx) {
                cv.sync_action_bar(content_active_editor.as_deref(), tracking_active);
            }
        }

        if let Tab::Content(idx) = self.active_tab {
            // Content tabs use dynamic hints from ContentView.
            let gkb = &self.keybindings.global;
            let mut hints: Vec<(String, String)> = vec![
                (gkb.label(&GlobalAction::Quit), "quit".to_string()),
            ];
            if let Some(cv) = self.content_view(idx) {
                for (k, v) in cv.status_bar_hints() {
                    hints.push((k, v));
                }
            }
            hints.push((
                self.keybindings.common.label(&CommonAction::SortMode),
                "sort".to_string(),
            ));
            hints.push((
                format!("{}/{}", gkb.label(&GlobalAction::TabNext), gkb.label(&GlobalAction::TabPrev)),
                "cycle tabs".to_string(),
            ));
            self.status_bar.set_custom_hints(hints);
        } else {
            let status_mode = if self.active_tab == Tab::Tasks && self.tasks_view.state.form_visible() {
                StatusMode::TasksFormOpen
            } else if self.active_tab == Tab::Tasks {
                StatusMode::TasksNormal
            } else if self.active_tab == Tab::Trackings {
                StatusMode::Trackings
            } else {
                StatusMode::Other
            };
            self.status_bar.set_mode(status_mode, &self.keybindings);
        }

        // Marker pill: link-mark always wins (existing UX), then the
        // DSF-4 DB-script-move source, then the M7/E6 generic move
        // clipboard — each with a distinct prefix so the user can tell
        // the states apart.
        let marker = match (
            &self.marked_link,
            &self.marked_db_script_for_move,
            &self.content_marked_node,
        ) {
            (Some(r), _, _) => Some(r.as_str().to_string()),
            (None, Some(n), _) => Some(format!("move: {n}")),
            (None, None, Some(m)) => Some(format!("move: {}", m.label)),
            (None, None, None) => None,
        };
        self.status_bar.set_link_marker(marker);
    }

    /// Rebuild the trackings table widget — delegates to TrackingsView.
    pub fn rebuild_trackings_table(&mut self) {
        self.trackings_view.rebuild_table();
    }

    fn process_sub_view_message(&mut self, msg: SubViewMessage) -> EditorRequest {
        match msg {
            SubViewMessage::Request(req) => self.process_view_request(req),
            SubViewMessage::SelectionChanged(_) => EditorRequest::None,
            SubViewMessage::RefreshRequested => {
                self.refresh_task_table();
                EditorRequest::None
            }
            SubViewMessage::FuzzyStateChanged { .. } => {
                // Refresh after filter change.
                self.refresh_task_table();
                EditorRequest::None
            }
            SubViewMessage::SearchStateChanged { .. } => {
                // Search state now lives on views; handled via Searchable trait.
                EditorRequest::None
            }
            SubViewMessage::EditorOpened(_) | SubViewMessage::EditorClosed => EditorRequest::None,
            SubViewMessage::ActionBarHints(_) | SubViewMessage::StatusBarHints(_) => EditorRequest::None,
            SubViewMessage::ContentDrill { .. } => {
                // ContentDrill is internal to ContentView — it should be
                // intercepted there and rewritten as a `ViewRequest::DrillDown`
                // before reaching the App. If we see one here it means the
                // interception path missed; ignore rather than crash.
                EditorRequest::None
            }
            SubViewMessage::Unhandled => EditorRequest::None,
        }
    }

    fn process_view_request(&mut self, req: ViewRequest) -> EditorRequest {
        match req {
            ViewRequest::OpenEditorForAdd { parent_id: _ } => {
                // Uses the existing method which reads selected_task_id from the view.
                self.open_editor_for_add()
            }
            ViewRequest::OpenEditorForEdit(_id) => {
                self.open_editor_for_edit()
            }
            ViewRequest::OpenEditorForEditNode(_id) => {
                self.open_editor_for_restructure()
            }
            ViewRequest::OpenEditorForNotes(id) => {
                if let Some(task) = self.tasks_view.state.task_rows.iter().find(|t| t.id == id).cloned() {
                    let session = crate::edit_session::TaskNotesSession::new(
                        task,
                        self.tasks_view.state.task_rows.clone(),
                    );
                    self.open_session(Box::new(session))
                } else {
                    EditorRequest::None
                }
            }
            ViewRequest::DeleteTask(id) | ViewRequest::DeleteTaskRecursive(id) => {
                self.delete_selected_task();
                EditorRequest::None
            }
            ViewRequest::Undelete => {
                self.undelete_last();
                EditorRequest::None
            }
            ViewRequest::ToggleTracking(id) => {
                // Find if task has active tracking and toggle.
                let tracked = self.tracked_ids.contains(&id);
                self.toggle_tracking_for_task(id, tracked);
                EditorRequest::None
            }
            ViewRequest::OpenColumnConfig => {
                self.open_column_config_popup();
                EditorRequest::None
            }
            ViewRequest::SpawnLoad => {
                self.spawn_load();
                EditorRequest::None
            }
            ViewRequest::Notify(msg) => {
                self.notify(msg);
                EditorRequest::None
            }
            ViewRequest::ModalMessage(msg) => {
                self.modal_message = Some(msg);
                EditorRequest::None
            }
            // Content views (generic adapter-driven)
            ViewRequest::FetchContentPreview { view_index, pane_id, cache_key, node_id, action_id } => {
                let adapter = self.content_view(view_index)
                    .and_then(|cv| cv.adapter.as_ref())
                    .map(Arc::clone);
                let Some(adapter) = adapter else {
                    return EditorRequest::None;
                };
                let tx = self.load_tx.clone();
                tokio::spawn(async move {
                    let Ok(node) = adapter.get_by_id(&node_id).await else {
                        return;
                    };
                    let text = match action_id.as_deref() {
                        Some(action) => match node.prepare(action).await {
                            Ok(prep) => Some(prep.template),
                            Err(_) => None,
                        },
                        None => match node.content() {
                            Some(content) => content.read_text().await.ok(),
                            None => None,
                        },
                    };
                    if let Some(text) = text {
                        let _ = tx.send(LoadMsg::ContentPreview {
                            view_index, pane_id, cache_key, text,
                        });
                    }
                });
                EditorRequest::None
            }
            ViewRequest::OpenContentEditor { view_index, pane_id, node_id, action_id, label, editor_profile, commit_on_save } => {
                let adapter = self.content_view(view_index)
                    .and_then(|cv| cv.adapter.as_ref())
                    .map(Arc::clone);
                let Some(adapter) = adapter else {
                    self.notify("No adapter available".to_string());
                    return EditorRequest::None;
                };
                // Reject a second open while one is already up *or* still
                // loading. `open_session` has its own busy guard, but that
                // only catches the window after the detached child exists;
                // this closes the gap while the off-thread prepare runs.
                if self.editor_busy() {
                    self.notify("Editor is already open".to_string());
                    return EditorRequest::None;
                }
                // Build the session off-thread. Its `prepare` does the
                // network-heavy metadata/comment fetches that previously ran
                // under a `block_on` on the render thread — a dead connection
                // there froze the whole TUI. Now the ready session arrives via
                // `LoadMsg::EditorSessionReady` and is opened from
                // `handle_load_msg`; the wait is bounded by the adapter's own
                // request timeout, and the UI stays responsive throughout.
                let reload = Some(crate::edit_session::ReloadTarget { view_index, pane_id });
                self.editor_load_token = self.editor_load_token.wrapping_add(1);
                let token = self.editor_load_token;
                let msg = format!("⏳ Opening editor: {label}…");
                self.notify(msg.clone());
                self.editor_loading_msg = Some(msg);
                let tx = self.load_tx.clone();
                tokio::spawn(async move {
                    let result = crate::edit_session::NodeActionEditSession::new(
                        adapter, node_id.clone(), action_id, label, None, reload,
                        editor_profile, commit_on_save,
                    )
                    .await
                    .map(|s| Box::new(s) as Box<dyn crate::edit_session::EditSession>)
                    .map_err(|e| e.to_string());
                    let _ = tx.send(LoadMsg::EditorSessionReady { node_id, token, result });
                });
                EditorRequest::None
            }
            ViewRequest::SpawnContentLoad { view_index, pane_id } => {
                self.spawn_content_load(view_index, pane_id);
                EditorRequest::None
            }
            ViewRequest::TreeFindStart { view_index, pane_id, query } => {
                // Stamp the loading state synchronously so the status
                // hint shows the moment the user hits Enter — the
                // adapter call lives off in a tokio task.
                if let Some(cv) = self.content_view_mut(view_index) {
                    if let Some(pane) = cv.find_pane_mut(pane_id) {
                        pane.tree_find_begin(query.clone());
                    }
                }
                self.spawn_tree_find(view_index, pane_id, query, Self::TREE_FIND_DEFAULT_LIMIT);
                EditorRequest::None
            }
            ViewRequest::ApplyContentSavedQuery { view_index, pane_id, query, name } => {
                let target = crate::components::query_var_popup::QueryVarPopupTarget {
                    tab_idx: view_index,
                    pane_id,
                    raw_query: query,
                    saved_name: Some(name),
                };
                self.start_query_apply(target, std::collections::HashMap::new(), true);
                EditorRequest::None
            }
            ViewRequest::DrillDown { view_index, pane_id, node_id, node_label: _, child_node_type } => {
                self.spawn_content_drill_down(view_index, pane_id, node_id, child_node_type);
                EditorRequest::None
            }
            ViewRequest::ExpandTreeNode {
                view_index, pane_id, parent_path, parent_node_id, child_node_type, page_size, page, append,
            } => {
                self.spawn_tree_expand(
                    view_index, pane_id, parent_path, parent_node_id, child_node_type, page_size, page, append,
                );
                EditorRequest::None
            }
            ViewRequest::ExpandTreeNodeMulti {
                view_index, pane_id, parent_path, parent_node_id, child_node_types, page_size,
            } => {
                if let Some(cv) = self.content_view_mut(view_index) {
                    cv.begin_tree_multi_load(pane_id, parent_path.clone(), child_node_types.clone());
                }
                for ty in child_node_types {
                    self.spawn_tree_expand(
                        view_index,
                        pane_id,
                        parent_path.clone(),
                        parent_node_id.clone(),
                        ty,
                        page_size,
                        None,
                        false,
                    );
                }
                EditorRequest::None
            }
            ViewRequest::OpenContentQueryEditor { view_index, pane_id: _, save_name, is_new } => {
                let query_text = self.content_view(view_index)
                    .map(|cv| if is_new { cv.default_query_text() } else { cv.current_query_text() })
                    .unwrap_or_default();
                let session = crate::edit_session::ContentQueryFilterSession::new(
                    view_index, save_name, is_new, query_text,
                );
                self.open_session(Box::new(session))
            }
            ViewRequest::OpenAdapterQueryEditor { view_index, pane_id, parent_node_id } => {
                self.open_adapter_query_editor(view_index, pane_id, parent_node_id)
            }
            ViewRequest::OpenPostgresScriptsMenu { view_index, pane_id, table_node_id } => {
                self.open_postgres_scripts_menu(view_index, pane_id, table_node_id)
            }
            ViewRequest::RunPostgresScript { view_index, pane_id, database, schema, table, script } => {
                self.run_postgres_script(view_index, pane_id, database, schema, table, script)
            }
            ViewRequest::RunPostgresQuery { view_index, pane_id, database, query, page, cursor } => {
                self.spawn_postgres_query(view_index, pane_id, database, query, Some(page), cursor);
                EditorRequest::None
            }
            ViewRequest::CloseAdapterCursor { view_index, cursor_id } => {
                self.spawn_close_adapter_cursor(view_index, cursor_id);
                EditorRequest::None
            }
            ViewRequest::RunAdapterDbScript { view_index, pane_id, source_node_id, source_label, database, sql } => {
                self.run_adapter_db_script(view_index, pane_id, source_node_id, source_label, database, sql);
                EditorRequest::None
            }
            ViewRequest::OpenAdapterDbScriptEditor { view_index, pane_id, database, script, in_place } => {
                self.open_adapter_db_script_editor(view_index, pane_id, database, script, in_place)
            }
            ViewRequest::OpenDbScriptNewPrompt { view_index, pane_id, database, parent_rel } => {
                self.open_db_script_new_prompt(view_index, pane_id, database, parent_rel);
                EditorRequest::None
            }
            ViewRequest::OpenDbScriptDirNewPrompt { view_index, pane_id, database, parent_rel } => {
                self.open_db_script_dir_new_prompt(view_index, pane_id, database, parent_rel);
                EditorRequest::None
            }
            ViewRequest::ConfirmDeleteAdapterDbScript { view_index, pane_id, database, script } => {
                self.confirm_delete_adapter_db_script(view_index, pane_id, database, script);
                EditorRequest::None
            }
            ViewRequest::ConfirmDeleteAdapterDbScriptDir { view_index, pane_id, database, rel_path } => {
                self.confirm_delete_adapter_db_script_dir(view_index, pane_id, database, rel_path);
                EditorRequest::None
            }
            ViewRequest::ConfirmDeleteContentNode { view_index, pane_id, node_id } => {
                self.confirm_delete_content_node(view_index, pane_id, node_id);
                EditorRequest::None
            }
            ViewRequest::OpenDbScriptRenamePrompt { view_index, pane_id, database, rel_path, is_dir } => {
                self.open_db_script_rename_prompt(view_index, pane_id, database, rel_path, is_dir);
                EditorRequest::None
            }
            ViewRequest::MarkDbScriptForMove { node_id } => {
                self.mark_db_script_for_move(node_id);
                EditorRequest::None
            }
            ViewRequest::PasteDbScriptMove { target_node_id } => {
                self.paste_db_script_move(target_node_id);
                EditorRequest::None
            }
            ViewRequest::EditPostgresScript { view_index, pane_id, database, schema, table, script, is_new } => {
                self.edit_postgres_script(view_index, pane_id, database, schema, table, script, is_new)
            }
            ViewRequest::DeletePostgresScript { view_index, pane_id, database, schema, table, script } => {
                self.delete_postgres_script(view_index, pane_id, database, schema, table, script);
                EditorRequest::None
            }
            ViewRequest::PromptPostgresScriptShortcut { view_index, pane_id: _, database, schema, table, script } => {
                self.prompt_postgres_script_shortcut(view_index, database, schema, table, script);
                EditorRequest::None
            }
            ViewRequest::ExecuteContentAction { view_index, pane_id, node_id, action_id } => {
                self.open_content_action_popup(view_index, pane_id, node_id, action_id);
                EditorRequest::None
            }
            ViewRequest::InvokeNodeAction { view_index, pane_id, node_id, action_name } => {
                self.spawn_invoke_node_action(view_index, pane_id, node_id, action_name);
                EditorRequest::None
            }
            ViewRequest::InvalidateContentSession { view_index } => {
                self.spawn_invalidate_auth(view_index, AuthInvalidate::Session);
                EditorRequest::None
            }
            ViewRequest::InvalidateContentCredentials { view_index } => {
                self.spawn_invalidate_auth(view_index, AuthInvalidate::Credentials);
                EditorRequest::None
            }
            ViewRequest::CreateContentChild { view_index, pane_id, parent_node_id, child_node_type, action_id, label, editor_profile, commit_on_save } => {
                let adapter = self.content_view(view_index)
                    .and_then(|cv| cv.adapter.as_ref())
                    .map(Arc::clone);
                let Some(adapter) = adapter else {
                    self.notify("No adapter available".to_string());
                    return EditorRequest::None;
                };
                let nav = crate::edit_session::NavContext {
                    view_index,
                    parent_node_id: parent_node_id.clone(),
                    child_node_type,
                };
                let reload = Some(crate::edit_session::ReloadTarget { view_index, pane_id });
                let result = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        crate::edit_session::NodeActionEditSession::new(
                            adapter, parent_node_id.clone(), action_id, label, Some(nav), reload, editor_profile, commit_on_save,
                        ).await
                    })
                });
                match result {
                    Ok(session) => self.open_session(Box::new(session)),
                    Err(e) => {
                        self.notify_error(format!("Failed to load {parent_node_id}: {e}"));
                        EditorRequest::None
                    }
                }
            }
            ViewRequest::SaveContentQuery { view_index, scope: _, name, query } => {
                self.save_content_query_body(view_index, &name, &query);
                self.reload_content_saved_queries(view_index);
                EditorRequest::None
            }
            ViewRequest::DeleteContentQuery { view_index, scope, name } => {
                self.delete_content_query(view_index, &scope, &name);
                self.reload_content_saved_queries(view_index);
                self.notify(format!("Deleted query '{name}'"));
                EditorRequest::None
            }
            ViewRequest::SetDefaultContentQuery { view_index, name } => {
                self.set_default_content_query(view_index, &name);
                EditorRequest::None
            }
            ViewRequest::PromptContentQueryShortcut { view_index, scope, name, query } => {
                self.save_content_query_body(view_index, &name, &query);
                self.reload_content_saved_queries(view_index);
                self.modal_message = Some(format!("Press a shortcut key for '{}'\n\nEsc to cancel", name));
                self.awaiting_favorite_shortcut = Some((scope, name, query));
                EditorRequest::None
            }
            ViewRequest::RenameContentQuery { view_index, scope, old_name, new_name } => {
                self.rename_content_query(view_index, &scope, &old_name, &new_name);
                self.reload_content_saved_queries(view_index);
                EditorRequest::None
            }
            ViewRequest::ApplySavedQuery { scope, content } => {
                self.apply_saved_query(&scope, &content)
            }
            ViewRequest::OpenSavedQueryEditor { scope, name, current_query, is_new } => {
                self.open_editor_for_saved_query(&scope, name, current_query, is_new)
            }
            ViewRequest::DeleteSavedQuery { scope, name } => {
                self.delete_saved_query(&scope, &name);
                EditorRequest::None
            }
            ViewRequest::PromptSavedQueryShortcut { scope, name, query } => {
                self.prompt_saved_query_shortcut(scope, name, query);
                EditorRequest::None
            }
            ViewRequest::SetDefaultSavedQuery { scope, name } => {
                self.set_default_saved_query(&scope, &name);
                EditorRequest::None
            }
            ViewRequest::OpenScriptMenuForNode { view_index, pane_id } => {
                self.open_script_menu_for_content(view_index, pane_id);
                EditorRequest::None
            }
            ViewRequest::OpenScriptMenuForTasks => {
                self.open_script_menu_for_tasks();
                EditorRequest::None
            }
            _ => EditorRequest::None,
        }
    }

    fn process_trackings_message(&mut self, msg: SubViewMessage) -> EditorRequest {
        match msg {
            SubViewMessage::Request(req) => self.process_trackings_request(req),
            SubViewMessage::SelectionChanged(_) => EditorRequest::None,
            SubViewMessage::FuzzyStateChanged { .. } => EditorRequest::None,
            SubViewMessage::SearchStateChanged { .. } => {
                // Search state now lives on views; handled via Searchable trait.
                EditorRequest::None
            }
            _ => EditorRequest::None,
        }
    }

    fn process_trackings_request(&mut self, req: ViewRequest) -> EditorRequest {
        match req {
            ViewRequest::DeleteTracking => {
                self.delete_selected_tracking();
                EditorRequest::None
            }
            ViewRequest::ToggleTracking(id) => {
                let active = self.tracked_ids.contains(&id);
                self.toggle_tracking_for_task(id, active);
                EditorRequest::None
            }
            ViewRequest::OpenColumnConfig => {
                self.open_column_config_popup();
                EditorRequest::None
            }
            ViewRequest::OpenScriptMenuForTrackings => {
                self.open_script_menu_for_trackings();
                EditorRequest::None
            }
            ViewRequest::OpenScriptMenuForNode { view_index, pane_id } => {
                self.open_script_menu_for_content(view_index, pane_id);
                EditorRequest::None
            }
            ViewRequest::OpenTrackingGroupPopup => {
                self.trackings_view.open_group_popup();
                EditorRequest::None
            }
            ViewRequest::SaveTrackingGrouping(label) => {
                self.save_tracking_grouping_by_label(&label);
                EditorRequest::None
            }
            ViewRequest::RestoreTracking => {
                self.restore_selected_tracking();
                EditorRequest::None
            }
            ViewRequest::RestoreAllTrackings => {
                self.restore_all_deleted_trackings();
                EditorRequest::None
            }
            ViewRequest::ApplySavedQuery { scope, content } => {
                self.apply_saved_query(&scope, &content)
            }
            ViewRequest::OpenSavedQueryEditor { scope, name, current_query, is_new } => {
                self.open_editor_for_saved_query(&scope, name, current_query, is_new)
            }
            ViewRequest::DeleteSavedQuery { scope, name } => {
                self.delete_saved_query(&scope, &name);
                EditorRequest::None
            }
            ViewRequest::PromptSavedQueryShortcut { scope, name, query } => {
                self.prompt_saved_query_shortcut(scope, name, query);
                EditorRequest::None
            }
            ViewRequest::SetDefaultSavedQuery { scope, name } => {
                self.set_default_saved_query(&scope, &name);
                EditorRequest::None
            }
            _ => EditorRequest::None,
        }
    }

    // ── Content action popup (transitions, etc.) ─────────────────────

    /// Drive `ContentAdapter::invalidate_session` /
    /// `invalidate_credentials` for the active content view. The actual
    /// trait method is async-cheap (it only flips orchestrator state),
    /// but we still spawn it so the UI thread doesn't block; on
    /// completion we reuse `LoadMsg::ContentActionDone` which already
    /// notifies + reloads, naturally driving re-auth through the next
    /// list call.
    /// Spawn the async path for `ViewRequest::InvokeNodeAction`. Loads
    /// the node, calls `Node::invoke_action`, and routes the
    /// `ActionDispatch` (or error) back via `LoadMsg::NodeActionDispatched`.
    fn spawn_invoke_node_action(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        node_id: String,
        action_name: String,
    ) {
        let adapter = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone);
        let Some(adapter) = adapter else {
            self.notify("No adapter available".to_string());
            return;
        };
        let tx = self.load_tx.clone();
        let action_name_for_task = action_name.clone();
        let node_id_for_task = node_id.clone();
        // M7/E6: hand the current move clipboard to the adapter so a
        // `paste-move` invocation can read the marked node out of the
        // context and relocate it. Every other action ignores it.
        let marked = self.content_marked_node.clone();
        tokio::spawn(async move {
            let ctx = not_yet_done_content::ActionContext { marked };
            // Capture the node's label + type alongside the dispatch so a
            // `mark-move` can populate the clipboard without re-fetching.
            let outcome: not_yet_done_content::Result<(
                not_yet_done_content::ActionDispatch,
                String,
                not_yet_done_content::NodeType,
            )> = async {
                let node = adapter.get_by_id(&node_id_for_task).await?;
                let label = node.label().to_string();
                let node_type = node.node_type().clone();
                let dispatch = node.invoke_action(&action_name_for_task, &ctx).await?;
                Ok((dispatch, label, node_type))
            }
            .await;
            let (result, node_label, node_type) = match outcome {
                Ok((dispatch, label, node_type)) => (Ok(dispatch), Some(label), Some(node_type)),
                Err(e) => (
                    Err(format!("Action '{action_name_for_task}': {e}")),
                    None,
                    None,
                ),
            };
            let _ = tx.send(LoadMsg::NodeActionDispatched {
                view_index,
                pane_id,
                node_id: node_id_for_task,
                action_name: action_name_for_task,
                result,
                node_label,
                node_type,
            });
        });
    }

    /// Handle the async result of `Node::invoke_action`. Translates the
    /// returned `ActionDispatch` into the next `ViewRequest` (or a
    /// notification) via `node_actions::dispatch_to_view_request`, then
    /// dispatches it.
    fn handle_node_action_dispatched(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        node_id: String,
        action_name: String,
        result: Result<not_yet_done_content::ActionDispatch, String>,
        node_label: Option<String>,
        node_type: Option<not_yet_done_content::NodeType>,
    ) {
        let dispatch = match result {
            Ok(d) => d,
            Err(msg) => {
                self.notify_error(msg);
                return;
            }
        };
        // M7/E6: generic mark/paste-move clipboard. db-script nodes keep
        // their bespoke path (handled inside `dispatch_to_view_request`),
        // so `generic_mark_move_effect` returns `Ignore` for them — the
        // two clipboards stay disjoint until the consolidation follow-up.
        match crate::app::node_actions::generic_mark_move_effect(&action_name, &node_id) {
            crate::app::node_actions::MarkMoveEffect::Mark => {
                if let (Some(label), Some(nt)) = (node_label, node_type) {
                    self.content_marked_node = Some(not_yet_done_content::MarkedNode {
                        node_id,
                        node_type: nt,
                        label: label.clone(),
                    });
                    self.notify(format!(
                        "Marked '{label}' for move — paste with `paste-move` on the target"
                    ));
                } else {
                    self.notify_error("Could not mark node for move".to_string());
                }
                return;
            }
            crate::app::node_actions::MarkMoveEffect::ClearOnPasteSuccess => {
                // The adapter performed the move; a `Reload` dispatch
                // confirms success, so the source is no longer "cut".
                if matches!(dispatch, not_yet_done_content::ActionDispatch::Reload) {
                    self.content_marked_node = None;
                }
                // Fall through so the `Reload` reloads the target pane.
            }
            crate::app::node_actions::MarkMoveEffect::Ignore => {}
        }
        // M9: a `PatchRow` dispatch swaps the invoking row's state in
        // place (e.g. a tree row flipping its own tracking marker) without
        // refetching or rebuilding the pane — the node built the summary
        // with its own view-correct id, so a row the domain-event bridge
        // cannot address (scope-encoded `tree:<…>` id) still updates. We
        // patch directly here rather than minting a `ViewRequest` because
        // there is no async roundtrip to dispatch — it's a synchronous
        // edit of already-loaded pane state.
        if let not_yet_done_content::ActionDispatch::PatchRow(summary) = &dispatch {
            if let Some(cv) = self.content_view_mut(view_index) {
                cv.patch_row(summary);
            }
            return;
        }
        // Resolve the `editor_in_place` flag for the row's node-type
        // by looking it up in the view-config tree. DB scripts can
        // sit under multiple branches (DSF-6 recursive structure),
        // so any matching ChildDef sets the policy — they should all
        // agree because they describe the same node type.
        let editor_in_place = self
            .content_view(view_index)
            .and_then(|cv| cv.active_view_def())
            .map(|v| crate::app::node_actions::editor_in_place_for_node_id(v, &node_id))
            .unwrap_or(false);
        if let Some(req) = crate::app::node_actions::dispatch_to_view_request(
            dispatch,
            view_index,
            pane_id,
            node_id,
            action_name,
            editor_in_place,
        ) {
            // Routes back through the same dispatcher as in-band view
            // requests so a `Reload` dispatch behaves identically to a
            // user-triggered reload (including any in-flight cancellation,
            // status reset, etc). The returned EditorRequest needs to
            // bubble out to main.rs — stash it for the loop's post-
            // `poll_load` drain (see `pending_editor_request`).
            match self.process_view_request(req) {
                EditorRequest::None => {}
                other => self.pending_editor_request = Some(other),
            }
        }
    }

    fn spawn_invalidate_auth(&mut self, view_index: usize, kind: AuthInvalidate) {
        let cv = match self.content_view(view_index) {
            Some(cv) => cv,
            None => return,
        };
        let Some(adapter) = cv.adapter.as_ref().map(Arc::clone) else {
            self.notify("No adapter available".to_string());
            return;
        };
        let pane_id = cv.active_pane_id();

        let tx = self.load_tx.clone();
        tokio::spawn(async move {
            let outcome = match kind {
                AuthInvalidate::Session => adapter.invalidate_session().await,
                AuthInvalidate::Credentials => adapter.invalidate_credentials().await,
            };
            let result: Result<String, String> = match outcome {
                Ok(()) => Ok(match kind {
                    AuthInvalidate::Session => "Session invalidated, re-authenticating…".to_string(),
                    AuthInvalidate::Credentials => {
                        "Credentials invalidated, re-authenticating…".to_string()
                    }
                }),
                Err(e) => Err(format!("Invalidate failed: {e}")),
            };
            let _ = tx.send(LoadMsg::ContentActionDone {
                view_index,
                pane_id,
                result,
            });
        });
    }

    fn open_content_action_popup(&mut self, view_index: usize, pane_id: crate::views::content_view::PaneId, node_id: String, action_id: String) {
        let adapter = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone);
        let Some(adapter) = adapter else {
            self.notify("No adapter available".to_string());
            return;
        };

        // Inspect the action's InputSpec to decide whether to fire it
        // immediately (`None`) or surface a picker (`Picker`). Editor
        // actions reach app via `OpenContentEditor` and never land here.
        let spec_lookup = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let node = adapter.get_by_id(&node_id).await?;
                Ok::<_, not_yet_done_content::ContentError>(
                    node.actions()
                        .into_iter()
                        .find(|a| a.id == action_id)
                        .map(|a| a.input),
                )
            })
        });
        let spec = match spec_lookup {
            Ok(Some(spec)) => spec,
            Ok(None) => {
                self.notify(format!("Action `{action_id}` not exposed by node"));
                return;
            }
            Err(e) => {
                self.notify_error(format!("Failed to load node: {e}"));
                return;
            }
        };

        match spec {
            not_yet_done_content::InputSpec::None => {
                let tx = self.load_tx.clone();
                let vi = view_index;
                let pid = pane_id;
                tokio::spawn(async move {
                    let outcome = async {
                        let mut node = adapter.get_by_id(&node_id).await?;
                        node.execute(&action_id, not_yet_done_content::ActionInput::None).await
                    }
                    .await;
                    let result = match outcome {
                        Ok(not_yet_done_content::ActionOutcome::Done { message }) => {
                            Ok(message.unwrap_or_else(|| format!("{action_id} executed")))
                        }
                        Ok(_) => Ok(format!("{action_id} executed")),
                        Err(e) => Err(format!("Action failed: {action_id}: {e}")),
                    };
                    let _ = tx.send(LoadMsg::ContentActionDone {
                        view_index: vi,
                        pane_id: pid,
                        result,
                    });
                });
            }
            not_yet_done_content::InputSpec::Picker => {
                let result = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        let node = adapter.get_by_id(&node_id).await?;
                        node.picker_options(&action_id).await
                    })
                });
                match result {
                    Ok(options) if options.is_empty() => {
                        self.notify("No options available".to_string());
                    }
                    Ok(options) => {
                        let items: Vec<PopupItem> = options
                            .iter()
                            .map(|o| PopupItem {
                                label: o.label.clone(),
                                value: o.value.clone(),
                                ..Default::default()
                            })
                            .collect();
                        let popup = SearchablePopup::new(
                            Arc::clone(&self.shared_theme),
                            format!("Select {action_id}"),
                            items,
                        )
                        .with_popup_kb(
                            self.keybindings.popup.clone(),
                            self.keybindings.key_icons.clone(),
                        )
                        .with_hints(vec![
                            ("Enter".to_string(), "apply".to_string()),
                            ("Esc".to_string(), "close".to_string()),
                        ]);
                        self.content_action_popup = Some(ContentActionPopupState {
                            popup,
                            view_index,
                            pane_id,
                            node_id,
                            action_id,
                        });
                    }
                    Err(e) => {
                        self.notify_error(format!("Failed to load options: {e}"));
                    }
                }
            }
            not_yet_done_content::InputSpec::Editor => {
                self.notify(format!(
                    "Action `{action_id}` requires an editor — use `type: edit` in YAML, not `custom`"
                ));
            }
            not_yet_done_content::InputSpec::FilePicker { multi: _ } => {
                use not_yet_done_ratatui::{
                    FilePickerStyle, SelectListStyle, SelectListStyleType,
                    TextInputStyle, TextInputStyleType,
                };
                use ratatui::style::{Modifier, Style};
                use tuirealm::component::Component;
                use tuirealm::props::{AttrValue, Attribute};

                let theme = &*self.shared_theme;
                let panel_bg = theme.surface();
                let input_bg = theme.surface_2();
                let accent = theme.accent();
                let primary = theme.primary();
                let text_high = theme.text_high();
                let dim = theme.text_dim();

                let text_inactive = TextInputStyle::new()
                    .prefix_color(primary)
                    .set_style(TextInputStyleType::Title, Style::default().fg(primary).bg(panel_bg))
                    .set_style(TextInputStyleType::Input, Style::default().fg(text_high).bg(panel_bg))
                    .placeholder_color(dim);
                let text_active = TextInputStyle::new()
                    .prefix_color(accent)
                    .set_style(
                        TextInputStyleType::Title,
                        Style::default().fg(accent).bg(input_bg).add_modifier(Modifier::BOLD),
                    )
                    .set_style(TextInputStyleType::Input, Style::default().fg(text_high).bg(input_bg))
                    .placeholder_color(dim);

                let list_inactive = SelectListStyle::default()
                    .prefix_color(primary)
                    .placeholder_color(dim)
                    .set_style(SelectListStyleType::Item, Style::default().fg(text_high).bg(panel_bg))
                    .set_style(
                        SelectListStyleType::ItemSelected,
                        Style::default().fg(text_high).bg(input_bg),
                    )
                    .set_style(SelectListStyleType::ItemCursor, Style::default().fg(text_high).bg(panel_bg))
                    .set_style(
                        SelectListStyleType::ItemCursorSelected,
                        Style::default().fg(text_high).bg(input_bg),
                    )
                    .set_style(SelectListStyleType::FilterInput, Style::default().fg(dim).bg(panel_bg))
                    .set_style(SelectListStyleType::Footer, Style::default().fg(dim).bg(panel_bg));
                let list_active = SelectListStyle::default()
                    .prefix_color(accent)
                    .placeholder_color(dim)
                    .set_style(SelectListStyleType::Item, Style::default().fg(text_high).bg(input_bg))
                    .set_style(
                        SelectListStyleType::ItemSelected,
                        Style::default().fg(text_high).bg(panel_bg),
                    )
                    .set_style(
                        SelectListStyleType::ItemCursor,
                        Style::default().fg(accent).bg(input_bg).add_modifier(Modifier::BOLD),
                    )
                    .set_style(
                        SelectListStyleType::ItemCursorSelected,
                        Style::default().fg(accent).bg(panel_bg).add_modifier(Modifier::BOLD),
                    )
                    .set_style(
                        SelectListStyleType::FilterInput,
                        Style::default().fg(text_high).bg(input_bg),
                    )
                    .set_style(
                        SelectListStyleType::FilterCursor,
                        Style::default().fg(input_bg).bg(accent).add_modifier(Modifier::BOLD),
                    )
                    .set_style(SelectListStyleType::Footer, Style::default().fg(dim).bg(input_bg));

                let picker_style = FilePickerStyle::new()
                    .with_text_input_inactive(text_inactive)
                    .with_text_input_active(text_active)
                    .with_select_list_inactive(list_inactive)
                    .with_select_list_active(list_active)
                    .with_panel_bg(panel_bg)
                    .with_title_style(Style::default().fg(accent).bg(panel_bg).add_modifier(Modifier::BOLD))
                    .with_help_keys_style(Style::default().fg(primary).bg(panel_bg).add_modifier(Modifier::BOLD))
                    .with_help_labels_style(Style::default().fg(dim).bg(panel_bg))
                    .with_paste_error_style(
                        Style::default().fg(theme.error()).bg(panel_bg).add_modifier(Modifier::BOLD),
                    );

                let mut picker = FilePicker::default()
                    .with_style(picker_style)
                    .with_title(format!("✦ {action_id}"))
                    .with_initial_directory(
                        dirs::home_dir()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default(),
                    )
                    .with_files_title("Files in Directory".to_string())
                    .with_paste_provider(clipboard_text);
                picker.attr(Attribute::Focus, AttrValue::Flag(true));
                self.content_file_picker_popup = Some(ContentFilePickerPopupState {
                    picker,
                    view_index,
                    pane_id,
                    node_id,
                    action_id,
                });
            }
            not_yet_done_content::InputSpec::Form { fields } => {
                // Prefill values from the node (edit flow); falls back to
                // each field's static `default` inside the popup.
                let prefill = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        let node = adapter.get_by_id(&node_id).await?;
                        node.form_prep(&action_id).await
                    })
                });
                let prefill = match prefill {
                    Ok(p) => p,
                    Err(e) => {
                        self.notify_error(format!("Failed to prepare form: {e}"));
                        return;
                    }
                };
                let popup = ContentFormPopup::new(action_id.clone(), fields, &prefill);
                self.content_form_popup = Some(ContentFormPopupState {
                    popup,
                    view_index,
                    pane_id,
                    node_id,
                    action_id,
                });
            }
        }
    }

    fn execute_content_action_files(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        node_id: String,
        action_id: String,
        paths: Vec<std::path::PathBuf>,
    ) {
        let adapter = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone);
        let Some(adapter) = adapter else {
            self.notify("No adapter available".to_string());
            return;
        };
        let tx = self.load_tx.clone();
        let vi = view_index;
        let pid = pane_id;
        let aid_for_msg = action_id.clone();
        tokio::spawn(async move {
            let outcome = async {
                let mut node = adapter.get_by_id(&node_id).await?;
                node.execute(
                    &action_id,
                    not_yet_done_content::ActionInput::Files(paths),
                )
                .await
            }
            .await;
            let result = match outcome {
                Ok(not_yet_done_content::ActionOutcome::Done { message }) => {
                    Ok(message.unwrap_or_else(|| format!("{aid_for_msg} executed")))
                }
                Ok(_) => Ok(format!("{aid_for_msg} executed")),
                Err(e) => Err(format!("Action failed: {aid_for_msg}: {e}")),
            };
            let _ = tx.send(LoadMsg::ContentActionDone {
                view_index: vi,
                pane_id: pid,
                result,
            });
        });
    }

    fn execute_content_action_form(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        node_id: String,
        action_id: String,
        values: std::collections::HashMap<String, String>,
    ) {
        let adapter = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone);
        let Some(adapter) = adapter else {
            self.notify("No adapter available".to_string());
            return;
        };
        let tx = self.load_tx.clone();
        let vi = view_index;
        let pid = pane_id;
        let aid_for_msg = action_id.clone();
        tokio::spawn(async move {
            let outcome = async {
                let mut node = adapter.get_by_id(&node_id).await?;
                node.execute(
                    &action_id,
                    not_yet_done_content::ActionInput::Form(values),
                )
                .await
            }
            .await;
            let result = match outcome {
                Ok(not_yet_done_content::ActionOutcome::Done { message }) => {
                    Ok(message.unwrap_or_else(|| format!("{aid_for_msg} executed")))
                }
                Ok(_) => Ok(format!("{aid_for_msg} executed")),
                Err(e) => Err(format!("Action failed: {aid_for_msg}: {e}")),
            };
            let _ = tx.send(LoadMsg::ContentActionDone {
                view_index: vi,
                pane_id: pid,
                result,
            });
        });
    }

    fn execute_content_action(&mut self, view_index: usize, pane_id: crate::views::content_view::PaneId, node_id: String, action_id: String, value: String) {
        let adapter = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone);
        let Some(adapter) = adapter else {
            self.notify("No adapter available".to_string());
            return;
        };

        let tx = self.load_tx.clone();
        let vi = view_index;
        let pid = pane_id;
        tokio::spawn(async move {
            let outcome = async {
                let mut node = adapter.get_by_id(&node_id).await?;
                node.execute(
                    &action_id,
                    not_yet_done_content::ActionInput::Picked(value),
                )
                .await
            }
            .await;
            let result = match outcome {
                Ok(not_yet_done_content::ActionOutcome::Done { message }) => {
                    Ok(message.unwrap_or_else(|| format!("{action_id} executed")))
                }
                Ok(_) => Ok(format!("{action_id} executed")),
                Err(e) => Err(format!("Action failed: {action_id}: {e}")),
            };
            let _ = tx.send(LoadMsg::ContentActionDone {
                view_index: vi,
                pane_id: pid,
                result,
            });
        });
    }


    fn handle_common_action(&mut self, action: CommonAction) -> EditorRequest {
        match action {
            CommonAction::ListNext => {
                if let Some(table) = self.active_table_mut() {
                    table.handle_nav(tuirealm::command::Cmd::Move(tuirealm::command::Direction::Down));
                }
            }
            CommonAction::ListPrev => {
                if let Some(table) = self.active_table_mut() {
                    table.handle_nav(tuirealm::command::Cmd::Move(tuirealm::command::Direction::Up));
                }
            }
            CommonAction::ListFirst => {
                if let Some(table) = self.active_table_mut() {
                    table.handle_nav(tuirealm::command::Cmd::GoTo(tuirealm::command::Position::Begin));
                }
            }
            CommonAction::ListLast => {
                if let Some(table) = self.active_table_mut() {
                    table.handle_nav(tuirealm::command::Cmd::GoTo(tuirealm::command::Position::End));
                }
            }
            CommonAction::ScrollHalfUp => {
                if let Some(table) = self.active_table_mut() {
                    let n = (table.visible_rows() / 2).max(1) as isize;
                    table.scroll_by(-n);
                }
            }
            CommonAction::ScrollHalfDown => {
                if let Some(table) = self.active_table_mut() {
                    let n = (table.visible_rows() / 2).max(1) as isize;
                    table.scroll_by(n);
                }
            }
            CommonAction::ScrollPageUp => {
                if let Some(table) = self.active_table_mut() {
                    let n = table.visible_rows().max(1) as isize;
                    table.scroll_by(-n);
                }
            }
            CommonAction::ScrollPageDown => {
                if let Some(table) = self.active_table_mut() {
                    let n = table.visible_rows().max(1) as isize;
                    table.scroll_by(n);
                }
            }
            CommonAction::FuzzyFilterOpen => {
                if self.active_tab == Tab::Tasks {
                    self.tasks_view.state.close_form();
                    self.task_table_mut().fuzzy_open();
                } else {
                    self.trackings_view.state.fuzzy_open();
                }
            }
            CommonAction::FuzzyFilterAccept => {
                if self.active_tab == Tab::Tasks {
                    if self.task_table().fuzzy_active {
                        self.task_table_mut().fuzzy_close();
                    }
                } else {
                    self.trackings_view.state.fuzzy_close();
                    self.rebuild_trackings_table();
                }
            }
            CommonAction::FuzzyFilterClear => {
                if self.active_tab == Tab::Tasks {
                    if self.task_table().fuzzy_active {
                        self.task_table_mut().fuzzy_query.clear();
                        self.task_table_mut().fuzzy_cursor = 0;
                        self.task_table_mut().filter_text.clear();
                    }
                } else {
                    self.trackings_view.state.fuzzy_query.clear();
                    self.trackings_view.state.fuzzy_cursor = 0;
                    self.trackings_view.state.refilter();
                    self.rebuild_trackings_table();
                }
            }
            CommonAction::FuzzyFilterCancel => {
                if self.active_tab == Tab::Tasks {
                    if self.task_table().fuzzy_active {
                        if self.task_table_mut().fuzzy_query.is_empty() {
                            self.task_table_mut().fuzzy_close();
                        } else {
                            self.task_table_mut().fuzzy_query.clear();
                            self.task_table_mut().fuzzy_cursor = 0;
                            self.task_table_mut().filter_text.clear();
                        }
                    }
                } else {
                    if self.trackings_view.state.fuzzy_query.is_empty() {
                        self.trackings_view.state.fuzzy_close();
                    } else {
                        self.trackings_view.state.fuzzy_query.clear();
                        self.trackings_view.state.fuzzy_cursor = 0;
                        self.trackings_view.state.refilter();
                    }
                    self.rebuild_trackings_table();
                }
            }
            CommonAction::SearchOpen => {
                use crate::views::Searchable;
                match self.active_tab {
                    Tab::Tasks => self.tasks_view.search_open(),
                    Tab::Trackings => self.trackings_view.search_open(),
                    Tab::Content(_) => {}
                }
            }
            CommonAction::SearchNext => {
                use crate::views::Searchable;
                match self.active_tab {
                    Tab::Tasks => self.tasks_view.search_jump(1),
                    Tab::Trackings => self.trackings_view.search_jump(1),
                    Tab::Content(_) => {}
                }
            }
            CommonAction::SearchPrev => {
                use crate::views::Searchable;
                match self.active_tab {
                    Tab::Tasks => self.tasks_view.search_jump(-1),
                    Tab::Trackings => self.trackings_view.search_jump(-1),
                    Tab::Content(_) => {}
                }
            }
            CommonAction::SavedFilterSelect => {
                self.load_saved_queries();
                if self.active_tab == Tab::Trackings {
                    self.trackings_view.open_query_menu();
                } else if self.active_tab == Tab::Tasks {
                    self.tasks_view.open_query_menu();
                }
            }
            CommonAction::FormFilter => {
                // Deprecated — was a separate edit/create popup; the unified
                // query menu (q) now covers create/edit/delete/shortcut.
            }
            CommonAction::ColumnConfig => {
                self.open_column_config_popup();
            }
            CommonAction::TrackingToggle => {
                match self.active_tab {
                    Tab::Trackings => self.toggle_tracking_from_trackings_view(),
                    Tab::Tasks => self.toggle_tracking(),
                    Tab::Content(_) => {}
                }
            }
            CommonAction::FormClose => {
                if self.active_tab == Tab::Tasks {
                    self.tasks_view.state.close_form();
                }
            }
            CommonAction::FavoriteToggle => {
                // Handled before action resolution when popup is open.
            }
            CommonAction::CommandLineOpen => {
                use crate::views::HasCmdline;
                match self.active_tab {
                    Tab::Tasks => self.tasks_view.cmdline_open(),
                    Tab::Trackings => self.trackings_view.cmdline_open(),
                    Tab::Content(idx) => {
                        if let Some(cv) = self.content_view_mut(idx) {
                            cv.cmdline_open();
                        }
                    }
                }
            }
            CommonAction::JumpMode => {
                if let Some(table) = self.active_table_mut() {
                    table.jump_mode_open();
                }
            }
            CommonAction::SortMode => {
                self.enter_sort_hint_mode();
            }
            CommonAction::ColumnLeft => {
                if let Some(table) = self.active_table_mut() {
                    table.move_column_left();
                }
            }
            CommonAction::ColumnRight => {
                if let Some(table) = self.active_table_mut() {
                    table.move_column_right();
                }
            }
        }
        EditorRequest::None
    }

    fn handle_tasks_action(&mut self, action: TasksAction) -> EditorRequest {
        match action {
            TasksAction::ViewList | TasksAction::ViewTree => {
                // Handled by TasksView.handle_key() — only reachable via chord fallback.
            }
            TasksAction::FormAdd => {
                return self.open_editor_for_add();
            }
            TasksAction::FormEdit => {
                return self.open_editor_for_edit();
            }
            TasksAction::FormEditNode => {
                return self.open_editor_for_restructure();
            }
            TasksAction::Delete => {
                self.delete_selected_task();
            }
            TasksAction::Undelete => {
                self.undelete_last();
            }
            TasksAction::OpenNotes => {
                return self.open_notes_for_selected_task();
            }
            TasksAction::OpenScriptMenu => {
                self.open_script_menu_for_tasks();
            }
            TasksAction::TreeToggle
            | TasksAction::TreeExpandAll
            | TasksAction::TreeCollapseAll => {
                // Handled by TasksView.handle_key() in tree sub-view.
                // Reachable here only via chord-fallback dispatch.
            }
        }
        EditorRequest::None
    }

    fn handle_trackings_action(&mut self, action: TrackingsAction) -> EditorRequest {
        // Simulate the key event by converting the action to a synthetic key press
        // through the view. For chord-resolved actions, we replicate the same logic
        // the view uses internally.
        let forest = self.tasks_view.state.forest.as_ref();
        match action {
            TrackingsAction::TrackingGroup => {
                self.trackings_view.open_group_popup();
            }
            TrackingsAction::TrackingOrderToggle => {
                self.trackings_view.state.toggle_order();
                self.trackings_view.rebuild_table();
            }
            TrackingsAction::TrackingCondensedToggle => {
                let current = self.trackings_view.table.selected_row();
                let new_idx = self.trackings_view.state.toggle_condensed(current);
                self.trackings_view.rebuild_table();
                self.trackings_view.table.set_selected(new_idx);
            }
            TrackingsAction::TrackingTreeToggle => {
                if let Some(forest) = forest {
                    let current = self.trackings_view.table.selected_row();
                    let new_idx = self.trackings_view.state.toggle_tree_mode(current, forest);
                    self.trackings_view.rebuild_table();
                    self.trackings_view.table.set_selected(new_idx);
                }
            }
            TrackingsAction::TrackingNormalToggle => {
                if self.trackings_view.state.sub_view != TrackingsSubView::Normal {
                    self.trackings_view.state.sub_view = TrackingsSubView::Normal;
                    self.trackings_view.rebuild_table();
                }
            }
            TrackingsAction::TrackingScriptRun => {
                self.open_script_menu_for_trackings();
            }
            TrackingsAction::TrackingDelete => {
                self.delete_selected_tracking();
            }
            TrackingsAction::TrackingRestore => {
                self.restore_selected_tracking();
            }
            TrackingsAction::TrackingRestoreAll => {
                self.restore_all_deleted_trackings();
            }
        }
        EditorRequest::None
    }

    // -----------------------------------------------------------------------
    // Saved filter popup
    // -----------------------------------------------------------------------

    fn open_column_config_popup(&mut self) {
        use crate::components::column_config_popup::{ColumnConfigPopup, ColumnEntry};
        use crate::tabs::columns::{resolve_color, ColumnMeta, ALL_COLUMNS, ALL_TRACKING_COLUMNS};

        // Native tabs read from the static column registry; content tabs
        // ask their view for the active level's configured columns.
        let native_entries = |metas: &'static [ColumnMeta], theme: &Theme| -> Vec<ColumnEntry> {
            metas
                .iter()
                .map(|m| ColumnEntry {
                    id: m.id.to_string(),
                    header: m.header.to_string(),
                    display_name: m.display_name.to_string(),
                    color: resolve_color(m.color_key, theme),
                    hideable: m.hideable,
                })
                .collect()
        };

        let (config, entries) = match self.active_tab {
            Tab::Trackings => (
                self.trackings_view.column_config.clone(),
                native_entries(ALL_TRACKING_COLUMNS, &self.shared_theme),
            ),
            Tab::Tasks => (
                self.tasks_view.column_config.clone(),
                native_entries(ALL_COLUMNS, &self.shared_theme),
            ),
            Tab::Content(idx) => {
                match self.content_view(idx).and_then(|cv| cv.column_config_entries()) {
                    Some(pair) => pair,
                    None => {
                        self.notify("This level has no configurable columns".to_string());
                        return;
                    }
                }
            }
        };
        self.column_config_popup = Some(ColumnConfigPopup::new(
            Arc::clone(&self.shared_theme),
            &config,
            entries,
            &self.keybindings,
        ));
    }

    fn apply_column_config(&mut self, config: Vec<String>) {
        let settings = Arc::clone(&self.settings_repo);
        match self.active_tab {
            Tab::Trackings => {
                self.trackings_view.column_config = config;
                let value = self.trackings_view.column_config.join(",");
                let _ = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        settings.set("tracking_columns", &value).await
                    })
                });
                self.rebuild_trackings_table();
            }
            Tab::Tasks => {
                self.tasks_view.column_config = config;
                let value = self.tasks_view.column_config.join(",");
                let _ = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        settings.set("tree_columns", &value).await
                    })
                });
                self.spawn_load();
            }
            Tab::Content(idx) => {
                let Some(cv) = self.content_view_mut(idx) else { return };
                if !cv.apply_column_config(config) {
                    return;
                }
                // One JSON settings row per tab holds the whole override
                // map (level key → visible column keys); an emptied map
                // deletes the row so a full reset leaves no residue.
                let key = format!("content_columns:{}", cv.tab_name);
                let value = serde_json::to_string(cv.column_overrides()).unwrap_or_default();
                let empty = cv.column_overrides().is_empty();
                let _ = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        if empty {
                            settings.delete(&key).await
                        } else {
                            settings.set(&key, &value).await
                        }
                    })
                });
            }
        }
    }

    fn save_tracking_grouping_by_label(&self, label: &str) {
        let settings = Arc::clone(&self.settings_repo);
        let value = label.to_string();
        let _ = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                settings.set("tracking_grouping", &value).await
            })
        });
    }

    pub fn load_tracking_grouping(&mut self) {
        use crate::tabs::trackings_state::TrackingGrouping;
        let settings = Arc::clone(&self.settings_repo);
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                settings.get("tracking_grouping").await
            })
        });
        if let Ok(Some(value)) = result {
            let grouping = TrackingGrouping::ALL.iter()
                .find(|g| g.label() == value)
                .copied()
                .unwrap_or(TrackingGrouping::None);
            let cur = self.trackings_view.table.selected_row();
            let new_idx = self.trackings_view.state.set_grouping(grouping, cur);
            self.rebuild_trackings_table();
            self.trackings_view.table.set_selected(new_idx);
        }
    }

    /// Get the active task table (delegates to TasksView).
    pub fn task_table(&self) -> &DataTable {
        match self.tasks_view.sub_view() {
            TasksSubView::Tree => self.tasks_view.tree_view_table(),
            TasksSubView::List => self.tasks_view.list_view_table(),
        }
    }

    /// Get the active task table mutably.
    pub fn task_table_mut(&mut self) -> &mut DataTable {
        match self.tasks_view.sub_view() {
            TasksSubView::Tree => self.tasks_view.tree_view_table_mut(),
            TasksSubView::List => self.tasks_view.list_view_table_mut(),
        }
    }

    fn active_table_mut(&mut self) -> Option<&mut DataTable> {
        match self.active_tab {
            Tab::Trackings => Some(&mut self.trackings_view.table),
            Tab::Content(idx) => self.content_view_mut(idx)
                .map(|cv| &mut cv.active_pane_mut().table),
            Tab::Tasks => Some(self.task_table_mut()),
        }
    }

    /// Rebuild the task table component.
    pub fn refresh_task_table(&mut self) {
        self.tasks_view.refresh_from_own_state(
            &self.tracked_ids,
            &self.link_refs,
        );
    }

    /// Load column configuration from DB.
    pub fn load_column_config(&mut self) {
        let settings = Arc::clone(&self.settings_repo);
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                settings.get("tree_columns").await
            })
        });
        if let Ok(Some(value)) = result {
            let cols: Vec<String> = value.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            if !cols.is_empty() {
                self.tasks_view.column_config = cols;
            }
        }
        // Load tracking columns.
        let settings = Arc::clone(&self.settings_repo);
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                settings.get("tracking_columns").await
            })
        });
        if let Ok(Some(value)) = result {
            let cols: Vec<String> = value.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            if !cols.is_empty() {
                self.trackings_view.column_config = cols;
            }
        }
        // Load per-content-tab column overrides (one JSON row per tab,
        // mapping level key → visible column keys in order). An unparsable
        // row is ignored — the views then just show their YAML defaults.
        let targets: Vec<(usize, String)> = self
            .content_views_indexed()
            .map(|(i, cv)| (i, cv.tab_name.clone()))
            .collect();
        for (idx, tab_name) in targets {
            let settings = Arc::clone(&self.settings_repo);
            let key = format!("content_columns:{tab_name}");
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current()
                    .block_on(async { settings.get(&key).await })
            });
            if let Ok(Some(value)) = result {
                if let Ok(map) = serde_json::from_str::<
                    std::collections::HashMap<String, Vec<String>>,
                >(&value)
                {
                    if !map.is_empty() {
                        if let Some(cv) = self.content_view_mut(idx) {
                            cv.set_column_overrides(map);
                        }
                    }
                }
            }
        }
    }

    /// Load Tasks sort state from the `settings` table. Empty / missing
    /// entries leave the view at its natural default. The format is a
    /// comma-separated list of `column:direction` pairs.
    pub fn load_tasks_sort(&mut self) {
        use crate::views::SortableView;
        let settings = Arc::clone(&self.settings_repo);
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                settings.get("tasks.sort").await
            })
        });
        if let Ok(Some(value)) = result {
            let sort = parse_sort_state(&value);
            if !sort.is_empty() {
                self.tasks_view.set_current_sort(sort);
            }
        }
    }

    /// Persist the Tasks view's current sort state.
    pub fn save_tasks_sort(&self) {
        use crate::views::SortableView;
        let settings = Arc::clone(&self.settings_repo);
        let value = serialize_sort_state(self.tasks_view.current_sort());
        let _ = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                if value.is_empty() {
                    settings.delete("tasks.sort").await
                } else {
                    settings.set("tasks.sort", &value).await
                }
            })
        });
    }

    /// Pre-fill the saved sort spec for every configured content view
    /// from its adapter's persistence layer. Called once at startup,
    /// before the first content load fires.
    pub fn load_content_sort_states(&mut self) {
        use crate::views::SortableView;
        let entries: Vec<(usize, std::sync::Arc<dyn not_yet_done_content::ContentAdapter>, String)> =
            self.content_views_indexed()
                .filter_map(|(i, cv)| cv.adapter.as_ref().map(|a| (i, Arc::clone(a), cv.query_scope.clone())))
                .collect();
        for (idx, adapter, scope) in entries {
            let res = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    adapter.load_view_sort(&scope).await
                })
            });
            if let Ok(sort) = res {
                if !sort.is_empty() {
                    if let Some(cv) = self.content_view_mut(idx) {
                        SortableView::set_current_sort(cv, sort);
                    }
                }
            }
        }
    }

    /// Persist the current sort state of a content view through its
    /// adapter. Called by the sort-mode handler when the user changes
    /// a column's direction.
    pub fn save_content_sort(&self, view_index: usize) {
        use crate::views::SortableView;
        let Some(cv) = self.content_view(view_index) else { return };
        let Some(adapter) = cv.adapter.as_ref() else { return };
        let adapter = Arc::clone(adapter);
        let scope = cv.query_scope.clone();
        let sort = SortableView::current_sort(cv).to_vec();
        let _ = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                adapter.save_view_sort(&scope, &sort).await
            })
        });
    }

    /// Translate the current [`SortHintPhase`] into a [`HeaderOverlay`]
    /// and push it to the targeted view's table header. Cleared from
    /// the other views so only one view ever shows the overlay.
    fn update_header_overlays(&mut self) {
        use crate::components::sort_header::HeaderOverlay;
        use std::collections::HashMap;

        let (target, overlay) = match &self.sort_hint_phase {
            SortHintPhase::Off => (None, HeaderOverlay::None),
            SortHintPhase::WaitingForColumn { target, labels, columns, input } => {
                // Only labels still matching the typed prefix become candidates.
                let mut map: HashMap<String, String> = HashMap::new();
                for (col_idx, label) in labels {
                    if !label.starts_with(input.as_str()) {
                        continue;
                    }
                    if let Some(col) = columns.get(*col_idx) {
                        map.insert(col.key.clone(), label.clone());
                    }
                }
                let input_len = input.chars().count();
                (Some(*target), HeaderOverlay::PickColumn { labels: map, input_len })
            }
            SortHintPhase::WaitingForDirection { target, column_id, .. } => {
                (Some(*target), HeaderOverlay::PickDirection { column_key: column_id.clone() })
            }
        };

        // Clear all overlays first, then set the target.
        self.tasks_view.header_overlay = HeaderOverlay::None;
        for cv in self.content_views_iter_mut() {
            cv.header_overlay = HeaderOverlay::None;
        }
        if let Some(target) = target {
            match target {
                SortTarget::Tasks => self.tasks_view.header_overlay = overlay,
                SortTarget::Content(idx) => {
                    if let Some(cv) = self.content_view_mut(idx) {
                        cv.header_overlay = overlay;
                    }
                }
            }
        }
    }

    // ── Sort-hint mode ─────────────────────────────────────────────

    /// Enter sort-hint mode for the active tab. Builds the column → label
    /// map from the active view's [`SortableView::sortable_columns`].
    /// No-op for tabs that don't support sort (Trackings) or views that
    /// expose no sortable columns.
    pub fn enter_sort_hint_mode(&mut self) {
        use crate::views::SortableView;
        let (target, columns) = match self.active_tab {
            Tab::Tasks => (SortTarget::Tasks, SortableView::sortable_columns(&self.tasks_view)),
            Tab::Content(idx) => match self.content_view(idx) {
                Some(cv) => (SortTarget::Content(idx), SortableView::sortable_columns(cv)),
                None => return,
            },
            Tab::Trackings => return,
        };
        if columns.is_empty() {
            self.notify("No sortable columns".to_string());
            return;
        }
        let labels = generate_sort_labels(columns.len());
        let labels: Vec<(usize, String)> = labels.into_iter().enumerate().collect();
        self.sort_hint_phase = SortHintPhase::WaitingForColumn {
            target,
            labels,
            columns,
            input: String::new(),
        };
    }

    pub fn cancel_sort_hint_mode(&mut self) {
        self.sort_hint_phase = SortHintPhase::Off;
    }

    /// Feed a key to the sort-hint state machine. Always handled while
    /// `sort_hint_phase != Off`. Esc cancels.
    pub fn sort_hint_handle_key(&mut self, key: &str) {
        if key == "esc" {
            self.cancel_sort_hint_mode();
            return;
        }
        let current = std::mem::replace(&mut self.sort_hint_phase, SortHintPhase::Off);
        match current {
            SortHintPhase::Off => {}
            SortHintPhase::WaitingForColumn { target, labels, columns, mut input } => {
                if key.chars().count() != 1 {
                    self.sort_hint_phase = SortHintPhase::WaitingForColumn {
                        target, labels, columns, input,
                    };
                    return;
                }
                let ch = key.chars().next().unwrap();
                input.push(ch);
                let still_matching: usize = labels.iter()
                    .filter(|(_, l)| l.starts_with(&input))
                    .count();
                if still_matching == 0 {
                    self.notify(format!("No sort column for '{}'", input));
                    return;
                }
                if let Some((col_idx, _)) = labels.iter().find(|(_, l)| *l == input) {
                    let col = &columns[*col_idx];
                    self.sort_hint_phase = SortHintPhase::WaitingForDirection {
                        target,
                        column_id: col.key.clone(),
                        column_name: col.label.clone(),
                    };
                    return;
                }
                self.sort_hint_phase = SortHintPhase::WaitingForColumn {
                    target, labels, columns, input,
                };
            }
            SortHintPhase::WaitingForDirection { target, column_id, column_name } => {
                let action = match key {
                    "+" | "a" => Some(SortAction::Asc),
                    "-" | "d" => Some(SortAction::Desc),
                    "0" | "c" => Some(SortAction::Clear),
                    _ => None,
                };
                match action {
                    Some(act) => self.apply_sort(target, &column_id, act, &column_name),
                    None => {
                        self.sort_hint_phase = SortHintPhase::WaitingForDirection {
                            target, column_id, column_name,
                        };
                    }
                }
            }
        }
    }

    /// Apply a sort change additively: existing sort keys on other
    /// columns are preserved, the chosen column is added/updated/removed
    /// (depending on `action`).
    fn apply_sort(
        &mut self,
        target: SortTarget,
        column_id: &str,
        action: SortAction,
        column_name: &str,
    ) {
        use crate::views::SortableView;
        use not_yet_done_content::{SortDirection, SortKey};

        let current: Vec<SortKey> = match target {
            SortTarget::Tasks => SortableView::current_sort(&self.tasks_view).to_vec(),
            SortTarget::Content(idx) => self
                .content_view(idx)
                .map(|cv| SortableView::current_sort(cv).to_vec())
                .unwrap_or_default(),
        };

        let mut new_sort: Vec<SortKey> = current
            .into_iter()
            .filter(|k| k.column != column_id)
            .collect();
        let descr = match action {
            SortAction::Asc => {
                new_sort.push(SortKey {
                    column: column_id.to_string(),
                    direction: SortDirection::Asc,
                });
                format!("Sort by {} (asc)", column_name)
            }
            SortAction::Desc => {
                new_sort.push(SortKey {
                    column: column_id.to_string(),
                    direction: SortDirection::Desc,
                });
                format!("Sort by {} (desc)", column_name)
            }
            SortAction::Clear => format!("Sort cleared on {}", column_name),
        };

        match target {
            SortTarget::Tasks => {
                let changed = SortableView::set_current_sort(&mut self.tasks_view, new_sort);
                if changed {
                    self.refresh_task_table();
                    self.save_tasks_sort();
                }
            }
            SortTarget::Content(idx) => {
                let changed = self
                    .content_view_mut(idx)
                    .map(|cv| SortableView::set_current_sort(cv, new_sort))
                    .unwrap_or(false);
                if changed {
                    self.save_content_sort(idx);
                    let pane_id = self
                        .content_view(idx)
                        .map(|cv| cv.active_pane_id())
                        .unwrap_or_default();
                    self.spawn_content_load(idx, pane_id);
                }
            }
        }
        self.notify(descr);
    }

    // ── Saved queries / favorites ──────────────────────────────────

    pub fn load_saved_queries(&mut self) {
        let repo = Arc::clone(&self.saved_query_repo);
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                repo.list_by_scope("task").await
            })
        });
        if let Ok(models) = result {
            self.tasks_view.favorites = models.into_iter().map(SavedQuery::from_db).collect();
        }
        let repo = Arc::clone(&self.saved_query_repo);
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                repo.list_by_scope("tracking").await
            })
        });
        if let Ok(models) = result {
            self.trackings_view.favorites = models.into_iter().map(SavedQuery::from_db).collect();
        }
        let settings = Arc::clone(&self.settings_repo);
        let (task_default, tracking_default) = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                (
                    settings.get("default_query:task").await.ok().flatten(),
                    settings.get("default_query:tracking").await.ok().flatten(),
                )
            })
        });
        self.tasks_view.default_query_name = task_default;
        self.trackings_view.default_query_name = tracking_default;
    }

    /// Write or delete the `default_query:{scope}` settings row. `None`
    /// clears the default.
    fn persist_default_query(&self, scope: &str, name: Option<&str>) {
        let repo = Arc::clone(&self.settings_repo);
        let key = format!("default_query:{scope}");
        let _ = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                match name {
                    Some(n) => repo.set(&key, n).await,
                    None => repo.delete(&key).await,
                }
            })
        });
    }

    /// Toggle the default saved query for the native `task`/`tracking`
    /// scopes. Selecting the current default clears it; the default is
    /// applied automatically on app start (it beats the last-active
    /// filter restore).
    fn set_default_saved_query(&mut self, scope: &str, name: &str) {
        let current = match scope {
            "tracking" => self.trackings_view.default_query_name.as_deref(),
            _ => self.tasks_view.default_query_name.as_deref(),
        };
        let new = if current == Some(name) { None } else { Some(name.to_string()) };
        self.persist_default_query(scope, new.as_deref());
        match scope {
            "tracking" => self.trackings_view.default_query_name = new.clone(),
            _ => self.tasks_view.default_query_name = new.clone(),
        }
        match new {
            Some(n) => self.notify(format!("Default query: {n}")),
            None => self.notify("Default query cleared".to_string()),
        }
    }

    /// Content-tab counterpart of [`Self::set_default_saved_query`] —
    /// keyed on the view's `query_scope`.
    fn set_default_content_query(&mut self, view_index: usize, name: &str) {
        let Some(cv) = self.content_view(view_index) else { return };
        let scope = cv.query_scope.clone();
        let current = cv.default_saved_query.clone();
        let new = if current.as_deref() == Some(name) { None } else { Some(name.to_string()) };
        self.persist_default_query(&scope, new.as_deref());
        if let Some(cv) = self.content_view_mut(view_index) {
            cv.default_saved_query = new.clone();
        }
        match new {
            Some(n) => self.notify(format!("Default query: {n}")),
            None => self.notify("Default query cleared".to_string()),
        }
    }

    /// Apply a saved query (YAML content) to tasks_view or trackings_view
    /// based on scope. Routes to the existing apply_*_query_filter methods.
    fn apply_saved_query(&mut self, scope: &str, content: &str) -> EditorRequest {
        match scope {
            "tracking" => self.apply_tracking_query_filter(content),
            _ => self.apply_query_filter(content),
        }
        EditorRequest::None
    }

    /// Delete a saved query from DB and refresh in-memory favorites for all scopes.
    fn delete_saved_query(&mut self, scope: &str, name: &str) {
        let repo = Arc::clone(&self.saved_query_repo);
        let scope_owned = scope.to_string();
        let name_owned = name.to_string();
        let _ = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                if let Ok(Some(model)) = repo.find_by_scope_and_name(&scope_owned, &name_owned).await {
                    repo.delete(model.id).await
                } else {
                    Ok(())
                }
            })
        });
        self.load_saved_queries();
        self.notify(format!("Deleted query '{name}'"));
    }

    /// Save a saved query (without shortcut) and prompt the user to press a
    /// shortcut key, which is then captured by `awaiting_favorite_shortcut`.
    fn prompt_saved_query_shortcut(&mut self, scope: String, name: String, query: String) {
        let repo = Arc::clone(&self.saved_query_repo);
        let scope_for_save = scope.clone();
        let name_for_save = name.clone();
        let query_for_save = query.clone();
        let _ = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                repo.upsert(&scope_for_save, &name_for_save, &query_for_save, None).await
            })
        });
        self.load_saved_queries();
        self.modal_message = Some(format!("Press a shortcut key for '{}'\n\nEsc to cancel", name));
        self.awaiting_favorite_shortcut = Some((scope, name, query));
    }

    /// Load DB saved queries for all content views and merge with YAML defaults.
    pub fn load_content_saved_queries(&mut self) {
        for i in 0..self.content_views.len() {
            self.reload_content_saved_queries(i);
        }
    }

    /// Reload saved queries for a single content view.
    ///
    /// Bodies come from the adapter's `SavedQueryStore` (filesystem),
    /// shortcuts from the `query_shortcut` table scoped to this view.
    /// An adapter without a store (Postgres, plus any adapter that
    /// opts out) yields an empty list — view-YAML `default:` is the
    /// only fallback then.
    fn reload_content_saved_queries(&mut self, view_index: usize) {
        let cv = match self.content_view(view_index) {
            Some(cv) => cv,
            None => return,
        };
        let scope = cv.query_scope.clone();
        let adapter = cv.adapter.clone();
        let shortcut_repo = Arc::clone(&self.query_shortcut_repo);
        let settings_repo = Arc::clone(&self.settings_repo);

        let (entries, default_query): (Vec<(String, String, Option<String>)>, Option<String>) =
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    let default_query = settings_repo
                        .get(&format!("default_query:{scope}"))
                        .await
                        .ok()
                        .flatten();
                    let Some(adapter) = adapter.as_ref() else { return (Vec::new(), default_query) };
                    let Some(store) = adapter.saved_query_store() else { return (Vec::new(), default_query) };
                    let names = match store.list().await {
                        Ok(n) => n,
                        Err(_) => return (Vec::new(), default_query),
                    };
                    let shortcut_map: std::collections::HashMap<String, String> = shortcut_repo
                        .list_by_scope(&scope)
                        .await
                        .unwrap_or_default()
                        .into_iter()
                        .map(|m| (m.name, m.shortcut))
                        .collect();
                    let mut out = Vec::with_capacity(names.len());
                    for name in names {
                        let body = match store.load(&name).await {
                            Ok(b) => b,
                            Err(_) => continue,
                        };
                        let shortcut = shortcut_map.get(&name).cloned();
                        out.push((name, body, shortcut));
                    }
                    (out, default_query)
                })
            });

        // Load-time guard: `query_shortcut` rows written externally (or
        // predating a config change) can collide with keys that are now
        // bound — the shortcut claim would silently shadow them at the
        // view layer. The set-time gate can't catch those, so flag them
        // here. The shortcut stays active (the row is the user's own
        // data); the notification names the shadowed binding so they
        // can rebind via the query menu.
        let warnings: Vec<String> = match self.content_view(view_index) {
            Some(cv) => {
                let mut bound: Vec<(String, String)> = Vec::new();
                let mut warnings = Vec::new();
                for (name, _, shortcut) in &entries {
                    let Some(sc) = shortcut else { continue };
                    if let Some(conflict) = crate::keymap::saved_query_shortcut_conflict(
                        &cv.tab_name,
                        &cv.view_defs,
                        &self.keybindings,
                        name,
                        sc,
                        &bound,
                    ) {
                        warnings.push(format!(
                            "{}: saved-query shortcut [{}] ('{}') shadows {} — rebind it via the query menu",
                            cv.tab_name, sc, name, conflict
                        ));
                    }
                    bound.push((name.clone(), sc.clone()));
                }
                warnings
            }
            None => Vec::new(),
        };

        if let Some(cv) = self.content_view_mut(view_index) {
            cv.merge_saved_queries(entries);
            cv.default_saved_query = default_query;
        }
        for w in warnings {
            if self.warned_saved_query_conflicts.insert(w.clone()) {
                self.notify(w);
            }
        }
    }

    /// Populate `ContentView::postgres_table_shortcuts` for the
    /// currently-focused Postgres table (SQ-8d). Cache miss → one
    /// indexed `query_shortcut` lookup keyed on the table's NodeRef
    /// scope. Cache hits short-circuit. Called once per content-tab
    /// keypress; insulated from non-Postgres adapters by the cheap
    /// `adapter_type()`/`target_postgres_table_node_id()` checks.
    pub fn ensure_postgres_table_shortcuts_loaded(&mut self, view_index: usize) {
        let Some(cv) = self.content_view(view_index) else { return };
        let Some(adapter) = cv.adapter.as_ref() else { return };
        if adapter.adapter_type() != "postgres" {
            return;
        }
        let Some(table_node_id) = cv.target_postgres_table_node_id() else { return };
        if cv.postgres_table_shortcuts.contains_key(&table_node_id) {
            return;
        }
        let instance_id = adapter.instance_id().to_string();
        let Some((db, schema, table)) =
            crate::views::content_view::parse_postgres_table_node_id(&table_node_id)
        else {
            // Adapter id form changed — leave the cache empty so the
            // claim path quietly stays disabled until the parsers catch up.
            return;
        };
        let scope = format!(
            "postgres/{instance_id}/{db}/schemas/{schema}/tables/{table}",
        );
        let repo = Arc::clone(&self.query_shortcut_repo);
        let entries: Vec<(String, String)> = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                repo.list_by_scope(&scope)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|m| (m.name, m.shortcut))
                    .collect()
            })
        });
        if let Some(cv) = self.content_view_mut(view_index) {
            cv.postgres_table_shortcuts.insert(table_node_id, entries);
        }
    }

    fn active_favorites(&self) -> &[SavedQuery] {
        if self.active_tab == Tab::Trackings {
            &self.trackings_view.favorites
        } else {
            &self.tasks_view.favorites
        }
    }

    /// Conflict description for binding `shortcut` to the saved query
    /// `name` in `scope`, or `None` when the key is free. The native
    /// scopes keep the legacy domain (global + tasks bindings + native
    /// favorites); content-view scopes route through the keymap-based
    /// check so a saved-query shortcut can never shadow any key active
    /// in its tab (the `j`-shadows-list-navigation class of bug).
    fn favorite_shortcut_conflict(&self, scope: &str, name: &str, shortcut: &str) -> Option<String> {
        if scope == "tracking" || scope == "task" {
            return self
                .is_shortcut_taken(shortcut)
                .then(|| "an existing key or favorite".to_string());
        }
        self.content_views_indexed()
            .find(|(_, cv)| cv.query_scope == scope)
            .and_then(|(_, cv)| {
                cv.saved_query_shortcut_conflict(&self.keybindings, name, shortcut)
            })
    }

    fn is_shortcut_taken(&self, shortcut: &str) -> bool {
        if self.keybindings.global.bindings.values().any(|b| b.matches(shortcut)) { return true; }
        if self.keybindings.tasks.bindings.values().any(|b| b.matches(shortcut)) { return true; }
        if self.tasks_view.favorites.iter().any(|f| f.shortcut.as_deref() == Some(shortcut)) { return true; }
        if self.trackings_view.favorites.iter().any(|f| f.shortcut.as_deref() == Some(shortcut)) { return true; }
        false
    }

    fn add_favorite(&mut self, scope: &str, name: String, shortcut: String, query: String) {
        if scope == "tracking" || scope == "task" {
            let repo = Arc::clone(&self.saved_query_repo);
            let scope_owned = scope.to_string();
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    repo.upsert(&scope_owned, &name, &query, Some(&shortcut)).await
                })
            });
            if let Ok(model) = result {
                let sq = SavedQuery::from_db(model);
                let favs = if scope == "tracking" {
                    &mut self.trackings_view.favorites
                } else {
                    &mut self.tasks_view.favorites
                };
                if let Some(existing) = favs.iter_mut().find(|f| f.name == sq.name) {
                    *existing = sq;
                } else {
                    favs.push(sq);
                }
            }
        } else {
            // Content view scope — body in adapter store, shortcut in DB.
            let target_idx = self
                .content_views_indexed()
                .find(|(_, cv)| cv.query_scope == scope)
                .map(|(idx, _)| idx);
            if let Some(idx) = target_idx {
                self.save_content_query_body(idx, &name, &query);
                let shortcut_repo = Arc::clone(&self.query_shortcut_repo);
                let scope_owned = scope.to_string();
                let name_owned = name.clone();
                let shortcut_owned = shortcut.clone();
                let _ = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        shortcut_repo.set(&scope_owned, &name_owned, &shortcut_owned).await
                    })
                });
                self.reload_content_saved_queries(idx);
            }
        }
    }

    /// Write `body` to the active adapter's `SavedQueryStore` for the
    /// view at `view_index`. No-op if the adapter doesn't expose a
    /// store (e.g. Postgres).
    fn save_content_query_body(&self, view_index: usize, name: &str, body: &str) {
        let Some(cv) = self.content_view(view_index) else { return };
        let adapter = cv.adapter.clone();
        let name_owned = name.to_string();
        let body_owned = body.to_string();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let Some(adapter) = adapter.as_ref() else { return };
                let Some(store) = adapter.saved_query_store() else { return };
                let _ = store.save(&name_owned, &body_owned).await;
            })
        });
    }

    fn delete_content_query(&self, view_index: usize, scope: &str, name: &str) {
        let Some(cv) = self.content_view(view_index) else { return };
        let adapter = cv.adapter.clone();
        let shortcut_repo = Arc::clone(&self.query_shortcut_repo);
        let scope_owned = scope.to_string();
        let name_owned = name.to_string();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                if let Some(adapter) = adapter.as_ref() {
                    if let Some(store) = adapter.saved_query_store() {
                        let _ = store.delete(&name_owned).await;
                    }
                }
                let _ = shortcut_repo.unset(&scope_owned, &name_owned).await;
            })
        });
    }

    fn rename_content_query(&self, view_index: usize, scope: &str, old_name: &str, new_name: &str) {
        let Some(cv) = self.content_view(view_index) else { return };
        let adapter = cv.adapter.clone();
        let shortcut_repo = Arc::clone(&self.query_shortcut_repo);
        let scope_owned = scope.to_string();
        let old_owned = old_name.to_string();
        let new_owned = new_name.to_string();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                if let Some(adapter) = adapter.as_ref() {
                    if let Some(store) = adapter.saved_query_store() {
                        if let Ok(body) = store.load(&old_owned).await {
                            if store.save(&new_owned, &body).await.is_ok() {
                                let _ = store.delete(&old_owned).await;
                            }
                        }
                    }
                }
                let _ = shortcut_repo.rename(&scope_owned, &old_owned, &new_owned).await;
            })
        });
    }

    /// Update the query of any saved query matching the given name.
    pub fn update_favorite_json(&mut self, scope: &str, filter_name: &str, new_query: &str) {
        let favs = if scope == "tracking" {
            &mut self.trackings_view.favorites
        } else {
            &mut self.tasks_view.favorites
        };
        let mut changed = false;
        for fav in favs.iter_mut() {
            if fav.name == filter_name && fav.query != new_query {
                fav.query = new_query.to_string();
                changed = true;
                // Persist to DB.
                if let Some(id) = fav.id {
                    let repo = Arc::clone(&self.saved_query_repo);
                    let name = fav.name.clone();
                    let query = new_query.to_string();
                    let shortcut = fav.shortcut.clone();
                    let scope_owned = scope.to_string();
                    let _ = tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(async {
                            repo.upsert(&scope_owned, &name, &query, shortcut.as_deref()).await
                        })
                    });
                }
            }
        }
        let _ = changed; // suppress unused warning
    }

    fn try_activate_favorite(&mut self, key: &str) -> bool {
        let favs = self.active_favorites().to_vec();
        for fav in &favs {
            if fav.shortcut.as_deref() == Some(key) {
                if self.active_tab == Tab::Trackings {
                    self.apply_tracking_query_filter(&fav.query);
                    self.trackings_view.active_filter_json = Some(fav.query.clone());
                    self.trackings_view.active_filter_name = Some(fav.name.clone());
                } else {
                    self.apply_query_filter(&fav.query);
                    self.tasks_view.active_filter_json = Some(fav.query.clone());
                    self.tasks_view.active_filter_name = Some(fav.name.clone());
                }
                return true;
            }
        }
        false
    }

    fn toggle_tracking_from_trackings_view(&mut self) {
        let selected = self.trackings_view.table.selected_row();
        let active = self.trackings_view.state.is_active_at(selected);

        if self.trackings_view.state.sub_view != crate::tabs::TrackingsSubView::Normal {
            // Condensed/Tree mode: toggle by task_id.
            let Some(task_id) = self.trackings_view.state.task_id_at(selected) else {
                self.notify("No task selected".to_string());
                return;
            };
            self.toggle_tracking_for_task(task_id, active);
            return;
        }

        // Normal mode: if active, stop the specific tracking; otherwise start new.
        let Some(task_id) = self.trackings_view.state.task_id_at(selected) else {
            self.notify("No tracking selected".to_string());
            return;
        };

        if active {
            // Stop this specific tracking.
            let Some(tracking_id) = self.trackings_view.state.tracking_id_at(selected) else {
                return;
            };
            let repo = Arc::clone(&self.tracking_repo);
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    repo.stop(tracking_id, chrono::Utc::now()).await
                })
            });
            match result {
                Ok(_) => {
                    self.notify("Tracking stopped".to_string());
                    self.refresh_tracked_ids();
                    self.spawn_load_trackings();
                }
                Err(e) => self.notify_error(format!("Tracking error: {e}")),
            }
        } else {
            self.toggle_tracking_for_task(task_id, false);
        }
    }

    /// Toggle tracking for a task by task_id. Used from condensed mode and as fallback.
    fn toggle_tracking_for_task(&mut self, task_id: Uuid, currently_active: bool) {
        let repo = Arc::clone(&self.tracking_repo);
        let allow_parallel = self.config.tracking.allow_parallel;

        if currently_active {
            // Stop active tracking for this task.
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    if let Some(t) = repo.find_active_for_task(task_id).await? {
                        repo.stop(t.id, chrono::Utc::now()).await?;
                    }
                    Ok::<_, anyhow::Error>(())
                })
            });
            match result {
                Ok(_) => {
                    self.notify("Tracking stopped".to_string());
                    self.refresh_tracked_ids();
                    self.spawn_load_trackings();
                }
                Err(e) => self.notify_error(format!("Tracking error: {e}")),
            }
        } else {
            // Start a new tracking for this task.
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    if !allow_parallel {
                        let all_active = repo.find_all_active().await?;
                        for t in all_active {
                            repo.stop(t.id, chrono::Utc::now()).await?;
                        }
                    }
                    repo.insert(task_id, chrono::Utc::now(), None).await
                })
            });
            match result {
                Ok(_) => {
                    self.notify("Tracking started".to_string());
                    self.refresh_tracked_ids();
                    self.spawn_load_trackings();
                }
                Err(e) => self.notify_error(format!("Tracking error: {e}")),
            }
        }
    }

    // ── Command line (:) ──────────────────────────────────────────────

    fn execute_cmdline(&mut self, cmd: &str) {
        let args: Vec<&str> = cmd.trim().split_whitespace().collect();
        if args.is_empty() { return; }

        // Adapter-level commands routed in-process. They target whichever
        // content tab is currently active and have no CLI counterpart —
        // the orchestrator state lives in the running TUI's adapters.
        if args[0] == "linkprune" {
            if args.len() > 1 {
                self.modal_message = Some(":linkprune takes no arguments".to_string());
                return;
            }
            self.link_prune_command();
            return;
        }

        if args[0] == "config" {
            let prefilter = args.get(1).map(|s| s.to_string());
            self.open_config_picker(prefilter.as_deref());
            return;
        }

        if args[0] == "tag" {
            if args.len() > 1 {
                self.modal_message = Some(":tag takes no arguments".to_string());
                return;
            }
            self.open_tag_menu();
            return;
        }

        if args[0] == "script" {
            if args.len() > 1 {
                self.modal_message = Some(":script takes no arguments".to_string());
                return;
            }
            self.open_script_menu_from_current_tab();
            return;
        }

        if args[0] == "dismiss-notifications" {
            if args.len() > 1 {
                self.modal_message =
                    Some(":dismiss-notifications takes no arguments".to_string());
                return;
            }
            self.dismiss_notifications();
            return;
        }

        if args[0] == "cut-node" {
            if args.len() > 1 {
                self.modal_message = Some(":cut-node takes no arguments".to_string());
                return;
            }
            self.cut_node_command();
            return;
        }

        if args[0] == "paste-node" {
            if args.len() > 1 {
                self.modal_message = Some(":paste-node takes no arguments".to_string());
                return;
            }
            self.paste_node_command();
            return;
        }

        if args[0] == "jump" {
            if args.len() != 2 {
                self.modal_message =
                    Some(":jump expects one argument, e.g. :jump Tasks:tree".to_string());
                return;
            }
            self.jump_command(args[1]);
            return;
        }

        if args[0] == "focus-task" {
            // Everything after the command name is the path (may contain
            // spaces — name segments are split on `/` only).
            let rest = cmd.trim().splitn(2, char::is_whitespace).nth(1).unwrap_or("");
            if rest.trim().is_empty() {
                self.modal_message =
                    Some(":focus-task expects a /-separated path".to_string());
                return;
            }
            self.focus_task_command(rest.trim());
            return;
        }

        if args[0] == "focus-node" {
            // Everything after the command name is target + path; path may
            // contain `|` and other shell-active chars, so we hand the whole
            // rest off to `focus_node_command` unsplit.
            let rest = cmd.trim().splitn(2, char::is_whitespace).nth(1).unwrap_or("");
            if rest.trim().is_empty() {
                self.modal_message = Some(
                    ":focus-node expects <Tab>[:<view>] /col|pattern".to_string(),
                );
                return;
            }
            self.focus_node_command(rest.trim());
            return;
        }

        if args[0] == "tree-find" {
            // Everything after the command name is target + query; the
            // tab name may be quoted and the query may contain `:` etc,
            // so hand the whole rest off to `tree_find_command` unsplit.
            let rest = cmd.trim().splitn(2, char::is_whitespace).nth(1).unwrap_or("");
            if rest.trim().is_empty() {
                self.modal_message = Some(
                    ":tree-find expects <Tab>[:<view>] <query>".to_string(),
                );
                return;
            }
            self.tree_find_command(rest.trim());
            return;
        }

        if args[0] == "reload-tasks" {
            if args.len() > 1 {
                self.modal_message =
                    Some(":reload-tasks takes no arguments".to_string());
                return;
            }
            self.reload_tasks_command();
            return;
        }

        if args[0] == "db-script-new" {
            let rest = cmd.trim().splitn(2, char::is_whitespace).nth(1).unwrap_or("");
            self.db_script_new_command(rest.trim());
            return;
        }

        // DSF-5: `:db-script <sub>` namespace (mirrors `:query <sub>`).
        // Subcommands target the focused content pane's selected row;
        // see `db_script_command` for the per-subcommand contract.
        if args[0] == "db-script" {
            let sub = args.get(1).copied().unwrap_or("");
            let rest = cmd.trim()
                .splitn(3, char::is_whitespace)
                .nth(2)
                .unwrap_or("")
                .trim();
            self.db_script_command(sub, rest);
            return;
        }

        if args[0] == "query" {
            // `:query` is a namespace: `apply` activates a saved query
            // (read), `edit`/`new`/`delete` operate on the adapter's
            // saved-query store (write). The unsplit remainder after
            // the subcommand is the name (may contain whitespace).
            let sub = args.get(1).copied().unwrap_or("");
            let rest = cmd.trim()
                .splitn(3, char::is_whitespace)
                .nth(2)
                .unwrap_or("")
                .trim();
            match sub {
                "apply" => {
                    if rest.is_empty() {
                        self.modal_message = Some(
                            ":query apply expects [-t <Tab>[:<view>]] <name>".to_string(),
                        );
                        return;
                    }
                    self.query_apply_command(rest);
                }
                "edit" => {
                    if rest.is_empty() {
                        self.modal_message =
                            Some(":query edit expects <name>".to_string());
                        return;
                    }
                    self.query_edit_command(rest);
                }
                "new" => {
                    if rest.is_empty() {
                        self.modal_message =
                            Some(":query new expects <name>".to_string());
                        return;
                    }
                    self.query_new_command(rest);
                }
                "delete" => {
                    if rest.is_empty() {
                        self.modal_message =
                            Some(":query delete expects <name>".to_string());
                        return;
                    }
                    self.query_delete_command(rest);
                }
                "" => {
                    self.modal_message = Some(
                        ":query expects a subcommand (apply | edit | new | delete)"
                            .to_string(),
                    );
                }
                other => {
                    self.modal_message = Some(format!(
                        ":query — unknown subcommand '{other}' (apply | edit | new | delete)"
                    ));
                }
            }
            return;
        }

        if let Some(kind) = match args[0] {
            "invalidate-session" => Some(AuthInvalidate::Session),
            "invalidate-credentials" => Some(AuthInvalidate::Credentials),
            _ => None,
        } {
            if args.len() > 1 {
                self.modal_message =
                    Some(format!(":{} takes no arguments", args[0]));
                return;
            }
            match self.active_tab {
                Tab::Content(view_index) => {
                    self.spawn_invalidate_auth(view_index, kind);
                }
                _ => {
                    self.modal_message =
                        Some(format!(":{} only works on a content tab", args[0]));
                }
            }
            return;
        }

        let result = std::process::Command::new("not-yet-done-cli")
            .args(&args)
            .output();

        match result {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let msg = if output.status.success() {
                    if stdout.is_empty() { format!(":{cmd} — done") } else { stdout }
                } else if !stderr.is_empty() {
                    stderr
                } else {
                    format!(":{cmd} — exit code {}", output.status.code().unwrap_or(-1))
                };
                self.modal_message = Some(msg);
            }
            Err(e) => {
                self.modal_message = Some(format!("Failed to run '{cmd}': {e}"));
            }
        }
    }

    fn open_notes_for_selected_task(&mut self) -> EditorRequest {
        let Some(task) = self.selected_task() else {
            self.notify("No task selected".to_string());
            return EditorRequest::None;
        };
        let session = crate::edit_session::TaskNotesSession::new(
            task,
            self.tasks_view.state.task_rows.clone(),
        );
        self.open_session(Box::new(session))
    }

    fn toggle_tracking(&mut self) {
        let Some(task_id) = self.selected_task_id() else {
            self.notify("No task selected".to_string());
            return;
        };

        let repo = Arc::clone(&self.tracking_repo);
        let allow_parallel = self.config.tracking.allow_parallel;

        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                // Check if task is currently tracked.
                let active = repo.find_active_for_task(task_id).await?;
                if let Some(tracking) = active {
                    // Stop tracking.
                    repo.stop(tracking.id, chrono::Utc::now()).await?;
                    Ok::<String, not_yet_done_core::error::AppError>("Tracking stopped".into())
                } else {
                    // Start tracking. If !allow_parallel, stop others first.
                    if !allow_parallel {
                        let all_active = repo.find_all_active().await?;
                        for t in all_active {
                            repo.stop(t.id, chrono::Utc::now()).await?;
                        }
                    }
                    repo.insert(task_id, chrono::Utc::now(), None).await?;
                    Ok("Tracking started".into())
                }
            })
        });

        match result {
            Ok(msg) => {
                self.notify(msg);
                self.refresh_tracked_ids();
                self.spawn_load_trackings();
            }
            Err(e) => self.notify_error(format!("Tracking error: {e}")),
        }
    }

    fn delete_selected_task(&mut self) {
        let Some(task_id) = self.selected_task_id() else {
            self.notify("No task selected".to_string());
            return;
        };

        let has_children = self.tasks_view.state.task_rows.iter()
            .any(|t| t.parent_id == Some(task_id) && !t.deleted);

        let task_desc = self.tasks_view.state.task_rows.iter()
            .find(|t| t.id == task_id)
            .map(|t| t.description.clone())
            .unwrap_or_default();

        if has_children {
            let msg = format!("Delete task '{}' and all children? (y/n)", task_desc);
            self.modal_message = Some(msg.clone());
            self.pending_confirmation = Some((msg, PendingConfirmation::DeleteTaskRecursive(task_id)));
        } else {
            let msg = format!("Delete task '{}'? (y/n)", task_desc);
            self.modal_message = Some(msg.clone());
            self.pending_confirmation = Some((msg, PendingConfirmation::DeleteTask(task_id)));
        }
    }

    fn delete_selected_tracking(&mut self) {
        let selected = self.trackings_view.table.selected_row();
        let Some(tracking_id) = self.trackings_view.state.tracking_id_at(selected) else {
            self.notify("No tracking selected".to_string());
            return;
        };

        let task_desc = self.trackings_view.state.task_description_at(selected)
            .unwrap_or_default();

        let msg = format!("Delete tracking for '{}'? (y/n)", task_desc);
        self.modal_message = Some(msg.clone());
        self.pending_confirmation = Some((msg, PendingConfirmation::DeleteTracking(tracking_id)));
    }

    fn execute_confirmation(&mut self, confirmation: PendingConfirmation) {
        match confirmation {
            PendingConfirmation::DeleteTask(task_id) | PendingConfirmation::DeleteTaskRecursive(task_id) => {
                let service = Arc::clone(&self.task_service);
                let result = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        service.delete_task_recursive(task_id).await
                    })
                });
                match result {
                    Ok(count) => {
                        let subtree_ids: Vec<_> = self.tasks_view.state.task_rows.iter()
                            .filter(|t| t.id == task_id || t.parent_id == Some(task_id))
                            .cloned().collect();
                        for task in &subtree_ids {
                            crate::notes::mark_notes_deleted(task, &self.tasks_view.state.task_rows);
                        }
                        if count > 1 {
                            self.notify(format!("Deleted subtree ({count} tasks)"));
                        } else {
                            self.notify("Task deleted".to_string());
                        }
                        self.spawn_load();
                    }
                    Err(e) => self.notify_error(format!("Delete error: {e}")),
                }
            }
            PendingConfirmation::DeleteTracking(tracking_id) => {
                let repo = Arc::clone(&self.tracking_repo);
                let result = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        repo.soft_delete_keeping_times(tracking_id).await
                    })
                });
                match result {
                    Ok(_) => {
                        self.notify("Tracking deleted".to_string());
                        self.spawn_load_trackings();
                    }
                    Err(e) => self.notify_error(format!("Delete error: {e}")),
                }
            }
            PendingConfirmation::DeleteStaleLink(link_id) => {
                let repo = Arc::clone(&self.link_repo);
                let result = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current()
                        .block_on(async move { repo.delete(link_id).await })
                });
                match result {
                    Ok(()) => {
                        self.reload_link_refs();
                        self.notify("Stale link deleted".to_string());
                    }
                    Err(e) => self.notify_error(format!("Delete error: {e}")),
                }
            }
            PendingConfirmation::DeleteAdapterDbScript {
                view_index,
                pane_id,
                database,
                script,
            } => {
                self.delete_adapter_db_script_now(view_index, pane_id, database, script);
            }
            PendingConfirmation::DeleteAdapterDbScriptDir {
                view_index,
                pane_id,
                database,
                rel_path,
            } => {
                self.delete_adapter_db_script_dir_now(view_index, pane_id, database, rel_path);
            }
            PendingConfirmation::DeleteContentNode {
                view_index,
                pane_id,
                node_id,
            } => {
                self.delete_content_node_now(view_index, pane_id, node_id);
            }
            PendingConfirmation::BulkDeleteStaleLinks(link_ids) => {
                let repo = Arc::clone(&self.link_repo);
                let total = link_ids.len();
                let result = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async move {
                        let mut deleted = 0usize;
                        let mut first_err: Option<String> = None;
                        for id in link_ids {
                            match repo.delete(id).await {
                                Ok(()) => deleted += 1,
                                Err(e) => {
                                    if first_err.is_none() {
                                        first_err = Some(format!("{e}"));
                                    }
                                }
                            }
                        }
                        (deleted, first_err)
                    })
                });
                let (deleted, first_err) = result;
                self.reload_link_refs();
                match first_err {
                    None => self.notify(format!("Pruned {deleted} stale link(s)")),
                    Some(e) => self.notify_error(format!(
                        "Pruned {deleted}/{total} link(s); first error: {e}"
                    )),
                }
            }
        }
    }

    fn undelete_last(&mut self) {
        // Find the most recent deleted_at timestamp among all loaded tasks
        // so we can identify which tasks will be restored.
        let latest_deleted_at = self.tasks_view.state.task_rows.iter()
            .filter(|t| t.deleted && t.deleted_at.is_some())
            .max_by_key(|t| t.deleted_at)
            .and_then(|t| t.deleted_at);

        let tasks_to_restore: Vec<_> = if let Some(ts) = latest_deleted_at {
            self.tasks_view.state.task_rows.iter()
                .filter(|t| t.deleted_at == Some(ts))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };

        let service = Arc::clone(&self.task_service);
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                service.undelete_last().await
            })
        });
        match result {
            Ok(0) => self.notify("Nothing to undelete".to_string()),
            Ok(count) => {
                // Unmark notes as deleted for restored tasks.
                for task in &tasks_to_restore {
                    crate::notes::unmark_notes_deleted(task, &self.tasks_view.state.task_rows);
                }
                self.spawn_load();
                self.notify(format!("Restored {count} task(s)"));
            }
            Err(e) => self.notify_error(format!("Undelete error: {e}")),
        }
    }

    pub fn refresh_tracked_ids(&mut self) {
        self.tracked_ids = self.get_tracked_task_ids();
    }

    /// Refresh `link_refs` from the link table. Cheap — a single `list_all`
    /// scan. Called on startup and after every mutation that adds or
    /// removes a link row. Also syncs the snapshot held by views that
    /// render the `links` column.
    pub fn reload_link_refs(&mut self) {
        let repo = Arc::clone(&self.link_repo);
        let rows = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                repo.list_all().await.unwrap_or_default()
            })
        });
        let mut set = HashSet::with_capacity(rows.len() * 2);
        for row in rows {
            set.insert(row.source_ref);
            set.insert(row.target_ref);
        }
        self.link_refs = set;
        // Push the fresh snapshot down to views that render the column
        // without going through App on every rebuild.
        self.trackings_view.set_link_refs(&self.link_refs);
        for slot in self.content_views.iter_mut() {
            if let Some(cv) = slot.as_view_mut() {
                cv.set_link_refs(&self.link_refs);
            }
        }
    }

    pub fn get_tracked_task_ids(&self) -> HashSet<Uuid> {
        let repo = Arc::clone(&self.tracking_repo);
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                repo.find_all_active().await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|t| t.task_id)
                    .collect()
            })
        })
    }

    /// Returns `true` if an external editor is currently running OR if a
    /// previous commit is still being processed in the background. Both
    /// states should reject a new editor open.
    pub fn editor_busy(&self) -> bool {
        self.detached_editor.is_some()
            || self.commit_in_flight
            || self.editor_loading_msg.is_some()
    }

    /// Returns `true` if a session is awaiting a buffer from the editor
    /// subprocess. Excludes the post-editor "saving" phase — that's a
    /// separate state (`commit_in_flight`).
    pub fn has_pending_edit(&self) -> bool {
        self.pending_session.is_some()
    }

    /// Add a notification. Stays visible until dismissed with Esc.
    pub fn notify(&mut self, message: String) {
        self.notification_bar.push(message);
    }

    /// Force a redraw at ~1 Hz while a `Busy` banner is on screen. Its
    /// elapsed-seconds counter is derived from wall-clock time at render,
    /// so without this nudge it would freeze between events. Returns
    /// `true` at most once per second, and only while a banner is live —
    /// otherwise the loop has no reason to repaint. (Active-tracking
    /// duration cells are handled by `tick_active_trackings` on its own
    /// adaptive interval.)
    pub fn tick_animations(&mut self) -> bool {
        if !self.has_live_banner() {
            return false;
        }
        if self.last_anim_tick.elapsed() < std::time::Duration::from_secs(1) {
            return false;
        }
        self.last_anim_tick = Instant::now();
        true
    }

    /// True while any content adapter is in a `Busy` state — the only
    /// banner whose text advances purely with wall-clock time.
    fn has_live_banner(&self) -> bool {
        self.content_views_indexed().any(|(_, cv)| cv.is_busy())
    }

    /// Whether the event-driven (1b) loop must arm its periodic ticker.
    /// The poll-based change sources have no waker/channel to park on, so
    /// they only make progress when the loop wakes on a timer. We arm that
    /// timer *only* while one of them is actually pending — otherwise the
    /// loop parks purely on terminal events + channels (true ~0 % idle).
    /// Covers: a live `Busy` banner (1 Hz second counter), an active
    /// tracking (duration cells), a detached editor (`:w` live-reload /
    /// `.done` close) and a detached script (completion marker).
    /// After a draw, re-fit the active content tab's tables to the pane
    /// width they just rendered into. Returns `true` if any table was
    /// rebuilt — the render loop then requests one more frame so the
    /// re-fitted layout is shown. Handles first paint, terminal resize, and
    /// preview open/close uniformly. Native (non-adapter) tabs lay their
    /// columns out at render time already, so they need no re-fit here.
    pub fn refit_visible_tables(&mut self) -> bool {
        if let Tab::Content(idx) = self.active_tab {
            if let Some(cv) = self.content_view_mut(idx) {
                return cv.refit_tables_if_needed();
            }
        }
        false
    }

    pub fn needs_periodic_tick(&self) -> bool {
        self.has_live_banner()
            || self.trackings_view.state.rows.iter().any(|r| r.active)
            || self.detached_editor.is_some()
            || self.detached_script.is_some()
    }

    /// Update durations of active trackings and rebuild the table if needed.
    /// Uses adaptive intervals: <60s → 5s, <10min → 10s, <1h → 30s, else 60s.
    /// Returns `true` when it actually rebuilt the table (i.e. the
    /// adaptive interval elapsed and durations advanced), so the loop
    /// repaints only on those ticks rather than every frame.
    pub fn tick_active_trackings(&mut self) -> bool {
        let has_active = self.trackings_view.state.rows.iter().any(|r| r.active);
        if !has_active { return false; }

        // Determine shortest active tracking duration for adaptive interval.
        let now_utc = chrono::Utc::now();
        let shortest = self.trackings_view.state.rows.iter()
            .filter(|r| r.active)
            .map(|r| (now_utc - r.started_at).num_seconds())
            .min()
            .unwrap_or(0);

        let interval_secs = if shortest < 60 { 5 }
            else if shortest < 600 { 10 }
            else if shortest < 3600 { 30 }
            else { 60 };

        let elapsed = self.last_tracking_tick.elapsed();
        if elapsed < std::time::Duration::from_secs(interval_secs) {
            return false;
        }
        self.last_tracking_tick = Instant::now();

        // Update durations for active rows.
        for row in &mut self.trackings_view.state.rows {
            if row.active {
                row.duration = now_utc - row.started_at;
            }
        }

        // Rebuild display rows and table.
        self.trackings_view.state.rebuild_display_rows();
        if self.trackings_view.state.sub_view == crate::tabs::TrackingsSubView::Condensed {
            self.trackings_view.state.rebuild_condensed_rows();
        }
        if self.trackings_view.state.sub_view == crate::tabs::TrackingsSubView::Tree {
            if let Some(ref forest) = self.tasks_view.state.forest {
                self.trackings_view.state.rebuild_tree_rows(forest);
            }
        }
        self.rebuild_trackings_table();
        true
    }

    /// Get the UUID of the currently selected task, respecting the active view.
    fn selected_task_id(&self) -> Option<Uuid> {
        self.task_table().selected_id()
    }

    /// Get a clone of the currently selected task, respecting the active view.
    fn selected_task(&self) -> Option<Task> {
        let id = self.selected_task_id()?;
        self.tasks_view.state.task_rows.iter().find(|t| t.id == id).cloned()
    }

    fn restore_selected_tracking(&mut self) {
        let Some(tracking_id) = self.trackings_view.state.tracking_id_at(self.trackings_view.table.selected_row()) else {
            self.notify("No tracking selected".to_string());
            return;
        };

        let repo = Arc::clone(&self.tracking_repo);
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let tracking = repo.find_by_id(tracking_id).await?
                    .ok_or(not_yet_done_core::error::AppError::TrackingNotFound(tracking_id))?;
                if !tracking.deleted {
                    return Err(not_yet_done_core::error::AppError::TrackingNotDeleted(tracking_id));
                }

                // BFS: find and hard-delete all successors.
                let mut queue = vec![tracking_id];
                let mut to_delete = Vec::new();
                while let Some(id) = queue.pop() {
                    let successors = repo.find_by_predecessor(id).await?;
                    for s in successors {
                        queue.push(s.id);
                        to_delete.push(s.id);
                    }
                }
                for id in to_delete.into_iter().rev() {
                    repo.hard_delete(id).await?;
                }

                repo.undelete(tracking_id).await?;
                Ok::<_, not_yet_done_core::error::AppError>(())
            })
        });

        match result {
            Ok(_) => {
                self.notify("Tracking restored".to_string());
                self.spawn_load_trackings();
            }
            Err(e) => self.notify_error(format!("Restore error: {e}")),
        }
    }

    fn restore_all_deleted_trackings(&mut self) {
        // Collect IDs of all deleted trackings currently visible.
        let repo = Arc::clone(&self.tracking_repo);
        let deleted_ids: Vec<Uuid> = self.trackings_view.state.filtered_indices.iter()
            .filter_map(|&i| {
                let row = &self.trackings_view.state.rows[i];
                // We can't check `deleted` on TrackingRow since it's not stored there.
                // Instead, we try to restore each one — the service will reject non-deleted.
                Some(row.id)
            })
            .collect();

        if deleted_ids.is_empty() {
            self.notify("No trackings to restore".to_string());
            return;
        }

        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let mut restored = 0u32;
                let mut skipped = 0u32;
                for id in &deleted_ids {
                    let tracking = match repo.find_by_id(*id).await? {
                        Some(t) => t,
                        None => { skipped += 1; continue; }
                    };
                    if !tracking.deleted {
                        skipped += 1;
                        continue;
                    }
                    // BFS: hard-delete all successors.
                    let mut queue = vec![*id];
                    let mut to_delete = Vec::new();
                    while let Some(qid) = queue.pop() {
                        let successors = repo.find_by_predecessor(qid).await?;
                        for s in successors {
                            queue.push(s.id);
                            to_delete.push(s.id);
                        }
                    }
                    for did in to_delete.into_iter().rev() {
                        repo.hard_delete(did).await?;
                    }
                    repo.undelete(*id).await?;
                    restored += 1;
                }
                Ok::<_, not_yet_done_core::error::AppError>((restored, skipped))
            })
        });

        match result {
            Ok((restored, _skipped)) => {
                self.notify(format!("{restored} tracking(s) restored"));
                self.spawn_load_trackings();
            }
            Err(e) => self.notify_error(format!("Restore error: {e}")),
        }
    }

    // -----------------------------------------------------------------------
    // Detached script polling
    // -----------------------------------------------------------------------

    /// Poll the marker file written by the most recently launched
    /// detached script. When found, surface captured output (if any)
    /// in a [`ScriptOutputSession`] and trigger a Trackings reload —
    /// the latter is a no-op for content-tab scripts (the reload
    /// just runs against the Trackings DB and doesn't disturb the
    /// content view).
    /// Returns `true` when a detached script finished and its output was
    /// processed this tick; `false` while none is pending or still running.
    pub fn poll_detached_script(&mut self) -> bool {
        let Some(ref script) = self.detached_script else { return false; };
        if !script.is_done() { return false; }

        let output_path = script.output_path.clone();
        let output = script.read_output();
        let capture = script.capture;
        let emits_commands = script.emits_commands;
        script.cleanup();
        self.detached_script = None;

        if emits_commands {
            // Re-use the same JSON-commands handler the background path
            // uses. We pass the path because read_output() already drained
            // the file; the helper re-reads it itself, so we touch the
            // file once at this layer instead of duplicating the parser.
            self.run_script_output_commands(&output_path);
        } else if capture {
            if let Some(content) = output.filter(|s| !s.trim().is_empty()) {
                let session = crate::edit_session::ScriptOutputSession::new(content);
                let _ = self.open_session(Box::new(session));
            } else {
                self.notify("Script finished (no output)".to_string());
            }
        }

        self.spawn_load_trackings();
        true
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Copy text to the system clipboard. Requires the `clipboard` feature.
#[cfg(feature = "clipboard")]
fn copy_to_clipboard(text: &str) {
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        let _ = clipboard.set_text(text);
    }
}

/// Read text from the system clipboard. Returns `None` when the
/// `clipboard` feature is off or no text is available.
#[cfg(feature = "clipboard")]
fn clipboard_text() -> Option<String> {
    arboard::Clipboard::new().ok().and_then(|mut c| c.get_text().ok())
}

#[cfg(not(feature = "clipboard"))]
fn clipboard_text() -> Option<String> {
    None
}

/// Returns true if clipboard support is compiled in.
#[allow(dead_code)]
fn has_clipboard() -> bool {
    cfg!(feature = "clipboard")
}

/// Check if a task belongs to the subtree rooted at `root_id`.
pub(crate) fn is_in_subtree(task: &Task, root_id: Uuid, all_tasks: &[Task]) -> bool {
    if task.id == root_id {
        return true;
    }
    let mut current = task.parent_id;
    while let Some(pid) = current {
        if pid == root_id {
            return true;
        }
        current = all_tasks.iter().find(|t| t.id == pid).and_then(|t| t.parent_id);
    }
    false
}

/// Load content views from YAML files in `~/.config/not_yet_done/views/`.
/// Each file becomes one [`ContentSlot`]: `Working` if the YAML loaded,
/// validated, and an adapter (or fallback) bound; `Broken` if the YAML
/// is invalid (parse/validate failure). Tab indices stay stable so the
/// user sees a labeled tab for the broken file with an in-app error
/// panel instead of the process exiting.
/// Build the [`TabLayout`] for the current config + loaded content
/// views. Returns the layout plus an optional hard-error message (a
/// duplicate tab name) for the caller to surface as a startup modal; on
/// that error the layout falls back to legacy so the app still runs.
/// Soft issues (unknown / missing constellation) are logged, not
/// returned.
fn build_tab_layout(
    tabs_cfg: &crate::config::TabsConfig,
    content_views: &[ContentSlot],
) -> (TabLayout, Option<String>) {
    // Built-in tabs first, then content tabs in slot order — this is
    // also the legacy display/cycle order.
    let mut available: Vec<(String, Tab)> = vec![
        ("Tasks".to_string(), Tab::Tasks),
        ("Trackings".to_string(), Tab::Trackings),
    ];
    for (idx, slot) in content_views.iter().enumerate() {
        available.push((slot.tab_name().to_string(), Tab::Content(idx)));
    }

    match crate::tabs::resolve_tab_layout(
        tabs_cfg,
        &available,
        content_views.len(),
        |w| not_yet_done_content::http_log::log_error("tab_layout", &w),
    ) {
        Ok(layout) => (layout, None),
        Err(hard) => {
            not_yet_done_content::http_log::log_error("tab_layout", &hard);
            (TabLayout::legacy(content_views.len()), Some(hard))
        }
    }
}

fn load_content_views(
    theme: &Arc<Theme>,
    keybindings: &crate::config::keybindings::KeyBindingConfig,
    editors: &crate::config::editor::EditorsConfig,
    factories: std::collections::HashMap<String, Box<dyn not_yet_done_content::AdapterFactory>>,
) -> Vec<ContentSlot> {
    use crate::config::view_config::ViewFileConfig;

    let views_dir = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("not_yet_done")
        .join("views");

    let mut yaml_files: Vec<std::path::PathBuf> = std::fs::read_dir(&views_dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "yaml" || ext == "yml"))
        .collect();
    yaml_files.sort();

    /// Per-file outcome of the YAML pass. We split parse/validate from
    /// adapter construction because broken files keep their slot — the
    /// adapter-uniqueness check below only runs on validated files.
    enum Loaded {
        Ok(ViewFileConfig),
        Broken { name: String, errors: Vec<String> },
    }

    let mut loaded: Vec<(std::path::PathBuf, Loaded)> = Vec::new();
    for path in &yaml_files {
        let yaml = match std::fs::read_to_string(path) {
            Ok(y) => y,
            Err(e) => { eprintln!("Warning: {}: {e}", path.display()); continue; }
        };
        // Heuristic: a view-config has top-level `tab` AND `adapter` keys.
        // Files without both (e.g. adapter credentials like jira-adapter.yaml)
        // are skipped silently.
        let raw: serde_yaml::Value = match serde_yaml::from_str(&yaml) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let is_view_config = raw.get("tab").is_some() && raw.get("adapter").is_some();
        if !is_view_config { continue; }

        // YAML-parse failure: take the file's stem as a fallback tab name
        // (the actual `tab.name` is unreadable).
        let config: ViewFileConfig = match serde_yaml::from_str(&yaml) {
            Ok(c) => c,
            Err(e) => {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("?")
                    .to_string();
                loaded.push((path.clone(), Loaded::Broken {
                    name,
                    errors: vec![format!("YAML parse error: {e}")],
                }));
                continue;
            }
        };

        match config.validate(keybindings, editors) {
            Ok(()) => loaded.push((path.clone(), Loaded::Ok(config))),
            Err(errors) => loaded.push((path.clone(), Loaded::Broken {
                name: config.tab.name.clone(),
                errors,
            })),
        }
    }

    // Cross-file: adapter-instance-id uniqueness. Only validated files
    // are checked — broken files don't construct an adapter.
    let mut seen_ids: std::collections::HashMap<String, std::path::PathBuf> =
        std::collections::HashMap::new();
    for (path, l) in &loaded {
        if let Loaded::Ok(config) = l {
            let id = config.adapter.effective_instance_id().to_string();
            if let Some(prev) = seen_ids.get(&id) {
                eprintln!(
                    "View config error: duplicate adapter instance id '{id}' in:\n  {}\n  {}\nset an explicit `adapter.id:` in one of them to disambiguate",
                    prev.display(),
                    path.display(),
                );
                std::process::exit(1);
            }
            seen_ids.insert(id, path.clone());
        }
    }

    let mut slots: Vec<ContentSlot> = Vec::new();
    for (path, l) in loaded {
        match l {
            Loaded::Broken { name, errors } => {
                slots.push(ContentSlot::Broken { name, path, errors });
            }
            Loaded::Ok(config) => {
                let path_ref = path.as_path();
                let mut init_error: Option<String> = None;
                let adapter: Option<Arc<dyn not_yet_done_content::ContentAdapter>> =
                    match factories.get(&config.adapter.adapter_type) {
                        None => {
                            init_error = Some(format!(
                                "no adapter factory registered for type '{}'",
                                config.adapter.adapter_type
                            ));
                            None
                        }
                        Some(factory) => {
                            let adapter_config = config.adapter.config_inline.as_ref().cloned()
                                .or_else(|| {
                                    config.adapter.config.as_ref().and_then(|cfg_path| {
                                        let resolved = if std::path::Path::new(cfg_path).is_absolute() {
                                            std::path::PathBuf::from(cfg_path)
                                        } else {
                                            path_ref.parent().unwrap_or(std::path::Path::new(".")).join(cfg_path)
                                        };
                                        std::fs::read_to_string(&resolved).ok()
                                    })
                                });
                            match adapter_config {
                                None => {
                                    init_error = Some(
                                        "adapter config missing (neither `config_inline` nor a readable `config:` path)"
                                            .into(),
                                    );
                                    None
                                }
                                Some(cfg) => match factory.create(config.adapter.effective_instance_id(), &cfg) {
                                    Ok(a) => Some(Arc::from(a)),
                                    Err(e) => {
                                        init_error = Some(e.to_string());
                                        None
                                    }
                                },
                            }
                        }
                    };

                let mut view = ContentView::new(Arc::clone(theme), &config, adapter, keybindings);
                if let Some(err) = init_error {
                    view.set_adapter_init_error(err);
                }
                view.source_path = Some(path.clone());
                slots.push(ContentSlot::Working(view));
            }
        }
    }

    // Sort: working slots by tab_order, broken slots keep their relative
    // load order at the end (their tab "position" doesn't matter for
    // ordering — the panel is a static error display).
    slots.sort_by_key(|s| match s {
        ContentSlot::Working(cv) => cv.tab_order,
        ContentSlot::Broken { .. } => i32::MAX,
    });

    // Fallback: if no slots loaded at all, create a default Jira view
    // without adapter so the TUI is never empty.
    if slots.is_empty() {
        let config = crate::views::content_view::default_jira_view_config();
        slots.push(ContentSlot::Working(ContentView::new(
            Arc::clone(theme),
            &config,
            None,
            keybindings,
        )));
    }

    // Assign view indices. Working slots only — broken slots don't
    // address `App::content_views` reactively, but their slot index
    // still matches their position for `Tab::Content` purposes.
    for (i, slot) in slots.iter_mut().enumerate() {
        if let ContentSlot::Working(cv) = slot {
            cv.view_index = i;
        }
    }

    slots
}

#[cfg(test)]
mod tests {
    use super::split_leading_token;

    #[test]
    fn split_leading_token_quoted_tab_name_with_spaces() {
        // The `:tree-find "Tasks (A)" id:42` case: a quoted tab name
        // keeps its spaces, the rest is the query.
        let (tok, rest) = split_leading_token(r#""Tasks (A)" id:42"#);
        assert_eq!(tok, "Tasks (A)");
        assert_eq!(rest, "id:42");
    }

    #[test]
    fn split_leading_token_unquoted_splits_on_first_space() {
        let (tok, rest) = split_leading_token("Taiga:items /ref|acme#42");
        assert_eq!(tok, "Taiga:items");
        assert_eq!(rest, "/ref|acme#42");
    }

    #[test]
    fn split_leading_token_single_token_has_empty_remainder() {
        let (tok, rest) = split_leading_token("Trackings");
        assert_eq!(tok, "Trackings");
        assert_eq!(rest, "");
    }

    #[test]
    fn split_leading_token_unterminated_quote_takes_whole_rest() {
        let (tok, rest) = split_leading_token(r#""Tasks (A"#);
        assert_eq!(tok, "Tasks (A");
        assert_eq!(rest, "");
    }
}

