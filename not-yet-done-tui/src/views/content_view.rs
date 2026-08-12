//! ContentView — generic adapter-driven tab.
//!
//! Replaces JiraView with a config-driven component that works with
//! any ContentAdapter. Configuration comes from ViewFileConfig (YAML).
//!
//! Per-drill state (items, drill stack, filter/search/sort/page/preview,
//! table widget) lives on [`ContentPane`]. `ContentView` is the tab-level
//! container — it owns the adapter, view definitions, action bar /
//! cmdline / query menu, and one [`PaneTree`] per `ViewDef`. Each tree
//! starts as a single leaf and (Phase 2 onwards) can be split into a
//! recursive `Leaf | Branch` structure. `active_subtab` selects the
//! tree; the tree's own `focus` selects the focused leaf.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use tuirealm::command::{Cmd, CmdResult, Direction, Position};
use tuirealm::component::Component;

use not_yet_done_ratatui::{
    ColumnStyles, StyleMap, TableStyle, TableStyleType, TableWidgetCell, TableWidgetLine,
    TableWidgetRow,
};
use not_yet_done_table::{
    CardBorder, CardField, CardLabels, CardSpanKind, CardSpec, CellAlignment, CellContent,
    ColStrategy, ColumnId as TColumnId, LineTemplate, MixedColSizer, PlanRow, Row as TRow,
    RowTemplate, StyledSpan, TableConfig, compute_cards, compute_multiline_table, compute_table,
    fit_aligned, group,
};

use not_yet_done_content::{
    AdapterStatus, ContentAdapter, CursorIntent, GroupSpec, NodeSummary, PageInfo, PageRequest,
    QueryKind, SortDirection, SortKey, Subtree, TreeFindHit,
};

use crate::active_surface::ActiveSurface;
use crate::components::action_bar::{ActionBarComponent, ActionHint};
use crate::components::cmdline::CmdlineComponent;
use crate::components::data_table::DataTable;
use crate::components::query_menu::{QueryMenuComponent, QueryMenuEntry, QueryMenuMessage};
use crate::components::search::SearchComponent;
use crate::components::tab_set_popup::{TabSetEntry, TabSetPopup, TabSetPopupMessage};
use crate::config::keybindings::{
    CommonAction, ContentAction, KeyBinding, KeyBindingConfig, KeyBindingSection, KeyIconMap,
    QueryMenuAction, WindowAction,
};
use crate::config::tui_config::LoadBannerRoute;
use crate::config::view_config::{
    ActionDef, AggregateDef, CardBorderMode, CardConfig, CardLabelMode, ChildDef, ColumnDef,
    ColumnKind, CursorOnOpen, DateBucket, ExpandDepth, GroupBy, GroupHeadersDef, GroupOrder,
    LineLayout, PaginationMode, PreviewConfig, ReminderConfig, SplitDirection,
    TreeAggregateDefault, ViewDef, ViewFileConfig,
};
use crate::keymap::{KeyClaim, KeyMap, KeyScope, KeySource, PaneStateProfile, SearchJump, TabRef};
use crate::ui::theme::Theme;
use crate::views::column_format::{
    column_kind_from_value_type, format_elapsed_since, format_typed_value,
    value_type_from_column_kind,
};
use crate::views::content_action_hints::{
    ActionBarHint, HintBar, ShortcutHint, nav_hint_for_source, window_nav_hint,
};
use crate::views::content_detail;
use crate::views::content_tree::{
    TreeLevel, TreeState, child_def_for_type_chain, effective_child_children, icon_opt_for_chain,
    leaf_glyph_opt_for_chain, tree_child_def_at_depth, tree_level_at_depth, tree_level_children,
    tree_level_children_for_chain, tree_level_for_chain, tree_self_at_depth,
};
use crate::views::group_aggregate::{
    agg_value, bucket_display_label, group_label, to_group_bucket,
};
use crate::views::images::ImageStore;
use crate::views::markdown::{
    StyleMapBuilder, lines_to_widget_lines_with_images, render_markdown_lines,
    render_markdown_lines_with_images,
};
use crate::views::{
    BarHint, CmdlineKeyResult, CmdlineState, HasCmdline, PaginatedView, SearchKeyResult,
    SearchState, Searchable, SortableView, SubViewMessage, ViewRequest,
};

// ── Navigation stack ─────────────────────────────────────────────────

/// Walk a view's declared tree, emitting `(view_name, child_name_path,
/// node_type)` for the level itself and every descendant. Mirrors the
/// recursion in `keymap::collect_node_shortcuts` so the levels line up with
/// the `NodeShortcut` child paths used for binding.
fn collect_declared_levels(
    view_name: &str,
    child_path: &[String],
    node_type: &str,
    children: &[ChildDef],
    out: &mut Vec<(String, Vec<String>, String)>,
) {
    out.push((
        view_name.to_string(),
        child_path.to_vec(),
        node_type.to_string(),
    ));
    for child in children {
        let mut path = child_path.to_vec();
        path.push(child.name.clone());
        collect_declared_levels(view_name, &path, &child.node_type, &child.children, out);
    }
}

/// Snapshot of a previous navigation level (pushed when drilling down).
struct NavFrame {
    /// Breadcrumb label for this level.
    label: String,
    /// Node ID of the item we drilled into (parent for child-level actions).
    parent_node_id: String,
    /// Items that were displayed.
    items: Vec<NodeSummary>,
    /// Selected row index.
    selected_row: usize,
    /// Selected column index, or `None` if the column cursor was off
    /// at this level. Restored on nav_back so that drilling between
    /// column-cursor-enabled levels preserves the column position.
    selected_column: Option<usize>,
    /// The active_child at this level (None = root ViewDef level).
    active_child: Option<ChildDef>,
    /// Preview state.
    preview_open: bool,
    preview_key: String,
    preview_description: String,
    preview_scroll: u16,
    /// Whether this level's preview renders as Markdown.
    preview_markdown: bool,
    /// Tree state stashed when drilling out of a tree-mode level into a
    /// non-tree child. `None` for frames pushed from flat-mode panes.
    /// Restored on `nav_back` so the resurrected level resumes with the
    /// same expanded set, cached children, and cursor depth.
    tree: Option<TreeState>,
}

/// Parameters for an outgoing `FetchContentPreview` request.
pub struct PreviewFetchParams {
    /// Pane-side cache key — equal to the row's own id, used by
    /// `set_preview_description` to match the reply against the
    /// currently selected row regardless of any redirect.
    pub cache_key: String,
    /// Node id passed to `adapter.get_by_id`. Equals `cache_key` unless
    /// `preview.node_id_from` redirects to a linked node.
    pub node_id: String,
    /// Adapter-side action id (from `preview.action`); when `Some` the
    /// fetcher uses `Node::prepare(action)` instead of `content()`.
    pub action_id: Option<String>,
}

/// What the search input bar feeds into.
#[derive(Debug, Clone)]
enum SearchMode {
    /// Local: filter visible items as the user types (existing `/`-search).
    Local,
    /// Adapter: on Enter, render the user's input through `template` (with
    /// `{q}` replaced) and reload the view with the resulting query.
    Adapter {
        template: String,
        prompt: Option<String>,
    },
    /// CT-7: tree-find — on Enter, dispatch
    /// [`crate::views::ViewRequest::TreeFindStart`] with the raw
    /// query string. The adapter performs a server-side search and
    /// the pane's [`TreeFindState`] cache (CT-5) drives the
    /// subsequent `n`/`N` navigation. Only registered on
    /// tree-enabled views; the validator rejects `tree_find` outside
    /// a tree chain.
    TreeFind { prompt: Option<String> },
}

/// What [`ContentPane::root_load_request`] / drill-down handlers feed
/// into the adapter call. Bundles the pieces an adapter's `list()` needs.
///
/// `query` is the **raw** saved-query string (still containing any
/// `${var:default}` placeholders the adapter understands). The caller
/// is responsible for running `adapter.render_query(query, &vars)`
/// before passing the result into `ListParams::query`.
#[derive(Clone, Debug)]
pub struct LoadRequest {
    pub node_type_id: String,
    pub query: Option<String>,
    /// How `query` is to be run: handed to the adapter as-is (`Saved`), or
    /// parsed and executed as an extended-query document (`Extended`). The
    /// loader must not render an extended document itself — each branch is
    /// rendered separately, with the same bindings.
    pub kind: QueryKind,
    pub sort: Vec<SortKey>,
    pub page: Option<PageRequest>,
    /// Variable bindings to substitute into `query` via
    /// `ContentAdapter::render_query`. Empty when no variables are in
    /// play.
    pub vars: std::collections::HashMap<String, String>,
}

/// State remembered when a pane is showing the result of a custom
/// adapter query (e.g. raw SQL via the Q-editor). Lets next/prev-page
/// keys re-execute the same query with a new offset instead of falling
/// back to `list()`. Cleared on any non-custom item load (drill, back,
/// regular reload).
#[derive(Clone, Debug)]
pub struct CustomQueryRunState {
    /// SQL/query text exactly as the user wrote it (no LIMIT/OFFSET
    /// wrap — the adapter applies its own pagination wrap).
    pub query: String,
    /// Node the query was issued against, in the adapter's own id form.
    /// Carried opaquely; the re-execution path hands it back to
    /// [`ContentAdapter::custom_query_context`](not_yet_done_content::ContentAdapter::custom_query_context)
    /// so the adapter re-derives its own routing keys (for Postgres: the
    /// target database).
    pub node_id: String,
    /// Pagination strategy for follow-up `>` / `<` keys. Resolved
    /// from the active view's `pagination.mode` when the result
    /// lands (see [`ContentView::apply_custom_query_result`]).
    /// `Server` → offset/limit re-issue; `Cursor` → adapter-side
    /// cursor lifecycle; `All` → no follow-up paging.
    pub mode: PaginationMode,
    /// Server-side cursor handle from the adapter's most recent
    /// open / continue response. `Some` only while `mode == Cursor`
    /// and an active cursor is held; cleared on close and replaced
    /// when a new cursor is opened. The pane uses this to issue
    /// [`not_yet_done_content::CursorIntent::Continue`] on `>` and
    /// to re-issue [`Open`] on `<` (NO SCROLL cursor can't fetch
    /// backwards, so prev re-opens — leaving the old cursor for the
    /// pane-close cleanup hook to tear down).
    pub cursor_id: Option<String>,
}

/// A saved query made available to a content view. Body comes from
/// one of the two adapter-managed stores (filesystem); shortcut comes
/// from the DB `query_shortcut` table.
#[derive(Debug, Clone)]
pub struct MergedSavedQuery {
    pub name: String,
    pub query: String,
    pub shortcut: Option<String>,
    /// Which store the body came from — the only thing that tells an extended
    /// document from an adapter-native query, since names are unique across
    /// both stores and the user is not meant to tell them apart.
    pub kind: QueryKind,
}

// ── ContentPane ──────────────────────────────────────────────────────

/// In-flight retry information for a failed load. `attempt` is the
/// 1-based index of the **next** attempt the loader is about to start;
/// `max_attempts` is `1 + ViewDef::retries` (the total cap including
/// the original try). `last_error` is the message from the most
/// recent failure, surfaced in the banner so the user knows what's
/// being retried.
#[derive(Debug, Clone)]
pub struct RetryState {
    pub attempt: u32,
    pub max_attempts: u32,
    pub last_error: String,
}

/// CT-7: one step in the lazy-expand-to-current-hit walk.
///
/// Returned by [`ContentPane::advance_tree_find`]. The App-side
/// caller dispatches the embedded request (if any), waits for the
/// matching response to land in `poll_load`, and re-invokes the
/// walker until it terminates with `Ready` or `NotInTree`.
#[derive(Debug, Clone)]
pub enum TreeFindAdvance {
    /// All ancestors are cached + expanded; cursor positioned on
    /// `row`. State stays active so `n`/`N` keeps working.
    Ready(usize),
    /// Root-level items haven't been loaded yet — the caller should
    /// dispatch [`crate::views::ViewRequest::SpawnContentLoad`].
    NeedRootLoad,
    /// Lazy-load the next chain level. Caller dispatches
    /// [`crate::views::ViewRequest::ExpandTreeNode`] with these
    /// fields verbatim.
    NeedTreeExpand {
        parent_path: Vec<String>,
        parent_node_id: String,
        child_node_type: String,
        page_size: u32,
    },
    /// A load is already in flight (or the result is still being
    /// processed). No-op — re-poll after the next `TreeChildren`
    /// settles.
    Waiting,
    /// The path can't be reached in the current view (filter,
    /// pagination cap, deleted ancestor, missing ChildDef). Surface
    /// the message to the user.
    NotInTree(String),
    /// No active tree-find or no hits. Caller drops the walk.
    Idle,
}

/// Pane-local state for the `tree_find` action — server-side search
/// over a tree-mode view (CT-5).
///
/// Lifecycle:
/// - `tree_find_begin(query)` → `loading = true`, `hits` empty.
/// - On adapter response, `tree_find_complete(hits, truncated)` lands
///   pre-sorted hits in tree-render order; `current` resets to `0`.
/// - `tree_find_next` / `tree_find_prev` wrap around `hits.len()`.
/// - `tree_find_clear` drops the state entirely (Esc, reload, fresh
///   search input).
///
/// Survives manual expand/collapse: nothing here references
/// `TreeState`; the cached path lookup happens at jump-time. The
/// state is invalidated only by explicit user actions (CT-9).
#[derive(Debug, Clone)]
pub struct TreeFindState {
    /// Raw user query as it was sent to the adapter. Surfaced in the
    /// status-bar hint (CT-8); kept here so re-renders don't have to
    /// reach into a separate component.
    pub query: String,
    /// Hits in tree-render order. Empty while `loading`, and stays
    /// empty when the search returns no matches.
    pub hits: Vec<TreeFindHit>,
    /// Cursor into `hits`. Always `< hits.len()` when `hits` is
    /// non-empty; clamped to `0` when empty. Wrap-around on
    /// next/prev keeps it valid.
    pub current: usize,
    /// `true` between `tree_find_begin` and the matching
    /// complete/fail call. Used to render the "Loading…" hint and to
    /// reject duplicate dispatches.
    pub loading: bool,
    /// Server reported more matches than fit in `hits`. UI surfaces
    /// this so the user knows to refine the query.
    pub truncated: bool,
    /// Walker has reached the current hit and positioned the cursor.
    /// Subsequent `TreeChildren` lands (e.g. the user expanded a node
    /// by hand with Enter) must NOT re-run the walker — otherwise the
    /// cursor snaps back to the hit on every drill. `next`/`prev` and
    /// a fresh `tree_find_complete` re-arm by clearing this flag.
    pub settled: bool,
    /// Ancestor `parent_path`s the walk has already force-refreshed this
    /// hit because they were cached+loaded but did **not** contain the
    /// expected child — the classic "a sibling was just created (e.g. by
    /// an external `nyd-t task add`) but this level's cache predates it"
    /// case. Re-fetching the level once (fresh `list`, replacing the stale
    /// slot) surfaces the new child; this set bounds it to a single retry
    /// per level so a genuinely-absent id degrades to `NotInTree` instead
    /// of looping. Cleared whenever the walk targets a new hit
    /// (`tree_find_complete` / `next` / `prev`).
    pub refreshed_paths: std::collections::HashSet<Vec<String>>,
}

/// Per-drill state for one navigation context. A pane shows one
/// [`ViewDef`] (and optionally a drilled-down child level), owns its
/// own table widget, drill stack, filter/search/sort/page state, and
/// preview pane. `ContentView` holds a vector of these — one per
/// [`ViewDef`] for now; future split work will replace the flat vector
/// with a tree of panes.
pub struct ContentPane {
    /// Index into `ContentView.view_defs`. Stays fixed for this pane's
    /// lifetime — switching subtab moves the active-pane pointer
    /// instead of re-pointing a single pane at a new ViewDef.
    view_def_index: usize,
    theme: Arc<Theme>,
    pub table: DataTable,

    pub items: Vec<NodeSummary>,
    pub fetch_error: Option<String>,
    /// Maps table row index → items index when fuzzy filter is active.
    filtered_indices: Vec<usize>,

    // Navigation stack (drill-down into children)
    nav_stack: Vec<NavFrame>,
    /// When drilled into a child, this holds the ChildDef config.
    active_child: Option<ChildDef>,
    /// The drilled-into level's `cursor_on_open` placement, armed by
    /// [`Self::drill_down_prepare`] and consumed by the first
    /// [`Self::set_items`] that brings rows in. Pending rather than applied
    /// at drill time because the drill only *clears* the pane — the items the
    /// cursor should land on arrive one async load later.
    pending_cursor_on_open: Option<CursorOnOpen>,

    /// Active query override (set by editor or saved query selection).
    /// Holds the **raw** string; if the adapter understands inline
    /// variable syntax (e.g. Taiga's `${name:default}`), substitution
    /// happens at load time via `ContentAdapter::render_query`.
    active_query: Option<String>,
    /// Name of the active saved query (for status bar display).
    active_query_name: Option<String>,
    /// Variable bindings for the active query. Empty when the query
    /// has no variables (or the adapter doesn't support them).
    active_query_vars: std::collections::HashMap<String, String>,
    /// Which language [`Self::active_query`] is written in: the adapter's own
    /// (`Saved`) or an extended-query Markdown document (`Extended`). It rides
    /// with the body rather than being derived from it — a document is only
    /// recognisable by where it was loaded from, and guessing from the text
    /// would make a `yaml`-adapter's query indistinguishable from a spec fence.
    active_query_kind: QueryKind,

    // Preview pane
    preview_open: bool,
    preview_description: String,
    preview_key: String,
    preview_scroll: u16,
    preview_loading: bool,
    /// Whether the current level's preview renders its text as Markdown
    /// (set from `PreviewConfig.markdown` when a preview is resolved).
    preview_markdown: bool,
    /// Last rendered inner height of the preview pane (without borders).
    /// Drives ctrl+u/ctrl+d half-page scrolling.
    preview_visible_height: u16,

    /// Text search component (driven by configured search action).
    pub search: SearchComponent,
    /// Which fields to search in fuzzy filter. Empty = all fields + label.
    fuzzy_filter_fields: Vec<String>,
    /// Which fields to search in `/`-search. Empty = label + all fields.
    search_fields: Vec<String>,
    /// Key that jumps to the next search match while results exist.
    /// Set by the latest `search`-type action; defaults to `n`.
    search_next_key: String,
    /// Key that jumps to the previous search match while results exist.
    /// Set by the latest `search`-type action; defaults to `N`.
    search_prev_key: String,
    /// What pressing Enter in the search bar should do.
    search_mode: SearchMode,
    /// Query rendered by the last accepted adapter text search
    /// ([`SearchMode::Adapter`]). Kept so the action bar can keep the
    /// `text_search` hint lit for as long as the pane still shows that
    /// search — comparing against [`Self::active_query`] means any other
    /// query (saved query, default query, editor) clears the state on its
    /// own, with no reset call to forget.
    text_search_query: Option<String>,

    // ── Sort + pagination state ─────────────────────────────────────
    /// Sort the user has requested. Empty = let the adapter pick.
    current_sort: Vec<SortKey>,
    /// Page request the user is currently on.
    current_page: Option<PageRequest>,
    /// Sort the adapter actually applied to the last result.
    last_applied_sort: Vec<SortKey>,
    /// Pagination state of the last result, if the adapter returned one.
    last_page_info: Option<PageInfo>,
    /// Sortable columns advertised by the adapter for the active node
    /// type at the time of the last load.
    last_columns: Vec<not_yet_done_content::ColumnSchema>,

    /// When the pane is showing the result of an adapter-native custom
    /// query (e.g. raw SQL from the Postgres Q-editor), this holds the
    /// query text + addressing data needed to re-run with a new page
    /// offset. `None` when the pane's items came from a regular
    /// `list()` call — next/prev-page falls back to the normal reload
    /// path.
    active_custom_query: Option<CustomQueryRunState>,

    /// Column widths from the most recent layout. Used to position the
    /// sort-mode direction-picker overlay correctly across the header.
    last_col_widths: Vec<usize>,
    /// Pane width (terminal columns) the most recent table layout was
    /// fitted to. Compared against the table's actual render width after a
    /// draw (`ContentView::refit_tables_if_needed`): a mismatch — first
    /// paint, terminal resize, preview open/close — triggers a one-frame
    /// re-fit so columns always span exactly the current pane.
    built_table_width: u16,
    /// Column keys in display order from the most recent rebuild_table.
    last_column_keys: Vec<String>,

    /// Whether this pane has ever been loaded from the adapter — controls
    /// whether activating it triggers an automatic SpawnContentLoad.
    loaded: bool,

    /// Backlink for a coupled split-drill: when this pane drilled into a
    /// child via a `SplitDef { coupled: true, .. }`, this stores the
    /// `ChildDef.name` and the [`PaneId`] of the spawned child pane.
    /// Re-drilling from the same parent into a child with the same name
    /// hot-replaces the linked child in place. Closing the parent
    /// cascades to the child; closing the child clears the backlink
    /// here. Treated as lazy/optimistic: a stale `PaneId` (no longer
    /// present in the tree) is silently ignored and overwritten.
    linked_child: Option<(String, PaneId)>,

    /// Record-detail split: when this (source) pane has opened a
    /// coupled record-detail pane via `ToggleRecordDetail`, this holds
    /// the [`PaneId`] of that detail pane. Kept separate from
    /// [`Self::linked_child`] so it never interferes with coupled
    /// split-drill. Closing this pane cascades to the detail pane;
    /// closing the detail pane clears this backlink. Optimistic: a
    /// stale id (gone from the tree) is silently ignored/overwritten.
    detail_child: Option<PaneId>,
    /// Record-detail split: when this pane *is* a detail follower, this
    /// holds the [`PaneId`] of its source table pane. The per-frame
    /// `refresh_detail_panes` reads the source's `selected_item()` and
    /// transposes it into this pane's field/value rows. `None` for
    /// ordinary panes.
    detail_source: Option<PaneId>,
    /// Record-detail split: the source row currently transposed into
    /// this detail pane, set by the per-frame sync. Compared (via
    /// `NodeSummary: Eq`) against the source's live selection so the
    /// detail table only rebuilds when the selected record actually
    /// changes (preserving scroll otherwise). `None` for ordinary panes
    /// and for a detail pane whose source has no selection.
    detail_summary: Option<NodeSummary>,
    /// Record-detail split: whether long field values wrap onto
    /// continuation rows (`X` toggles it). Default `false` — long
    /// values are clipped to the value column. Meaningful only on a
    /// detail pane (`detail_source.is_some()`).
    detail_wrap: bool,
    /// Long-text mode (`v`): when on, a column that declares `long_source`
    /// renders the full field as a soft-wrapped multi-line block, growing
    /// that row vertically; the header, columns, grouping and totals are
    /// untouched. Default `false`. Meaningful only on a view whose columns
    /// declare a `long_source` (see [`long_text_available`](Self::long_text_available)).
    long_text: bool,

    /// Snapshot of [`App::link_refs`] for rendering the `has_links`
    /// YAML column source. Synced by [`ContentView::set_link_refs`].
    link_refs: std::collections::HashSet<String>,
    /// Cached NodeRef prefix `"{kind}/{instance_id}"` for items in this
    /// pane. Used together with `item.id` to build a full NodeRef for
    /// the `has_links` column lookup. `None` until the parent
    /// `ContentView` pushes the adapter context via
    /// [`ContentView::set_link_refs`].
    link_node_ref_prefix: Option<String>,

    /// In-flight retry state for the most recent failed load. `Some`
    /// only while a retry is queued/running; cleared when a load
    /// succeeds or all retries are exhausted (in which case
    /// `fetch_error` becomes the sticky banner instead). Drives the
    /// "Retrying… (n/total): {err}" banner.
    pub retry_state: Option<RetryState>,

    /// Tree-mode state, populated when the active `ViewDef` declares
    /// `tree_label`. `None` for the legacy flat-list mode. While
    /// `Some(...)`, `items`/`nav_stack`/`active_child` are unused —
    /// the flattened tree drives rendering and cursor positioning.
    pub tree: Option<TreeState>,
    /// Maps table row index → `tree.entries` index in tree mode.
    /// Computed by `refresh_tree_visible_indices` from the active
    /// fuzzy filter (which only narrows entries at
    /// `tree_filter_depth`). Empty outside tree mode.
    tree_visible_indices: Vec<usize>,
    /// Depth at which a `fuzzy_filter` action is configured in the
    /// tree chain. Set when the action fires; resolved by walking the
    /// tree levels. `None` when no level defines fuzzy_filter (filter
    /// then can't be opened) or outside tree mode.
    tree_filter_depth: Option<usize>,
    /// Snapshot of `tree.expanded` captured the moment a fuzzy filter is
    /// opened on an **eager** tree (`supports_eager_subtree`). Opening the
    /// filter pulls the *whole* subtree (`list_subtree(u32::MAX)`) and marks
    /// every node expanded, so matches that live in collapsed or not-yet-paged
    /// branches surface — the native tab's "filter sees the entire forest"
    /// behaviour. When the filter is cleared we restore this set, re-collapsing
    /// the tree to exactly its pre-filter shape instead of leaving it blown
    /// open. `None` when no filter-expand is in effect.
    tree_filter_expand_stash: Option<std::collections::HashSet<Vec<String>>>,

    /// CT-5: pane-local cache for the active `tree_find` (server-side
    /// tree search). `Some` from `tree_find_begin` until the user
    /// clears it (Esc, reload, or a fresh search). When `Some`, the
    /// pane's `n`/`N` keys jump between `hits` instead of the local
    /// `/`-search match list — wire-up happens in CT-7. Independent
    /// of `tree` cache contents so expand/collapse leaves it intact.
    pub tree_find: Option<TreeFindState>,

    /// A tree-find query queued by the `:tree-find` command, to fire
    /// once the *next* root load lands. The command forces a fresh
    /// reload first (so out-of-process CLI mutations — e.g. a task the
    /// Taiga `goto_task` script just created — are in the snapshot
    /// before the search runs), then this query drives the normal
    /// expand-to-hit walk. Cleared by [`Self::take_pending_tree_find`].
    pub pending_tree_find: Option<String>,

    /// Reload fold-preservation signal for eager trees
    /// (`supports_eager_subtree`). `set_items` sets this to `true` on a
    /// *reload* (`was_loaded`) and leaves it `false` on the first load.
    /// The following [`ingest_subtree_level`] reads it: on first load it
    /// force-expands the whole eager subtree (the initial `expand_depth`
    /// shape); on reload it leaves `expanded` untouched so the user's
    /// collapse/expand choices survive re-reading the DB. Consumed (reset
    /// to `false`) by [`ContentView::apply_subtree`].
    eager_reload_preserve_expansion: bool,

    /// Node id the cursor sat on when a reload began, captured by
    /// `set_items` (reload only). After the eager subtree lands,
    /// [`ContentView::apply_subtree`] re-anchors the cursor onto this node
    /// via [`Self::focus_item_by_id`] so a reload keeps the selection on
    /// the same task rather than the same row index (which drifts when
    /// external rows are added/removed above it). Falls back to the
    /// clamped row when the node is gone (e.g. deleted). Taken (cleared)
    /// on use.
    eager_reload_reanchor_id: Option<String>,

    /// Runtime override of the level's configured `group_by` (M3). The
    /// `cycle_grouping` action rotates the date-bucket granularity through
    /// this field without touching the YAML default:
    ///
    /// - `None` → use the active level's configured `group_by`.
    /// - `Some(None)` → explicitly ungrouped (overrides a configured default).
    /// - `Some(Some(gb))` → explicit grouping (e.g. a coarser bucket).
    ///
    /// Reset to `None` whenever the pane drills to another level so each
    /// level starts from its own configured default.
    group_by_override: Option<Option<GroupBy>>,

    /// Runtime override of `tree_aggregate` columns' shown value (M4). The
    /// `toggle_tree_aggregate` action flips this without touching the YAML
    /// defaults:
    ///
    /// - `None` → each `tree_aggregate` column follows its own `default`
    ///   (`own` / `cumulated`).
    /// - `Some(false)` → all `tree_aggregate` columns show their own value.
    /// - `Some(true)` → all show the adapter's cumulated value.
    ///
    /// Tree-mode only; ignored by flat views. The TUI never folds the tree
    /// itself — the cumulated value is the adapter-supplied `cumulated_field`.
    tree_aggregate_override: Option<bool>,

    /// User-configured column visibility/order per level (column-config
    /// popup, `c`). Keyed by [`Self::column_level_key`] — one entry per
    /// view root / drilled child / tree chain — and holding the visible
    /// column keys in display order. Levels without an entry show their
    /// YAML-configured columns unchanged. The owning [`ContentView`]
    /// holds the source-of-truth map and mirrors it into every pane
    /// (including freshly split ones), so a per-level override applies
    /// uniformly across splits.
    column_overrides: std::collections::HashMap<String, Vec<String>>,

    /// Card mode per level (the `card.key` toggle). Keyed exactly like
    /// [`Self::column_overrides`], holding the user's explicit choice for
    /// that level; a level without an entry follows its `card.default`.
    /// Mirrored from the owning [`ContentView`] like the column overrides,
    /// and persisted by the App, so a toggled mode survives a restart.
    card_mode_overrides: std::collections::HashMap<String, bool>,

    /// Columns the adapter *describes* per node type
    /// ([`ContentAdapter::describe_columns`]), keyed by `node_type.type_id`.
    /// The owning [`ContentView`] fetches these on load and mirrors them into
    /// every pane. Their `value_type` is authoritative for rendering: a
    /// described column overrides the matching YAML column's `kind`, so a
    /// backend-owned column (custom columns) is typed correctly without the
    /// view YAML having to restate `kind:`. Empty until the first fetch lands.
    column_schema: std::collections::HashMap<String, Vec<not_yet_done_content::ColumnSchema>>,

    /// Capabilities of the owning view's adapter, snapshotted once at pane
    /// construction (the adapter is fixed for a `ContentView`'s lifetime).
    /// Lets pane-local logic gate UI affordances on what the adapter can
    /// actually do, independent of (and in addition to) the YAML config —
    /// e.g. `toggle_tree_aggregate` requires `supports_tree_aggregation`.
    /// Defaults to all-false (no adapter), which hides every capability-gated
    /// affordance. This is the generic capability-gating path: future
    /// affordances read the relevant flag here rather than re-deriving it.
    capabilities: not_yet_done_content::AdapterCapabilities,

    /// Inline pictures for this pane's `markdown: true` bodies: the decoded
    /// cache, the download wish-list and the terminal-protocol objects.
    /// Shared with `table` (which paints through it) via `Rc`, and borrowed
    /// by [`Self::rebuild_table`] while it renders markdown.
    ///
    /// Per pane, not per view: a pane is one scroll position over one list,
    /// so its pictures die with it. Two panes on the same chat download twice
    /// — an acceptable price for not threading a shared store through the
    /// dozen places that build a pane.
    images: Rc<RefCell<ImageStore>>,
}

// ── Pane tree ────────────────────────────────────────────────────────

/// Stable per-leaf identifier scoped to a [`ContentView`]. The counter
/// never recycles inside a tab's lifetime, so async load callbacks can
/// route back to the original pane even after siblings have been closed.
pub type PaneId = u32;

/// How a [`PaneNode::Branch`] divides its area between its two children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitOrientation {
    /// Side-by-side split: `first` on the left, `second` on the right
    /// (`Layout::horizontal`).
    Horizontal,
    /// Stacked split: `first` on top, `second` on the bottom
    /// (`Layout::vertical`).
    Vertical,
}

/// Where the new leaf lands when splitting an existing leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitSide {
    /// New leaf becomes `first` (left for Horizontal / top for Vertical).
    First,
    /// New leaf becomes `second` (right for Horizontal / bottom for Vertical).
    Second,
}

/// A leaf node in the pane tree — owns one [`ContentPane`].
pub struct LeafEntry {
    pub id: PaneId,
    pub pane: ContentPane,
}

/// Recursive binary pane tree. Branch nodes carry no id of their own
/// because routing only ever lands on a leaf.
pub enum PaneNode {
    Leaf(LeafEntry),
    Branch {
        orientation: SplitOrientation,
        /// Share allocated to `first` (e.g. `0.5` for an even split).
        ratio: f32,
        first: Box<PaneNode>,
        second: Box<PaneNode>,
    },
    /// Internal placeholder used during structural mutations
    /// ([`PaneTree::split_focus`] / [`PaneTree::close_focus`]). Should
    /// never be observable via the public API.
    #[doc(hidden)]
    Hole,
}

impl PaneNode {
    pub fn find_leaf(&self, id: PaneId) -> Option<&LeafEntry> {
        match self {
            PaneNode::Leaf(leaf) if leaf.id == id => Some(leaf),
            PaneNode::Leaf(_) | PaneNode::Hole => None,
            PaneNode::Branch { first, second, .. } => {
                first.find_leaf(id).or_else(|| second.find_leaf(id))
            }
        }
    }

    pub fn find_leaf_mut(&mut self, id: PaneId) -> Option<&mut LeafEntry> {
        match self {
            PaneNode::Leaf(leaf) if leaf.id == id => Some(leaf),
            PaneNode::Leaf(_) | PaneNode::Hole => None,
            PaneNode::Branch { first, second, .. } => {
                first.find_leaf_mut(id).or_else(|| second.find_leaf_mut(id))
            }
        }
    }

    pub fn collect_leaf_ids(&self, out: &mut Vec<PaneId>) {
        match self {
            PaneNode::Leaf(leaf) => out.push(leaf.id),
            PaneNode::Hole => {}
            PaneNode::Branch { first, second, .. } => {
                first.collect_leaf_ids(out);
                second.collect_leaf_ids(out);
            }
        }
    }

    /// Visit every leaf pane mutably (depth-first). Used by the repaint
    /// handler to rebuild live (`kind: elapsed`) panes in place without
    /// cloning the view defs per pane.
    pub fn for_each_leaf_mut(&mut self, f: &mut impl FnMut(&mut LeafEntry)) {
        match self {
            PaneNode::Leaf(leaf) => f(leaf),
            PaneNode::Hole => {}
            PaneNode::Branch { first, second, .. } => {
                first.for_each_leaf_mut(f);
                second.for_each_leaf_mut(f);
            }
        }
    }

    /// Replace the leaf with id `target` with a Branch wrapping the old
    /// leaf and `new_leaf`. `side` decides where the new leaf lands;
    /// `ratio` is the share allocated to `first` of the resulting branch.
    /// Returns `Err(new_leaf)` if `target` was not found in this subtree —
    /// the caller can retry on a sibling subtree.
    fn split_leaf(
        &mut self,
        target: PaneId,
        orientation: SplitOrientation,
        ratio: f32,
        new_leaf: LeafEntry,
        side: SplitSide,
    ) -> Result<(), LeafEntry> {
        match self {
            PaneNode::Leaf(leaf) if leaf.id == target => {
                let old = std::mem::replace(self, PaneNode::Hole);
                let (first, second) = match side {
                    SplitSide::Second => (Box::new(old), Box::new(PaneNode::Leaf(new_leaf))),
                    SplitSide::First => (Box::new(PaneNode::Leaf(new_leaf)), Box::new(old)),
                };
                *self = PaneNode::Branch {
                    orientation,
                    ratio,
                    first,
                    second,
                };
                Ok(())
            }
            PaneNode::Leaf(_) | PaneNode::Hole => Err(new_leaf),
            PaneNode::Branch { first, second, .. } => {
                match first.split_leaf(target, orientation, ratio, new_leaf, side) {
                    Ok(()) => Ok(()),
                    Err(returned) => second.split_leaf(target, orientation, ratio, returned, side),
                }
            }
        }
    }

    /// Recursively render this subtree, allocating a `Rect` per leaf
    /// according to the orientation/ratio of the branches above it.
    /// `multi` is `true` when the tree has 2+ leaves overall — leaves
    /// then get a focus-coloured border around their content.
    fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        focused_id: PaneId,
        multi: bool,
        last_rects: &mut HashMap<PaneId, Rect>,
        pane_tags: &HashMap<PaneId, char>,
        theme: &Theme,
        header_overlay: &crate::components::sort_header::HeaderOverlay,
    ) {
        match self {
            PaneNode::Leaf(leaf) => {
                last_rects.insert(leaf.id, area);
                let is_focused = leaf.id == focused_id;
                let inner = if multi {
                    let border_style = if is_focused {
                        Style::default().fg(theme.accent())
                    } else {
                        Style::default().fg(theme.text_dim())
                    };
                    let mut block = Block::default()
                        .borders(Borders::ALL)
                        .border_style(border_style);
                    if let Some(&letter) = pane_tags.get(&leaf.id) {
                        let title_style = if is_focused {
                            Style::default()
                                .fg(theme.accent())
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(theme.text_dim())
                        };
                        block = block.title(Span::styled(format!(" {letter} "), title_style));
                    }
                    let inner_area = block.inner(area);
                    frame.render_widget(block, area);
                    inner_area
                } else {
                    area
                };
                let table_area = leaf.pane.render_table_and_preview(frame, inner);
                if is_focused && header_overlay.is_active() {
                    let keys: Vec<&str> = leaf
                        .pane
                        .last_column_keys
                        .iter()
                        .map(|s| s.as_str())
                        .collect();
                    let style = Style::default().fg(theme.accent());
                    crate::components::sort_header::render_direction_picker_overlay(
                        frame,
                        table_area,
                        &keys,
                        &leaf.pane.last_col_widths,
                        2,
                        header_overlay,
                        style,
                    );
                }
            }
            PaneNode::Branch {
                orientation,
                ratio,
                first,
                second,
            } => {
                let r1 = (*ratio * 100.0).clamp(1.0, 99.0) as u16;
                let r2 = 100u16.saturating_sub(r1);
                let constraints = [Constraint::Percentage(r1), Constraint::Percentage(r2)];
                let chunks = match orientation {
                    SplitOrientation::Horizontal => Layout::horizontal(constraints).split(area),
                    SplitOrientation::Vertical => Layout::vertical(constraints).split(area),
                };
                first.render(
                    frame,
                    chunks[0],
                    focused_id,
                    multi,
                    last_rects,
                    pane_tags,
                    theme,
                    header_overlay,
                );
                second.render(
                    frame,
                    chunks[1],
                    focused_id,
                    multi,
                    last_rects,
                    pane_tags,
                    theme,
                    header_overlay,
                );
            }
            PaneNode::Hole => {}
        }
    }

    /// Remove the leaf with id `target`. The parent branch is replaced
    /// by its surviving sibling. Returns `true` if the target was found
    /// and removed somewhere in the subtree.
    fn close_leaf(&mut self, target: PaneId) -> bool {
        // Branch-with-target-as-direct-child case: hoist the surviving sibling.
        let drain_first = matches!(
            self,
            PaneNode::Branch { first, .. }
                if matches!(first.as_ref(), PaneNode::Leaf(leaf) if leaf.id == target)
        );
        let drain_second = matches!(
            self,
            PaneNode::Branch { second, .. }
                if matches!(second.as_ref(), PaneNode::Leaf(leaf) if leaf.id == target)
        );
        if drain_first || drain_second {
            let owned = std::mem::replace(self, PaneNode::Hole);
            if let PaneNode::Branch { first, second, .. } = owned {
                let surviving = if drain_first { *second } else { *first };
                *self = surviving;
                return true;
            }
            unreachable!();
        }

        if let PaneNode::Branch { first, second, .. } = self {
            first.close_leaf(target) || second.close_leaf(target)
        } else {
            false
        }
    }
}

/// Per-subtab pane tree.
pub struct PaneTree {
    /// Index into `ContentView.view_defs` that this subtab represents.
    /// Fixed for the lifetime of the tree.
    pub view_def_index: usize,
    pub root: PaneNode,
    /// Currently focused leaf within this tree.
    pub focus: PaneId,
    /// Cached screen rect of each leaf from the most recent render.
    /// Populated by the render path; consumed by future geometric
    /// focus-movement.
    pub last_rects: HashMap<PaneId, Rect>,
    /// Letter assigned to each leaf — drives the `Ctrl-w <letter>` focus
    /// switcher and the tag glyph rendered top-left in the pane border.
    /// Persistent for the lifetime of a `PaneId`; freed slots are reused
    /// on the next allocation.
    pub pane_tags: HashMap<PaneId, char>,
}

impl PaneTree {
    fn new(view_def_index: usize, leaf_id: PaneId, pane: ContentPane) -> Self {
        Self {
            view_def_index,
            root: PaneNode::Leaf(LeafEntry { id: leaf_id, pane }),
            focus: leaf_id,
            last_rects: HashMap::new(),
            pane_tags: HashMap::new(),
        }
    }

    /// Reserve the lowest still-free letter from `alphabet` for `pane_id`.
    /// Idempotent: re-assigning a pane that already has a tag is a no-op.
    /// Returns `None` if the alphabet is exhausted (every letter is in
    /// use by another live pane in this tree).
    fn assign_tag(&mut self, pane_id: PaneId, alphabet: &str) -> Option<char> {
        if let Some(&existing) = self.pane_tags.get(&pane_id) {
            return Some(existing);
        }
        let used: std::collections::HashSet<char> = self.pane_tags.values().copied().collect();
        let next = alphabet.chars().find(|c| !used.contains(c))?;
        self.pane_tags.insert(pane_id, next);
        Some(next)
    }

    fn release_tag(&mut self, pane_id: PaneId) {
        self.pane_tags.remove(&pane_id);
    }

    /// Look up the leaf wearing this letter.
    pub fn pane_id_for_tag(&self, letter: char) -> Option<PaneId> {
        self.pane_tags
            .iter()
            .find_map(|(id, &c)| (c == letter).then_some(*id))
    }

    pub fn focused_leaf(&self) -> &LeafEntry {
        self.root
            .find_leaf(self.focus)
            .expect("PaneTree.focus must point to a valid leaf")
    }

    pub fn focused_leaf_mut(&mut self) -> &mut LeafEntry {
        let id = self.focus;
        self.root
            .find_leaf_mut(id)
            .expect("PaneTree.focus must point to a valid leaf")
    }

    pub fn leaf_count(&self) -> usize {
        let mut ids = Vec::new();
        self.root.collect_leaf_ids(&mut ids);
        ids.len()
    }

    /// Find the structural sibling of `id`: the first leaf inside the
    /// **other** side of the deepest branch that contains `id`. Returns
    /// `None` when `id` is the only leaf in the tree (or absent).
    pub fn sibling_of(&self, id: PaneId) -> Option<PaneId> {
        fn find_other<'a>(node: &'a PaneNode, target: PaneId) -> Option<&'a PaneNode> {
            if let PaneNode::Branch { first, second, .. } = node {
                if let PaneNode::Leaf(l) = first.as_ref() {
                    if l.id == target {
                        return Some(second);
                    }
                }
                if let PaneNode::Leaf(l) = second.as_ref() {
                    if l.id == target {
                        return Some(first);
                    }
                }
                return find_other(first, target).or_else(|| find_other(second, target));
            }
            None
        }
        let other = find_other(&self.root, id)?;
        let mut leaves = Vec::new();
        other.collect_leaf_ids(&mut leaves);
        leaves.into_iter().next()
    }

    /// Split the focused leaf along `orientation`. The new leaf becomes
    /// the focus. `side` decides whether the new leaf lands as `first`
    /// (left/top) or `second` (right/bottom). `ratio` is the share given
    /// to the resulting branch's `first` child.
    fn split_focus(
        &mut self,
        orientation: SplitOrientation,
        ratio: f32,
        side: SplitSide,
        new_id: PaneId,
        new_pane: ContentPane,
    ) {
        let focus_id = self.focus;
        let new_leaf = LeafEntry {
            id: new_id,
            pane: new_pane,
        };
        // The closest-to-leaf split fits because we only split the focused
        // leaf and `focus_id` exists exactly once in the tree.
        let _ = self
            .root
            .split_leaf(focus_id, orientation, ratio, new_leaf, side);
        self.focus = new_id;
    }

    /// Close a specific leaf by id. Used both for the user's "close
    /// focused pane" action and for the coupled-pane cascade where the
    /// parent close needs to take its linked child with it. Returns
    /// `false` if the leaf was not in this tree or it would have been
    /// the last surviving leaf.
    fn close_specific(&mut self, id: PaneId) -> bool {
        if self.leaf_count() <= 1 {
            return false;
        }
        if !self.root.close_leaf(id) {
            return false;
        }
        if self.focus == id {
            let mut ids = Vec::new();
            self.root.collect_leaf_ids(&mut ids);
            if let Some(&first) = ids.first() {
                self.focus = first;
            }
        }
        true
    }
}

// ── ContentView ──────────────────────────────────────────────────────

/// Which kind of menu the shared [`QueryMenuComponent`] is currently
/// presenting. The component itself is data-agnostic; this enum tells
/// `handle_query_popup_key` which persistence layer to hit.
#[derive(Debug, Clone)]
pub enum QueryMenuMode {
    /// Default: DB-backed saved-query picker (Jira / Taiga / Tasks).
    SavedQueries,
    /// Per-node scripts held by the adapter's
    /// [`ScriptStore`](not_yet_done_content::ScriptStore), for a level
    /// with `node_scripts: true`. Carries the owning node's id in the
    /// adapter's own form — the view never parses it.
    NodeScripts { node_id: String },
}

pub struct ContentView {
    pub theme: Arc<Theme>,
    pub action_bar: ActionBarComponent,
    /// Cross-cutting "active" state pushed by the App once per frame via
    /// [`sync_action_bar`], stored so the hint builder can stamp each
    /// [`ActionHint`]'s `active` flag. (Jump-mode is read live from the
    /// pane and needs no storage.)
    active_editor: Option<String>,
    /// Action id of the currently-focused content editor (e.g.
    /// `"convert:userstory"`), so a modal `custom` action's bar hint lights
    /// up while its editor is open. `None` when no content editor is focused.
    content_editor_action_id: Option<String>,
    /// Action id the open content-action picker popup was launched for (e.g.
    /// `"convert"`), App-owned (`content_action_popup`). `None` when closed.
    content_action_popup_id: Option<String>,
    tracking_active: bool,
    cut_active: bool,
    /// A content-delete confirmation popup is open (App-owned
    /// `pending_confirmation` is a `DeleteContentNode`).
    confirm_active: bool,
    /// The column-config popup is open (App-owned `column_config_popup`).
    column_config_active: bool,
    /// A detached script is running (App-owned `detached_script`).
    script_active: bool,
    /// App-global action-bar hints (the `BarPlacement::Active` globals, e.g.
    /// the shortcut menu) pushed in once per frame via [`sync_action_bar`].
    /// These belong in the action bar with the activatable shortcuts, but are
    /// owned by the App (their key binding and active state live on `App`, not
    /// a content view), so the view only stores and appends them — it never
    /// resolves their `active` flag. Appended after the view's own hints.
    global_action_hints: Vec<ActionHint>,
    /// Command-line component, driven by `:`. Tab-global — operates
    /// on the active pane.
    pub cmdline: CmdlineComponent,
    pub tab_name: String,
    pub tab_icon: String,
    pub tab_order: i32,
    /// Per-tab switch-key override from the view file's `tab.key`. `None`
    /// falls back to the positional autonumber digit; `Some` with an empty
    /// list means the tab-switch key is disabled. See [`Self::tab_key_override`].
    pub tab_key: Option<crate::config::keybindings::KeyBinding>,
    /// `tab.unread_marker` from the view file — the glyph the tab bar puts
    /// in front of this tab's label while the view holds unread items.
    /// `None` falls back to the view's own `unread_marker`.
    tab_unread_marker: Option<String>,
    /// `tab.unread_style` from the view file — how the tab's label itself is
    /// emphasised while unread. `None` renders it bold.
    tab_unread_style: Option<crate::config::view_config::TabUnreadStyle>,
    /// `tab.load_banner` from the view file — this tab's override for where
    /// its load banner goes. Kept next to the resolved value so a config
    /// reload can re-resolve without re-reading the YAML.
    tab_load_banner: Option<LoadBannerRoute>,
    /// Where this tab's load banner goes: the `tab.load_banner` override when
    /// it has one, else `notifications.load_banner`, filled in by App via
    /// [`Self::set_load_banner_default`]. The banner line below draws a `Busy`
    /// state only while this is [`LoadBannerRoute::Tab`]; the other two routes
    /// are the App's business, which owns the cross-tab surface.
    load_banner_route: LoadBannerRoute,
    /// Tab id within the App's `content_views` vector. Set by App
    /// after construction. Used as the `view_index` field on the
    /// outgoing `ViewRequest`s.
    pub view_index: usize,

    pub adapter: Option<Arc<dyn ContentAdapter>>,
    pub view_defs: Vec<ViewDef>,
    /// One [`PaneTree`] per `view_defs` entry. Each tree starts as a
    /// single leaf; Phase 2 splits push a `Branch` over the focused
    /// leaf and add a sibling.
    pane_trees: Vec<PaneTree>,
    /// Index into both `view_defs` and `pane_trees` for the active subtab.
    active_subtab: usize,
    /// Counter for the next-allocated [`PaneId`]. Monotonic across the
    /// lifetime of this `ContentView` so async fetches keep routing
    /// even after siblings close.
    next_pane_id: PaneId,

    /// Live auth/connection status pushed by the App's status watcher.
    pub auth_status: AdapterStatus,
    /// Permanent error captured at adapter-construction time.
    pub adapter_init_error: Option<String>,

    /// Query menu popup (saved query picker OR adapter-native script
    /// manager). Tab-level overlay; the [`query_menu_mode`] field
    /// decides how `handle_query_popup_key` routes the emitted
    /// [`QueryMenuMessage`].
    query_menu: QueryMenuComponent,
    /// Group-by menu (M3, `content.group_menu`): a fixed hotkey popup over
    /// the same states `cycle_grouping` walks. Reuses the tab-set popup
    /// chrome; acts on the active pane's `group_by_override`.
    group_menu: TabSetPopup,
    /// Current routing for `query_menu`. `SavedQueries` is the default
    /// (DB-backed list); `PostgresScripts` means the popup is showing
    /// per-table on-disk `.sql` scripts.
    query_menu_mode: QueryMenuMode,
    /// Query menu keybindings (from global tui config).
    query_menu_kb: KeyBindingSection<QueryMenuAction>,
    /// Common keybindings shared with Tasks/Trackings: list navigation,
    /// scroll, column-config, etc. ContentView routes table cursor moves
    /// through this section so user-overrides in `tui.yaml` take effect.
    common_kb: KeyBindingSection<CommonAction>,
    /// Content-level keybindings: back, open, prev/next page, edit query.
    content_kb: KeyBindingSection<ContentAction>,
    /// Window/split keybindings. Each binding is a chord
    /// (e.g. `wv` = leader `w`, action key `v`).
    window_kb: KeyBindingSection<WindowAction>,
    /// `Some(leader)` while the user has pressed the window-leader and
    /// we are waiting for the resolution key. Stores the actual leader
    /// string so the chord lookup works for any user-configured prefix.
    window_pending: Option<String>,
    /// Alphabet from which per-pane letter tags are drawn, with letters
    /// reserved by the static `window_kb` bindings (v/s/q in defaults)
    /// already filtered out so chord-action keys never collide with
    /// pane-switch tags. Each leaf in the active subtab tree wears the
    /// lowest still-free letter; pressing `<leader><letter>` switches
    /// focus to that leaf.
    pane_tag_alphabet: String,
    /// Glyph map for status/action-bar hints (e.g. backspace → ⌫).
    key_icons: KeyIconMap,
    /// DB-persisted saved queries (merged with YAML defaults).
    pub db_saved_queries: Vec<MergedSavedQuery>,
    /// Name of the saved query marked as default (★ in the query menu).
    /// Persisted by the App as a settings row; applied automatically on
    /// app start instead of the view-YAML `query.default`.
    pub default_saved_query: Option<String>,
    /// `NodeRef`-style scope string for DB-side saved-query shortcuts
    /// (e.g. `"jira/jira/tickets"`). Today set once to the view-root
    /// NodeRef; future work may track the drill-down level.
    pub query_scope: String,

    /// Visual overlay applied to the column header row — drives the
    /// sort-hint mode (column picker / direction picker). Pushed in by
    /// `App` before each `rebuild_table` via the public field.
    pub header_overlay: crate::components::sort_header::HeaderOverlay,

    /// Path of the YAML config file this view was constructed from.
    /// `None` for the fallback Jira view created when no view yamls
    /// were found. Drives granular `:config` reload — the App finds
    /// the slot to replace by matching this path.
    pub source_path: Option<std::path::PathBuf>,

    /// Mirrors `AdapterConfig.manual_connect`. When `true`, App-level
    /// auto-load and subtab-switch loads are suppressed for this tab;
    /// the user must trigger a `reload` action to populate the pane.
    pub manual_connect: bool,

    /// Per-tab reminder handling, mirrored from `ViewFileConfig.reminder`.
    /// When present and `enabled`, the App subscribes to this tab's adapter
    /// reminder stream and runs `command` for each fired reminder. `None` →
    /// the tab ignores reminders entirely (no subscription). The adapter
    /// owns *when* a reminder fires; this decides *whether* and *what runs*.
    pub reminder: Option<ReminderConfig>,

    /// Set once any pane in this view has completed a load without an
    /// error — i.e. the (single, shared) adapter connection has been
    /// established. After that, switching to a sibling subtab auto-loads
    /// it instead of showing the `manual_connect` "press … to connect"
    /// banner: it is one adapter instance, so one connection serves every
    /// subtab. Only gates the *implicit* subtab-switch load; the first
    /// connect on a `manual_connect` tab still requires the explicit
    /// reload action.
    connected_once: bool,

    /// Cursor ids harvested when panes that hold an
    /// `active_custom_query.cursor_id` are destroyed (CP-6). Drained
    /// by the App after every interaction with this view — each id
    /// turns into a `ViewRequest::CloseAdapterCursor` so the adapter
    /// tears down the idle TX. Per-pane harvest happens in
    /// [`ContentView::close_focused`].
    pending_cursor_closes: Vec<String>,

    /// A `mark_read_on_reach_end` action queued by [`Self::handle_key`] when
    /// the selection just landed on the (unread) last row of a flat drill
    /// level. The App drains it right after `handle_key` (see
    /// [`Self::take_pending_mark_read`]) and dispatches it as a normal
    /// `InvokeNodeAction`, so the selection-changed side effects of the same
    /// keystroke still run. At most one is held — a fresh detection
    /// overwrites a stale one.
    pending_mark_read: Option<ViewRequest>,

    /// Lazy cache of per-node script shortcuts, keyed by the
    /// adapter-internal node id (for Postgres e.g.
    /// `live/schemas/public/tables/users`) and holding
    /// `(script_name, key_chord)` pairs (SQ-8d). Populated by the App when
    /// a script-owning node comes into focus and consulted by
    /// [`build_view_claims`] to register global apply-on-chord handlers
    /// symmetric to Jira/Taiga saved-query shortcuts. Cleared on bind /
    /// delete via [`Self::invalidate_node_script_shortcuts`].
    pub node_script_shortcuts: std::collections::HashMap<String, Vec<(String, String)>>,

    /// Lazy cache of `:script`-menu shortcuts, keyed by the focused level's
    /// script scope (`script:<tab>/<view_path…>`) and holding
    /// `(script_name, key_chord)` pairs. Populated by the App when a level
    /// that offers a `type: script` action comes into focus and consulted by
    /// [`build_view_claims`] to register apply-on-chord handlers symmetric to
    /// [`Self::node_script_shortcuts`]. Cleared on bind via
    /// `App::bind_script_shortcut`.
    pub script_shortcuts: std::collections::HashMap<String, Vec<(String, String)>>,

    /// Source of truth for user column-config overrides (popup `c`),
    /// keyed by [`ContentPane::column_level_key`]. Mirrored into every
    /// pane (including ones created by later splits/drills) so a level's
    /// layout is consistent across panes; persisted by the App as one
    /// JSON settings row per tab.
    column_overrides: std::collections::HashMap<String, Vec<String>>,

    /// Source of truth for the per-level card-mode choice (`card.key`),
    /// keyed like [`Self::column_overrides`]. Mirrored into every pane and
    /// persisted by the App as one JSON settings row per tab, so the mode a
    /// user toggled is still there after a restart.
    card_mode_overrides: std::collections::HashMap<String, bool>,

    /// Alphabet for vimium-style jump-mode labels (`content.jump_mode`,
    /// default `J`). Set once by the App from `navigation.jump_chars`.
    /// Applied to the focused pane's table at jump-open time, so panes
    /// created later by splits/drills pick it up without extra wiring.
    nav_chars: Vec<char>,
}

/// A `source: custom` column as the front-end knows it: the key it renders
/// under, a human label, and the canonical `value_type` derived from the
/// view YAML's `kind:`. Feeds the `edit-cells` [`InputSpec::ColumnForm`] so
/// the TUI can offer a typed editor over columns that may not yet exist in
/// the store (type-on-first-write). See [`ContentPane::custom_column_fields`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomColumnField {
    pub key: String,
    pub label: String,
    pub value_type: String,
}

impl ContentPane {
    /// Construct an empty pane for `view_def_index`. Pass
    /// `tree_enabled = true` when the matching `ViewDef` declares
    /// `tree_label`; the pane's [`TreeState`] is then initialized to
    /// an empty tree and rendering switches to tree mode.
    fn new(
        theme: Arc<Theme>,
        view_def_index: usize,
        tree_enabled: bool,
        capabilities: not_yet_done_content::AdapterCapabilities,
    ) -> Self {
        // The pane's picture store doubles as its table's painter — the
        // table only knows *where* a picture goes, the store knows how to
        // draw one. Wired here so every pane, however it was created,
        // renders inline images.
        let images = Rc::new(RefCell::new(ImageStore::new()));
        let mut table = DataTable::new();
        table.set_image_painter(images.clone());

        Self {
            view_def_index,
            theme,
            table,
            images,
            items: Vec::new(),
            fetch_error: None,
            filtered_indices: Vec::new(),
            nav_stack: Vec::new(),
            active_child: None,
            pending_cursor_on_open: None,
            active_query: None,
            active_query_name: None,
            active_query_vars: std::collections::HashMap::new(),
            active_query_kind: QueryKind::Saved,
            preview_open: false,
            preview_description: String::new(),
            preview_key: String::new(),
            preview_scroll: 0,
            preview_loading: false,
            preview_markdown: false,
            preview_visible_height: 0,
            search: SearchComponent::new(),
            fuzzy_filter_fields: Vec::new(),
            search_fields: Vec::new(),
            search_next_key: "n".to_string(),
            search_prev_key: "N".to_string(),
            search_mode: SearchMode::Local,
            text_search_query: None,
            current_sort: Vec::new(),
            current_page: None,
            last_applied_sort: Vec::new(),
            last_page_info: None,
            last_columns: Vec::new(),
            active_custom_query: None,
            last_col_widths: Vec::new(),
            built_table_width: 0,
            last_column_keys: Vec::new(),
            loaded: false,
            linked_child: None,
            detail_child: None,
            detail_source: None,
            detail_summary: None,
            detail_wrap: false,
            long_text: false,
            link_refs: std::collections::HashSet::new(),
            link_node_ref_prefix: None,
            retry_state: None,
            tree: tree_enabled.then(TreeState::new),
            tree_visible_indices: Vec::new(),
            tree_filter_depth: None,
            tree_filter_expand_stash: None,
            tree_find: None,
            pending_tree_find: None,
            eager_reload_preserve_expansion: false,
            eager_reload_reanchor_id: None,
            group_by_override: None,
            tree_aggregate_override: None,
            column_overrides: std::collections::HashMap::new(),
            card_mode_overrides: std::collections::HashMap::new(),
            column_schema: std::collections::HashMap::new(),
            capabilities,
        }
    }

    // ── Inline images ────────────────────────────────────────────────

    /// URLs the last markdown render wanted but didn't have yet, marked
    /// in-flight so a rebuild in between doesn't request them twice. The App
    /// drains this after every rebuild and fetches through the view's adapter
    /// — only the adapter knows how to authenticate against its host.
    pub fn take_wanted_images(&mut self) -> Vec<String> {
        self.images.borrow_mut().take_wanted()
    }

    /// Hand in what the download produced. `None` means it failed (or the
    /// bytes weren't a picture) and the URL is retired. Returns `true` when
    /// the table should be rebuilt so the picture gets its lines.
    pub fn insert_decoded_image(
        &mut self,
        url: &str,
        image: Option<crate::views::images::DecodedImage>,
    ) -> bool {
        self.images.borrow_mut().insert_decoded(url, image)
    }

    /// The terminal cell size the store decodes against, and the configured
    /// height cap — the App needs both to downscale off-thread.
    pub fn image_decode_params(&self) -> (u16, (u16, u16)) {
        let store = self.images.borrow();
        (store.max_height(), store.font_size())
    }

    // ── Tree-find lifecycle (CT-5) ───────────────────────────────────
    //
    // These wrap the `tree_find` field so the App-side dispatch
    // (CT-6/CT-7) never has to spell out the state shape. Each helper
    // is a one-liner today; the centralised wrappers keep the
    // "begin/complete/clear" contract obvious and let later phases
    // grow the state (e.g. last-seen path for "next from cursor")
    // without touching the call sites.

    /// Begin a fresh tree-find: stash the query, mark loading, clear
    /// any previous hits. Idempotent — a second call replaces the
    /// in-flight state (last writer wins; the App is responsible for
    /// ignoring stale completions if it queues parallel searches,
    /// which CT-6 doesn't).
    pub fn tree_find_begin(&mut self, query: String) {
        self.tree_find = Some(TreeFindState {
            query,
            hits: Vec::new(),
            current: 0,
            loading: true,
            truncated: false,
            settled: false,
            refreshed_paths: std::collections::HashSet::new(),
        });
    }

    /// Queue a `:tree-find` query to fire after the next root load (see
    /// [`Self::pending_tree_find`]).
    pub fn queue_pending_tree_find(&mut self, query: String) {
        self.pending_tree_find = Some(query);
    }

    /// Take the queued `:tree-find` query, if any, clearing it so it
    /// fires exactly once (on the load that follows the command's
    /// forced reload).
    pub fn take_pending_tree_find(&mut self) -> Option<String> {
        self.pending_tree_find.take()
    }

    /// Land hits from a successful adapter response. No-op when the
    /// state was cleared in the meantime (CT-9: Esc/r/new-search wipe
    /// `tree_find` before the in-flight call returns).
    pub fn tree_find_complete(&mut self, hits: Vec<TreeFindHit>, truncated: bool) {
        if let Some(state) = self.tree_find.as_mut() {
            state.hits = hits;
            state.current = 0;
            state.loading = false;
            state.truncated = truncated;
            state.settled = false;
            state.refreshed_paths.clear();
        }
    }

    /// Mark the in-flight search as failed: drop the loading flag but
    /// keep the (empty) hits + query so the status-bar hint can show
    /// "no matches". No-op when the state was cleared meanwhile.
    pub fn tree_find_fail(&mut self) {
        if let Some(state) = self.tree_find.as_mut() {
            state.loading = false;
            state.hits.clear();
            state.truncated = false;
        }
    }

    /// Drop the entire tree-find state. Called on Esc, reload, or
    /// when the user opens a new search input (CT-9).
    pub fn tree_find_clear(&mut self) {
        self.tree_find = None;
    }

    /// Advance the cursor to the next hit with wrap-around. Returns
    /// the newly-selected hit (or `None` when there are no hits / no
    /// active state — caller should no-op).
    pub fn tree_find_next(&mut self) -> Option<&TreeFindHit> {
        let state = self.tree_find.as_mut()?;
        if state.hits.is_empty() {
            return None;
        }
        state.current = (state.current + 1) % state.hits.len();
        state.settled = false;
        state.refreshed_paths.clear();
        state.hits.get(state.current)
    }

    /// Step the cursor to the previous hit with wrap-around. Returns
    /// the newly-selected hit (or `None` when empty / no state).
    pub fn tree_find_prev(&mut self) -> Option<&TreeFindHit> {
        let state = self.tree_find.as_mut()?;
        if state.hits.is_empty() {
            return None;
        }
        state.current = if state.current == 0 {
            state.hits.len() - 1
        } else {
            state.current - 1
        };
        state.settled = false;
        state.refreshed_paths.clear();
        state.hits.get(state.current)
    }

    /// Reference to the hit at the current cursor — i.e. what `n`/`N`
    /// would land on if pressed without movement. `None` when no
    /// state is active or `hits` is empty.
    pub fn tree_find_current(&self) -> Option<&TreeFindHit> {
        let state = self.tree_find.as_ref()?;
        state.hits.get(state.current)
    }

    /// `true` while a tree-find cache (loading or hits) is live. CT-7
    /// gates `n`/`N` on this so the keys jump tree-find hits instead
    /// of the local `/`-search match list.
    pub fn tree_find_active(&self) -> bool {
        self.tree_find.is_some()
    }

    /// `true` while the search input is open in adapter (`text_search`) mode
    /// — the user is typing a term that will be rendered into a query.
    pub fn text_search_input_open(&self) -> bool {
        self.search.active() && matches!(self.search_mode, SearchMode::Adapter { .. })
    }

    /// `true` while the pane still shows the result of the last accepted
    /// adapter text search, i.e. the query that search rendered is still the
    /// active one. Any other query (saved, default, hand-edited) replaces
    /// `active_query` and thereby ends the state — nothing has to reset it.
    pub fn text_search_applied(&self) -> bool {
        self.text_search_query.is_some() && self.text_search_query == self.active_query
    }

    /// CT-7: locate the visible-table row of a hit's leaf node, only
    /// if the path is already fully expanded + cached. Returns the
    /// row index into the rendered table (post fuzzy-filter) when
    /// found, else `None`. The full lazy-expand driver lives in
    /// [`Self::advance_tree_find`]; this helper is the read-only
    /// part used by both the driver's terminal step and tests.
    ///
    /// Matches on `(parent_path, leaf_id)` rather than `leaf_id`
    /// alone so two pages with the same id under different parents
    /// stay distinguishable.
    pub fn find_tree_find_visible_row(&self, hit: &TreeFindHit) -> Option<usize> {
        let tree = self.tree.as_ref()?;
        let (leaf_id, ancestors) = hit.path.split_last()?;
        for (row_idx, &entry_idx) in self.tree_visible_indices.iter().enumerate() {
            let entry = tree.entries.get(entry_idx)?;
            if entry.is_more_placeholder {
                continue;
            }
            if entry.node.id == *leaf_id && entry.parent_path.as_slice() == ancestors {
                return Some(row_idx);
            }
        }
        None
    }

    /// CT-7: wrap [`Self::advance_tree_find`] into a single
    /// `SubViewMessage` for the pane's key dispatch + App-side
    /// drivers. `Ready` collapses into a `SelectionChanged` (cursor
    /// already moved), `NeedRootLoad` / `NeedTreeExpand` into the
    /// matching `ViewRequest`, `Waiting` / `Idle` into a no-op
    /// `SelectionChanged`, and `NotInTree` into a status-bar notice.
    pub fn tree_find_dispatch_step(
        &mut self,
        view_index: usize,
        pane_id: PaneId,
        view_defs: &[ViewDef],
    ) -> SubViewMessage {
        if std::env::var_os("NYD_DEBUG_TREEFIND").is_some() {
            let (cur, total, path, settled) = self
                .tree_find
                .as_ref()
                .and_then(|s| {
                    s.hits
                        .get(s.current)
                        .map(|h| (s.current, s.hits.len(), h.path.clone(), s.settled))
                })
                .unwrap_or((0, 0, Vec::new(), false));
            treefind_walk_trace(format!(
                "step hit={cur}/{total} settled={settled} path={path:?}"
            ));
        }
        let advance = self.advance_tree_find(view_index, pane_id, view_defs);
        if std::env::var_os("NYD_DEBUG_TREEFIND").is_some() {
            let variant = match &advance {
                TreeFindAdvance::Ready(row) => format!("Ready(row={row})"),
                TreeFindAdvance::Waiting => "Waiting".to_string(),
                TreeFindAdvance::Idle => "Idle".to_string(),
                TreeFindAdvance::NeedRootLoad => "NeedRootLoad".to_string(),
                TreeFindAdvance::NeedTreeExpand {
                    parent_path,
                    parent_node_id,
                    ..
                } => {
                    format!("NeedTreeExpand(parent_path={parent_path:?} parent={parent_node_id})")
                }
                TreeFindAdvance::NotInTree(r) => format!("NotInTree({r})"),
            };
            treefind_walk_trace(format!("outcome={variant}"));
        }
        match advance {
            TreeFindAdvance::Ready(_) | TreeFindAdvance::Waiting | TreeFindAdvance::Idle => {
                SubViewMessage::SelectionChanged(None)
            }
            TreeFindAdvance::NeedRootLoad => {
                SubViewMessage::Request(ViewRequest::SpawnContentLoad {
                    view_index,
                    pane_id,
                })
            }
            TreeFindAdvance::NeedTreeExpand {
                parent_path,
                parent_node_id,
                child_node_type,
                page_size,
            } => SubViewMessage::Request(ViewRequest::ExpandTreeNode {
                view_index,
                pane_id,
                parent_path,
                parent_node_id,
                child_node_type,
                page_size,
                page: None,
                append: false,
            }),
            TreeFindAdvance::NotInTree(reason) => {
                SubViewMessage::Request(ViewRequest::Notify(format!("Tree find: {reason}",)))
            }
        }
    }

    /// CT-7: drive one step of the "expand-to-current-hit" walk for
    /// the active tree-find. The App-side caller consumes the result:
    /// dispatch the returned [`ViewRequest`] (if any), then re-poll
    /// once the response lands. Mutates `self` along the way (marks
    /// ancestor prefixes as expanded, rebuilds entries, positions
    /// cursor on `Ready`).
    ///
    /// Limitations of this first cut:
    /// - Single-load only (multi-load fan-out for heterogeneous tree
    ///   levels is reserved for the `Schemas + Scripts` pattern;
    ///   Confluence — the only adapter with `search_in_tree` — is
    ///   single-load).
    /// - The hit's path must address nodes in the current root list;
    ///   if the root pane hasn't loaded yet, it returns
    ///   `NeedRootLoad` and the caller must dispatch
    ///   `SpawnContentLoad` (re-poll after the items land).
    /// - When an ancestor `id` isn't present in its cached parent's
    ///   children, returns `NotInTree(reason)` — typical cause: a
    ///   filter / pagination cap excludes it. CT-9 will retry after
    ///   the user re-reloads.
    pub fn advance_tree_find(
        &mut self,
        view_index: usize,
        pane_id: PaneId,
        view_defs: &[ViewDef],
    ) -> TreeFindAdvance {
        let state = match self.tree_find.as_ref() {
            Some(s) => s,
            None => return TreeFindAdvance::Idle,
        };
        // Loading state — nothing to advance against yet.
        if state.loading {
            return TreeFindAdvance::Waiting;
        }
        // Already landed the cursor on the current hit — any later
        // TreeChildren (e.g. user expanded a node by hand) must not
        // snap the cursor back. `next`/`prev` (or a fresh search)
        // clear `settled` to re-arm.
        if state.settled {
            return TreeFindAdvance::Idle;
        }
        let hit = match state.hits.get(state.current) {
            Some(h) => h.clone(),
            None => return TreeFindAdvance::Idle,
        };
        let path = hit.path.clone();
        if path.is_empty() {
            return TreeFindAdvance::NotInTree(
                "Tree-find hit has an empty path — adapter bug".into(),
            );
        }
        let view_def = match self.view_def(view_defs).cloned() {
            Some(v) => v,
            None => return TreeFindAdvance::Idle,
        };
        let _ = view_index; // reserved for future per-view dispatch differentiation
        let _ = pane_id;
        if view_def.tree_label.is_none() || self.tree.is_none() {
            return TreeFindAdvance::NotInTree("Tree-find requires a tree-mode view".into());
        }

        // Walk each prefix of `path`. For depth d, the parent's
        // node-id list is `path[..d]` (empty for d=0 = root). We
        // need `cache[parent_path]` to be loaded and to contain
        // `path[d]` among its children. The first level that isn't
        // cached/loaded yields the next dispatch step.
        for d in 0..path.len() {
            let parent_path: Vec<String> = path[..d].to_vec();
            // Probe the cache slot into an owned verdict so the immutable
            // borrow of `self.tree` ends before we may mutate
            // `self.tree_find` (the stale-level refresh below).
            enum Slot {
                Missing,
                Loading,
                LoadedHas,
                LoadedMissing,
            }
            let slot = {
                let tree = self.tree.as_ref().expect("checked above");
                match tree.cache.get(&parent_path) {
                    None => Slot::Missing,
                    Some(e) if !e.loaded => Slot::Loading,
                    Some(e) if e.children.iter().any(|c| c.id == path[d]) => Slot::LoadedHas,
                    Some(_) => Slot::LoadedMissing,
                }
            };
            match slot {
                Slot::Missing => {
                    if d == 0 {
                        return TreeFindAdvance::NeedRootLoad;
                    }
                    // We need the ChildDef whose own level *is* `d`
                    // — its `node_type` is what we'll request from
                    // the parent's `list()`. `tree_self_at_depth`
                    // does this lookup correctly across recursive
                    // ChildDefs (where every depth ≥ 1 resolves to
                    // the same recursive def), unlike the
                    // depth-shifted `tree_child_def_at_depth`.
                    let Some(child_def) = tree_self_at_depth(&view_def, d) else {
                        return TreeFindAdvance::NotInTree(format!(
                            "View config has no tree-continuing child at depth {d}",
                        ));
                    };
                    return TreeFindAdvance::NeedTreeExpand {
                        parent_path,
                        parent_node_id: path[d - 1].clone(),
                        child_node_type: child_def.node_type.clone(),
                        page_size: 50,
                    };
                }
                Slot::Loading => return TreeFindAdvance::Waiting,
                Slot::LoadedHas => {}
                Slot::LoadedMissing => {
                    // Cached + loaded, but the expected child is absent.
                    // The classic cause is a stale level: a sibling was
                    // created after this level was last fetched — notably
                    // an external `nyd-t task add` (goto_task.py) whose new
                    // node isn't in this pane's lazily-expanded cache (the
                    // eager reload only renews up to `expand_depth`). Re-fetch
                    // the level ONCE (replacing the stale slot) so the new
                    // child surfaces; `refreshed_paths` bounds it to a single
                    // retry so a genuinely-absent id still degrades to
                    // `NotInTree` instead of looping.
                    let already = self
                        .tree_find
                        .as_ref()
                        .map(|s| s.refreshed_paths.contains(&parent_path))
                        .unwrap_or(true);
                    if !already {
                        if let Some(state) = self.tree_find.as_mut() {
                            state.refreshed_paths.insert(parent_path.clone());
                        }
                        if d == 0 {
                            return TreeFindAdvance::NeedRootLoad;
                        }
                        if let Some(child_def) = tree_self_at_depth(&view_def, d) {
                            return TreeFindAdvance::NeedTreeExpand {
                                parent_path,
                                parent_node_id: path[d - 1].clone(),
                                child_node_type: child_def.node_type.clone(),
                                page_size: 50,
                            };
                        }
                    }
                    return TreeFindAdvance::NotInTree(format!(
                        "Hit's ancestor '{}' at depth {d} not in loaded children",
                        path[d],
                    ));
                }
            }
        }

        // All ancestor prefixes are cached + path is addressable.
        // Mark every strict ancestor prefix as expanded so the
        // renderer surfaces the leaf, then rebuild + locate the row.
        {
            let tree = self.tree.as_mut().expect("checked above");
            for d in 0..path.len() {
                let prefix: Vec<String> = path[..d].to_vec();
                tree.expanded.insert(prefix);
            }
            tree.rebuild_entries(&view_def);
        }
        self.rebuild_table(view_defs);
        if let Some(row) = self.find_tree_find_visible_row(&hit) {
            self.table.set_selected(row);
            if let Some(state) = self.tree_find.as_mut() {
                state.settled = true;
            }
            TreeFindAdvance::Ready(row)
        } else {
            // Defensive: rebuild succeeded but the row isn't in the
            // visible-indices map. Usually means a fuzzy filter is
            // active and hides this leaf — surface as NotInTree so
            // the user knows to clear the filter.
            TreeFindAdvance::NotInTree("Hit's leaf row is hidden (active fuzzy filter?)".into())
        }
    }

    /// Replace the link cache + adapter NodeRef prefix used by the
    /// `has_links` column. Called by [`ContentView::set_link_refs`].
    pub fn set_link_context(
        &mut self,
        link_refs: &std::collections::HashSet<String>,
        prefix: Option<String>,
    ) {
        self.link_refs = link_refs.clone();
        self.link_node_ref_prefix = prefix;
    }

    // ── Shortcut hints ───────────────────────────────────────────────

    /// Collect every YAML `shortcuts:` entry visible at the current
    /// chain position — chain-aware in tree mode, drill-aware in flat
    /// mode. Deeper levels win on duplicate keys, mirroring
    /// `app::node_actions::resolve_shortcut`'s precedence. Returned
    /// entries are `(key, raw_value)`: the raw value still carries
    /// the `parent:` prefix when present, so the caller decides which
    /// node_id to look up actions on.
    fn current_shortcuts(&self, view_defs: &[ViewDef]) -> Vec<(char, String)> {
        let Some(vd) = self.view_def(view_defs) else {
            return Vec::new();
        };
        let chain = self.selected_node_type_chain(view_defs);
        // BTreeMap (not HashMap): callers feed the result straight into
        // the action / status bar in iteration order, so the order must
        // be stable across frames. HashMap::new() seeds its hasher per
        // instance — every per-frame rebuild would shuffle the hint
        // positions and the bar flickers visibly (3 actions on a
        // postgres db_script node was the trigger that surfaced this).
        // Sorted-by-char gives a deterministic, predictable layout.
        let mut out: std::collections::BTreeMap<char, String> = std::collections::BTreeMap::new();
        // Walk from deepest to root — first-seen wins. `resolve_shortcut`
        // does the same and `entry().or_insert_with()` keeps the
        // deeper-level binding.
        for end in (1..=chain.len()).rev() {
            if let Some(child) =
                crate::views::content_tree::child_def_for_type_chain(vd, &chain[..end])
            {
                for (k, v) in &child.shortcuts {
                    out.entry(*k).or_insert_with(|| v.action().to_string());
                }
            }
        }
        for (k, v) in &vd.shortcuts {
            out.entry(*k).or_insert_with(|| v.action().to_string());
        }
        out.into_iter().collect()
    }

    /// Resolve a YAML shortcut target to `(node_id, node_type)`. The
    /// `node_id` is the concrete instance to fetch from (any row of
    /// the right type works, the selected row is the cheapest choice);
    /// the `node_type` is the cache key under which the resulting
    /// actions list is stored. Returns `None` when the target isn't
    /// addressable from the current pane state (e.g. parent shortcut
    /// at root level, empty list, …).
    fn shortcut_target_ref(
        &self,
        raw_value: &str,
        view_defs: &[ViewDef],
    ) -> Option<(String, String)> {
        use crate::app::node_actions::{ShortcutTarget, parse_shortcut_value};
        let (target, _action_name) = parse_shortcut_value(raw_value);
        match target {
            ShortcutTarget::Selected => {
                let id = self.selected_item_id()?.to_string();
                let ty = self.selected_target_node_type(view_defs)?;
                Some((id, ty))
            }
            ShortcutTarget::Parent => {
                let id = self.selected_parent_node_id_for_shortcut()?;
                let ty = self.parent_target_node_type(view_defs)?;
                Some((id, ty))
            }
        }
    }

    /// Node-type id of the row currently under the cursor. Tree mode
    /// reads it from the entry's `node_type_chain`; flat mode from the
    /// selected `NodeSummary` (falling back to the chain's leaf when
    /// the items list is empty — e.g. while the first load is in
    /// flight).
    fn selected_target_node_type(&self, view_defs: &[ViewDef]) -> Option<String> {
        if self.tree.is_some() {
            let row = self.table.selected_row();
            let entry = self.tree_entry_at_row(row)?;
            return entry.node_type_chain.last().cloned();
        }
        let row = self.table.selected_row();
        if let Some(item) = self.items.get(row) {
            return Some(item.node_type.type_id.clone());
        }
        self.view_path_node_types(view_defs).last().cloned()
    }

    /// Node-type id of the *parent* of the row currently under the
    /// cursor (for `parent:`-prefixed shortcuts). Tree mode walks the
    /// entry's `node_type_chain` back one step; flat mode uses the
    /// view-path chain (the level above the active child).
    fn parent_target_node_type(&self, view_defs: &[ViewDef]) -> Option<String> {
        if self.tree.is_some() {
            let row = self.table.selected_row();
            let entry = self.tree_entry_at_row(row)?;
            let n = entry.node_type_chain.len();
            return if n >= 2 {
                entry.node_type_chain.get(n - 2).cloned()
            } else {
                None
            };
        }
        let chain = self.view_path_node_types(view_defs);
        let n = chain.len();
        if n >= 2 {
            chain.get(n - 2).cloned()
        } else {
            None
        }
    }

    /// Cousin of the private `selected_parent_node_id` in
    /// [`Self::handle_key`]'s neighbourhood — same logic, exposed
    /// inside this `impl` for the cache-trigger and hint-render
    /// paths. Returns `None` at root (no parent to look up).
    fn selected_parent_node_id_for_shortcut(&self) -> Option<String> {
        if self.tree.is_some() {
            let row = self.table.selected_row();
            let entry = self.tree_entry_at_row(row)?;
            return entry.parent_path.last().cloned();
        }
        self.parent_node_id().map(str::to_string)
    }

    /// Render shortcut hints for the current chain position by
    /// joining each visible YAML `shortcuts:` entry with the
    /// adapter's `actions_for_type()` lookup. Returns one
    /// [`ShortcutHint`] per resolvable entry (unknown node_type or
    /// action_id → drop silently), carrying the adapter's `placement`
    /// and the [`ActiveSurface`] derived from the action's `id` + input
    /// shape. Caller splits by placement for the action / status bars.
    ///
    /// Synchronous: the adapter is required to answer without I/O
    /// (`actions_for_type` is instance-free and type-keyed). No
    /// `get_by_id` walk, no DB round-trip per cursor move.
    fn collect_shortcut_hints(
        &self,
        view_defs: &[ViewDef],
        adapter: Option<&dyn not_yet_done_content::ContentAdapter>,
    ) -> Vec<ShortcutHint> {
        use crate::app::node_actions::parse_shortcut_value;
        use crate::views::content_action_hints::source_for_shortcut;
        let Some(adapter) = adapter else {
            return Vec::new();
        };
        let shortcuts = self.current_shortcuts(view_defs);
        if shortcuts.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (key, raw) in shortcuts {
            let (_target, action_name) = parse_shortcut_value(&raw);
            let Some((_id, node_type)) = self.shortcut_target_ref(&raw, view_defs) else {
                continue;
            };
            let nt = not_yet_done_content::NodeType {
                type_id: node_type,
                mime_type: String::new(),
                syntax: None,
                file_extension: String::new(),
                display_name: String::new(),
            };
            let actions = adapter.actions_for_type(&nt);
            if let Some(action) = actions.iter().find(|a| a.id == action_name) {
                let opens_input = !matches!(action.input, not_yet_done_content::InputSpec::None);
                let source = source_for_shortcut(&action.id, &action.label, opens_input);
                out.push(ShortcutHint {
                    key: key.to_string(),
                    label: action.label.clone(),
                    source,
                });
            }
        }
        out
    }

    /// Lookup helper for the `has_links` column. Returns `true` if a
    /// link row exists for `"{prefix}/{item_id}"`. `false` when the
    /// adapter context isn't set yet.
    fn item_has_link(&self, item_id: &str) -> bool {
        let Some(prefix) = self.link_node_ref_prefix.as_deref() else {
            return false;
        };
        self.link_refs.contains(&format!("{prefix}/{item_id}"))
    }

    pub fn view_def_index(&self) -> usize {
        self.view_def_index
    }

    fn view_def<'a>(&self, view_defs: &'a [ViewDef]) -> Option<&'a ViewDef> {
        view_defs.get(self.view_def_index)
    }

    /// Whether the level this pane currently shows opts into the
    /// record-detail split (`record_detail: true`). Resolved like
    /// [`Self::current_columns`]'s flat path: a drilled child reads its
    /// `ChildDef.record_detail`, otherwise the view root's
    /// `ViewDef.record_detail`. Tree levels are intentionally excluded —
    /// the record-detail split targets wide *flat* rows (Postgres rows /
    /// script results), and a tree already expands records inline. A
    /// pane that is itself a detail follower never re-offers the toggle.
    fn record_detail_enabled(&self, view_defs: &[ViewDef]) -> bool {
        if self.detail_source.is_some() || self.tree.is_some() {
            return false;
        }
        if let Some(ref child) = self.active_child {
            return child.record_detail;
        }
        self.view_def(view_defs)
            .map(|vd| vd.record_detail)
            .unwrap_or(false)
    }

    /// `true` when this pane is the detail-follower half of a
    /// record-detail split (its rows are the transposed fields of the
    /// source pane's selected record).
    fn is_detail_pane(&self) -> bool {
        self.detail_source.is_some()
    }

    /// The source pane this follower mirrors, if this pane is a
    /// record-detail follower. `None` for ordinary panes. Lets callers
    /// redirect data operations (e.g. a post-script reload) to the real
    /// source instead of the synthetic follower, whose items are produced
    /// by [`Self::detail_items`](content_detail::detail_items), not a fetch.
    pub fn detail_source(&self) -> Option<PaneId> {
        self.detail_source
    }

    /// Resolve this tree's connector color: the active view's per-view
    /// `tree_connector_style` (a theme color name) if set, else the global
    /// theme `tree_connector`. Used to fill [`TREE_CONNECTOR_STYLE_ID`].
    /// Whether this pane's current level holds an unread row — the tree's
    /// loaded nodes in tree mode, the plain item list otherwise. Feeds
    /// [`ContentView::has_unread`]; see there for why only the current level
    /// counts.
    fn has_unread(&self) -> bool {
        match self.tree.as_ref() {
            Some(tree) => tree
                .entries
                .iter()
                .any(|e| metadata_field_value(&e.node, "unread") == "true"),
            None => self
                .items
                .iter()
                .any(|it| metadata_field_value(it, "unread") == "true"),
        }
    }

    fn tree_connector_color(&self, view_defs: &[ViewDef], t: &Theme) -> ratatui::style::Color {
        self.view_def(view_defs)
            .and_then(|vd| vd.tree_connector_style.as_deref())
            .map(|name| resolve_theme_color(t, name))
            .unwrap_or_else(|| t.tree_connector())
    }

    /// Resolve this view's unread-highlight color: the active view's per-view
    /// `unread_style` (a theme color name) if set, else the global theme
    /// `unread`. Used to fill [`UNREAD_STYLE_ID`].
    fn unread_color(&self, view_defs: &[ViewDef], t: &Theme) -> ratatui::style::Color {
        self.view_def(view_defs)
            .and_then(|vd| vd.unread_style.as_deref())
            .map(|name| resolve_theme_color(t, name))
            .unwrap_or_else(|| t.unread())
    }

    /// The leading marker glyph prefixed to unread tree rows / message headers
    /// in this view: the per-view `unread_marker` if set, else the default
    /// `💬`. May be empty (marker suppressed, color-only highlight).
    fn unread_marker<'a>(&self, view_defs: &'a [ViewDef]) -> &'a str {
        self.view_def(view_defs)
            .and_then(|vd| vd.unread_marker.as_deref())
            .unwrap_or(DEFAULT_UNREAD_MARKER)
    }

    /// Resolve this tree's drawing options from the view's `tree_lines` /
    /// `tree_markers` config: whether the `├──`/`└──`/`│` line connectors
    /// are drawn (default yes) and which expand/collapse markers prefix
    /// expandable rows (default `▶`/`▼`; empty when markers are disabled).
    fn tree_draw_options<'a>(&self, view_defs: &'a [ViewDef]) -> TreeDrawOptions<'a> {
        let vd = self.view_def(view_defs);
        let markers = vd.and_then(|v| v.tree_markers.as_ref());
        let enabled = markers.and_then(|m| m.enabled).unwrap_or(true);
        let (collapsed_marker, expanded_marker) = if enabled {
            (
                markers.and_then(|m| m.collapsed.as_deref()).unwrap_or("▶"),
                markers.and_then(|m| m.expanded.as_deref()).unwrap_or("▼"),
            )
        } else {
            ("", "")
        };
        TreeDrawOptions {
            lines: vd.and_then(|v| v.tree_lines).unwrap_or(true),
            collapsed_marker,
            expanded_marker,
        }
    }

    // ── Current-level config (respects nav depth) ───────────────────

    /// Whether the active level has any live (time-derived) column whose
    /// rendering changes between frames without a refetch — currently only
    /// `kind: elapsed` (M5). The App's repaint handler uses this to rebuild
    /// just the panes that actually tick, leaving the rest's cached rows be.
    fn has_live_column(&self, view_defs: &[ViewDef]) -> bool {
        self.current_columns(view_defs)
            .iter()
            .any(|c| c.kind == ColumnKind::Elapsed)
    }

    fn current_columns(&self, view_defs: &[ViewDef]) -> Vec<ColumnDef> {
        // Record-detail follower: always the synthetic field|value pair,
        // regardless of what the source level configures. The width of
        // the field column is clamped to the longest field label so the
        // value column gets the rest of the pane.
        if self.is_detail_pane() {
            let field_width = self
                .items
                .iter()
                .filter_map(|it| {
                    it.metadata
                        .fields
                        .iter()
                        .find(|f| f.key == content_detail::FIELD_KEY)
                        .map(|f| f.value.chars().count())
                })
                .max()
                .unwrap_or(content_detail::FIELD_COL_MIN);
            return content_detail::detail_columns(field_width);
        }
        if self.tree.is_some() {
            // Chain-based: the cursor row's own branch decides the column
            // set, so a multi-branch tree shows the right columns at a
            // given depth instead of the first branch's. When the cursor
            // can't be resolved (the tree shrank under it — e.g. a new
            // query replaced the entries — or the filtered tree is empty),
            // fall back to the root level: an empty column set would abort
            // `rebuild_table` before `set_data`, leaving the widget
            // painting the previous (stale) rows.
            let level = self.cursor_tree_level(view_defs).or_else(|| {
                self.view_def(view_defs)
                    .and_then(|vd| tree_level_at_depth(vd, 0))
            });
            let tree_label = level.as_ref().map(|l| l.tree_label.to_string());
            let mut cols = level.map(|l| l.columns.to_vec()).unwrap_or_default();
            if let Some(visible) = self
                .column_level_key(view_defs)
                .and_then(|k| self.column_overrides.get(&k))
            {
                // The tree-label column carries the tree itself and is
                // never hideable; keep it even against a stale override.
                cols = apply_column_override(cols, visible, tree_label.as_deref());
            } else {
                // No explicit override → drop columns flagged `hidden` from
                // the default layout (keeping the tree-label column).
                cols.retain(|c| column_shown_by_default(c, tree_label.as_deref()));
            }
            self.merge_described_kinds(&mut cols);
            return cols;
        }
        let configured: &[ColumnDef] = if let Some(ref child) = self.active_child {
            &child.columns
        } else if let Some(vd) = self.view_def(view_defs) {
            &vd.columns
        } else {
            &[]
        };
        if !configured.is_empty() {
            let mut cols = configured.to_vec();
            if let Some(visible) = self
                .column_level_key(view_defs)
                .and_then(|k| self.column_overrides.get(&k))
            {
                cols = apply_column_override(cols, visible, None);
            } else {
                // No explicit override → drop columns flagged `hidden` from
                // the default layout.
                cols.retain(|c| column_shown_by_default(c, None));
            }
            // An aggregate's `total_column` only carries per-group totals;
            // with grouping cycled off the column would be permanently
            // blank, so it is hidden then (matching the native trackings
            // view, where Total only appears while grouped).
            let total_targets: Vec<String> = self
                .current_aggregates(view_defs)
                .iter()
                .filter_map(|a| a.total_column.clone())
                .collect();
            if !total_targets.is_empty() && self.current_levels(view_defs).is_empty() {
                cols.retain(|c| !total_targets.contains(&c.key));
            }
            self.merge_described_kinds(&mut cols);
            return cols;
        }
        // Auto-fallback: derive one ColumnDef per metadata field of the
        // first item, all evenly sized. Used by the postgres rows view
        // (and any other dynamic-schema adapter) where the YAML cannot
        // enumerate columns ahead of time.
        let Some(first) = self.items.first() else {
            return Vec::new();
        };
        first
            .metadata
            .fields
            .iter()
            .map(|f| ColumnDef {
                key: f.key.clone(),
                label: Some(f.display_label.clone()),
                source: None,
                collapsed_source: None,
                long_source: None,
                style: None,
                sizing: "auto".into(),
                markdown: false,
                kind: ColumnKind::Text,
                format: None,
                separator: None,
                elapsed_from: None,
                tree_aggregate: None,
                hidden: false,
            })
            .collect()
    }

    /// The `source: custom` columns configured for the pane's active level, as
    /// typed [`CustomColumnField`]s. This is the front-end's own view of the
    /// lib-owned custom-column set: the `edit-cells` [`InputSpec::ColumnForm`]
    /// builds its fields from here (not from the backend schema), so a column
    /// that has never been stored still gets an input, and its YAML `kind:`
    /// supplies the `value_type` the TUI sends on submit — letting the store
    /// bootstrap the column on first write (type-on-first-write).
    pub fn custom_column_fields(&self, view_defs: &[ViewDef]) -> Vec<CustomColumnField> {
        self.current_columns(view_defs)
            .into_iter()
            .filter(|c| c.source.as_deref() == Some("custom"))
            .map(|c| CustomColumnField {
                label: c.label.clone().unwrap_or_else(|| c.key.clone()),
                value_type: value_type_from_column_kind(c.kind).to_string(),
                key: c.key,
            })
            .collect()
    }

    /// Stable identity of the level whose columns the pane currently
    /// shows, used as the key into `column_overrides`. One key per
    /// configurable coordinate:
    ///
    /// - tree mode → `tree:{node_type_chain}` of the cursor row (each
    ///   branch/depth configures independently, mirroring how
    ///   `current_columns` resolves the column set),
    /// - drilled into a child → `child:{view}/{child name}`,
    /// - view root → `view:{view name}`.
    ///
    /// `None` when the level has no YAML-configured columns (the
    /// auto-fallback derives them from item metadata — e.g. postgres
    /// rows — so there is nothing stable to configure against).
    fn column_level_key(&self, view_defs: &[ViewDef]) -> Option<String> {
        let vd = self.view_def(view_defs)?;
        if self.tree.is_some() {
            let chain = self.cursor_node_type_chain();
            // An empty/unresolvable cursor level has no column set to
            // configure (empty tree); root level has an empty chain but
            // resolves fine.
            let level = self.cursor_tree_level(view_defs)?;
            let owner_len = column_owner_chain_len(vd, &chain, level.columns);
            return Some(format!("tree:{}/{}", vd.name, chain[..owner_len].join("/")));
        }
        if let Some(ref child) = self.active_child {
            if child.columns.is_empty() {
                return None;
            }
            return Some(format!("child:{}/{}", vd.name, child.name));
        }
        if vd.columns.is_empty() {
            return None;
        }
        Some(format!("view:{}", vd.name))
    }

    /// Mirror the owning view's override map into this pane. Called on
    /// construction of split panes and whenever the map changes so all
    /// panes of a tab agree on per-level column layouts.
    fn set_column_overrides(&mut self, overrides: std::collections::HashMap<String, Vec<String>>) {
        self.column_overrides = overrides;
    }

    /// Replace this pane's described-column schema for one node type (fetched
    /// by the owning view on load).
    fn set_column_schema(
        &mut self,
        node_type: String,
        schema: Vec<not_yet_done_content::ColumnSchema>,
    ) {
        self.column_schema.insert(node_type, schema);
    }

    /// Override each column's `kind` with the backend-described `value_type`
    /// for a matching key. Custom-column keys are unique per column, so a flat
    /// lookup across every cached node type is enough (and covers tree levels
    /// whose node type isn't the one in `self.items`). YAML stays the source of
    /// truth for width/order/visibility; only the type is taken from the
    /// backend. Unknown/`text` types leave the YAML `kind` untouched.
    fn merge_described_kinds(&self, cols: &mut [ColumnDef]) {
        if self.column_schema.is_empty() {
            return;
        }
        for col in cols.iter_mut() {
            if let Some(kind) = self
                .column_schema
                .values()
                .flatten()
                .find(|s| s.key == col.key)
                .and_then(|s| column_kind_from_value_type(&s.value_type))
            {
                col.kind = kind;
            }
        }
    }

    /// The active level's **raw** configured columns — the YAML truth the
    /// column-config popup edits, before any user override is applied —
    /// plus the tree-label key in tree mode (that column is not hideable).
    /// `None` when the level has no configured columns (auto-fallback
    /// levels derive theirs from item metadata and aren't configurable).
    fn column_config_source(
        &self,
        view_defs: &[ViewDef],
    ) -> Option<(Vec<ColumnDef>, Option<String>)> {
        if self.tree.is_some() {
            let level = self.cursor_tree_level(view_defs)?;
            return Some((level.columns.to_vec(), Some(level.tree_label.to_string())));
        }
        let configured: &[ColumnDef] = if let Some(ref child) = self.active_child {
            &child.columns
        } else if let Some(vd) = self.view_def(view_defs) {
            &vd.columns
        } else {
            &[]
        };
        if configured.is_empty() {
            return None;
        }
        Some((configured.to_vec(), None))
    }

    /// The active level's multi-line `row_layout`, if any. Tree mode never
    /// uses it (multi-line rendering targets flat drill lists for now, e.g.
    /// the Stoat message list), so it returns `None` there.
    fn current_row_layout(&self, view_defs: &[ViewDef]) -> Option<Vec<LineLayout>> {
        if self.is_detail_pane() || self.tree.is_some() {
            return None;
        }
        if let Some(ref child) = self.active_child {
            child.row_layout.clone()
        } else {
            self.view_def(view_defs)
                .and_then(|vd| vd.row_layout.clone())
        }
    }

    /// The active level's `card:` block, if it declares one. Read from the
    /// same level as [`current_row_layout`](Self::current_row_layout) and with
    /// the same restriction: tree levels and the record-detail follower pane
    /// have no card mode (v1 targets flat lists).
    fn current_card(&self, view_defs: &[ViewDef]) -> Option<CardConfig> {
        if self.is_detail_pane() || self.tree.is_some() {
            return None;
        }
        if let Some(ref child) = self.active_child {
            child.card.clone()
        } else {
            self.view_def(view_defs).and_then(|vd| vd.card.clone())
        }
    }

    /// Whether card mode is *available* here — the active level declares a
    /// `card:` block. Gates the toggle key and its status-bar hint, so the
    /// configured key stays free on every level without cards.
    fn card_available(&self, view_defs: &[ViewDef]) -> bool {
        self.current_card(view_defs).is_some()
    }

    /// Whether the active level renders as cards *right now*: it declares a
    /// `card:` block and either the user's stored per-level choice or (absent
    /// that) the block's `default:` says so.
    fn card_mode_active(&self, view_defs: &[ViewDef]) -> bool {
        let Some(card) = self.current_card(view_defs) else {
            return false;
        };
        self.column_level_key(view_defs)
            .and_then(|k| self.card_mode_overrides.get(&k).copied())
            .unwrap_or(card.default)
    }

    /// The key that toggles card mode on this level: the level's own
    /// `card.key`, else a global `content.toggle_card_mode` binding if the
    /// user set one. `None` → the mode is only reachable via `card.default`.
    fn card_toggle_binding(
        &self,
        view_defs: &[ViewDef],
        content_kb: &KeyBindingSection<ContentAction>,
    ) -> Option<KeyBinding> {
        self.current_card(view_defs)?
            .key
            .clone()
            .or_else(|| content_kb.get(&ContentAction::ToggleCardMode).cloned())
    }

    /// Mirror the owning view's card-mode map into this pane (same contract
    /// as [`set_column_overrides`](Self::set_column_overrides)).
    fn set_card_mode_overrides(&mut self, overrides: std::collections::HashMap<String, bool>) {
        self.card_mode_overrides = overrides;
    }

    /// Translate the level's `card:` config into a layout spec for the table
    /// engine, resolving each field's label (explicit `label:` → the column's
    /// `label:` → its key).
    ///
    /// Fields whose column isn't in `columns` are dropped: `columns` is the
    /// *visible* set, so hiding a column in the column-config popup (`c`)
    /// also removes it from the card instead of leaving an empty slot.
    fn card_spec(&self, card: &CardConfig, columns: &[ColumnDef]) -> CardSpec {
        let label_of =
            |col: &ColumnDef| -> String { col.label.clone().unwrap_or_else(|| col.key.clone()) };
        // An omitted `fields:` means "every column of this level", in the
        // effective column order — so the card tracks the table rather than
        // repeating its column list. `markdown:` columns drop out either way:
        // they expand into N soft-wrapped lines and have no fixed-height slot.
        let fields: Vec<CardField> = if card.fields.is_empty() {
            columns
                .iter()
                .filter(|c| !c.markdown)
                .map(|c| CardField::new(c.key.clone(), label_of(c)))
                .collect()
        } else {
            card.fields
                .iter()
                .filter_map(|f| {
                    let col = columns.iter().find(|c| c.key == f.column)?;
                    let label = f.label.clone().unwrap_or_else(|| label_of(col));
                    Some(CardField::new(f.column.clone(), label))
                })
                .collect()
        };
        CardSpec {
            fields,
            columns: card.columns.max(1),
            weights: card.weights.clone(),
            labels: match card.labels {
                CardLabelMode::None => CardLabels::None,
                CardLabelMode::Inline => CardLabels::Inline,
                CardLabelMode::Above => CardLabels::Above,
            },
            border: match card.border {
                CardBorderMode::None => CardBorder::None,
                CardBorderMode::Plain => CardBorder::Plain,
                CardBorderMode::Rounded => CardBorder::Rounded,
            },
            padding: card.padding,
            gap: card.gap,
            separator: card.separator.clone(),
            divider: card.divider.clone(),
        }
    }

    /// Whether the active level opts into smooth (line-wise) scrolling.
    /// Read from the same level as [`current_row_layout`](Self::current_row_layout)
    /// (active `ChildDef`, else the root `ViewDef`); defaults to `false`.
    fn current_smooth_scroll(&self, view_defs: &[ViewDef]) -> bool {
        if let Some(ref child) = self.active_child {
            child.smooth_scroll
        } else {
            self.view_def(view_defs)
                .map(|vd| vd.smooth_scroll)
                .unwrap_or(false)
        }
    }

    /// The active flat drill level's `mark_read_on_reach_end` action id, if
    /// configured. Only the drilled-in `ChildDef` carries it (the hook is a
    /// flat-list, reach-the-end notion); the root `ViewDef` never does.
    fn mark_read_action(&self) -> Option<&str> {
        self.active_child
            .as_ref()
            .and_then(|c| c.mark_read_on_reach_end.as_deref())
    }

    /// The active level's configured `group_by` (M3), read from the same
    /// level as [`current_columns`](Self::current_columns). A runtime
    /// `cycle_grouping` override (`group_by_override`) takes precedence so
    /// the user can regroup or turn grouping off without an adapter
    /// round-trip.
    ///
    /// Tree mode: the engine never groups a tree itself (the adapter owns
    /// the fold), so this is `None` there — *unless* the adapter advertises
    /// `group_by_via_adapter`, in which case the root `ViewDef`'s
    /// `group_by` (or the override) names the grouping the adapter is asked
    /// to apply (see [`adapter_group_spec`](Self::adapter_group_spec));
    /// regrouping then means a reload, not a rebuild.
    fn current_group_by(&self, view_defs: &[ViewDef]) -> Option<GroupBy> {
        if self.tree.is_some() {
            if !self.capabilities.group_by_via_adapter {
                return None;
            }
            if let Some(ovr) = &self.group_by_override {
                return ovr.clone();
            }
            return self.view_def(view_defs).and_then(|vd| vd.group_by.clone());
        }
        if let Some(ovr) = &self.group_by_override {
            return ovr.clone();
        }
        if let Some(ref child) = self.active_child {
            child.group_by.clone()
        } else {
            self.view_def(view_defs).and_then(|vd| vd.group_by.clone())
        }
    }

    /// The active level's `aggregates` (M3). Read from the same level as
    /// [`current_group_by`](Self::current_group_by); empty in tree mode or
    /// when none are configured.
    fn current_aggregates(&self, view_defs: &[ViewDef]) -> Vec<AggregateDef> {
        if self.tree.is_some() {
            return Vec::new();
        }
        if let Some(ref child) = self.active_child {
            child.aggregates.clone()
        } else {
            self.view_def(view_defs)
                .map(|vd| vd.aggregates.clone())
                .unwrap_or_default()
        }
    }

    /// The effective grouping level, or empty when the active level configures
    /// no `group_by` at all (flat list) or the user has cycled grouping off.
    /// Engine grouping is single-level; an adapter that wants finer-grained
    /// condensing pre-condenses its rows itself. This is what the grouped
    /// render path consumes — and that path is **flat-only**: in tree mode this
    /// is always empty, even when the adapter groups the tree itself
    /// (`group_by_via_adapter` makes [`current_group_by`] non-`None` there, but
    /// those groups arrive as tree *nodes*, not as engine-built group headers).
    fn current_levels(&self, view_defs: &[ViewDef]) -> Vec<GroupBy> {
        if self.is_detail_pane() || self.tree.is_some() {
            return Vec::new();
        }
        if !self.level_has_group_by(view_defs) {
            return Vec::new();
        }
        self.current_group_by(view_defs).into_iter().collect()
    }

    /// Whether the active level *configures* a `group_by` (M3). Unlike
    /// [`current_group_by`](Self::current_group_by) this ignores the runtime
    /// override, so the `cycle_grouping` key stays claimable even after the
    /// user has cycled the view to "ungrouped".
    ///
    /// In tree mode this gates on the adapter capability instead of the
    /// active child: only an adapter that groups the tree itself
    /// (`group_by_via_adapter`) plus a root `group_by` in the config makes
    /// `zg`/`u` meaningful there (both then reload through the adapter).
    fn level_has_group_by(&self, view_defs: &[ViewDef]) -> bool {
        if self.tree.is_some() {
            return self.capabilities.group_by_via_adapter
                && self
                    .view_def(view_defs)
                    .map(|vd| vd.group_by.is_some())
                    .unwrap_or(false);
        }
        if let Some(ref child) = self.active_child {
            child.group_by.is_some()
        } else {
            self.view_def(view_defs)
                .map(|vd| vd.group_by.is_some())
                .unwrap_or(false)
        }
    }

    /// Rotate the runtime date-bucket granularity of the active level's
    /// grouping (M3, `content.cycle_grouping`). The configured `group_by`
    /// names the column to bucket; this walks
    /// `ungrouped → Day → Week → Month → Year → ungrouped`, storing the
    /// result in [`group_by_override`](Self::group_by_override). A no-op
    /// when the active level (and override) declare no `group_by` — there
    /// is then no column to bucket. Returns `true` when the state changed
    /// (the caller rebuilds the table).
    fn cycle_grouping(&mut self, view_defs: &[ViewDef]) -> bool {
        let current_bucket = self.current_group_by(view_defs).and_then(|gb| gb.bucket);
        let grouped_now = self.current_group_by(view_defs).is_some();
        let next = next_bucket_state(grouped_now, current_bucket);
        self.set_grouping_bucket(next, view_defs)
    }

    /// The column to bucket (and the configured group order) — from the
    /// *configured* default (or the current override). `None` when the
    /// active level declares no `group_by` at all: there is then nothing
    /// to (re)group. Shared base of [`cycle_grouping`](Self::cycle_grouping)
    /// and [`set_grouping_bucket`](Self::set_grouping_bucket).
    fn configured_grouping_base(&self, view_defs: &[ViewDef]) -> Option<(String, GroupOrder)> {
        // A tree only regroups through the adapter; without that capability
        // there is nothing a runtime override could apply to (this guards
        // the action-chain path, which bypasses the key-claim gate).
        if self.tree.is_some() && !self.capabilities.group_by_via_adapter {
            return None;
        }
        self.current_group_by(view_defs)
            .map(|gb| (gb.column.clone(), gb.order))
            .or_else(|| {
                if let Some(ref child) = self.active_child {
                    child
                        .group_by
                        .as_ref()
                        .map(|gb| (gb.column.clone(), gb.order))
                } else {
                    self.view_def(view_defs)
                        .and_then(|vd| vd.group_by.as_ref().map(|gb| (gb.column.clone(), gb.order)))
                }
            })
    }

    /// Jump the runtime grouping of the active level directly to a date
    /// bucket (`None` = ungrouped) — the group-by menu's counterpart of
    /// the stepwise [`cycle_grouping`](Self::cycle_grouping). Returns
    /// `true` when applied (the caller rebuilds the table).
    pub(crate) fn set_grouping_bucket(
        &mut self,
        bucket: Option<DateBucket>,
        view_defs: &[ViewDef],
    ) -> bool {
        let Some((column, order)) = self.configured_grouping_base(view_defs) else {
            return false;
        };
        self.group_by_override = Some(bucket.map(|bucket| GroupBy {
            column,
            bucket: Some(bucket),
            order,
        }));
        true
    }

    /// Whether this pane's grouping is applied **adapter-side**: tree mode
    /// plus an adapter that advertises `group_by_via_adapter`. Regrouping is
    /// then a reload (the adapter re-folds per bucket), not an engine
    /// rebuild.
    fn tree_groups_via_adapter(&self) -> bool {
        self.tree.is_some() && self.capabilities.group_by_via_adapter
    }

    /// The active `group_headers:` rendering config — `Some` only while the
    /// adapter-grouped tree is actually grouped (capability + view config +
    /// grouping not cycled off). The depth-0 rows are then the adapter's
    /// group buckets and render as `── label` header rows; with grouping
    /// off the adapter returns plain rows at depth 0 and the tree renders
    /// normally.
    fn tree_group_headers_def<'a>(&self, view_defs: &'a [ViewDef]) -> Option<&'a GroupHeadersDef> {
        if !self.tree_groups_via_adapter() || self.current_group_by(view_defs).is_none() {
            return None;
        }
        self.view_def(view_defs)?.group_headers.as_ref()
    }

    /// The grouping to hand the adapter in `ListParams::group_by` for this
    /// pane's **root** load — the content-crate [`GroupSpec`] twin of the
    /// effective [`current_group_by`](Self::current_group_by). `None`
    /// outside the adapter-grouped-tree case (flat lists group engine-side;
    /// the adapter must never see a grouping it isn't asked to apply) and
    /// when the user has cycled grouping off (the adapter then returns the
    /// plain tree).
    pub(crate) fn adapter_group_spec(&self, view_defs: &[ViewDef]) -> Option<GroupSpec> {
        if !self.tree_groups_via_adapter() {
            return None;
        }
        self.current_group_by(view_defs).map(|gb| GroupSpec {
            column: gb.column,
            bucket: gb.bucket.map(to_group_bucket),
            order: match gb.order {
                GroupOrder::Asc => SortDirection::Asc,
                GroupOrder::Desc => SortDirection::Desc,
            },
        })
    }

    /// Dispatch wrapper for `content.cycle_grouping`: rotate the grouping
    /// granularity and, when it changed, rebuild the table — or, when the
    /// adapter owns the grouping (adapter-grouped tree), request a reload of
    /// the current level so the adapter re-buckets. Returns
    /// `SelectionChanged(None)` so the bars refresh, or `Unhandled` when the
    /// active level declares no `group_by` (nothing to cycle).
    pub(crate) fn try_cycle_grouping(
        &mut self,
        view_defs: &[ViewDef],
        view_index: usize,
        pane_id: PaneId,
    ) -> SubViewMessage {
        if self.cycle_grouping(view_defs) {
            if self.tree_groups_via_adapter() {
                return self.reload_current_level(view_index, pane_id);
            }
            self.rebuild_table(view_defs);
            SubViewMessage::SelectionChanged(None)
        } else {
            SubViewMessage::Unhandled
        }
    }

    /// Flip the **group ordering** (asc ⟷ desc) of the active grouped level,
    /// preserving the bucket granularity and leaving item order within groups
    /// untouched. Records the flipped [`GroupBy`] in
    /// [`group_by_override`](Self::group_by_override). A no-op when the level
    /// declares no `group_by` or when grouping is currently cycled off
    /// (`Some(None)` override — there is no bucket order to flip). Returns
    /// `true` when the state changed (the caller rebuilds / reloads).
    fn toggle_group_order(&mut self, view_defs: &[ViewDef]) -> bool {
        // Effective grouping (override → child → view). `None` = level isn't
        // groupable; nothing to flip.
        let Some(gb) = self.current_group_by(view_defs) else {
            return false;
        };
        let flipped = match gb.order {
            GroupOrder::Asc => GroupOrder::Desc,
            GroupOrder::Desc => GroupOrder::Asc,
        };
        self.group_by_override = Some(Some(GroupBy {
            column: gb.column,
            bucket: gb.bucket,
            order: flipped,
        }));
        true
    }

    /// Dispatch wrapper for `content.toggle_group_order`: flip the bucket
    /// order and, when it changed, rebuild the table — or, when the adapter
    /// owns the grouping (adapter-grouped tree), reload the current level so
    /// the adapter re-buckets in the new order. Returns `SelectionChanged`
    /// so the bars refresh, or `Unhandled` when there is no grouping to flip.
    pub(crate) fn try_toggle_group_order(
        &mut self,
        view_defs: &[ViewDef],
        view_index: usize,
        pane_id: PaneId,
    ) -> SubViewMessage {
        if self.toggle_group_order(view_defs) {
            if self.tree_groups_via_adapter() {
                return self.reload_current_level(view_index, pane_id);
            }
            self.rebuild_table(view_defs);
            SubViewMessage::SelectionChanged(None)
        } else {
            SubViewMessage::Unhandled
        }
    }

    /// Whether long-text mode is offered here: at least one column in the
    /// active level declares a `long_source`. Gates the `toggle_long_text`
    /// key and its status-bar hint, so `v` stays free on every view whose
    /// columns opt out. Config-only (no adapter capability needed — the full
    /// field is already in the row's metadata).
    fn long_text_available(&self, view_defs: &[ViewDef]) -> bool {
        self.current_columns(view_defs)
            .iter()
            .any(|c| c.long_source.is_some())
    }

    /// Toggle long-text mode (`v`): flip the pane flag and rebuild the table
    /// so a `long_source` column re-renders as a soft-wrapped multi-line
    /// block (or back to a single fitted line). A no-op — key unclaimed —
    /// when no column declares `long_source`.
    pub(crate) fn try_toggle_long_text(
        &mut self,
        view_defs: &[ViewDef],
        _view_index: usize,
        _pane_id: PaneId,
    ) -> SubViewMessage {
        if !self.long_text_available(view_defs) {
            return SubViewMessage::Unhandled;
        }
        self.long_text = !self.long_text;
        self.rebuild_table(view_defs);
        SubViewMessage::SelectionChanged(None)
    }

    // ── Tree-fold aggregation (M4) ───────────────────────────────────

    /// Whether the active (cursor-depth) tree level declares any
    /// `tree_aggregate` column *and* the adapter advertises
    /// `supports_tree_aggregation`. Gates the `toggle_tree_aggregate` action
    /// and its hint, analogous to [`level_has_group_by`](Self::level_has_group_by)
    /// for `cycle_grouping`. `false` outside tree mode.
    ///
    /// Two gates, both required (this is the generic capability-gating path):
    ///
    /// 1. **Config** — a column declares `tree_aggregate` (the view author
    ///    opted the column into the fold).
    /// 2. **Capability** — the adapter's `supports_tree_aggregation` is set,
    ///    snapshotted into [`capabilities`](Self::capabilities) at pane
    ///    construction. An adapter that cannot supply a cumulated value (or
    ///    no adapter at all) leaves this `false`, so the toggle stays a no-op
    ///    and its key stays unclaimable even if a stray config declares the
    ///    column. Config alone is not enough.
    fn level_has_tree_aggregate(&self, view_defs: &[ViewDef]) -> bool {
        self.tree.is_some()
            && self.capabilities.supports_tree_aggregation
            && self
                .current_columns(view_defs)
                .iter()
                .any(|c| c.tree_aggregate.is_some())
    }

    /// The effective global cumulated-state used by `toggle_tree_aggregate`
    /// to decide which way to flip. The runtime override wins; otherwise it
    /// derives from whether any `tree_aggregate` column defaults to
    /// `cumulated`. (Per-column rendering still respects each column's own
    /// `default` while the override is `None` — see
    /// [`column_shows_cumulated`](Self::column_shows_cumulated).)
    fn tree_aggregate_cumulated_now(&self, view_defs: &[ViewDef]) -> bool {
        if let Some(v) = self.tree_aggregate_override {
            return v;
        }
        self.current_columns(view_defs).iter().any(|c| {
            c.tree_aggregate
                .as_ref()
                .map(|ta| ta.default == TreeAggregateDefault::Cumulated)
                .unwrap_or(false)
        })
    }

    /// Whether a single `tree_aggregate` column should render its cumulated
    /// value right now: the pane-wide override if set, else the column's own
    /// `default`. Read per row build in [`build_tree_data_rows`].
    fn column_shows_cumulated(&self, col: &ColumnDef) -> bool {
        match (&self.tree_aggregate_override, &col.tree_aggregate) {
            (Some(v), _) => *v,
            (None, Some(ta)) => ta.default == TreeAggregateDefault::Cumulated,
            (None, None) => false,
        }
    }

    /// Flip every `tree_aggregate` column between its own and the adapter's
    /// cumulated value (M4, `content.toggle_tree_aggregate`). A no-op when the
    /// active level declares no `tree_aggregate` column. Returns `true` when
    /// the state changed (the caller rebuilds the table).
    fn toggle_tree_aggregate(&mut self, view_defs: &[ViewDef]) -> bool {
        if !self.level_has_tree_aggregate(view_defs) {
            return false;
        }
        let next = !self.tree_aggregate_cumulated_now(view_defs);
        self.tree_aggregate_override = Some(next);
        true
    }

    /// Dispatch wrapper for `content.toggle_tree_aggregate`: flip the shown
    /// value and, when it changed, rebuild the table. Returns
    /// `SelectionChanged(None)` so the bars refresh, or `Unhandled` when the
    /// active level declares no `tree_aggregate` column.
    pub(crate) fn try_toggle_tree_aggregate(&mut self, view_defs: &[ViewDef]) -> SubViewMessage {
        if self.toggle_tree_aggregate(view_defs) {
            self.rebuild_table(view_defs);
            SubViewMessage::SelectionChanged(None)
        } else {
            SubViewMessage::Unhandled
        }
    }

    // ── Tree mode helpers ────────────────────────────────────────────

    /// Depth of the currently selected tree entry. Drives the column
    /// set / header used for rendering. Defaults to `0` (root level)
    /// when no row is selected or the tree is empty.
    fn tree_active_depth(&self) -> usize {
        self.tree_entry_at_row(self.table.selected_row())
            .map(|e| e.depth)
            .unwrap_or(0)
    }

    /// Resolve a visible-row index to its entry in `tree.entries`,
    /// going through `tree_visible_indices` so an active fuzzy filter
    /// (which hides some entries) doesn't desync row → entry.
    fn tree_entry_at_row(&self, row: usize) -> Option<&crate::views::content_tree::TreeEntry> {
        let tree = self.tree.as_ref()?;
        let eidx = self.tree_visible_indices.get(row).copied()?;
        tree.entries.get(eidx)
    }

    /// `node_type_chain` of the cursor row — its exact coordinate in a
    /// (possibly multi-branch) tree. Empty when the tree is empty or no
    /// row is selected. Per-row config lookups key off this instead of
    /// the lossy `depth`, which can't tell branches apart.
    fn cursor_node_type_chain(&self) -> Vec<String> {
        self.tree_entry_at_row(self.table.selected_row())
            .map(|e| e.node_type_chain.clone())
            .unwrap_or_default()
    }

    /// Resolve the [`TreeLevel`] of the cursor row via its
    /// `node_type_chain`. Single source of truth for the active column
    /// set, label column, actions and preview in tree mode — replaces
    /// the `*_at_depth(tree_active_depth())` lookups that silently
    /// follow the *first* branch and so mis-resolve on multi-branch
    /// trees (the root cause of the recurring blank-label-row bug).
    fn cursor_tree_level<'a>(&self, view_defs: &'a [ViewDef]) -> Option<TreeLevel<'a>> {
        let vd = self.view_def(view_defs)?;
        let entry = self.tree_entry_at_row(self.table.selected_row())?;
        tree_level_for_chain(vd, &entry.node_type_chain)
    }

    /// Walk the tree chain to find the depth whose level defines a
    /// `fuzzy_filter` action. Validator ensures at most one level has
    /// one, so the first match is the answer. Returns `None` outside
    /// tree mode or when no level configures fuzzy_filter.
    fn resolve_tree_filter_depth(&self, view_defs: &[ViewDef]) -> Option<usize> {
        let vd = self.view_def(view_defs)?;
        self.tree.as_ref()?;
        for depth in 0..32 {
            let level = tree_level_at_depth(vd, depth)?;
            if level
                .actions
                .iter()
                .any(|a| a.action_type == "fuzzy_filter")
            {
                return Some(depth);
            }
        }
        None
    }

    /// Recompute `tree_visible_indices` from `tree.entries`, applying the
    /// active fuzzy filter (if any) as a **path-pruning** tree filter: an
    /// entry survives iff it matches the query tokens itself *or* has a
    /// surviving descendant. So matches plus the ancestor chain that leads
    /// to them stay visible, while non-matching sibling subtrees disappear.
    ///
    /// This differs from the native Tasks tab, which keeps the *whole* root
    /// subtree of any match (no inner pruning); here a match shows only its
    /// own path, not its unrelated children. Matching runs on every depth —
    /// each entry against the columns of its own tree level — so a deeply
    /// nested match surfaces its parents, which the old single-depth filter
    /// (only `tree_filter_depth`) could not do.
    ///
    /// The filter is armed only when some level declares a `fuzzy_filter`
    /// action (`tree_filter_depth` is `Some`). Matching runs over the entries
    /// currently in the tree — but on an eager tree (`supports_eager_subtree`)
    /// opening the filter first pulls and expands the *whole* subtree (see
    /// [`Self::tree_filter_expand_stash`]), so collapsed and not-yet-paged
    /// branches are present and their matches surface. On a non-eager (remote)
    /// tree the bound stands: a match hidden in an unloaded branch stays hidden
    /// until that branch is loaded. Pagination placeholders ride along with
    /// their parent: visible iff the parent survives (root-level placeholders
    /// are always visible, since the next page may hold the only matches).
    fn refresh_tree_visible_indices(&mut self, view_defs: &[ViewDef]) {
        let Some(tree) = self.tree.as_ref() else {
            self.tree_visible_indices.clear();
            return;
        };
        let filter = self.table.filter_text.clone();
        if filter.is_empty() || self.tree_filter_depth.is_none() {
            self.tree_visible_indices = (0..tree.entries.len()).collect();
            return;
        }
        let filter_fields = self.fuzzy_filter_fields.clone();
        let matcher = fuzzy_matcher::skim::SkimMatcherV2::default();
        let tokens: Vec<String> = filter.split_whitespace().map(String::from).collect();

        // Columns per depth, resolved lazily down the first tree chain. The
        // haystack also falls back to raw metadata, so branches that differ
        // only in column labels still match on their field values.
        let max_depth = tree.entries.iter().map(|e| e.depth).max().unwrap_or(0);
        let vd = self.view_def(view_defs);
        let columns_by_depth: Vec<Vec<ColumnDef>> = (0..=max_depth)
            .map(|d| {
                vd.and_then(|v| tree_level_at_depth(v, d))
                    .map(|l| l.columns.to_vec())
                    .unwrap_or_default()
            })
            .collect();

        let n = tree.entries.len();
        // parent[i] = nearest shallower ancestor in DFS order; keep[i] =
        // entry itself matches; placeholder[i] = pagination loader row.
        let mut parent: Vec<Option<usize>> = vec![None; n];
        let mut keep = vec![false; n];
        let mut placeholder = vec![false; n];
        let mut stack: Vec<usize> = Vec::new();

        use fuzzy_matcher::FuzzyMatcher;
        for (idx, entry) in tree.entries.iter().enumerate() {
            while let Some(&top) = stack.last() {
                if tree.entries[top].depth >= entry.depth {
                    stack.pop();
                } else {
                    break;
                }
            }
            parent[idx] = stack.last().copied();
            stack.push(idx);

            if entry.is_more_placeholder {
                placeholder[idx] = true;
                continue;
            }
            let cols = columns_by_depth
                .get(entry.depth)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let haystack = build_field_haystack(&entry.node, cols, &filter_fields);
            keep[idx] = tokens
                .iter()
                .all(|t| matcher.fuzzy_match(&haystack, t).is_some());
        }

        // Propagate "has a surviving descendant" up the ancestor chain. In
        // DFS preorder every descendant has a higher index than its ancestor,
        // so one reverse pass accumulates the OR transitively.
        for idx in (0..n).rev() {
            if keep[idx] {
                if let Some(p) = parent[idx] {
                    keep[p] = true;
                }
            }
        }

        let visible: Vec<usize> = (0..n)
            .filter(|&idx| {
                if placeholder[idx] {
                    parent[idx].map(|p| keep[p]).unwrap_or(true)
                } else {
                    keep[idx]
                }
            })
            .collect();
        self.tree_visible_indices = visible;
    }

    /// Undo the eager filter-expand. Opening a fuzzy filter on an eager tree
    /// pulls the whole subtree and marks everything expanded (see
    /// [`Self::tree_filter_expand_stash`]); when the filter is cleared this
    /// restores the stashed pre-filter `expanded` set and rebuilds, so the tree
    /// re-collapses to exactly its previous shape (the deeper cache stays —
    /// harmless, and a later manual expand is now instant). No-op when no
    /// filter-expand is in effect.
    fn restore_tree_filter_expand(&mut self, view_defs: &[ViewDef]) {
        let Some(expanded) = self.tree_filter_expand_stash.take() else {
            return;
        };
        if let Some(tree) = self.tree.as_mut() {
            tree.expanded = expanded;
            if let Some(vd) = view_defs.get(self.view_def_index) {
                tree.rebuild_entries(vd);
            }
        }
        self.rebuild_table(view_defs);
    }

    /// Tree-mode handler for `content.open`. Toggles expand on a row
    /// whose level has a tree-continuing child; on first expand emits
    /// an async [`ViewRequest::ExpandTreeNode`]. Falls through to the
    /// legacy drill (Split/in-place) when the cursor is on a tree-leaf
    /// row with a non-tree-continuing child available. Returns `None`
    /// when nothing actionable is configured for this row.
    fn try_tree_open(
        &mut self,
        view_index: usize,
        pane_id: PaneId,
        view_defs: &[ViewDef],
    ) -> Option<SubViewMessage> {
        let row = self.table.selected_row();
        let (depth, parent_path, node_id, node_label, node_type_chain, is_placeholder) = {
            let entry = self.tree_entry_at_row(row)?;
            (
                entry.depth,
                entry.parent_path.clone(),
                entry.node.id.clone(),
                entry.node.label.clone(),
                entry.node_type_chain.clone(),
                entry.is_more_placeholder,
            )
        };
        let view_def = self.view_def(view_defs)?;
        if is_placeholder {
            // "… weitere laden" pagination row: parent_path is the
            // path of the parent whose children we're paginating.
            // Pagination is only armed in single-load mode (multi-load
            // doesn't paginate), so the first-chain lookup at
            // `parent_depth` is correct here.
            let parent_depth = depth.checked_sub(1)?;
            let tree_child = tree_child_def_at_depth(view_def, parent_depth)?;
            let child_node_type = tree_child.node_type.clone();
            let parent_node_id = parent_path.last()?.clone();
            let next_page = {
                let tree = self.tree.as_ref()?;
                tree.cache.get(&parent_path).and_then(|s| s.next_page)?
            };
            return Some(SubViewMessage::Request(ViewRequest::ExpandTreeNode {
                view_index,
                pane_id,
                parent_path,
                parent_node_id,
                child_node_type,
                page_size: next_page.limit.max(1),
                page: Some(next_page),
                append: true,
            }));
        }
        let mut own_path = parent_path.clone();
        own_path.push(node_id.clone());

        // Look up the entry's producing ChildDef from its node_type
        // chain (chain length == depth + 1, so this disambiguates
        // multi-branch siblings at the same depth). Falls back to a
        // first-chain depth walk only at depth 0 when an unknown root
        // type slips through — in that case the legacy lookup gives
        // the same answer.
        let entry_child: Option<&ChildDef> = child_def_for_type_chain(view_def, &node_type_chain);
        // YAML override: if the entry's ChildDef declares an
        // `enter_action`, route Enter through `Node::invoke_action`
        // instead of the drill / expand branches below. Used for rows
        // whose only "child" is a synthetic anchor for split +
        // pagination config — e.g. `postgres:db_script` → execute path
        // opens the result pane via the same dispatch as `x`.
        if let Some(action_name) = entry_child.and_then(|c| c.enter_action.as_deref()) {
            return Some(SubViewMessage::Request(ViewRequest::InvokeNodeAction {
                view_index,
                pane_id,
                node_id,
                action_name: action_name.to_string(),
            }));
        }
        // Children of the entry's ChildDef — the candidates we may
        // expand into / drill into. Empty when entry is a true leaf.
        // For a `recursive: true` ChildDef, self is an implicit member
        // of its own children (DSF-3), so expand re-loads the same
        // type one level deeper.
        let kids: Vec<&ChildDef> = match entry_child {
            Some(c) => effective_child_children(c),
            None => tree_level_children(view_def, depth)
                .unwrap_or(&[])
                .iter()
                .collect(),
        };
        let tree_children: Vec<&ChildDef> = kids
            .iter()
            .copied()
            .filter(|c| c.tree_label.is_some())
            .collect();

        // Branch 1: there is at least one tree-continuing child at
        // this entry — expand/collapse. With one tree-continuing
        // sibling: single load (legacy path). With N > 1: fan out
        // N loads via ExpandTreeNodeMulti.
        if !tree_children.is_empty() {
            let need_load: bool;
            {
                let tree = self.tree.as_mut()?;
                if tree.expanded.contains(&own_path) {
                    tree.expanded.remove(&own_path);
                    tree.rebuild_entries(view_def);
                    need_load = false;
                } else {
                    tree.expanded.insert(own_path.clone());
                    let cached = tree.cache.get(&own_path).map(|s| s.loaded).unwrap_or(false);
                    if cached {
                        tree.rebuild_entries(view_def);
                        need_load = false;
                    } else {
                        need_load = true;
                    }
                }
            }
            self.rebuild_table(view_defs);
            if need_load {
                if tree_children.len() == 1 {
                    return Some(SubViewMessage::Request(ViewRequest::ExpandTreeNode {
                        view_index,
                        pane_id,
                        parent_path: own_path,
                        parent_node_id: node_id,
                        child_node_type: tree_children[0].node_type.clone(),
                        page_size: 50,
                        page: None,
                        append: false,
                    }));
                }
                let types: Vec<String> =
                    tree_children.iter().map(|c| c.node_type.clone()).collect();
                return Some(SubViewMessage::Request(ViewRequest::ExpandTreeNodeMulti {
                    view_index,
                    pane_id,
                    parent_path: own_path,
                    parent_node_id: node_id,
                    child_node_types: types,
                    page_size: 50,
                }));
            }
            return Some(SubViewMessage::SelectionChanged(None));
        }

        // Branch 2: tree-leaf — drill the first non-tree-continuing
        // child of the entry's ChildDef (e.g. `Rows` with `split:
        // right` from a table row). Returns ContentDrill so
        // `dispatch_content_drill` does its split/in-place magic.
        let child_def = kids
            .iter()
            .copied()
            .find(|c| c.tree_label.is_none())?
            .clone();
        Some(SubViewMessage::ContentDrill {
            item_id: node_id,
            item_label: node_label,
            child_def: Box::new(child_def),
        })
    }

    /// The `list_subtree` depth to request when this pane is an **eager**
    /// tree — i.e. the adapter advertises `supports_eager_subtree` and the
    /// view has a non-zero `expand_depth`. `None` means "not eligible": the
    /// engine falls back to the per-node [`Self::pending_auto_expand_requests`]
    /// cascade (remote adapters, flat views, `expand_depth: 0`).
    ///
    /// The mapping passes the cascade's level target straight through
    /// (`all` → `u32::MAX`, `Levels(n)` → `n`): `list_subtree(depth)` yields
    /// `depth + 1` visible levels — exactly the depths `0..=target` the
    /// cascade would reach — so the two paths render an identical tree.
    pub(crate) fn eager_subtree_depth(&self, view_defs: &[ViewDef]) -> Option<u32> {
        if self.tree.is_none() || !self.capabilities.supports_eager_subtree {
            return None;
        }
        match self.view_def(view_defs)?.expand_depth {
            Some(ExpandDepth::All) => Some(u32::MAX),
            Some(ExpandDepth::Levels(n)) if n > 0 => Some(n),
            _ => None,
        }
    }

    /// One-shot `expand_depth` auto-expansion cascade (root ViewDef
    /// field). Called after tree data lands (root list or expanded
    /// children). While the cascade is armed (fresh tree / new query),
    /// every visible entry at depth `< expand_depth` with a
    /// tree-continuing child branch is expanded: cached subtrees unfold
    /// synchronously, unloaded ones produce the same `ExpandTreeNode` /
    /// `ExpandTreeNodeMulti` requests a manual Enter would — their
    /// children land through the normal `LoadMsg::TreeChildren` path,
    /// which calls back in here until a pass has nothing left to load
    /// and disarms the cascade. After that the user's manual
    /// expand/collapse state is never overridden.
    pub(crate) fn pending_auto_expand_requests(
        &mut self,
        view_index: usize,
        pane_id: PaneId,
        view_defs: &[ViewDef],
    ) -> Vec<ViewRequest> {
        if !self
            .tree
            .as_ref()
            .map(|t| t.auto_expand_pending)
            .unwrap_or(false)
        {
            return Vec::new();
        }
        let Some(view_def) = self.view_def(view_defs).cloned() else {
            return Vec::new();
        };
        // `zr` (TreeExpandAll) arms an unbounded target so the same cascade
        // unfolds every level instead of stopping at the configured
        // `expand_depth`. Otherwise the target is the view's initial depth.
        let expand_all = self
            .tree
            .as_ref()
            .map(|t| t.expand_all_armed)
            .unwrap_or(false);
        let target = if expand_all {
            usize::MAX
        } else {
            match view_def.expand_depth {
                Some(ExpandDepth::All) => usize::MAX,
                Some(ExpandDepth::Levels(n)) => n as usize,
                None => 0,
            }
        };
        if target == 0 {
            if let Some(tree) = self.tree.as_mut() {
                tree.auto_expand_pending = false;
            }
            return Vec::new();
        }
        struct Candidate {
            own_path: Vec<String>,
            node_id: String,
            tree_child_types: Vec<String>,
        }
        let mut requests = Vec::new();
        let mut expanded_any = false;
        loop {
            // Candidates from the *current* flattened entries — re-scanned
            // per pass because expanding a cached node reveals new entries
            // one level deeper.
            let mut candidates: Vec<Candidate> = Vec::new();
            {
                let Some(tree) = self.tree.as_ref() else {
                    return requests;
                };
                for e in &tree.entries {
                    if e.depth >= target
                        || e.is_more_placeholder
                        || e.node.has_children == Some(false)
                    {
                        continue;
                    }
                    let mut own_path = e.parent_path.clone();
                    own_path.push(e.node.id.clone());
                    if tree.expanded.contains(&own_path) {
                        continue;
                    }
                    let entry_child = child_def_for_type_chain(&view_def, &e.node_type_chain);
                    // Rows routing Enter to an adapter action never expand.
                    if entry_child
                        .and_then(|c| c.enter_action.as_deref())
                        .is_some()
                    {
                        continue;
                    }
                    let kids: Vec<&ChildDef> = match entry_child {
                        Some(c) => effective_child_children(c),
                        None => tree_level_children(&view_def, e.depth)
                            .unwrap_or(&[])
                            .iter()
                            .collect(),
                    };
                    let types: Vec<String> = kids
                        .iter()
                        .filter(|c| c.tree_label.is_some())
                        .map(|c| c.node_type.clone())
                        .collect();
                    if types.is_empty() {
                        continue;
                    }
                    candidates.push(Candidate {
                        own_path,
                        node_id: e.node.id.clone(),
                        tree_child_types: types,
                    });
                }
            }
            if candidates.is_empty() {
                break;
            }
            let mut any_cached = false;
            for c in candidates {
                let Some(tree) = self.tree.as_mut() else {
                    return requests;
                };
                tree.expanded.insert(c.own_path.clone());
                expanded_any = true;
                let cached = tree
                    .cache
                    .get(&c.own_path)
                    .map(|s| s.loaded)
                    .unwrap_or(false);
                if cached {
                    any_cached = true;
                    continue;
                }
                if c.tree_child_types.len() == 1 {
                    let child_node_type = c.tree_child_types.into_iter().next().unwrap();
                    requests.push(ViewRequest::ExpandTreeNode {
                        view_index,
                        pane_id,
                        parent_path: c.own_path,
                        parent_node_id: c.node_id,
                        child_node_type,
                        page_size: 50,
                        page: None,
                        append: false,
                    });
                } else {
                    requests.push(ViewRequest::ExpandTreeNodeMulti {
                        view_index,
                        pane_id,
                        parent_path: c.own_path,
                        parent_node_id: c.node_id,
                        child_node_types: c.tree_child_types,
                        page_size: 50,
                    });
                }
            }
            if !any_cached {
                break;
            }
            if let Some(tree) = self.tree.as_mut() {
                tree.rebuild_entries(&view_def);
            }
        }
        if expanded_any {
            if let Some(tree) = self.tree.as_mut() {
                tree.rebuild_entries(&view_def);
            }
            self.rebuild_table(view_defs);
        }
        if requests.is_empty() {
            // This pump emitted no new load — but the cascade may only
            // disarm once nothing is still *in flight*. An expanded path
            // whose children haven't landed yet will fire another pump
            // when they arrive, and that pump can reveal a deeper level to
            // expand. Disarming now (just because *this* arrival, e.g. a
            // leaf on one branch, had no follow-up) strands every level
            // below any sibling branch still loading — the "tree only opens
            // the top two levels" bug. Stay armed until the last in-flight
            // expansion has resolved.
            let in_flight = self
                .tree
                .as_ref()
                .map(|tree| {
                    tree.expanded
                        .iter()
                        .any(|p| !tree.cache.get(p).map(|s| s.loaded).unwrap_or(false))
                })
                .unwrap_or(false);
            if !in_flight {
                if let Some(tree) = self.tree.as_mut() {
                    tree.auto_expand_pending = false;
                    // The expand-all cascade has fully drained; drop the
                    // one-shot override so the next fresh load/query falls
                    // back to the configured `expand_depth`.
                    tree.expand_all_armed = false;
                }
            }
        }
        requests
    }

    /// Re-fetch the children of every *expanded* tree node — the staleness
    /// counterpart to the auto-expand cascade. A root reload (the `r`
    /// reload action, an adapter `Invalidation::All`, an action's
    /// post-mutation reload) replaces only the depth-0 rows; everything
    /// expanded below them still renders from children cached *before* the
    /// change — e.g. a freshly started tracking's `⏱` marker wouldn't show
    /// on a nested task. Called after a root list lands: emits one
    /// `ExpandTreeNode`/`…Multi` request per expanded path, exactly what a
    /// manual collapse+re-expand would issue. The stale rows stay visible
    /// until the fresh children land (`apply_tree_children` replaces them
    /// in place). Paths whose cache isn't `loaded` are skipped — a fetch
    /// (cascade or manual expand) is already in flight for them. Expanded
    /// paths hidden under a *collapsed* ancestor aren't walked (they're
    /// not in `entries`); they keep stale children until re-expanded.
    pub(crate) fn pending_expanded_refresh_requests(
        &self,
        view_index: usize,
        pane_id: PaneId,
        view_defs: &[ViewDef],
    ) -> Vec<ViewRequest> {
        let Some(tree) = self.tree.as_ref() else {
            return Vec::new();
        };
        let Some(view_def) = self.view_def(view_defs) else {
            return Vec::new();
        };
        let mut requests = Vec::new();
        for e in &tree.entries {
            if e.is_more_placeholder {
                continue;
            }
            let mut own_path = e.parent_path.clone();
            own_path.push(e.node.id.clone());
            if !tree.expanded.contains(&own_path) {
                continue;
            }
            if !tree.cache.get(&own_path).map(|s| s.loaded).unwrap_or(false) {
                continue;
            }
            let entry_child = child_def_for_type_chain(view_def, &e.node_type_chain);
            let kids: Vec<&ChildDef> = match entry_child {
                Some(c) => effective_child_children(c),
                None => tree_level_children(view_def, e.depth)
                    .unwrap_or(&[])
                    .iter()
                    .collect(),
            };
            let types: Vec<String> = kids
                .iter()
                .filter(|c| c.tree_label.is_some())
                .map(|c| c.node_type.clone())
                .collect();
            if types.is_empty() {
                continue;
            }
            if types.len() == 1 {
                requests.push(ViewRequest::ExpandTreeNode {
                    view_index,
                    pane_id,
                    parent_path: own_path,
                    parent_node_id: e.node.id.clone(),
                    child_node_type: types.into_iter().next().unwrap(),
                    page_size: 50,
                    page: None,
                    append: false,
                });
            } else {
                requests.push(ViewRequest::ExpandTreeNodeMulti {
                    view_index,
                    pane_id,
                    parent_path: own_path,
                    parent_node_id: e.node.id.clone(),
                    child_node_types: types,
                    page_size: 50,
                });
            }
        }
        requests
    }

    /// Tree-mode handler for `content.tree_collapse` (default `c`).
    /// Smart-collapse: when the selected row's own node is currently
    /// expanded, collapse it (cursor stays on the same row).
    /// Otherwise fall through to [`try_tree_back`] so the parent
    /// collapses and the cursor jumps up to it. Returns `None` for
    /// non-tree panes and at depth 0 on a collapsed node — the
    /// caller treats it as `SubViewMessage::Unhandled`.
    pub(crate) fn try_tree_smart_collapse(
        &mut self,
        view_defs: &[ViewDef],
    ) -> Option<SubViewMessage> {
        let view_def_owned = self.view_def(view_defs)?.clone();
        let row = self.table.selected_row();
        let (own_path, is_expanded) = {
            let entry = self.tree_entry_at_row(row)?;
            let mut own = entry.parent_path.clone();
            own.push(entry.node.id.clone());
            let tree = self.tree.as_ref()?;
            let expanded = tree.expanded.contains(&own);
            (own, expanded)
        };
        if is_expanded {
            {
                let tree = self.tree.as_mut()?;
                tree.expanded.remove(&own_path);
                tree.rebuild_entries(&view_def_owned);
            }
            self.rebuild_table(view_defs);
            return Some(SubViewMessage::SelectionChanged(None));
        }
        self.try_tree_back(view_defs)
    }

    /// Tree-mode handler for `content.tree_collapse_all` (default `zm`).
    /// Collapses the tree back to its configured initial depth
    /// (`expand_depth`) rather than all the way to the root: an expanded
    /// path is kept only while it sits within that depth. With
    /// `expand_depth` unset (or `0`) this is the old "snap back to the
    /// root rows" behaviour. Loaded children stay in `tree.cache`, so
    /// reopening a node reuses the cached children instead of refetching.
    /// Also disarms any pending expand-all so a half-finished `zr` cascade
    /// can't re-expand what we just folded. Cursor moves to the first
    /// visible row. Returns `None` on non-tree panes.
    pub(crate) fn try_tree_collapse_all(
        &mut self,
        view_defs: &[ViewDef],
    ) -> Option<SubViewMessage> {
        let view_def_owned = self.view_def(view_defs)?.clone();
        // An `own_path` of length `n` denotes a node at depth `n - 1`, so a
        // node at depth `d` stays expanded when `d < target`, i.e. when
        // `own_path.len() <= target`. `target == 0` keeps nothing (full
        // collapse), matching the previous behaviour.
        let target = match view_def_owned.expand_depth {
            Some(ExpandDepth::All) => usize::MAX,
            Some(ExpandDepth::Levels(n)) => n as usize,
            None => 0,
        };
        {
            let tree = self.tree.as_mut()?;
            tree.auto_expand_pending = false;
            tree.expand_all_armed = false;
            let before = tree.expanded.len();
            tree.expanded.retain(|p| p.len() <= target);
            if tree.expanded.len() == before {
                return Some(SubViewMessage::SelectionChanged(None));
            }
            tree.rebuild_entries(&view_def_owned);
        }
        self.rebuild_table(view_defs);
        self.table.set_selected(0);
        Some(SubViewMessage::SelectionChanged(None))
    }

    /// Tree-mode handler for `content.tree_expand_all` (default `zr`),
    /// the mirror of [`Self::try_tree_collapse_all`]. Arms the one-shot
    /// auto-expand cascade with an unbounded depth target (see
    /// [`Self::pending_auto_expand_requests`]) so every node unfolds,
    /// lazily loading any unloaded children. Returns a request asking the
    /// App to drive the cascade now (the same entry point a fresh load
    /// uses); `None` on non-tree panes.
    pub(crate) fn try_tree_expand_all(
        &mut self,
        view_index: usize,
        pane_id: PaneId,
        _view_defs: &[ViewDef],
    ) -> Option<SubViewMessage> {
        {
            let tree = self.tree.as_mut()?;
            tree.expand_all_armed = true;
            tree.auto_expand_pending = true;
        }
        Some(SubViewMessage::Request(ViewRequest::DriveTreeAutoExpand {
            view_index,
            pane_id,
        }))
    }

    /// Tree-mode handler for `content.back`. Collapses the cursor's
    /// parent path and moves the cursor up to that parent row. Depth-0
    /// rows (no parent in the tree) return `None` so the caller can
    /// fall through to `SubViewMessage::Unhandled` instead of beeping.
    fn try_tree_back(&mut self, view_defs: &[ViewDef]) -> Option<SubViewMessage> {
        let view_def_owned = self.view_def(view_defs)?.clone();
        let row = self.table.selected_row();
        let parent_path: Vec<String>;
        let parent_parent_path: Vec<String>;
        let parent_node_id: String;
        {
            let entry = self.tree_entry_at_row(row)?;
            if entry.parent_path.is_empty() {
                return None;
            }
            parent_path = entry.parent_path.clone();
            let (last, head) = parent_path.split_last()?;
            parent_node_id = last.clone();
            parent_parent_path = head.to_vec();
        }
        {
            let tree = self.tree.as_mut()?;
            tree.expanded.remove(&parent_path);
            tree.rebuild_entries(&view_def_owned);
        }
        // Position of the parent in the (post-collapse) entry list; the
        // row index we feed `set_selected` must come through the
        // visible-indices map so an active fuzzy filter doesn't desync
        // the cursor.
        let new_eidx = self.tree.as_ref().and_then(|t| {
            t.entries
                .iter()
                .position(|e| e.parent_path == parent_parent_path && e.node.id == parent_node_id)
        })?;
        self.rebuild_table(view_defs);
        let new_row = self
            .tree_visible_indices
            .iter()
            .position(|&i| i == new_eidx)?;
        self.table.set_selected(new_row);
        Some(SubViewMessage::SelectionChanged(None))
    }

    /// Move the row cursor and, when in tree mode, refresh the table
    /// if the active depth changed (header / column set switches with
    /// cursor level). Flat-list panes skip the rebuild.
    fn nav_and_refresh(&mut self, cmd: Cmd, view_defs: &[ViewDef]) {
        let before = self.tree_active_depth();
        self.table.handle_nav(cmd);
        if self.tree.is_some() && self.tree_active_depth() != before {
            self.rebuild_table(view_defs);
        }
    }

    /// Build the table rows for the tree-mode pane. The column set comes
    /// from the cursor row's level (resolved via its `node_type_chain`,
    /// so the header switches branch-correctly as the user moves across
    /// levels). Each entry contributes one row:
    /// - the **designated label column** — the cursor level's
    ///   `tree_label` key, a fixed slot of the active column set — gets
    ///   `<indent><glyph> <label>` for *every* row, regardless of that
    ///   row's own level. This is the structural fix for the recurring
    ///   blank-label bug: the label column is chosen once from the same
    ///   level that supplies the columns, so it is always present in the
    ///   set. It no longer requires `tree_label` keys to align across
    ///   levels (the old, fragile convention).
    /// - other cells hold the entry's `column_value` when the entry's
    ///   **own** level (resolved from its `node_type_chain`) declares a
    ///   column with that key. In a uniform recursive tree every depth
    ///   declares the same columns, so every row shows its data; in a
    ///   multi-branch tree each row fills only the columns its own branch
    ///   declares, leaving columns unique to another branch blank.
    fn build_tree_data_rows(
        &self,
        columns: &[ColumnDef],
        view_defs: &[ViewDef],
        now: chrono::DateTime<chrono::Local>,
        headers_active: bool,
        total_col_idx: Option<usize>,
    ) -> Vec<TRow<u32>> {
        let Some(tree) = self.tree.as_ref() else {
            return Vec::new();
        };
        // Label column + active level resolved once from the cursor row's
        // chain — never per-entry by depth (which can't tell branches
        // apart and blanks rows whose depth maps to a different type).
        let label_col_key = self
            .cursor_tree_level(view_defs)
            .map(|l| l.tree_label.to_string());

        // Tree-label prefix per visible row: the `├──`/`└──`/`│` box
        // connectors + expand glyph, built with the SAME helpers as the
        // native forest renderer (`forest_connector`/`forest_child_prefix`)
        // so the generic tree looks identical. `is_last` is "last among the
        // *visible* siblings sharing a parent_path"; the running
        // `child_prefix_at[depth]` stack carries each ancestor's `│`/blank
        // continuation down to its descendants (valid because the visible
        // entries are in DFS order, so a node's descendants immediately
        // follow it before any sibling). The view's `tree_lines` /
        // `tree_markers` config can swap the line connectors for plain
        // indentation and override or hide the expand markers.
        let connectors: Vec<String> = {
            let opts = self.tree_draw_options(view_defs);
            let visible: Vec<&crate::views::content_tree::TreeEntry> = self
                .tree_visible_indices
                .iter()
                .filter_map(|&e| tree.entries.get(e))
                .collect();
            let mut is_last = vec![false; visible.len()];
            let mut seen: std::collections::HashSet<&[String]> = std::collections::HashSet::new();
            for i in (0..visible.len()).rev() {
                if seen.insert(visible[i].parent_path.as_slice()) {
                    is_last[i] = true;
                }
            }
            let mut out = Vec::with_capacity(visible.len());
            // child_prefix_at[d] = the prefix a depth-`d` row renders with.
            let mut child_prefix_at: Vec<String> = vec![String::new()];
            for (i, e) in visible.iter().enumerate() {
                // With `group_headers:` active the depth-0 rows are the
                // adapter's group buckets, rendered as `── label` header
                // rows (no connector), and every deeper row sheds the
                // indentation level the bucket would otherwise add — the
                // forest starts at indent 0 under each header, like the
                // flat-list grouping.
                if headers_active && e.depth == 0 {
                    out.push(String::new());
                    continue;
                }
                let d = if headers_active {
                    e.depth.saturating_sub(1)
                } else {
                    e.depth
                };
                if child_prefix_at.len() <= d {
                    child_prefix_at.resize(d + 1, String::new());
                }
                let prefix = child_prefix_at[d].clone();
                let expanded = if e.is_more_placeholder || !e.has_children {
                    None
                } else {
                    let mut own = e.parent_path.clone();
                    own.push(e.node.id.clone());
                    Some(tree.expanded.contains(&own))
                };
                // The marker is appended here (not via `ConnectorSpec.
                // expanded`) so the per-view `tree_markers` config can
                // override or hide it; an empty marker leaves no stray
                // trailing space.
                let marker = match expanded {
                    Some(true) => opts.expanded_marker,
                    Some(false) => opts.collapsed_marker,
                    None => "",
                };
                let base = if opts.lines {
                    not_yet_done_forest::forest_connector(not_yet_done_forest::ConnectorSpec {
                        depth: d,
                        is_last: is_last[i],
                        prefix: &prefix,
                        has_description: true,
                        has_children: e.has_children,
                        expanded: None,
                    })
                } else {
                    "  ".repeat(d)
                };
                let connector = if marker.is_empty() {
                    base
                } else {
                    format!("{base}{marker} ")
                };
                let cp = not_yet_done_forest::forest_child_prefix(d, is_last[i], true, &prefix);
                if child_prefix_at.len() <= d + 1 {
                    child_prefix_at.resize(d + 2, String::new());
                }
                child_prefix_at[d + 1] = cp;
                out.push(connector);
            }
            out
        };

        // `group_headers.total`: the bucket's total closes its group — map
        // each group's LAST visible row to the bucket node's total value
        // (read from the total column's `source` metadata field, falling
        // back to its `key`). The classic time-sheet layout, matching the
        // flat grouping's `total_column` semantics.
        let closing_totals: std::collections::HashMap<usize, String> =
            match total_col_idx.and_then(|ci| columns.get(ci)) {
                Some(tc) if headers_active => {
                    let field = tc.source.as_deref().unwrap_or(&tc.key);
                    let mut map = std::collections::HashMap::new();
                    let mut bucket_total: Option<String> = None;
                    let mut last_item_row: Option<usize> = None;
                    let close =
                    |total: &Option<String>,
                     row: Option<usize>,
                     map: &mut std::collections::HashMap<usize, String>| {
                        if let (Some(t), Some(r)) = (total.as_ref(), row) {
                            map.insert(r, t.clone());
                        }
                    };
                    for (row_idx, &eidx) in self.tree_visible_indices.iter().enumerate() {
                        let Some(e) = tree.entries.get(eidx) else {
                            continue;
                        };
                        if e.depth == 0 {
                            close(&bucket_total, last_item_row, &mut map);
                            bucket_total = Some(metadata_field_value(&e.node, field).to_string());
                            last_item_row = None;
                        } else {
                            last_item_row = Some(row_idx);
                        }
                    }
                    close(&bucket_total, last_item_row, &mut map);
                    map
                }
                _ => std::collections::HashMap::new(),
            };

        self.tree_visible_indices
            .iter()
            .enumerate()
            .filter_map(|(row_idx, &eidx)| {
                let entry = tree.entries.get(eidx)?;
                // A group-bucket row renders as a `── label` summary row at
                // the widget stage; emit an empty, non-selectable row here so
                // the engine's width fit ignores it while the 1:1 row ↔
                // visible-entry mapping (cursor → tree entry) stays intact.
                if headers_active && entry.depth == 0 {
                    let mut row = TRow::new(row_idx as u32).not_selectable();
                    for col in columns {
                        row = row.cell(&col.key, CellContent::text(String::new()));
                    }
                    return Some(row);
                }
                // Each entry's data cells are filled per its OWN level's
                // column set (resolved from its node_type_chain), not the
                // cursor's. In a uniform recursive tree every depth declares
                // the same columns, so every row shows its data; in a
                // multi-branch tree each row fills only the columns its own
                // branch declares (columns unique to another branch stay
                // blank).
                let entry_cols: Option<&[ColumnDef]> = self
                    .view_def(view_defs)
                    .and_then(|vd| tree_level_for_chain(vd, &entry.node_type_chain))
                    .map(|l| l.columns);
                let entry_declares =
                    |key: &str| entry_cols.is_some_and(|cols| cols.iter().any(|c| c.key == key));
                // A row is *collapsed* when it has (visible) children but its
                // own path is not in the expanded set — the same predicate the
                // connector marker uses (Some(false)). Drives a column's
                // `collapsed_source`: a collapsed node can surface a metadata
                // field that rolls up hidden-descendant state (e.g. the Tasks
                // tree's `⏱` tracking marker). Leaves and the more-placeholder
                // are never "collapsed".
                let entry_is_collapsed = entry.has_children && !entry.is_more_placeholder && {
                    let mut own = entry.parent_path.clone();
                    own.push(entry.node.id.clone());
                    !tree.expanded.contains(&own)
                };
                let mut row = TRow::new(row_idx as u32);
                for (ci, col) in columns.iter().enumerate() {
                    // The synthetic group-total column: the bucket's total on
                    // the group's closing row, blank everywhere else. Typed
                    // through the column's own `kind` so a duration total
                    // stays right-aligned like the data cells.
                    if total_col_idx == Some(ci) {
                        let raw = closing_totals
                            .get(&row_idx)
                            .map(String::as_str)
                            .unwrap_or("");
                        row = row.cell(&col.key, typed_cell_content(raw, col));
                        continue;
                    }
                    let is_label_cell = label_col_key.as_deref() == Some(col.key.as_str());
                    // The label cell carries the tree glyph + indent and is
                    // never typed; data cells get M2 formatting when the
                    // entry's own level declares the column (see above).
                    let cell: CellContent = if is_label_cell {
                        // The connector already carries the indent + box
                        // glyphs + expand arrow. For a leaf the connector has
                        // no glyph, so append the level's *configured* leaf
                        // glyph if any (e.g. Confluence `📄`); native-style
                        // leaves with none configured render as just the
                        // connector + label.
                        let connector = connectors.get(row_idx).map(String::as_str).unwrap_or("");
                        let leaf = if !entry.is_more_placeholder && !entry.has_children {
                            self.view_def(view_defs)
                                .and_then(|vd| leaf_glyph_opt_for_chain(vd, &entry.node_type_chain))
                                .map(|g| format!("{g} "))
                                .unwrap_or_default()
                        } else {
                            String::new()
                        };
                        // Tag the leading connector run (box glyphs + expand
                        // arrow) with the tree-connector style id so the render
                        // path can paint it apart from the label. The span
                        // rides the cell through `compute_table` (projected /
                        // clamped on truncation like a highlight range) and is
                        // resolved back into a styled segment when the widget
                        // row is built. The leaf glyph + label stay unstyled
                        // (the cell's default fg).
                        // Unread chat items (Stoat): prefix the configured
                        // `unread_marker` glyph (after the connector + leaf, so
                        // the box prefix stays put) when the node carries an
                        // `unread = "true"` metadata field. The marker + label
                        // are painted in the unread color at the widget stage
                        // (see `unread_rows` in `rebuild_table_with`); here we
                        // only weave the glyph into the cell text so column
                        // sizing accounts for its (double-cell) width.
                        let unread = metadata_field_value(&entry.node, "unread") == "true";
                        let marker = if unread {
                            self.unread_marker(view_defs)
                        } else {
                            ""
                        };
                        let marker_prefix = if marker.is_empty() {
                            String::new()
                        } else {
                            format!("{marker} ")
                        };
                        // Type icon of the row's own level (`icon:`), drawn
                        // last before the label — independent of the expand
                        // state, so two branches sharing one depth (Stoat:
                        // uncategorized channels next to categories) stay
                        // distinguishable. Sits after the unread marker so
                        // that marker keeps its leading slot.
                        let icon = self
                            .view_def(view_defs)
                            .and_then(|vd| icon_opt_for_chain(vd, &entry.node_type_chain))
                            .filter(|g| !g.is_empty())
                            .map(|g| format!("{g} "))
                            .unwrap_or_default();
                        let connector_chars = connector.chars().count();
                        let text =
                            format!("{connector}{leaf}{marker_prefix}{icon}{}", entry.node.label);
                        let mut spans: Vec<StyledSpan> = Vec::new();
                        if connector_chars > 0 {
                            spans.push(StyledSpan {
                                range: 0..connector_chars,
                                style_id: TREE_CONNECTOR_STYLE_ID,
                            });
                        }
                        // Fuzzy-match highlight (native Tasks-tree parity): tag
                        // the matched runs in the *label* so the filtered
                        // substring stands out. Ranges are computed against the
                        // bare label and shifted past the connector + leaf glyph
                        // (+ unread marker + type icon) into the full-cell
                        // coordinate; the engine projects / clamps them through
                        // truncation like any span.
                        if !self.table.filter_text.is_empty() {
                            let prefix_chars = connector_chars
                                + leaf.chars().count()
                                + marker_prefix.chars().count()
                                + icon.chars().count();
                            for r in fuzzy_label_ranges(&entry.node.label, &self.table.filter_text)
                            {
                                spans.push(StyledSpan {
                                    range: (r.start + prefix_chars)..(r.end + prefix_chars),
                                    style_id: FUZZY_MATCH_STYLE_ID,
                                });
                            }
                        }
                        if spans.is_empty() {
                            CellContent::text(text)
                        } else {
                            CellContent::text(text).with_spans(spans)
                        }
                    } else if !entry.is_more_placeholder && entry_declares(col.key.as_str()) {
                        if col.source.as_deref() == Some("has_links") {
                            let icon = if self.item_has_link(&entry.node.id) {
                                "🔗"
                            } else {
                                " "
                            };
                            CellContent::text(icon)
                        } else if let Some(field) = col
                            .collapsed_source
                            .as_deref()
                            .filter(|_| entry_is_collapsed)
                        {
                            // Collapsed tree node: read the roll-up field
                            // instead of the node's own value, so a marker
                            // surfaces a hidden-descendant state (e.g. `⏱`).
                            // Typed through the column's own `kind` for layout
                            // parity with the expanded-state cell.
                            let raw = metadata_field_value(&entry.node, field);
                            typed_cell_content(raw, col)
                        } else if let Some(ta) = col
                            .tree_aggregate
                            .as_ref()
                            .filter(|_| self.column_shows_cumulated(col))
                        {
                            // M4: show the adapter's subtree-cumulated value
                            // (read from `cumulated_field`) instead of the
                            // node's own value, formatted with the column's
                            // own `kind` so it lines up identically.
                            let raw = metadata_field_value(&entry.node, &ta.cumulated_field);
                            typed_cell_content(raw, col)
                        } else {
                            cell_content_for(&entry.node, col, now)
                        }
                    } else {
                        CellContent::text(String::new())
                    };
                    row = row.cell(&col.key, cell);
                }
                Some(row)
            })
            .collect()
    }

    /// Actions registered as YAML keybindings at the active level. In
    /// tree mode this resolves to the **cursor-depth** level's
    /// `actions:` list, with globally-scoped action types
    /// (`fuzzy_filter`, `search`, `text_search`) discovered on other
    /// chain levels appended so they remain reachable regardless of
    /// where the cursor sits. Same-key entries at the active level take
    /// precedence over a duplicate further up/down the chain.
    fn current_actions<'a>(&'a self, view_defs: &'a [ViewDef]) -> Vec<&'a ActionDef> {
        if self.tree.is_some() {
            return self.tree_current_actions(view_defs);
        }
        let slice: &[ActionDef] = if let Some(ref child) = self.active_child {
            &child.actions
        } else if let Some(vd) = self.view_def(view_defs) {
            &vd.actions
        } else {
            &[]
        };
        slice.iter().collect()
    }

    /// Tree-mode resolution of `current_actions`. Splits into two
    /// passes: first the cursor-depth level's own actions, then a
    /// chain-wide sweep for action types the user expects to work
    /// regardless of cursor depth.
    fn tree_current_actions<'a>(&'a self, view_defs: &'a [ViewDef]) -> Vec<&'a ActionDef> {
        let mut out: Vec<&'a ActionDef> = Vec::new();
        let Some(vd) = self.view_def(view_defs) else {
            return out;
        };
        let active_depth = self.tree_active_depth();
        // Active level's own actions, resolved via the cursor row's chain
        // so a multi-branch level dispatches its own actions. When the
        // tree is empty there is no cursor row (e.g. the initial load
        // failed, or `manual_connect` hasn't loaded yet) — fall back to
        // the root (depth-0) level so its actions stay reachable. Without
        // this the `reload` action vanishes on an empty tree and the user
        // can't retry a failed load.
        if let Some(level) = self
            .cursor_tree_level(view_defs)
            .or_else(|| tree_level_at_depth(vd, 0))
        {
            out.extend(level.actions.iter());
        }
        // Pull globally-scoped action types from other chain levels.
        // Validator already warns about multi-level definition, so a
        // first-seen wins approach is enough. `tree_find` belongs here
        // with the rest of the search family: the user expects `/` to
        // open the tree search from any cursor depth (the search already
        // spans the whole forest, not the cursor subtree), but tasks.yaml
        // declares it only on the root ViewDef — without this it would
        // dispatch only while the cursor sits on a top-level node.
        const GLOBAL: &[&str] = &["fuzzy_filter", "search", "text_search", "tree_find"];
        for depth in 0..32 {
            if depth == active_depth {
                continue;
            }
            let Some(level) = tree_level_at_depth(vd, depth) else {
                break;
            };
            for action in level.actions {
                if !GLOBAL.contains(&action.action_type.as_str()) {
                    continue;
                }
                // Dedup by key; keyless (event-only) actions are never in
                // this GLOBAL search family, so a None key can't collide.
                if action.key.is_some() && out.iter().any(|a| a.key == action.key) {
                    continue;
                }
                out.push(action);
            }
        }
        out
    }

    fn current_preview_config<'a>(&'a self, view_defs: &'a [ViewDef]) -> Option<&'a PreviewConfig> {
        if self.tree.is_some() {
            // Tree mode: preview lives on the cursor-depth level. Depth
            // 0 has no ChildDef — fall back to the ViewDef's preview.
            if let Some(child) = self.tree_active_child_def(view_defs) {
                return child.preview.as_ref();
            }
            return self.view_def(view_defs).and_then(|vd| vd.preview.as_ref());
        }
        if let Some(ref child) = self.active_child {
            child.preview.as_ref()
        } else {
            self.view_def(view_defs).and_then(|vd| vd.preview.as_ref())
        }
    }

    /// Whether the cursor row can be opened (expanded or drilled). In
    /// tree mode this mirrors `try_tree_open`'s child computation —
    /// crucially via `effective_child_children`, which counts a
    /// `recursive: true` ChildDef as an implicit member of its own
    /// `children:` (DSF-3). `current_children` returns the *raw* declared
    /// `children:` slice, which is empty for a recursive leaf (e.g. the
    /// uniform `task:item`/`task:item` tasks tree) even though the row
    /// still expands one level deeper. Gating the `Open` key claim on
    /// `current_children` alone left Enter unbound on every recursive
    /// node below the root: the expand glyph showed (its predicate is
    /// recursion-aware) but the key did nothing. Outside tree mode this
    /// falls back to `current_children`.
    fn cursor_can_open(&self, view_defs: &[ViewDef]) -> bool {
        if self.tree.is_some() {
            let Some(vd) = self.view_def(view_defs) else {
                return false;
            };
            let Some(entry) = self.tree_entry_at_row(self.table.selected_row()) else {
                return false;
            };
            return match child_def_for_type_chain(vd, &entry.node_type_chain) {
                Some(c) => !effective_child_children(c).is_empty(),
                None => !tree_level_children(vd, entry.depth)
                    .unwrap_or(&[])
                    .is_empty(),
            };
        }
        !self.current_children(view_defs).is_empty()
    }

    fn current_children<'a>(&'a self, view_defs: &'a [ViewDef]) -> &'a [ChildDef] {
        if self.tree.is_some() {
            if let Some(vd) = self.view_def(view_defs) {
                if let Some(entry) = self.tree_entry_at_row(self.table.selected_row()) {
                    if let Some(kids) = tree_level_children_for_chain(vd, &entry.node_type_chain) {
                        return kids;
                    }
                }
                return tree_level_children(vd, self.tree_active_depth()).unwrap_or(&[]);
            }
            return &[];
        }
        if let Some(ref child) = self.active_child {
            &child.children
        } else if let Some(vd) = self.view_def(view_defs) {
            &vd.children
        } else {
            &[]
        }
    }

    /// In tree mode, the `ChildDef` whose level is the cursor's active
    /// depth. Depth 0 has no ChildDef (it is the ViewDef) — so this
    /// returns `None` at root. Outside tree mode it always returns
    /// `None`; callers should fall through to `active_child`.
    fn tree_active_child_def<'a>(&self, view_defs: &'a [ViewDef]) -> Option<&'a ChildDef> {
        let vd = self.view_def(view_defs)?;
        // Chain-based: the cursor row's own ChildDef (its preview config),
        // not the first branch's at that depth. Empty chain (root) → None,
        // matching the old depth-0 behaviour (caller falls back to ViewDef).
        child_def_for_type_chain(vd, &self.cursor_node_type_chain())
    }

    pub fn nav_depth(&self) -> usize {
        self.nav_stack.len()
    }

    pub fn breadcrumbs(&self) -> Vec<&str> {
        let mut crumbs: Vec<&str> = self.nav_stack.iter().map(|f| f.label.as_str()).collect();
        if let Some(ref child) = self.active_child {
            crumbs.push(&child.name);
        }
        crumbs
    }

    pub fn parent_node_id(&self) -> Option<&str> {
        self.nav_stack.last().map(|f| f.parent_node_id.as_str())
    }

    pub fn current_child_node_type(&self) -> Option<&str> {
        self.active_child.as_ref().map(|c| c.node_type.as_str())
    }

    /// View-hierarchy node_types from the root ViewDef down to the
    /// currently drilled-into ChildDef. Used to scope on-disk artefacts
    /// (e.g. the `:script` menu directory) to the *pane path* rather
    /// than the item-type currently selected — so the menu stays stable
    /// when a pane mixes node types (e.g. Taiga items mixing issues /
    /// userstories / tasks under a single `taiga:item` view).
    /// Node-type chain for the row currently under the cursor (or for
    /// the current level when in flat mode without a selection). Tree
    /// mode returns the selected entry's `node_type_chain` directly;
    /// flat mode falls back to [`Self::view_path_node_types`] which
    /// covers root → active_child. Used by the per-node shortcut
    /// resolver (`app::node_actions::resolve_shortcut`).
    pub fn selected_node_type_chain(&self, view_defs: &[ViewDef]) -> Vec<String> {
        if self.tree.is_some() {
            let row = self.table.selected_row();
            if let Some(entry) = self.tree_entry_at_row(row) {
                return entry.node_type_chain.clone();
            }
            return Vec::new();
        }
        self.view_path_node_types(view_defs)
    }

    pub fn view_path_node_types(&self, view_defs: &[ViewDef]) -> Vec<String> {
        let mut path: Vec<String> = Vec::new();
        if let Some(vd) = self.view_def(view_defs) {
            path.push(vd.node_type.clone());
        }
        // Frame 0 captures the pre-first-drill state (active_child = None);
        // every later frame holds the active_child that *was* current
        // before drilling deeper, i.e. the prior level's identity.
        for frame in self.nav_stack.iter().skip(1) {
            if let Some(ac) = &frame.active_child {
                path.push(ac.node_type.clone());
            }
        }
        if let Some(ac) = &self.active_child {
            path.push(ac.node_type.clone());
        }
        path
    }

    /// Child *names* from the root down to the current drilldown level (the
    /// root itself contributes nothing). Mirrors
    /// [`Self::view_path_node_types`] but keys on `ChildDef` names, matching
    /// the identity that `KeySource::NodeShortcut` records — so the shortcut
    /// menu can select the node shortcuts (`shortcuts:`) that apply here.
    pub fn current_child_name_path(&self) -> Vec<String> {
        let mut path: Vec<String> = Vec::new();
        for frame in self.nav_stack.iter().skip(1) {
            if let Some(ac) = &frame.active_child {
                path.push(ac.name.clone());
            }
        }
        if let Some(ac) = &self.active_child {
            path.push(ac.name.clone());
        }
        path
    }

    /// Node-type path used to derive this view's *script* scope (script
    /// directory + DB shortcut scope). Identical to
    /// [`Self::view_path_node_types`] except that when the active root
    /// ViewDef declares `script_source: <name>`, the root segment is
    /// replaced by the referenced sibling view's `node_type` — so two
    /// views can share one script source (see `ViewDef::script_source`).
    /// An unknown / self-referential name is a silent no-op. Drilled child
    /// levels always keep their own node_types. Do NOT use this for
    /// node-identity purposes — only for script scoping.
    pub fn script_scope_path(&self, view_defs: &[ViewDef]) -> Vec<String> {
        let mut path = self.view_path_node_types(view_defs);
        if let Some(vd) = self.view_def(view_defs) {
            if let Some(name) = vd.script_source.as_deref() {
                if let Some(src) = view_defs.iter().find(|v| v.name == name) {
                    if let Some(root) = path.first_mut() {
                        *root = src.node_type.clone();
                    }
                }
            }
        }
        path
    }

    pub fn selected_item_id(&self) -> Option<&str> {
        if self.tree.is_some() {
            let row = self.table.selected_row();
            let entry = self.tree_entry_at_row(row)?;
            if entry.is_more_placeholder {
                return None;
            }
            return Some(entry.node.id.as_str());
        }
        let row = self.table.selected_row();
        let item_idx = self.filtered_indices.get(row).copied().unwrap_or(row);
        self.items.get(item_idx).map(|item| item.id.as_str())
    }

    /// Ids of every currently-visible (filtered) row, in display order.
    /// Drives `scope: filtered_set` batch scripts. When a fuzzy filter is
    /// active the set follows `filtered_indices`; otherwise it's the whole
    /// loaded list. Flat-list oriented — batch scope is only configured on
    /// flat views, so the depth-0 `items` set is exactly the right one.
    pub fn filtered_item_ids(&self) -> Vec<String> {
        if self.table.fuzzy_active {
            // Fuzzy filter narrows the visible set — follow it exactly (an
            // empty match set yields an empty id list, as the user sees).
            self.filtered_indices
                .iter()
                .filter_map(|&i| self.items.get(i))
                .map(|item| item.id.clone())
                .collect()
        } else {
            // No local filter: the whole loaded list is the query-filtered
            // (e.g. date-bounded) set.
            self.items.iter().map(|item| item.id.clone()).collect()
        }
    }

    /// Every currently-visible row's full [`NodeSummary`], in display order.
    /// Drives `scope: table` scripts (the whole displayed table, with fields).
    /// Tree-aware: in tree mode it walks the visible entries (skipping `…more`
    /// placeholders); flat mode follows the fuzzy filter exactly like
    /// [`Self::filtered_item_ids`], else returns the whole loaded list.
    pub fn visible_items(&self) -> Vec<NodeSummary> {
        if self.tree.is_some() {
            let mut out = Vec::new();
            for row in 0..self.table.row_count() {
                if let Some(entry) = self.tree_entry_at_row(row) {
                    if !entry.is_more_placeholder {
                        out.push(entry.node.clone());
                    }
                }
            }
            return out;
        }
        if self.table.fuzzy_active {
            self.filtered_indices
                .iter()
                .filter_map(|&i| self.items.get(i))
                .cloned()
                .collect()
        } else {
            self.items.clone()
        }
    }

    /// The selected row's index in the *displayed* order (the value a
    /// `scope: table` script reports as `selected_index`). This is the table
    /// cursor row, matching the order of [`Self::visible_items`].
    pub fn selected_row_index(&self) -> usize {
        self.table.selected_row()
    }

    /// The field key under the column cursor, or `None` when the column cursor
    /// is off. Maps the visible column index through the last-rendered column
    /// keys, so it tracks per-level column config. Used for `scope: table`'s
    /// `selected_field` (the caller substitutes its `default_field` on `None`).
    pub fn selected_field_key(&self) -> Option<String> {
        let col = self.table.selected_column()?;
        self.last_column_keys.get(col).cloned()
    }

    /// CF-11: companion of [`Self::selected_item_id`] returning the row's
    /// human-readable label (the `NodeSummary.label` set by the adapter).
    /// Used by generic confirm popups where the id alone (e.g. a numeric
    /// Confluence page id) isn't recognisable to the user.
    pub fn selected_item_label(&self) -> Option<&str> {
        if self.tree.is_some() {
            let row = self.table.selected_row();
            let entry = self.tree_entry_at_row(row)?;
            if entry.is_more_placeholder {
                return None;
            }
            return Some(entry.node.label.as_str());
        }
        let row = self.table.selected_row();
        let item_idx = self.filtered_indices.get(row).copied().unwrap_or(row);
        self.items.get(item_idx).map(|item| item.label.as_str())
    }

    /// The selected row's full [`NodeSummary`] — the tree-aware way to
    /// reach its label + metadata. In tree mode the summary lives on
    /// the tree entry (`self.items` only holds the depth-0 rows, so an
    /// `items` lookup by id silently misses every nested node); in
    /// flat mode it indexes `items` through the fuzzy filter.
    pub fn selected_item(&self) -> Option<&NodeSummary> {
        if self.tree.is_some() {
            let row = self.table.selected_row();
            let entry = self.tree_entry_at_row(row)?;
            if entry.is_more_placeholder {
                return None;
            }
            return Some(&entry.node);
        }
        let row = self.table.selected_row();
        let item_idx = self.filtered_indices.get(row).copied().unwrap_or(row);
        self.items.get(item_idx)
    }

    /// Position the table cursor on the row whose node id matches
    /// `node_id`. Works in flat and tree mode and honours an active
    /// fuzzy filter via `filtered_indices` / `tree_visible_indices`.
    /// Returns `true` if a matching visible row was found.
    pub fn focus_item_by_id(&mut self, node_id: &str) -> bool {
        if self.tree.is_some() {
            for row in 0..self.table.row_count() {
                if let Some(entry) = self.tree_entry_at_row(row) {
                    if !entry.is_more_placeholder && entry.node.id == node_id {
                        self.table.set_selected(row);
                        return true;
                    }
                }
            }
            return false;
        }
        if self.filtered_indices.is_empty() {
            if let Some(idx) = self.items.iter().position(|i| i.id == node_id) {
                self.table.set_selected(idx);
                return true;
            }
            return false;
        }
        for (row, item_idx) in self.filtered_indices.iter().enumerate() {
            if self.items.get(*item_idx).map(|i| i.id.as_str()) == Some(node_id) {
                self.table.set_selected(row);
                return true;
            }
        }
        false
    }

    /// Resolve the `node_id` an adapter-routed action should target.
    fn resolve_action_node_id(&self, action: &ActionDef) -> Option<String> {
        let row = self.table.selected_row();
        if self.tree.is_some() {
            let entry = self.tree_entry_at_row(row)?;
            if entry.is_more_placeholder {
                return None;
            }
            if let Some(key) = action.node_id_from.as_deref() {
                let value = entry
                    .node
                    .metadata
                    .fields
                    .iter()
                    .find(|f| f.key == key)
                    .map(|f| f.value.as_str())
                    .unwrap_or("");
                if value.is_empty() {
                    return None;
                }
                return Some(value.to_string());
            }
            return Some(entry.node.id.clone());
        }
        let item_idx = self.filtered_indices.get(row).copied().unwrap_or(row);
        let item = self.items.get(item_idx)?;
        if let Some(key) = action.node_id_from.as_deref() {
            let value = item
                .metadata
                .fields
                .iter()
                .find(|f| f.key == key)
                .map(|f| f.value.as_str())
                .unwrap_or("");
            if value.is_empty() {
                return None;
            }
            return Some(value.to_string());
        }
        Some(item.id.clone())
    }

    // ── Navigation (drill-down / back) ──────────────────────────────

    /// Prepare drill-down: snapshot current level, set child config.
    /// Returns the child_node_type so the caller can spawn the load.
    fn drill_down_prepare(
        &mut self,
        item_id: &str,
        item_label: &str,
        child_def: &ChildDef,
        view_defs: &[ViewDef],
    ) -> String {
        // Tree mode terminates at any child without `tree_label`. Drop the
        // tree state into the frame so the leaf level renders flat, then
        // restore it on nav_back. For flat-mode panes this is a no-op
        // (`self.tree` is already None).
        let stashed_tree = if child_def.tree_label.is_none() {
            self.tree.take()
        } else {
            None
        };
        let frame = NavFrame {
            label: item_label.to_string(),
            parent_node_id: item_id.to_string(),
            items: std::mem::take(&mut self.items),
            selected_row: self.table.selected_row(),
            selected_column: self.table.selected_column(),
            active_child: self.active_child.take(),
            preview_open: self.preview_open,
            preview_key: std::mem::take(&mut self.preview_key),
            preview_description: std::mem::take(&mut self.preview_description),
            preview_scroll: self.preview_scroll,
            preview_markdown: self.preview_markdown,
            tree: stashed_tree,
        };
        self.nav_stack.push(frame);

        let node_type_id = child_def.node_type.clone();
        self.active_child = Some(child_def.clone());
        // Arm the level's initial cursor placement; the load that follows
        // applies it (see `pending_cursor_on_open`).
        self.pending_cursor_on_open = child_def.cursor_on_open;

        self.preview_open = false;
        self.preview_scroll = 0;
        self.preview_loading = false;
        self.items.clear();
        self.table.set_selected(0);
        // Initialize column cursor based on the new level's opt-in.
        self.table.set_selected_column(if child_def.column_cursor {
            Some(0)
        } else {
            None
        });
        self.rebuild_table(view_defs);

        node_type_id
    }

    /// Hot-replace path for coupled split-panes: drop any nav stack, paste
    /// in the source pane's current items/active_child/selected_row so
    /// `nav_back` from inside the child returns to the same parent-level
    /// snapshot the source is showing now, then drill into the new
    /// `child_def` target. The result is identical to spawning a fresh
    /// coupled child, except the [`PaneId`] is preserved.
    fn coupled_replace_with_source(
        &mut self,
        source_items: Vec<NodeSummary>,
        source_selected_row: usize,
        source_active_child: Option<ChildDef>,
        item_id: &str,
        item_label: &str,
        child_def: &ChildDef,
        view_defs: &[ViewDef],
    ) -> String {
        self.nav_stack.clear();
        self.items = source_items;
        self.active_child = source_active_child;
        self.table.set_selected(source_selected_row);
        self.preview_open = false;
        self.preview_description.clear();
        self.preview_key.clear();
        self.preview_scroll = 0;
        self.preview_loading = false;
        self.drill_down_prepare(item_id, item_label, child_def, view_defs)
    }

    fn nav_back(&mut self, view_defs: &[ViewDef]) -> bool {
        let Some(frame) = self.nav_stack.pop() else {
            return false;
        };

        self.items = frame.items;
        self.active_child = frame.active_child;
        self.preview_open = frame.preview_open;
        self.preview_key = frame.preview_key;
        self.preview_description = frame.preview_description;
        self.preview_scroll = frame.preview_scroll;
        self.preview_markdown = frame.preview_markdown;
        self.preview_loading = false;
        // Restore the tree state stashed when we drilled into a non-tree
        // child. `rebuild_table` below re-runs through the tree path
        // and refreshes `tree_visible_indices` against the restored entries.
        if frame.tree.is_some() {
            self.tree = frame.tree;
        }

        self.rebuild_table(view_defs);
        self.table.set_selected(frame.selected_row);
        self.table.set_selected_column(frame.selected_column);

        true
    }

    // ── Preview ──────────────────────────────────────────────────────

    pub fn set_preview_description(&mut self, key: &str, description: String) {
        if self.preview_key == key {
            self.preview_description = description;
            self.preview_loading = false;
            self.preview_scroll = 0;
        }
    }

    /// If a new preview should be fetched for the current selection,
    /// return the parameters and update the cache; otherwise None.
    /// `cache_key` is the row's own id (matches `preview_key` for the
    /// reply); `node_id` is what the adapter looks up — they differ
    /// when `preview.node_id_from` redirects to a linked node.
    /// `action_id` carries `preview.action` so the App can route
    /// through `Node::prepare` instead of `content().read_text()`.
    pub fn update_preview_for_selection(
        &mut self,
        view_defs: &[ViewDef],
    ) -> Option<PreviewFetchParams> {
        if !self.preview_open {
            return None;
        }
        let row_id = self.selected_item_id().map(|s| s.to_string())?;
        if row_id == self.preview_key {
            return None;
        }

        let cfg = self.current_preview_config(view_defs);
        let action_id = cfg.and_then(|c| c.action.clone());
        let node_id_from = cfg.and_then(|c| c.node_id_from.clone());
        self.preview_markdown = cfg.map(|c| c.markdown).unwrap_or(false);

        let node_id = if let Some(key) = node_id_from {
            let row = self.table.selected_row();
            let item_idx = self.filtered_indices.get(row).copied().unwrap_or(row);
            let value = self
                .items
                .get(item_idx)
                .and_then(|item| {
                    item.metadata
                        .fields
                        .iter()
                        .find(|f| f.key == key)
                        .map(|f| f.value.clone())
                })
                .filter(|v| !v.is_empty());
            match value {
                Some(v) => v,
                None => return None,
            }
        } else {
            row_id.clone()
        };

        self.preview_key = row_id.clone();
        self.preview_description.clear();
        self.preview_scroll = 0;
        self.preview_loading = true;
        Some(PreviewFetchParams {
            cache_key: row_id,
            node_id,
            action_id,
        })
    }

    fn render_breadcrumbs(&self, frame: &mut Frame, area: Rect, view_defs: &[ViewDef]) {
        let t = &*self.theme;
        let mut spans = Vec::new();

        let root_label = self
            .view_def(view_defs)
            .map(|v| v.name.as_str())
            .unwrap_or("root");
        spans.push(Span::styled(root_label, Style::default().fg(t.accent())));

        for f in &self.nav_stack {
            spans.push(Span::styled(" › ", Style::default().fg(t.text_dim())));
            spans.push(Span::styled(&f.label, Style::default().fg(t.text_med())));
        }

        if let Some(ref child) = self.active_child {
            spans.push(Span::styled(" › ", Style::default().fg(t.text_dim())));
            spans.push(Span::styled(
                &child.name,
                Style::default().fg(t.text_high()),
            ));
        }

        let line = Line::from(spans);
        let paragraph = Paragraph::new(line).style(Style::default().bg(t.surface()));
        frame.render_widget(paragraph, area);
    }

    fn render_preview(&mut self, frame: &mut Frame, area: Rect) {
        // Track inner height (without the two border rows) so ctrl+u/d
        // can scroll by the actual visible half-page.
        self.preview_visible_height = area.height.saturating_sub(2);

        let t = &*self.theme;
        let title = if self.preview_loading {
            format!(" {} (loading…) ", self.preview_key)
        } else {
            format!(" {} ", self.preview_key)
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(t.text_dim()))
            .title(Span::styled(title, Style::default().fg(t.accent())));

        if self.preview_markdown {
            // Markdown preview: the body is already soft-wrapped to the inner
            // width by the renderer, so no Paragraph wrap (which would re-wrap
            // and double-count). Reuses the Phase-1 render module.
            let inner_w = (area.width.saturating_sub(2) as usize).max(1);
            let lines = render_markdown_lines(&self.preview_description, inner_w, t);
            let paragraph = Paragraph::new(lines)
                .block(block)
                .scroll((self.preview_scroll, 0));
            frame.render_widget(paragraph, area);
            return;
        }

        let text: Vec<Line> = self
            .preview_description
            .lines()
            .map(|l| {
                Line::from(Span::styled(
                    l.to_string(),
                    Style::default().fg(t.text_med()),
                ))
            })
            .collect();

        let paragraph = Paragraph::new(text)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((self.preview_scroll, 0));

        frame.render_widget(paragraph, area);
    }

    /// Render the pane's table and optional preview within `area`.
    /// Returns the `Rect` actually occupied by the table — callers can
    /// use that to position the sort-header overlay on the focused pane.
    fn render_table_and_preview(&mut self, frame: &mut Frame, area: Rect) -> Rect {
        if self.preview_open {
            let chunks =
                Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .split(area);
            self.table.view(frame, chunks[0]);
            self.render_preview(frame, chunks[1]);
            chunks[0]
        } else {
            self.table.view(frame, area);
            area
        }
    }

    fn render_page_footer(&self, frame: &mut Frame, area: Rect) {
        let Some(info) = self.last_page_info else {
            return;
        };
        let text = format_page_footer(info, &self.last_applied_sort);
        let style = Style::default()
            .fg(self.theme.text_dim())
            .bg(self.theme.bg());
        let paragraph = Paragraph::new(Line::from(Span::styled(text, style)));
        frame.render_widget(paragraph, area);
    }

    // ── Loading ──────────────────────────────────────────────────────

    pub fn root_load_request(&self, view_defs: &[ViewDef]) -> Option<LoadRequest> {
        let view_def = self.view_def(view_defs)?;
        // The view's configured default is a literal body in the adapter's own
        // language; only an active query can be an extended document, so the
        // kind travels with `active_query` and not with the fallback.
        let (query, kind) = match self.active_query.clone() {
            Some(q) => (Some(q), self.active_query_kind),
            None => (
                view_def.query.as_ref().and_then(|q| q.default.clone()),
                QueryKind::Saved,
            ),
        };
        let page = self.current_page.or_else(|| {
            view_def.pagination.as_ref().and_then(|p| match p.mode {
                PaginationMode::Server | PaginationMode::Cursor => Some(PageRequest {
                    offset: 0,
                    limit: p.page_size.unwrap_or(0),
                }),
                PaginationMode::All => None,
            })
        });
        Some(LoadRequest {
            node_type_id: view_def.node_type.clone(),
            query,
            kind,
            sort: self.current_sort.clone(),
            page,
            vars: self.active_query_vars.clone(),
        })
    }

    pub fn current_sort(&self) -> &[SortKey] {
        &self.current_sort
    }

    pub fn set_current_sort(&mut self, sort: Vec<SortKey>) -> bool {
        if self.current_sort == sort {
            return false;
        }
        self.current_sort = sort;
        // Changing sort resets the page offset (a different ordering
        // makes the prior offset meaningless).
        self.current_page = self.current_page.map(|p| PageRequest {
            offset: 0,
            limit: p.limit,
        });
        true
    }

    pub fn last_applied_sort(&self) -> &[SortKey] {
        &self.last_applied_sort
    }

    pub fn set_current_page(&mut self, page: Option<PageRequest>) -> bool {
        if self.current_page == page {
            return false;
        }
        self.current_page = page;
        true
    }

    pub fn current_page(&self) -> Option<PageRequest> {
        self.current_page
    }

    /// Effective pagination mode for the current drill level. Used by
    /// the custom-query lifecycle (CP-5) to decide whether `>` / `<`
    /// re-issue a LIMIT/OFFSET query or step a server-side cursor.
    /// Falls back to [`PaginationMode::Server`] when no `pagination`
    /// block is configured — matches the legacy default.
    pub fn resolve_pagination_mode(&self, view_defs: &[ViewDef]) -> PaginationMode {
        if let Some(child) = self.active_child.as_ref() {
            if let Some(p) = child.pagination.as_ref() {
                return p.mode;
            }
        }
        if let Some(vd) = self.view_def(view_defs) {
            if let Some(p) = vd.pagination.as_ref() {
                return p.mode;
            }
        }
        PaginationMode::Server
    }

    /// What page to ask the adapter for when (re)loading a drill-down
    /// level. Falls back to the active child's pagination config and
    /// finally to `None`, letting the caller pick a default.
    pub fn drill_load_page(&self) -> Option<PageRequest> {
        if let Some(p) = self.current_page {
            return Some(p);
        }
        let pagination = self
            .active_child
            .as_ref()
            .and_then(|c| c.pagination.as_ref())?;
        match pagination.mode {
            PaginationMode::Server | PaginationMode::Cursor => Some(PageRequest {
                offset: 0,
                limit: pagination.page_size.unwrap_or(0),
            }),
            PaginationMode::All => None,
        }
    }

    pub fn last_page_info(&self) -> Option<PageInfo> {
        self.last_page_info
    }

    pub fn next_page_request(&self) -> Option<PageRequest> {
        let info = self.last_page_info?;
        if !info.has_next || info.limit == 0 {
            return None;
        }
        let next_offset = (info.offset as u64).saturating_add(info.limit as u64);
        Some(PageRequest {
            offset: u32::try_from(next_offset).unwrap_or(u32::MAX),
            limit: info.limit,
        })
    }

    pub fn prev_page_request(&self) -> Option<PageRequest> {
        let info = self.last_page_info?;
        if !info.has_prev || info.limit == 0 {
            return None;
        }
        let prev_offset = info.offset.saturating_sub(info.limit);
        Some(PageRequest {
            offset: prev_offset,
            limit: info.limit,
        })
    }

    /// Effective binding for `action` at the current drill level.
    /// Returns `None` when the level explicitly disables the binding
    /// (`keybindings: { back: null }`) or when no global default
    /// exists. Falls back to the global content keybindings when the
    /// active child has no override entry for `action`.
    fn level_binding(
        &self,
        action: &ContentAction,
        content_kb: &KeyBindingSection<ContentAction>,
        view_defs: &[ViewDef],
    ) -> Option<KeyBinding> {
        // Tree mode: per-level keybinding overrides live on the
        // cursor-depth ChildDef. Depth 0 is the ViewDef (no overrides);
        // fall through to the global content keybindings.
        if self.tree.is_some() {
            if let Some(child) = self.tree_active_child_def(view_defs) {
                if let Some(over) = child.keybindings.get(action) {
                    return over.clone();
                }
            }
            return content_kb.get(action).cloned();
        }
        if let Some(child) = self.active_child.as_ref() {
            if let Some(over) = child.keybindings.get(action) {
                return over.clone();
            }
        }
        content_kb.get(action).cloned()
    }

    /// Build the request that re-fetches items at the **current**
    /// drill level. At root → `SpawnContentLoad` (uses ViewDef). Inside
    /// a drill → `DrillDown` (uses `active_child` + parent id) so the
    /// reload doesn't fall back to the root view's `node_type`.
    fn reload_current_level(&self, view_index: usize, pane_id: PaneId) -> SubViewMessage {
        match (self.parent_node_id(), self.current_child_node_type()) {
            (Some(parent_id), Some(child_type)) => {
                SubViewMessage::Request(ViewRequest::DrillDown {
                    view_index,
                    pane_id,
                    node_id: parent_id.to_string(),
                    node_label: String::new(),
                    child_node_type: child_type.to_string(),
                })
            }
            _ => SubViewMessage::Request(ViewRequest::SpawnContentLoad {
                view_index,
                pane_id,
            }),
        }
    }

    pub fn current_query_text(&self, view_defs: &[ViewDef]) -> String {
        if let Some(ref q) = self.active_query {
            return q.clone();
        }
        self.default_query_text(view_defs)
    }

    pub fn default_query_text(&self, view_defs: &[ViewDef]) -> String {
        self.view_def(view_defs)
            .and_then(|vd| vd.query.as_ref())
            .and_then(|q| q.template.clone().or_else(|| q.default.clone()))
            .unwrap_or_default()
    }

    pub fn is_query_editable(&self, view_defs: &[ViewDef]) -> bool {
        self.view_def(view_defs)
            .and_then(|vd| vd.query.as_ref())
            .map(|q| q.editable)
            .unwrap_or(false)
    }

    pub fn set_query(&mut self, query: String, name: Option<String>) {
        self.set_query_of_kind(
            query,
            name,
            std::collections::HashMap::new(),
            QueryKind::Saved,
        );
    }

    /// Variant of [`set_query`] that also stores variable bindings to
    /// substitute into the raw query via `ContentAdapter::render_query`
    /// at load time.
    pub fn set_query_with_vars(
        &mut self,
        query: String,
        name: Option<String>,
        vars: std::collections::HashMap<String, String>,
    ) {
        self.set_query_of_kind(query, name, vars, QueryKind::Saved);
    }

    /// The one place the active query is replaced. The two variants above are
    /// the adapter-native case, which is every caller that types or edits a
    /// query body; `Extended` comes only from applying a document loaded out
    /// of the extended store.
    pub fn set_query_of_kind(
        &mut self,
        query: String,
        name: Option<String>,
        vars: std::collections::HashMap<String, String>,
        kind: QueryKind,
    ) {
        self.active_query = Some(query);
        self.active_query_name = name;
        self.active_query_vars = vars;
        self.active_query_kind = kind;
        // A new query invalidates any expanded subtree (loaded under the
        // old query): re-derive the tree from the upcoming root reload.
        if let Some(tree) = self.tree.as_mut() {
            tree.clear_for_new_query();
        }
    }

    pub fn active_query_vars(&self) -> &std::collections::HashMap<String, String> {
        &self.active_query_vars
    }

    /// Apply loaded items.
    pub fn set_items(
        &mut self,
        items: Vec<NodeSummary>,
        applied_sort: Vec<SortKey>,
        page: Option<PageInfo>,
        columns: Vec<not_yet_done_content::ColumnSchema>,
        error: Option<String>,
        view_defs: &[ViewDef],
    ) {
        let was_loaded = self.loaded;
        // Reload of a flat pane: remember which node the cursor sits on so it
        // can be re-selected below. The table alone only restores the previous
        // row *index*, which points at a different node as soon as the page
        // shifts — a feed that renders its newest page (chat messages) moves
        // every row up as soon as one message arrives. Captured before
        // `self.items` is replaced. Tree panes use `eager_reload_reanchor_id`.
        let flat_reanchor_id = if was_loaded && self.tree.is_none() {
            self.selected_item_id().map(str::to_string)
        } else {
            None
        };
        // Reload (not first load): preserve the eager tree's fold state and
        // remember the cursor's node so `apply_subtree` can re-anchor it.
        // Capture the selected node id *before* `self.items`/entries are
        // rebuilt below — `tree_entry_at_row` still maps against the
        // pre-reload tree here. On the first load there is nothing to
        // preserve or re-anchor.
        if was_loaded {
            self.eager_reload_preserve_expansion = true;
            self.eager_reload_reanchor_id = self
                .tree_entry_at_row(self.table.selected_row())
                .filter(|e| !e.is_more_placeholder)
                .map(|e| e.node.id.clone());
        } else {
            self.eager_reload_preserve_expansion = false;
            self.eager_reload_reanchor_id = None;
        }
        self.fetch_error = error;
        // Final result lands — clear the in-flight retry banner. On
        // success the pane shows fresh data; on exhausted retries the
        // `fetch_error` we just set becomes the sticky banner instead.
        self.retry_state = None;
        self.items = items;
        self.last_applied_sort = applied_sort;
        self.last_page_info = page;
        self.last_columns = columns;
        self.loaded = true;
        // Tree mode: the freshly-loaded list is the depth-0 children.
        // Feed them into the cache + re-flatten so the renderer has
        // entries to walk. (Drill-down panes use the same view_def, so
        // tree mode is only active at the root — `active_child` is
        // unused while `tree.is_some()`.)
        if let Some(tree) = self.tree.as_mut() {
            // Root-level pagination via the tree placeholder is not yet
            // wired (Phase 5 covers child-level pagination only). The
            // flat-list NextPage/PrevPage keys still page root.
            tree.set_cached_children(Vec::new(), self.items.clone(), None);
            if let Some(vd) = view_defs.get(self.view_def_index) {
                tree.rebuild_entries(vd);
            }
        }
        // Regular list() results clear any custom-query state — page
        // navigation now goes through the normal reload path again.
        // Callers that want the pane to stay in custom-query mode
        // (i.e. ContentView::apply_custom_query_result) restore the
        // state after this call.
        self.active_custom_query = None;
        // First-time load at root level: apply the top-level ViewDef's
        // `column_cursor` opt-in. Drill-down levels are initialized from
        // `drill_down_prepare`. Subsequent reloads (page change, refresh)
        // leave the cursor alone — the user's column position persists.
        if !was_loaded
            && self.nav_stack.is_empty()
            && self.active_child.is_none()
            && self.table.selected_column().is_none()
        {
            if let Some(vd) = self.view_def(view_defs) {
                if vd.column_cursor {
                    self.table.set_selected_column(Some(0));
                }
            }
        }
        self.rebuild_table(view_defs);
        // After the rebuild: `filtered_indices` now describes the display
        // order both the re-anchor and the placement index into. A node that
        // is gone (deleted, paged out) leaves the restored index alone.
        if let Some(id) = flat_reanchor_id {
            self.focus_item_by_id(&id);
        }
        // An armed placement wins over the re-anchor: it is only armed while
        // the level is being opened, and an open has no cursor to preserve.
        self.apply_cursor_on_open();
    }

    /// Apply — and clear — a `cursor_on_open` placement armed by
    /// [`Self::drill_down_prepare`]. Nothing armed leaves the cursor on row 0,
    /// which is what every level without the opt-in has always done.
    ///
    /// A still-empty first page keeps the placement armed instead of burning
    /// it: an empty channel that receives its first message should still put
    /// the cursor there, and the alternative (consume it now) would silently
    /// do nothing at all.
    fn apply_cursor_on_open(&mut self) {
        let Some(placement) = self.pending_cursor_on_open else {
            return;
        };
        // Tree mode drives its own cursor (expansion, not a fresh list) — the
        // hook is a flat-level notion, exactly like `mark_read_on_reach_end`.
        if self.tree.is_some() {
            self.pending_cursor_on_open = None;
            return;
        }
        let rows = self.filtered_indices.len();
        if rows == 0 {
            return;
        }
        self.pending_cursor_on_open = None;
        let target = match placement {
            CursorOnOpen::First => Some(0),
            CursorOnOpen::Last => None,
            CursorOnOpen::FirstUnread => self.first_unread_row(),
        };
        match target {
            // Anchored at the top edge, so what follows the target — the rest
            // of the unread run — is what fills the pane below it.
            Some(row) => self.table.set_selected_at_top(row),
            // `last`, and `first_unread` with nothing unread (see
            // [`CursorOnOpen::FirstUnread`]): the newest row at the bottom
            // edge, which is where `set_selected` scrolls it.
            None => self.table.set_selected(rows - 1),
        }
    }

    /// Display-order row index of the first row carrying an `unread`
    /// metadata field of `"true"`. Flat panes only: rows map back to `items`
    /// through `filtered_indices`, exactly like [`Self::selected_item`].
    fn first_unread_row(&self) -> Option<usize> {
        self.filtered_indices.iter().position(|&i| {
            self.items
                .get(i)
                .is_some_and(|it| metadata_field_value(it, "unread") == "true")
        })
    }

    pub fn rebuild_table(&mut self, view_defs: &[ViewDef]) {
        self.rebuild_table_with(
            view_defs,
            &crate::components::sort_header::HeaderOverlay::None,
        );
    }

    /// Variant that takes the active header overlay so the sort-mode
    /// picker (column / direction) renders correctly. The simple
    /// [`rebuild_table`] is kept for callers that don't have an
    /// overlay handy (no overlay = no picker visible).
    fn rebuild_table_with(
        &mut self,
        view_defs: &[ViewDef],
        header_overlay: &crate::components::sort_header::HeaderOverlay,
    ) {
        // Tree mode: refresh visible-indices before borrowing `theme`
        // immutably — `current_columns` reads them (active depth lookup
        // goes through the visible map) and the build below needs them
        // primed too.
        if self.tree.is_some() {
            self.refresh_tree_visible_indices(view_defs);
            // The entries may have shrunk under the cursor (a new query
            // rebuilt the tree from scratch): clamp the selection so the
            // cursor-based column/level lookups below resolve against a
            // row that actually exists.
            let visible = self.tree_visible_indices.len();
            if visible > 0 && self.table.selected_row() >= visible {
                self.table.set_selected(visible - 1);
            }
        }
        let t = &*self.theme;
        let mut columns: Vec<ColumnDef> = self.current_columns(view_defs);
        if columns.is_empty() {
            // Still record the width we were asked to build for: the
            // post-draw re-fit pass (`refit_tables_if_needed`) compares it
            // against the widget's render width and rebuilds on mismatch —
            // without this stamp a column-less pane (e.g. `manual_connect`
            // before the first load) re-fits forever and the render loop
            // spins at 100 % CPU.
            self.built_table_width = self.table.last_render_width();
            return;
        }

        // Adapter-grouped tree with `group_headers:`: the depth-0 bucket
        // rows render as `── label` header rows (built at the widget stage
        // below) and the optional `total` column is appended for the
        // duration of the grouping — it closes each group, and disappears
        // with grouping cycled off, like the native Trackings tree's Total.
        let tree_headers_active = self.tree_group_headers_def(view_defs).is_some();
        let tree_total_col_idx = self
            .tree_group_headers_def(view_defs)
            .and_then(|g| g.total.as_ref())
            .map(|tc| {
                columns.push(tc.clone());
                columns.len() - 1
            });

        // Lay columns out into the pane's *actual* render width, recorded by
        // the table widget on its last paint. This makes a `flex` column fill
        // exactly to the pane edge (and no further), so trailing columns stay
        // on-screen — matching the native render-time layout. Before the first
        // paint the width is unknown (0); fall back to `PRE_PAINT_TABLE_WIDTH`,
        // then `refit_tables_if_needed` re-fits once the real width is known.
        // `Auto` columns ignore this budget entirely (they self-sufficiently
        // overflow into horizontal scroll), so wide dynamic-schema tables
        // (e.g. postgres rows) are unaffected by the value used here.
        let render_width = self.table.last_render_width();
        let max_width = if render_width == 0 {
            PRE_PAINT_TABLE_WIDTH
        } else {
            render_width as usize
        };
        self.built_table_width = render_width;
        // Single `now` for the whole rebuild so every live (`kind: elapsed`)
        // cell in this frame measures against the same instant. A repaint
        // tick calls rebuild again with a fresh `now` → the timer advances.
        let now = chrono::Local::now();

        // Snapshot the link cache + adapter prefix into a closure so the
        // flat-mode `map(...)` can stay free of `&self` borrows (it needs
        // `&mut self.filtered_indices`).
        let link_refs_snapshot = self.link_refs.clone();
        let link_prefix = self.link_node_ref_prefix.clone();
        let has_link_lookup = |item_id: &str| -> bool {
            let Some(prefix) = link_prefix.as_deref() else {
                return false;
            };
            link_refs_snapshot.contains(&format!("{prefix}/{item_id}"))
        };

        let col_ids: Vec<TColumnId> = columns.iter().map(|c| TColumnId::new(&c.key)).collect();

        let mut strategies = std::collections::HashMap::new();
        for col in &columns {
            let strategy = parse_sizing(&col.sizing);
            strategies.insert(TColumnId::new(&col.key), strategy);
        }

        let config = TableConfig {
            max_width,
            separator: "  ".to_string(),
            sizer: Box::new(MixedColSizer { strategies }),
        };

        let mut header = TRow::new(0u32).not_selectable();
        for col in &columns {
            let label = col.label.as_deref().unwrap_or(&col.key);
            let label = crate::components::sort_header::header_text(
                label,
                &col.key,
                &self.last_applied_sort,
                header_overlay,
            );
            header = header.cell(&col.key, label);
        }

        self.filtered_indices.clear();
        let data_rows: Vec<TRow<u32>> = if self.tree.is_some() {
            // Tree mode renders from `tree.entries` through
            // `tree_visible_indices` (refreshed above). Fuzzy filter
            // only narrows entries at `tree_filter_depth`; `/`-search
            // steps over the same visible rows.
            self.build_tree_data_rows(
                &columns,
                view_defs,
                now,
                tree_headers_active,
                tree_total_col_idx,
            )
        } else {
            // Flat list. Fuzzy filter first (SkimMatcherV2; whitespace-split
            // AND of fuzzy tokens — see `fuzzy_filtered_order`), then either
            // group (M3) or build one plain row per surviving item.
            let order = fuzzy_filtered_order(
                &self.items,
                &columns,
                &self.table.filter_text,
                &self.fuzzy_filter_fields,
            );

            // Grouping (M3): flat list only and never under a multi-line
            // layout. Builds group-header rows + per-group totals + a pinned
            // grand-total footer here and returns — the plain row list below
            // is skipped entirely.
            // Card mode owns the whole row block (frame + grid), so group
            // headers and totals have nowhere to render — grouping stays off
            // while cards are on, exactly like under a multi-line layout.
            if self.current_row_layout(view_defs).is_none() && !self.card_mode_active(view_defs) {
                let levels = self.current_levels(view_defs);
                if !levels.is_empty() {
                    let aggregates = self.current_aggregates(view_defs);
                    self.last_column_keys = columns.iter().map(|c| c.key.clone()).collect();
                    self.table
                        .set_smooth_scroll(self.current_smooth_scroll(view_defs));
                    let build = build_grouped_table(
                        &self.items,
                        &order,
                        &columns,
                        &levels,
                        &aggregates,
                        now,
                        &has_link_lookup,
                        &config,
                        &col_ids,
                        &header,
                        self.long_text,
                    );
                    self.last_col_widths = build.col_widths;
                    self.filtered_indices = build.filtered_indices;
                    let headers = vec![build_header_row(
                        build.header_cells,
                        &columns,
                        header_overlay,
                    )];
                    self.table.set_data(
                        build.widget_rows,
                        vec![],
                        headers,
                        build.footers,
                        ColumnStyles::new(content_col_styles(&columns, t)),
                        build_content_table_style(t),
                        // Grouped views are never trees, so the connector slot
                        // is unused here — the default theme colors are fine.
                        content_style_map(t, t.tree_connector(), t.unread()),
                        "  ",
                    );
                    return;
                }
            }

            self.filtered_indices = order.clone();
            // Flat-mode fuzzy-match highlight: only the searched columns get a
            // highlight (an empty `fuzzy_filter_fields` searches everything, so
            // every column is eligible), so an incidental match in an unrelated
            // column can't paint a misleading highlight.
            let filter_text = self.table.filter_text.clone();
            let highlight_col = |key: &str| -> bool {
                !filter_text.is_empty()
                    && (self.fuzzy_filter_fields.is_empty()
                        || self.fuzzy_filter_fields.iter().any(|f| f == key))
            };
            order
                .iter()
                .enumerate()
                .map(|(row_idx, &item_idx)| {
                    let item = &self.items[item_idx];
                    let mut row = TRow::new(row_idx as u32);
                    for col in &columns {
                        if col.source.as_deref() == Some("has_links") {
                            let icon = if has_link_lookup(&item.id) {
                                "🔗"
                            } else {
                                " "
                            };
                            row = row.cell(&col.key, icon);
                        } else {
                            let mut content = cell_content_for(item, col, now);
                            if highlight_col(&col.key) {
                                let ranges = fuzzy_label_ranges(&content.text, &filter_text);
                                if !ranges.is_empty() {
                                    content = content.with_spans(
                                        ranges
                                            .into_iter()
                                            .map(|r| StyledSpan {
                                                range: r,
                                                style_id: FUZZY_MATCH_STYLE_ID,
                                            })
                                            .collect(),
                                    );
                                }
                            }
                            row = row.cell(&col.key, content);
                        }
                    }
                    row
                })
                .collect()
        };

        self.last_column_keys = columns.iter().map(|c| c.key.clone()).collect();

        // Apply the active level's scroll mode before set_data: set_data →
        // set_rows consults the flag to preserve a line-wise scroll position.
        self.table
            .set_smooth_scroll(self.current_smooth_scroll(view_defs));

        // Card mode: each row becomes a framed card whose fields sit in a
        // grid of `card.columns` slots per line. Checked before `row_layout`
        // — a level may declare both, and the card toggle is the explicit
        // user choice, so it wins while it is on. No column header (the
        // labels live inside the card).
        if let Some(card) = self
            .current_card(view_defs)
            .filter(|_| self.card_mode_active(view_defs))
        {
            let spec = self.card_spec(&card, &columns);
            let (rows, style_map) =
                build_card_widget_rows(&data_rows, &columns, &card, &spec, config.max_width, t);
            self.last_col_widths = Vec::new();
            self.table.set_data(
                rows,
                vec![],
                vec![],
                vec![],
                ColumnStyles::default(),
                build_content_table_style(t),
                style_map,
                // Card spans already carry their own padding and inter-slot
                // separator, so the table must not insert one between cells.
                "",
            );
            return;
        }

        // Multi-line (chat) layout: render each row as a stack of physical
        // lines per `row_layout` instead of one table row. No column header,
        // and the column cursor / horizontal scroll are unused here.
        if let Some(layout) = self.current_row_layout(view_defs) {
            // Per-row unread flags (chat adapters), in the same row order as
            // `data_rows`: the flat path's `filtered_indices` maps each row
            // back to its `self.items` summary so its `unread` field can paint
            // the message header. Empty for the tree path (no chat multiline).
            let unread_rows: Vec<bool> = self
                .filtered_indices
                .iter()
                .map(|&i| {
                    self.items
                        .get(i)
                        .is_some_and(|it| metadata_field_value(it, "unread") == "true")
                })
                .collect();
            let unread_color = self.unread_color(view_defs, t);
            // The store is borrowed only for the build: the table's painter
            // holds the same `Rc` and borrows it again at draw time.
            let images = Rc::clone(&self.images);
            let (rows, style_map) = build_multiline_widget_rows(
                &data_rows,
                &columns,
                &col_ids,
                &config,
                &layout,
                t,
                &unread_rows,
                unread_color,
                &mut images.borrow_mut(),
            );
            self.last_col_widths = Vec::new();
            self.table.set_data(
                rows,
                vec![],
                vec![],
                vec![],
                ColumnStyles::default(),
                build_content_table_style(t),
                style_map,
                "  ",
            );
            return;
        }

        let computed = compute_table(&data_rows, &config, &col_ids, Some(&header));
        self.last_col_widths = computed.col_widths.clone();

        let computed_header = computed
            .header
            .map(|h| build_header_row(h.cells, &columns, header_overlay));

        // `path`-kind columns get their separator drawn in the dedicated
        // style slot (see the StyleMap below); every other cell is plain.
        // Resolved once into a per-column-index lookup so the row loop stays
        // a cheap index check.
        let path_separators: Vec<Option<String>> = columns
            .iter()
            .map(|c| (c.kind == ColumnKind::Path).then(|| path_separator(c).to_string()))
            .collect();
        // In tree mode the label column's leading connector run is tagged with
        // a `StyledSpan` (see `build_tree_data_rows`); that span travels through
        // `compute_table` as the cell's first highlight range. Resolve the
        // label column index once so the row loop can split it into a styled
        // connector segment + plain label. `None` outside tree mode.
        let tree_label_col: Option<usize> = if self.tree.is_some() {
            self.cursor_tree_level(view_defs)
                .map(|l| l.tree_label.to_string())
                .and_then(|key| columns.iter().position(|c| c.key == key))
        } else {
            None
        };
        // Per-visible-row unread flag (chat adapters): the label cell of an
        // unread channel/category paints its marker + name in the unread
        // slot. Parallel to `tree_visible_indices`, so `ri` in the widget
        // loop indexes it directly. Empty (all-false) outside tree mode or
        // when no node carries the `unread` field.
        let unread_rows: Vec<bool> = match self.tree.as_ref() {
            Some(tree) => self
                .tree_visible_indices
                .iter()
                .map(|&eidx| {
                    tree.entries
                        .get(eidx)
                        .is_some_and(|e| metadata_field_value(&e.node, "unread") == "true")
                })
                .collect(),
            None => Vec::new(),
        };
        // Per-visible-row deleted flag: a node whose adapter marks it with a
        // `deleted` metadata field of `"true"` (a soft-deleted record kept in
        // the universe — e.g. a deleted task parent surfaced as context for a
        // matching child) renders every cell dimmed. Parallel to the row order
        // the widget loop walks: tree rows index `tree_visible_indices`, flat
        // rows index `filtered_indices`.
        let deleted_rows: Vec<bool> = match self.tree.as_ref() {
            Some(tree) => self
                .tree_visible_indices
                .iter()
                .map(|&eidx| {
                    tree.entries
                        .get(eidx)
                        .is_some_and(|e| metadata_field_value(&e.node, "deleted") == "true")
                })
                .collect(),
            None => self
                .filtered_indices
                .iter()
                .map(|&i| {
                    self.items
                        .get(i)
                        .is_some_and(|it| metadata_field_value(it, "deleted") == "true")
                })
                .collect(),
        };
        // `group_headers:` rows: map each depth-0 (bucket) row to its label
        // so the loop below can swap in a `── label` summary row — same
        // chrome (`summary_row`, group-header style, non-selectable) as the
        // flat grouping's headers. The bucket label is already the human
        // display label (the adapter formats it via `bucket_display_label`).
        let tree_header_labels: std::collections::HashMap<usize, String> = match self.tree.as_ref()
        {
            Some(tree) if tree_headers_active => self
                .tree_visible_indices
                .iter()
                .enumerate()
                .filter_map(|(row_idx, &eidx)| {
                    let e = tree.entries.get(eidx)?;
                    (e.depth == 0).then(|| (row_idx, e.node.label.clone()))
                })
                .collect(),
            _ => Default::default(),
        };
        let col_widths = self.last_col_widths.clone();
        let widget_rows: Vec<TableWidgetRow> = computed
            .rows
            .into_iter()
            .enumerate()
            .map(|(ri, cr)| {
                if let Some(label) = tree_header_labels.get(&ri) {
                    return summary_row(format!("── {label} "), &[], &columns, &col_widths);
                }
                let highlights = cr.highlights;
                let cells: Vec<TableWidgetCell> = cr
                    .cells
                    .into_iter()
                    .enumerate()
                    .map(|(i, fitted)| {
                        if tree_label_col == Some(i) {
                            // The label cell carries the connector span (always
                            // anchored at char 0) plus any fuzzy-match spans
                            // (always past the connector, so start > 0). Both
                            // arrive here as bare projected ranges — the style
                            // id was dropped by the engine — so split them by
                            // position: the range starting at 0 is the
                            // connector, the rest are matches. Truncation clamps
                            // every range for free.
                            let cell_ranges = highlights.get(i).cloned().unwrap_or_default();
                            let conn = cell_ranges
                                .iter()
                                .find(|r| r.start == 0)
                                .map(|r| r.end)
                                .unwrap_or(0);
                            let matches: Vec<std::ops::Range<usize>> = cell_ranges
                                .iter()
                                .filter(|r| r.start > 0)
                                .cloned()
                                .collect();
                            // Unread rows paint the label remainder (marker +
                            // name) in the unread slot; matched runs still win.
                            let base = if unread_rows.get(ri).copied().unwrap_or(false) {
                                Some(UNREAD_STYLE_ID)
                            } else {
                                None
                            };
                            return TableWidgetCell::from_segments(
                                tree_label_segments_with_highlights(
                                    &fitted,
                                    conn,
                                    TREE_CONNECTOR_STYLE_ID,
                                    &matches,
                                    FUZZY_MATCH_STYLE_ID,
                                    base,
                                ),
                            );
                        }
                        match path_separators.get(i).and_then(|s| s.as_deref()) {
                            Some(sep) => TableWidgetCell::from_segments(path_cell_segments(
                                &fitted,
                                sep,
                                PATH_SEPARATOR_STYLE_ID,
                            )),
                            None => {
                                // Flat-mode fuzzy-match highlight: cells outside
                                // the tree label carry their match ranges as
                                // plain projected ranges (no connector), painted
                                // by the engine's `Highlight` style.
                                let hl = highlights.get(i).cloned().unwrap_or_default();
                                if hl.is_empty() {
                                    TableWidgetCell::plain(fitted)
                                } else {
                                    TableWidgetCell::with_highlights(fitted, hl)
                                }
                            }
                        }
                    })
                    .collect();
                // Deleted rows: override every cell's foreground with the
                // dim slot. On segmented cells (tree label, path) this dims
                // the label/value text while the structural glyphs keep their
                // own slot color — the row reads as present-but-greyed.
                let cells = if deleted_rows.get(ri).copied().unwrap_or(false) {
                    cells
                        .into_iter()
                        .map(|c| c.with_style(DELETED_STYLE_ID))
                        .collect()
                } else {
                    cells
                };
                let row = TableWidgetRow::new(cells);
                // Long-text mode (`v`): expand a `long_source` column into a
                // soft-wrapped multi-line block. Flat list only — tree rows map
                // through `tree_visible_indices`, not `filtered_indices`, and
                // carry their own structure, so they are left untouched.
                if self.long_text && self.tree.is_none() {
                    match self
                        .filtered_indices
                        .get(ri)
                        .and_then(|&i| self.items.get(i))
                    {
                        Some(item) => expand_long_text_row(row, item, &columns, &col_widths),
                        None => row,
                    }
                } else {
                    row
                }
            })
            .collect();

        let tree_connector_col = self.tree_connector_color(view_defs, t);
        let unread_col = self.unread_color(view_defs, t);
        let headers = computed_header.map(|h| vec![h]).unwrap_or_default();
        self.table.set_data(
            widget_rows,
            vec![],
            headers,
            vec![],
            ColumnStyles::new(content_col_styles(&columns, t)),
            build_content_table_style(t),
            content_style_map(t, tree_connector_col, unread_col),
            "  ",
        );
    }

    // ── Bar hints ────────────────────────────────────────────────────

    /// Build the action-bar hints for the current chain position. Each
    /// hint carries its [`ActiveSurface`] (derived at build time from the
    /// action's type / id), so the view's resolver — not the renderer —
    /// decides active-ness. The structural contract holds: every entry here
    /// has a source, so nothing fire-and-forget reaches the top bar.
    pub fn action_bar_hints(
        &self,
        view_defs: &[ViewDef],
        query_menu_key: Option<&str>,
        content_kb: &KeyBindingSection<ContentAction>,
        key_icons: &KeyIconMap,
        adapter: Option<&dyn not_yet_done_content::ContentAdapter>,
    ) -> Vec<ActionBarHint> {
        use crate::views::content_action_hints::source_for_action_type;
        let mut hints: Vec<ActionBarHint> = Vec::new();
        for action in self.current_actions(view_defs) {
            // Event-only actions (no key) never appear in the action bar.
            let Some(key) = action.primary_key().map(str::to_string) else {
                continue;
            };
            if action.shows_in_action_bar() {
                // A `custom` action forced into the bar (`in_action_bar`) is a
                // modal menu→editor flow keyed on its stable id, not one of the
                // fixed typed sources; `on_container` custom keeps the Confirm
                // mapping from `source_for_action_type`.
                let source = match (action.action_type.as_str(), &action.id) {
                    ("custom", Some(id)) if action.in_action_bar => {
                        ActiveSurface::ContentAction(id.clone())
                    }
                    _ => source_for_action_type(&action.action_type, &action.name),
                };
                hints.push(ActionBarHint::new(key, action.name.clone(), source));
            }
        }
        // SH: YAML `shortcuts:` entries whose adapter action is *activatable*
        // — i.e. a bar placement is derived (`source` is `Some`). Placement
        // is a TUI concern derived from the action's InputSpec + id, not
        // declared by the adapter: an action that opens an editor/form/picker
        // (or is delete / toggle-tracking / mark-move) can light up, so it
        // belongs here; a fire-and-forget action has no source and drops to
        // the status bar below. Unknown adapter / node_type → dropped in
        // `collect_shortcut_hints`. Deduplicate against the `actions:`-derived
        // entries above to avoid double-display when the user binds both
        // `actions:` and `shortcuts:` to the same key.
        for sh in self.collect_shortcut_hints(view_defs, adapter) {
            let Some(source) = sh.source else {
                continue;
            };
            if hints.iter().any(|h| h.key == sh.key) {
                continue;
            }
            hints.push(ActionBarHint::new(sh.key, sh.label, source));
        }
        if self.nav_stack.is_empty() {
            if let Some(mk) = query_menu_key {
                if !hints.iter().any(|h| h.key == mk) {
                    hints.push(ActionBarHint::new(
                        mk.to_string(),
                        "queries",
                        ActiveSurface::QueryMenu,
                    ));
                }
            }
        }
        if self.nav_stack.is_empty() && self.is_query_editable(view_defs) {
            if !hints.iter().any(|h| h.label == "edit query") {
                hints.push(ActionBarHint::new(
                    content_kb.hint_label(&ContentAction::EditQuery, key_icons),
                    "edit query",
                    ActiveSurface::Editor("edit query".to_string()),
                ));
            }
        }
        hints
    }

    pub fn status_bar_hints(
        &self,
        view_defs: &[ViewDef],
        common_kb: &KeyBindingSection<CommonAction>,
        content_kb: &KeyBindingSection<ContentAction>,
        key_icons: &KeyIconMap,
        adapter: Option<&dyn not_yet_done_content::ContentAdapter>,
    ) -> Vec<BarHint> {
        let mut hints = Vec::new();
        // CT-8: persistent tree-find status. Placed first so the
        // user sees it on the leftmost slot — once you've kicked off
        // a tree search the only useful action keys are n/N
        // anyway. Drops back to default hints when CT-9 (Esc /
        // reload / new search) clears the cache.
        if let Some(state) = self.tree_find.as_ref() {
            let body = if state.loading {
                format!("Tree find \"{}\": loading…", state.query)
            } else if state.hits.is_empty() {
                format!("Tree find \"{}\": no matches", state.query)
            } else {
                let suffix = if state.truncated { ", truncated" } else { "" };
                format!(
                    "Tree find \"{}\": {}/{}{}",
                    state.query,
                    state.current + 1,
                    state.hits.len(),
                    suffix,
                )
            };
            // Use the configured search-next/prev keys so the hint
            // matches the user's actual bindings (they'd be n/N by
            // default, but a `search.next_key` override in YAML
            // would surface here too).
            let keys = format!("{}/{}", self.search_next_key, self.search_prev_key);
            hints.push((keys, body));
        }
        // Typed Content/Common navigation & fold hints (back, open, paging,
        // tree collapse/expand, grouping, aggregate), derived from the very
        // claim set the dispatcher uses (`build_claims`). This is the heart
        // of the "automatic by design" contract: every claim that the
        // resolver maps to a status-bar nav hint appears here with its live
        // key binding, so a new fold chord like `zm`/`zr` shows up the moment
        // it is claimed — no per-feature hint wiring, and the bar can never
        // drift from what actually dispatches. The resolver returns `None`
        // for elementary keys (list-move, scroll) and for sources rendered
        // through their own richer path below (YAML actions, shortcuts,
        // preview) or in the action bar (menus, jump, edit-query).
        for claim in self.build_claims(view_defs, common_kb, content_kb).claims {
            let Some(nav) = nav_hint_for_source(&claim.source) else {
                continue;
            };
            if nav.bar != HintBar::Status {
                continue;
            }
            let key = claim.key.hint_label(key_icons);
            if hints.iter().any(|(k, _)| k == &key) {
                continue;
            }
            hints.push((key, nav.label.to_string()));
        }
        for action in self.current_actions(view_defs) {
            if !action.shows_in_action_bar() {
                // Event-only actions (no key) have no status-bar hint.
                if let Some(key) = action.primary_key() {
                    hints.push((key.to_string(), action.name.clone()));
                }
            }
        }
        // SH: YAML `shortcuts:` entries whose adapter action is
        // fire-and-forget — no derivable active source, so it renders in the
        // status bar (mirror of the action-bar branch above, which claims the
        // activatable `Some(source)` entries). Same dedup-against-existing-key
        // guard.
        for sh in self.collect_shortcut_hints(view_defs, adapter) {
            if sh.source.is_some() {
                continue;
            }
            if hints.iter().any(|(k, _)| k == &sh.key) {
                continue;
            }
            hints.push((sh.key, sh.label));
        }
        if let Some(preview) = self.current_preview_config(view_defs) {
            if let Some(ref kb) = preview.keybinding {
                hints.push((kb.clone(), "preview".into()));
            }
        }
        // `open` and `prev/next page` are no longer hand-listed here: they are
        // ContentActions claimed by `build_claims` (Open under `cursor_can_open`,
        // paging gated on live page info) and so emitted by the claim-derived
        // loop above.
        hints
    }

    // ── Action-chain entry points ────────────────────────────────────
    // Phase-2 chains call into these methods directly instead of going
    // through the key-binding match in [`handle_key`]. They mirror the
    // semantics of the Open/Back/NextPage/PrevPage arms there but never
    // consult any keymap — the caller has already decided which action to
    // run. Each returns the same kind of [`SubViewMessage`] the key path
    // produces so the App-side handler is shared.

    pub(crate) fn try_drill_open(&self, view_defs: &[ViewDef]) -> SubViewMessage {
        let children = self.current_children(view_defs).to_vec();
        if children.is_empty() {
            return SubViewMessage::Unhandled;
        }
        let row = self.table.selected_row();
        let item_idx = self.filtered_indices.get(row).copied().unwrap_or(row);
        let Some(item) = self.items.get(item_idx) else {
            return SubViewMessage::Unhandled;
        };
        let id = item.id.clone();
        let label = item.label.clone();
        let child_def = children.into_iter().next().unwrap();
        SubViewMessage::ContentDrill {
            item_id: id,
            item_label: label,
            child_def: Box::new(child_def),
        }
    }

    pub(crate) fn try_back(&mut self, view_defs: &[ViewDef]) -> SubViewMessage {
        if self.tree.is_some() {
            return self
                .try_tree_back(view_defs)
                .unwrap_or(SubViewMessage::Unhandled);
        }
        if self.nav_stack.is_empty() {
            return SubViewMessage::Unhandled;
        }
        self.nav_back(view_defs);
        SubViewMessage::SelectionChanged(None)
    }

    pub(crate) fn try_next_page(&mut self, view_index: usize, pane_id: PaneId) -> SubViewMessage {
        let Some(req) = self.next_page_request() else {
            return SubViewMessage::Unhandled;
        };
        self.set_current_page(Some(req));
        if let Some(cq) = self.active_custom_query.clone() {
            let cursor = match cq.mode {
                PaginationMode::Cursor => Some(match cq.cursor_id.clone() {
                    Some(id) => CursorIntent::Continue { cursor_id: id },
                    None => CursorIntent::Open,
                }),
                PaginationMode::Server | PaginationMode::All => None,
            };
            return SubViewMessage::Request(ViewRequest::RunAdapterQuery {
                view_index,
                pane_id,
                node_id: cq.node_id,
                query: cq.query,
                page: req,
                cursor,
            });
        }
        self.reload_current_level(view_index, pane_id)
    }

    pub(crate) fn try_prev_page(&mut self, view_index: usize, pane_id: PaneId) -> SubViewMessage {
        let Some(req) = self.prev_page_request() else {
            return SubViewMessage::Unhandled;
        };
        self.set_current_page(Some(req));
        if let Some(cq) = self.active_custom_query.clone() {
            // NO SCROLL cursors don't fetch backward — `prev` re-opens
            // a fresh cursor (= back to page 1). The old cursor leaks
            // until pane-close cleanup (CP-6); acceptable for the
            // single-pane case and explicitly documented in the plan.
            let cursor = match cq.mode {
                PaginationMode::Cursor => Some(CursorIntent::Open),
                PaginationMode::Server | PaginationMode::All => None,
            };
            return SubViewMessage::Request(ViewRequest::RunAdapterQuery {
                view_index,
                pane_id,
                node_id: cq.node_id,
                query: cq.query,
                page: req,
                cursor,
            });
        }
        self.reload_current_level(view_index, pane_id)
    }

    // ── Key handling ─────────────────────────────────────────────────

    /// Resolve a single-char keypress against the YAML `shortcuts:`
    /// maps for the selected row's node-type chain (Phase CP-1c). On
    /// hit, emits a [`ViewRequest::InvokeNodeAction`] so the App can
    /// drive the async `Node::invoke_action` call. Returns `None` when
    /// the key has no shortcut binding, no node is selected, or the
    /// shortcut targets `parent:` but the cursor sits at root.
    pub(super) fn try_node_action_shortcut(
        &self,
        key: &str,
        view_index: usize,
        pane_id: PaneId,
        view_defs: &[ViewDef],
    ) -> Option<ViewRequest> {
        use crate::app::node_actions::{ShortcutTarget, resolve_shortcut};
        if key.chars().count() != 1 {
            return None;
        }
        let ch = key.chars().next()?;
        let view_def = self.view_def(view_defs)?;
        let chain = self.selected_node_type_chain(view_defs);
        let resolved = resolve_shortcut(view_def, &chain, ch)?;
        let action_name = resolved.action_name.to_string();
        let node_id = match resolved.target {
            ShortcutTarget::Selected => self.selected_item_id()?.to_string(),
            ShortcutTarget::Parent => self.selected_parent_node_id()?,
        };
        Some(ViewRequest::InvokeNodeAction {
            view_index,
            pane_id,
            node_id,
            action_name,
        })
    }

    /// Immediate parent node id of the currently selected row — used
    /// by `parent:`-prefixed shortcuts. In tree mode this walks the
    /// selected entry's `parent_path`; in flat mode it returns
    /// [`Self::parent_node_id`]. Returns `None` at the root level
    /// (the user is on a row whose container has no addressable id).
    fn selected_parent_node_id(&self) -> Option<String> {
        if self.tree.is_some() {
            let row = self.table.selected_row();
            let entry = self.tree_entry_at_row(row)?;
            return entry.parent_path.last().cloned();
        }
        self.parent_node_id().map(str::to_string)
    }

    /// Handle a key press for this pane. Tab-level concerns (subtab
    /// switch, query menu open, saved-query shortcuts, query popup
    /// input routing) are handled by [`ContentView::handle_key`] before
    /// the key reaches here.
    fn handle_key(
        &mut self,
        key: &str,
        view_index: usize,
        pane_id: PaneId,
        view_defs: &[ViewDef],
        common_kb: &KeyBindingSection<CommonAction>,
        content_kb: &KeyBindingSection<ContentAction>,
    ) -> SubViewMessage {
        // Input-mode intercepts. These are not key bindings in the usual
        // sense — the active component absorbs every keystroke until it
        // exits, so they live outside the keymap.
        if self.table.fuzzy_active {
            return self.handle_fuzzy_key(key, view_defs);
        }
        if self.search.active() {
            return self.handle_search_key(key, view_index, pane_id, view_defs);
        }

        // Hardcoded preview-scroll on ctrl+u/d while the preview is open.
        // Not user-configurable, kept outside the keymap.
        if self.preview_open && (key == "ctrl+u" || key == "ctrl+d") {
            let step = (self.preview_visible_height / 2).max(1);
            let max_scroll = self.preview_description.lines().count().saturating_sub(1) as u16;
            self.preview_scroll = if key == "ctrl+d" {
                self.preview_scroll.saturating_add(step).min(max_scroll)
            } else {
                self.preview_scroll.saturating_sub(step)
            };
            return SubViewMessage::SelectionChanged(None);
        }

        // Per-node shortcuts (Phase CP-1c). Resolved before the claims
        // loop so a YAML `shortcuts:` binding wins over any matching
        // ContentAction / ActionDef key. Only single-char keys are
        // eligible — modifier-bearing keys (`ctrl+e`) never trigger.
        if let Some(req) = self.try_node_action_shortcut(key, view_index, pane_id, view_defs) {
            return SubViewMessage::Request(req);
        }

        // Build the active claims for this pane state and dispatch.
        // The same builder feeds the validator (Phase 3); see keymap.rs.
        let claims = self.build_claims(view_defs, common_kb, content_kb);
        for claim in &claims.claims {
            if !claim.key.matches(key) {
                continue;
            }
            if let Some(msg) = self.dispatch_claim(&claim.source, view_index, pane_id, view_defs) {
                return msg;
            }
        }
        SubViewMessage::Unhandled
    }

    /// Emit the [`KeyClaim`]s active for this pane right now. Order is
    /// dispatch priority (earlier wins). Dynamic guards (`nav_stack`
    /// empty, `preview_open`, …) gate which claims appear, so the
    /// dispatcher itself does not need to re-check them.
    fn build_claims(
        &self,
        view_defs: &[ViewDef],
        common_kb: &KeyBindingSection<CommonAction>,
        content_kb: &KeyBindingSection<ContentAction>,
    ) -> KeyMap {
        let mut km = KeyMap::new();
        // Phase 2 uses a placeholder TabRef — Phase 3's validator will
        // plumb the real tab name through from above.
        let scope = KeyScope::Pane(
            TabRef::new(""),
            PaneStateProfile::Normal { drilldown: None },
        );

        // Column cursor: when active, `h`/`l` move the cursor and are
        // stripped from any Content binding below so the user's
        // `content.back = [backspace, h]` etc. doesn't shadow them.
        let column_cursor_on = self.table.selected_column().is_some();
        let reserved_column_keys: &[&str] = if column_cursor_on {
            &[
                crate::keymap::COLUMN_CURSOR_LEFT_KEY,
                crate::keymap::COLUMN_CURSOR_RIGHT_KEY,
            ]
        } else {
            &[]
        };
        let strip_reserved = |mut b: KeyBinding| -> Option<KeyBinding> {
            if reserved_column_keys.is_empty() {
                return Some(b);
            }
            b.0.retain(|k| !reserved_column_keys.contains(&k.as_str()));
            (!b.0.is_empty()).then_some(b)
        };

        // Back — only meaningful when drilled in.
        if !self.nav_stack.is_empty() {
            if let Some(b) = self
                .level_binding(&ContentAction::Back, content_kb, view_defs)
                .and_then(strip_reserved)
            {
                km.push(KeyClaim::handler(
                    b,
                    scope.clone(),
                    KeySource::Content(ContentAction::Back),
                ));
            }
        }

        // Tree smart-collapse — only on tree-mode panes (root has
        // `tree_label`). Defaults to `backspace` (a navigation gesture) so
        // it never shadows the `c` leader; `strip_reserved` keeps it out
        // of the way if column_cursor ever co-exists with a tree leaf.
        if self.tree.is_some() {
            if let Some(b) = content_kb
                .get(&ContentAction::TreeCollapse)
                .cloned()
                .and_then(strip_reserved)
            {
                km.push(KeyClaim::handler(
                    b,
                    scope.clone(),
                    KeySource::Content(ContentAction::TreeCollapse),
                ));
            }
            // Fold-all chords `zm`/`zr` (TreeCollapseAll / TreeExpandAll).
            // These arrive as `z`-prefix chords that the App-level chord
            // interceptor resolves via `dispatch_content_action`, so they
            // never reach this pane's dispatch loop — but they belong in the
            // claim set all the same: it is the single source the status bar
            // and `yaml_action_chord_prefix` derive from, so a fold chord is
            // surfaced (and recognised as a chord prefix) automatically,
            // exactly under the same `tree.is_some()` gate as smart-collapse.
            for ca in [ContentAction::TreeCollapseAll, ContentAction::TreeExpandAll] {
                if let Some(b) = content_kb.get(&ca).cloned().and_then(strip_reserved) {
                    km.push(KeyClaim::handler(b, scope.clone(), KeySource::Content(ca)));
                }
            }
        }

        // Grouping cycle (M3) — only where the level declares a `group_by`.
        // Keyed off the *configured* default (not the effective grouping) so
        // the key stays claimable after the user cycles to "ungrouped".
        if self.level_has_group_by(view_defs) {
            if let Some(b) = content_kb
                .get(&ContentAction::CycleGrouping)
                .cloned()
                .and_then(strip_reserved)
            {
                km.push(KeyClaim::handler(
                    b,
                    scope.clone(),
                    KeySource::Content(ContentAction::CycleGrouping),
                ));
            }
        }

        // Tree-fold aggregation toggle (M4) — only where the active tree
        // level declares a `tree_aggregate` column.
        if self.level_has_tree_aggregate(view_defs) {
            if let Some(b) = content_kb
                .get(&ContentAction::ToggleTreeAggregate)
                .cloned()
                .and_then(strip_reserved)
            {
                km.push(KeyClaim::handler(
                    b,
                    scope.clone(),
                    KeySource::Content(ContentAction::ToggleTreeAggregate),
                ));
            }
        }

        // Jump mode (vimium-style hop) — native Tasks-tab parity. Always
        // claimable: on an empty pane the search just finds nothing and
        // closes. The App-level interceptor (handle_key) drives phases 1/2
        // once open, since `active_table_mut` covers content tabs. Default
        // `J`; configurable via `keybindings.content.jump_mode`.
        if let Some(b) = content_kb
            .get(&ContentAction::JumpMode)
            .cloned()
            .and_then(strip_reserved)
        {
            km.push(KeyClaim::handler(
                b,
                scope.clone(),
                KeySource::Content(ContentAction::JumpMode),
            ));
        }

        // Link-hop — label every link visible in the pane; typing a label
        // opens that URL in the browser. Opt-in per view/child: it is only
        // claimed where a binding is present via `level_binding` (a view or
        // child `keybindings: { link_hop: f }`, or a global
        // `keybindings.content.link_hop`). There is no built-in default, so
        // on views that don't enable it `f` stays a free key. App-level
        // interceptor drives label input.
        if let Some(b) = self
            .level_binding(&ContentAction::LinkHop, content_kb, view_defs)
            .and_then(strip_reserved)
        {
            km.push(KeyClaim::handler(
                b,
                scope.clone(),
                KeySource::Content(ContentAction::LinkHop),
            ));
        }

        // Drill-down / expand — only when the cursor row can open.
        // Recursion-aware (`cursor_can_open`): a recursive tree node has
        // no declared `children:` but still expands into itself.
        if self.cursor_can_open(view_defs) {
            if let Some(b) = self
                .level_binding(&ContentAction::Open, content_kb, view_defs)
                .and_then(strip_reserved)
            {
                km.push(KeyClaim::handler(
                    b,
                    scope.clone(),
                    KeySource::Content(ContentAction::Open),
                ));
            }
        }

        // Common navigation. Always active.
        for ca in [
            CommonAction::ListNext,
            CommonAction::ListPrev,
            CommonAction::ListFirst,
            CommonAction::ListLast,
        ] {
            if let Some(b) = common_kb.bindings.get(&ca) {
                km.push(KeyClaim::handler(
                    b.clone(),
                    scope.clone(),
                    KeySource::Common(ca),
                ));
            }
        }

        // Column cursor. h/l are hardcoded today; if a leaf wants
        // different keys, add fields on ViewDef / ChildDef and keep the
        // keymap.rs constants in sync.
        if column_cursor_on {
            km.push(KeyClaim::handler(
                KeyBinding::new(crate::keymap::COLUMN_CURSOR_LEFT_KEY),
                scope.clone(),
                KeySource::Common(CommonAction::ColumnLeft),
            ));
            km.push(KeyClaim::handler(
                KeyBinding::new(crate::keymap::COLUMN_CURSOR_RIGHT_KEY),
                scope.clone(),
                KeySource::Common(CommonAction::ColumnRight),
            ));
        }

        // Half-page scroll. Suppressed while the preview is open so the
        // hardcoded preview-scroll handler above can claim ctrl+u/d.
        if !self.preview_open {
            for ca in [
                CommonAction::ScrollHalfUp,
                CommonAction::ScrollHalfDown,
                CommonAction::ScrollPageUp,
                CommonAction::ScrollPageDown,
            ] {
                if let Some(b) = common_kb.bindings.get(&ca) {
                    km.push(KeyClaim::handler(
                        b.clone(),
                        scope.clone(),
                        KeySource::Common(ca),
                    ));
                }
            }
        }

        // Pagination — `level_binding` honours per-child overrides. Gated on
        // the live page info so a claim only exists when paging that way is
        // actually possible: this is what lets the status bar derive the
        // `next page` / `prev page` hints straight from the claim set without
        // re-checking `has_next` / `has_prev` itself. Pressing the key at a
        // boundary was already a no-op (`*_page_request()` returns `None`);
        // now the binding simply isn't claimed there.
        for ca in [ContentAction::NextPage, ContentAction::PrevPage] {
            let can_page = match ca {
                ContentAction::NextPage => self.last_page_info.is_some_and(|i| i.has_next),
                ContentAction::PrevPage => self.last_page_info.is_some_and(|i| i.has_prev),
                _ => unreachable!(),
            };
            if !can_page {
                continue;
            }
            if let Some(b) = self.level_binding(&ca, content_kb, view_defs) {
                km.push(KeyClaim::handler(b, scope.clone(), KeySource::Content(ca)));
            }
        }

        // Edit-query — root level + editable.
        if self.nav_stack.is_empty() && self.is_query_editable(view_defs) {
            if let Some(b) = content_kb.get(&ContentAction::EditQuery) {
                km.push(KeyClaim::handler(
                    b.clone(),
                    scope.clone(),
                    KeySource::Content(ContentAction::EditQuery),
                ));
            }
        }

        // Search-result navigation. Registered while either the
        // local `/`-search has matches OR a tree-find cache (CT-5)
        // is live with hits. Dispatch (`PaneSearchJump` arm in
        // `dispatch_claim`) picks the right backend at press time:
        // tree-find wins when active. Otherwise n/N stays free for
        // an action binding.
        // Identity for the routable sources below. Dispatch ignores it (the
        // `YamlAction` / `PaneSearchJump` arms match on name/direction only),
        // but the shortcut menu surfaces these claims as *bindable* rows in
        // the context scope, and the editor's write path resolves the view
        // file from `view` — a blank one yields "No config file found for
        // view ''". So carry the real view name + drilldown child path.
        let source_view = self
            .view_def(view_defs)
            .map(|vd| vd.name.clone())
            .unwrap_or_default();
        let source_child_path = self.current_child_name_path();

        let tree_find_has_hits = self.tree_find.as_ref().is_some_and(|s| !s.hits.is_empty());
        if !self.search.matches().is_empty() || tree_find_has_hits {
            km.push(KeyClaim::handler(
                KeyBinding::new(self.search_next_key.clone()),
                scope.clone(),
                KeySource::PaneSearchJump {
                    view: source_view.clone(),
                    child_path: source_child_path.clone(),
                    action: String::new(),
                    direction: SearchJump::Next,
                },
            ));
            km.push(KeyClaim::handler(
                KeyBinding::new(self.search_prev_key.clone()),
                scope.clone(),
                KeySource::PaneSearchJump {
                    view: source_view.clone(),
                    child_path: source_child_path.clone(),
                    action: String::new(),
                    direction: SearchJump::Prev,
                },
            ));
        }

        // YAML actions for the active level. Event-only actions (no key)
        // claim no key — they run only via the rule engine.
        for action in self.current_actions(view_defs) {
            let Some(binding) = action.key.clone() else {
                continue;
            };
            km.push(KeyClaim::handler(
                binding,
                scope.clone(),
                KeySource::YamlAction {
                    view: source_view.clone(),
                    child_path: source_child_path.clone(),
                    name: action.name.clone(),
                },
            ));
        }

        // Preview toggle.
        if let Some(kb) = self
            .current_preview_config(view_defs)
            .and_then(|p| p.keybinding.as_deref())
        {
            km.push(KeyClaim::handler(
                KeyBinding::new(kb.to_string()),
                scope.clone(),
                KeySource::YamlPreviewKey {
                    view: String::new(),
                    child_path: Vec::new(),
                },
            ));
        }

        km
    }

    /// Run the handler for `source`. Returns `None` only on YAML-action
    /// lookup miss (config drift), which the caller treats as
    /// `SubViewMessage::Unhandled`.
    fn dispatch_claim(
        &mut self,
        source: &KeySource,
        view_index: usize,
        pane_id: PaneId,
        view_defs: &[ViewDef],
    ) -> Option<SubViewMessage> {
        match source {
            KeySource::Content(ContentAction::Back) => {
                if self.tree.is_some() {
                    return self.try_tree_back(view_defs);
                }
                self.nav_back(view_defs);
                Some(SubViewMessage::SelectionChanged(None))
            }
            KeySource::Content(ContentAction::Open) => {
                if self.tree.is_some() {
                    return self.try_tree_open(view_index, pane_id, view_defs);
                }
                // Same `enter_action` override as the tree path — the
                // row's producing ChildDef is `self.active_child` in
                // flat mode. Root-level (active_child == None) has no
                // ChildDef to consult, so the legacy drill path runs.
                if let Some(action_name) = self
                    .active_child
                    .as_ref()
                    .and_then(|c| c.enter_action.as_deref())
                {
                    let row = self.table.selected_row();
                    let item_idx = self.filtered_indices.get(row).copied().unwrap_or(row);
                    let item = self.items.get(item_idx)?;
                    return Some(SubViewMessage::Request(ViewRequest::InvokeNodeAction {
                        view_index,
                        pane_id,
                        node_id: item.id.clone(),
                        action_name: action_name.to_string(),
                    }));
                }
                let children = self.current_children(view_defs).to_vec();
                if children.is_empty() {
                    return None;
                }
                let row = self.table.selected_row();
                let item_idx = self.filtered_indices.get(row).copied().unwrap_or(row);
                let item = self.items.get(item_idx)?;
                let id = item.id.clone();
                let label = item.label.clone();
                let child_def = children.into_iter().next().unwrap();
                Some(SubViewMessage::ContentDrill {
                    item_id: id,
                    item_label: label,
                    child_def: Box::new(child_def),
                })
            }
            KeySource::Content(ContentAction::NextPage) => {
                let req = self.next_page_request()?;
                self.set_current_page(Some(req));
                Some(self.reload_current_level(view_index, pane_id))
            }
            KeySource::Content(ContentAction::PrevPage) => {
                let req = self.prev_page_request()?;
                self.set_current_page(Some(req));
                Some(self.reload_current_level(view_index, pane_id))
            }
            KeySource::Content(ContentAction::EditQuery) => Some(SubViewMessage::Request(
                ViewRequest::OpenContentQueryEditor {
                    view_index,
                    pane_id,
                    save_name: None,
                    is_new: false,
                    // Editing what the pane is running: an extended pane
                    // opens on its document, not on a native query body.
                    kind: self.active_query_kind,
                },
            )),
            KeySource::Content(ContentAction::TreeCollapse) => {
                self.try_tree_smart_collapse(view_defs)
            }
            KeySource::Content(ContentAction::TreeCollapseAll) => {
                self.try_tree_collapse_all(view_defs)
            }
            KeySource::Content(ContentAction::TreeExpandAll) => {
                self.try_tree_expand_all(view_index, pane_id, view_defs)
            }
            KeySource::Content(ContentAction::CycleGrouping) => {
                Some(self.try_cycle_grouping(view_defs, view_index, pane_id))
            }
            KeySource::Content(ContentAction::ToggleTreeAggregate) => {
                Some(self.try_toggle_tree_aggregate(view_defs))
            }
            KeySource::Content(ContentAction::JumpMode) => {
                // Phase 1 only — open the hop overlay. The App-level
                // interceptor feeds subsequent search/label keystrokes.
                self.table.jump_mode_open();
                Some(SubViewMessage::SelectionChanged(None))
            }
            KeySource::Content(ContentAction::LinkHop) => {
                // Label every link visible in this pane. The App-level
                // interceptor feeds subsequent label keystrokes and opens the
                // picked URL.
                Some(self.open_link_hop())
            }
            KeySource::Common(CommonAction::ListNext) => {
                self.nav_and_refresh(Cmd::Move(Direction::Down), view_defs);
                Some(self.preview_after_nav(view_index, pane_id, view_defs))
            }
            KeySource::Common(CommonAction::ListPrev) => {
                self.nav_and_refresh(Cmd::Move(Direction::Up), view_defs);
                Some(self.preview_after_nav(view_index, pane_id, view_defs))
            }
            KeySource::Common(CommonAction::ListFirst) => {
                self.nav_and_refresh(Cmd::GoTo(Position::Begin), view_defs);
                Some(self.preview_after_nav(view_index, pane_id, view_defs))
            }
            KeySource::Common(CommonAction::ListLast) => {
                self.nav_and_refresh(Cmd::GoTo(Position::End), view_defs);
                Some(self.preview_after_nav(view_index, pane_id, view_defs))
            }
            KeySource::Common(CommonAction::ColumnLeft) => {
                self.table.move_column_left();
                Some(SubViewMessage::SelectionChanged(None))
            }
            KeySource::Common(CommonAction::ColumnRight) => {
                self.table.move_column_right();
                Some(SubViewMessage::SelectionChanged(None))
            }
            KeySource::Common(CommonAction::ScrollHalfUp)
            | KeySource::Common(CommonAction::ScrollPageUp) => {
                self.table.scroll_half_page(false);
                Some(SubViewMessage::SelectionChanged(None))
            }
            KeySource::Common(CommonAction::ScrollHalfDown)
            | KeySource::Common(CommonAction::ScrollPageDown) => {
                self.table.scroll_half_page(true);
                Some(SubViewMessage::SelectionChanged(None))
            }
            KeySource::PaneSearchJump { direction, .. } => {
                // CT-7: tree-find wins over local /-search when its
                // cache is live. The walker handles cursor bumping +
                // lazy expansion (it may dispatch ExpandTreeNode
                // requests; the App re-invokes the walker after each
                // child-load settles).
                if self.tree_find_active() {
                    if matches!(direction, SearchJump::Next) {
                        self.tree_find_next();
                    } else {
                        self.tree_find_prev();
                    }
                    return Some(self.tree_find_dispatch_step(view_index, pane_id, view_defs));
                }
                let step = match direction {
                    SearchJump::Next => 1,
                    SearchJump::Prev => -1,
                };
                if let Some(row) = self.search.jump(step) {
                    self.table.set_selected(row);
                }
                Some(SubViewMessage::SelectionChanged(None))
            }
            KeySource::YamlAction { name, .. } => {
                let action = self
                    .current_actions(view_defs)
                    .into_iter()
                    .find(|a| a.name == *name)
                    .cloned()?;
                Some(self.execute_action(&action, view_index, pane_id, view_defs))
            }
            KeySource::YamlPreviewKey { .. } => {
                self.preview_open = !self.preview_open;
                if self.preview_open {
                    if let Some(p) = self.update_preview_for_selection(view_defs) {
                        return Some(SubViewMessage::Request(ViewRequest::FetchContentPreview {
                            view_index,
                            pane_id,
                            cache_key: p.cache_key,
                            node_id: p.node_id,
                            action_id: p.action_id,
                        }));
                    }
                }
                Some(SubViewMessage::SelectionChanged(None))
            }
            // Sources the pane never emits — defensive default; reaching
            // this would mean `build_claims` and `dispatch_claim` drifted.
            _ => None,
        }
    }

    /// Open link-hop on this pane: extract `(needle, url)` targets from every
    /// rendered fragment (each item's label + metadata field values), label
    /// the ones the table can see, and leave the overlay armed for the
    /// App-level interceptor. Notifies (rather than arming) when nothing is
    /// visible to open. See [`crate::views::link_extract`].
    fn open_link_hop(&mut self) -> SubViewMessage {
        let fragments: Vec<&str> = self
            .items
            .iter()
            .flat_map(|it| {
                std::iter::once(it.label.as_str())
                    .chain(it.metadata.fields.iter().map(|f| f.value.as_str()))
            })
            .collect();
        let targets = crate::views::link_extract::extract_links_from(fragments.iter().copied());
        if targets.is_empty() {
            return SubViewMessage::Request(ViewRequest::Notify("No links on screen".to_string()));
        }
        let count = self.table.link_hop_open(&targets);
        if count == 0 {
            return SubViewMessage::Request(ViewRequest::Notify("No links on screen".to_string()));
        }
        SubViewMessage::SelectionChanged(None)
    }

    /// Every distinct image URL linked from this pane's items, first-seen
    /// order preserved. The link-hop uses this to download all images in the
    /// pane into one directory so the OS viewer can page between them.
    pub fn image_link_urls(&self) -> Vec<String> {
        let fragments: Vec<&str> = self
            .items
            .iter()
            .flat_map(|it| {
                std::iter::once(it.label.as_str())
                    .chain(it.metadata.fields.iter().map(|f| f.value.as_str()))
            })
            .collect();
        let mut urls: Vec<String> = Vec::new();
        for (_needle, url) in
            crate::views::link_extract::extract_links_from(fragments.iter().copied())
        {
            if crate::views::link_extract::is_image_url(&url) && !urls.contains(&url) {
                urls.push(url);
            }
        }
        urls
    }

    /// Helper used after every cursor move: emit a preview-fetch request
    /// when the new selection has no preview yet, otherwise just notify.
    fn preview_after_nav(
        &mut self,
        view_index: usize,
        pane_id: PaneId,
        view_defs: &[ViewDef],
    ) -> SubViewMessage {
        if let Some(p) = self.update_preview_for_selection(view_defs) {
            return SubViewMessage::Request(ViewRequest::FetchContentPreview {
                view_index,
                pane_id,
                cache_key: p.cache_key,
                node_id: p.node_id,
                action_id: p.action_id,
            });
        }
        SubViewMessage::SelectionChanged(None)
    }

    fn handle_fuzzy_key(&mut self, key: &str, view_defs: &[ViewDef]) -> SubViewMessage {
        match key {
            "enter" => {
                self.table.fuzzy_close();
                self.rebuild_table(view_defs);
                // Closing with an empty query is a cancel: restore the
                // pre-filter tree shape. A non-empty query keeps the filter
                // (and the eager expansion) live.
                if self.table.filter_text.is_empty() {
                    self.restore_tree_filter_expand(view_defs);
                }
            }
            "esc" => {
                if self.table.fuzzy_query.is_empty() {
                    self.table.fuzzy_close();
                    self.restore_tree_filter_expand(view_defs);
                } else {
                    self.table.fuzzy_query.clear();
                    self.table.fuzzy_cursor = 0;
                    self.table.filter_text.clear();
                    self.rebuild_table(view_defs);
                    self.restore_tree_filter_expand(view_defs);
                }
            }
            "ctrl+u" => {
                self.table.fuzzy_query.clear();
                self.table.fuzzy_cursor = 0;
                self.table.filter_text.clear();
                self.rebuild_table(view_defs);
                self.restore_tree_filter_expand(view_defs);
            }
            "backspace" => {
                self.table.fuzzy_backspace();
                self.rebuild_table(view_defs);
                // Backspacing the last character clears the filter — drop the
                // eager expansion too.
                if self.table.filter_text.is_empty() {
                    self.restore_tree_filter_expand(view_defs);
                }
            }
            "left" => {
                self.table.fuzzy_cursor_left();
            }
            "right" => {
                self.table.fuzzy_cursor_right();
            }
            ch if ch.chars().count() == 1 && !ch.chars().next().unwrap().is_control() => {
                self.table.fuzzy_insert(ch.chars().next().unwrap());
                self.rebuild_table(view_defs);
            }
            _ => {}
        }
        SubViewMessage::SelectionChanged(None)
    }

    fn handle_search_key(
        &mut self,
        key: &str,
        view_index: usize,
        pane_id: PaneId,
        view_defs: &[ViewDef],
    ) -> SubViewMessage {
        let result = self.search.handle_key(key);
        let mode = self.search_mode.clone();
        match (result, mode) {
            (SearchKeyResult::QueryChanged, SearchMode::Local) => {
                let descs = self.search_descriptions(view_defs);
                let refs: Vec<(usize, &str)> =
                    descs.iter().map(|(i, s)| (*i, s.as_str())).collect();
                self.search.update_matches(&refs);
                if let Some(row) = self.search.first_match() {
                    self.table.set_selected(row);
                }
            }
            (SearchKeyResult::Accepted, SearchMode::Adapter { ref template, .. }) => {
                let q = self.search.state().query;
                self.search.clear();
                self.search_mode = SearchMode::Local;
                let query = render_text_search(template, &q);
                self.set_query(query.clone(), None);
                // Remember what this search produced: the pane keeps showing
                // its result after the input closes, and the action bar keeps
                // the hint lit for as long as it does.
                self.text_search_query = Some(query);
                return SubViewMessage::Request(ViewRequest::SpawnContentLoad {
                    view_index,
                    pane_id,
                });
            }
            (SearchKeyResult::Cancelled, SearchMode::Adapter { .. }) => {
                self.search_mode = SearchMode::Local;
            }
            (SearchKeyResult::Accepted, SearchMode::TreeFind { .. }) => {
                let q = self.search.state().query;
                self.search.clear();
                self.search_mode = SearchMode::Local;
                // Empty query → no point dispatching; act like Cancel.
                if q.trim().is_empty() {
                    self.tree_find_clear();
                    return SubViewMessage::SelectionChanged(None);
                }
                return SubViewMessage::Request(ViewRequest::TreeFindStart {
                    view_index,
                    pane_id,
                    query: q,
                });
            }
            (SearchKeyResult::Cancelled, SearchMode::TreeFind { .. }) => {
                self.search_mode = SearchMode::Local;
                // CT-9: cancelling the input also drops any cached
                // hits so n/N revert to the local /-search dispatch.
                self.tree_find_clear();
            }
            _ => {}
        }
        SubViewMessage::SelectionChanged(None)
    }

    fn search_descriptions(&self, view_defs: &[ViewDef]) -> Vec<(usize, String)> {
        if let Some(tree) = self.tree.as_ref() {
            // Tree mode: iterate visible (post-fuzzy-filter) entries so
            // `/`-search only steps over what the user can actually see.
            // Pagination placeholders are skipped — searching the
            // literal "weitere laden" string would only ever land the
            // cursor on the loader row.
            let vd = self.view_def(view_defs);
            let mut out = Vec::new();
            for (row_idx, &eidx) in self.tree_visible_indices.iter().enumerate() {
                let Some(entry) = tree.entries.get(eidx) else {
                    continue;
                };
                if entry.is_more_placeholder {
                    continue;
                }
                let level_columns: Vec<ColumnDef> = vd
                    .and_then(|v| tree_level_for_chain(v, &entry.node_type_chain))
                    .map(|l| l.columns.to_vec())
                    .unwrap_or_default();
                let text = build_field_haystack(&entry.node, &level_columns, &self.search_fields);
                out.push((row_idx, text));
            }
            return out;
        }
        let columns: Vec<ColumnDef> = self.current_columns(view_defs);
        let mut out = Vec::new();
        for (row_idx, &item_idx) in self.filtered_indices.iter().enumerate() {
            let Some(item) = self.items.get(item_idx) else {
                continue;
            };
            let text = build_field_haystack(item, &columns, &self.search_fields);
            out.push((row_idx, text));
        }
        out
    }

    fn execute_action(
        &mut self,
        action: &ActionDef,
        view_index: usize,
        pane_id: PaneId,
        view_defs: &[ViewDef],
    ) -> SubViewMessage {
        match action.action_type.as_str() {
            "edit" => {
                if let Some(id) = self.resolve_action_node_id(action) {
                    let action_id = action.id.clone().unwrap_or_else(|| "edit_full".into());
                    return SubViewMessage::Request(ViewRequest::OpenContentEditor {
                        view_index,
                        pane_id,
                        node_id: id,
                        action_id,
                        label: action.name.clone(),
                        editor_profile: action.editor.clone(),
                        commit_on_save: action.commit_on_save,
                    });
                }
            }
            "navigate" => {
                if let Some(ref target) = action.navigate_to {
                    let children = self.current_children(view_defs).to_vec();
                    if let Some(child_def) = children.into_iter().find(|c| c.node_type == *target) {
                        let row = self.table.selected_row();
                        let item_idx = self.filtered_indices.get(row).copied().unwrap_or(row);
                        if let Some(item) = self.items.get(item_idx) {
                            let id = item.id.clone();
                            let label = item.label.clone();
                            return SubViewMessage::ContentDrill {
                                item_id: id,
                                item_label: label,
                                child_def: Box::new(child_def),
                            };
                        }
                    }
                }
            }
            "query_edit" => {
                if self.is_query_editable(view_defs) {
                    return SubViewMessage::Request(ViewRequest::OpenContentQueryEditor {
                        view_index,
                        pane_id,
                        save_name: None,
                        is_new: false,
                        kind: self.active_query_kind,
                    });
                }
            }
            "reload" => {
                // CT-9: the tree cache about to be rebuilt may
                // invalidate the hit's ancestor ids (stale page
                // copies, deleted parents, …). Drop tree_find so
                // the user gets clean n/N + status hints again.
                self.tree_find_clear();
                // A user-initiated reload is a *hard* refresh: abort any
                // in-flight adapter load and drop caches before re-listing, so
                // `r` always fetches fresh (a warm cache would otherwise just
                // re-serve the same rows).
                return SubViewMessage::Request(ViewRequest::ForceReloadContent {
                    view_index,
                    pane_id,
                });
            }
            "open_url" => {
                // TODO: open node URL in browser
            }
            "download" => {
                // TODO: download node content to file
            }
            "create" => {
                let Some(action_id) = action.id.clone() else {
                    return SubViewMessage::Request(ViewRequest::Notify(format!(
                        "create action '{}' missing `id` (e.g. id: create_comment)",
                        action.name
                    )));
                };
                // `under_selection`: parent the new node on the highlighted
                // row (tree or flat) rather than the drilled-into container.
                // The new child's node_type is the selected node's own type —
                // correct for a recursive tree (task:item → task:item); the
                // container path stays the default for every other create.
                let (parent_id, child_type) = if action.under_selection {
                    (
                        self.selected_item_id().map(str::to_string),
                        self.selected_node_type_chain(view_defs).last().cloned(),
                    )
                } else {
                    (
                        self.parent_node_id().map(str::to_string),
                        self.current_child_node_type().map(str::to_string),
                    )
                };
                // A `None` parent means "create on the adapter root": an
                // `under_selection` create with nothing selected (empty tree),
                // or a container create at the un-drilled root level (nav stack
                // empty). The async handler resolves it to `adapter.root()`.
                // Either way we still need a child node_type for the reload
                // drilldown; fall back to this view's own node_type when there's
                // no selection/active child to read it from.
                let child_type =
                    child_type.or_else(|| self.view_def(view_defs).map(|v| v.node_type.clone()));
                if let Some(child_type) = child_type {
                    return SubViewMessage::Request(ViewRequest::CreateContentChild {
                        view_index,
                        pane_id,
                        parent_node_id: parent_id,
                        child_node_type: child_type,
                        action_id,
                        label: action.name.clone(),
                        editor_profile: action.editor.clone(),
                        commit_on_save: action.commit_on_save,
                    });
                }
            }
            "custom" => {
                // `on_container`: invoke the action on the adapter's
                // container (root) rather than the selected row, through the
                // `invoke_action` dispatch path (so the adapter's
                // `ActionDispatch` — e.g. a `Confirm` — is honoured). This is
                // the only `custom` flavour that goes through `invoke_action`;
                // the default `custom` still uses the popup/`execute` path.
                if action.on_container {
                    if let Some(action_id) = &action.id {
                        return SubViewMessage::Request(ViewRequest::InvokeContainerAction {
                            view_index,
                            pane_id,
                            action_name: action_id.clone(),
                        });
                    }
                    return SubViewMessage::Request(ViewRequest::Notify(format!(
                        "container action '{}' missing `id`",
                        action.name
                    )));
                }
                if let (Some(id), Some(action_id)) =
                    (self.resolve_action_node_id(action), &action.id)
                {
                    return SubViewMessage::Request(ViewRequest::ExecuteContentAction {
                        view_index,
                        pane_id,
                        node_id: id,
                        action_id: action_id.clone(),
                    });
                }
            }
            "script" => {
                return SubViewMessage::Request(ViewRequest::OpenScriptMenuForNode {
                    view_index,
                    pane_id,
                    scope: action.script_scope,
                    default_field: action.script_default_field.clone(),
                });
            }
            "option_menu" => {
                let Some(config) = action.option_menu.clone() else {
                    return SubViewMessage::Request(ViewRequest::Notify(format!(
                        "option_menu action '{}' missing `option_menu` config",
                        action.name
                    )));
                };
                return SubViewMessage::Request(ViewRequest::OpenOptionMenuForNode {
                    view_index,
                    pane_id,
                    config,
                });
            }
            "invalidate_session" => {
                return SubViewMessage::Request(ViewRequest::InvalidateContentSession {
                    view_index,
                });
            }
            "invalidate_credentials" => {
                return SubViewMessage::Request(ViewRequest::InvalidateContentCredentials {
                    view_index,
                });
            }
            "fuzzy_filter" => {
                self.fuzzy_filter_fields = action
                    .fuzzy_filter
                    .as_ref()
                    .map(|c| c.fields.clone())
                    .unwrap_or_default();
                if self.tree.is_some() {
                    self.tree_filter_depth = self.resolve_tree_filter_depth(view_defs);
                }
                self.table.fuzzy_open();
                // Eager tree: pull the WHOLE subtree up front and expand it, so
                // the filter matches across collapsed / not-yet-paged branches
                // (native parity). Stash the pre-filter expansion first so the
                // tree re-collapses to its old shape when the filter clears.
                if self.eager_subtree_depth(view_defs).is_some()
                    && self.tree_filter_expand_stash.is_none()
                {
                    if let Some(tree) = self.tree.as_ref() {
                        self.tree_filter_expand_stash = Some(tree.expanded.clone());
                    }
                    return SubViewMessage::Request(ViewRequest::EagerExpandSubtree {
                        view_index,
                        pane_id,
                    });
                }
                return SubViewMessage::SelectionChanged(None);
            }
            "search" => {
                let cfg = action.search.as_ref();
                self.search_fields = cfg.map(|c| c.fields.clone()).unwrap_or_default();
                self.search_next_key = cfg
                    .and_then(|c| c.next_key.clone())
                    .unwrap_or_else(|| "n".to_string());
                self.search_prev_key = cfg
                    .and_then(|c| c.prev_key.clone())
                    .unwrap_or_else(|| "N".to_string());
                self.search_mode = SearchMode::Local;
                self.search.clear();
                self.search.open();
                return SubViewMessage::SelectionChanged(None);
            }
            "text_search" => {
                let (template, prompt) = action
                    .text_search
                    .as_ref()
                    .map(|c| (c.query_template.clone(), c.prompt.clone()))
                    .unwrap_or_else(|| ("{q}".to_string(), None));
                self.search_mode = SearchMode::Adapter { template, prompt };
                self.search.clear();
                self.search.open();
                return SubViewMessage::SelectionChanged(None);
            }
            "tree_find" => {
                // CT-7: open the search input in tree-find mode.
                // The optional `tree_find: { prompt: ... }` YAML
                // block labels the bar ("Search pages"); falls back
                // to "tree search…" if absent. Wipe any stale
                // tree-find cache so re-opening doesn't surface
                // yesterday's hits.
                let prompt = action.tree_find.as_ref().and_then(|c| c.prompt.clone());
                self.tree_find_clear();
                self.search_mode = SearchMode::TreeFind { prompt };
                self.search.clear();
                self.search.open();
                return SubViewMessage::SelectionChanged(None);
            }
            _ => {}
        }
        SubViewMessage::Unhandled
    }
}

impl ContentView {
    /// Names of the actions this view binds to bus `topic` via its
    /// `event_actions:` list, across every `view_defs` entry (subtab). Used by
    /// the App's rule engine to route a [`BusEvent`] to the right action(s)
    /// regardless of which tab or subtab is currently active.
    ///
    /// [`BusEvent`]: not_yet_done_content::BusEvent
    pub fn event_action_targets(&self, topic: &str) -> Vec<String> {
        self.view_defs
            .iter()
            .flat_map(|vd| vd.event_actions.iter())
            .filter(|b| b.on == topic)
            .map(|b| b.run.clone())
            .collect()
    }

    /// Look up an action by `name` across every `view_defs` entry's own
    /// actions (cursor-independent — event actions may be keyless and sit on
    /// any level). Returns a clone so the caller can inspect it without holding
    /// a borrow on the view.
    pub fn find_action_by_name(&self, name: &str) -> Option<ActionDef> {
        self.view_defs
            .iter()
            .flat_map(|vd| vd.actions.iter())
            .find(|a| a.name == name)
            .cloned()
    }

    /// Run the action named `name` in response to a bus event. Unlike the key
    /// path this carries **no cursor context**: it looks the action up by name
    /// across every `view_defs` entry's own actions (event-only actions have no
    /// key and need not sit on the active level), so e.g. an MFA notify fires
    /// no matter what row or subtab is focused. Returns
    /// [`SubViewMessage::Unhandled`] when no such action exists.
    pub fn dispatch_event_action(&mut self, name: &str) -> SubViewMessage {
        let view_index = self.view_index;
        let pane_id = self.active_pane_id();
        let view_defs = self.view_defs.clone();
        let Some(action) = view_defs
            .iter()
            .flat_map(|vd| vd.actions.iter())
            .find(|a| a.name == name)
            .cloned()
        else {
            return SubViewMessage::Unhandled;
        };
        self.active_pane_mut()
            .execute_action(&action, view_index, pane_id, &view_defs)
    }

    pub fn new(
        theme: Arc<Theme>,
        config: &ViewFileConfig,
        adapter: Option<Arc<dyn ContentAdapter>>,
        keybindings: &KeyBindingConfig,
    ) -> Self {
        let query_menu_kb = keybindings.query_menu.clone();
        let common_kb = keybindings.common.clone();
        let content_kb = keybindings.content.clone();
        let window_kb = keybindings.window.clone();
        // Pane-tag alphabet, filtered against the chord-action keys
        // configured on `window_kb` so a tag like `s` can never shadow
        // a chord like `ws` (close).
        let reserved: std::collections::HashSet<char> = window_kb
            .bindings
            .values()
            .flat_map(|kb| kb.0.iter())
            .filter_map(|s| s.chars().last())
            .collect();
        let pane_tag_alphabet: String = keybindings
            .pane_tags
            .0
            .chars()
            .filter(|c| !reserved.contains(c))
            .collect();
        let key_icons = keybindings.key_icons.clone();
        let action_bar = ActionBarComponent::new(Arc::clone(&theme));

        let default_view = config
            .views
            .iter()
            .find(|v| v.default)
            .or(config.views.first());
        // Scope: NodeRef of the view root, structurally `<adapter>/<instance>/<view_name>`
        // (e.g. "jira/jira/tickets"). Path-segment form mirrors the
        // app-wide `NodeRef` convention so a shortcut row can in
        // principle live on any level of the hierarchy, identified by
        // its full path. Default instance_id falls back to adapter_type
        // for single-instance configs.
        let view_name = default_view.map(|vd| vd.name.as_str()).unwrap_or("default");
        let query_scope = format!(
            "{}/{}/{}",
            config.adapter.adapter_type,
            config.adapter.effective_instance_id(),
            view_name,
        );

        let active_subtab = config.views.iter().position(|v| v.default).unwrap_or(0);

        // Snapshot the adapter's capabilities once — the adapter is fixed for
        // this view's lifetime. Panes read this to gate capability-dependent
        // affordances (e.g. `toggle_tree_aggregate`). No adapter → all-false.
        let capabilities = adapter
            .as_ref()
            .map(|a| a.capabilities())
            .unwrap_or_default();

        // One single-leaf pane tree per ViewDef. Splits add leaves later.
        let mut next_pane_id: PaneId = 0;
        let pane_trees: Vec<PaneTree> = (0..config.views.len())
            .map(|i| {
                let id = next_pane_id;
                next_pane_id += 1;
                let tree_enabled = config.views[i].tree_label.is_some();
                let pane =
                    ContentPane::new(Arc::clone(&theme), i, tree_enabled, capabilities.clone());
                let mut tree = PaneTree::new(i, id, pane);
                tree.assign_tag(id, &pane_tag_alphabet);
                tree
            })
            .collect();

        let query_menu = QueryMenuComponent::new(Arc::clone(&theme), "Queries")
            .with_popup_kb(keybindings.popup.clone(), keybindings.key_icons.clone());
        let group_menu = TabSetPopup::new(Arc::clone(&theme)).with_title("Group by");
        let mut cv = Self {
            action_bar,
            active_editor: None,
            content_editor_action_id: None,
            content_action_popup_id: None,
            tracking_active: false,
            cut_active: false,
            confirm_active: false,
            column_config_active: false,
            script_active: false,
            global_action_hints: Vec::new(),
            theme,
            cmdline: CmdlineComponent::new(),
            tab_name: config.tab.name.clone(),
            tab_icon: config.tab.icon.clone().unwrap_or_default(),
            tab_order: config.tab.order,
            tab_key: config.tab.key.clone(),
            tab_unread_marker: config.tab.unread_marker.clone(),
            tab_unread_style: config.tab.unread_style.clone(),
            tab_load_banner: config.tab.load_banner,
            // Without an override this is the config default; App replaces it
            // with the user's `notifications.load_banner` when it wires the view.
            load_banner_route: config.tab.load_banner.unwrap_or_default(),
            view_index: 0, // set by App after construction
            adapter,
            view_defs: config.views.clone(),
            pane_trees,
            active_subtab,
            next_pane_id,
            auth_status: AdapterStatus::Ready,
            adapter_init_error: None,
            query_menu,
            group_menu,
            query_menu_mode: QueryMenuMode::SavedQueries,
            query_menu_kb,
            common_kb,
            content_kb,
            window_kb,
            window_pending: None,
            pane_tag_alphabet,
            key_icons,
            db_saved_queries: Vec::new(),
            default_saved_query: None,
            query_scope,
            header_overlay: crate::components::sort_header::HeaderOverlay::default(),
            source_path: None,
            manual_connect: config.adapter.manual_connect,
            reminder: config.reminder.clone(),
            connected_once: false,
            pending_cursor_closes: Vec::new(),
            pending_mark_read: None,
            node_script_shortcuts: std::collections::HashMap::new(),
            script_shortcuts: std::collections::HashMap::new(),
            column_overrides: std::collections::HashMap::new(),
            card_mode_overrides: std::collections::HashMap::new(),
            nav_chars: Vec::new(),
        };
        cv.sync_action_bar_hints();
        cv
    }

    /// Drain the queue of cursor ids that the App should close on the
    /// adapter (CP-6). Populated by [`Self::close_focused`] when panes
    /// that were paginating via a server-side cursor get destroyed.
    /// The App calls this after every interaction with the view and
    /// emits one `ViewRequest::CloseAdapterCursor` per id.
    pub fn take_pending_cursor_closes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_cursor_closes)
    }

    pub fn has_adapter(&self) -> bool {
        self.adapter.is_some()
    }

    /// Borrow the active pane (the focused leaf in the active subtab tree).
    pub fn active_pane(&self) -> &ContentPane {
        &self.pane_trees[self.active_subtab].focused_leaf().pane
    }

    /// Mutably borrow the active pane.
    pub fn active_pane_mut(&mut self) -> &mut ContentPane {
        &mut self.pane_trees[self.active_subtab].focused_leaf_mut().pane
    }

    /// [`PaneId`] of the focused leaf in the active subtab.
    pub fn active_pane_id(&self) -> PaneId {
        self.pane_trees[self.active_subtab].focus
    }

    /// The pagination mode a pane's current level declares (see
    /// [`ContentPane::resolve_pagination_mode`]). The App side needs it
    /// *before* the first custom query runs — only a level configured for
    /// cursor pagination may ask an adapter to open a cursor, and not every
    /// backend has one. Falls back to [`PaginationMode::Server`] for an
    /// unknown pane, which is the mode every adapter supports.
    pub fn pane_pagination_mode(&self, id: PaneId) -> PaginationMode {
        let view_defs = self.view_defs.clone();
        self.find_pane(id)
            .map(|pane| pane.resolve_pagination_mode(&view_defs))
            .unwrap_or(PaginationMode::Server)
    }

    /// Walk every pane tree and return the leaf with the given id, if any.
    pub fn find_pane(&self, id: PaneId) -> Option<&ContentPane> {
        self.pane_trees
            .iter()
            .find_map(|tree| tree.root.find_leaf(id).map(|leaf| &leaf.pane))
    }

    /// Every leaf pane id across all subtab split-trees. Used by the
    /// App's live-invalidation handler to decide which panes to reload.
    pub fn all_pane_ids(&self) -> Vec<PaneId> {
        let mut ids = Vec::new();
        for tree in self.pane_trees.iter() {
            tree.root.collect_leaf_ids(&mut ids);
        }
        ids
    }

    /// Rebuild one specific pane's table, focused or not. Used for
    /// out-of-band results that concern a single pane — an inline picture
    /// finishing its download. An unknown id (pane closed meanwhile) is a
    /// no-op.
    ///
    /// The disjoint-field access mirrors [`Self::drive_tree_find`]: the
    /// borrow checker only sees `&self.view_defs` and `&mut self.pane_trees`
    /// as disjoint through direct field projection, not through
    /// [`Self::find_pane_mut`].
    pub fn rebuild_pane_table(&mut self, id: PaneId) {
        let view_defs = &self.view_defs;
        let pane = self
            .pane_trees
            .iter_mut()
            .find_map(|tree| tree.root.find_leaf_mut(id).map(|leaf| &mut leaf.pane));
        if let Some(pane) = pane {
            pane.rebuild_table(view_defs);
        }
    }

    /// Mutable variant of [`find_pane`].
    pub fn find_pane_mut(&mut self, id: PaneId) -> Option<&mut ContentPane> {
        self.pane_trees
            .iter_mut()
            .find_map(|tree| tree.root.find_leaf_mut(id).map(|leaf| &mut leaf.pane))
    }

    /// CT-7: advance the lazy-expand walk for the pane's active
    /// tree-find. Returns the next [`SubViewMessage`] to process,
    /// or `None` when the pane has no active tree-find / doesn't
    /// exist. Used by the App after `TreeFindResult` and after each
    /// `TreeChildren` to drive the chain forward until the leaf is
    /// reached (or the walk reports `NotInTree`).
    ///
    /// The disjoint-field access (`&self.view_defs` +
    /// `&mut self.pane_trees`) sidesteps the `find_pane_mut`
    /// shorthand because the borrow checker can only see disjoint
    /// borrows through direct field projection.
    pub fn drive_tree_find(
        &mut self,
        view_index: usize,
        pane_id: PaneId,
    ) -> Option<SubViewMessage> {
        let view_defs: &[ViewDef] = &self.view_defs;
        let pane: &mut ContentPane = self
            .pane_trees
            .iter_mut()
            .find_map(|tree| tree.root.find_leaf_mut(pane_id).map(|leaf| &mut leaf.pane))?;
        if !pane.tree_find_active() {
            return None;
        }
        Some(pane.tree_find_dispatch_step(view_index, pane_id, view_defs))
    }

    /// Push current view state into the bar. Called by App once per frame.
    pub fn sync_action_bar(
        &mut self,
        active_editor: Option<&str>,
        content_editor_action_id: Option<&str>,
        content_action_popup_id: Option<&str>,
        tracking_active: bool,
        cut_active: bool,
        confirm_active: bool,
        column_config_active: bool,
        script_active: bool,
        global_action_hints: Vec<ActionHint>,
    ) {
        // Store the cross-cutting active state so the hint builder can
        // resolve each hint's `active` flag (the bar no longer special-cases
        // descriptions). Must happen before `action_bar_hints()` reads it.
        self.active_editor = active_editor.map(|s| s.to_string());
        self.content_editor_action_id = content_editor_action_id.map(|s| s.to_string());
        self.content_action_popup_id = content_action_popup_id.map(|s| s.to_string());
        self.tracking_active = tracking_active;
        self.cut_active = cut_active;
        self.confirm_active = confirm_active;
        self.column_config_active = column_config_active;
        self.script_active = script_active;
        // App-global hints (shortcut menu, …) are appended by
        // `action_bar_hints`; store them first so that path — and the lighter
        // `sync_action_bar_hints` refresh — both include them.
        self.global_action_hints = global_action_hints;

        // Snapshot every pane-derived value into locals before touching
        // `self.action_bar` so the borrow on `self.pane_trees[..]` ends
        // before the mutable borrow on `action_bar` begins.
        let hints = self.action_bar_hints();
        let active_filter_name = self.active_pane().active_query_name.clone();
        let favs: Vec<(String, String)> = self
            .db_saved_queries
            .iter()
            .filter_map(|sq| sq.shortcut.as_ref().map(|s| (sq.name.clone(), s.clone())))
            .collect();
        // Script shortcuts (`:script`-menu chords and, on Postgres, per-table
        // script chords) already dispatch via the claims registered in
        // `build_view_claims`, but — unlike saved-query favorites — never had
        // a bar entry. Surface them as their own bar group (rendered with a
        // separator only when non-empty) so a bound script chord is
        // discoverable, not just its underlying menu key. Same scopes as the
        // claim registration so what shows is exactly what dispatches.
        let mut script_favs: Vec<(String, String)> = Vec::new();
        if let Some(scope) = self.focused_script_scope() {
            if let Some(entries) = self.script_shortcuts.get(&scope) {
                script_favs.extend(entries.iter().cloned());
            }
        }
        if let Some(node_id) = self.target_node_script_node_id() {
            if let Some(entries) = self.node_script_shortcuts.get(&node_id) {
                script_favs.extend(entries.iter().cloned());
            }
        }
        let (fuzzy_active, fuzzy_query, fuzzy_cursor) = {
            let p = self.active_pane();
            (
                p.table.fuzzy_active,
                p.table.fuzzy_query.clone(),
                p.table.fuzzy_cursor,
            )
        };
        let search_state = self.active_pane().search.state();
        let cmdline_state = self.cmdline.state();
        let (chrome_prefix, chrome_placeholder, chrome_local) =
            match &self.active_pane().search_mode {
                SearchMode::Local => (None, None, true),
                SearchMode::Adapter { prompt, .. } => {
                    let placeholder = prompt
                        .clone()
                        .unwrap_or_else(|| "free-text search…".to_string());
                    (Some("? ".to_string()), Some(placeholder), false)
                }
                SearchMode::TreeFind { prompt } => {
                    let placeholder = prompt.clone().unwrap_or_else(|| "tree search…".to_string());
                    (Some("? ".to_string()), Some(placeholder), false)
                }
            };

        let mode_label = self.action_bar_mode_label();
        self.action_bar.set_hints(hints);
        self.action_bar.set_mode_label(mode_label);
        self.action_bar.set_active_filter_name(active_filter_name);
        self.action_bar.set_favorites(favs);
        self.action_bar.set_script_favorites(script_favs);
        self.action_bar
            .set_fuzzy(fuzzy_active, &fuzzy_query, fuzzy_cursor);
        self.action_bar.set_search(
            search_state.active,
            &search_state.query,
            search_state.cursor,
            search_state.current,
            search_state.match_count,
        );
        self.action_bar.set_cmdline(
            cmdline_state.active,
            &cmdline_state.query,
            cmdline_state.cursor,
        );
        self.action_bar
            .set_search_chrome(chrome_prefix, chrome_placeholder, chrome_local);
    }

    pub fn action_bar_height(&self, width: u16) -> u16 {
        self.action_bar.required_height(width)
    }

    pub fn render_action_bar(&mut self, frame: &mut Frame, area: Rect) {
        self.action_bar.view(frame, area);
    }

    /// Refresh action bar hints from the active pane's nav-level config.
    fn sync_action_bar_hints(&mut self) {
        self.action_bar.set_hints(self.action_bar_hints());
    }

    pub fn active_view_def(&self) -> Option<&ViewDef> {
        self.view_defs.get(self.active_subtab)
    }

    pub fn active_view_index(&self) -> usize {
        self.active_subtab
    }

    /// True when the active view renders as a tree (has a `tree_label`),
    /// i.e. the lazy expand-to-hit `:tree-find` walk is meaningful here.
    pub fn active_view_is_tree(&self) -> bool {
        self.active_view_def()
            .map(|vd| vd.tree_label.is_some())
            .unwrap_or(false)
    }

    /// Switch the active subtab by view name (case-insensitive). Returns
    /// `Ok(())` on success; `Err(available)` lists view names when the
    /// requested name doesn't exist. Drives the `<view>` half of
    /// `:focus-node <tab>:<view>` and is intentionally a no-op when the
    /// requested view is already active.
    pub fn switch_to_view_by_name(&mut self, name: &str) -> Result<bool, Vec<String>> {
        let idx = self
            .view_defs
            .iter()
            .position(|vd| vd.name.eq_ignore_ascii_case(name));
        match idx {
            Some(i) => Ok(self.switch_to_view(i)),
            None => Err(self.view_defs.iter().map(|vd| vd.name.clone()).collect()),
        }
    }

    /// Walk the active pane's items with a parsed [`focus_node::FocusSegment`]
    /// sequence (currently single-segment only — see
    /// [`focus_node::FocusError::MultiSegmentUnsupported`]). Parks the
    /// table cursor on the matched row.
    pub fn focus_node_in_active_pane(
        &mut self,
        segments: &[crate::views::focus_node::FocusSegment],
    ) -> Result<(), crate::views::focus_node::FocusError> {
        let pane = self.active_pane();
        let item_id = crate::views::focus_node::focus_in_flat_items(&pane.items, segments)?;
        self.active_pane_mut().focus_item_by_id(&item_id);
        Ok(())
    }

    /// True when the level whose items the active pane currently shows
    /// declares `node_scripts: true` — i.e. those rows own per-node
    /// scripts. Used to decide whether `q` (scripts menu) and `Q` (SQL
    /// editor) should be claimed.
    fn displays_node_scripts(&self) -> bool {
        match self.active_pane().active_child.as_ref() {
            Some(child) => child.node_scripts,
            None => self
                .active_view_def()
                .map(|vd| vd.node_scripts)
                .unwrap_or(false),
        }
    }

    /// True when the level *one step up* declares `node_scripts: true` —
    /// we drilled out of a script-owning row (e.g. into a table's rows),
    /// so `parent_node_id()` addresses the script owner. Reads the
    /// stashed `active_child` of the top nav frame, falling back to the
    /// root ViewDef for a frame pushed from the root level.
    fn parent_displays_node_scripts(&self) -> bool {
        let Some(frame) = self.active_pane().nav_stack.last() else {
            return false;
        };
        match frame.active_child.as_ref() {
            Some(child) => child.node_scripts,
            None => self
                .active_view_def()
                .map(|vd| vd.node_scripts)
                .unwrap_or(false),
        }
    }

    /// Node id that `q` (scripts menu) and `Q` (SQL editor) act on in the
    /// active pane. `selected_item_id()` when the displayed rows own the
    /// scripts themselves; `parent_node_id()` when we've drilled one level
    /// deeper (e.g. from a table into its rows). `None` when neither level
    /// declares `node_scripts: true`.
    pub fn target_node_script_node_id(&self) -> Option<String> {
        let pane = self.active_pane();
        if self.displays_node_scripts() {
            pane.selected_item_id().map(str::to_string)
        } else if self.parent_displays_node_scripts() {
            pane.parent_node_id().map(str::to_string)
        } else {
            None
        }
    }

    /// Subtab labels in `view_defs` order, paired with whether each is the
    /// currently-active subtab.
    pub fn subtab_labels(&self) -> Vec<(String, bool)> {
        let active = self.active_subtab;
        self.view_defs
            .iter()
            .enumerate()
            .map(|(i, vd)| {
                let label = match &vd.key {
                    Some(k) if !k.0.is_empty() => {
                        format!("{} {}", vd.name, k.0.join("/"))
                    }
                    _ => vd.name.clone(),
                };
                (label, i == active)
            })
            .collect()
    }

    /// Switch the active subtab. Returns whether a load is needed for
    /// the focused pane of the destination tree (true when that pane has
    /// never been populated).
    fn switch_to_view(&mut self, target: usize) -> bool {
        if target >= self.pane_trees.len() || target == self.active_subtab {
            return false;
        }
        self.active_subtab = target;
        self.sync_action_bar_hints();
        !self.pane_trees[target].focused_leaf().pane.loaded
    }

    /// Invalidate every *other* subtab's focused pane after a mutation in
    /// the active subtab, so switching to a sibling re-loads it instead of
    /// showing a stale snapshot. The subtabs of one tab share a single
    /// adapter instance (and its stores), so a change made in one — e.g.
    /// bookmarking an issue in the tickets subtab — can affect what a
    /// sibling lists (the bookmarks subtab). Marking the pane unloaded is
    /// cheap; the reload is deferred until the user actually switches there
    /// (and, with `connected_once`, happens transparently).
    pub fn invalidate_sibling_subtabs(&mut self) {
        let active = self.active_subtab;
        for (i, tree) in self.pane_trees.iter_mut().enumerate() {
            if i == active {
                continue;
            }
            tree.focused_leaf_mut().pane.loaded = false;
        }
    }

    /// Allocate a fresh [`PaneId`].
    fn alloc_pane_id(&mut self) -> PaneId {
        let id = self.next_pane_id;
        self.next_pane_id = self.next_pane_id.wrapping_add(1);
        id
    }

    /// Whether the active pane is currently consuming keystrokes as
    /// text input (fuzzy filter or `/`-search). Outside callers
    /// (App-level chord/chain interceptor) check this to avoid
    /// shadowing characters that belong in the input buffer.
    pub fn is_text_input_active(&self) -> bool {
        self.active_pane().table.fuzzy_active || self.active_pane().search.active()
    }

    /// Window-leader chord (configurable, default `w`). Consumes the
    /// leader on first press and the action key on second press.
    /// Returns `None` to fall through (no chord in progress and `key`
    /// isn't a leader, or the active pane is in a text-input mode and
    /// must see the key untouched).
    fn handle_window_chord(&mut self, key: &str) -> Option<SubViewMessage> {
        // Window/split operations are opt-in per view (`window_ops: true`).
        // On any view that doesn't enable them the `w` leader must never
        // engage, so the key falls through to normal handling (subtab
        // switch, node shortcut, …) exactly like any other letter.
        if !self
            .active_view_def()
            .map(|v| v.window_ops)
            .unwrap_or(false)
        {
            return None;
        }
        // Don't intercept while the user is typing into a text input —
        // matters for short leaders like `w` that double as ordinary
        // letters in search / cmdline / fuzzy buffers.
        if self.cmdline.active()
            || self.active_pane().search.active()
            || self.active_pane().table.fuzzy_active
        {
            return None;
        }
        if let Some(leader) = self.window_pending.take() {
            // Compose the full chord and look up against the static
            // WindowAction bindings first.
            for (action, binding) in &self.window_kb.bindings {
                if binding.matches_chord(&leader, key) {
                    return Some(self.execute_window_action(action.clone()));
                }
            }
            // Not a static action — try interpreting `key` as a pane tag
            // letter and switch focus to that pane.
            if let Some(letter) = key.chars().next().filter(|_| key.chars().count() == 1) {
                if self.try_switch_pane_by_tag(letter) {
                    return Some(SubViewMessage::SelectionChanged(None));
                }
            }
            // Unmapped resolution key cancels the chord with no effect.
            return Some(SubViewMessage::SelectionChanged(None));
        }
        // Are any window bindings prefixed with this key?
        let is_prefix = self.window_kb.bindings.values().any(|b| b.is_prefix(key));
        if is_prefix {
            self.window_pending = Some(key.to_string());
            return Some(SubViewMessage::SelectionChanged(None));
        }
        None
    }

    /// Switch focus inside the active subtab tree to the pane wearing
    /// `letter`. Returns `false` if no pane in this tree carries that
    /// tag — caller should then treat the key as unhandled.
    fn try_switch_pane_by_tag(&mut self, letter: char) -> bool {
        let tree = &mut self.pane_trees[self.active_subtab];
        let Some(pane_id) = tree.pane_id_for_tag(letter) else {
            return false;
        };
        if pane_id == tree.focus {
            return true;
        }
        tree.focus = pane_id;
        self.sync_action_bar_hints();
        true
    }

    /// Phase-2 entry point: dispatch a [`WindowAction`] requested by an
    /// action chain. Re-uses the same code path as the chord handler.
    pub fn dispatch_window_action(&mut self, action: WindowAction) -> SubViewMessage {
        self.execute_window_action(action)
    }

    /// Local action-chain scope stack of the focused pane, innermost
    /// first. App appends the global map to this list and feeds it to
    /// [`crate::action::resolve_chain_in_scopes`] so the "child wins
    /// over view wins over global" rule lives in a single place. In
    /// tree mode the innermost scope is the cursor-depth `ChildDef`'s
    /// `action_chains` (depth 0 has no ChildDef so we go straight to
    /// the view).
    pub fn action_chain_scopes(&self) -> Vec<&crate::action::ActionChains> {
        let mut out = Vec::new();
        let pane = self.active_pane();
        if pane.tree.is_some() {
            if let Some(child) = pane.tree_active_child_def(&self.view_defs) {
                out.push(&child.action_chains);
            }
        } else if let Some(child) = pane.active_child.as_ref() {
            out.push(&child.action_chains);
        }
        if let Some(view) = self.view_defs.get(self.active_subtab) {
            out.push(&view.action_chains);
        }
        out
    }

    /// Phase-2 entry point: dispatch a [`ContentAction`] requested by an
    /// action chain. Routes through the active pane's `try_*` helpers and
    /// post-processes a `ContentDrill` exactly like the key path so the
    /// split / coupled-replace logic is shared.
    pub fn dispatch_content_action(&mut self, action: ContentAction) -> SubViewMessage {
        let view_index = self.view_index;
        let pane_id = self.active_pane_id();
        let view_defs = self.view_defs.clone();
        let msg = match action {
            ContentAction::Open => self.active_pane().try_drill_open(&view_defs),
            ContentAction::Back => self.active_pane_mut().try_back(&view_defs),
            ContentAction::NextPage => self.active_pane_mut().try_next_page(view_index, pane_id),
            ContentAction::PrevPage => self.active_pane_mut().try_prev_page(view_index, pane_id),
            ContentAction::EditQuery => SubViewMessage::Unhandled,
            ContentAction::OpenScriptsMenu => SubViewMessage::Unhandled,
            ContentAction::TreeCollapse => self
                .active_pane_mut()
                .try_tree_smart_collapse(&view_defs)
                .unwrap_or(SubViewMessage::Unhandled),
            ContentAction::TreeCollapseAll => self
                .active_pane_mut()
                .try_tree_collapse_all(&view_defs)
                .unwrap_or(SubViewMessage::Unhandled),
            ContentAction::TreeExpandAll => self
                .active_pane_mut()
                .try_tree_expand_all(view_index, pane_id, &view_defs)
                .unwrap_or(SubViewMessage::Unhandled),
            ContentAction::CycleGrouping => self
                .active_pane_mut()
                .try_cycle_grouping(&view_defs, view_index, pane_id),
            ContentAction::ToggleGroupOrder => self
                .active_pane_mut()
                .try_toggle_group_order(&view_defs, view_index, pane_id),
            ContentAction::GroupMenu => {
                if self.active_pane().level_has_group_by(&self.view_defs) {
                    self.open_group_menu();
                    SubViewMessage::SelectionChanged(None)
                } else {
                    SubViewMessage::Unhandled
                }
            }
            ContentAction::ToggleTreeAggregate => {
                self.active_pane_mut().try_toggle_tree_aggregate(&view_defs)
            }
            ContentAction::JumpMode => {
                let nav_chars = self.nav_chars.clone();
                let pane = self.active_pane_mut();
                pane.table.set_nav_chars(&nav_chars);
                pane.table.jump_mode_open();
                SubViewMessage::SelectionChanged(None)
            }
            ContentAction::LinkHop => {
                // Label every link visible in the focused pane; the App-level
                // input intercept resolves a picked label to its URL and opens
                // it. See `ContentPane::open_link_hop` / `link_extract`.
                let nav_chars = self.nav_chars.clone();
                let pane = self.active_pane_mut();
                pane.table.set_nav_chars(&nav_chars);
                pane.open_link_hop()
            }
            ContentAction::ToggleRecordDetail => self.toggle_record_detail(),
            ContentAction::ToggleDetailWrap => self.toggle_detail_wrap(),
            ContentAction::ToggleLongText => self
                .active_pane_mut()
                .try_toggle_long_text(&view_defs, view_index, pane_id),
            ContentAction::ToggleCardMode => self.toggle_card_mode(),
        };
        if let SubViewMessage::ContentDrill {
            item_id,
            item_label,
            child_def,
        } = msg
        {
            return self.dispatch_content_drill(item_id, item_label, *child_def);
        }
        msg
    }

    fn execute_window_action(&mut self, action: WindowAction) -> SubViewMessage {
        match action {
            WindowAction::SplitRight => self.split_focused(SplitOrientation::Horizontal),
            WindowAction::SplitDown => self.split_focused(SplitOrientation::Vertical),
            WindowAction::Close => self.close_focused(),
            WindowAction::FocusParent => self.focus_parent_pane(),
            WindowAction::FocusChild => self.focus_child_pane(),
        }
    }

    /// Move focus to the pane that owns the focused pane via `linked_child`
    /// (the source side of a coupled split). When no parent backlink exists,
    /// fall back to the structural sibling — the other leaf produced by the
    /// most recent ancestor split — so plain (non-coupled) splits still get
    /// a useful "back to neighbour" move. Returns the standard
    /// `SelectionChanged(None)` so the App refreshes hints/breadcrumbs.
    fn focus_parent_pane(&mut self) -> SubViewMessage {
        let active_subtab = self.active_subtab;
        let focus_id = self.pane_trees[active_subtab].focus;
        let parent_via_backlink = {
            let mut leaf_ids = Vec::new();
            self.pane_trees[active_subtab]
                .root
                .collect_leaf_ids(&mut leaf_ids);
            leaf_ids.into_iter().find(|&id| {
                self.find_pane(id)
                    .and_then(|p| p.linked_child.as_ref().map(|(_, child)| *child == focus_id))
                    .unwrap_or(false)
            })
        };
        let target =
            parent_via_backlink.or_else(|| self.pane_trees[active_subtab].sibling_of(focus_id));
        if let Some(t) = target {
            self.pane_trees[active_subtab].focus = t;
            self.sync_action_bar_hints();
        }
        SubViewMessage::SelectionChanged(None)
    }

    /// Move focus to the pane this one opened — preferring the coupled
    /// `linked_child` when present, falling back to the structural sibling.
    /// Symmetric to `focus_parent_pane`; mainly useful as a chain tail to
    /// return to the just-replaced child after a `content.open`.
    fn focus_child_pane(&mut self) -> SubViewMessage {
        let active_subtab = self.active_subtab;
        let focus_id = self.pane_trees[active_subtab].focus;
        let child_via_backlink = self
            .find_pane(focus_id)
            .and_then(|p| p.linked_child.as_ref().map(|(_, child)| *child));
        let target =
            child_via_backlink.or_else(|| self.pane_trees[active_subtab].sibling_of(focus_id));
        if let Some(t) = target {
            self.pane_trees[active_subtab].focus = t;
            self.sync_action_bar_hints();
        }
        SubViewMessage::SelectionChanged(None)
    }

    /// Split the focused pane along `orientation`. The new pane is empty
    /// but inherits sort / page / active query from the source so its
    /// initial fetch matches.
    fn split_focused(&mut self, orientation: SplitOrientation) -> SubViewMessage {
        let new_pane_id = self.alloc_pane_id();
        let view_def_index = self.active_subtab;
        let tree_enabled = self.view_defs[view_def_index].tree_label.is_some();
        let new_pane = {
            let theme = Arc::clone(&self.theme);
            let source = self.active_pane();
            let mut p = ContentPane::new(
                theme,
                view_def_index,
                tree_enabled,
                source.capabilities.clone(),
            );
            p.active_query = source.active_query.clone();
            p.active_query_name = source.active_query_name.clone();
            p.active_query_vars = source.active_query_vars.clone();
            p.active_query_kind = source.active_query_kind;
            p.text_search_query = source.text_search_query.clone();
            p.current_sort = source.current_sort.clone();
            p.current_page = source.current_page;
            p.set_column_overrides(self.column_overrides.clone());
            p.set_card_mode_overrides(self.card_mode_overrides.clone());
            p
        };
        let tree = &mut self.pane_trees[self.active_subtab];
        tree.split_focus(orientation, 0.5, SplitSide::Second, new_pane_id, new_pane);
        tree.assign_tag(new_pane_id, &self.pane_tag_alphabet);
        self.sync_action_bar_hints();
        SubViewMessage::Request(ViewRequest::SpawnContentLoad {
            view_index: self.view_index,
            pane_id: new_pane_id,
        })
    }

    /// Resolve a `ContentDrill` message produced by the focused pane:
    /// either drill in-place (today's behavior) or open the child level
    /// in a split pane next to the source, depending on `child_def.split`.
    /// In both cases, returns a `ViewRequest::DrillDown` for the App to
    /// dispatch to the adapter — the difference is which pane id the
    /// async response routes back to.
    fn dispatch_content_drill(
        &mut self,
        item_id: String,
        item_label: String,
        child_def: ChildDef,
    ) -> SubViewMessage {
        let view_index = self.view_index;
        match child_def.split.clone() {
            None => {
                // In-place drill — mutate the focused pane.
                let view_defs = self.view_defs.clone();
                let pane_id = self.active_pane_id();
                let child_node_type = self.active_pane_mut().drill_down_prepare(
                    &item_id,
                    &item_label,
                    &child_def,
                    &view_defs,
                );
                SubViewMessage::Request(ViewRequest::DrillDown {
                    view_index,
                    pane_id,
                    node_id: item_id,
                    node_label: item_label,
                    child_node_type,
                })
            }
            Some(split_def) => {
                // Coupled hot-replace path: if the source pane already owns
                // a linked child for this ChildDef and the child is still
                // in the tree, re-drill the existing child in place
                // instead of opening another split. Focus stays on the
                // source so chains like `[list_next, open]` keep firing
                // from the parent.
                if split_def.coupled {
                    let source_id = self.active_pane_id();
                    let linked = self
                        .find_pane(source_id)
                        .and_then(|p| p.linked_child.clone())
                        .filter(|(name, child_id)| {
                            name == &child_def.name && self.find_pane(*child_id).is_some()
                        });
                    if let Some((_, child_pane_id)) = linked {
                        let view_defs = self.view_defs.clone();
                        let (source_items, source_selected_row, source_active_child) = {
                            let s = self.find_pane(source_id).expect("source alive");
                            (
                                s.items.clone(),
                                s.table.selected_row(),
                                s.active_child.clone(),
                            )
                        };
                        let child_node_type = self
                            .find_pane_mut(child_pane_id)
                            .expect("alive checked above")
                            .coupled_replace_with_source(
                                source_items,
                                source_selected_row,
                                source_active_child,
                                &item_id,
                                &item_label,
                                &child_def,
                                &view_defs,
                            );
                        return SubViewMessage::Request(ViewRequest::DrillDown {
                            view_index,
                            pane_id: child_pane_id,
                            node_id: item_id,
                            node_label: item_label,
                            child_node_type,
                        });
                    }
                    // Linked child stale or never existed — fall through
                    // to fresh spawn below and set the backlink afterwards.
                }

                // Split-drill — allocate a new pane next to the focused one.
                // Map split direction to (orientation, side):
                //   Right  → Horizontal, new pane second
                //   Left   → Horizontal, new pane first
                //   Bottom → Vertical,   new pane second
                //   Top    → Vertical,   new pane first
                let (orientation, side) = match split_def.direction {
                    SplitDirection::Right => (SplitOrientation::Horizontal, SplitSide::Second),
                    SplitDirection::Left => (SplitOrientation::Horizontal, SplitSide::First),
                    SplitDirection::Bottom => (SplitOrientation::Vertical, SplitSide::Second),
                    SplitDirection::Top => (SplitOrientation::Vertical, SplitSide::First),
                };
                // `split_def.ratio` is the share of the **new** pane.
                // `Branch.ratio` is the share of `first`. Convert based on side.
                let branch_ratio = match side {
                    SplitSide::Second => 1.0 - split_def.ratio,
                    SplitSide::First => split_def.ratio,
                };

                let new_pane_id = self.alloc_pane_id();
                let view_def_index = self.active_subtab;
                let theme = Arc::clone(&self.theme);
                let view_defs = self.view_defs.clone();
                let source_id = self.active_pane_id();
                let coupled = split_def.coupled;
                // Tree mode in the new pane depends on the *target* child,
                // not the source view: drilling out of a tree into a leaf
                // child (no tree_label) yields a flat pane.
                let tree_enabled = child_def.tree_label.is_some();

                let mut new_pane = {
                    let source = self.active_pane();
                    let mut p = ContentPane::new(
                        theme,
                        view_def_index,
                        tree_enabled,
                        source.capabilities.clone(),
                    );
                    // Inherit query/sort/page so any child-level fetch params
                    // line up with what the source had.
                    p.active_query = source.active_query.clone();
                    p.active_query_name = source.active_query_name.clone();
                    p.active_query_vars = source.active_query_vars.clone();
                    p.active_query_kind = source.active_query_kind;
                    p.text_search_query = source.text_search_query.clone();
                    p.current_sort = source.current_sort.clone();
                    p.current_page = source.current_page;
                    // Mirror the source's level + items so back-nav from
                    // inside the new pane returns to the parent listing.
                    p.active_child = source.active_child.clone();
                    p.items = source.items.clone();
                    p.set_column_overrides(self.column_overrides.clone());
                    p.set_card_mode_overrides(self.card_mode_overrides.clone());
                    p
                };
                let child_node_type =
                    new_pane.drill_down_prepare(&item_id, &item_label, &child_def, &view_defs);

                let tree = &mut self.pane_trees[self.active_subtab];
                tree.split_focus(orientation, branch_ratio, side, new_pane_id, new_pane);
                tree.assign_tag(new_pane_id, &self.pane_tag_alphabet);
                if coupled {
                    if let Some(source) = self.find_pane_mut(source_id) {
                        source.linked_child = Some((child_def.name.clone(), new_pane_id));
                    }
                }
                self.sync_action_bar_hints();

                SubViewMessage::Request(ViewRequest::DrillDown {
                    view_index,
                    pane_id: new_pane_id,
                    node_id: item_id,
                    node_label: item_label,
                    child_node_type,
                })
            }
        }
    }

    /// Mirror Enter-on-row for a node-script run: figure out the right
    /// pane (split-allocated result child, or in-place if we're already
    /// inside one) and emit `RunNodeScript` against it. Without this, the
    /// script result would land in the source pane (e.g. the flat tables
    /// list) whose columns can't display dynamic `qrow:*` items, leaving
    /// an empty-looking pane.
    ///
    /// The breadcrumb label is the node id's last path segment — a table
    /// name for Postgres, and the readable tail for any other adapter
    /// whose ids are `/`-joined paths. Ids without a `/` label as-is.
    fn dispatch_node_script_apply(&mut self, node_id: String, script: String) -> SubViewMessage {
        let view_index = self.view_index;
        let view_defs = self.view_defs.clone();
        let label = node_id.rsplit('/').next().unwrap_or(&node_id).to_string();
        let target_pane_id = self.split_for_query_into_child(&node_id, &label, &view_defs);
        SubViewMessage::Request(ViewRequest::RunNodeScript {
            view_index,
            pane_id: target_pane_id,
            node_id,
            script,
        })
    }

    /// Public wrapper around [`Self::split_for_query_into_child`] for the
    /// App-side `RunAdapterDbScript` dispatcher (CP-8). The dispatcher
    /// supplies the source script's `node_id` + label; the helper
    /// allocates / reuses the result-pane child per the active level's
    /// first `ChildDef`, and returns its `PaneId`.
    pub fn open_db_script_result_pane(
        &mut self,
        source_node_id: &str,
        source_label: &str,
    ) -> PaneId {
        let view_defs = self.view_defs.clone();
        self.split_for_query_into_child(source_node_id, source_label, &view_defs)
    }

    /// Shared split-and-prepare helper for "run a custom query against
    /// the postgres:table the user is looking at". Picks the first
    /// child-def of the active level — typically the `Rows` child with
    /// `split: right`. When split-config is present, allocates a new
    /// pane and drills it into the Rows level. When empty (already
    /// inside a Rows pane) or no split, returns the active pane id so
    /// the result replaces items in place.
    fn split_for_query_into_child(
        &mut self,
        node_id: &str,
        node_label: &str,
        view_defs: &[ViewDef],
    ) -> PaneId {
        let children = self.active_pane().current_children(view_defs).to_vec();
        let Some(child_def) = children.into_iter().next() else {
            return self.active_pane_id();
        };
        let Some(split_def) = child_def.split.clone() else {
            self.active_pane_mut()
                .drill_down_prepare(node_id, node_label, &child_def, view_defs);
            return self.active_pane_id();
        };
        // Reuse an existing coupled child pane if one is alive for the
        // same child name — matches Enter-on-table's hot-replace path so
        // repeated `q`→Apply doesn't stack new splits.
        if split_def.coupled {
            let source_id = self.active_pane_id();
            let linked = self
                .find_pane(source_id)
                .and_then(|p| p.linked_child.clone())
                .filter(|(name, child_id)| {
                    name == &child_def.name && self.find_pane(*child_id).is_some()
                });
            if let Some((_, child_pane_id)) = linked {
                if let Some(child) = self.find_pane_mut(child_pane_id) {
                    child.drill_down_prepare(node_id, node_label, &child_def, view_defs);
                }
                return child_pane_id;
            }
        }
        let (orientation, side) = match split_def.direction {
            SplitDirection::Right => (SplitOrientation::Horizontal, SplitSide::Second),
            SplitDirection::Left => (SplitOrientation::Horizontal, SplitSide::First),
            SplitDirection::Bottom => (SplitOrientation::Vertical, SplitSide::Second),
            SplitDirection::Top => (SplitOrientation::Vertical, SplitSide::First),
        };
        let branch_ratio = match side {
            SplitSide::Second => 1.0 - split_def.ratio,
            SplitSide::First => split_def.ratio,
        };
        let new_pane_id = self.alloc_pane_id();
        let view_def_index = self.active_subtab;
        let theme = Arc::clone(&self.theme);
        let source_id = self.active_pane_id();
        let coupled = split_def.coupled;
        // The new pane represents the leaf level — its tree state must
        // come from the target child, not the source ViewDef.
        let tree_enabled = child_def.tree_label.is_some();
        let mut new_pane = {
            let source = self.active_pane();
            let mut p = ContentPane::new(
                theme,
                view_def_index,
                tree_enabled,
                source.capabilities.clone(),
            );
            p.active_query = source.active_query.clone();
            p.active_query_name = source.active_query_name.clone();
            p.active_query_vars = source.active_query_vars.clone();
            p.active_query_kind = source.active_query_kind;
            p.text_search_query = source.text_search_query.clone();
            p.current_sort = source.current_sort.clone();
            p.current_page = source.current_page;
            p.active_child = source.active_child.clone();
            p.items = source.items.clone();
            p.set_column_overrides(self.column_overrides.clone());
            p.set_card_mode_overrides(self.card_mode_overrides.clone());
            p
        };
        new_pane.drill_down_prepare(node_id, node_label, &child_def, view_defs);
        let tree = &mut self.pane_trees[self.active_subtab];
        tree.split_focus(orientation, branch_ratio, side, new_pane_id, new_pane);
        tree.assign_tag(new_pane_id, &self.pane_tag_alphabet);
        if coupled {
            if let Some(source) = self.find_pane_mut(source_id) {
                source.linked_child = Some((child_def.name.clone(), new_pane_id));
            }
        }
        self.sync_action_bar_hints();
        new_pane_id
    }

    /// Close the focused pane. Cascades through any coupled
    /// `linked_child` chain so closing a parent also closes the child it
    /// owns. Refuses the close if the cascade would empty the tree (the
    /// user must close one of the linked panes manually first). After
    /// the cascade, surviving panes that referenced any closed pane
    /// have their `linked_child` backlink cleared.
    fn close_focused(&mut self) -> SubViewMessage {
        let active_subtab = self.active_subtab;
        let focus_id = self.pane_trees[active_subtab].focus;

        // Build the cascade set in parent → child order, following BOTH the
        // coupled-drill `linked_child` and the record-detail `detail_child`
        // backlinks so closing a source also closes whatever it spawned.
        // The visited check stops dead/cyclic references from trapping us;
        // `focus_id` stays first so the close-children-first pass (rev) still
        // tears followers down before their parent.
        let mut chain: Vec<PaneId> = Vec::new();
        let mut frontier: Vec<PaneId> = vec![focus_id];
        while let Some(cur) = frontier.pop() {
            if chain.contains(&cur) {
                continue;
            }
            chain.push(cur);
            if let Some(p) = self.find_pane(cur) {
                if let Some((_, child)) = p.linked_child.as_ref() {
                    if self.find_pane(*child).is_some() {
                        frontier.push(*child);
                    }
                }
                if let Some(child) = p.detail_child {
                    if self.find_pane(child).is_some() {
                        frontier.push(child);
                    }
                }
            }
        }

        let tree_leaf_count = self.pane_trees[active_subtab].leaf_count();
        if chain.len() >= tree_leaf_count {
            // Cascade would empty the tree — refuse.
            return SubViewMessage::SelectionChanged(None);
        }

        // Harvest cursor ids of every pane about to be destroyed so the
        // App can tear them down on the adapter (CP-6). Must run BEFORE
        // `close_specific` since the pane state vanishes with the leaf.
        for &id in &chain {
            if let Some(pane) = self.find_pane(id) {
                if let Some(cq) = pane.active_custom_query.as_ref() {
                    if let Some(cursor_id) = cq.cursor_id.clone() {
                        self.pending_cursor_closes.push(cursor_id);
                    }
                }
            }
        }

        // Close children first, parent last so each `close_specific` call
        // sees a non-empty tree.
        for &id in chain.iter().rev() {
            let tree = &mut self.pane_trees[active_subtab];
            if tree.close_specific(id) {
                tree.release_tag(id);
            }
        }

        // Clear any stale backlink in surviving panes of the same tree.
        let closed: std::collections::HashSet<PaneId> = chain.iter().copied().collect();
        let mut surviving_ids = Vec::new();
        self.pane_trees[active_subtab]
            .root
            .collect_leaf_ids(&mut surviving_ids);
        for id in surviving_ids {
            if let Some(leaf) = self.pane_trees[active_subtab].root.find_leaf_mut(id) {
                let stale = leaf
                    .pane
                    .linked_child
                    .as_ref()
                    .map(|(_, child)| closed.contains(child))
                    .unwrap_or(false);
                if stale {
                    leaf.pane.linked_child = None;
                }
                // Same for the record-detail backlinks: a survivor that
                // pointed at (or followed) a just-closed pane must drop the
                // reference so a later toggle doesn't chase a dead pane.
                if leaf.pane.detail_child.is_some_and(|c| closed.contains(&c)) {
                    leaf.pane.detail_child = None;
                }
                if leaf.pane.detail_source.is_some_and(|s| closed.contains(&s)) {
                    leaf.pane.detail_source = None;
                }
            }
        }

        self.sync_action_bar_hints();
        SubViewMessage::SelectionChanged(None)
    }

    /// Toggle the record-detail follower for the focused pane (`o`).
    ///
    /// Opening: the focused pane must be a `record_detail`-enabled flat
    /// table. A follower is split off to its right (`Horizontal` /
    /// `SplitSide::Second`) and linked back through the dedicated
    /// `detail_child` / `detail_source` backlinks — kept entirely separate
    /// from the coupled-drill `linked_child` link so the two never
    /// interfere. Focus stays on the *source* so the user keeps navigating
    /// the table; the follower re-syncs every frame in [`sync_detail_panes`]
    /// and so never needs its own fetch (it is purely synthetic).
    ///
    /// Closing: pressed on the source (its `detail_child` is live) or from
    /// inside the follower (its `detail_source` is set), the follower leaf
    /// is torn down and the source's backlink cleared. Closing is scoped to
    /// the follower only — unlike [`Self::close_focused`], the source pane
    /// survives.
    fn toggle_record_detail(&mut self) -> SubViewMessage {
        let active_subtab = self.active_subtab;
        let focus_id = self.pane_trees[active_subtab].focus;

        // Already split? Resolve (source, follower) from either side and
        // close. A stale `detail_child` (child already gone) is ignored so
        // it falls through to a fresh open instead of a dead close.
        let existing = match self.find_pane(focus_id) {
            Some(p) if p.detail_child.is_some_and(|c| self.find_pane(c).is_some()) => {
                Some((focus_id, p.detail_child.unwrap()))
            }
            Some(p) => p.detail_source.map(|src| (src, focus_id)),
            None => None,
        };
        if let Some((source_id, follower_id)) = existing {
            return self.close_detail_follower(source_id, follower_id);
        }

        // Open — only from a record_detail-enabled flat source.
        if !self.active_pane().record_detail_enabled(&self.view_defs) {
            return SubViewMessage::Unhandled;
        }
        let new_pane_id = self.alloc_pane_id();
        let view_def_index = self.active_subtab;
        let follower = {
            let theme = Arc::clone(&self.theme);
            let source = self.active_pane();
            let mut p = ContentPane::new(theme, view_def_index, false, source.capabilities.clone());
            p.detail_source = Some(focus_id);
            p.detail_wrap = false;
            // Mirror the source's drill level so the follower resolves the
            // *same* actions (e.g. a `scope: table` script) and scripts dir
            // as the source row — without this `current_actions` falls back
            // to the root ViewDef's actions and a row-level `x` is missing
            // when the follower has focus. Rendering stays synthetic:
            // `is_detail_pane()` short-circuits columns/layout regardless.
            p.active_child = source.active_child.clone();
            // Synthetic content: never fetched, so mark it loaded up front
            // (an unloaded pane renders a "loading…" placeholder).
            p.loaded = true;
            p
        };
        let tree = &mut self.pane_trees[active_subtab];
        tree.split_focus(
            SplitOrientation::Horizontal,
            0.5,
            SplitSide::Second,
            new_pane_id,
            follower,
        );
        tree.assign_tag(new_pane_id, &self.pane_tag_alphabet);
        if let Some(source) = self.find_pane_mut(focus_id) {
            source.detail_child = Some(new_pane_id);
        }
        // `split_focus` moves focus to the new pane; keep it on the source
        // so the cursor the follower tracks stays under the user's hands.
        self.pane_trees[active_subtab].focus = focus_id;
        self.sync_action_bar_hints();
        SubViewMessage::SelectionChanged(None)
    }

    /// Tear down a record-detail follower leaf and clear the source's
    /// `detail_child` backlink, focusing the surviving source. Refuses if it
    /// would empty the tree. The follower carries no custom-query cursor
    /// (its rows are synthetic), so nothing needs harvesting.
    fn close_detail_follower(&mut self, source_id: PaneId, follower_id: PaneId) -> SubViewMessage {
        let active_subtab = self.active_subtab;
        if self.pane_trees[active_subtab].leaf_count() <= 1 {
            return SubViewMessage::SelectionChanged(None);
        }
        let tree = &mut self.pane_trees[active_subtab];
        if tree.close_specific(follower_id) {
            tree.release_tag(follower_id);
        }
        if let Some(src) = self.find_pane_mut(source_id) {
            src.detail_child = None;
        }
        if self.find_pane(source_id).is_some() {
            self.pane_trees[active_subtab].focus = source_id;
        }
        self.sync_action_bar_hints();
        SubViewMessage::SelectionChanged(None)
    }

    /// Toggle value wrapping in the focused record-detail follower (`X`).
    /// Resolves the follower from the focused pane (the follower itself, or
    /// the focused source's live `detail_child`), flips its `detail_wrap`,
    /// and clears its cached `detail_summary` so the next
    /// [`sync_detail_panes`] re-transposes the record at the new wrap mode.
    fn toggle_detail_wrap(&mut self) -> SubViewMessage {
        let active_subtab = self.active_subtab;
        let focus_id = self.pane_trees[active_subtab].focus;
        let follower_id = match self.find_pane(focus_id) {
            Some(p) if p.is_detail_pane() => Some(focus_id),
            Some(p) => p.detail_child.filter(|c| self.find_pane(*c).is_some()),
            None => None,
        };
        let Some(follower_id) = follower_id else {
            return SubViewMessage::Unhandled;
        };
        if let Some(f) = self.find_pane_mut(follower_id) {
            f.detail_wrap = !f.detail_wrap;
            f.detail_summary = None;
        }
        SubViewMessage::SelectionChanged(None)
    }

    /// Re-seed every record-detail follower in the active subtab from its
    /// source pane's current selection, then rebuild it. This is the live
    /// coupling: moving the source cursor changes its selected record, the
    /// diff below fires, and the follower repaints — no explicit wiring on
    /// the navigation path. Cheap on the common frame: with wrap off and an
    /// unchanged selection the follower is skipped entirely. With wrap on it
    /// always rebuilds so the value re-wraps once the post-draw pass learns
    /// the true render width. Called from [`Self::rebuild_table`], i.e. once
    /// per `sync_components`.
    fn sync_detail_panes(&mut self) {
        let active_subtab = self.active_subtab;
        let mut ids = Vec::new();
        self.pane_trees[active_subtab]
            .root
            .collect_leaf_ids(&mut ids);
        let pairs: Vec<(PaneId, PaneId)> = ids
            .into_iter()
            .filter_map(|id| {
                self.find_pane(id)
                    .and_then(|p| p.detail_source)
                    .map(|s| (id, s))
            })
            .collect();
        if pairs.is_empty() {
            return;
        }
        let view_defs = self.view_defs.clone();
        let overlay = self.header_overlay.clone();
        let now = chrono::Local::now();
        for (follower_id, source_id) in pairs {
            let current = self
                .find_pane(source_id)
                .and_then(|p| p.selected_item().cloned());
            // Mirror the source table's configured columns (selection, order,
            // labels, `source: label`) so the detail follower matches the row
            // view exactly. Postgres and other dynamic-schema views have no
            // configured columns; `current_columns` then auto-derives one per
            // record field, so the follower still shows the whole record as
            // before this change.
            let columns = self
                .find_pane(source_id)
                .map(|p| p.current_columns(&view_defs))
                .unwrap_or_default();
            let Some(follower) = self.find_pane_mut(follower_id) else {
                continue;
            };
            let unchanged = current == follower.detail_summary;
            if unchanged && !follower.detail_wrap {
                continue;
            }
            follower.detail_summary = current.clone();
            let wrap = follower.detail_wrap;
            follower.items = match current {
                Some(ref s) => {
                    // Same label + value resolution as the row view:
                    // `cell_content_for` applies `source: label` and typed
                    // (date/duration) formatting; the label falls back to the
                    // column key only when no YAML label is set.
                    let fields: Vec<content_detail::DetailField> = columns
                        .iter()
                        .map(|col| content_detail::DetailField {
                            label: col
                                .label
                                .clone()
                                .filter(|l| !l.is_empty())
                                .unwrap_or_else(|| col.key.clone()),
                            value: cell_content_for(s, col, now).text,
                        })
                        .collect();
                    let width = follower.table.last_render_width() as usize;
                    let value_width = content_detail::value_width(width, &fields);
                    content_detail::detail_items(&fields, wrap, value_width)
                }
                None => Vec::new(),
            };
            // The record under the cursor swapped wholesale; reset the
            // follower's own selection so it never points past the new rows.
            follower.table.set_selected(0);
            follower.loaded = true;
            follower.rebuild_table_with(&view_defs, &overlay);
        }
    }

    pub fn selected_item_id(&self) -> Option<&str> {
        self.active_pane().selected_item_id()
    }

    pub fn nav_depth(&self) -> usize {
        self.active_pane().nav_depth()
    }

    pub fn breadcrumbs(&self) -> Vec<&str> {
        self.active_pane().breadcrumbs()
    }

    pub fn parent_node_id(&self) -> Option<&str> {
        self.active_pane().parent_node_id()
    }

    pub fn current_child_node_type(&self) -> Option<&str> {
        self.active_pane().current_child_node_type()
    }

    pub fn set_preview_description(&mut self, key: &str, description: String) {
        self.active_pane_mut()
            .set_preview_description(key, description);
    }

    /// Returns the load parameters needed for the root-level adapter call.
    pub fn root_load_request(&self) -> Option<LoadRequest> {
        self.active_pane().root_load_request(&self.view_defs)
    }

    pub fn current_sort(&self) -> &[SortKey] {
        self.active_pane().current_sort()
    }

    pub fn set_current_sort(&mut self, sort: Vec<SortKey>) -> bool {
        self.active_pane_mut().set_current_sort(sort)
    }

    pub fn last_applied_sort(&self) -> &[SortKey] {
        self.active_pane().last_applied_sort()
    }

    pub fn set_current_page(&mut self, page: Option<PageRequest>) -> bool {
        self.active_pane_mut().set_current_page(page)
    }

    pub fn current_page(&self) -> Option<PageRequest> {
        self.active_pane().current_page()
    }

    pub fn last_page_info(&self) -> Option<PageInfo> {
        self.active_pane().last_page_info()
    }

    pub fn next_page_request(&self) -> Option<PageRequest> {
        self.active_pane().next_page_request()
    }

    pub fn prev_page_request(&self) -> Option<PageRequest> {
        self.active_pane().prev_page_request()
    }

    pub fn current_query_text(&self) -> String {
        self.active_pane().current_query_text(&self.view_defs)
    }

    pub fn default_query_text(&self) -> String {
        self.active_pane().default_query_text(&self.view_defs)
    }

    pub fn is_query_editable(&self) -> bool {
        self.active_pane().is_query_editable(&self.view_defs)
    }

    pub fn set_query(&mut self, query: String, name: Option<String>) {
        self.active_pane_mut().set_query(query, name);
    }

    /// [`set_query`] for a body whose store is known — what the query
    /// editor hands back, since it was opened for one kind or the other.
    /// Bindings are empty: an edited body is applied as written.
    pub fn set_query_of_kind(&mut self, query: String, name: Option<String>, kind: QueryKind) {
        self.active_pane_mut().set_query_of_kind(
            query,
            name,
            std::collections::HashMap::new(),
            kind,
        );
    }

    /// Stamp the tab's user-set default saved query onto the active pane
    /// (the default view, as the plain startup apply always did) *and*
    /// onto every pane whose view opts in via `query.inherit_default` —
    /// subtabs that are projections of the same rows, where the default
    /// filter should follow the user. Runs once at startup, before any
    /// pane has loaded.
    ///
    /// `kind` travels with the body because an extended document is a legal
    /// default: what distinguishes it from an adapter-native query is the
    /// store it came from, which nothing downstream can recover from the text.
    pub fn apply_default_query(&mut self, query: String, name: Option<String>, kind: QueryKind) {
        self.active_pane_mut().set_query_of_kind(
            query.clone(),
            name.clone(),
            std::collections::HashMap::new(),
            kind,
        );
        let view_defs = self.view_defs.clone();
        let active = self.active_pane_id();
        for tree in &mut self.pane_trees {
            tree.root.for_each_leaf_mut(&mut |leaf| {
                if leaf.id == active {
                    return; // already stamped above
                }
                let inherits = view_defs
                    .get(leaf.pane.view_def_index())
                    .and_then(|vd| vd.query.as_ref())
                    .map(|q| q.inherit_default)
                    .unwrap_or(false);
                if inherits {
                    leaf.pane.set_query_of_kind(
                        query.clone(),
                        name.clone(),
                        std::collections::HashMap::new(),
                        kind,
                    );
                }
            });
        }
    }

    pub fn set_query_with_vars(
        &mut self,
        query: String,
        name: Option<String>,
        vars: std::collections::HashMap<String, String>,
    ) {
        self.active_pane_mut()
            .set_query_with_vars(query, name, vars);
    }

    /// Pane-targeted variant of `set_query_with_vars`. The App-side
    /// `:query apply` dispatcher applies to whichever pane the saved
    /// query was bound to, which may not be the active one when
    /// switching tabs.
    pub fn set_query_for_pane_with_vars(
        &mut self,
        pane_id: PaneId,
        query: String,
        name: Option<String>,
        vars: std::collections::HashMap<String, String>,
        kind: QueryKind,
    ) {
        if let Some(pane) = self.find_pane_mut(pane_id) {
            pane.set_query_of_kind(query, name, vars, kind);
        }
    }

    /// The query-menu key from the *active* ViewDef. Pulled from config
    /// dynamically because each subtab can carry its own menu key.
    pub fn query_menu_key(&self) -> Option<&str> {
        self.active_view_def()
            .and_then(|vd| vd.query.as_ref())
            .and_then(|q| q.menu_key.as_deref())
    }

    // ── Query popup ─────────────────────────────────────────────────

    pub fn has_query_popup(&self) -> bool {
        self.query_menu.is_open()
    }

    /// Open the query menu popup with merged YAML + DB queries.
    pub fn open_query_popup(&mut self) {
        let entries: Vec<QueryMenuEntry> = self
            .db_saved_queries
            .iter()
            .map(|sq| QueryMenuEntry {
                name: sq.name.clone(),
                query: sq.query.clone(),
                shortcut: sq.shortcut.clone(),
                is_default: self.default_saved_query.as_deref() == Some(sq.name.as_str()),
            })
            .collect();
        self.query_menu_mode = QueryMenuMode::SavedQueries;
        self.query_menu.open(&entries, &self.query_menu_kb);
    }

    /// Open the per-node scripts menu for `node_id`. Called by the App
    /// after it has listed the node's scripts via the adapter's script
    /// store. The `query` field on each entry is unused for the script
    /// menu but kept non-empty so the popup widget treats the row as
    /// selectable.
    pub fn open_node_scripts_popup(&mut self, node_id: String, entries: Vec<QueryMenuEntry>) {
        self.query_menu_mode = QueryMenuMode::NodeScripts { node_id };
        // Scripts are files, not queries — no default-query semantics.
        self.query_menu
            .open_without_default(&entries, &self.query_menu_kb);
    }

    /// Handle key events when the query popup is open.
    pub fn handle_query_popup_key(&mut self, key: &str) -> Option<SubViewMessage> {
        if !self.query_menu.is_open() {
            return None;
        }
        let view_index = self.view_index;
        let pane_id = self.active_pane_id();
        let mode = self.query_menu_mode.clone();
        let msg = self.query_menu.handle_key(key, &self.query_menu_kb);
        let noop = Some(SubViewMessage::SelectionChanged(None));
        match (mode, msg) {
            // Closed / unhandled / handled — route shared regardless of mode.
            (_, QueryMenuMessage::Unhandled)
            | (_, QueryMenuMessage::Handled)
            | (_, QueryMenuMessage::Closed) => noop,

            // ── Saved queries (existing behaviour) ────────────────────
            (QueryMenuMode::SavedQueries, QueryMenuMessage::Apply { name, query }) => {
                let kind = self.query_kind_of(&name);
                Some(SubViewMessage::Request(
                    ViewRequest::ApplyContentSavedQuery {
                        view_index,
                        pane_id,
                        query,
                        name,
                        kind,
                    },
                ))
            }
            (QueryMenuMode::SavedQueries, QueryMenuMessage::EditExisting { name, query }) => {
                // Stamped, not applied: the editor opens on this body. The
                // kind still has to follow it, or a pane that last ran an
                // extended document would keep claiming so for a native body.
                let kind = self.query_kind_of(&name);
                let pane = self.active_pane_mut();
                pane.active_query = Some(query);
                pane.active_query_name = Some(name.clone());
                pane.active_query_kind = kind;
                Some(SubViewMessage::Request(
                    ViewRequest::OpenContentQueryEditor {
                        view_index,
                        pane_id,
                        save_name: Some(name),
                        is_new: false,
                        kind,
                    },
                ))
            }
            (QueryMenuMode::SavedQueries, QueryMenuMessage::Delete { name }) => {
                let scope = self.query_scope.clone();
                Some(SubViewMessage::Request(ViewRequest::DeleteContentQuery {
                    view_index,
                    scope,
                    name,
                }))
            }
            (QueryMenuMode::SavedQueries, QueryMenuMessage::EditShortcut { name, query }) => {
                let scope = self.query_scope.clone();
                Some(SubViewMessage::Request(
                    ViewRequest::PromptContentQueryShortcut {
                        view_index,
                        scope,
                        name,
                        query,
                    },
                ))
            }
            (QueryMenuMode::SavedQueries, QueryMenuMessage::ClearShortcut { name }) => {
                let scope = self.query_scope.clone();
                Some(SubViewMessage::Request(
                    ViewRequest::ClearContentQueryShortcut {
                        view_index,
                        scope,
                        name,
                    },
                ))
            }
            (QueryMenuMode::SavedQueries, QueryMenuMessage::CreateNew { name, kind }) => Some(
                SubViewMessage::Request(ViewRequest::OpenContentQueryEditor {
                    view_index,
                    pane_id,
                    save_name: Some(name),
                    is_new: true,
                    kind,
                }),
            ),
            (QueryMenuMode::SavedQueries, QueryMenuMessage::SetDefault { name }) => Some(
                SubViewMessage::Request(ViewRequest::SetDefaultContentQuery { view_index, name }),
            ),
            // Unreachable — the scripts popup opens via
            // `open_without_default`, which never emits SetDefault.
            (QueryMenuMode::NodeScripts { .. }, QueryMenuMessage::SetDefault { .. }) => noop,

            // ── Per-node scripts ──────────────────────────────────────
            (QueryMenuMode::NodeScripts { node_id }, QueryMenuMessage::Apply { name, .. }) => {
                Some(self.dispatch_node_script_apply(node_id, name))
            }
            (
                QueryMenuMode::NodeScripts { node_id },
                QueryMenuMessage::EditExisting { name, .. },
            ) => Some(SubViewMessage::Request(ViewRequest::EditNodeScript {
                view_index,
                pane_id,
                node_id,
                script: name,
                is_new: false,
            })),
            (QueryMenuMode::NodeScripts { node_id }, QueryMenuMessage::Delete { name }) => {
                Some(SubViewMessage::Request(ViewRequest::DeleteNodeScript {
                    view_index,
                    pane_id,
                    node_id,
                    script: name,
                }))
            }
            (
                QueryMenuMode::NodeScripts { node_id },
                QueryMenuMessage::EditShortcut { name, .. },
            ) => Some(SubViewMessage::Request(
                ViewRequest::PromptNodeScriptShortcut {
                    view_index,
                    pane_id,
                    node_id,
                    script: name,
                },
            )),
            (QueryMenuMode::NodeScripts { node_id }, QueryMenuMessage::ClearShortcut { name }) => {
                Some(SubViewMessage::Request(
                    ViewRequest::ClearNodeScriptShortcut {
                        view_index,
                        pane_id,
                        node_id,
                        script: name,
                    },
                ))
            }
            // Scripts are files in one store; the kind the menu offers for
            // query bodies means nothing here, but a typed `+` prefix is
            // still stripped, which is what the script menu does too.
            (QueryMenuMode::NodeScripts { node_id }, QueryMenuMessage::CreateNew { name, .. }) => {
                Some(SubViewMessage::Request(ViewRequest::EditNodeScript {
                    view_index,
                    pane_id,
                    node_id,
                    script: name,
                    is_new: true,
                }))
            }
        }
    }

    pub fn render_query_popup(&mut self, frame: &mut Frame, area: Rect) {
        self.query_menu.render(frame, area);
    }

    /// Open the group-by menu (M3, `content.group_menu`) over the active
    /// pane's current grouping state: the same five states the stepwise
    /// `cycle_grouping` walks, as a direct-jump hotkey popup (native
    /// Trackings `u` parity). The current state is marked; first-letter
    /// hotkeys (n/d/w/m/y) select immediately.
    fn open_group_menu(&mut self) {
        // `None` = ungrouped; `Some(b)` = grouped by bucket `b`. A plain
        // (bucket-less) column grouping reads as `Some(None)` and marks no
        // entry — selecting one re-buckets it, exactly like `cycle_grouping`.
        let current = self
            .active_pane()
            .current_group_by(&self.view_defs)
            .map(|gb| gb.bucket);
        let options: [(Option<DateBucket>, &str, &str); 5] = [
            (None, "No grouping", "n"),
            (Some(DateBucket::Day), "Day", "d"),
            (Some(DateBucket::Week), "Week", "w"),
            (Some(DateBucket::Month), "Month", "m"),
            (Some(DateBucket::Year), "Year", "y"),
        ];
        let entries: Vec<TabSetEntry> = options
            .into_iter()
            .map(|(bucket, label, key)| TabSetEntry {
                name: key.to_string(),
                label: label.to_string(),
                icon: None,
                shortcut: Some(key.to_string()),
                active: match bucket {
                    None => current.is_none(),
                    some_bucket => current == Some(some_bucket),
                },
            })
            .collect();
        self.group_menu.open(entries);
    }

    /// Key handler while the group-by menu is open (it intercepts every
    /// key). A selection jumps the active pane's runtime grouping to the
    /// chosen state and rebuilds the table — or reloads the level when the
    /// adapter owns the grouping (adapter-grouped tree), mirroring
    /// [`ContentPane::try_cycle_grouping`].
    fn handle_group_menu_key(&mut self, key: &str) -> SubViewMessage {
        match self.group_menu.handle_key(key) {
            TabSetPopupMessage::Switch(name) => {
                let bucket = match name.as_str() {
                    "d" => Some(DateBucket::Day),
                    "w" => Some(DateBucket::Week),
                    "m" => Some(DateBucket::Month),
                    "y" => Some(DateBucket::Year),
                    _ => None,
                };
                let view_index = self.view_index;
                let pane_id = self.active_pane_id();
                let view_defs = self.view_defs.clone();
                let pane = self.active_pane_mut();
                if pane.set_grouping_bucket(bucket, &view_defs) {
                    if pane.tree_groups_via_adapter() {
                        return pane.reload_current_level(view_index, pane_id);
                    }
                    pane.rebuild_table(&view_defs);
                }
                SubViewMessage::SelectionChanged(None)
            }
            TabSetPopupMessage::Unhandled => SubViewMessage::Unhandled,
            _ => SubViewMessage::SelectionChanged(None),
        }
    }

    /// Would binding `shortcut` to the saved query `query_name` collide
    /// with any other key handler in this tab (built-in, YAML, window
    /// chord, or another saved query)? Returns a human-readable label
    /// for the conflicting binding. Used as the set-time gate when the
    /// user assigns a shortcut via the query menu; the load-time
    /// counterpart runs in `App::reload_content_saved_queries` against
    /// the freshly-loaded batch.
    pub fn saved_query_shortcut_conflict(
        &self,
        kb: &KeyBindingConfig,
        query_name: &str,
        shortcut: &str,
    ) -> Option<String> {
        let bound: Vec<(String, String)> = self
            .db_saved_queries
            .iter()
            .filter_map(|sq| sq.shortcut.clone().map(|s| (sq.name.clone(), s)))
            .collect();
        crate::keymap::saved_query_shortcut_conflict(
            &self.tab_name,
            &self.view_defs,
            kb,
            query_name,
            shortcut,
            &bound,
        )
    }

    /// The focused level's `type: script` action, if any — returns its
    /// payload scope and the action's `default_field`. Drives both the
    /// script-shortcut scope and the App's run-context rebuild on dispatch.
    pub fn active_script_action(
        &self,
    ) -> Option<(crate::config::view_config::ScriptScope, Option<String>)> {
        self.active_pane()
            .current_actions(&self.view_defs)
            .into_iter()
            .find(|a| a.action_type == "script")
            .map(|a| (a.script_scope, a.script_default_field.clone()))
    }

    /// Script-shortcut scope for the focused level — `script:<tab>/<view…>` —
    /// or `None` when the level offers no `type: script` action (so no
    /// shortcut could be run there) or the view has no adapter. Computed
    /// identically at bind time (from the [`ScriptContext`]) and claim-
    /// registration time so the two always agree.
    /// Editor file suffix for this tab's query body (syntax highlighting),
    /// from the adapter's declared query language. Falls back to `.yaml` (the
    /// FilterExpr DSL) when no adapter is attached.
    pub fn query_body_suffix(&self) -> String {
        self.adapter
            .as_ref()
            .map(|a| a.query_body_suffix().to_string())
            .unwrap_or_else(|| ".yaml".to_string())
    }

    pub fn focused_script_scope(&self) -> Option<String> {
        self.active_script_action()?;
        let tab = self.adapter.as_ref()?.adapter_type().to_string();
        let view_path = self.active_pane().script_scope_path(&self.view_defs);
        Some(format!("script:{tab}/{}", view_path.join("/")))
    }

    /// Conflict description for binding `shortcut` to the script `name` in
    /// the focused level's scope, or `None` when the key is free. Reuses
    /// the keymap-wide saved-query collision check (globals, navigation,
    /// window chords, YAML actions/shortcuts, action chains, plus the
    /// already-bound script shortcuts) so a script shortcut can never
    /// shadow a key active in its tab.
    pub fn script_shortcut_conflict(
        &self,
        kb: &KeyBindingConfig,
        name: &str,
        shortcut: &str,
    ) -> Option<String> {
        let bound: Vec<(String, String)> = self
            .focused_script_scope()
            .and_then(|scope| self.script_shortcuts.get(&scope).cloned())
            .unwrap_or_default();
        crate::keymap::saved_query_shortcut_conflict(
            &self.tab_name,
            &self.view_defs,
            kb,
            name,
            shortcut,
            &bound,
        )
    }

    /// Apply the queries loaded for this view: bodies from the adapter's two
    /// stores, shortcuts from the `query_shortcut` table. Both kinds live in
    /// one list on purpose — the menu shows them together and the user is not
    /// meant to have to know which store a name came from.
    pub fn merge_saved_queries(&mut self, queries: Vec<MergedSavedQuery>) {
        self.db_saved_queries = queries;
    }

    /// Which store `name` was loaded from. The menu carries only the name and
    /// the body, so the kind is looked up here rather than threaded through
    /// the popup. An unknown name is adapter-native: that is what a body
    /// typed into the query editor is.
    pub fn query_kind_of(&self, name: &str) -> QueryKind {
        self.db_saved_queries
            .iter()
            .find(|q| q.name == name)
            .map(|q| q.kind)
            .unwrap_or(QueryKind::Saved)
    }

    /// Apply loaded items to the active pane (used by tests and code
    /// paths without an explicit [`PaneId`]).
    pub fn set_items(
        &mut self,
        items: Vec<NodeSummary>,
        applied_sort: Vec<SortKey>,
        page: Option<PageInfo>,
        columns: Vec<not_yet_done_content::ColumnSchema>,
        error: Option<String>,
    ) {
        let pane_id = self.active_pane_id();
        self.set_items_for_pane(pane_id, items, applied_sort, page, columns, error);
    }

    /// Apply the result of a custom adapter query (e.g. raw SQL from
    /// the Postgres Q-editor) to a specific pane. Distinguishes from
    /// `set_items_for_pane` in two ways:
    ///
    /// 1. The pane remembers `custom_query` so its next/prev-page keys
    ///    can re-execute the same query with a new offset instead of
    ///    falling back to `list()`.
    /// 2. The pane's `current_page` is synced to whatever the adapter
    ///    reports (so a freshly-run SELECT correctly shows page 0
    ///    even if the user previously paged into a regular list).
    ///
    /// Dropped silently if the pane has been closed.
    /// Apply children loaded for an expanded tree node. Updates the
    /// pane's `tree.cache[parent_path]`, re-flattens `tree.entries`,
    /// and rebuilds the visible table so the new rows show up. Dropped
    /// silently if the pane has been closed or is no longer in tree
    /// mode.
    /// Remove `node_id` from `pane_id`'s tree in place (local delete) and
    /// rebuild. Returns `false` when the pane has no tree or the node isn't a
    /// current row, so the caller can fall back to a full reload. See
    /// [`TreeState::remove_node`] for the cache/expansion bookkeeping.
    pub fn remove_tree_node(&mut self, pane_id: PaneId, node_id: &str) -> bool {
        let Some(tree_idx) = self
            .pane_trees
            .iter()
            .position(|tree| tree.root.find_leaf(pane_id).is_some())
        else {
            return false;
        };
        let view_defs = self.view_defs.clone();
        let tree = &mut self.pane_trees[tree_idx];
        let mut removed = false;
        if let Some(leaf) = tree.root.find_leaf_mut(pane_id) {
            if let Some(state) = leaf.pane.tree.as_mut() {
                removed = state.remove_node(node_id);
                if removed {
                    if let Some(vd) = view_defs.get(leaf.pane.view_def_index) {
                        state.rebuild_entries(vd);
                    }
                    leaf.pane.rebuild_table(&view_defs);
                }
            }
        }
        if removed {
            self.sync_action_bar_hints();
        }
        removed
    }

    pub fn apply_tree_children(
        &mut self,
        pane_id: PaneId,
        parent_path: Vec<String>,
        items: Vec<NodeSummary>,
        page_info: Option<not_yet_done_content::PageInfo>,
        append: bool,
        child_node_type: String,
    ) {
        let Some(tree_idx) = self
            .pane_trees
            .iter()
            .position(|tree| tree.root.find_leaf(pane_id).is_some())
        else {
            return;
        };
        let view_defs = self.view_defs.clone();
        let next_page = next_page_after(page_info);
        let tree = &mut self.pane_trees[tree_idx];
        if let Some(leaf) = tree.root.find_leaf_mut(pane_id) {
            let Some(state) = leaf.pane.tree.as_mut() else {
                return;
            };
            // Multi-load mode (heterogeneous fan-out): route each
            // per-type response into its own bucket. The merged
            // `children` list is rebuilt in YAML order so arrival
            // order doesn't shuffle the rendered tree.
            let is_multi = state
                .cache
                .get(&parent_path)
                .map(|s| s.expected_types.is_some())
                .unwrap_or(false);
            if is_multi {
                state.apply_multi_load_result(parent_path, child_node_type, items);
            } else if append {
                state.extend_cached_children(parent_path, items, next_page);
            } else {
                state.set_cached_children(parent_path, items, next_page);
            }
            if let Some(vd) = view_defs.get(leaf.pane.view_def_index) {
                state.rebuild_entries(vd);
            }
            leaf.pane.rebuild_table(&view_defs);
        }
        self.sync_action_bar_hints();
    }

    /// Ingest a whole eagerly-expanded subtree in one shot (capability
    /// `supports_eager_subtree`): fill the tree cache for every level and mark
    /// every node with children as `expanded`, then rebuild entries + table
    /// ONCE. This is the eager counterpart of [`Self::apply_tree_children`] —
    /// instead of one cache slot per round-trip + a rebuild per slot (the
    /// O(N²) cascade), the adapter hands us the full structure and we lay it
    /// down in a single pass.
    ///
    /// `parent_path` is the path the subtree hangs under — `vec![]` for a
    /// root load. The adapter has already merged any multi-type children into
    /// each node's ordered `children` list, so there is no per-type bucketing
    /// to do here (unlike the cascade's `apply_multi_load_result`).
    pub fn apply_subtree(&mut self, pane_id: PaneId, parent_path: Vec<String>, subtree: Subtree) {
        let Some(tree_idx) = self
            .pane_trees
            .iter()
            .position(|tree| tree.root.find_leaf(pane_id).is_some())
        else {
            return;
        };
        let view_defs = self.view_defs.clone();
        let tree = &mut self.pane_trees[tree_idx];
        if let Some(leaf) = tree.root.find_leaf_mut(pane_id) {
            // Consume the reload signals set by `set_items`. On a reload we
            // preserve the existing `expanded` set (the user's fold choices)
            // instead of force-expanding the whole subtree, and we re-anchor
            // the cursor onto the node it sat on before.
            let preserve = std::mem::take(&mut leaf.pane.eager_reload_preserve_expansion);
            let reanchor = leaf.pane.eager_reload_reanchor_id.take();
            let Some(state) = leaf.pane.tree.as_mut() else {
                return;
            };
            ingest_subtree_level(state, parent_path, subtree, preserve);
            if let Some(vd) = view_defs.get(leaf.pane.view_def_index) {
                state.rebuild_entries(vd);
            }
            leaf.pane.rebuild_table(&view_defs);
            // Re-anchor onto the previously-selected node (deleted nodes are
            // gone from the fresh tree, so this simply fails and the clamped
            // row stands).
            if let Some(id) = reanchor {
                leaf.pane.focus_item_by_id(&id);
            }
            // The whole expanded shape arrived at once, so the per-node
            // expand cascade has nothing left to drive for this pane.
            if let Some(state) = leaf.pane.tree.as_mut() {
                state.auto_expand_pending = false;
            }
        }
        self.sync_action_bar_hints();
    }

    /// M9 now-bucket refresh: splice one grouped-tree bucket's refreshed
    /// `header` (its shifted total / `⏱` marker) and re-folded `subtree` into
    /// `pane_id`, leaving every other bucket's cache untouched — the targeted
    /// counterpart of [`Self::apply_subtree`]'s whole-forest replacement. Used
    /// when a tracking starts/stops: only the bucket "now" falls into changed,
    /// so only it re-folds.
    ///
    /// Returns `false` (changing nothing) when the bucket isn't a visible root
    /// row — a *start* can mint a brand-new bucket the splice can't graft onto,
    /// so the caller falls back to a full pane reload. The bucket's own
    /// expansion is preserved (`expanded` is never touched here), so a
    /// collapsed bucket stays collapsed.
    pub fn reload_now_bucket(
        &mut self,
        pane_id: PaneId,
        header: NodeSummary,
        subtree: Subtree,
    ) -> bool {
        let Some(tree_idx) = self
            .pane_trees
            .iter()
            .position(|tree| tree.root.find_leaf(pane_id).is_some())
        else {
            return false;
        };
        let view_defs = self.view_defs.clone();
        let mut spliced = false;
        let tree = &mut self.pane_trees[tree_idx];
        if let Some(leaf) = tree.root.find_leaf_mut(pane_id) {
            if let Some(state) = leaf.pane.tree.as_mut() {
                // Swap the bucket's header row at the root level in place.
                // Absent → brand-new bucket: bail so the caller full-reloads.
                let present = state
                    .cache
                    .get_mut(&Vec::<String>::new())
                    .and_then(|root| root.children.iter_mut().find(|c| c.id == header.id))
                    .map(|slot| *slot = header.clone())
                    .is_some();
                if present {
                    // Replace the bucket's whole re-folded subtree under its
                    // own path; sibling buckets' cache slots stay as they are.
                    // `false` keeps the pre-existing force-expand within the
                    // refreshed bucket (this is the tracking-toggle splice, not
                    // the `r` reload path).
                    ingest_subtree_level(state, vec![header.id.clone()], subtree, false);
                    if let Some(vd) = view_defs.get(leaf.pane.view_def_index) {
                        state.rebuild_entries(vd);
                    }
                    leaf.pane.rebuild_table(&view_defs);
                    spliced = true;
                }
            }
        }
        if spliced {
            self.sync_action_bar_hints();
        }
        spliced
    }

    /// Initialise the tree cache entry for a parent that expects N
    /// per-type loads (heterogeneous fan-out). Called by the
    /// `ExpandTreeNodeMulti` dispatch before firing per-type loads.
    pub fn begin_tree_multi_load(
        &mut self,
        pane_id: PaneId,
        parent_path: Vec<String>,
        expected_types: Vec<String>,
    ) {
        let Some(tree_idx) = self
            .pane_trees
            .iter()
            .position(|tree| tree.root.find_leaf(pane_id).is_some())
        else {
            return;
        };
        let tree = &mut self.pane_trees[tree_idx];
        if let Some(leaf) = tree.root.find_leaf_mut(pane_id) {
            if let Some(state) = leaf.pane.tree.as_mut() {
                state.begin_multi_load(parent_path, expected_types);
            }
        }
    }

    /// Roll back a pending expand when the async load failed: drop
    /// the parent_path from `expanded` so the row appears collapsed
    /// again. Cache stays empty (no `loaded` flip), so the next
    /// expand attempt re-issues the request.
    pub fn cancel_tree_expand(&mut self, pane_id: PaneId, parent_path: Vec<String>) {
        let Some(tree_idx) = self
            .pane_trees
            .iter()
            .position(|tree| tree.root.find_leaf(pane_id).is_some())
        else {
            return;
        };
        let view_defs = self.view_defs.clone();
        let tree = &mut self.pane_trees[tree_idx];
        if let Some(leaf) = tree.root.find_leaf_mut(pane_id) {
            let Some(state) = leaf.pane.tree.as_mut() else {
                return;
            };
            state.expanded.remove(&parent_path);
            if let Some(vd) = view_defs.get(leaf.pane.view_def_index) {
                state.rebuild_entries(vd);
            }
            leaf.pane.rebuild_table(&view_defs);
        }
    }

    pub fn apply_custom_query_result(
        &mut self,
        pane_id: PaneId,
        items: Vec<NodeSummary>,
        page: Option<PageInfo>,
        custom_query: Option<CustomQueryRunState>,
    ) {
        let Some(tree_idx) = self
            .pane_trees
            .iter()
            .position(|tree| tree.root.find_leaf(pane_id).is_some())
        else {
            return;
        };
        let view_defs = &self.view_defs;
        let tree = &mut self.pane_trees[tree_idx];
        if let Some(leaf) = tree.root.find_leaf_mut(pane_id) {
            // Resolve the pane's effective pagination mode from its
            // active view/child config *before* set_items so the
            // restored CustomQueryRunState carries the right mode for
            // subsequent `>` / `<` keys (the spawn closure stamps a
            // placeholder `Server` because it has no view-config
            // reference).
            let mode = leaf.pane.resolve_pagination_mode(view_defs);
            leaf.pane
                .set_items(items, Vec::new(), page, Vec::new(), None, view_defs);
            if let Some(info) = page {
                leaf.pane.set_current_page(Some(PageRequest {
                    offset: info.offset,
                    limit: info.limit,
                }));
            }
            // set_items wipes active_custom_query — restore it after,
            // patching `mode` from the resolved view config.
            leaf.pane.active_custom_query = custom_query.map(|mut cq| {
                cq.mode = mode;
                cq
            });
        }
        self.sync_action_bar_hints();
    }

    /// Apply loaded items to a specific pane by id. Drops the response
    /// silently if the pane has been closed.
    pub fn set_items_for_pane(
        &mut self,
        pane_id: PaneId,
        items: Vec<NodeSummary>,
        applied_sort: Vec<SortKey>,
        page: Option<PageInfo>,
        columns: Vec<not_yet_done_content::ColumnSchema>,
        error: Option<String>,
    ) {
        // ContentPane::set_items needs &[ViewDef] for rebuild_table. Two
        // disjoint mutable borrows from `self` would clash, so split via
        // raw indexing — locate the pane's tree first, then borrow
        // `view_defs` and `pane_trees[i]` as separate fields of `self`.
        let Some(tree_idx) = self
            .pane_trees
            .iter()
            .position(|tree| tree.root.find_leaf(pane_id).is_some())
        else {
            return;
        };
        // A result without an error means the shared adapter connection is
        // live — record it so a later subtab switch auto-loads instead of
        // showing the `manual_connect` connect banner (one instance, one
        // connection serves every subtab).
        if error.is_none() {
            self.connected_once = true;
        }
        let view_defs = &self.view_defs;
        let tree = &mut self.pane_trees[tree_idx];
        if let Some(leaf) = tree.root.find_leaf_mut(pane_id) {
            leaf.pane
                .set_items(items, applied_sort, page, columns, error, view_defs);
        }
        self.sync_action_bar_hints();
    }

    /// Collect a pane's pending `expand_depth` auto-expansion requests
    /// (see [`ContentPane::pending_auto_expand_requests`]). Borrow dance
    /// mirrors [`Self::set_items_for_pane`].
    pub fn pending_auto_expand_requests(
        &mut self,
        view_index: usize,
        pane_id: PaneId,
    ) -> Vec<ViewRequest> {
        let Some(tree_idx) = self
            .pane_trees
            .iter()
            .position(|tree| tree.root.find_leaf(pane_id).is_some())
        else {
            return Vec::new();
        };
        let view_defs = self.view_defs.clone();
        let tree = &mut self.pane_trees[tree_idx];
        match tree.root.find_leaf_mut(pane_id) {
            Some(leaf) => leaf
                .pane
                .pending_auto_expand_requests(view_index, pane_id, &view_defs),
            None => Vec::new(),
        }
    }

    /// Collect a pane's expanded-subtree refresh requests after a root
    /// reload (see [`ContentPane::pending_expanded_refresh_requests`]).
    pub fn pending_expanded_refresh_requests(
        &self,
        view_index: usize,
        pane_id: PaneId,
    ) -> Vec<ViewRequest> {
        match self.find_pane(pane_id) {
            Some(pane) => {
                pane.pending_expanded_refresh_requests(view_index, pane_id, &self.view_defs)
            }
            None => Vec::new(),
        }
    }

    /// Apply a preview-fetch result to a specific pane by id.
    pub fn set_preview_description_for_pane(
        &mut self,
        pane_id: PaneId,
        key: &str,
        description: String,
    ) {
        if let Some(pane) = self.find_pane_mut(pane_id) {
            pane.set_preview_description(key, description);
        }
    }

    /// After a list reload (e.g. post-`mark_as_read`) the row at the
    /// cursor may now point at a different item than the one being
    /// previewed. Returns a follow-up `FetchContentPreview` request when
    /// the pane's preview is open and the selected row's id has drifted
    /// away from `preview_key`; the caller dispatches it like any other
    /// `ViewRequest`.
    pub fn pending_preview_request(
        &mut self,
        view_index: usize,
        pane_id: PaneId,
    ) -> Option<ViewRequest> {
        let tree_idx = self
            .pane_trees
            .iter()
            .position(|tree| tree.root.find_leaf(pane_id).is_some())?;
        let view_defs = &self.view_defs;
        let tree = &mut self.pane_trees[tree_idx];
        let leaf = tree.root.find_leaf_mut(pane_id)?;
        let p = leaf.pane.update_preview_for_selection(view_defs)?;
        Some(ViewRequest::FetchContentPreview {
            view_index,
            pane_id,
            cache_key: p.cache_key,
            node_id: p.node_id,
            action_id: p.action_id,
        })
    }

    pub fn set_auth_status(&mut self, status: AdapterStatus) {
        self.auth_status = status;
    }

    /// True while the adapter is `Busy` — the only banner state whose
    /// text advances purely with wall-clock time. The main loop polls
    /// this to keep the elapsed-seconds counter ticking when idle.
    pub fn is_busy(&self) -> bool {
        matches!(self.auth_status, AdapterStatus::Busy { .. })
    }

    /// Apply the global `notifications.load_banner` to this tab, unless its
    /// view file overrode it. Called by App when it wires the view, because
    /// [`Self::new`] sees only the view file and not the TUI config.
    pub fn set_load_banner_default(&mut self, global: LoadBannerRoute) {
        self.load_banner_route = self.tab_load_banner.unwrap_or(global);
    }

    /// Where this tab's load banner goes. The App asks before deciding
    /// whether to put the tab on the global surface.
    pub fn load_banner_route(&self) -> LoadBannerRoute {
        self.load_banner_route
    }

    /// This tab's load banner for the *global* surface, or `None` when the
    /// tab is not loading or does not route there. The text carries no tab
    /// name — the caller adds it, since only it knows whether the surface
    /// needs the attribution.
    pub fn global_load_banner(&self) -> Option<LoadBanner> {
        if self.load_banner_route != LoadBannerRoute::Global {
            return None;
        }
        match &self.auth_status {
            AdapterStatus::Busy {
                label,
                started_at_unix_ms,
                timeout_secs,
                progress,
            } => Some(LoadBanner {
                text: busy_banner(label, *started_at_unix_ms, *timeout_secs, *progress),
                started_at_unix_ms: *started_at_unix_ms,
            }),
            _ => None,
        }
    }

    pub fn set_adapter_init_error(&mut self, err: String) {
        self.adapter_init_error = Some(err);
    }

    /// Format the auth-status banner text (or `None` when no banner
    /// should render). The fetch_error fallback consults the active
    /// pane so each pane can surface its own per-load error.
    ///
    /// Precedence (top → bottom): adapter init error, Connecting /
    /// NeedsCreds / Failed auth states, in-flight pane retry
    /// (combined with adapter Busy countdown when both apply), bare
    /// adapter Busy, `manual_connect` not-yet-loaded hint, sticky
    /// `fetch_error`.
    ///
    /// Only the `Busy` part is routable ([`ContentView::load_banner_route`]) —
    /// everything else here is a state the user must act on *in this tab*, so
    /// it is always drawn locally.
    fn auth_status_banner(&self) -> Option<String> {
        if let Some(err) = &self.adapter_init_error {
            return Some(format!("Configuration error: {err}"));
        }
        match &self.auth_status {
            // Shared with the CLI's progress line so the wording cannot drift.
            AdapterStatus::Connecting { .. } | AdapterStatus::Failed { .. } => {
                self.auth_status.banner_text()
            }
            AdapterStatus::NeedsCreds { .. } => {
                Some("Login required (press the action key to enter credentials)".into())
            }
            // Busy routed away from this tab (`global` / `off`): the progress
            // line is not ours to draw. A retry is not progress but a fault the
            // user may need to locate, so it stays here on either route.
            AdapterStatus::Busy { .. } if self.load_banner_route != LoadBannerRoute::Tab => {
                self.active_pane().retry_state.as_ref().map(|rs| {
                    format!(
                        "Retrying ({}/{}): {}",
                        rs.attempt, rs.max_attempts, rs.last_error
                    )
                })
            }
            AdapterStatus::Busy {
                label,
                started_at_unix_ms,
                timeout_secs,
                progress,
            } => {
                let busy = busy_banner(label, *started_at_unix_ms, *timeout_secs, *progress);
                Some(match self.active_pane().retry_state.as_ref() {
                    Some(rs) => format!(
                        "Retrying ({}/{}) — {busy}: {}",
                        rs.attempt, rs.max_attempts, rs.last_error
                    ),
                    None => busy,
                })
            }
            AdapterStatus::Idle | AdapterStatus::Ready => {
                if let Some(rs) = self.active_pane().retry_state.as_ref() {
                    return Some(format!(
                        "Retrying ({}/{}): {}",
                        rs.attempt, rs.max_attempts, rs.last_error
                    ));
                }
                if let Some(banner) = self.manual_connect_banner() {
                    return Some(banner);
                }
                self.active_pane()
                    .fetch_error
                    .as_ref()
                    .map(|e| format!("Fetch failed: {e}"))
            }
        }
    }

    /// Banner shown when `manual_connect: true` and the active pane
    /// has not yet been loaded. Tells the user which key triggers
    /// the connection (the first `type: reload` action of the active
    /// subtab's ViewDef). Falls back to a generic message when no
    /// reload action is configured — the YAML is still consistent
    /// (the user can connect via the cmdline-equivalent), but the
    /// banner can't name a specific key.
    fn manual_connect_banner(&self) -> Option<String> {
        if !self.manual_connect {
            return None;
        }
        // The shared connection is already up (a sibling subtab connected) —
        // the unloaded pane auto-loads on switch, so don't tell the user to
        // reconnect.
        if self.connected_once {
            return None;
        }
        if self.active_pane().loaded {
            return None;
        }
        let reload_key = self.view_defs.get(self.active_subtab).and_then(|vd| {
            vd.actions
                .iter()
                .find(|a| a.action_type == "reload")
                .and_then(|a| a.primary_key().map(str::to_string))
        });
        Some(match reload_key {
            Some(k) => format!("Auto-connect disabled — press `{k}` to connect"),
            None => "Auto-connect disabled — no `reload` action configured for this view".into(),
        })
    }

    pub fn rebuild_table(&mut self) {
        // Live record-detail coupling: refresh any follower from its
        // source's current selection before the focused pane redraws, so a
        // cursor move in the source is reflected in the same frame.
        self.sync_detail_panes();
        // Forward the live header overlay (column / direction picker)
        // so the active pane's table reflects it.
        let view_defs = &self.view_defs;
        let overlay = self.header_overlay.clone();
        self.pane_trees[self.active_subtab]
            .focused_leaf_mut()
            .pane
            .rebuild_table_with(view_defs, &overlay);
    }

    /// Repaint-driven recompute of live (time-derived) cells (M5): rebuild
    /// the table of every pane — across all subtabs' split trees — whose
    /// active level has a `kind: elapsed` column, so `now − field` advances.
    /// Purely re-renders the already-loaded items (no refetch); panes
    /// without a live column are untouched and their cached rows redraw
    /// unchanged. Driven by `Invalidation::Repaint` from the domain-event
    /// bus. The disjoint borrow (`&self.view_defs` + `&mut self.pane_trees`)
    /// avoids cloning the view defs on every tick.
    pub fn repaint_live_columns(&mut self) {
        let view_defs = &self.view_defs;
        let overlay = self.header_overlay.clone();
        for tree in self.pane_trees.iter_mut() {
            tree.root.for_each_leaf_mut(&mut |leaf| {
                if leaf.pane.has_live_column(view_defs) {
                    leaf.pane.rebuild_table_with(view_defs, &overlay);
                }
            });
        }
    }

    /// Re-fit the active subtab's tables whose column layout was built for a
    /// different width than they just rendered into. Called once after each
    /// draw (`App::refit_visible_tables`): the table widget records its real
    /// render width during `view()`, but the column widths were computed at
    /// the previous `rebuild_table` — so first paint, a terminal resize, or a
    /// preview open/close leaves the cells fitted to a stale width until this
    /// pass rebuilds them. Returns `true` if any pane was rebuilt, so the
    /// caller can request one more frame; the next pass is then a no-op
    /// (widths match) and the loop parks. Only the active subtab's panes have
    /// a fresh render width, so other subtabs re-fit when they next render.
    pub fn refit_tables_if_needed(&mut self) -> bool {
        let view_defs = &self.view_defs;
        let overlay = self.header_overlay.clone();
        let mut refit = false;
        self.pane_trees[self.active_subtab]
            .root
            .for_each_leaf_mut(&mut |leaf| {
                let rw = leaf.pane.table.last_render_width();
                if rw != 0 && rw != leaf.pane.built_table_width {
                    leaf.pane.rebuild_table_with(view_defs, &overlay);
                    refit = true;
                }
            });
        refit
    }

    /// Patch a single row's complete state in place (M9 —
    /// [`Invalidation::Row`](not_yet_done_content::Invalidation::Row)). The
    /// adapter pushes the refreshed [`NodeSummary`]; every pane in this tab
    /// that currently shows a row with the same `id` has its loaded item
    /// replaced and its table rebuilt (which re-derives the cells and, when
    /// grouping is active, the per-group totals + footer). No refetch and
    /// no selection/scroll change. A summary matching no visible item is a
    /// no-op. Returns `true` if at least one pane was patched.
    ///
    /// Mirrors [`repaint_live_columns`](Self::repaint_live_columns)'s
    /// disjoint borrow (`&self.view_defs` + `&mut self.pane_trees`) so the
    /// view defs aren't cloned per patch.
    /// The currently-displayed [`NodeSummary`] for `node_id` in `pane_id`,
    /// if the row is visible — its flat-list item or a tree-cache child at
    /// any depth. Used to carry forward fields a bare re-fetched `Node`
    /// can't reconstruct (notably `has_children`) when patching a row after
    /// an in-place edit.
    pub fn visible_summary(
        &self,
        pane_id: PaneId,
        node_id: &str,
    ) -> Option<not_yet_done_content::NodeSummary> {
        let pane = self.find_pane(pane_id)?;
        if let Some(it) = pane.items.iter().find(|it| it.id == node_id) {
            return Some(it.clone());
        }
        let tree = pane.tree.as_ref()?;
        tree.cache
            .values()
            .find_map(|state| state.children.iter().find(|c| c.id == node_id).cloned())
    }

    pub fn patch_row(&mut self, summary: &not_yet_done_content::NodeSummary) -> bool {
        let view_defs = &self.view_defs;
        let overlay = self.header_overlay.clone();
        let mut patched = false;
        for tree in self.pane_trees.iter_mut() {
            tree.root.for_each_leaf_mut(&mut |leaf| {
                // Depth-0 list rows (flat / condensed panes).
                let mut item_hit = false;
                if let Some(slot) = leaf.pane.items.iter_mut().find(|it| it.id == summary.id) {
                    *slot = summary.clone();
                    item_hit = true;
                }
                // Tree rows at *any* depth (grouped / eager trees — a live
                // tick patches a ticking duration cell on a deep tree-item or
                // its bucket header, neither of which lives in `pane.items`).
                // Swap the cached child wherever it sits in the tree cache,
                // then rebuild the flattened entries so the new cell shows.
                let mut tree_hit = false;
                if let Some(state) = leaf.pane.tree.as_mut() {
                    for node_state in state.cache.values_mut() {
                        if let Some(slot) =
                            node_state.children.iter_mut().find(|c| c.id == summary.id)
                        {
                            *slot = summary.clone();
                            tree_hit = true;
                        }
                    }
                    if tree_hit {
                        if let Some(vd) = view_defs.get(leaf.pane.view_def_index) {
                            state.rebuild_entries(vd);
                        }
                    }
                }
                if item_hit {
                    leaf.pane.rebuild_table_with(view_defs, &overlay);
                    patched = true;
                } else if tree_hit {
                    leaf.pane.rebuild_table(view_defs);
                    patched = true;
                }
            });
        }
        patched
    }

    /// Push a refreshed `link_refs` snapshot — plus the adapter NodeRef
    /// prefix (`"{kind}/{instance_id}"`) — down to every pane in this
    /// tab. Called by the App whenever its own `App::link_refs` cache
    /// changes, so the `has_links` YAML column always reads from a
    /// current set without going through App on every rebuild.
    pub fn set_link_refs(&mut self, link_refs: &std::collections::HashSet<String>) {
        let prefix = self
            .adapter
            .as_ref()
            .map(|a| format!("{}/{}", a.adapter_type(), a.instance_id()));
        for tree in self.pane_trees.iter_mut() {
            let mut ids = Vec::new();
            tree.root.collect_leaf_ids(&mut ids);
            for id in ids {
                if let Some(leaf) = tree.root.find_leaf_mut(id) {
                    leaf.pane.set_link_context(link_refs, prefix.clone());
                }
            }
        }
    }

    // ── Column config (popup `c`) ────────────────────────────────────

    /// The persisted/persistable override map (level key → visible column
    /// keys in order). The App serializes this as one JSON settings row
    /// per tab.
    pub fn column_overrides(&self) -> &std::collections::HashMap<String, Vec<String>> {
        &self.column_overrides
    }

    /// Replace the override map (startup load from settings) and mirror it
    /// into every pane across all subtab split-trees, rebuilding their
    /// tables so already-loaded panes re-render with the new layout.
    pub fn set_column_overrides(
        &mut self,
        overrides: std::collections::HashMap<String, Vec<String>>,
    ) {
        self.column_overrides = overrides;
        self.distribute_column_overrides();
    }

    fn distribute_column_overrides(&mut self) {
        let view_defs = &self.view_defs;
        let overlay = self.header_overlay.clone();
        let overrides = self.column_overrides.clone();
        for tree in self.pane_trees.iter_mut() {
            tree.root.for_each_leaf_mut(&mut |leaf| {
                leaf.pane.set_column_overrides(overrides.clone());
                if leaf.pane.loaded {
                    leaf.pane.rebuild_table_with(view_defs, &overlay);
                }
            });
        }
    }

    /// Record the columns the adapter *described* for one node type
    /// ([`ContentAdapter::describe_columns`], fetched by the load pipeline),
    /// mirroring them into every pane so already-loaded tables re-render with
    /// the backend-authoritative column types. All panes share the view's one
    /// adapter, so a node type's schema is valid for every pane.
    pub fn record_column_schema(
        &mut self,
        node_type: String,
        schema: Vec<not_yet_done_content::ColumnSchema>,
    ) {
        let view_defs = &self.view_defs;
        let overlay = self.header_overlay.clone();
        for tree in self.pane_trees.iter_mut() {
            tree.root.for_each_leaf_mut(&mut |leaf| {
                leaf.pane
                    .set_column_schema(node_type.clone(), schema.clone());
                if leaf.pane.loaded {
                    leaf.pane.rebuild_table_with(view_defs, &overlay);
                }
            });
        }
    }

    /// Data for the column-config popup on the active pane's current
    /// level: `(currently visible keys in order, all configurable
    /// columns)`. `None` when the level has no YAML-configured columns
    /// (auto-fallback levels — e.g. postgres rows — derive their schema
    /// from the data and aren't configurable).
    pub fn column_config_entries(
        &self,
    ) -> Option<(
        Vec<String>,
        Vec<crate::components::column_config_popup::ColumnEntry>,
    )> {
        use crate::components::column_config_popup::ColumnEntry;
        let pane = self.active_pane();
        let (raw, tree_label) = pane.column_config_source(&self.view_defs)?;
        let entries: Vec<ColumnEntry> = raw
            .iter()
            .map(|col| {
                let name = col.label.clone().unwrap_or_else(|| col.key.clone());
                ColumnEntry {
                    id: col.key.clone(),
                    header: name.clone(),
                    display_name: name,
                    color: resolve_theme_color(
                        &self.theme,
                        col.style.as_deref().unwrap_or("text_med"),
                    ),
                    hideable: tree_label.as_deref() != Some(col.key.as_str()),
                }
            })
            .collect();
        // No override → pre-check the default visible set (non-`hidden`
        // columns plus the tree label), so a `hidden` column appears in the
        // popup as an available, unchecked row the user can enable.
        let current = pane
            .column_level_key(&self.view_defs)
            .and_then(|k| self.column_overrides.get(&k).cloned())
            .unwrap_or_else(|| default_visible_keys(&raw, tree_label.as_deref()));
        Some((current, entries))
    }

    /// Apply the popup result to the active pane's current level. A
    /// selection identical to the raw YAML layout removes the override
    /// (clean reset — nothing stale persists once the user restores the
    /// default). Distributes to all panes and rebuilds. Returns `false`
    /// when the level isn't configurable (no level key).
    pub fn apply_column_config(&mut self, visible: Vec<String>) -> bool {
        let Some((raw, tree_label)) = self.active_pane().column_config_source(&self.view_defs)
        else {
            return false;
        };
        let Some(key) = self.active_pane().column_level_key(&self.view_defs) else {
            return false;
        };
        // A selection identical to the default visible set (non-`hidden`
        // columns plus the tree label) removes the override — the clean
        // reset, so re-hiding an enabled column restores YAML defaults.
        if visible == default_visible_keys(&raw, tree_label.as_deref()) {
            self.column_overrides.remove(&key);
        } else {
            self.column_overrides.insert(key, visible);
        }
        self.distribute_column_overrides();
        true
    }

    // ── Card mode (`card.key`) ───────────────────────────────────────

    /// The persistable per-level card-mode map (level key → on/off). The App
    /// serializes it as one JSON settings row per tab.
    pub fn card_mode_overrides(&self) -> &std::collections::HashMap<String, bool> {
        &self.card_mode_overrides
    }

    /// Replace the card-mode map (startup load from settings) and mirror it
    /// into every pane, rebuilding loaded tables so they re-render in the
    /// stored mode. This is what makes a toggled card mode survive a restart.
    pub fn set_card_mode_overrides(&mut self, overrides: std::collections::HashMap<String, bool>) {
        self.card_mode_overrides = overrides;
        self.distribute_card_mode_overrides();
    }

    fn distribute_card_mode_overrides(&mut self) {
        let view_defs = &self.view_defs;
        let overlay = self.header_overlay.clone();
        let overrides = self.card_mode_overrides.clone();
        for tree in self.pane_trees.iter_mut() {
            tree.root.for_each_leaf_mut(&mut |leaf| {
                leaf.pane.set_card_mode_overrides(overrides.clone());
                if leaf.pane.loaded {
                    leaf.pane.rebuild_table_with(view_defs, &overlay);
                }
            });
        }
    }

    /// Flip card mode on the focused pane's level and ask the App to persist
    /// the choice. Owned by the view (not the pane) because the map is
    /// view-level state mirrored across splits, just like the column
    /// overrides. A no-op — key left unhandled — when the level declares no
    /// `card:` block or has no stable level key to remember it under.
    fn toggle_card_mode(&mut self) -> SubViewMessage {
        let pane = self.active_pane();
        if !pane.card_available(&self.view_defs) {
            return SubViewMessage::Unhandled;
        }
        let Some(key) = pane.column_level_key(&self.view_defs) else {
            return SubViewMessage::Unhandled;
        };
        let default = pane
            .current_card(&self.view_defs)
            .map(|c| c.default)
            .unwrap_or(false);
        let next = !pane.card_mode_active(&self.view_defs);
        // Back at the configured default → drop the entry, so a full round
        // trip leaves no stale row behind (same clean-reset rule as the
        // column overrides).
        if next == default {
            self.card_mode_overrides.remove(&key);
        } else {
            self.card_mode_overrides.insert(key, next);
        }
        self.distribute_card_mode_overrides();
        SubViewMessage::Request(ViewRequest::PersistCardMode {
            view_index: self.view_index,
        })
    }

    // ── Key handling ─────────────────────────────────────────────────

    pub fn handle_key(&mut self, key: &str) -> SubViewMessage {
        if self.active_view_def().is_none() {
            return SubViewMessage::Unhandled;
        }

        // Query popup mode — intercepts every key. Outside the keymap.
        if self.query_menu.is_open() {
            return self
                .handle_query_popup_key(key)
                .unwrap_or(SubViewMessage::Unhandled);
        }

        // Group-by menu — same popup-mode interception.
        if self.group_menu.is_open() {
            return self.handle_group_menu_key(key);
        }

        // Window-leader chord runs before any other key handler so that
        // once the leader is consumed, the resolution key is interpreted
        // strictly as a window action (split / close / pane-tag switch)
        // and never falls through to subtab / popup / saved-query
        // shortcuts. The chord handler still defers to text-input modes
        // via its own input-mode guard. Outside the keymap because chord
        // resolution depends on a multi-step state machine.
        if let Some(msg) = self.handle_window_chord(key) {
            return msg;
        }

        // Text-input intercept — fuzzy or `/`-search consumes characters
        // on the active pane. We still reach the pane (which dispatches
        // the input), but skip any tab-level claim that would steal a
        // single letter from the input buffer.
        let in_text_input =
            self.active_pane().table.fuzzy_active || self.active_pane().search.active();

        // Per-node YAML `shortcuts:` win over tab-level claims (subtab
        // switch, query-menu key, saved-query / per-table chords). The
        // user binds these on the deepest reachable ChildDef, so they
        // are by definition the most-specific binding visible at the
        // cursor — and the action-bar hint we surface to the user
        // commits to them being live. Without this pre-check, a subtab
        // key like `d` for the "databases" view in postgres.yaml
        // shadows the leaf-level `d: delete` on `postgres:db_script`
        // and the hint silently lies. Single-char keys only; modifier-
        // bearing keys (`ctrl+e`, …) can't collide with subtab keys
        // and don't need this fast-path.
        if !in_text_input {
            let view_index = self.view_index;
            let pane_id = self.active_pane_id();
            let view_defs_ref = &self.view_defs;
            let req = self.pane_trees[self.active_subtab]
                .focused_leaf()
                .pane
                .try_node_action_shortcut(key, view_index, pane_id, view_defs_ref);
            if let Some(req) = req {
                return SubViewMessage::Request(req);
            }
        }

        if !in_text_input {
            let claims = self.build_view_claims();
            for claim in &claims.claims {
                if !claim.key.matches(key) {
                    continue;
                }
                if let Some(msg) = self.dispatch_view_claim(&claim.source) {
                    return msg;
                }
            }
        }

        // Delegate the rest to the active pane. Refresh the focused
        // table's jump alphabet first so the `jump_mode` action (and label
        // rendering) work on panes created later by splits/drills, which
        // never went through the App's one-shot startup wiring.
        if !self.nav_chars.is_empty() {
            self.pane_trees[self.active_subtab]
                .focused_leaf_mut()
                .pane
                .table
                .set_nav_chars(&self.nav_chars);
        }
        let view_index = self.view_index;
        let pane_id = self.active_pane_id();
        // Selection row before the key, so the mark-read hook can tell an
        // arrival at the last row from a key pressed while already there.
        let before_row = self.focused_pane_selected_row();
        let msg = {
            let view_defs = &self.view_defs;
            let common_kb = &self.common_kb;
            let content_kb = &self.content_kb;
            self.pane_trees[self.active_subtab]
                .focused_leaf_mut()
                .pane
                .handle_key(key, view_index, pane_id, view_defs, common_kb, content_kb)
        };
        if let SubViewMessage::ContentDrill {
            item_id,
            item_label,
            child_def,
        } = msg
        {
            return self.dispatch_content_drill(item_id, item_label, *child_def);
        }
        // A plain navigation (or any in-pane key) may have landed the cursor
        // on the newest unread row — queue the configured mark-read action
        // for the App to drain alongside `msg`.
        self.detect_mark_read_reached(before_row);
        msg
    }

    /// Index of the focused pane's currently-selected visible row.
    fn focused_pane_selected_row(&self) -> usize {
        self.pane_trees[self.active_subtab]
            .focused_leaf()
            .pane
            .table
            .selected_row()
    }

    /// After the focused pane handled a key, queue a `mark_read_on_reach_end`
    /// action if the selection just moved onto the (still-unread) last row of
    /// a flat drill level. The arrival gate (`before_row != last`) keeps mere
    /// opening of a list — or a key pressed while already at the bottom —
    /// from acking; the unread gate keeps it from re-firing after the
    /// ack-driven reload (the row then reads as read).
    fn detect_mark_read_reached(&mut self, before_row: usize) {
        let resolved: Option<(String, String)> = {
            let pane = &self.pane_trees[self.active_subtab].focused_leaf().pane;
            // Flat lists only — a tree pane never carries chat messages.
            match (pane.tree.is_some(), pane.mark_read_action()) {
                (false, Some(action)) => {
                    let last = pane.filtered_indices.len().checked_sub(1);
                    let row = pane.table.selected_row();
                    match last {
                        Some(last) if row == last && before_row != last => {
                            let unread = pane
                                .selected_item()
                                .map(|it| metadata_field_value(it, "unread") == "true")
                                .unwrap_or(false);
                            if unread {
                                pane.selected_item_id()
                                    .map(|id| (id.to_string(), action.to_string()))
                            } else {
                                None
                            }
                        }
                        _ => None,
                    }
                }
                _ => None,
            }
        };
        if let Some((node_id, action_name)) = resolved {
            let view_index = self.view_index;
            let pane_id = self.active_pane_id();
            self.pending_mark_read = Some(ViewRequest::InvokeNodeAction {
                view_index,
                pane_id,
                node_id,
                action_name,
            });
        }
    }

    /// Drain a queued `mark_read_on_reach_end` action (see
    /// [`Self::detect_mark_read_reached`]). The App calls this right after
    /// `handle_key` and dispatches the returned request.
    pub fn take_pending_mark_read(&mut self) -> Option<ViewRequest> {
        self.pending_mark_read.take()
    }

    /// Tab-level claims (subtab switch, query menu, saved-query
    /// shortcuts, adapter query editor). Mirrors the conditions of the
    /// original if-chain — claims are emitted only when their dynamic
    /// guards are satisfied, so the dispatcher itself can stay
    /// condition-free.
    fn build_view_claims(&self) -> KeyMap {
        let mut km = KeyMap::new();
        let scope = KeyScope::Tab(TabRef::new(""));

        // Subtab switch — active in every leaf of the focused pane
        // (root or drilldown). The validator (Phase 3) guarantees no
        // key conflicts between subtab keys and drilled-level actions
        // because subtab claims are pushed into every leaf's KeyMap.
        for vd in &self.view_defs {
            if let Some(k) = &vd.key {
                if k.0.is_empty() {
                    continue;
                }
                km.push(KeyClaim::handler(
                    k.clone(),
                    scope.clone(),
                    KeySource::YamlSubtab {
                        view: vd.name.clone(),
                    },
                ));
            }
        }

        // Query popup menu key — active in every leaf of the focused
        // view (root or drilldown). Validator enforces non-collision.
        if let Some(mk) = self.query_menu_key() {
            km.push(KeyClaim::handler(
                KeyBinding::new(mk.to_string()),
                scope.clone(),
                KeySource::YamlMenuKey {
                    view: self.active_view_name(),
                },
            ));
        }

        // Saved-query shortcuts — active in every leaf of the focused
        // view (root or drilldown). Validator enforces non-collision.
        for sq in &self.db_saved_queries {
            if let Some(sc) = sq.shortcut.as_deref() {
                km.push(KeyClaim::handler(
                    KeyBinding::new(sc.to_string()),
                    scope.clone(),
                    KeySource::SavedQueryShortcut {
                        view: self.active_view_name(),
                        name: sq.name.clone(),
                    },
                ));
            }
        }

        // Postgres-table-scoped scripts menu. Gated on a postgres-table
        // node being in focus — either the selected item (flat tables
        // subtab, or drilled into a schema) or the pane's parent
        // (drilled into a table; rows being displayed). The SQL editor
        // (`Q sql`) lives in the new per-node-action shortcut path via
        // YAML `shortcuts:` on the postgres view config.
        // Group-by menu (M3) — direct-jump counterpart of the pane-level
        // `cycle_grouping` claim, under the same gate (the active level
        // must declare a `group_by`), so the default `u` stays free for
        // YAML shortcuts on ungroupable levels.
        if self.active_pane().level_has_group_by(&self.view_defs) {
            if let Some(b) = self.content_kb.get(&ContentAction::GroupMenu) {
                km.push(KeyClaim::handler(
                    b.clone(),
                    scope.clone(),
                    KeySource::Content(ContentAction::GroupMenu),
                ));
            }
        }

        // Group-order toggle (`o`) — flip the bucket order asc/desc on a
        // grouped flat view. Shares the `o` key with `ToggleRecordDetail`,
        // but the gates are disjoint: this claims `o` only when the level
        // groups *and* no record-detail split is offered or already open
        // (so on wide-row record_detail views `o` still means the split).
        {
            let pane = self.active_pane();
            let detail_owns_o = pane.record_detail_enabled(&self.view_defs)
                || pane.detail_child.is_some()
                || pane.is_detail_pane();
            if pane.level_has_group_by(&self.view_defs) && !detail_owns_o {
                if let Some(b) = self.content_kb.get(&ContentAction::ToggleGroupOrder) {
                    km.push(KeyClaim::handler(
                        b.clone(),
                        scope.clone(),
                        KeySource::Content(ContentAction::ToggleGroupOrder),
                    ));
                }
            }
        }

        if let Some(node_id) = self.target_node_script_node_id() {
            if let Some(b) = self.content_kb.get(&ContentAction::OpenScriptsMenu) {
                km.push(KeyClaim::handler(
                    b.clone(),
                    scope.clone(),
                    KeySource::Content(ContentAction::OpenScriptsMenu),
                ));
            }
            // Per-table script shortcuts (SQ-8d). Mirrors the
            // SavedQueryShortcut path above, but data lives in a
            // separate cache populated by the App when this table
            // comes into focus.
            if let Some(entries) = self.node_script_shortcuts.get(&node_id) {
                for (script, chord) in entries {
                    km.push(KeyClaim::handler(
                        KeyBinding::new(chord.clone()),
                        scope.clone(),
                        KeySource::NodeScriptShortcut {
                            node_id: node_id.clone(),
                            script: script.clone(),
                        },
                    ));
                }
            }
        }

        // `:script`-menu shortcuts — active in every leaf of the focused
        // view that offers a `type: script` action (the gate is folded into
        // `focused_script_scope`). Mirrors the SavedQueryShortcut path; the
        // cache is populated by the App when the level comes into focus.
        if let Some(script_scope) = self.focused_script_scope() {
            if let Some(entries) = self.script_shortcuts.get(&script_scope) {
                for (name, chord) in entries {
                    km.push(KeyClaim::handler(
                        KeyBinding::new(chord.clone()),
                        scope.clone(),
                        KeySource::ScriptShortcut {
                            scope: script_scope.clone(),
                            name: name.clone(),
                        },
                    ));
                }
            }
        }

        // Record-detail split toggle (`o`) + value-wrap toggle (`X`).
        // Claimed at view level — not in the pane's `build_claims` — because
        // (un)splitting touches `pane_trees`, which only the ContentView
        // owns. `o` is offered when the focused pane is a record_detail flat
        // table (to open) or already owns / embodies a follower (to close);
        // `X` only while a follower is in play. Gates mirror the dispatch
        // resolution and the status-bar hints exactly.
        {
            let pane = self.active_pane();
            let can_open = pane.record_detail_enabled(&self.view_defs);
            let detail_in_play = pane.detail_child.is_some() || pane.is_detail_pane();
            if can_open || detail_in_play {
                if let Some(b) = self.content_kb.get(&ContentAction::ToggleRecordDetail) {
                    km.push(KeyClaim::handler(
                        b.clone(),
                        scope.clone(),
                        KeySource::Content(ContentAction::ToggleRecordDetail),
                    ));
                }
            }
            if detail_in_play {
                if let Some(b) = self.content_kb.get(&ContentAction::ToggleDetailWrap) {
                    km.push(KeyClaim::handler(
                        b.clone(),
                        scope.clone(),
                        KeySource::Content(ContentAction::ToggleDetailWrap),
                    ));
                }
            }
            // Long-text mode (`v`): only when a column opts in via `long_source`,
            // so the key stays free on every other view.
            if pane.long_text_available(&self.view_defs) {
                if let Some(b) = self.content_kb.get(&ContentAction::ToggleLongText) {
                    km.push(KeyClaim::handler(
                        b.clone(),
                        scope.clone(),
                        KeySource::Content(ContentAction::ToggleLongText),
                    ));
                }
            }
            // Card mode: claimed only on a level that declares `card:`, under
            // the key that level names (`card.key`). Nothing is claimed
            // elsewhere, so the key stays free on every other view. Handled at
            // view level because the mode map is view-level state.
            if let Some(b) = pane.card_toggle_binding(&self.view_defs, &self.content_kb) {
                km.push(KeyClaim::handler(
                    b,
                    scope.clone(),
                    KeySource::Content(ContentAction::ToggleCardMode),
                ));
            }
        }

        km
    }

    /// Does `key` begin a multi-char chord that the focused pane/view
    /// would resolve right now (e.g. `a` starting `al` → "new channel")?
    ///
    /// The App's chord interceptor needs this because YAML `actions:`
    /// keys live in the per-pane ([`ContentPane::build_claims`]) and
    /// per-view ([`Self::build_view_claims`]) keymaps — *not* in the
    /// typed `keybindings.content` section it normally consults. Without
    /// it, a configured multi-char action key would be split into two
    /// single-key dispatches and never fire. By probing the very same
    /// keymaps the dispatcher uses, any chord-length YAML key becomes a
    /// usable chord automatically, with no per-feature wiring.
    ///
    /// Node `shortcuts:` are single-char by construction (see
    /// [`Self::try_node_action_shortcut`]) and so are never chords —
    /// they need no consideration here.
    pub fn yaml_action_chord_prefix(&self, key: &str) -> bool {
        if self.active_view_def().is_none() {
            return false;
        }
        let pane = &self.pane_trees[self.active_subtab].focused_leaf().pane;
        let pane_claims = pane.build_claims(&self.view_defs, &self.common_kb, &self.content_kb);
        let view_claims = self.build_view_claims();
        pane_claims
            .claims
            .iter()
            .chain(view_claims.claims.iter())
            .any(|c| c.key.is_prefix(key))
    }

    /// Live keymap the dispatcher currently consults for the focused pane:
    /// the pane's per-level claims unioned with the view-level (tab) claims.
    /// Feeds the shortcut menu's *context* scope, so it lists exactly the
    /// keys that would fire right now.
    pub fn context_keymap(&self) -> KeyMap {
        let pane = &self.pane_trees[self.active_subtab].focused_leaf().pane;
        let mut km = pane.build_claims(&self.view_defs, &self.common_kb, &self.content_kb);
        for claim in self.build_view_claims().claims {
            km.push(claim);
        }
        km
    }

    /// The tab-switch key override configured in this view file's
    /// `tab.key`, if any. `None` means the tab uses its positional
    /// autonumber digit; `Some` with an empty list means it is disabled.
    pub fn tab_key_override(&self) -> Option<&crate::config::keybindings::KeyBinding> {
        self.tab_key.as_ref()
    }

    /// Whether this view currently shows anything unread — any row, in any
    /// of its panes, whose adapter marked it with `unread = "true"`. Drives
    /// the tab bar's unread marker + emphasis.
    ///
    /// Only what a pane holds **right now** counts: the tree's loaded nodes
    /// in tree mode, the current level's items otherwise. A level a pane has
    /// drilled away from is a frozen snapshot that no invalidation refreshes,
    /// so counting it could keep the tab lit after the messages were read.
    /// For a chat view that is the right rule anyway — the tree keeps its own
    /// pane through the coupled `split:`, and the server rows there carry the
    /// unread state of every channel below them.
    ///
    /// Recomputed per frame rather than cached: it is a `find` over the
    /// nodes' metadata, cheap next to the render it feeds, and the alternative
    /// (invalidating a cache from every path that touches items) is exactly
    /// the bookkeeping that goes stale.
    pub fn has_unread(&self) -> bool {
        self.all_pane_ids()
            .into_iter()
            .filter_map(|id| self.find_pane(id))
            .any(|pane| pane.has_unread())
    }

    /// The glyph the tab bar puts in front of this tab's label while the view
    /// is unread: `tab.unread_marker` when set, else the view's own
    /// `unread_marker` (first view def that sets one — the tab is one file, so
    /// its subtabs share the cue), else [`DEFAULT_TAB_UNREAD_MARKER`] (`🔔`).
    /// May be empty, which suppresses the glyph and leaves the emphasis to
    /// carry the signal.
    pub fn unread_tab_marker(&self) -> &str {
        if let Some(marker) = self.tab_unread_marker.as_deref() {
            return marker;
        }
        self.view_defs
            .iter()
            .find_map(|vd| vd.unread_marker.as_deref())
            .unwrap_or(DEFAULT_TAB_UNREAD_MARKER)
    }

    /// The style patch the tab bar layers over this tab's normal label style
    /// while the view is unread. Unconfigured → bold, nothing else; the
    /// bar's active/inactive colors stay untouched unless `tab.unread_style`
    /// names one.
    pub fn unread_tab_style(&self) -> ratatui::style::Style {
        use ratatui::style::{Modifier, Style};
        let Some(cfg) = self.tab_unread_style.as_ref() else {
            return Style::default().add_modifier(Modifier::BOLD);
        };
        let style = Style::default().add_modifier(cfg.modifiers());
        match cfg.fg() {
            Some(name) => style.fg(resolve_theme_color(&self.theme, name)),
            None => style,
        }
    }

    /// Human-readable scope label for the focused pane's current level,
    /// e.g. `jira` at the root or `jira › comments` when drilled in. The
    /// root node type stands in for the tab itself and is dropped.
    pub fn context_scope_label(&self) -> String {
        let pane = &self.pane_trees[self.active_subtab].focused_leaf().pane;
        let child: Vec<String> = pane
            .view_path_node_types(&self.view_defs)
            .into_iter()
            .skip(1)
            .collect();
        crate::keymap::leaf_scope_label(&self.tab_name, &child)
    }

    /// Shortcut-menu rows for the focused pane's current level: the live
    /// keymap projected to rows, plus the keyless actions available here.
    /// Keyless actions run via the action menu (not a key) and so are not in
    /// the keymap; they are appended with an empty keys column so the menu
    /// is a complete inventory of what the level offers.
    pub fn context_shortcut_rows(&self) -> Vec<crate::keymap::ShortcutRow> {
        let label = self.context_scope_label();
        let mut rows = crate::keymap::shortcut_rows(&self.context_keymap(), &label);
        let pane = &self.pane_trees[self.active_subtab].focused_leaf().pane;
        let mut seen: std::collections::HashSet<String> =
            rows.iter().map(|r| r.name.clone()).collect();

        // Node `shortcuts:` (e.g. `s: toggle-tracking`) dispatch through the
        // node-action path, not the pane's live keymap, so the projection
        // above misses them. Append the ones that apply at the current level:
        // a shortcut defined at child-name path `P` is live at `P` and every
        // level below it, with the nearest definition winning per key. These
        // carry a real `NodeShortcut` source, so they stay editable.
        let view_name = pane
            .view_def(&self.view_defs)
            .map(|vd| vd.name.clone())
            .unwrap_or_default();
        let current = pane.current_child_name_path();
        let mut by_key: std::collections::HashMap<String, crate::keymap::KeyClaim> =
            std::collections::HashMap::new();
        for claim in crate::keymap::node_shortcut_claims(&self.tab_name, &self.view_defs) {
            let KeySource::NodeShortcut {
                view,
                child_path,
                key,
                ..
            } = &claim.source
            else {
                continue;
            };
            // Only this focused subtab's view, and only ancestor-or-self
            // levels (child_path a prefix of the current drilldown path).
            if *view != view_name {
                continue;
            }
            if child_path.len() > current.len() || current[..child_path.len()] != child_path[..] {
                continue;
            }
            match by_key.get(key) {
                Some(existing)
                    if matches!(
                        &existing.source,
                        KeySource::NodeShortcut { child_path: cp, .. } if cp.len() >= child_path.len()
                    ) => {}
                _ => {
                    by_key.insert(key.clone(), claim.clone());
                }
            }
        }
        for claim in by_key.into_values() {
            let name = claim.source.action_name();
            if seen.insert(name.clone()) {
                rows.push(crate::keymap::ShortcutRow {
                    name,
                    keys: claim.key.0.join(" / "),
                    scope: label.clone(),
                    source: Some(claim.source.clone()),
                    key_scope: Some(claim.scope.clone()),
                });
            }
        }

        // Adapter-declared actions are the source of truth for what the
        // focused node can do — the YAML `shortcuts:` map above only *binds
        // keys* to a subset of them. Enumerate the adapter's actions for the
        // current level's node type and surface any that no shortcut binds yet
        // as keyless, bindable rows (a `NodeShortcut` source with an empty
        // key). This is why an adapter action like `toggle-tracking` now shows
        // up — and can be bound — even in a tab whose view file never mentions
        // it. Bound ones were already emitted above and dedup out by name.
        if let (Some(adapter), Some(node_type)) = (
            self.adapter.as_deref(),
            pane.selected_target_node_type(&self.view_defs),
        ) {
            let nt = not_yet_done_content::NodeType {
                type_id: node_type,
                mime_type: String::new(),
                syntax: None,
                file_extension: String::new(),
                display_name: String::new(),
            };
            for action in adapter.actions_for_type(&nt) {
                if not_yet_done_content::describe::is_builtin(&action.id) {
                    continue;
                }
                let source = KeySource::NodeShortcut {
                    view: view_name.clone(),
                    child_path: current.clone(),
                    key: String::new(),
                    action: action.id.clone(),
                };
                let name = source.action_name();
                if seen.insert(name.clone()) {
                    rows.push(crate::keymap::ShortcutRow {
                        name,
                        keys: String::new(),
                        scope: label.clone(),
                        source: Some(source),
                        key_scope: Some(crate::keymap::node_shortcut_scope(
                            &self.tab_name,
                            &current,
                        )),
                    });
                }
            }
        }

        for action in pane.current_actions(&self.view_defs) {
            if action.key.is_none() && seen.insert(action.name.clone()) {
                rows.push(crate::keymap::ShortcutRow {
                    name: action.name.clone(),
                    keys: String::new(),
                    scope: label.clone(),
                    // Menu-only actions appended here (not projected as keymap
                    // claims) carry no claim source; the keymap already emits
                    // keyless YAML actions with a routable source, so these
                    // extras stay read-only rather than risk a wrong path.
                    source: None,
                    key_scope: None,
                });
            }
        }
        rows
    }

    /// Every node-shortcut row across *all* declared levels of this view's
    /// tree: the keys already bound in the `shortcuts:` maps *and* the
    /// adapter-declared actions that nothing binds yet (keyless, bindable).
    ///
    /// The shortcut menu's "All tabs" / "Unbound" scopes call this so an
    /// unbound adapter action (e.g. `toggle-tracking`) is listed — and can be
    /// bound — from any tab, not just the focused drilldown level. It mirrors
    /// the node-shortcut portion of [`Self::context_shortcut_rows`] but walks
    /// the whole declared tree instead of the single focused level, keying the
    /// adapter lookup off each level's *configured* `node_type` (no live
    /// selection needed).
    pub fn all_node_shortcut_rows(&self) -> Vec<crate::keymap::ShortcutRow> {
        let mut rows = Vec::new();
        // Node shortcuts are declared per subtab (view) *and* per drill level,
        // so a tab with several subtabs (e.g. Trackings' `trackings` /
        // `condensed` / `tree`) exposes the same adapter action — say
        // `toggle-tracking` — once per subtab, each independently bindable.
        // The display scope only carried tab + child path, so those rows
        // collapsed to one under dedup and the `subtasks` drill level gave no
        // hint which subtab it belonged to. When the tab has more than one
        // subtab, fold the subtab name into the scope path so every subtab
        // gets its own labelled, bindable row (`Trackings › tree › subtasks`).
        let multi_subtab = self.view_defs.len() > 1;
        let scope_label = |view: &str, child_path: &[String]| -> String {
            if multi_subtab {
                let mut parts = Vec::with_capacity(child_path.len() + 1);
                parts.push(view.to_string());
                parts.extend(child_path.iter().cloned());
                crate::keymap::leaf_scope_label(&self.tab_name, &parts)
            } else {
                crate::keymap::leaf_scope_label(&self.tab_name, child_path)
            }
        };
        // Keys already bound in the `shortcuts:` maps, at every level. These
        // dispatch through the node-action path, so `build_leaf_maps_for`
        // (the pane keymap) never emits them — we add them here.
        let mut bound: std::collections::HashSet<(String, Vec<String>, String)> =
            std::collections::HashSet::new();
        for claim in crate::keymap::node_shortcut_claims(&self.tab_name, &self.view_defs) {
            let KeySource::NodeShortcut {
                view,
                child_path,
                action,
                ..
            } = &claim.source
            else {
                continue;
            };
            bound.insert((view.clone(), child_path.clone(), action.clone()));
            rows.push(crate::keymap::ShortcutRow {
                name: claim.source.action_name(),
                keys: claim.key.0.join(" / "),
                scope: scope_label(view, child_path),
                source: Some(claim.source.clone()),
                key_scope: Some(claim.scope.clone()),
            });
        }

        // Adapter-declared actions that no shortcut binds yet, at every level.
        let Some(adapter) = self.adapter.as_deref() else {
            return rows;
        };
        let mut levels: Vec<(String, Vec<String>, String)> = Vec::new();
        for view in &self.view_defs {
            collect_declared_levels(
                &view.name,
                &[],
                &view.node_type,
                &view.children,
                &mut levels,
            );
        }
        for (view_name, child_path, node_type) in levels {
            let nt = not_yet_done_content::NodeType {
                type_id: node_type,
                mime_type: String::new(),
                syntax: None,
                file_extension: String::new(),
                display_name: String::new(),
            };
            for action in adapter.actions_for_type(&nt) {
                if not_yet_done_content::describe::is_builtin(&action.id) {
                    continue;
                }
                if bound.contains(&(view_name.clone(), child_path.clone(), action.id.clone())) {
                    continue;
                }
                let source = KeySource::NodeShortcut {
                    view: view_name.clone(),
                    child_path: child_path.clone(),
                    key: String::new(),
                    action: action.id.clone(),
                };
                rows.push(crate::keymap::ShortcutRow {
                    name: source.action_name(),
                    keys: String::new(),
                    scope: scope_label(&view_name, &child_path),
                    source: Some(source),
                    key_scope: Some(crate::keymap::node_shortcut_scope(
                        &self.tab_name,
                        &child_path,
                    )),
                });
            }
        }
        rows
    }

    /// Cycle the active subtab forward (`forward = true`) or backward,
    /// wrapping around. Mirrors the per-view YAML switch key
    /// ([`dispatch_view_claim`](Self::dispatch_view_claim)'s `YamlSubtab`
    /// branch): it changes `active_subtab` and, when the destination pane has
    /// never been populated, asks the app to spawn its load (respecting
    /// `manual_connect`). Returns `None` — a no-op — when the tab has fewer
    /// than two subtabs.
    pub fn cycle_subtab(&mut self, forward: bool) -> Option<SubViewMessage> {
        let n = self.view_defs.len();
        if n < 2 {
            return None;
        }
        let target = if forward {
            (self.active_subtab + 1) % n
        } else {
            (self.active_subtab + n - 1) % n
        };
        let needs_load = self.switch_to_view(target);
        if needs_load && (!self.manual_connect || self.connected_once) {
            let pane_id = self.active_pane_id();
            Some(SubViewMessage::Request(ViewRequest::SpawnContentLoad {
                view_index: self.view_index,
                pane_id,
            }))
        } else {
            Some(SubViewMessage::SelectionChanged(None))
        }
    }

    fn dispatch_view_claim(&mut self, source: &KeySource) -> Option<SubViewMessage> {
        match source {
            KeySource::YamlSubtab { view } => {
                let target = self.view_defs.iter().position(|vd| vd.name == *view)?;
                if target == self.active_subtab {
                    // Already on this subtab — consume the key.
                    return Some(SubViewMessage::SelectionChanged(None));
                }
                let needs_load = self.switch_to_view(target);
                // Auto-load the destination subtab when it has never been
                // populated — unless this is a `manual_connect` tab that has
                // not connected yet. Once any subtab has connected, the shared
                // adapter connection serves every sibling, so switching loads
                // them transparently (no second "press … to connect").
                if needs_load && (!self.manual_connect || self.connected_once) {
                    let pane_id = self.active_pane_id();
                    Some(SubViewMessage::Request(ViewRequest::SpawnContentLoad {
                        view_index: self.view_index,
                        pane_id,
                    }))
                } else {
                    Some(SubViewMessage::SelectionChanged(None))
                }
            }
            KeySource::YamlMenuKey { .. } => {
                self.open_query_popup();
                Some(SubViewMessage::SelectionChanged(None))
            }
            KeySource::SavedQueryShortcut { name, .. } => {
                let sq = self
                    .db_saved_queries
                    .iter()
                    .find(|sq| sq.name == *name)
                    .cloned()?;
                let pane_id = self.active_pane_id();
                Some(SubViewMessage::Request(
                    ViewRequest::ApplyContentSavedQuery {
                        view_index: self.view_index,
                        pane_id,
                        query: sq.query,
                        name: sq.name,
                        kind: sq.kind,
                    },
                ))
            }
            KeySource::Content(ContentAction::GroupMenu) => {
                self.open_group_menu();
                Some(SubViewMessage::SelectionChanged(None))
            }
            KeySource::Content(ContentAction::OpenScriptsMenu) => {
                let view_index = self.view_index;
                let pane_id = self.active_pane_id();
                let node_id = self.target_node_script_node_id()?;
                Some(SubViewMessage::Request(ViewRequest::OpenNodeScriptsMenu {
                    view_index,
                    pane_id,
                    node_id,
                }))
            }
            KeySource::NodeScriptShortcut { node_id, script } => {
                Some(self.dispatch_node_script_apply(node_id.clone(), script.clone()))
            }
            KeySource::ScriptShortcut { name, .. } => {
                let view_index = self.view_index;
                let pane_id = self.active_pane_id();
                Some(SubViewMessage::Request(ViewRequest::RunScriptShortcut {
                    view_index,
                    pane_id,
                    name: name.clone(),
                }))
            }
            KeySource::Content(ContentAction::ToggleRecordDetail) => {
                Some(self.toggle_record_detail())
            }
            KeySource::Content(ContentAction::ToggleDetailWrap) => Some(self.toggle_detail_wrap()),
            KeySource::Content(ContentAction::ToggleGroupOrder) => {
                // Same latent gap as ToggleLongText below: the claim is pushed
                // in `build_view_claims`, but with no arm here the single-key
                // path fell through to the pane (which never claims `o`) and
                // did nothing. The `dispatch_content_action` arm only served
                // the chained / cmdline path.
                let view_defs = self.view_defs.clone();
                let view_index = self.view_index;
                let pane_id = self.active_pane_id();
                Some(
                    self.active_pane_mut()
                        .try_toggle_group_order(&view_defs, view_index, pane_id),
                )
            }
            KeySource::Content(ContentAction::ToggleLongText) => {
                // View-level dispatch of the pane toggle (like ToggleDetailWrap
                // above): the single-key path routes here, so the arm must
                // exist or `v` falls through to the pane — which never claims
                // it — and is silently unhandled.
                let view_defs = self.view_defs.clone();
                let view_index = self.view_index;
                let pane_id = self.active_pane_id();
                Some(
                    self.active_pane_mut()
                        .try_toggle_long_text(&view_defs, view_index, pane_id),
                )
            }
            KeySource::Content(ContentAction::ToggleCardMode) => Some(self.toggle_card_mode()),
            _ => None,
        }
    }

    pub(crate) fn active_view_name(&self) -> String {
        self.view_defs
            .get(self.active_subtab)
            .map(|vd| vd.name.clone())
            .unwrap_or_default()
    }

    /// Set the jump-mode label alphabet (from `navigation.jump_chars`).
    /// Stored on the view; applied to the focused pane's table when jump
    /// mode opens, so dynamically-created panes (splits/drills) inherit it.
    pub fn set_nav_chars(&mut self, chars: &[char]) {
        self.nav_chars = chars.to_vec();
    }

    pub fn action_bar_hints(&self) -> Vec<ActionHint> {
        if self.window_pending.is_some() {
            return self.window_mode_hints();
        }
        let mut hints = self.active_pane().action_bar_hints(
            &self.view_defs,
            self.query_menu_key(),
            &self.content_kb,
            &self.key_icons,
            self.adapter.as_deref(),
        );
        // Postgres scripts-menu (`q queries`) hint — gated on the same
        // condition as the keybinding claim in `build_view_claims`.
        // Per-node-action shortcut hints (e.g. `Q sql`) are not yet
        // surfaced here; the bindings work via the async shortcut
        // dispatcher in `ContentPane::try_node_action_shortcut`. Reuses the
        // shared `query_menu` popup, hence `ActiveSurface::QueryMenu`.
        if self.target_node_script_node_id().is_some() {
            let q_key = self
                .content_kb
                .hint_label(&ContentAction::OpenScriptsMenu, &self.key_icons);
            if !hints.iter().any(|h| h.key == q_key) {
                hints.push(ActionBarHint::new(
                    q_key,
                    "queries",
                    ActiveSurface::QueryMenu,
                ));
            }
        }
        // Group-by menu hint — same gate as the claim in `build_view_claims`
        // (native trackings shows `u group` too).
        if self.active_pane().level_has_group_by(&self.view_defs) {
            let u_key = self
                .content_kb
                .hint_label(&ContentAction::GroupMenu, &self.key_icons);
            if !hints.iter().any(|h| h.key == u_key) {
                hints.push(ActionBarHint::new(u_key, "group", ActiveSurface::GroupMenu));
            }
        }
        // Column-config hint — `c` opens the generic column-config popup
        // whenever the active level exposes configurable columns. Gating on
        // `column_config_entries().is_some()` mirrors `open_column_config_popup`.
        if self.column_config_entries().is_some() {
            let c_key = self
                .common_kb
                .hint_label(&CommonAction::ColumnConfig, &self.key_icons);
            if !hints.iter().any(|h| h.key == c_key) {
                hints.push(ActionBarHint::new(
                    c_key,
                    "columns",
                    ActiveSurface::ColumnConfig,
                ));
            }
        }
        // Jump-mode hint — always claimable (native Tasks tab shows `p jump`;
        // here it defaults to `J`).
        let jump_key = self
            .content_kb
            .hint_label(&ContentAction::JumpMode, &self.key_icons);
        if !hints.iter().any(|h| h.key == jump_key) {
            hints.push(ActionBarHint::new(jump_key, "jump", ActiveSurface::Jump));
        }

        // Resolve each hint's `active` flag from its build-time `ActiveSurface`
        // against live UI state — the single resolver replaces the old
        // per-description string matching.
        let mut resolved: Vec<ActionHint> = hints
            .into_iter()
            .map(|h| {
                let active = self.resolve_active(&h.source);
                ActionHint {
                    key: h.key,
                    desc: h.label,
                    active,
                }
            })
            .collect();
        // App-global activatable hints (the shortcut menu today) belong here
        // with the rest of the activatable shortcuts, not in the tab bar.
        // Their `active` flag is resolved by the App (which owns the surface)
        // before it hands them in, so the view just appends them, skipping any
        // key already claimed by a view hint.
        for gh in &self.global_action_hints {
            if resolved.iter().any(|h| h.key == gh.key) {
                continue;
            }
            resolved.push(gh.clone());
        }
        resolved
    }

    /// Resolve whether an action-bar hint with the given [`ActiveSurface`] is
    /// currently active, reading live UI state (popups open, modes armed,
    /// editor focused). The cross-cutting App-owned flags
    /// (`active_editor` / `tracking_active` / `cut_active` /
    /// `column_config_active` / `confirm_active` / `script_active`) are
    /// pushed in once per frame by [`Self::sync_action_bar`]; the rest live
    /// on this view or its focused pane.
    fn resolve_active(&self, source: &ActiveSurface) -> bool {
        match source {
            ActiveSurface::Editor(label) => self.active_editor.as_deref() == Some(label.as_str()),
            ActiveSurface::Confirm => self.confirm_active,
            ActiveSurface::QueryMenu => self.query_menu.is_open(),
            ActiveSurface::GroupMenu => self.group_menu.is_open(),
            ActiveSurface::ColumnConfig => self.column_config_active,
            ActiveSurface::Fuzzy => self.active_pane().table.fuzzy_active,
            ActiveSurface::Search => {
                // Local `/`-search and tree-find only. The adapter text search
                // borrows the same input widget, so exclude its mode here —
                // otherwise `/` would light up while an `f s` term is typed.
                let pane = self.active_pane();
                (pane.search.active() && !pane.text_search_input_open()) || pane.tree_find_active()
            }
            ActiveSurface::TextSearch => {
                let pane = self.active_pane();
                pane.text_search_input_open() || pane.text_search_applied()
            }
            ActiveSurface::Jump => self.active_pane().table.jump_active(),
            ActiveSurface::Tracking => self.tracking_active,
            ActiveSurface::MarkMove => self.cut_active,
            ActiveSurface::Script => self.script_active,
            ActiveSurface::ContentAction(id) => {
                // Lit while the target picker popup for this action is open, or
                // the editor it opened is focused (its action id equals `id` or
                // is prefixed `"<id>:"`, so `convert` covers `convert:userstory`).
                self.content_action_popup_id.as_deref() == Some(id.as_str())
                    || self
                        .content_editor_action_id
                        .as_deref()
                        .is_some_and(|eid| eid == id || eid.starts_with(&format!("{id}:")))
            }
            // App-native surfaces are owned by `App`, never by a content view,
            // and are never carried on a content action-bar hint. From within a
            // view they are definitionally inactive. Kept as an explicit arm
            // (no wildcard) so a new app-native surface forces a decision here.
            ActiveSurface::ShortcutMenu => false,
        }
    }

    /// Hints rendered while the window-leader chord is pending. Lists
    /// the resolution key for each `WindowAction` plus a pane-tag-switch
    /// reminder when the active subtab has more than one pane.
    fn window_mode_hints(&self) -> Vec<ActionHint> {
        // Window-leader chord prompts — momentary key hints, never "active".
        // Iterating `WindowAction::ALL` (rather than the binding map) fixes the
        // order to the enum's declaration order and makes a newly added window
        // action show up here on its own. Labels come from the shared
        // `window_nav_hint`, so this prompt and the always-on status-bar
        // listing can never disagree.
        let mut hints: Vec<ActionHint> = Vec::new();
        for action in WindowAction::ALL {
            let Some(binding) = self.window_kb.get(action) else {
                continue;
            };
            let Some(last) = binding.0.first().and_then(|s| s.chars().last()) else {
                continue;
            };
            hints.push(ActionHint::new(
                last.to_string(),
                window_nav_hint(action).label,
            ));
        }
        let tree = &self.pane_trees[self.active_subtab];
        if tree.pane_tags.len() > 1 {
            let mut tags: Vec<char> = tree.pane_tags.values().copied().collect();
            tags.sort();
            let tags_str: String = tags.iter().collect();
            hints.push(ActionHint::new(tags_str, "switch pane"));
        }
        hints
    }

    /// Mode label shown at the very start of the action bar — currently
    /// only `Some("WINDOW")` while the window-leader chord is pending.
    fn action_bar_mode_label(&self) -> Option<String> {
        self.window_pending.as_ref().map(|_| "WINDOW".to_string())
    }

    pub fn status_bar_hints(&self) -> Vec<BarHint> {
        let mut hints = self.active_pane().status_bar_hints(
            &self.view_defs,
            &self.common_kb,
            &self.content_kb,
            &self.key_icons,
            self.adapter.as_deref(),
        );
        // Record-detail toggle / wrap hints. These derive from the view-level
        // claim (`build_view_claims`), so the pane's nav-hint loop — which
        // only sees the pane's own `build_claims` — can't surface them; we
        // add them here under the very same gate as the claim. The label
        // flips to reflect the live toggle state (open vs close, wrap vs
        // no-wrap) so the bar reads as the action the key performs next.
        let pane = self.active_pane();
        let can_open = pane.record_detail_enabled(&self.view_defs);
        let detail_in_play = pane.detail_child.is_some() || pane.is_detail_pane();
        if can_open || detail_in_play {
            if let Some(b) = self.content_kb.get(&ContentAction::ToggleRecordDetail) {
                let label = if detail_in_play {
                    "close detail"
                } else {
                    "detail"
                };
                hints.push((b.hint_label(&self.key_icons), label.to_string()));
            }
        }
        if detail_in_play {
            if let Some(b) = self.content_kb.get(&ContentAction::ToggleDetailWrap) {
                let label = if pane.detail_wrap { "no-wrap" } else { "wrap" };
                hints.push((b.hint_label(&self.key_icons), label.to_string()));
            }
        }
        // Group-order toggle (`o`) — same view-level claim/gate as
        // `build_view_claims` (groupable level, and `o` not owned by a
        // record-detail split). The label carries the *current* group order
        // direction so the bar reflects state at a glance (↓ newest-first /
        // ↑ oldest-first); pressing `o` flips it.
        let detail_owns_o = can_open || detail_in_play;
        if pane.level_has_group_by(&self.view_defs) && !detail_owns_o {
            if let Some(gb) = pane.current_group_by(&self.view_defs) {
                if let Some(b) = self.content_kb.get(&ContentAction::ToggleGroupOrder) {
                    let arrow = match gb.order {
                        GroupOrder::Desc => "↓",
                        GroupOrder::Asc => "↑",
                    };
                    hints.push((b.hint_label(&self.key_icons), format!("order {arrow}")));
                }
            }
        }
        // Long-text toggle (`v`) — offered only when a column declares
        // `long_source`. The label reflects the current mode so the bar shows
        // state at a glance.
        if pane.long_text_available(&self.view_defs) {
            if let Some(b) = self.content_kb.get(&ContentAction::ToggleLongText) {
                let label = if pane.long_text { "short" } else { "long" };
                hints.push((b.hint_label(&self.key_icons), label.to_string()));
            }
        }
        // Card-mode toggle — same gate and key as the view-level claim. The
        // label names the mode the key switches *to*, so the bar reads as the
        // action rather than the state.
        if let Some(b) = pane.card_toggle_binding(&self.view_defs, &self.content_kb) {
            let label = if pane.card_mode_active(&self.view_defs) {
                "table"
            } else {
                "cards"
            };
            hints.push((b.hint_label(&self.key_icons), label.to_string()));
        }
        // Window/split chords (`wv` / `ws` / `wq` / `wh` / `wl` by default),
        // listed with their full chord just like the tree-fold chords. Until
        // now they only surfaced *after* the leader was already pressed (the
        // WINDOW-mode action bar), so there was nothing to discover them from.
        //
        // The gate is the one `handle_window_chord` uses — the active view's
        // `window_ops: true` — so the bar lists a chord exactly when that
        // chord would fire. They live in the status bar because splitting or
        // refocusing a pane arms no mode and opens no popup, so they can never
        // light up (see the action-bar contract in `content_action_hints`).
        //
        // Deliberately absent: `w<tag>` pane-tag switching. Its resolution key
        // is a per-pane letter handed out by the current split layout, not a
        // stable binding, so it stays in the WINDOW-mode action bar, which
        // shows the live tags.
        if self
            .active_view_def()
            .map(|v| v.window_ops)
            .unwrap_or(false)
        {
            for action in WindowAction::ALL {
                let Some(b) = self.window_kb.get(action) else {
                    continue;
                };
                let key = b.hint_label(&self.key_icons);
                if hints.iter().any(|(k, _)| k == &key) {
                    continue;
                }
                hints.push((key, window_nav_hint(action).label.to_string()));
            }
        }
        hints
    }
}

// ── SortableView / PaginatedView ─────────────────────────────────────

impl SortableView for ContentView {
    fn columns(&self) -> Vec<not_yet_done_content::ColumnSchema> {
        self.active_pane().last_columns.clone()
    }

    fn current_sort(&self) -> &[SortKey] {
        ContentView::current_sort(self)
    }

    fn set_current_sort(&mut self, sort: Vec<SortKey>) -> bool {
        ContentView::set_current_sort(self, sort)
    }

    fn last_applied_sort(&self) -> &[SortKey] {
        ContentView::last_applied_sort(self)
    }
}

impl PaginatedView for ContentView {
    fn current_page(&self) -> Option<PageRequest> {
        ContentView::current_page(self)
    }

    fn set_current_page(&mut self, page: Option<PageRequest>) -> bool {
        ContentView::set_current_page(self, page)
    }

    fn last_page_info(&self) -> Option<PageInfo> {
        ContentView::last_page_info(self)
    }

    fn next_page_request(&self) -> Option<PageRequest> {
        ContentView::next_page_request(self)
    }

    fn prev_page_request(&self) -> Option<PageRequest> {
        ContentView::prev_page_request(self)
    }
}

// ── Searchable ───────────────────────────────────────────────────────

impl Searchable for ContentView {
    fn search_active(&self) -> bool {
        self.active_pane().search.active()
    }
    fn search_state(&self) -> SearchState {
        self.active_pane().search.state()
    }
    fn search_open(&mut self) {
        self.active_pane_mut().search.open();
    }
    fn search_close(&mut self) {
        self.active_pane_mut().search.close();
    }
    fn search_clear(&mut self) {
        self.active_pane_mut().search.clear();
    }
    fn search_handle_key(&mut self, key: &str) -> SearchKeyResult {
        let view_defs = &self.view_defs;
        let pane = &mut self.pane_trees[self.active_subtab].focused_leaf_mut().pane;
        let result = pane.search.handle_key(key);
        if matches!(result, SearchKeyResult::QueryChanged) {
            let descs = pane.search_descriptions(view_defs);
            let refs: Vec<(usize, &str)> = descs.iter().map(|(i, s)| (*i, s.as_str())).collect();
            pane.search.update_matches(&refs);
            if let Some(row) = pane.search.first_match() {
                pane.table.set_selected(row);
            }
        }
        result
    }
    fn search_jump(&mut self, direction: isize) {
        let pane = self.active_pane_mut();
        if let Some(row) = pane.search.jump(direction) {
            pane.table.set_selected(row);
        }
    }
}

// ── HasCmdline ───────────────────────────────────────────────────────

impl HasCmdline for ContentView {
    fn cmdline_active(&self) -> bool {
        self.cmdline.active()
    }

    fn cmdline_state(&self) -> CmdlineState {
        self.cmdline.state()
    }

    fn cmdline_open(&mut self) {
        self.cmdline.open();
    }

    fn cmdline_open_with(&mut self, prefill: &str) {
        self.cmdline.open_with(prefill);
    }

    fn cmdline_close(&mut self) {
        self.cmdline.close();
    }

    fn cmdline_handle_key(&mut self, key: &str) -> CmdlineKeyResult {
        self.cmdline.handle_key(key)
    }
}

// ── Component ────────────────────────────────────────────────────────

impl Component for ContentView {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        // Reserve a top line for the auth-status banner when set.
        let banner_text = self.auth_status_banner();
        let (banner_area, after_banner) = if banner_text.is_some() {
            let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(area);
            (Some(chunks[0]), chunks[1])
        } else {
            (None, area)
        };

        // Reserve a line for breadcrumbs when drilled down (active pane).
        let drilled = !self.active_pane().nav_stack.is_empty();
        let (breadcrumb_area, content_area) = if drilled {
            let chunks =
                Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(after_banner);
            (Some(chunks[0]), chunks[1])
        } else {
            (None, after_banner)
        };

        // Render auth-status banner.
        if let (Some(bn_area), Some(text)) = (banner_area, banner_text) {
            let t = &*self.theme;
            let is_failure = self.adapter_init_error.is_some()
                || matches!(self.auth_status, AdapterStatus::Failed { .. })
                || self.active_pane().fetch_error.is_some();
            let style = if is_failure {
                Style::default().fg(t.error())
            } else {
                Style::default().fg(t.accent())
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(text, style))),
                bn_area,
            );
        }

        // Render breadcrumbs.
        if let Some(bc_area) = breadcrumb_area {
            self.active_pane()
                .render_breadcrumbs(frame, bc_area, &self.view_defs);
        }

        // Reserve one line at the bottom for the pagination footer when set.
        let (content_area, page_footer_area) = if self.active_pane().last_page_info.is_some() {
            let chunks =
                Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(content_area);
            (chunks[0], Some(chunks[1]))
        } else {
            (content_area, None)
        };

        // Recursively render the pane tree. Multi-leaf trees get focus
        // borders; single-leaf trees render unchanged.
        let theme = Arc::clone(&self.theme);
        let overlay = self.header_overlay.clone();
        let active_subtab = self.active_subtab;
        let tree = &mut self.pane_trees[active_subtab];
        let multi = tree.leaf_count() > 1;
        let focused_id = tree.focus;
        tree.last_rects.clear();
        let PaneTree {
            root,
            last_rects,
            pane_tags,
            ..
        } = tree;
        root.render(
            frame,
            content_area,
            focused_id,
            multi,
            last_rects,
            pane_tags,
            &theme,
            &overlay,
        );

        // Render pagination footer (driven by focused pane).
        if let Some(area) = page_footer_area {
            self.active_pane().render_page_footer(frame, area);
        }

        // Overlay: query popup + group-by menu (tab-level).
        self.render_query_popup(frame, area);
        self.group_menu.render(frame, area);
    }

    fn query(&self, attr: tuirealm::props::Attribute) -> Option<tuirealm::props::QueryResult<'_>> {
        self.active_pane().table.query(attr)
    }

    fn attr(&mut self, attr: tuirealm::props::Attribute, value: tuirealm::props::AttrValue) {
        self.active_pane_mut().table.attr(attr, value);
    }

    fn state(&self) -> tuirealm::state::State {
        self.active_pane().table.state()
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        self.active_pane_mut().table.perform(cmd)
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Resolve a column's value from the underlying [`NodeSummary`]. Returns
/// `&'a str` so the result can be threaded into the table-build pipeline
/// without allocation.
///
/// Note: the synthetic `source: "has_links"` column is *not* handled
/// here — it depends on the pane's `link_refs` cache + adapter NodeRef
/// prefix, neither of which this stateless helper has access to.
/// Call sites that build render rows special-case `"has_links"`
/// upstream of the call to this function (see flat-mode and tree-mode
/// row builders inside `rebuild_table_with` / `build_tree_data_rows`).
fn column_value<'a>(item: &'a NodeSummary, col: &ColumnDef) -> &'a str {
    if col.source.as_deref() == Some("label") {
        return &item.label;
    }
    metadata_field_value(item, &col.key)
}

/// Raw value of an arbitrary metadata field by key (`""` when absent).
/// Used by `column_value` and by the `elapsed` cell, which reads a field
/// other than the column's own key (`elapsed_from`).
fn metadata_field_value<'a>(item: &'a NodeSummary, key: &str) -> &'a str {
    item.metadata
        .fields
        .iter()
        .find(|f| f.key == key)
        .map(|f| f.value.as_str())
        .unwrap_or("")
}

/// Display separator for a `path` column (default `/`).
fn path_separator(col: &ColumnDef) -> &str {
    col.separator.as_deref().unwrap_or("/")
}

/// Build the [`CellContent`] for a data cell, applying the column's typed
/// formatting (M2): `raw` is the adapter's canonical value string, which
/// [`format_typed_value`] turns into the display text plus alignment.
/// `kind: text` (every remote column) is the identity case — verbatim,
/// left-aligned.
fn typed_cell_content(raw: &str, col: &ColumnDef) -> CellContent {
    let (text, alignment) =
        format_typed_value(raw, col.kind, col.format.as_deref(), path_separator(col));
    CellContent::aligned(text, alignment)
}

/// Build a data cell, resolving live (time-derived) kinds against `now`
/// (M5 live-elapsed). For `kind: elapsed` the value is read from the
/// column's `elapsed_from` field (default: the column's own key) and
/// rendered as `now − that instant`; every other kind is the pure,
/// time-independent [`typed_cell_content`]. Called per row build, so
/// passing a fresh `now` on each repaint tick makes the timer advance.
fn cell_content_for(
    item: &NodeSummary,
    col: &ColumnDef,
    now: chrono::DateTime<chrono::Local>,
) -> CellContent {
    if col.kind == ColumnKind::Elapsed {
        let src_key = col.elapsed_from.as_deref().unwrap_or(col.key.as_str());
        let (text, alignment) = format_elapsed_since(metadata_field_value(item, src_key), now);
        return CellContent::aligned(text, alignment);
    }
    typed_cell_content(column_value(item, col), col)
}

/// Fallback column-layout width used only before the table's first paint,
/// when its real render width is not yet known. The very next draw records
/// the actual pane width and `refit_tables_if_needed` re-fits, so this value
/// is transient; it is kept generously wide so `Max` columns aren't starved
/// in the one-frame interim. (`Auto` columns ignore it entirely.)
const PRE_PAINT_TABLE_WIDTH: usize = 300;

/// StyleMap slot for the taskpath separator in the flat single-line table
/// (slot 0 is the sort-mode dim overlay). Referenced when building
/// `kind: path` cells; kept in sync with the `StyleMap::new(...)` in
/// `rebuild_table_with`.
const PATH_SEPARATOR_STYLE_ID: usize = 1;

/// StyleMap slot for group-header rows and the grand-total footer in a
/// grouped content view (M3). Painted in the theme's `group_header` color.
/// Kept in sync with the `StyleMap::new(...)` in `rebuild_table_with`.
const GROUP_HEADER_STYLE_ID: usize = 2;

/// StyleMap slot for the tree-mode connector run (box glyphs + expand arrow)
/// in the `tree_label` column. Painted in the per-view `tree_connector_style`
/// color (falling back to the theme `tree_connector`), resolved into the slot
/// by `content_style_map`. Kept in sync with the `StyleMap::new(...)` there.
const TREE_CONNECTOR_STYLE_ID: usize = 3;

/// StyleMap slot for fuzzy-match runs inside the `tree_label` column (the box
/// connector uses [`TREE_CONNECTOR_STYLE_ID`], so the matched substring needs
/// its own slot to render apart from it). Painted in the theme `accent` —
/// matching the native Tasks tree's match highlight and the engine's
/// `Highlight` style used for non-tree cells. Kept in sync with the
/// `StyleMap::new(...)` in `content_style_map`.
const FUZZY_MATCH_STYLE_ID: usize = 4;

/// StyleMap slot for the unread highlight in chat-style adapters (Stoat). A
/// tree row whose node carries an `unread` metadata field set to `"true"`
/// (a channel/category with unread messages) paints its label — and the
/// leading `unread_marker` glyph — in this slot; an unread message paints
/// its multi-line header line the same way. Painted in the per-view
/// `unread_style` color (falling back to the theme `unread`), resolved into
/// the slot by `content_style_map`. Kept in sync with the `StyleMap::new(...)`
/// there.
const UNREAD_STYLE_ID: usize = 5;

/// StyleMap slot for *deleted* rows. An adapter that keeps soft-deleted
/// records in its universe (e.g. the Tasks adapter, so a deleted parent
/// stays on screen as context for a matching child) marks them with a
/// `deleted` metadata field set to `"true"`; every cell of such a row is
/// painted in this slot — a dimmed (`text_dim`) foreground — so the row
/// reads as struck-through-without-the-line, present but greyed. Kept in
/// sync with the `StyleMap::new(...)` in `content_style_map`.
const DELETED_STYLE_ID: usize = 6;

/// Default leading marker glyph for unread chat items when a view sets no
/// `unread_marker`. `💬` (speech balloon) — a colorful, at-a-glance "new
/// message" cue. Emoji are two terminal cells wide; the tree-label builder
/// accounts for the rendered width when prefixing it.
const DEFAULT_UNREAD_MARKER: &str = "💬";

/// Default leading marker glyph for an unread **tab** label when neither
/// `tab.unread_marker` nor the view's `unread_marker` is set. `🔔` (bell)
/// rather than the row default: a tab already carries its own `icon:`, and
/// a speech balloon there would compete with the very glyph chat views use
/// to mark "this is a channel".
const DEFAULT_TAB_UNREAD_MARKER: &str = "🔔";

/// Split an already-fitted tree-label cell into a styled connector segment +
/// plain label. The first `connector_chars` characters (the `├──`/`└──`/`│`
/// box prefix and any `▶`/`▼` expand arrow) carry `connector_style_id`; the
/// remaining leaf glyph + label carry `None` (the cell's default style).
/// `connector_chars` is a *char* count (matches the `StyledSpan` range and the
/// engine's char-indexed highlight projection); it is converted to a byte
/// offset here. Mirrors [`path_cell_segments`] but splits at a fixed prefix
/// length instead of a separator.
/// `base_style_id` styles the label remainder (everything after the connector
/// prefix) — `None` leaves it the cell default, `Some(id)` paints it in that
/// slot (used for [`UNREAD_STYLE_ID`] so an unread channel's name + leading
/// marker glow in the unread color).
fn tree_label_cell_segments(
    fitted: &str,
    connector_chars: usize,
    connector_style_id: usize,
    base_style_id: Option<usize>,
) -> Vec<(String, Option<usize>)> {
    if connector_chars == 0 {
        return vec![(fitted.to_string(), base_style_id)];
    }
    let byte_idx = fitted
        .char_indices()
        .nth(connector_chars)
        .map(|(i, _)| i)
        .unwrap_or(fitted.len());
    let (connector, rest) = fitted.split_at(byte_idx);
    let mut segments = vec![(connector.to_string(), Some(connector_style_id))];
    if !rest.is_empty() {
        segments.push((rest.to_string(), base_style_id));
    }
    segments
}

/// Like [`tree_label_cell_segments`], but additionally splits the label part
/// (everything after the connector prefix) at the fuzzy-match `highlights` so
/// matched runs carry `highlight_style_id` (painted in the theme's match color,
/// the native Tasks tree's underline-less accent). `highlights` are **char**
/// ranges into the *fitted* cell (already projected / clamped by the table
/// engine), parallel to and disjoint from the connector run. Falls back to the
/// plain connector split when there is nothing to highlight, so the
/// non-filtering hot path stays identical.
/// `base_style_id` styles the non-connector, non-match runs (see
/// [`tree_label_cell_segments`]); the matched runs always win over it.
fn tree_label_segments_with_highlights(
    fitted: &str,
    connector_chars: usize,
    connector_style_id: usize,
    highlights: &[std::ops::Range<usize>],
    highlight_style_id: usize,
    base_style_id: Option<usize>,
) -> Vec<(String, Option<usize>)> {
    if highlights.is_empty() {
        return tree_label_cell_segments(
            fitted,
            connector_chars,
            connector_style_id,
            base_style_id,
        );
    }
    let chars: Vec<char> = fitted.chars().collect();
    let len = chars.len();
    let conn = connector_chars.min(len);
    let take = |range: std::ops::Range<usize>| -> String { chars[range].iter().collect() };

    let mut segments: Vec<(String, Option<usize>)> = Vec::new();
    if conn > 0 {
        segments.push((take(0..conn), Some(connector_style_id)));
    }
    // Walk the label remainder, emitting base-styled runs interleaved with the
    // matched runs. `highlights` arrive sorted and non-overlapping (merged at
    // build time, order preserved through the engine's projection); clamp each
    // into the post-connector window so a connector-overlapping range can't
    // double-style the prefix.
    let mut cursor = conn;
    for r in highlights {
        let start = r.start.max(conn).min(len);
        let end = r.end.max(conn).min(len);
        if start >= end {
            continue;
        }
        if start > cursor {
            segments.push((take(cursor..start), base_style_id));
        }
        segments.push((take(start..end), Some(highlight_style_id)));
        cursor = end;
    }
    if cursor < len {
        segments.push((take(cursor..len), base_style_id));
    }
    if segments.is_empty() {
        segments.push((fitted.to_string(), None));
    }
    segments
}

/// Char-index ranges within `label` that match the active fuzzy `filter_text`.
///
/// Mirrors the native Tasks tree highlight ([`fill_highlight_ranges`] in
/// `ui::tasks::highlight`): each whitespace-separated token is matched against
/// the label with `fuzzy_indices`, all matched char indices are unioned, and
/// consecutive indices collapse into contiguous ranges. Empty when the filter
/// is empty or the label itself carries no match (the row may have survived
/// the filter via another field — then nothing in the label is highlighted,
/// exactly as upstream).
fn fuzzy_label_ranges(label: &str, filter_text: &str) -> Vec<std::ops::Range<usize>> {
    use fuzzy_matcher::FuzzyMatcher;
    let filter = filter_text.trim();
    if filter.is_empty() {
        return Vec::new();
    }
    let matcher = fuzzy_matcher::skim::SkimMatcherV2::default();
    let mut indices: Vec<usize> = Vec::new();
    for token in filter.split_whitespace() {
        if let Some((_score, char_indices)) = matcher.fuzzy_indices(label, token) {
            indices.extend(char_indices);
        }
    }
    indices.sort_unstable();
    indices.dedup();
    merge_consecutive_char_indices(&indices)
}

/// Collapse a sorted list of matched char indices into contiguous ranges:
/// `[0, 1, 2, 5, 6]` → `[0..3, 5..7]`. Local mirror of the helper in
/// `ui::tasks::highlight` (kept private there).
fn merge_consecutive_char_indices(indices: &[usize]) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut iter = indices.iter().peekable();
    while let Some(&start) = iter.next() {
        let mut end = start + 1;
        while iter.peek().map(|&&next| next == end).unwrap_or(false) {
            iter.next();
            end += 1;
        }
        ranges.push(start..end);
    }
    ranges
}

/// Resolved per-view tree drawing options (see
/// [`ContentPane::tree_draw_options`]): whether the box-drawing line
/// connectors are drawn and which expand/collapse markers (possibly empty
/// when disabled) prefix expandable rows. Borrows the marker strings from
/// the view config they were resolved from.
struct TreeDrawOptions<'a> {
    lines: bool,
    collapsed_marker: &'a str,
    expanded_marker: &'a str,
}

/// Split an already-fitted `path`-column string into render segments,
/// tagging each run of the display `separator` with `sep_style_id` so the
/// renderer paints it in the theme's taskpath-separator style. Segment text
/// (and trailing alignment padding) carries `None` = the cell's default
/// style. Mirrors the legacy Trackings taskpath styling, but operates on the
/// post-fit string so column sizing stays the table engine's job.
fn path_cell_segments(
    fitted: &str,
    separator: &str,
    sep_style_id: usize,
) -> Vec<(String, Option<usize>)> {
    if separator.is_empty() {
        return vec![(fitted.to_string(), None)];
    }
    let mut segments: Vec<(String, Option<usize>)> = Vec::new();
    let mut rest = fitted;
    while let Some(pos) = rest.find(separator) {
        if pos > 0 {
            segments.push((rest[..pos].to_string(), None));
        }
        segments.push((separator.to_string(), Some(sep_style_id)));
        rest = &rest[pos + separator.len()..];
    }
    if !rest.is_empty() {
        segments.push((rest.to_string(), None));
    }
    segments
}

/// Build the haystack string used by both fuzzy_filter and `/`-search.
/// With no `fields` configured the haystack is `label` followed by every
/// metadata field value (space-joined). With explicit fields, only those
/// values are joined — column resolution falls back to `label` for the
/// special key and to raw metadata otherwise. Kept as a free helper so
/// tree mode and flat mode share the same matcher input.
fn build_field_haystack(node: &NodeSummary, columns: &[ColumnDef], fields: &[String]) -> String {
    if fields.is_empty() {
        let mut s = node.label.clone();
        for f in &node.metadata.fields {
            s.push(' ');
            s.push_str(&f.value);
        }
        return s;
    }
    let mut s = String::new();
    for key in fields {
        let value = columns
            .iter()
            .find(|c| c.key == *key)
            .map(|c| column_value(node, c))
            .unwrap_or_else(|| {
                if key == "label" {
                    &node.label
                } else {
                    node.metadata
                        .fields
                        .iter()
                        .find(|f| f.key == *key)
                        .map(|f| f.value.as_str())
                        .unwrap_or("")
                }
            });
        if !s.is_empty() {
            s.push(' ');
        }
        s.push_str(value);
    }
    s
}

// ── Grouping + aggregation render path (M3) ──────────────────────────────

/// The next runtime grouping state for the `cycle_grouping` action. Walks
/// `ungrouped → Day → Week → Month → Year → ungrouped`; `None` means
/// ungrouped. `grouped_now` distinguishes "currently off" from "currently on
/// with no bucket" so the very first press always lands on `Day`.
fn next_bucket_state(grouped_now: bool, current: Option<DateBucket>) -> Option<DateBucket> {
    if !grouped_now {
        return Some(DateBucket::Day);
    }
    match current {
        None => Some(DateBucket::Day),
        Some(DateBucket::Day) => Some(DateBucket::Week),
        Some(DateBucket::Week) => Some(DateBucket::Month),
        Some(DateBucket::Month) => Some(DateBucket::Year),
        Some(DateBucket::Year) => None,
    }
}

/// Original `items` indices passing the active fuzzy filter, in input order.
/// Factored out so the flat row builder and the grouped builder share one
/// matcher definition (SkimMatcherV2, whitespace-split AND of fuzzy tokens).
fn fuzzy_filtered_order(
    items: &[NodeSummary],
    columns: &[ColumnDef],
    filter_text: &str,
    fields: &[String],
) -> Vec<usize> {
    use fuzzy_matcher::FuzzyMatcher;
    let matcher = fuzzy_matcher::skim::SkimMatcherV2::default();
    let tokens: Vec<&str> = filter_text.split_whitespace().collect();
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            if tokens.is_empty() {
                return true;
            }
            let haystack = build_field_haystack(item, columns, fields);
            tokens
                .iter()
                .all(|tok| matcher.fuzzy_match(&haystack, tok).is_some())
        })
        .map(|(i, _)| i)
        .collect()
}

/// Raw canonical value of a column referenced by `key`. Resolves through the
/// matching [`ColumnDef`] (so `source: label` is honoured) and falls back to
/// a bare metadata-field lookup when the key names a field that is not a
/// displayed column. Used by grouping/aggregation, which address columns by
/// `key` (the group-by column, the aggregated columns) rather than by index.
fn raw_value_by_key<'a>(item: &'a NodeSummary, columns: &[ColumnDef], key: &str) -> &'a str {
    if let Some(col) = columns.iter().find(|c| c.key == key) {
        column_value(item, col)
    } else {
        metadata_field_value(item, key)
    }
}

/// Render an aggregate's integer total back through the column's typed
/// formatter, so a `duration` total prints `H:MM:SS` and a `number` total
/// prints verbatim — matching the data cells in the same column.
fn format_total(total: i64, col: &ColumnDef) -> String {
    let (text, _) = format_typed_value(
        &total.to_string(),
        col.kind,
        col.format.as_deref(),
        path_separator(col),
    );
    text
}

/// One group-header / summary / grand-total row: a spanning label across the
/// columns left of the first totalled column, then each total right-aligned
/// in its own column, with the rest blank. `totals` pairs a column index with
/// the value rendered there (aggregates routed to a `total_column` are simply
/// absent here — their totals live on data rows instead). The whole row
/// paints in the group-header style and is non-selectable.
fn summary_row(
    label: String,
    totals: &[(usize, i64)],
    columns: &[ColumnDef],
    col_widths: &[usize],
) -> TableWidgetRow {
    // The widget renders pre-fitted text and never pads a spanning cell
    // itself, so the label must be padded to the spanned columns' combined
    // width here — otherwise the first total renders right after the label
    // instead of aligned under its column. Matches the grouped path's
    // two-space column separator.
    const SEP_WIDTH: usize = 2;
    let ncols = columns.len();
    let first_agg = totals.iter().map(|&(ci, _)| ci).min().unwrap_or(ncols);
    let label_span = first_agg.max(1);
    let width_of = |ci: usize| col_widths.get(ci).copied().unwrap_or(0);

    let spanned: usize = (0..label_span.min(ncols)).map(width_of).sum::<usize>()
        + SEP_WIDTH * label_span.min(ncols).saturating_sub(1);
    let label = if label.chars().count() < spanned {
        let pad = spanned - label.chars().count();
        format!("{label}{}", " ".repeat(pad))
    } else {
        label
    };

    let mut cells =
        vec![TableWidgetCell::grouped(label, label_span).with_style(GROUP_HEADER_STYLE_ID)];
    for ci in label_span..ncols {
        match totals.iter().find(|&&(c, _)| c == ci) {
            Some(&(_, total)) => {
                let text = format_total(total, &columns[ci]);
                let fitted = fit_aligned(&text, width_of(ci), CellAlignment::Right);
                cells.push(TableWidgetCell::plain(fitted).with_style(GROUP_HEADER_STYLE_ID));
            }
            None => cells.push(TableWidgetCell::plain(" ".repeat(width_of(ci)))),
        }
    }
    TableWidgetRow::new(cells).not_selectable()
}

/// Turn one engine-fitted item row into a widget row, applying the same
/// `kind: path` separator styling as the flat (ungrouped) builder.
fn item_widget_row(
    cr: &not_yet_done_table::ComputedRow<u32>,
    columns: &[ColumnDef],
    deleted: bool,
) -> TableWidgetRow {
    let cells: Vec<TableWidgetCell> = cr
        .cells
        .iter()
        .enumerate()
        .map(|(i, fitted)| match columns.get(i) {
            Some(col) if col.kind == ColumnKind::Path => TableWidgetCell::from_segments(
                path_cell_segments(fitted, path_separator(col), PATH_SEPARATOR_STYLE_ID),
            ),
            _ => TableWidgetCell::plain(fitted.clone()),
        })
        // Deleted rows: override every cell's foreground with the dim slot,
        // identical to the ungrouped/tree path — so a soft-deleted row that
        // the query surfaces reads as present-but-greyed inside its group too.
        .map(|c| {
            if deleted {
                c.with_style(DELETED_STYLE_ID)
            } else {
                c
            }
        })
        .collect();
    TableWidgetRow::new(cells)
}

/// Soft-wrap a full field value to `width` columns, honouring hard line
/// breaks: every `\n`-delimited line is char-chunked to `width`. An empty
/// input (or an empty embedded line) yields an empty line. Mirrors the
/// record-detail follower's value wrapping so both read the same way.
fn wrap_long_field(value: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for line in value.split('\n') {
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut start = 0;
        while start < chars.len() {
            let end = (start + width).min(chars.len());
            out.push(chars[start..end].iter().collect());
            start = end;
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Long-text mode: re-render one item row as a multi-line block when a
/// column declares `long_source`. The long column's cell on the first
/// physical line becomes the first soft-wrapped line of the full field, and
/// one continuation line per remaining wrapped line is appended — each with
/// every preceding column blank-padded to its width so the text stays under
/// its column (the grouped/flat paths both use a two-space separator, which
/// the padding widths reproduce). The row stays one selectable unit, so the
/// cursor, `filtered_indices` and actions are unaffected.
///
/// Returns `base` untouched when no column declares `long_source`, the field
/// is empty, or it wraps to a single line (nothing to expand). The long
/// column's cell style (e.g. the dimmed slot on a deleted row) is carried
/// onto every wrapped line.
fn expand_long_text_row(
    base: TableWidgetRow,
    item: &NodeSummary,
    columns: &[ColumnDef],
    col_widths: &[usize],
) -> TableWidgetRow {
    let Some((lci, source)) = columns
        .iter()
        .enumerate()
        .find_map(|(i, c)| c.long_source.as_deref().map(|s| (i, s)))
    else {
        return base;
    };
    let width = col_widths.get(lci).copied().unwrap_or(0);
    if width == 0 {
        return base;
    }
    let full = metadata_field_value(item, source);
    let wrapped = wrap_long_field(full, width);
    if wrapped.len() <= 1 {
        return base;
    }

    let selectable = base.selectable;
    let mut cells0 = match base.lines.into_iter().next() {
        Some(l) => l.cells,
        None => return TableWidgetRow::new(Vec::new()),
    };
    let carried_style = cells0.get(lci).and_then(|c| c.style_id);
    let fit_long = |text: &str| -> TableWidgetCell {
        let mut c = TableWidgetCell::plain(fit_aligned(text, width, CellAlignment::Left));
        if let Some(sid) = carried_style {
            c = c.with_style(sid);
        }
        c
    };
    if let Some(slot) = cells0.get_mut(lci) {
        *slot = fit_long(&wrapped[0]);
    }

    let mut out_lines = vec![TableWidgetLine::new(cells0)];
    for chunk in &wrapped[1..] {
        let mut cont: Vec<TableWidgetCell> = col_widths
            .iter()
            .take(lci)
            .map(|w| TableWidgetCell::plain(" ".repeat(*w)))
            .collect();
        cont.push(fit_long(chunk));
        out_lines.push(TableWidgetLine::new(cont));
    }
    let mut row = TableWidgetRow::multiline(out_lines);
    row.selectable = selectable;
    row
}

/// Output of [`build_grouped_table`].
struct GroupedBuild {
    /// Interspersed header + item rows (the scrolling body); `filtered_indices`
    /// aligns to this 1:1.
    widget_rows: Vec<TableWidgetRow>,
    /// Pinned grand-total footer rows (empty when no aggregates are declared).
    footers: Vec<TableWidgetRow>,
    /// Fitted header-label strings (column widths already applied), for the
    /// caller to turn into the table header with the sort overlay.
    header_cells: Vec<String>,
    /// Row → original `items` index, aligned to `widget_rows`. Header rows
    /// carry `usize::MAX` — they are non-selectable, so it is never read.
    filtered_indices: Vec<usize>,
    /// Column widths from the item-row layout (drives the sort overlay and
    /// horizontal scroll, like the ungrouped path's `last_col_widths`).
    col_widths: Vec<usize>,
}

/// Build the full grouped flat table (M3): partition the filtered items by
/// the group key, total the aggregate columns, and interleave group-header
/// rows (plus a pinned grand-total footer) with the engine-fitted item rows.
///
/// Items are ordered by their group label first, so groups are contiguous and
/// — for the ISO-formatted date-bucket labels — chronological. The generic
/// partition/total mechanism lives in [`not_yet_done_table::group`]; this
/// function supplies the typed extraction (label + aggregate value) and the
/// widget-row rendering.
#[allow(clippy::too_many_arguments)]
fn build_grouped_table(
    items: &[NodeSummary],
    order: &[usize],
    columns: &[ColumnDef],
    levels: &[GroupBy],
    aggregates: &[AggregateDef],
    now: chrono::DateTime<chrono::Local>,
    has_link_lookup: &dyn Fn(&str) -> bool,
    config: &TableConfig,
    col_ids: &[TColumnId],
    header: &TRow<u32>,
    long: bool,
) -> GroupedBuild {
    // Engine grouping is single-level; the caller gates on a non-empty
    // `levels`, so exactly one level is present here.
    let spec = &levels[0];

    // 1. Group-label per filtered item; order items by that label — honouring
    //    the configured `order` — so groups are contiguous (and chronological
    //    for ISO date buckets; `desc` puts the newest bucket first). The sort
    //    is stable, so rows inside a group keep the adapter's order (this is
    //    how the adapter's item sort survives into each group).
    let mut tagged: Vec<(usize, String)> = order
        .iter()
        .map(|&i| {
            let key = group_label(
                raw_value_by_key(&items[i], columns, &spec.column),
                spec.bucket,
            );
            (i, key)
        })
        .collect();
    tagged.sort_by(|a, b| match spec.order {
        GroupOrder::Asc => a.1.cmp(&b.1),
        GroupOrder::Desc => b.1.cmp(&a.1),
    });
    let keys: Vec<String> = tagged.iter().map(|(_, k)| k.clone()).collect();
    let sorted_idx: Vec<usize> = tagged.iter().map(|(i, _)| *i).collect();

    // 2. Aggregate columns split by destination: an aggregate without
    //    `total_column` totals on the `── label ──` header rows; one *with* it
    //    writes per-group totals into that dedicated column on the closing data
    //    row of each group instead. Pairs are (index into `aggregates`/the
    //    totals arrays, column index).
    let header_aggs: Vec<(usize, usize)> = aggregates
        .iter()
        .enumerate()
        .filter(|(_, a)| a.total_column.is_none())
        .filter_map(|(ai, a)| {
            columns
                .iter()
                .position(|c| c.key == a.column)
                .map(|ci| (ai, ci))
        })
        .collect();
    let column_total_aggs: Vec<(usize, usize)> = aggregates
        .iter()
        .enumerate()
        .filter_map(|(ai, a)| {
            let key = a.total_column.as_deref()?;
            columns.iter().position(|c| c.key == key).map(|ci| (ai, ci))
        })
        .collect();
    let total_target_cols: Vec<usize> = column_total_aggs.iter().map(|&(_, ci)| ci).collect();
    let values_owned: Vec<Vec<Option<i64>>> = aggregates
        .iter()
        .map(|a| {
            sorted_idx
                .iter()
                .map(|&i| agg_value(raw_value_by_key(&items[i], columns, &a.column), a.op))
                .collect()
        })
        .collect();
    let values_refs: Vec<&[Option<i64>]> = values_owned.iter().map(|v| v.as_slice()).collect();

    let plan = group(&keys, &values_refs, !aggregates.is_empty());

    // Which data-row position closes each group, and the totals written into
    // the total-target columns there. Computed up front so the engine's width
    // fitting (step 3) already sees the totals in the cells.
    let mut pos_totals: std::collections::HashMap<usize, Vec<(usize, i64)>> =
        std::collections::HashMap::new();
    if !column_total_aggs.is_empty() {
        let close =
            |grp: Option<usize>,
             last: Option<usize>,
             map: &mut std::collections::HashMap<usize, Vec<(usize, i64)>>| {
                if let (Some(g), Some(p)) = (grp, last) {
                    map.insert(
                        p,
                        column_total_aggs
                            .iter()
                            .map(|&(ai, ci)| (ci, plan.group_totals[g][ai]))
                            .collect(),
                    );
                }
            };
        let mut cur_group: Option<usize> = None;
        let mut last_pos: Option<usize> = None;
        let mut data_pos = 0usize;
        for prow in &plan.rows {
            match prow {
                PlanRow::Header { group, .. } => {
                    close(cur_group, last_pos, &mut pos_totals);
                    cur_group = Some(*group);
                    last_pos = None;
                }
                PlanRow::Item { .. } => {
                    last_pos = Some(data_pos);
                    data_pos += 1;
                }
                PlanRow::GrandTotal => {}
            }
        }
        close(cur_group, last_pos, &mut pos_totals);
    }
    // The total a target column shows at data-row `row_idx` — the outermost
    // group's total on its closing row, blank everywhere else. Routed
    // through `typed_cell_content` so the cell keeps the column kind's
    // alignment (a duration total stays right-aligned like the data cells).
    let total_cell = |row_idx: usize, ci: usize, col: &ColumnDef| -> CellContent {
        pos_totals
            .get(&row_idx)
            .and_then(|t| t.iter().find(|&&(c, _)| c == ci))
            .map(|&(_, total)| typed_cell_content(&total.to_string(), col))
            .unwrap_or_else(|| typed_cell_content("", col))
    };

    // A plain data row built from item `pos` (in `sorted_idx` space).
    let item_row = |row_idx: usize, pos: usize| -> TRow<u32> {
        let item = &items[sorted_idx[pos]];
        let mut row = TRow::new(row_idx as u32);
        for (ci, col) in columns.iter().enumerate() {
            if total_target_cols.contains(&ci) {
                row = row.cell(&col.key, total_cell(row_idx, ci, col));
            } else if col.source.as_deref() == Some("has_links") {
                let icon = if has_link_lookup(&item.id) {
                    "🔗"
                } else {
                    " "
                };
                row = row.cell(&col.key, icon);
            } else {
                row = row.cell(&col.key, cell_content_for(item, col, now));
            }
        }
        row
    };

    // 3. Fit every data (`Item`) row through the engine in plan order so the
    //    column widths are consistent. Each row's original-`items` index is
    //    remembered so it stays selectable and maps back correctly.
    let mut data_rows: Vec<TRow<u32>> = Vec::new();
    let mut data_filtered: Vec<usize> = Vec::new();
    for prow in &plan.rows {
        if let PlanRow::Item { index } = prow {
            data_rows.push(item_row(data_rows.len(), *index));
            data_filtered.push(sorted_idx[*index]);
        }
    }
    // Phantom sizing row: the Σ footer's grand totals can be wider than any
    // single data cell (e.g. `260:04:54` vs `7:35:50`) and would otherwise
    // truncate — let them participate in the width fit through one extra
    // row that is computed but never rendered (the plan-row interleave below
    // consumes exactly the real data rows).
    if !aggregates.is_empty() {
        let mut sizing = TRow::new(data_rows.len() as u32).not_selectable();
        for (ci, col) in columns.iter().enumerate() {
            let text = header_aggs
                .iter()
                .chain(column_total_aggs.iter())
                .find(|&&(_, c)| c == ci)
                .map(|&(ai, _)| format_total(plan.grand_totals[ai], col))
                .unwrap_or_default();
            sizing = sizing.cell(&col.key, text);
        }
        data_rows.push(sizing);
    }
    let computed = compute_table(&data_rows, config, col_ids, Some(header));
    let col_widths = computed.col_widths.clone();
    let header_cells = computed.header.map(|h| h.cells).unwrap_or_default();

    // 4. Interleave `── label ──` group-header rows with the fitted data rows
    //    in plan order.
    let mut widget_rows: Vec<TableWidgetRow> = Vec::new();
    let mut filtered_indices: Vec<usize> = Vec::new();
    let mut next_data = 0usize;
    for prow in &plan.rows {
        match prow {
            PlanRow::Header { label, group, .. } => {
                // A `── label ──` group header. The ISO group key stays the
                // sort identity; only the rendered text goes through the
                // human-facing mapping.
                let display = bucket_display_label(label, spec.bucket);
                let totals: Vec<(usize, i64)> = header_aggs
                    .iter()
                    .map(|&(ai, ci)| (ci, plan.group_totals[*group][ai]))
                    .collect();
                widget_rows.push(summary_row(
                    format!("── {display} "),
                    &totals,
                    columns,
                    &col_widths,
                ));
                filtered_indices.push(usize::MAX);
            }
            PlanRow::Item { .. } => {
                let cr = &computed.rows[next_data];
                let deleted = items
                    .get(data_filtered[next_data])
                    .is_some_and(|it| metadata_field_value(it, "deleted") == "true");
                let row = item_widget_row(cr, columns, deleted);
                let row = if long {
                    expand_long_text_row(
                        row,
                        &items[data_filtered[next_data]],
                        columns,
                        &col_widths,
                    )
                } else {
                    row
                };
                widget_rows.push(row);
                filtered_indices.push(data_filtered[next_data]);
                next_data += 1;
            }
            // The grand total is pinned as a footer (below), not interleaved.
            PlanRow::GrandTotal => {}
        }
    }

    // 5. Grand-total footer (only when something is aggregated). Aggregates
    //    routed to a total column show their grand total there too.
    let footers = if aggregates.is_empty() {
        Vec::new()
    } else {
        let mut totals: Vec<(usize, i64)> = header_aggs
            .iter()
            .chain(column_total_aggs.iter())
            .map(|&(ai, ci)| (ci, plan.grand_totals[ai]))
            .collect();
        totals.sort_by_key(|&(ci, _)| ci);
        vec![summary_row(
            "── Σ ".to_string(),
            &totals,
            columns,
            &col_widths,
        )]
    };

    GroupedBuild {
        widget_rows,
        footers,
        header_cells,
        filtered_indices,
        col_widths,
    }
}

/// Render a free-text search query by substituting placeholders in `template`.
///
/// Two placeholder syntaxes coexist so a single function handles both
/// JQL-style adapters (Jira) and structured adapters (Taiga):
///
/// **Curly-brace form (Jira):**
/// - `{q}` → escaped user input (safe inside JQL `"..."` literals).
/// - `{key_or}` → expands to `issuekey = "{q}" OR ` when the input looks
///   like a Jira issue key (`PROJECT-123`), otherwise to the empty
///   string.
///
/// **Angle-bracket form (Taiga and other adapters that consume YAML
/// query templates):**
/// - `<input>` → escaped user input.
/// - `<input_if_numeric>` → escaped input if it's all ASCII digits;
///   otherwise the sentinel `__OMIT__` so the adapter can drop the
///   containing query entry.
fn render_text_search(template: &str, input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            other => escaped.push(other),
        }
    }
    let trimmed = input.trim();
    let key_or = if looks_like_issue_key(trimmed) {
        format!(r#"issuekey = "{}" OR "#, escaped.trim())
    } else {
        String::new()
    };
    let input_if_numeric = if !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit()) {
        trimmed.to_string()
    } else {
        "__OMIT__".to_string()
    };
    template
        .replace("{key_or}", &key_or)
        .replace("{q}", &escaped)
        .replace("<input_if_numeric>", &input_if_numeric)
        .replace("<input>", &escaped)
}

/// True if `s` parses as a Jira issue-key shape: one or more letters/digits/`_`
/// (starting with a letter), then `-`, then one or more digits. Case-insensitive
/// on the prefix; whitespace must already be trimmed by the caller.
fn looks_like_issue_key(s: &str) -> bool {
    let Some((prefix, suffix)) = s.split_once('-') else {
        return false;
    };
    if prefix.is_empty() || suffix.is_empty() {
        return false;
    }
    if !prefix
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
    {
        return false;
    }
    prefix
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
        && suffix.chars().all(|c| c.is_ascii_digit())
}

const DEFAULT_AUTO_MIN: usize = 5;
const DEFAULT_AUTO_MAX: usize = 11;

fn parse_sizing(s: &str) -> ColStrategy {
    let trimmed = s.trim();
    if trimmed == "max" {
        return ColStrategy::Max;
    }
    if trimmed == "fit" {
        return ColStrategy::Fit;
    }
    if trimmed == "auto" {
        return ColStrategy::Auto {
            min: DEFAULT_AUTO_MIN,
            max: DEFAULT_AUTO_MAX,
        };
    }
    if let Some(inner) = trimmed
        .strip_prefix("flex(")
        .and_then(|s| s.strip_suffix(')'))
    {
        if let Ok(n) = inner.parse::<usize>() {
            return ColStrategy::Flex(n);
        }
    }
    // `fixed(n)`: a constant column width that COUNTS toward the pane-width
    // budget (unlike `auto(min,max)`, which deliberately overflows into
    // horizontal scroll). Use it to cap a column without pushing trailing
    // columns off-screen.
    if let Some(inner) = trimmed
        .strip_prefix("fixed(")
        .and_then(|s| s.strip_suffix(')'))
    {
        if let Ok(n) = inner.parse::<usize>() {
            return ColStrategy::Fixed(n);
        }
    }
    if let Some(inner) = trimmed
        .strip_prefix("auto(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let parts: Vec<&str> = inner.split(',').map(|p| p.trim()).collect();
        if parts.len() == 2 {
            if let (Ok(min), Ok(max)) = (parts[0].parse::<usize>(), parts[1].parse::<usize>()) {
                return ColStrategy::Auto { min, max };
            }
        }
        return ColStrategy::Auto {
            min: DEFAULT_AUTO_MIN,
            max: DEFAULT_AUTO_MAX,
        };
    }
    ColStrategy::Max
}

/// Opt-in tree-find walk tracing (`NYD_DEBUG_TREEFIND=1`), sharing the TUI
/// pipeline log written by the App and the Tasks adapter. Emits the hit path
/// and the walk outcome so a live "created task not visible" occurrence pins
/// whether the expand-to-hit walk stalls (`NeedTreeExpand` never completing),
/// reports `NotInTree` (filter / pagination excludes the branch), or lands
/// `Ready` on a row the user still can't see.
pub(crate) fn treefind_walk_trace(detail: impl std::fmt::Display) {
    if std::env::var_os("NYD_DEBUG_TREEFIND").is_none() {
        return;
    }
    let path = std::env::temp_dir().join("nyd-treefind-debug.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let _ = writeln!(f, "[treefind] walk: {detail}");
    }
}

/// Recursively lay an eager [`Subtree`] level into the pane's tree cache:
/// cache this level's children under `parent_path`, then for every node that
/// carries children mark its own path `expanded` and recurse. Pure cache
/// mutation — the single `rebuild_entries` + `rebuild_table` happens once in
/// [`ContentView::apply_subtree`] after the whole walk. The path scheme is
/// identical to the cascade's (`flatten_into`): a node's children live at
/// `parent_path + [node.id]`, so a subtree laid down here and a subtree built
/// by the cascade are indistinguishable to selection / collapse.
fn ingest_subtree_level(
    state: &mut TreeState,
    parent_path: Vec<String>,
    subtree: Subtree,
    preserve_expansion: bool,
) {
    let next_page = next_page_after(subtree.page);
    let summaries: Vec<NodeSummary> = subtree.items.iter().map(|n| n.summary.clone()).collect();
    state.set_cached_children(parent_path.clone(), summaries, next_page);
    for node in subtree.items {
        let mut own_path = parent_path.clone();
        own_path.push(node.summary.id.clone());
        if node.children.items.is_empty() {
            // Empty `children` is ambiguous (see `SubtreeNode` docs): a real
            // leaf vs. a depth-limited frontier. For a *genuine* leaf
            // (`has_children == Some(false)`) we must positively clear any
            // children this node still has cached — otherwise a reload that
            // emptied the node (e.g. its last child was deleted) leaves the
            // stale rows rendering under it, since `rebuild_entries` still
            // walks the old cache while the node sits in `expanded`. A
            // frontier (`has_children != Some(false)`) is left untouched so a
            // later lazy expand can still fill it.
            if node.summary.has_children == Some(false) {
                state.set_cached_children(own_path.clone(), Vec::new(), None);
                state.expanded.remove(&own_path);
            }
            continue;
        }
        // First load: force-expand to build the initial `expand_depth`
        // shape. Reload (`preserve_expansion`): leave `expanded` untouched so
        // the node keeps whatever fold state the user gave it — a collapsed
        // branch stays collapsed even though its (now-fresh) children are
        // still cached beneath it. Either way recurse to renew the deeper
        // cache levels.
        if !preserve_expansion {
            state.expanded.insert(own_path.clone());
        }
        ingest_subtree_level(state, own_path, node.children, preserve_expansion);
    }
}

/// Derive the next-page request from a result's `PageInfo`. Mirrors
/// [`ContentPane::next_page_request`]; lifted into a free function so
/// tree-mode callers can compute it from raw `PageInfo` without a pane.
fn next_page_after(info: Option<PageInfo>) -> Option<PageRequest> {
    let info = info?;
    if !info.has_next || info.limit == 0 {
        return None;
    }
    let next_offset = (info.offset as u64).saturating_add(info.limit as u64);
    Some(PageRequest {
        offset: u32::try_from(next_offset).unwrap_or(u32::MAX),
        limit: info.limit,
    })
}

/// Build the right-hand status text for the pagination footer.
///
/// The function is split out from the renderer so it can be unit-tested
/// without spinning up a Frame.
fn format_page_footer(info: PageInfo, applied_sort: &[SortKey]) -> String {
    let returned_end_inclusive = (info.offset as u64).saturating_add(info.limit as u64);
    let mut parts: Vec<String> = Vec::new();
    match info.total {
        Some(0) => parts.push("0 items".to_string()),
        Some(total) if total <= info.limit as u64 && info.offset == 0 => {
            parts.push(format!("{total} items"));
        }
        Some(total) => {
            let last = returned_end_inclusive.min(total);
            let first = (info.offset as u64) + 1;
            parts.push(format!("Items {first}\u{2013}{last} of {total}"));
            if info.limit > 0 {
                let total_pages = ((total + info.limit as u64 - 1) / info.limit as u64).max(1);
                let current_page = (info.offset as u64 / info.limit as u64) + 1;
                parts.push(format!("Page {current_page}/{total_pages}"));
            }
        }
        None => {
            let first = (info.offset as u64) + 1;
            parts.push(format!("Items {first}\u{2013}{returned_end_inclusive}"));
            if info.has_next || info.has_prev {
                let current_page = if info.limit > 0 {
                    (info.offset as u64 / info.limit as u64) + 1
                } else {
                    1
                };
                parts.push(format!("Page {current_page}"));
            }
        }
    }
    if !applied_sort.is_empty() {
        let sort_text = applied_sort
            .iter()
            .map(|k| {
                let arrow = match k.direction {
                    not_yet_done_content::SortDirection::Asc => '\u{25B2}',
                    not_yet_done_content::SortDirection::Desc => '\u{25BC}',
                };
                format!("{}{arrow}", k.column)
            })
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("Sort: {sort_text}"));
    }
    parts.join(" \u{00B7} ")
}

/// Format the auth-status banner text for `AdapterStatus::Busy`.
/// Renders elapsed/timeout side-by-side so the user can watch the
/// deadline count down on each render-tick: e.g. `"DB: rows of … (3s/7s)"`.
///
/// `timeout_secs == 0` means "no configured timeout" — show elapsed only.
/// If the wall-clock has somehow gone backwards (`now < started_at`) we
/// clamp elapsed to 0 instead of showing a giant negative.
///
/// `progress` is an optional completion estimate in `[0, 1]` for incremental
/// loads; when present it renders as a percentage (`"Loading… 45 % (12s)"`).
/// The adapter reports the raw fraction — the percentage lives here, in the
/// frontend.
/// One tab's load banner, handed to the App for the global surface
/// ([`ContentView::global_load_banner`]). `started_at_unix_ms` travels with the
/// text because several tabs loading at once are shown as one line, whose
/// elapsed counter runs from the oldest of them.
pub struct LoadBanner {
    pub text: String,
    pub started_at_unix_ms: u64,
}

/// The one line that stands for *several* tabs loading at once, e.g.
/// `"3 tabs loading… (4s)"`.
///
/// Collapsing rather than listing keeps the global bar from filling with
/// counters and pushing out the message the user actually has to answer — the
/// bar's line cap would otherwise bind exactly when the app is busiest. The
/// individual labels are dropped on purpose: what the user needs from another
/// tab is "something is still running", and the tab-local banner has the
/// detail for whoever switches over.
///
/// Elapsed runs from the *oldest* start, so the number never jumps backwards
/// when a fast tab finishes and the group shrinks.
pub fn collapsed_load_banner(count: usize, oldest_started_at_unix_ms: u64) -> String {
    format!(
        "{count} tabs loading… ({}s)",
        elapsed_secs(oldest_started_at_unix_ms)
    )
}

/// Whole seconds since a wall-clock instant, clamped at 0 if the clock has
/// gone backwards (NTP step, suspend) — a giant number would read as a bug.
fn elapsed_secs(started_at_unix_ms: u64) -> u64 {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(started_at_unix_ms);
    now_ms.saturating_sub(started_at_unix_ms) / 1000
}

fn busy_banner(
    label: &str,
    started_at_unix_ms: u64,
    timeout_secs: u64,
    progress: Option<f32>,
) -> String {
    let elapsed_secs = elapsed_secs(started_at_unix_ms);
    let pct = progress
        .map(|f| format!(" {} %", (f.clamp(0.0, 1.0) * 100.0).round() as u32))
        .unwrap_or_default();
    if timeout_secs == 0 {
        format!("{label}…{pct} ({elapsed_secs}s)")
    } else {
        format!("{label}…{pct} ({elapsed_secs}s/{timeout_secs}s)")
    }
}

/// Collapse a cursor's `node_type_chain` up to the level that *owns* its
/// column layout, for the column-config override key (see
/// [`ContentPane::column_level_key`]). Walks toward the root while each
/// shallower tree level shows the **identical** column set, and returns the
/// prefix length that identifies the owning level.
///
/// All depths of a tree render into one shared grid, so a deeper level that
/// shows the same columns as the level above it — the signature of
/// [`ViewFileConfig::inherit_tree_columns`] having filled an omitted
/// `columns:` — should configure as the *same* coordinate, not one key per
/// depth. This also folds every recursion depth (which resolves to one and
/// the same `ChildDef`) onto a single key. A level whose columns actually
/// differ from its parent's stops the walk and keeps its own per-level key,
/// so a tree that deliberately diverges per depth stays independently
/// configurable.
fn column_owner_chain_len(vd: &ViewDef, chain: &[String], cursor_cols: &[ColumnDef]) -> usize {
    let mut owner_len = chain.len();
    while owner_len > 1 {
        match tree_level_for_chain(vd, &chain[..owner_len - 1]) {
            Some(parent) if parent.columns == cursor_cols => owner_len -= 1,
            _ => break,
        }
    }
    owner_len
}

/// Whether a column is part of the *default* visible set: shown unless it
/// declares `hidden: true`, with the exception of `keep` (the tree-label
/// column) which is never hideable.
fn column_shown_by_default(col: &ColumnDef, keep: Option<&str>) -> bool {
    !col.hidden || keep == Some(col.key.as_str())
}

/// Keys of the columns shown by default — every non-`hidden` column plus the
/// (never-hideable) tree-label `keep`, in configured order. This is the
/// baseline the column-config popup pre-checks and the layout
/// `apply_column_config` treats as "no override" (so toggling a column back
/// to exactly this set clears the override cleanly).
fn default_visible_keys(cols: &[ColumnDef], keep: Option<&str>) -> Vec<String> {
    cols.iter()
        .filter(|c| column_shown_by_default(c, keep))
        .map(|c| c.key.clone())
        .collect()
}

/// Project a configured column set through a user override (column-config
/// popup): keep only the keys in `visible`, in `visible`'s order. Keys the
/// config no longer knows (stale persisted override after a YAML edit) are
/// skipped. `keep` names a column that must survive regardless — the tree
/// mode's label column, which carries the tree itself — re-inserted at its
/// configured position when an (older/corrupt) override dropped it.
fn apply_column_override(
    cols: Vec<ColumnDef>,
    visible: &[String],
    keep: Option<&str>,
) -> Vec<ColumnDef> {
    let mut result: Vec<ColumnDef> = visible
        .iter()
        .filter_map(|key| cols.iter().find(|c| &c.key == key).cloned())
        .collect();
    if let Some(keep_key) = keep {
        if !result.iter().any(|c| c.key == keep_key) {
            if let Some(pos) = cols.iter().position(|c| c.key == keep_key) {
                let idx = pos.min(result.len());
                result.insert(idx, cols[pos].clone());
            }
        }
    }
    result
}

fn resolve_theme_color(t: &Theme, name: &str) -> ratatui::style::Color {
    match name {
        "accent" => t.accent(),
        "text_high" => t.text_high(),
        "text_med" => t.text_med(),
        "text_dim" => t.text_dim(),
        // `secondary`/`tertiary` mirror the native task table's column color
        // keys (see `tabs/columns.rs`), so an adapter view can reproduce the
        // bespoke tab's per-column coloring exactly. Both route through the
        // theme — no hardcoded colors.
        "secondary" => t.secondary(),
        "tertiary" => t.tertiary(),
        "success" => t.success(),
        "warning" => t.warning(),
        "error" => t.error(),
        // The dedicated tree-connector color, so a view's `tree_connector_style`
        // can point back at the global default (or any view at it explicitly).
        "tree_connector" => t.tree_connector(),
        // The dedicated unread accent, so a view's `unread_style` can point
        // back at the global default (or any view at it explicitly).
        "unread" => t.unread(),
        // Card-mode slots, so a level's `card.border_style` /
        // `card.label_style` can name the global default explicitly.
        "card_border" => t.card_border(),
        "card_label" => t.card_label(),
        _ => t.text_med(),
    }
}

/// The shared content-table style (row / selection / header / scroll colors),
/// derived purely from the theme. Used by both the single-line and multi-line
/// render paths.
fn build_content_table_style(t: &Theme) -> TableStyle {
    TableStyle::new()
        .set_style(TableStyleType::Header, Style::default().bg(t.surface()))
        .set_style(
            TableStyleType::Row,
            Style::default().fg(t.text_med()).bg(t.bg()),
        )
        .set_style(
            TableStyleType::RowSelected,
            Style::default().fg(t.text_high()).bg(t.surface_2()),
        )
        .set_style(
            TableStyleType::ColumnSelected,
            Style::default().fg(t.text_high()).bg(t.surface()),
        )
        .set_style(
            TableStyleType::CellSelected,
            Style::default()
                .fg(t.on_primary())
                .bg(t.primary())
                .add_modifier(Modifier::BOLD),
        )
        .set_style(TableStyleType::Highlight, Style::default().fg(t.accent()))
        .set_style(
            TableStyleType::ScrollIndicator,
            Style::default()
                .fg(t.accent())
                .bg(t.surface())
                .add_modifier(Modifier::BOLD),
        )
}

/// Per-column foreground styles for the single-line content table, resolved
/// from each `ColumnDef.style` theme reference (default: `text_med`). Shared
/// by the ungrouped and grouped (M3) render paths.
fn content_col_styles(columns: &[ColumnDef], t: &Theme) -> Vec<Style> {
    columns
        .iter()
        .map(|col| {
            let color = col
                .style
                .as_deref()
                .map(|s| resolve_theme_color(t, s))
                .unwrap_or(t.text_med());
            Style::default().fg(color)
        })
        .collect()
}

/// Style-map for the single-line content table. Slot indices are fixed and
/// referenced by `*_STYLE_ID` constants:
///
/// - slot 0 — sort-mode dim overlay,
/// - slot 1 ([`PATH_SEPARATOR_STYLE_ID`]) — `kind: path` separator,
/// - slot 2 ([`GROUP_HEADER_STYLE_ID`]) — group headers + grand-total footer,
/// - slot 3 ([`TREE_CONNECTOR_STYLE_ID`]) — tree connector glyphs + arrows,
/// - slot 4 ([`FUZZY_MATCH_STYLE_ID`]) — fuzzy-match runs in the tree label.
/// - slot 5 ([`UNREAD_STYLE_ID`]) — unread chat items (channel/category label
///   + leading marker; unread message header line).
/// - slot 6 ([`DELETED_STYLE_ID`]) — soft-deleted rows kept on screen as
///   context, painted dimmed (`text_dim`).
///
/// The group-header, tree-connector, fuzzy-match, unread, and deleted slots
/// are always present (harmless when nothing is grouped / the view isn't a
/// tree / no filter is active / no node is unread / no node is deleted) so the
/// same map serves every render path. `tree_connector` and `unread` are
/// resolved per view (the caller passes the view's `tree_connector_style` /
/// `unread_style` colors, or the theme defaults).
fn content_style_map(
    t: &Theme,
    tree_connector: ratatui::style::Color,
    unread: ratatui::style::Color,
) -> StyleMap {
    StyleMap::new(vec![
        Style::default().fg(t.text_dim()),
        Style::default()
            .fg(t.taskpath_separator())
            .add_modifier(Modifier::BOLD),
        Style::default()
            .fg(t.group_header())
            .add_modifier(Modifier::BOLD),
        Style::default().fg(tree_connector),
        Style::default().fg(t.accent()).add_modifier(Modifier::BOLD),
        Style::default().fg(unread).add_modifier(Modifier::BOLD),
        Style::default().fg(t.text_dim()),
    ])
}

/// Build the table header row from engine-fitted label strings, applying the
/// sort-mode overlay per column. Shared by the ungrouped and grouped paths.
fn build_header_row(
    fitted_cells: Vec<String>,
    columns: &[ColumnDef],
    header_overlay: &crate::components::sort_header::HeaderOverlay,
) -> TableWidgetRow {
    let cells: Vec<TableWidgetCell> = fitted_cells
        .into_iter()
        .enumerate()
        .map(|(i, fitted)| {
            let key = columns.get(i).map(|c| c.key.as_str()).unwrap_or("");
            crate::components::sort_header::header_cell(&fitted, key, header_overlay)
        })
        .collect();
    TableWidgetRow::new(cells).not_selectable()
}

/// Build the widget rows for card mode.
///
/// [`compute_cards`] does the layout: it derives the card's line stack from
/// `fields / columns`, distributes the pane width over the grid slots, and
/// returns each physical line as a list of typed spans (frame glyph, chrome
/// filler, label, value). This function only turns spans into widget cells
/// and picks each one's style slot — frame glyphs in the card-border color,
/// labels in the card-label color, a value in its own column's `style:`.
///
/// Every span becomes its own cell because the widget lays cells out
/// sequentially; the caller therefore renders the table with an empty
/// separator (the spans already carry padding and inter-slot filler). One
/// cell per span is also what keeps fuzzy-match highlights alive: a widget
/// cell can carry highlights *or* pre-styled segments, never both.
fn build_card_widget_rows(
    data_rows: &[TRow<u32>],
    columns: &[ColumnDef],
    card: &CardConfig,
    spec: &CardSpec,
    max_width: usize,
    t: &Theme,
) -> (Vec<TableWidgetRow>, StyleMap) {
    // One style slot per declared column (fg from `style:`, else `text_med`),
    // so a value keeps the color it has in table mode.
    let mut styles: Vec<Style> = Vec::with_capacity(columns.len() + 2);
    let mut style_idx: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for col in columns {
        let color = col
            .style
            .as_deref()
            .map(|s| resolve_theme_color(t, s))
            .unwrap_or(t.text_med());
        style_idx.insert(col.key.as_str(), styles.len());
        styles.push(Style::default().fg(color));
    }
    let border_color = card
        .border_style
        .as_deref()
        .map(|s| resolve_theme_color(t, s))
        .unwrap_or(t.card_border());
    let border_id = styles.len();
    styles.push(Style::default().fg(border_color));
    let label_color = card
        .label_style
        .as_deref()
        .map(|s| resolve_theme_color(t, s))
        .unwrap_or(t.card_label());
    let label_id = styles.len();
    styles.push(Style::default().fg(label_color));

    let computed = compute_cards(data_rows, spec, max_width);
    let rows: Vec<TableWidgetRow> = computed
        .cards
        .into_iter()
        .map(|card_out| {
            let lines: Vec<TableWidgetLine> = card_out
                .lines
                .into_iter()
                .map(|line| {
                    let cells: Vec<TableWidgetCell> = line
                        .spans
                        .into_iter()
                        .map(|span| {
                            let cell = TableWidgetCell::with_highlights(span.text, span.highlights);
                            match span.kind {
                                CardSpanKind::Border => cell.with_style(border_id),
                                CardSpanKind::Label => cell.with_style(label_id),
                                CardSpanKind::Value => {
                                    // The span's field index points into the
                                    // spec, whose fields carry the column key.
                                    match span
                                        .field
                                        .and_then(|i| spec.fields.get(i))
                                        .and_then(|f| style_idx.get(f.column.0.as_str()))
                                    {
                                        Some(id) => cell.with_style(*id),
                                        None => cell,
                                    }
                                }
                                // Padding / filler is blank space — no color
                                // to apply, so it stays on the row's base style.
                                CardSpanKind::Chrome => cell,
                            }
                        })
                        .collect();
                    TableWidgetLine {
                        cells,
                        highlight_on_select: line.highlight_on_select,
                        image: None,
                    }
                })
                .collect();
            let row = TableWidgetRow::multiline(lines);
            if card_out.selectable {
                row
            } else {
                row.not_selectable()
            }
        })
        .collect();

    (rows, StyleMap::new(styles))
}

/// Build the widget rows for a multi-line (chat) layout.
///
/// Each [`LineLayout`] entry becomes one physical line of every row; an empty
/// line is a blank spacer. Column foreground colors come from each
/// `ColumnDef.style` (theme reference) and are carried per cell via a
/// `StyleMap` style id, so they apply regardless of the cell's position on
/// its line. The column header is suppressed by the caller. Returns the rows
/// plus the style map their cells index into.
///
/// `unread_rows` flags (parallel to `data_rows`) which messages are unread
/// (chat adapters); an unread row's non-markdown lines — the author/time
/// header — are repainted in `unread_color` so the unread message stands out
/// without auto-acking it. Empty (or all-false) leaves every row plain.
fn build_multiline_widget_rows(
    data_rows: &[TRow<u32>],
    columns: &[ColumnDef],
    _col_ids: &[TColumnId],
    config: &TableConfig,
    layout: &[LineLayout],
    t: &Theme,
    unread_rows: &[bool],
    unread_color: ratatui::style::Color,
    images: &mut ImageStore,
) -> (Vec<TableWidgetRow>, StyleMap) {
    // One style slot per declared column (fg from `style:` or `text_med`),
    // keyed by column name so any line can look its column's slot up.
    let mut style_styles: Vec<Style> = Vec::with_capacity(columns.len());
    let mut style_idx: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for col in columns {
        let color = col
            .style
            .as_deref()
            .map(|s| resolve_theme_color(t, s))
            .unwrap_or(t.text_med());
        style_idx.insert(col.key.as_str(), style_styles.len());
        style_styles.push(Style::default().fg(color));
    }

    let template = RowTemplate {
        lines: layout
            .iter()
            .map(|line| {
                let cols = line.columns.iter().map(TColumnId::new).collect();
                LineTemplate::new(cols).with_highlight_on_select(line.highlight_on_select)
            })
            .collect(),
    };

    // For each layout line, the key of its lone column when that column is
    // `markdown: true`. The validator guarantees a markdown column stands
    // alone on its line, so a one-column line is the only candidate. Such a
    // line is expanded from the raw body into N soft-wrapped markdown lines
    // instead of a single fitted cell.
    let markdown_line_key: Vec<Option<&str>> = layout
        .iter()
        .map(|line| {
            if line.columns.len() != 1 {
                return None;
            }
            let key = line.columns[0].as_str();
            columns
                .iter()
                .find(|c| c.key == key && c.markdown)
                .map(|c| c.key.as_str())
        })
        .collect();

    let computed = compute_multiline_table(data_rows, config, &template, None);
    let line_col_widths = computed.line_col_widths;
    let computed_rows = computed.rows;

    // Markdown span styles are appended to the per-column styles in a single
    // shared map, so segment style ids don't collide with column style ids.
    let mut builder = StyleMapBuilder::from_styles(style_styles);
    // The unread header style (chat adapters): interned once so every unread
    // row's header line shares the slot.
    let unread_style_id = builder.intern(
        Style::default()
            .fg(unread_color)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<TableWidgetRow> = computed_rows
        .into_iter()
        .enumerate()
        .map(|(ri, mr)| {
            let selectable = mr.selectable;
            let mut lines: Vec<TableWidgetLine> = Vec::with_capacity(mr.lines.len());
            for (li, cl) in mr.lines.into_iter().enumerate() {
                if let Some(md_key) = markdown_line_key.get(li).copied().flatten() {
                    // Soft-wrap the raw (un-fitted) body to the column's width.
                    let width = line_col_widths
                        .get(li)
                        .and_then(|w| w.first())
                        .copied()
                        .unwrap_or(config.max_width)
                        .max(1);
                    let body = data_rows
                        .get(ri)
                        .and_then(|r| r.cells.get(&TColumnId::new(md_key)))
                        .map(|c| c.text.as_str())
                        .unwrap_or("");
                    let (md_lines, image_refs) =
                        render_markdown_lines_with_images(body, width, t, images);
                    let widget_lines = lines_to_widget_lines_with_images(
                        md_lines,
                        image_refs,
                        &mut builder,
                        cl.highlight_on_select,
                    );
                    if widget_lines.is_empty() {
                        // Empty body: keep one (empty) line so the row's shape
                        // stays stable rather than collapsing the body block.
                        lines.push(
                            TableWidgetLine::new(vec![TableWidgetCell::from_segments(vec![])])
                                .with_highlight_on_select(cl.highlight_on_select),
                        );
                    } else {
                        lines.extend(widget_lines);
                    }
                    continue;
                }
                let line_keys = &layout[li].columns;
                // An unread message paints its header (the non-markdown meta
                // lines — author/time) in the unread slot, overriding the
                // per-column colors so the whole line reads as "new".
                let row_unread = unread_rows.get(ri).copied().unwrap_or(false);
                let cells: Vec<TableWidgetCell> = cl
                    .cells
                    .into_iter()
                    .zip(cl.highlights.into_iter())
                    .enumerate()
                    .map(|(ci, (text, hl))| {
                        let mut cell = TableWidgetCell::with_highlights(text, hl);
                        if row_unread {
                            cell = cell.with_style(unread_style_id);
                        } else if let Some(idx) =
                            line_keys.get(ci).and_then(|k| style_idx.get(k.as_str()))
                        {
                            cell = cell.with_style(*idx);
                        }
                        cell
                    })
                    .collect();
                lines.push(TableWidgetLine {
                    cells,
                    highlight_on_select: cl.highlight_on_select,
                    image: None,
                });
            }
            let row = TableWidgetRow::multiline(lines);
            if selectable {
                row
            } else {
                row.not_selectable()
            }
        })
        .collect();

    (rows, StyleMap::new(builder.into_styles()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ThemeConfig;
    use crate::config::view_config::*;
    use not_yet_done_content::mock::*;

    fn test_theme() -> Arc<Theme> {
        Arc::new(Theme::new(ThemeConfig::default()))
    }

    #[test]
    fn path_segments_tag_only_separators() {
        // Fitted, left-aligned, padded: "/a/b   ".
        let segs = path_cell_segments("/a/b   ", "/", PATH_SEPARATOR_STYLE_ID);
        assert_eq!(
            segs,
            vec![
                ("/".to_string(), Some(PATH_SEPARATOR_STYLE_ID)),
                ("a".to_string(), None),
                ("/".to_string(), Some(PATH_SEPARATOR_STYLE_ID)),
                ("b   ".to_string(), None),
            ]
        );
    }

    #[test]
    fn path_segments_handle_multichar_separator_and_padding() {
        let segs = path_cell_segments(" › a  ", " › ", PATH_SEPARATOR_STYLE_ID);
        assert_eq!(
            segs,
            vec![
                (" › ".to_string(), Some(PATH_SEPARATOR_STYLE_ID)),
                ("a  ".to_string(), None),
            ]
        );
    }

    #[test]
    fn multiline_widget_rows_chat_layout() {
        let theme = test_theme();
        let columns = vec![
            ColumnDef {
                key: "author".into(),
                label: None,
                source: Some("author".into()),
                style: Some("accent".into()),
                sizing: "max".into(),
                markdown: false,
                kind: ColumnKind::Text,
                format: None,
                separator: None,
                elapsed_from: None,
                tree_aggregate: None,
                hidden: false,
                collapsed_source: None,
                long_source: None,
            },
            ColumnDef {
                key: "time".into(),
                label: None,
                source: Some("time".into()),
                style: Some("text_dim".into()),
                sizing: "max".into(),
                markdown: false,
                kind: ColumnKind::Text,
                format: None,
                separator: None,
                elapsed_from: None,
                tree_aggregate: None,
                hidden: false,
                collapsed_source: None,
                long_source: None,
            },
            ColumnDef {
                key: "content".into(),
                label: None,
                source: Some("label".into()),
                style: None,
                sizing: "flex(1)".into(),
                markdown: false,
                kind: ColumnKind::Text,
                format: None,
                separator: None,
                elapsed_from: None,
                tree_aggregate: None,
                hidden: false,
                collapsed_source: None,
                long_source: None,
            },
        ];
        let col_ids: Vec<TColumnId> = columns.iter().map(|c| TColumnId::new(&c.key)).collect();
        let mut strategies = std::collections::HashMap::new();
        for c in &columns {
            strategies.insert(TColumnId::new(&c.key), parse_sizing(&c.sizing));
        }
        let config = TableConfig {
            max_width: 80,
            separator: "  ".into(),
            sizer: Box::new(MixedColSizer { strategies }),
        };
        let data_rows = vec![
            TRow::new(0u32)
                .cell("author", "alice")
                .cell("time", "12:00")
                .cell("content", "hello"),
        ];
        let layout = vec![
            LineLayout {
                columns: vec!["author".into(), "time".into()],
                highlight_on_select: true,
            },
            LineLayout {
                columns: vec!["content".into()],
                highlight_on_select: true,
            },
            LineLayout {
                columns: vec![],
                highlight_on_select: false,
            },
        ];

        let (rows, _style_map) = build_multiline_widget_rows(
            &data_rows,
            &columns,
            &col_ids,
            &config,
            &layout,
            &theme,
            &[],
            theme.unread(),
            // No terminal, no pictures: markdown images stay fallback text.
            &mut ImageStore::disabled(),
        );

        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.height(), 3);
        // Line 0: author + time, both styled (style_id set from `style:`).
        assert_eq!(row.lines[0].cells.len(), 2);
        assert!(row.lines[0].cells[0].text.starts_with("alice"));
        assert!(row.lines[0].cells[0].style_id.is_some());
        assert!(row.lines[0].cells[1].style_id.is_some());
        assert!(row.lines[0].highlight_on_select);
        // Line 1: the message body.
        assert_eq!(row.lines[1].cells.len(), 1);
        assert!(row.lines[1].cells[0].text.starts_with("hello"));
        // Line 2: spacer — empty and outside the selection block.
        assert!(row.lines[2].cells.is_empty());
        assert!(!row.lines[2].highlight_on_select);
    }

    #[test]
    fn multiline_widget_rows_markdown_expands_body() {
        let theme = test_theme();
        let columns = vec![
            ColumnDef {
                key: "author".into(),
                label: None,
                source: Some("author".into()),
                style: Some("accent".into()),
                sizing: "max".into(),
                markdown: false,
                kind: ColumnKind::Text,
                format: None,
                separator: None,
                elapsed_from: None,
                tree_aggregate: None,
                hidden: false,
                collapsed_source: None,
                long_source: None,
            },
            ColumnDef {
                key: "content".into(),
                label: None,
                source: Some("content".into()),
                style: None,
                sizing: "flex(1)".into(),
                markdown: true,
                kind: ColumnKind::Text,
                format: None,
                separator: None,
                elapsed_from: None,
                tree_aggregate: None,
                hidden: false,
                collapsed_source: None,
                long_source: None,
            },
        ];
        let col_ids: Vec<TColumnId> = columns.iter().map(|c| TColumnId::new(&c.key)).collect();
        let mut strategies = std::collections::HashMap::new();
        for c in &columns {
            strategies.insert(TColumnId::new(&c.key), parse_sizing(&c.sizing));
        }
        let config = TableConfig {
            max_width: 30,
            separator: "  ".into(),
            sizer: Box::new(MixedColSizer { strategies }),
        };
        // Two paragraphs, the second long enough to wrap at width 30.
        let body = "First paragraph.\n\nSecond paragraph that is deliberately long \
                    so it has to wrap across several physical lines.";
        let data_rows = vec![
            TRow::new(0u32)
                .cell("author", "alice")
                .cell("content", body),
        ];
        let layout = vec![
            LineLayout {
                columns: vec!["author".into()],
                highlight_on_select: true,
            },
            LineLayout {
                columns: vec!["content".into()],
                highlight_on_select: true,
            },
            LineLayout {
                columns: vec![],
                highlight_on_select: false,
            },
        ];

        let (rows, style_map) = build_multiline_widget_rows(
            &data_rows,
            &columns,
            &col_ids,
            &config,
            &layout,
            &theme,
            &[],
            theme.unread(),
            // No terminal, no pictures: markdown images stay fallback text.
            &mut ImageStore::disabled(),
        );

        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        // author (1) + several body lines (>1) + spacer (1) => height > 3.
        assert!(row.height() > 3, "height was {}", row.height());
        // Body lines are built from styled segments (markdown path), not the
        // plain fitted cell.
        let body_lines = &row.lines[1..row.lines.len() - 1];
        assert!(body_lines.len() > 1, "body should span multiple lines");
        assert!(
            body_lines.iter().any(|l| l
                .cells
                .first()
                .map(|c| !c.segments.is_empty())
                .unwrap_or(false)),
            "at least one body line carries markdown segments"
        );
        // Every interned segment id is valid in the returned StyleMap.
        for l in body_lines {
            for cell in &l.cells {
                for (_t, id) in &cell.segments {
                    if let Some(id) = id {
                        assert!(style_map.get(*id).is_some(), "segment id {id} in map");
                    }
                }
            }
        }
        // Last line is still the spacer.
        assert!(row.lines.last().unwrap().cells.is_empty());
    }

    #[test]
    fn render_text_search_escapes_quotes_and_backslashes() {
        let tpl = r#"text ~ "{q}" ORDER BY updated DESC"#;
        assert_eq!(
            render_text_search(tpl, "memory leak"),
            r#"text ~ "memory leak" ORDER BY updated DESC"#,
        );
        assert_eq!(
            render_text_search(tpl, r#"the "real" issue"#),
            r#"text ~ "the \"real\" issue" ORDER BY updated DESC"#,
        );
        assert_eq!(
            render_text_search(tpl, r"path\to\file"),
            r#"text ~ "path\\to\\file" ORDER BY updated DESC"#,
        );
        assert_eq!(
            render_text_search("status = Open", "anything"),
            "status = Open"
        );
    }

    #[test]
    fn render_text_search_key_or_placeholder() {
        let tpl = r#"({key_or}text ~ "{q}") ORDER BY updated DESC"#;
        assert_eq!(
            render_text_search(tpl, "memory leak"),
            r#"(text ~ "memory leak") ORDER BY updated DESC"#,
        );
        assert_eq!(
            render_text_search(tpl, "ABC-123"),
            r#"(issuekey = "ABC-123" OR text ~ "ABC-123") ORDER BY updated DESC"#,
        );
        assert_eq!(
            render_text_search(tpl, "abc-1"),
            r#"(issuekey = "abc-1" OR text ~ "abc-1") ORDER BY updated DESC"#,
        );
        assert_eq!(
            render_text_search(tpl, "  ABC-7  "),
            r#"(issuekey = "ABC-7" OR text ~ "  ABC-7  ") ORDER BY updated DESC"#,
        );
    }

    #[test]
    fn render_text_search_taiga_input_placeholder() {
        let tpl = "- { type: task,  q: \"<input>\" }\n- { type: issue, q: \"<input>\" }";
        let rendered = render_text_search(tpl, "memory leak");
        assert!(rendered.contains(r#"q: "memory leak""#));
        assert!(rendered.contains("type: task"));
        assert!(rendered.contains("type: issue"));
    }

    #[test]
    fn render_text_search_input_if_numeric_substitutes_when_digits() {
        let tpl = "- { type: task,  ref: <input_if_numeric> }";
        let rendered = render_text_search(tpl, "42");
        assert_eq!(rendered, "- { type: task,  ref: 42 }");
    }

    #[test]
    fn render_text_search_input_if_numeric_emits_omit_for_non_digits() {
        let tpl = "- { type: task,  ref: <input_if_numeric> }";
        let rendered = render_text_search(tpl, "hello");
        assert_eq!(rendered, "- { type: task,  ref: __OMIT__ }");
    }

    #[test]
    fn render_text_search_input_escapes_quotes_for_yaml() {
        let tpl = r#"- { type: task,  q: "<input>" }"#;
        let rendered = render_text_search(tpl, r#"the "real" issue"#);
        assert_eq!(rendered, r#"- { type: task,  q: "the \"real\" issue" }"#);
    }

    #[test]
    fn looks_like_issue_key_detects_jira_keys() {
        assert!(looks_like_issue_key("ABC-123"));
        assert!(looks_like_issue_key("A-1"));
        assert!(looks_like_issue_key("PRJ_42-7"));
        assert!(looks_like_issue_key("abc-9"));
        assert!(!looks_like_issue_key("hello"));
        assert!(!looks_like_issue_key("ABC-"));
        assert!(!looks_like_issue_key("-123"));
        assert!(!looks_like_issue_key("123-456"));
        assert!(!looks_like_issue_key("ABC-12a"));
        assert!(!looks_like_issue_key(""));
        assert!(!looks_like_issue_key("foo bar"));
    }

    fn test_config_with_children() -> ViewFileConfig {
        ViewFileConfig {
            reminder: None,
            tab: TabConfig {
                name: "Test".into(),
                order: 0,
                icon: None,
                key: None,
                unread_marker: None,
                unread_style: None,
                load_banner: None,
            },
            adapter: AdapterConfig {
                adapter_type: "mock".into(),
                id: None,
                config: None,
                config_inline: None,
                manual_connect: false,
            },
            views: vec![ViewDef {
                card: None,
                row_layout: None,
                smooth_scroll: false,
                name: "issues".into(),
                node_type: "mock:issue".into(),
                default: true,
                window_ops: false,
                key: None,
                query: None,
                columns: vec![
                    ColumnDef {
                        key: "key".into(),
                        label: Some("Key".into()),
                        source: None,
                        style: None,
                        sizing: "max".into(),
                        markdown: false,
                        kind: ColumnKind::Text,
                        format: None,
                        separator: None,
                        elapsed_from: None,
                        tree_aggregate: None,
                        hidden: false,
                        collapsed_source: None,
                        long_source: None,
                    },
                    ColumnDef {
                        key: "summary".into(),
                        label: None,
                        source: Some("label".into()),
                        style: None,
                        sizing: "flex(1)".into(),
                        markdown: false,
                        kind: ColumnKind::Text,
                        format: None,
                        separator: None,
                        elapsed_from: None,
                        tree_aggregate: None,
                        hidden: false,
                        collapsed_source: None,
                        long_source: None,
                    },
                ],
                preview: Some(PreviewConfig {
                    enabled: true,
                    source: "content".into(),
                    action: None,
                    node_id_from: None,
                    split: "horizontal".into(),
                    ratio: 50,
                    keybinding: Some("p".into()),
                    markdown: false,
                }),
                actions: vec![ActionDef {
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
                }],
                children: vec![ChildDef {
                    card: None,
                    row_layout: None,
                    smooth_scroll: false,
                    name: "Comments".into(),
                    node_type: "mock:comment".into(),
                    columns: vec![ColumnDef {
                        key: "body".into(),
                        label: Some("Comment".into()),
                        source: Some("label".into()),
                        style: None,
                        sizing: "flex(1)".into(),
                        markdown: false,
                        kind: ColumnKind::Text,
                        format: None,
                        separator: None,
                        elapsed_from: None,
                        tree_aggregate: None,
                        hidden: false,
                        collapsed_source: None,
                        long_source: None,
                    }],
                    preview: None,
                    actions: vec![],
                    children: vec![],
                    split: None,
                    pagination: None,
                    keybindings: HashMap::new(),
                    action_chains: Default::default(),
                    column_cursor: false,
                    record_detail: false,
                    node_scripts: false,
                    tree_label: None,
                    shortcuts: HashMap::new(),
                    enter_action: None,
                    recursive: false,
                    editor_in_place: false,
                    leaf_glyph: None,
                    icon: None,
                    group_by: None,
                    aggregates: Vec::new(),
                    mark_read_on_reach_end: None,
                    cursor_on_open: None,
                }],
                pagination: None,
                action_chains: Default::default(),
                column_cursor: false,
                record_detail: false,
                node_scripts: false,
                tree_label: None,
                retries: 0,
                script_template: None,
                script_source: None,
                shortcuts: HashMap::new(),
                leaf_glyph: None,
                icon: None,
                group_by: None,
                aggregates: Vec::new(),
                tree_connector_style: None,
                unread_style: None,
                unread_marker: None,
                tree_lines: None,
                tree_markers: None,
                expand_depth: None,
                group_headers: None,
                event_actions: Vec::new(),
            }],
        }
    }

    fn mock_issues() -> Vec<NodeSummary> {
        vec![
            NodeSummary {
                id: "ISS-1".into(),
                label: "First issue".into(),
                node_type: issue_type(),
                metadata: not_yet_done_content::Metadata {
                    fields: vec![not_yet_done_content::MetadataField {
                        key: "key".into(),
                        value: "ISS-1".into(),
                        display_label: "Key".into(),
                        editable: false,
                        allowed_values: None,
                    }],
                },
                has_children: None,
            },
            NodeSummary {
                id: "ISS-2".into(),
                label: "Second issue".into(),
                node_type: issue_type(),
                metadata: not_yet_done_content::Metadata::default(),
                has_children: None,
            },
        ]
    }

    fn mock_comments() -> Vec<NodeSummary> {
        vec![
            NodeSummary {
                id: "COM-1".into(),
                label: "Great work".into(),
                node_type: comment_type(),
                metadata: not_yet_done_content::Metadata::default(),
                has_children: None,
            },
            NodeSummary {
                id: "COM-2".into(),
                label: "Needs fix".into(),
                node_type: comment_type(),
                metadata: not_yet_done_content::Metadata::default(),
                has_children: None,
            },
        ]
    }

    #[test]
    fn new_content_view_starts_at_root() {
        let config = test_config_with_children();
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        assert_eq!(view.nav_depth(), 0);
        assert!(view.breadcrumbs().is_empty());
        assert!(view.active_pane().active_child.is_none());
    }

    #[test]
    fn current_columns_at_root() {
        let config = test_config_with_children();
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let cols = view.active_pane().current_columns(&view.view_defs);
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0].key, "key");
        assert_eq!(cols[1].key, "summary");
    }

    #[test]
    fn described_column_type_overrides_yaml_kind() {
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        // The backend describes `summary` as a duration; the YAML leaves it
        // the default text kind. The described type must win for rendering,
        // while an undescribed column keeps its YAML kind.
        view.record_column_schema(
            "any".into(),
            vec![not_yet_done_content::ColumnSchema {
                label: None,
                ..not_yet_done_content::ColumnSchema::new("summary", "").typed("duration")
            }],
        );
        let cols = view.active_pane().current_columns(&view.view_defs);
        assert_eq!(
            cols.iter().find(|c| c.key == "summary").unwrap().kind,
            ColumnKind::Duration
        );
        assert_eq!(
            cols.iter().find(|c| c.key == "key").unwrap().kind,
            ColumnKind::Text
        );
    }

    #[test]
    fn current_actions_at_root() {
        let config = test_config_with_children();
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let actions = view.active_pane().current_actions(&view.view_defs);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].primary_key(), Some("e"));
    }

    #[test]
    fn context_shortcut_rows_include_node_shortcuts() {
        // A per-node `shortcuts:` entry (e.g. `s: toggle-tracking`) dispatches
        // through the node-action path, not the pane keymap — it must still be
        // surfaced (and stay editable) in the shortcut menu's context scope.
        let mut config = test_config_with_children();
        config.views[0].shortcuts.insert(
            's',
            crate::config::view_config::ShortcutDef::Action("toggle-tracking".into()),
        );
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let rows = view.context_shortcut_rows();
        let row = rows
            .iter()
            .find(|r| {
                matches!(
                    &r.source,
                    Some(crate::keymap::KeySource::NodeShortcut { action, .. })
                        if action == "toggle-tracking"
                )
            })
            .expect("node shortcut 's' should appear in the context menu");
        assert_eq!(row.keys, "s");
    }

    #[test]
    fn context_shortcut_rows_surface_unbound_adapter_actions() {
        // The adapter — not the YAML — is the source of truth for what a node
        // can do. An adapter action with *no* `shortcuts:` binding must still
        // appear in the context menu as a keyless, bindable row, so the user
        // can assign it a key. Built-in framework actions (e.g. `help`) are
        // excluded — they are not per-node shortcuts.
        let config = test_config_with_children(); // root node_type: mock:issue
        let adapter: Arc<dyn not_yet_done_content::ContentAdapter> = Arc::new(test_adapter(&[(
            "mock:issue",
            vec![
                make_action("toggle-tracking", "Toggle Tracking", InputSpec::None),
                make_action("help", "Help", InputSpec::None),
            ],
        )]));
        let view = ContentView::new(
            test_theme(),
            &config,
            Some(adapter),
            &KeyBindingConfig::default(),
        );
        let rows = view.context_shortcut_rows();

        let row = rows
            .iter()
            .find(|r| {
                matches!(
                    &r.source,
                    Some(crate::keymap::KeySource::NodeShortcut { action, .. })
                        if action == "toggle-tracking"
                )
            })
            .expect("unbound adapter action should appear as a bindable row");
        assert_eq!(row.keys, "", "an unbound action shows no key");
        assert!(
            row.key_scope.is_some(),
            "it must carry an editable scope so Ctrl+N can bind it"
        );

        assert!(
            !rows.iter().any(|r| matches!(
                &r.source,
                Some(crate::keymap::KeySource::NodeShortcut { action, .. }) if action == "help"
            )),
            "built-in framework actions must not surface as per-node shortcuts"
        );
    }

    #[test]
    fn keyless_yaml_action_carries_a_routable_view() {
        // Regression: a YAML action declared with `key: []` (an *empty* binding
        // — deserializes to `Some(vec![])`, not `None`, e.g. Jira's "free
        // text") still reaches the pane keymap and surfaces in the context
        // scope as a bindable row. Its `YamlAction` source used to carry a
        // blank `view`, so binding it a key failed with "No config file found
        // for view ''". The runtime keymap must carry the real view name.
        let mut config = test_config_with_children();
        let mut keyless = config.views[0].actions[0].clone();
        keyless.name = "free text".into();
        keyless.key = Some(KeyBinding(vec![]));
        config.views[0].actions.push(keyless);

        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let rows = view.context_shortcut_rows();
        let row = rows
            .iter()
            .find(|r| {
                matches!(
                    &r.source,
                    Some(crate::keymap::KeySource::YamlAction { name, .. })
                        if name == "free text"
                )
            })
            .expect("keyless YAML action should surface as a bindable context row");
        let Some(crate::keymap::KeySource::YamlAction { view, .. }) = &row.source else {
            unreachable!("matched above");
        };
        assert_eq!(
            view, "issues",
            "the keyless action must carry a routable view, not ''"
        );
    }

    #[test]
    fn all_node_shortcut_rows_span_every_declared_level() {
        // The "All tabs" / "Unbound" scopes must list unbound adapter actions
        // from *every* declared level, not just the focused one — walking the
        // configured node_types (root `mock:issue`, child `mock:comment`)
        // rather than the live selection. A bound `shortcuts:` key surfaces
        // with its key; the same action stops appearing as a keyless row.
        let mut config = test_config_with_children();
        config.views[0].shortcuts.insert(
            's',
            crate::config::view_config::ShortcutDef::Action("toggle-tracking".into()),
        );
        let adapter: Arc<dyn not_yet_done_content::ContentAdapter> = Arc::new(test_adapter(&[
            (
                "mock:issue",
                vec![
                    make_action("toggle-tracking", "Toggle Tracking", InputSpec::None),
                    make_action("help", "Help", InputSpec::None),
                ],
            ),
            (
                "mock:comment",
                vec![make_action("resolve", "Resolve", InputSpec::None)],
            ),
        ]));
        let view = ContentView::new(
            test_theme(),
            &config,
            Some(adapter),
            &KeyBindingConfig::default(),
        );
        let rows = view.all_node_shortcut_rows();

        // The root's bound `toggle-tracking` shows its key, at root scope, and
        // is *not* duplicated as an unbound row.
        let bound: Vec<_> = rows
            .iter()
            .filter(|r| {
                matches!(
                    &r.source,
                    Some(crate::keymap::KeySource::NodeShortcut { action, child_path, .. })
                        if action == "toggle-tracking" && child_path.is_empty()
                )
            })
            .collect();
        assert_eq!(bound.len(), 1, "bound action appears once, not twice");
        assert_eq!(bound[0].keys, "s");

        // The child level's `resolve` is surfaced unbound and bindable, at the
        // child's scope with the child in its path.
        let child = rows
            .iter()
            .find(|r| {
                matches!(
                    &r.source,
                    Some(crate::keymap::KeySource::NodeShortcut { action, child_path, .. })
                        if action == "resolve" && !child_path.is_empty()
                )
            })
            .expect("child-level adapter action should appear from the whole tree");
        assert_eq!(child.keys, "");
        assert!(child.key_scope.is_some());

        // Built-ins never surface.
        assert!(!rows.iter().any(|r| matches!(
            &r.source,
            Some(crate::keymap::KeySource::NodeShortcut { action, .. }) if action == "help"
        )));
    }

    #[test]
    fn all_node_shortcut_rows_label_each_subtab_distinctly() {
        // A tab with several subtabs exposes the same adapter action once per
        // subtab (Trackings' `trackings` / `condensed` / `tree` each carry
        // `toggle-tracking`). Each must be its own row, labelled with the
        // subtab so it stays bindable on its own — not collapsed under dedup
        // into a single ambiguous entry.
        let mut config = test_config_with_children();
        config.views[0].name = "issues".into();
        config.views[0].node_type = "mock:issue".into();
        config.views[0].children = vec![];
        let mut second = config.views[0].clone();
        second.name = "condensed".into();
        second.node_type = "mock:track".into();
        second.default = false;
        config.views.push(second);

        let adapter: Arc<dyn not_yet_done_content::ContentAdapter> = Arc::new(test_adapter(&[
            (
                "mock:issue",
                vec![make_action(
                    "toggle-tracking",
                    "Toggle Tracking",
                    InputSpec::None,
                )],
            ),
            (
                "mock:track",
                vec![make_action(
                    "toggle-tracking",
                    "Toggle Tracking",
                    InputSpec::None,
                )],
            ),
        ]));
        let view = ContentView::new(
            test_theme(),
            &config,
            Some(adapter),
            &KeyBindingConfig::default(),
        );

        let rows = view.all_node_shortcut_rows();
        let scopes: Vec<&str> = rows
            .iter()
            .filter(|r| {
                matches!(
                    &r.source,
                    Some(crate::keymap::KeySource::NodeShortcut { action, .. })
                        if action == "toggle-tracking"
                )
            })
            .map(|r| r.scope.as_str())
            .collect();
        assert!(scopes.contains(&"Test › issues"), "got {scopes:?}");
        assert!(scopes.contains(&"Test › condensed"), "got {scopes:?}");
        assert_eq!(
            scopes.len(),
            2,
            "one bindable row per subtab, none collapsed: {scopes:?}"
        );
    }

    #[test]
    fn cycle_subtab_wraps_across_subtabs() {
        // `]` / `[` cycle subtabs like the main-tab switch cycles tabs.
        let mut config = test_config_with_children();
        config.views[0].name = "issues".into();
        let mut second = config.views[0].clone();
        second.name = "condensed".into();
        second.default = false;
        config.views.push(second);

        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        assert_eq!(view.active_subtab, 0);

        assert!(view.cycle_subtab(true).is_some());
        assert_eq!(view.active_subtab, 1);
        // Forward from the last subtab wraps back to the first.
        assert!(view.cycle_subtab(true).is_some());
        assert_eq!(view.active_subtab, 0);
        // Backward from the first subtab wraps to the last.
        assert!(view.cycle_subtab(false).is_some());
        assert_eq!(view.active_subtab, 1);
    }

    #[test]
    fn cycle_subtab_is_noop_with_a_single_subtab() {
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        assert_eq!(view.view_defs.len(), 1);
        assert!(view.cycle_subtab(true).is_none());
        assert!(view.cycle_subtab(false).is_none());
        assert_eq!(view.active_subtab, 0);
    }

    #[test]
    fn current_children_at_root() {
        let config = test_config_with_children();
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let children = view.active_pane().current_children(&view.view_defs);
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].node_type, "mock:comment");
    }

    #[test]
    fn yaml_action_chord_prefix_detects_multi_char_action_keys() {
        // The App's chord interceptor relies on this to stash the first
        // key of a YAML `actions:` chord (e.g. `al` → new channel) that
        // does not live in the typed `keybindings.content` section.
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        // Mirror stoat.yaml: a multi-char chord action on the root view.
        view.view_defs[0].actions.push(ActionDef {
            name: "new channel".into(),
            key: Some("al".into()),
            action_type: "custom".into(),
            id: Some("create_channel".into()),
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
        });
        // `a` begins the `al` chord → detected as a prefix …
        assert!(view.yaml_action_chord_prefix("a"));
        // … the full chord is the binding itself, not a prefix of it …
        assert!(!view.yaml_action_chord_prefix("al"));
        // … an unrelated key is a prefix of nothing here …
        assert!(!view.yaml_action_chord_prefix("z"));
        // … and the single-char `edit` key (`e`) is no chord at all.
        assert!(!view.yaml_action_chord_prefix("e"));
    }

    #[test]
    fn current_preview_at_root() {
        let config = test_config_with_children();
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let preview = view
            .active_pane()
            .current_preview_config(&view.view_defs)
            .unwrap();
        assert_eq!(preview.keybinding.as_deref(), Some("p"));
    }

    #[test]
    fn set_items_populates_table() {
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);
        assert_eq!(view.active_pane().items.len(), 2);
        assert_eq!(view.active_pane().items[0].id, "ISS-1");
        assert_eq!(view.active_pane().items[1].id, "ISS-2");
        assert!(view.active_pane().fetch_error.is_none());
    }

    /// An `unread` metadata field as the chat adapters emit it: `"true"`
    /// when unread, empty when read.
    fn unread_meta(value: &str) -> not_yet_done_content::MetadataField {
        not_yet_done_content::MetadataField {
            key: "unread".into(),
            value: value.into(),
            display_label: "Unread".into(),
            editable: false,
            allowed_values: None,
        }
    }

    #[test]
    fn has_unread_follows_the_rows_unread_metadata() {
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());

        // Rows without the field at all (every non-chat adapter) never light
        // the tab.
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);
        assert!(!view.has_unread());

        let mut items = mock_issues();
        items[1].metadata.fields.push(unread_meta("true"));
        view.set_items(items, Vec::new(), None, Vec::new(), None);
        assert!(view.has_unread(), "one unread row is enough");

        // Reading it (the adapter re-emits the field empty) clears the tab.
        let mut items = mock_issues();
        items[1].metadata.fields.push(unread_meta(""));
        view.set_items(items, Vec::new(), None, Vec::new(), None);
        assert!(!view.has_unread());
    }

    #[test]
    fn unread_tab_marker_falls_back_from_tab_to_view_to_default() {
        // Neither level configures one → the bell, NOT the rows' default
        // speech balloon (which doubles as a channel-type icon in chat views).
        let config = test_config_with_children();
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        assert_eq!(view.unread_tab_marker(), DEFAULT_TAB_UNREAD_MARKER);
        assert_ne!(DEFAULT_TAB_UNREAD_MARKER, DEFAULT_UNREAD_MARKER);

        // The view's own marker still wins over that default, so tree and tab
        // agree without configuring the glyph twice.
        let mut config = test_config_with_children();
        config.views[0].unread_marker = Some("•".into());
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        assert_eq!(view.unread_tab_marker(), "•");

        // `tab.unread_marker` wins over it — including an explicit empty
        // string, which suppresses the glyph.
        config.tab.unread_marker = Some(String::new());
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        assert_eq!(view.unread_tab_marker(), "");
    }

    #[test]
    fn unread_tab_style_defaults_to_bold_only() {
        use crate::config::view_config::{TabUnreadStyle, TextModifier};
        use ratatui::style::Modifier;

        let config = test_config_with_children();
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let style = view.unread_tab_style();
        assert!(style.add_modifier.contains(Modifier::BOLD));
        assert!(
            style.fg.is_none(),
            "no color by default — the bar keeps its own palette"
        );

        // A bare color name recolors without touching the font …
        let mut config = test_config_with_children();
        config.tab.unread_style = Some(TabUnreadStyle::Color("accent".into()));
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let style = view.unread_tab_style();
        assert!(style.add_modifier.is_empty());
        assert!(style.fg.is_some());

        // … and a bare modifier list does the opposite.
        config.tab.unread_style = Some(TabUnreadStyle::Modifiers(vec![TextModifier::Italic]));
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let style = view.unread_tab_style();
        assert!(style.add_modifier.contains(Modifier::ITALIC));
        assert!(!style.add_modifier.contains(Modifier::BOLD));
        assert!(style.fg.is_none());
    }

    /// Flat (root-level) view with `smooth_scroll: true` and a 2-line
    /// `row_layout` (body + spacer) — a minimal stand-in for the chat
    /// message list.
    fn smooth_chat_config() -> ViewFileConfig {
        ViewFileConfig {
            reminder: None,
            tab: TabConfig {
                name: "Chat".into(),
                order: 0,
                icon: None,
                key: None,
                unread_marker: None,
                unread_style: None,
                load_banner: None,
            },
            adapter: AdapterConfig {
                adapter_type: "mock".into(),
                id: None,
                config: None,
                config_inline: None,
                manual_connect: false,
            },
            views: vec![ViewDef {
                card: None,
                row_layout: Some(vec![
                    LineLayout {
                        columns: vec!["body".into()],
                        highlight_on_select: true,
                    },
                    LineLayout {
                        columns: vec![],
                        highlight_on_select: false,
                    },
                ]),
                smooth_scroll: true,
                name: "messages".into(),
                node_type: "mock:msg".into(),
                default: true,
                window_ops: false,
                key: None,
                query: None,
                columns: vec![ColumnDef {
                    key: "body".into(),
                    label: None,
                    source: Some("label".into()),
                    style: None,
                    sizing: "flex(1)".into(),
                    markdown: false,
                    kind: ColumnKind::Text,
                    format: None,
                    separator: None,
                    elapsed_from: None,
                    tree_aggregate: None,
                    hidden: false,
                    collapsed_source: None,
                    long_source: None,
                }],
                preview: None,
                actions: vec![],
                children: vec![],
                pagination: None,
                action_chains: Default::default(),
                column_cursor: false,
                record_detail: false,
                node_scripts: false,
                tree_label: None,
                retries: 0,
                script_template: None,
                script_source: None,
                shortcuts: HashMap::new(),
                leaf_glyph: None,
                icon: None,
                group_by: None,
                aggregates: Vec::new(),
                tree_connector_style: None,
                unread_style: None,
                unread_marker: None,
                tree_lines: None,
                tree_markers: None,
                expand_depth: None,
                group_headers: None,
                event_actions: Vec::new(),
            }],
        }
    }

    /// End-to-end reproduction of the "j/k does not scroll the chat" report:
    /// drive the real key path (`handle_key("j")`) and the per-frame rebuild
    /// (`rebuild_table`, what `sync_components` runs every frame), rendering
    /// to a real backend in between, and assert the rendered buffer actually
    /// scrolls. The per-frame rebuild used to re-select the sticky row and
    /// undo the one-line scroll.
    #[test]
    fn smooth_pane_scrolls_on_j_across_per_frame_rebuild() {
        use ratatui::{Terminal, backend::TestBackend};

        let config = smooth_chat_config();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        // 30 two-line messages = 60 physical lines, far past a 12-row view.
        let items: Vec<_> = (0..30)
            .map(|i| tnode(&format!("m{i}"), &format!("message {i}"), "mock:msg"))
            .collect();
        view.set_items(items, Vec::new(), None, Vec::new(), None);

        let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
        let render = |view: &mut ContentView, terminal: &mut Terminal<TestBackend>| {
            terminal
                .draw(|f| {
                    let area = f.area();
                    view.active_pane_mut().table.view(f, area);
                })
                .unwrap();
            terminal.backend().buffer().clone()
        };

        let before = render(&mut view, &mut terminal);
        // Real key dispatch: `j` → ListNext → nav → scroll_lines(1).
        view.handle_key("j");
        // The frame loop rebuilds the active content table every frame.
        view.rebuild_table();
        let after = render(&mut view, &mut terminal);

        assert_ne!(
            before, after,
            "pressing `j` must scroll the smooth pane (content shifts up one line)"
        );
    }

    /// The reported dead-zone: when the whole chat fits on screen there is
    /// nothing to scroll, so `j`/`k` used to do nothing at all. The virtual
    /// cursor must still step the focus message-by-message. Drives the real
    /// key path and per-frame rebuild, like the test above.
    #[test]
    fn smooth_pane_steps_focus_when_everything_fits() {
        use ratatui::{Terminal, backend::TestBackend};

        let config = smooth_chat_config();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        // Only 3 two-line messages = 6 physical lines, well within a 20-row view.
        let items: Vec<_> = (0..3)
            .map(|i| tnode(&format!("m{i}"), &format!("message {i}"), "mock:msg"))
            .collect();
        view.set_items(items, Vec::new(), None, Vec::new(), None);

        let mut terminal = Terminal::new(TestBackend::new(40, 20)).unwrap();
        let render = |view: &mut ContentView, terminal: &mut Terminal<TestBackend>| {
            terminal
                .draw(|f| {
                    let area = f.area();
                    view.active_pane_mut().table.view(f, area);
                })
                .unwrap();
            terminal.backend().buffer().clone()
        };

        // Render once so the table learns its line budget.
        let before = render(&mut view, &mut terminal);
        assert_eq!(view.active_pane_mut().table.selected_row(), 0);

        view.handle_key("j");
        view.rebuild_table();
        let after = render(&mut view, &mut terminal);

        assert_eq!(
            view.active_pane_mut().table.selected_row(),
            1,
            "`j` must move the focus to the next message even when nothing scrolls"
        );
        assert_ne!(before, after, "the moved highlight must be visible");
    }

    /// Regression: pressing `r` on a not-yet-loaded `manual_connect` pane
    /// (postgres / confluence) froze the whole app at 100 % CPU. The pane
    /// has no columns before its first load (`current_columns` → empty), so
    /// `rebuild_table_with` bailed out *without* stamping
    /// `built_table_width`; the post-draw re-fit pass then saw
    /// `render width != built width` on every frame and requested a redraw
    /// forever. The re-fit must converge: at most one rebuild, then a no-op.
    #[test]
    fn refit_converges_on_column_less_pane() {
        use ratatui::{Terminal, backend::TestBackend};

        let mut config = smooth_chat_config();
        config.views[0].columns = Vec::new();
        config.views[0].row_layout = None;
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        // No items either, so the auto-fallback column derivation is empty too.

        let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
        terminal
            .draw(|f| {
                let area = f.area();
                view.active_pane_mut().table.view(f, area);
            })
            .unwrap();
        assert_ne!(
            view.active_pane_mut().table.last_render_width(),
            0,
            "premise: the draw must record a render width"
        );

        // First pass may rebuild (the width just became known) …
        view.refit_tables_if_needed();
        // … but the second must be a no-op, or the render loop spins forever.
        assert!(
            !view.refit_tables_if_needed(),
            "re-fit must converge for a pane without columns (manual_connect pre-load)"
        );
    }

    /// Like above, but through the **split** path the chat actually uses:
    /// a channel list whose `messages` child carries `split: right` +
    /// `smooth_scroll` + `row_layout`. Drilling opens (and focuses) a new
    /// split pane; `j` must scroll *that* pane.
    #[test]
    fn smooth_split_pane_scrolls_on_j() {
        use ratatui::{Terminal, backend::TestBackend};

        // Root channel list + a messages child opened in a right split.
        let mut config = smooth_chat_config();
        let root = &mut config.views[0];
        root.node_type = "mock:channel".into();
        root.row_layout = None;
        root.smooth_scroll = false;
        root.children = vec![ChildDef {
            card: None,
            row_layout: Some(vec![
                LineLayout {
                    columns: vec!["body".into()],
                    highlight_on_select: true,
                },
                LineLayout {
                    columns: vec![],
                    highlight_on_select: false,
                },
            ]),
            smooth_scroll: true,
            name: "messages".into(),
            node_type: "mock:msg".into(),
            columns: vec![ColumnDef {
                key: "body".into(),
                label: None,
                source: Some("label".into()),
                style: None,
                sizing: "flex(1)".into(),
                markdown: false,
                kind: ColumnKind::Text,
                format: None,
                separator: None,
                elapsed_from: None,
                tree_aggregate: None,
                hidden: false,
                collapsed_source: None,
                long_source: None,
            }],
            preview: None,
            actions: Vec::new(),
            children: Vec::new(),
            split: Some(SplitDef {
                direction: SplitDirection::Right,
                ratio: 0.8,
                coupled: false,
            }),
            pagination: None,
            keybindings: HashMap::new(),
            action_chains: Default::default(),
            column_cursor: false,
            record_detail: false,
            node_scripts: false,
            tree_label: None,
            shortcuts: HashMap::new(),
            enter_action: None,
            recursive: false,
            editor_in_place: false,
            leaf_glyph: None,
            icon: None,
            group_by: None,
            aggregates: Vec::new(),
            mark_read_on_reach_end: None,
            cursor_on_open: None,
        }];

        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(
            vec![tnode("chan1", "general", "mock:channel")],
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        let child_def = view.view_defs[0].children[0].clone();

        // Drill into the channel → split messages pane (focused).
        let result = view.dispatch_content_drill("chan1".into(), "general".into(), child_def);
        let pane_id = match result {
            SubViewMessage::Request(ViewRequest::DrillDown { pane_id, .. }) => pane_id,
            other => panic!("expected DrillDown, got {other:?}"),
        };
        // Load 30 two-line messages into the split pane.
        let items: Vec<_> = (0..30)
            .map(|i| tnode(&format!("m{i}"), &format!("message {i}"), "mock:msg"))
            .collect();
        view.set_items_for_pane(pane_id, items, Vec::new(), None, Vec::new(), None);
        assert_eq!(
            view.active_pane_id(),
            pane_id,
            "split pane is focused after drill"
        );
        assert!(
            view.active_pane().current_smooth_scroll(&view.view_defs),
            "messages pane is smooth"
        );

        let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
        let render = |view: &mut ContentView, terminal: &mut Terminal<TestBackend>| {
            terminal
                .draw(|f| {
                    let area = f.area();
                    view.active_pane_mut().table.view(f, area);
                })
                .unwrap();
            terminal.backend().buffer().clone()
        };

        let before = render(&mut view, &mut terminal);
        view.handle_key("j");
        view.rebuild_table();
        let after = render(&mut view, &mut terminal);

        assert_ne!(
            before, after,
            "pressing `j` must scroll the split smooth pane"
        );
    }

    /// The real chat uses a `markdown: true` column that expands into N
    /// soft-wrapped physical lines (a different multiline build path than a
    /// plain column). Make sure smooth scrolling still pans line-by-line
    /// there.
    #[test]
    fn smooth_markdown_pane_scrolls_on_j() {
        use ratatui::{Terminal, backend::TestBackend};

        let mut config = smooth_chat_config();
        // Turn the body column into a markdown column (stands alone on its
        // row_layout line, as the validator requires).
        config.views[0].columns[0].markdown = true;

        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        // Long, multi-paragraph bodies so each message wraps to several lines.
        let body = "# Heading\n\nFirst paragraph with enough words to wrap across the narrow pane width.\n\n- bullet one\n- bullet two";
        let items: Vec<_> = (0..20)
            .map(|i| tnode(&format!("m{i}"), &format!("{body} ({i})"), "mock:msg"))
            .collect();
        view.set_items(items, Vec::new(), None, Vec::new(), None);

        let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
        let render = |view: &mut ContentView, terminal: &mut Terminal<TestBackend>| {
            terminal
                .draw(|f| {
                    let area = f.area();
                    view.active_pane_mut().table.view(f, area);
                })
                .unwrap();
            terminal.backend().buffer().clone()
        };

        let before = render(&mut view, &mut terminal);
        view.handle_key("j");
        view.rebuild_table();
        let after = render(&mut view, &mut terminal);

        assert_ne!(
            before, after,
            "pressing `j` must scroll the markdown smooth pane"
        );
    }

    #[test]
    fn set_items_with_error() {
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(
            vec![],
            Vec::new(),
            None,
            Vec::new(),
            Some("connection failed".into()),
        );
        assert!(view.active_pane().items.is_empty());
        assert_eq!(
            view.active_pane().fetch_error.as_deref(),
            Some("connection failed")
        );
    }

    #[test]
    fn drill_down_prepare_changes_level() {
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);

        let child_def = config.views[0].children[0].clone();
        let view_defs = view.view_defs.clone();
        let child_type = view.active_pane_mut().drill_down_prepare(
            "ISS-1",
            "First issue",
            &child_def,
            &view_defs,
        );

        assert_eq!(child_type, "mock:comment");
        assert_eq!(view.nav_depth(), 1);
        assert!(view.active_pane().active_child.is_some());
        assert_eq!(
            view.active_pane().active_child.as_ref().unwrap().node_type,
            "mock:comment"
        );
        assert!(view.active_pane().items.is_empty());

        view.set_items(mock_comments(), Vec::new(), None, Vec::new(), None);
        assert_eq!(view.active_pane().items.len(), 2);
        assert_eq!(view.active_pane().items[0].id, "COM-1");
    }

    #[test]
    fn drill_down_changes_columns() {
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);

        let child_def = config.views[0].children[0].clone();
        let view_defs = view.view_defs.clone();
        view.active_pane_mut()
            .drill_down_prepare("ISS-1", "First issue", &child_def, &view_defs);

        let cols = view.active_pane().current_columns(&view.view_defs);
        assert_eq!(cols.len(), 1);
        assert_eq!(cols[0].key, "body");
    }

    #[test]
    fn drill_down_no_children_at_child_level() {
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);

        let child_def = config.views[0].children[0].clone();
        let view_defs = view.view_defs.clone();
        view.active_pane_mut()
            .drill_down_prepare("ISS-1", "First issue", &child_def, &view_defs);

        assert!(
            view.active_pane()
                .current_children(&view.view_defs)
                .is_empty()
        );
    }

    #[test]
    fn breadcrumbs_after_drill_down() {
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);

        let child_def = config.views[0].children[0].clone();
        let view_defs = view.view_defs.clone();
        view.active_pane_mut()
            .drill_down_prepare("ISS-1", "First issue", &child_def, &view_defs);

        let crumbs = view.breadcrumbs();
        assert_eq!(crumbs.len(), 2);
        assert_eq!(crumbs[0], "First issue");
        assert_eq!(crumbs[1], "Comments");
    }

    #[test]
    fn nav_back_restores_previous_level() {
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);
        assert_eq!(view.active_pane().items.len(), 2);

        let child_def = config.views[0].children[0].clone();
        let view_defs = view.view_defs.clone();
        view.active_pane_mut()
            .drill_down_prepare("ISS-1", "First issue", &child_def, &view_defs);
        view.set_items(mock_comments(), Vec::new(), None, Vec::new(), None);
        assert_eq!(view.active_pane().items.len(), 2);

        let view_defs = view.view_defs.clone();
        let went_back = view.active_pane_mut().nav_back(&view_defs);
        assert!(went_back);
        assert_eq!(view.nav_depth(), 0);
        assert!(view.active_pane().active_child.is_none());
        assert_eq!(view.active_pane().items.len(), 2);
        assert_eq!(view.active_pane().items[0].id, "ISS-1");
    }

    #[test]
    fn nav_back_at_root_returns_false() {
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let view_defs = view.view_defs.clone();
        assert!(!view.active_pane_mut().nav_back(&view_defs));
    }

    fn test_config_with_tree() -> ViewFileConfig {
        ViewFileConfig {
            reminder: None,
            tab: TabConfig {
                name: "Test".into(),
                order: 0,
                icon: None,
                key: None,
                unread_marker: None,
                unread_style: None,
                load_banner: None,
            },
            adapter: AdapterConfig {
                adapter_type: "mock".into(),
                id: None,
                config: None,
                config_inline: None,
                manual_connect: false,
            },
            views: vec![ViewDef {
                card: None,
                row_layout: None,
                smooth_scroll: false,
                name: "databases".into(),
                node_type: "mock:db".into(),
                default: true,
                window_ops: false,
                key: None,
                query: None,
                columns: vec![ColumnDef {
                    key: "name".into(),
                    label: Some("Name".into()),
                    source: Some("label".into()),
                    style: None,
                    sizing: "max".into(),
                    markdown: false,
                    kind: ColumnKind::Text,
                    format: None,
                    separator: None,
                    elapsed_from: None,
                    tree_aggregate: None,
                    hidden: false,
                    collapsed_source: None,
                    long_source: None,
                }],
                preview: None,
                actions: vec![],
                children: vec![ChildDef {
                    card: None,
                    row_layout: None,
                    smooth_scroll: false,
                    name: "Schemas".into(),
                    node_type: "mock:schema".into(),
                    columns: vec![ColumnDef {
                        key: "name".into(),
                        label: Some("Name".into()),
                        source: Some("label".into()),
                        style: None,
                        sizing: "max".into(),
                        markdown: false,
                        kind: ColumnKind::Text,
                        format: None,
                        separator: None,
                        elapsed_from: None,
                        tree_aggregate: None,
                        hidden: false,
                        collapsed_source: None,
                        long_source: None,
                    }],
                    preview: None,
                    actions: vec![],
                    children: vec![],
                    split: None,
                    pagination: None,
                    keybindings: HashMap::new(),
                    action_chains: Default::default(),
                    column_cursor: false,
                    record_detail: false,
                    node_scripts: false,
                    tree_label: Some("name".into()),
                    shortcuts: HashMap::new(),
                    enter_action: None,
                    recursive: false,
                    editor_in_place: false,
                    leaf_glyph: None,
                    icon: None,
                    group_by: None,
                    aggregates: Vec::new(),
                    mark_read_on_reach_end: None,
                    cursor_on_open: None,
                }],
                pagination: None,
                action_chains: Default::default(),
                column_cursor: false,
                record_detail: false,
                node_scripts: false,
                tree_label: Some("name".into()),
                retries: 0,
                script_template: None,
                script_source: None,
                shortcuts: HashMap::new(),
                leaf_glyph: None,
                icon: None,
                group_by: None,
                aggregates: Vec::new(),
                tree_connector_style: None,
                unread_style: None,
                unread_marker: None,
                tree_lines: None,
                tree_markers: None,
                expand_depth: None,
                group_headers: None,
                event_actions: Vec::new(),
            }],
        }
    }

    fn mock_dbs() -> Vec<NodeSummary> {
        vec![
            NodeSummary {
                id: "db1".into(),
                label: "db1".into(),
                node_type: not_yet_done_content::NodeType {
                    type_id: "mock:db".into(),
                    mime_type: "text/plain".into(),
                    syntax: None,
                    file_extension: ".txt".into(),
                    display_name: "DB".into(),
                },
                metadata: not_yet_done_content::Metadata::default(),
                has_children: None,
            },
            NodeSummary {
                id: "db2".into(),
                label: "db2".into(),
                node_type: not_yet_done_content::NodeType {
                    type_id: "mock:db".into(),
                    mime_type: "text/plain".into(),
                    syntax: None,
                    file_extension: ".txt".into(),
                    display_name: "DB".into(),
                },
                metadata: not_yet_done_content::Metadata::default(),
                has_children: None,
            },
        ]
    }

    fn mock_schemas() -> Vec<NodeSummary> {
        vec![
            NodeSummary {
                id: "public".into(),
                label: "public".into(),
                node_type: not_yet_done_content::NodeType {
                    type_id: "mock:schema".into(),
                    mime_type: "text/plain".into(),
                    syntax: None,
                    file_extension: ".txt".into(),
                    display_name: "Schema".into(),
                },
                metadata: not_yet_done_content::Metadata::default(),
                has_children: None,
            },
            NodeSummary {
                id: "private".into(),
                label: "private".into(),
                node_type: not_yet_done_content::NodeType {
                    type_id: "mock:schema".into(),
                    mime_type: "text/plain".into(),
                    syntax: None,
                    file_extension: ".txt".into(),
                    display_name: "Schema".into(),
                },
                metadata: not_yet_done_content::Metadata::default(),
                has_children: None,
            },
        ]
    }

    // ── Multi-branch render regression (chain-based label column) ────

    fn hcol(key: &str) -> ColumnDef {
        ColumnDef {
            key: key.into(),
            label: Some(key.into()),
            source: Some("label".into()),
            collapsed_source: None,
            long_source: None,
            style: None,
            sizing: "max".into(),
            markdown: false,
            kind: ColumnKind::Text,
            format: None,
            separator: None,
            elapsed_from: None,
            tree_aggregate: None,
            hidden: false,
        }
    }

    fn hchild(
        name: &str,
        node_type: &str,
        tree_label: Option<&str>,
        columns: Vec<ColumnDef>,
        children: Vec<ChildDef>,
    ) -> ChildDef {
        ChildDef {
            card: None,
            row_layout: None,
            smooth_scroll: false,
            name: name.into(),
            node_type: node_type.into(),
            columns,
            preview: None,
            actions: vec![],
            children,
            split: None,
            pagination: None,
            keybindings: HashMap::new(),
            action_chains: Default::default(),
            column_cursor: false,
            record_detail: false,
            node_scripts: false,
            tree_label: tree_label.map(String::from),
            shortcuts: HashMap::new(),
            enter_action: None,
            recursive: false,
            editor_in_place: false,
            leaf_glyph: None,
            icon: None,
            group_by: None,
            aggregates: Vec::new(),
            mark_read_on_reach_end: None,
            cursor_on_open: None,
        }
    }

    fn tnode(id: &str, label: &str, type_id: &str) -> NodeSummary {
        NodeSummary {
            id: id.into(),
            label: label.into(),
            node_type: not_yet_done_content::NodeType {
                type_id: type_id.into(),
                mime_type: "text/plain".into(),
                syntax: None,
                file_extension: ".txt".into(),
                display_name: type_id.into(),
            },
            metadata: not_yet_done_content::Metadata::default(),
            has_children: None,
        }
    }

    /// server ─┬─ dm (key `name`)  ── msg  (leaf)          [shallow 1st branch]
    ///         └─ cat (key `title`) ── chan (key `title`) ── msg (leaf)  [deeper, divergent key]
    fn heterogeneous_uneven_tree_config() -> ViewFileConfig {
        let dm_msg = hchild("dm_msg", "mock:dmmsg", None, vec![hcol("body")], vec![]);
        let dm = hchild(
            "dm",
            "mock:dm",
            Some("name"),
            vec![hcol("name")],
            vec![dm_msg],
        );
        let chan_msg = hchild("chan_msg", "mock:msg", None, vec![hcol("body")], vec![]);
        let chan = hchild(
            "chan",
            "mock:chan",
            Some("title"),
            vec![hcol("title")],
            vec![chan_msg],
        );
        let cat = hchild(
            "cat",
            "mock:cat",
            Some("title"),
            vec![hcol("title")],
            vec![chan],
        );
        ViewFileConfig {
            reminder: None,
            tab: TabConfig {
                name: "Chat".into(),
                order: 0,
                icon: None,
                key: None,
                unread_marker: None,
                unread_style: None,
                load_banner: None,
            },
            adapter: AdapterConfig {
                adapter_type: "mock".into(),
                id: None,
                config: None,
                config_inline: None,
                manual_connect: false,
            },
            views: vec![ViewDef {
                card: None,
                row_layout: None,
                smooth_scroll: false,
                name: "servers".into(),
                node_type: "mock:server".into(),
                default: true,
                window_ops: false,
                key: None,
                query: None,
                columns: vec![hcol("name")],
                preview: None,
                actions: vec![],
                children: vec![dm, cat],
                pagination: None,
                action_chains: Default::default(),
                column_cursor: false,
                record_detail: false,
                node_scripts: false,
                tree_label: Some("name".into()),
                retries: 0,
                script_template: None,
                script_source: None,
                shortcuts: HashMap::new(),
                leaf_glyph: None,
                icon: None,
                group_by: None,
                aggregates: Vec::new(),
                tree_connector_style: None,
                unread_style: None,
                unread_marker: None,
                tree_lines: None,
                tree_markers: None,
                expand_depth: None,
                group_headers: None,
                event_actions: Vec::new(),
            }],
        }
    }

    #[test]
    fn tree_renders_deep_branch_label_despite_divergent_keys() {
        // Regression for the recurring blank-label bug. A multi-branch
        // tree with unevenly-deep branches that use *different*
        // tree_label keys. The old renderer resolved the label column by
        // walking the FIRST branch by depth, so the deeper second branch
        // (whose depth maps to a leaf in the first branch) blanked out —
        // exactly the Stoat "channels under a category render empty"
        // case. The chain-based renderer paints every row's label into
        // the cursor level's designated label column.
        let config = heterogeneous_uneven_tree_config();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(
            vec![tnode("srv1", "srv1", "mock:server")],
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        let view_defs = view.view_defs.clone();
        {
            let pane = view.active_pane_mut();
            let tree = pane.tree.as_mut().expect("tree mode");
            // server → DM (first branch) + category (second branch)
            tree.set_cached_children(
                vec!["srv1".into()],
                vec![
                    tnode("dm1", "dm1", "mock:dm"),
                    tnode("cat1", "Cat", "mock:cat"),
                ],
                None,
            );
            tree.expanded.insert(vec!["srv1".into()]);
            // category → channel (the deep, divergent-key row)
            tree.set_cached_children(
                vec!["srv1".into(), "cat1".into()],
                vec![tnode("chan1", "general", "mock:chan")],
                None,
            );
            tree.expanded.insert(vec!["srv1".into(), "cat1".into()]);
            tree.rebuild_entries(&view_defs[0]);
        }
        // Cursor on the server (depth 0) — the realistic "just expanded".
        view.active_pane_mut().rebuild_table(&view_defs);
        view.active_pane_mut().table.set_selected(0);

        let pane = view.active_pane();
        // DFS order: srv1, dm1, cat1, chan1.
        let tree = pane.tree.as_ref().unwrap();
        assert_eq!(
            tree.entries
                .iter()
                .map(|e| e.node.id.as_str())
                .collect::<Vec<_>>(),
            vec!["srv1", "dm1", "cat1", "chan1"],
        );

        let columns = pane.current_columns(&view_defs);
        // Cursor level is the root → its label column key is "name".
        let label_key = TColumnId::new("name");
        let rows =
            pane.build_tree_data_rows(&columns, &view_defs, chrono::Local::now(), false, None);
        assert_eq!(rows.len(), 4, "every visible entry produces a row");

        // Every row — including the deep, divergent-key channel — must
        // carry a non-empty label cell in the designated label column.
        for (i, label) in ["srv1", "dm1", "Cat", "general"].iter().enumerate() {
            let cell = rows[i]
                .cells
                .get(&label_key)
                .map(|c| c.text.as_str())
                .unwrap_or("");
            assert!(
                cell.contains(label),
                "row {i} label cell empty/wrong: {cell:?} (expected to contain {label:?})",
            );
        }
    }

    /// Data column (read from the metadata field by `key`, not the node
    /// label) — `hcol` sources `label`, this one doesn't.
    fn dcol(key: &str) -> ColumnDef {
        let mut c = hcol(key);
        c.source = None;
        c
    }

    /// A `mock:task` node carrying a single `val` metadata field.
    fn tnode_val(id: &str, label: &str, val: &str) -> NodeSummary {
        use not_yet_done_content::{Metadata, MetadataField};
        let mut n = tnode(id, label, "mock:task");
        n.metadata = Metadata {
            fields: vec![MetadataField {
                key: "val".into(),
                value: val.into(),
                display_label: "Val".into(),
                editable: false,
                allowed_values: None,
            }],
        };
        n
    }

    /// Uniform recursive tree: every depth is `mock:task` with the same
    /// columns (`name` label + `val` data).
    fn uniform_recursive_config() -> ViewFileConfig {
        let mut child = hchild(
            "subtasks",
            "mock:task",
            Some("name"),
            vec![hcol("name"), dcol("val")],
            vec![],
        );
        child.recursive = true;
        ViewFileConfig {
            reminder: None,
            tab: TabConfig {
                name: "Tasks".into(),
                order: 0,
                icon: None,
                key: None,
                unread_marker: None,
                unread_style: None,
                load_banner: None,
            },
            adapter: AdapterConfig {
                adapter_type: "mock".into(),
                id: None,
                config: None,
                config_inline: None,
                manual_connect: false,
            },
            views: vec![ViewDef {
                card: None,
                row_layout: None,
                smooth_scroll: false,
                name: "tasks".into(),
                node_type: "mock:task".into(),
                default: true,
                window_ops: false,
                key: None,
                query: None,
                columns: vec![hcol("name"), dcol("val")],
                preview: None,
                actions: vec![],
                children: vec![child],
                pagination: None,
                action_chains: Default::default(),
                column_cursor: false,
                record_detail: false,
                node_scripts: false,
                tree_label: Some("name".into()),
                retries: 0,
                script_template: None,
                script_source: None,
                shortcuts: HashMap::new(),
                leaf_glyph: None,
                icon: None,
                group_by: None,
                aggregates: Vec::new(),
                tree_connector_style: None,
                unread_style: None,
                unread_marker: None,
                tree_lines: None,
                tree_markers: None,
                expand_depth: None,
                group_headers: None,
                event_actions: Vec::new(),
            }],
        }
    }

    #[test]
    fn uniform_recursive_tree_fills_data_cells_at_every_depth() {
        // Regression: in a uniform recursive tree (same node_type + columns
        // at every depth), the renderer used to fill data cells only on rows
        // whose chain exactly equalled the cursor's, blanking every other
        // depth. With the cursor on the root, the child row's `val` cell was
        // empty. Now each entry fills the columns its OWN level declares, so
        // every depth shows its data.
        let config = uniform_recursive_config();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(
            vec![tnode_val("root", "Root", "ROOTVAL")],
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        let view_defs = view.view_defs.clone();
        {
            let pane = view.active_pane_mut();
            let tree = pane.tree.as_mut().expect("tree mode");
            tree.set_cached_children(
                vec!["root".into()],
                vec![tnode_val("child", "Child", "CHILDVAL")],
                None,
            );
            tree.expanded.insert(vec!["root".into()]);
            tree.rebuild_entries(&view_defs[0]);
        }
        view.active_pane_mut().rebuild_table(&view_defs);
        // Cursor on the root (depth 0).
        view.active_pane_mut().table.set_selected(0);

        let pane = view.active_pane();
        let columns = pane.current_columns(&view_defs);
        let rows =
            pane.build_tree_data_rows(&columns, &view_defs, chrono::Local::now(), false, None);
        assert_eq!(rows.len(), 2, "root + expanded child");
        let val_key = TColumnId::new("val");
        let cell = |i: usize| {
            rows[i]
                .cells
                .get(&val_key)
                .map(|c| c.text.trim().to_string())
                .unwrap_or_default()
        };
        assert_eq!(cell(0), "ROOTVAL", "cursor-depth row shows its data");
        assert_eq!(
            cell(1),
            "CHILDVAL",
            "off-cursor-depth child row now shows its own data too",
        );
    }

    #[test]
    fn uniform_recursive_tree_renders_box_connectors() {
        // The generic tree draws ├──/└── box connectors (via the shared
        // forest helpers), identical to the native task tree — not a plain
        // indent + `·`.
        let config = uniform_recursive_config();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(
            vec![tnode_val("root", "Root", "RV")],
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        let view_defs = view.view_defs.clone();
        {
            let pane = view.active_pane_mut();
            let tree = pane.tree.as_mut().expect("tree mode");
            tree.set_cached_children(
                vec!["root".into()],
                vec![tnode_val("child", "Child", "CV")],
                None,
            );
            tree.expanded.insert(vec!["root".into()]);
            tree.rebuild_entries(&view_defs[0]);
        }
        view.active_pane_mut().rebuild_table(&view_defs);

        let pane = view.active_pane();
        let columns = pane.current_columns(&view_defs);
        let rows =
            pane.build_tree_data_rows(&columns, &view_defs, chrono::Local::now(), false, None);
        let name_key = TColumnId::new("name");
        let label = |i: usize| {
            rows[i]
                .cells
                .get(&name_key)
                .map(|c| c.text.clone())
                .unwrap_or_default()
        };
        // Root (depth 0, expanded): expand glyph, no box prefix.
        assert!(label(0).starts_with("▼ "), "root label: {:?}", label(0));
        assert!(label(0).contains("Root"));
        // Child (depth 1, last/only sibling): `└── ` box connector.
        assert!(
            label(1).contains("└── "),
            "child should carry a box connector: {:?}",
            label(1),
        );
        assert!(label(1).contains("Child"));
    }

    /// Build the standard two-row tree (expanded Root → expandable Child)
    /// from `config` and return the rendered tree-label cell texts.
    /// Shared by the `tree_lines` / `tree_markers` config tests.
    fn tree_label_texts(config: ViewFileConfig) -> Vec<String> {
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(
            vec![tnode_val("root", "Root", "RV")],
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        let view_defs = view.view_defs.clone();
        {
            let pane = view.active_pane_mut();
            let tree = pane.tree.as_mut().expect("tree mode");
            tree.set_cached_children(
                vec!["root".into()],
                vec![tnode_val("child", "Child", "CV")],
                None,
            );
            tree.expanded.insert(vec!["root".into()]);
            tree.rebuild_entries(&view_defs[0]);
        }
        view.active_pane_mut().rebuild_table(&view_defs);

        let pane = view.active_pane();
        let columns = pane.current_columns(&view_defs);
        let rows =
            pane.build_tree_data_rows(&columns, &view_defs, chrono::Local::now(), false, None);
        let name_key = TColumnId::new("name");
        rows.iter()
            .map(|r| {
                r.cells
                    .get(&name_key)
                    .map(|c| c.text.clone())
                    .unwrap_or_default()
            })
            .collect()
    }

    #[test]
    fn tree_lines_off_indents_without_box_connectors() {
        // `tree_lines: false` swaps the `├──`/`└──` box prefix for plain
        // two-space-per-depth indentation; the expand markers stay.
        let mut config = uniform_recursive_config();
        config.views[0].tree_lines = Some(false);
        let labels = tree_label_texts(config);
        assert!(labels[0].starts_with("▼ Root"), "root: {:?}", labels[0]);
        assert!(
            labels[1].starts_with("  ▶ Child"),
            "child should be indent + marker, no box glyphs: {:?}",
            labels[1],
        );
    }

    #[test]
    fn tree_markers_config_overrides_arrows() {
        use crate::config::view_config::TreeMarkerDef;
        let mut config = uniform_recursive_config();
        config.views[0].tree_markers = Some(TreeMarkerDef {
            enabled: None,
            collapsed: Some("+".into()),
            expanded: Some("-".into()),
        });
        let labels = tree_label_texts(config);
        assert!(labels[0].starts_with("- Root"), "root: {:?}", labels[0]);
        assert!(
            labels[1].starts_with("└── + Child"),
            "child: {:?}",
            labels[1]
        );
    }

    #[test]
    fn tree_markers_disabled_hides_arrows_keeps_lines() {
        use crate::config::view_config::TreeMarkerDef;
        let mut config = uniform_recursive_config();
        config.views[0].tree_markers = Some(TreeMarkerDef {
            enabled: Some(false),
            collapsed: None,
            expanded: None,
        });
        let labels = tree_label_texts(config);
        assert!(labels[0].starts_with("Root"), "root: {:?}", labels[0]);
        assert!(
            labels[1].starts_with("└── Child"),
            "child keeps the box connector, loses the arrow: {:?}",
            labels[1],
        );
    }

    #[test]
    fn unread_node_prefixes_marker_in_tree_label() {
        use not_yet_done_content::{Metadata, MetadataField};
        // A node carrying `unread = "true"` gets the configured marker woven
        // into its tree-label cell (after the connector + expand arrow), so an
        // unread channel reads as "new" at a glance. The default marker is 💬.
        let unread = |id: &str, label: &str| -> NodeSummary {
            let mut n = tnode(id, label, "mock:task");
            n.metadata = Metadata {
                fields: vec![MetadataField {
                    key: "unread".into(),
                    value: "true".into(),
                    display_label: "Unread".into(),
                    editable: false,
                    allowed_values: None,
                }],
            };
            n
        };

        let config = uniform_recursive_config();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(
            vec![unread("root", "Root")],
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        let view_defs = view.view_defs.clone();
        {
            let pane = view.active_pane_mut();
            let tree = pane.tree.as_mut().expect("tree mode");
            tree.set_cached_children(vec!["root".into()], vec![unread("child", "Child")], None);
            tree.expanded.insert(vec!["root".into()]);
            tree.rebuild_entries(&view_defs[0]);
        }
        view.active_pane_mut().rebuild_table(&view_defs);

        let pane = view.active_pane();
        let columns = pane.current_columns(&view_defs);
        let rows =
            pane.build_tree_data_rows(&columns, &view_defs, chrono::Local::now(), false, None);
        let name_key = TColumnId::new("name");
        let label = |i: usize| -> String {
            rows[i]
                .cells
                .get(&name_key)
                .map(|c| c.text.clone())
                .unwrap_or_default()
        };
        // Root: expand arrow, then the marker, then the label.
        assert_eq!(label(0), "▼ 💬 Root", "root: {:?}", label(0));
        // Child: box connector + arrow, then marker, then label.
        assert_eq!(label(1), "└── ▶ 💬 Child", "child: {:?}", label(1));
    }

    /// A flat message list with `count` rows, unread from index
    /// `unread_from` on (`None` = everything read).
    fn unread_messages(count: usize, unread_from: Option<usize>) -> Vec<NodeSummary> {
        use not_yet_done_content::{Metadata, MetadataField};
        (0..count)
            .map(|i| {
                let mut n = tnode(&format!("m{i}"), &format!("m{i}"), "mock:msg");
                if unread_from.is_some_and(|from| i >= from) {
                    n.metadata = Metadata {
                        fields: vec![MetadataField {
                            key: "unread".into(),
                            value: "true".into(),
                            display_label: "Unread".into(),
                            editable: false,
                            allowed_values: None,
                        }],
                    };
                }
                n
            })
            .collect()
    }

    /// Drill into a flat `messages` level configured with `placement`, land
    /// `count` rows on it (unread from `unread_from`) and report where the
    /// cursor ended up.
    fn open_messages_level(
        placement: Option<CursorOnOpen>,
        count: usize,
        unread_from: Option<usize>,
    ) -> usize {
        let config = heterogeneous_uneven_tree_config();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let view_defs = view.view_defs.clone();
        let mut child = hchild("messages", "mock:msg", None, vec![hcol("name")], vec![]);
        child.cursor_on_open = placement;
        let pane = view.active_pane_mut();
        pane.tree = None; // flat list, not tree mode
        pane.drill_down_prepare("c1", "Channel", &child, &view_defs);
        pane.set_items(
            unread_messages(count, unread_from),
            Vec::new(),
            None,
            Vec::new(),
            None,
            &view_defs,
        );
        pane.table.selected_row()
    }

    #[test]
    fn cursor_on_open_first_unread_opens_on_the_oldest_unread_row() {
        // Opening a chat should start reading where the user left off, not at
        // the top of the loaded page. The placement is armed by the drill and
        // applied by the load that follows it.
        assert_eq!(
            open_messages_level(Some(CursorOnOpen::FirstUnread), 5, Some(3)),
            3,
            "cursor on the first of the unread rows"
        );
        assert_eq!(
            open_messages_level(Some(CursorOnOpen::FirstUnread), 5, Some(0)),
            0,
            "everything unread → the oldest one"
        );
        // Nothing unread: no catching-up to do, so the newest row is what the
        // user came for (documented on `CursorOnOpen::FirstUnread`).
        assert_eq!(
            open_messages_level(Some(CursorOnOpen::FirstUnread), 5, None),
            4,
            "all read → the newest row"
        );
        // The other placements, and the unconfigured default.
        assert_eq!(open_messages_level(Some(CursorOnOpen::Last), 5, Some(3)), 4);
        assert_eq!(
            open_messages_level(Some(CursorOnOpen::First), 5, Some(3)),
            0
        );
        assert_eq!(
            open_messages_level(None, 5, Some(3)),
            0,
            "no opt-in → the historical first row"
        );
        // An empty page has nothing to place the cursor on — and must not
        // panic on the `rows - 1` arithmetic.
        assert_eq!(
            open_messages_level(Some(CursorOnOpen::FirstUnread), 0, None),
            0
        );
    }

    #[test]
    fn cursor_on_open_applies_to_the_open_not_to_later_reloads() {
        // The placement belongs to *opening* the level. A refresh, a live
        // invalidation or an incoming message re-runs `set_items` — none of
        // them may yank the cursor away from where the user scrolled to.
        let config = heterogeneous_uneven_tree_config();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let view_defs = view.view_defs.clone();
        let mut child = hchild("messages", "mock:msg", None, vec![hcol("name")], vec![]);
        child.cursor_on_open = Some(CursorOnOpen::FirstUnread);
        let pane = view.active_pane_mut();
        pane.tree = None;
        pane.drill_down_prepare("c1", "Channel", &child, &view_defs);

        let load = |pane: &mut ContentPane| {
            pane.set_items(
                unread_messages(5, Some(3)),
                Vec::new(),
                None,
                Vec::new(),
                None,
                &view_defs,
            );
        };
        load(pane);
        assert_eq!(pane.table.selected_row(), 3, "opened on the first unread");

        // User scrolls back into the history, then a reload lands.
        pane.table.set_selected(1);
        load(pane);
        assert_eq!(
            pane.table.selected_row(),
            1,
            "reload keeps the user's cursor"
        );
    }

    #[test]
    fn reload_re_selects_the_same_node_after_the_page_shifted() {
        // A flat feed renders its newest page: one incoming message slides the
        // whole window up by a row. Restoring the cursor by row index would
        // land on the neighbour, so the reload re-selects by node id.
        let config = heterogeneous_uneven_tree_config();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let view_defs = view.view_defs.clone();
        let child = hchild("messages", "mock:msg", None, vec![hcol("name")], vec![]);
        let pane = view.active_pane_mut();
        pane.tree = None;
        pane.drill_down_prepare("c1", "Channel", &child, &view_defs);

        let page = |from: usize| -> Vec<NodeSummary> {
            (from..from + 5)
                .map(|i| tnode(&format!("m{i}"), &format!("m{i}"), "mock:msg"))
                .collect()
        };
        let load = |pane: &mut ContentPane, from: usize| {
            pane.set_items(page(from), Vec::new(), None, Vec::new(), None, &view_defs);
        };

        load(pane, 0); // m0..m4
        pane.table.set_selected(2); // m2
        load(pane, 1); // m1..m5 — everything moved up one row
        assert_eq!(
            pane.selected_item_id(),
            Some("m2"),
            "cursor stays on the same message"
        );
        assert_eq!(pane.table.selected_row(), 1, "which is now one row higher");

        // The selected node is gone (paged out): nothing to re-anchor to, so
        // the table's own index restore stands.
        load(pane, 9); // m9..m13
        assert_eq!(pane.table.selected_row(), 1, "index fallback");
    }

    #[test]
    fn mark_read_on_reach_end_queues_only_on_fresh_arrival_at_unread_last() {
        use not_yet_done_content::{Metadata, MetadataField};
        // The generic `mark_read_on_reach_end` hook fires once when the cursor
        // first lands on the still-unread LAST row of a flat drill level. Two
        // gates keep it honest: arrival (`before_row != last`, so merely
        // opening the list or pressing a key while already at the bottom does
        // not ack) and unread (so it never re-fires after the ack-driven
        // reload, which flips the row to read).
        let msg = |id: &str, unread: bool| -> NodeSummary {
            let mut n = tnode(id, id, "mock:msg");
            if unread {
                n.metadata = Metadata {
                    fields: vec![MetadataField {
                        key: "unread".into(),
                        value: "true".into(),
                        display_label: "Unread".into(),
                        editable: false,
                        allowed_values: None,
                    }],
                };
            }
            n
        };

        // Configure the focused pane as a flat message list whose ChildDef
        // carries the hook, with two rows (an older read one, a newest one).
        let setup = |last_unread: bool| -> ContentView {
            let config = heterogeneous_uneven_tree_config();
            let mut view =
                ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
            let view_defs = view.view_defs.clone();
            let mut child = hchild("messages", "mock:msg", None, vec![hcol("name")], vec![]);
            child.mark_read_on_reach_end = Some("mark-read".into());
            let pane = view.active_pane_mut();
            pane.tree = None; // flat list, not tree mode
            pane.active_child = Some(child);
            pane.items = vec![msg("m1", false), msg("m2", last_unread)];
            pane.filtered_indices = vec![0, 1];
            pane.rebuild_table(&view_defs);
            view
        };

        // Fresh arrival at the unread last row → queues a mark-read invoke on
        // that very node.
        let mut view = setup(true);
        view.active_pane_mut().table.set_selected(1);
        view.detect_mark_read_reached(0);
        match view.take_pending_mark_read() {
            Some(ViewRequest::InvokeNodeAction {
                node_id,
                action_name,
                ..
            }) => {
                assert_eq!(node_id, "m2");
                assert_eq!(action_name, "mark-read");
            }
            other => panic!("expected an InvokeNodeAction, got {other:?}"),
        }

        // Already at the last row (no arrival) → nothing queued.
        let mut view = setup(true);
        view.active_pane_mut().table.set_selected(1);
        view.detect_mark_read_reached(1);
        assert!(
            view.take_pending_mark_read().is_none(),
            "no arrival, no ack"
        );

        // Arrival, but the last row is already read → nothing queued.
        let mut view = setup(false);
        view.active_pane_mut().table.set_selected(1);
        view.detect_mark_read_reached(0);
        assert!(view.take_pending_mark_read().is_none(), "read row → no ack");
    }

    #[test]
    fn filtered_item_ids_follows_fuzzy_filter() {
        // `scope: filtered_set` batch scripts hand over exactly what the user
        // sees: the whole loaded (query-filtered) list when no fuzzy filter is
        // active, the matched subset when it is — empty matches yield no ids.
        let config = uniform_recursive_config();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let pane = view.active_pane_mut();
        pane.tree = None;
        pane.items = vec![
            tnode_val("a", "Alpha", "1"),
            tnode_val("b", "Beta", "2"),
            tnode_val("c", "Gamma", "3"),
        ];

        pane.table.fuzzy_active = false;
        assert_eq!(
            pane.filtered_item_ids(),
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
        );

        pane.table.fuzzy_active = true;
        pane.filtered_indices = vec![0, 2];
        assert_eq!(
            pane.filtered_item_ids(),
            vec!["a".to_string(), "c".to_string()],
        );

        pane.filtered_indices = vec![];
        assert!(pane.filtered_item_ids().is_empty());
    }

    #[test]
    fn tree_label_cell_carries_connector_style_span() {
        // The connector run (box glyphs + expand arrow) of a tree-label cell
        // is tagged with `TREE_CONNECTOR_STYLE_ID` via a `StyledSpan`, so the
        // render path can paint it apart from the label. The span must cover
        // exactly the connector prefix — not the label text.
        let config = uniform_recursive_config();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(
            vec![tnode_val("root", "Root", "RV")],
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        let view_defs = view.view_defs.clone();
        {
            let pane = view.active_pane_mut();
            let tree = pane.tree.as_mut().expect("tree mode");
            tree.set_cached_children(
                vec!["root".into()],
                vec![tnode_val("child", "Child", "CV")],
                None,
            );
            tree.expanded.insert(vec!["root".into()]);
            tree.rebuild_entries(&view_defs[0]);
        }
        view.active_pane_mut().rebuild_table(&view_defs);

        let pane = view.active_pane();
        let columns = pane.current_columns(&view_defs);
        let rows =
            pane.build_tree_data_rows(&columns, &view_defs, chrono::Local::now(), false, None);
        let name_key = TColumnId::new("name");
        let cell = |i: usize| rows[i].cells.get(&name_key).expect("label cell");

        // Every label cell carries exactly one connector span, styled with the
        // dedicated slot, starting at char 0.
        for i in 0..rows.len() {
            let spans = &cell(i).spans;
            assert_eq!(spans.len(), 1, "row {i} should have one connector span");
            assert_eq!(spans[0].style_id, TREE_CONNECTOR_STYLE_ID, "row {i}");
            assert_eq!(spans[0].range.start, 0, "row {i} span starts at 0");
        }
        // The span must end where the connector ends — i.e. its char-range
        // delimits exactly the box/arrow prefix, leaving the label untouched.
        let span_prefix = |i: usize| -> String {
            let c = cell(i);
            c.text.chars().take(c.spans[0].range.end).collect()
        };
        assert_eq!(span_prefix(0), "▼ ", "root connector = expand arrow");
        // The child is itself expandable (uniform recursive tree), so its
        // connector is the box prefix *plus* the collapsed-arrow — both are
        // part of the styled connector run, the label `Child` is not.
        assert_eq!(
            span_prefix(1),
            "└── ▶ ",
            "child connector = box glyphs + arrow"
        );
    }

    #[test]
    fn tree_label_cell_segments_splits_connector_from_label() {
        // Helper: first `connector_chars` chars → styled segment, rest plain.
        let segs = tree_label_cell_segments("└── Child  ", 4, TREE_CONNECTOR_STYLE_ID, None);
        assert_eq!(
            segs,
            vec![
                ("└── ".to_string(), Some(TREE_CONNECTOR_STYLE_ID)),
                ("Child  ".to_string(), None),
            ],
        );
        // Zero-length connector (e.g. a flat leaf) → single plain segment.
        assert_eq!(
            tree_label_cell_segments("Root  ", 0, TREE_CONNECTOR_STYLE_ID, None),
            vec![("Root  ".to_string(), None)],
        );
        // Connector longer than the (truncated) cell → whole cell is connector,
        // no trailing label segment. Guards the byte-offset clamp.
        assert_eq!(
            tree_label_cell_segments("└─", 9, TREE_CONNECTOR_STYLE_ID, None),
            vec![("└─".to_string(), Some(TREE_CONNECTOR_STYLE_ID))],
        );
        // A `base_style_id` paints the label remainder (unread highlight) while
        // leaving the connector run in its own slot.
        assert_eq!(
            tree_label_cell_segments(
                "└── #general ",
                4,
                TREE_CONNECTOR_STYLE_ID,
                Some(UNREAD_STYLE_ID)
            ),
            vec![
                ("└── ".to_string(), Some(TREE_CONNECTOR_STYLE_ID)),
                ("#general ".to_string(), Some(UNREAD_STYLE_ID)),
            ],
        );
    }

    #[test]
    fn fuzzy_label_ranges_merges_consecutive_and_unions_tokens() {
        // Contiguous match collapses into one range.
        assert_eq!(fuzzy_label_ranges("hello world", "hello"), vec![0..5]);
        // No filter → no ranges.
        assert_eq!(
            fuzzy_label_ranges("hello", "   "),
            Vec::<std::ops::Range<usize>>::new(),
        );
        // Two whitespace-separated tokens union their matched runs.
        let ranges = fuzzy_label_ranges("alpha beta", "alpha beta");
        assert_eq!(ranges, vec![0..5, 6..10]);
        // A label with no match (row survived via another field) → empty.
        assert_eq!(
            fuzzy_label_ranges("nothing", "zzz"),
            Vec::<std::ops::Range<usize>>::new(),
        );
    }

    #[test]
    fn tree_label_segments_split_connector_and_highlight() {
        // Connector (0..4) + a fuzzy match inside the label ("Child" at chars
        // 4..9, highlight the "hi" run at 5..7) → three segments: connector,
        // plain lead, highlighted run, plain tail.
        let segs = tree_label_segments_with_highlights(
            "└── Child ",
            4,
            TREE_CONNECTOR_STYLE_ID,
            &[5..7],
            FUZZY_MATCH_STYLE_ID,
            None,
        );
        assert_eq!(
            segs,
            vec![
                ("└── ".to_string(), Some(TREE_CONNECTOR_STYLE_ID)),
                ("C".to_string(), None),
                ("hi".to_string(), Some(FUZZY_MATCH_STYLE_ID)),
                ("ld ".to_string(), None),
            ],
        );
        // No highlights → falls back to the plain connector split.
        assert_eq!(
            tree_label_segments_with_highlights(
                "└── Child ",
                4,
                TREE_CONNECTOR_STYLE_ID,
                &[],
                FUZZY_MATCH_STYLE_ID,
                None,
            ),
            tree_label_cell_segments("└── Child ", 4, TREE_CONNECTOR_STYLE_ID, None),
        );
        // With a base style, the plain runs around a match carry it (unread +
        // fuzzy combined): connector, unread lead, matched run, unread tail.
        assert_eq!(
            tree_label_segments_with_highlights(
                "└── Child ",
                4,
                TREE_CONNECTOR_STYLE_ID,
                &[5..7],
                FUZZY_MATCH_STYLE_ID,
                Some(UNREAD_STYLE_ID),
            ),
            vec![
                ("└── ".to_string(), Some(TREE_CONNECTOR_STYLE_ID)),
                ("C".to_string(), Some(UNREAD_STYLE_ID)),
                ("hi".to_string(), Some(FUZZY_MATCH_STYLE_ID)),
                ("ld ".to_string(), Some(UNREAD_STYLE_ID)),
            ],
        );
        // Zero connector (root leaf) + a match at the very start of the label.
        assert_eq!(
            tree_label_segments_with_highlights(
                "Root ",
                0,
                TREE_CONNECTOR_STYLE_ID,
                &[0..4],
                FUZZY_MATCH_STYLE_ID,
                None,
            ),
            vec![
                ("Root".to_string(), Some(FUZZY_MATCH_STYLE_ID)),
                (" ".to_string(), None),
            ],
        );
    }

    // ── Tree-fold aggregation (M4) ───────────────────────────────────

    /// Single-level tree config whose root carries a label column (`name`)
    /// and a `tree_aggregate` column (`dur`, cumulated field `dur_cum`).
    /// `kind: Number` so the cell renders the value verbatim (the toggle
    /// path is what's under test, not duration formatting).
    fn tree_aggregate_config(default: TreeAggregateDefault) -> ViewFileConfig {
        let dur = ColumnDef {
            key: "dur".into(),
            label: Some("Dur".into()),
            source: None,
            collapsed_source: None,
            long_source: None,
            style: None,
            sizing: "max".into(),
            markdown: false,
            kind: ColumnKind::Number,
            format: None,
            separator: None,
            elapsed_from: None,
            tree_aggregate: Some(crate::config::view_config::TreeAggregate {
                cumulated_field: "dur_cum".into(),
                default,
            }),
            hidden: false,
        };
        ViewFileConfig {
            reminder: None,
            tab: TabConfig {
                name: "Worklog".into(),
                order: 0,
                icon: None,
                key: None,
                unread_marker: None,
                unread_style: None,
                load_banner: None,
            },
            adapter: AdapterConfig {
                adapter_type: "mock".into(),
                id: None,
                config: None,
                config_inline: None,
                manual_connect: false,
            },
            views: vec![ViewDef {
                card: None,
                row_layout: None,
                smooth_scroll: false,
                name: "tasks".into(),
                node_type: "mock:task".into(),
                default: true,
                window_ops: false,
                key: None,
                query: None,
                columns: vec![hcol("name"), dur],
                preview: None,
                actions: vec![],
                children: vec![],
                pagination: None,
                action_chains: Default::default(),
                column_cursor: false,
                record_detail: false,
                node_scripts: false,
                tree_label: Some("name".into()),
                retries: 0,
                script_template: None,
                script_source: None,
                shortcuts: HashMap::new(),
                leaf_glyph: None,
                icon: None,
                group_by: None,
                aggregates: Vec::new(),
                tree_connector_style: None,
                unread_style: None,
                unread_marker: None,
                tree_lines: None,
                tree_markers: None,
                expand_depth: None,
                group_headers: None,
                event_actions: Vec::new(),
            }],
        }
    }

    /// A tree node carrying both the own (`dur`) and cumulated (`dur_cum`)
    /// metadata fields the `tree_aggregate` column toggles between.
    /// A mock adapter that advertises `supports_tree_aggregation` so the
    /// capability gate in [`ContentPane::level_has_tree_aggregate`] opens.
    /// Items are injected via `set_items`, so the node tree is irrelevant.
    fn tree_aggregating_adapter() -> Arc<dyn ContentAdapter> {
        Arc::new(
            MockAdapterBuilder::new("mock")
                .capabilities(not_yet_done_content::AdapterCapabilities {
                    supports_tree_aggregation: true,
                    ..Default::default()
                })
                .build(),
        )
    }

    fn tnode_dur(id: &str, label: &str, own: &str, cumulated: &str) -> NodeSummary {
        use not_yet_done_content::{Metadata, MetadataField};
        let field = |k: &str, v: &str| MetadataField {
            key: k.into(),
            value: v.into(),
            display_label: k.into(),
            editable: false,
            allowed_values: None,
        };
        let mut n = tnode(id, label, "mock:task");
        n.metadata = Metadata {
            fields: vec![field("dur", own), field("dur_cum", cumulated)],
        };
        n
    }

    /// Render the single visible row's `dur` cell text for a pane built from
    /// `tree_aggregate_config(default)`, optionally after firing the toggle.
    fn tree_aggregate_dur_cell(default: TreeAggregateDefault, toggle: bool) -> String {
        let config = tree_aggregate_config(default);
        let mut view = ContentView::new(
            test_theme(),
            &config,
            Some(tree_aggregating_adapter()),
            &KeyBindingConfig::default(),
        );
        view.set_items(
            vec![tnode_dur("t1", "Task 1", "30", "100")],
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        let view_defs = view.view_defs.clone();
        view.active_pane_mut().rebuild_table(&view_defs);
        view.active_pane_mut().table.set_selected(0);
        if toggle {
            assert!(
                view.active_pane_mut().toggle_tree_aggregate(&view_defs),
                "toggle must apply on a level with a tree_aggregate column",
            );
        }
        let pane = view.active_pane();
        let columns = pane.current_columns(&view_defs);
        let rows =
            pane.build_tree_data_rows(&columns, &view_defs, chrono::Local::now(), false, None);
        assert_eq!(rows.len(), 1, "exactly one visible row");
        rows[0]
            .cells
            .get(&TColumnId::new("dur"))
            .map(|c| c.text.trim().to_string())
            .unwrap_or_default()
    }

    #[test]
    fn tree_aggregate_default_own_shows_node_value() {
        // No toggle, default `own` → the column's own `dur` field (30).
        assert_eq!(
            tree_aggregate_dur_cell(TreeAggregateDefault::Own, false),
            "30"
        );
    }

    #[test]
    fn tree_aggregate_default_cumulated_shows_cumulated_field() {
        // No toggle, default `cumulated` → the `dur_cum` field (100).
        assert_eq!(
            tree_aggregate_dur_cell(TreeAggregateDefault::Cumulated, false),
            "100",
        );
    }

    #[test]
    fn tree_aggregate_toggle_flips_own_to_cumulated() {
        // Default `own` (30) → one toggle flips to the cumulated field (100).
        assert_eq!(
            tree_aggregate_dur_cell(TreeAggregateDefault::Own, true),
            "100"
        );
    }

    #[test]
    fn tree_aggregate_toggle_flips_cumulated_to_own() {
        // Default `cumulated` (100) → one toggle flips back to own (30).
        assert_eq!(
            tree_aggregate_dur_cell(TreeAggregateDefault::Cumulated, true),
            "30",
        );
    }

    #[test]
    fn tree_aggregate_toggle_noop_without_column() {
        // A plain tree level (no tree_aggregate column) → toggle is a no-op
        // and the action stays unclaimable.
        let config = test_config_with_tree();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        let view_defs = view.view_defs.clone();
        assert!(!view.active_pane().level_has_tree_aggregate(&view_defs));
        assert!(!view.active_pane_mut().toggle_tree_aggregate(&view_defs));
        match view.active_pane_mut().try_toggle_tree_aggregate(&view_defs) {
            SubViewMessage::Unhandled => {}
            other => panic!("expected Unhandled without a tree_aggregate column, got {other:?}"),
        }
    }

    #[test]
    fn tree_aggregate_gated_off_without_capability() {
        // The config *does* declare a `tree_aggregate` column, but the
        // adapter (here: none → all-false capabilities) does not advertise
        // `supports_tree_aggregation`. The capability gate must keep the
        // toggle unclaimable and a no-op even though the column is present —
        // config alone is not enough.
        let config = tree_aggregate_config(TreeAggregateDefault::Cumulated);
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(
            vec![tnode_dur("t1", "Task 1", "30", "100")],
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        let view_defs = view.view_defs.clone();
        assert!(
            !view.active_pane().level_has_tree_aggregate(&view_defs),
            "capability absent → gate closed despite the tree_aggregate column",
        );
        assert!(!view.active_pane_mut().toggle_tree_aggregate(&view_defs));
        match view.active_pane_mut().try_toggle_tree_aggregate(&view_defs) {
            SubViewMessage::Unhandled => {}
            other => panic!("expected Unhandled without the capability, got {other:?}"),
        }
    }

    #[test]
    fn tree_aggregate_claimable_with_capability_and_column() {
        // Mirror of the gate-off test: column present *and* the adapter
        // advertises `supports_tree_aggregation` → the toggle is claimable.
        let config = tree_aggregate_config(TreeAggregateDefault::Cumulated);
        let mut view = ContentView::new(
            test_theme(),
            &config,
            Some(tree_aggregating_adapter()),
            &KeyBindingConfig::default(),
        );
        view.set_items(
            vec![tnode_dur("t1", "Task 1", "30", "100")],
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        let view_defs = view.view_defs.clone();
        assert!(
            view.active_pane().level_has_tree_aggregate(&view_defs),
            "capability + column → gate open",
        );
    }

    #[test]
    fn tree_back_at_depth_zero_unhandled() {
        let config = test_config_with_tree();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        let view_defs = view.view_defs.clone();
        match view.active_pane_mut().try_back(&view_defs) {
            SubViewMessage::Unhandled => {}
            other => panic!("expected Unhandled at depth 0, got {other:?}"),
        }
    }

    #[test]
    fn next_page_after_arms_when_has_next() {
        use not_yet_done_content::PageInfo;
        let info = PageInfo {
            offset: 0,
            limit: 50,
            total: Some(125),
            has_next: true,
            has_prev: false,
        };
        let np = next_page_after(Some(info)).expect("has_next → Some");
        assert_eq!(np.offset, 50);
        assert_eq!(np.limit, 50);
    }

    #[test]
    fn next_page_after_returns_none_when_done() {
        use not_yet_done_content::PageInfo;
        let info = PageInfo {
            offset: 50,
            limit: 50,
            total: Some(100),
            has_next: false,
            has_prev: true,
        };
        assert!(next_page_after(Some(info)).is_none());
        assert!(next_page_after(None).is_none());
    }

    /// A leaf summary with the given id (type is irrelevant to the cache
    /// path scheme — `ingest_subtree_level` keys purely on `id`).
    fn sub_summary(id: &str) -> NodeSummary {
        NodeSummary {
            id: id.into(),
            label: id.into(),
            node_type: not_yet_done_content::NodeType {
                type_id: "mock:node".into(),
                mime_type: "text/plain".into(),
                syntax: None,
                file_extension: ".txt".into(),
                display_name: "Node".into(),
            },
            metadata: not_yet_done_content::Metadata::default(),
            has_children: None,
        }
    }

    fn sub_node(
        id: &str,
        children: Vec<not_yet_done_content::SubtreeNode>,
    ) -> not_yet_done_content::SubtreeNode {
        not_yet_done_content::SubtreeNode {
            summary: sub_summary(id),
            children: Subtree {
                items: children,
                page: None,
            },
        }
    }

    /// The eager-ingest path must lay down a cache + `expanded` set that is
    /// byte-for-byte identical to what the per-node cascade would build —
    /// otherwise selection, collapse, and re-expand desync. This pins the
    /// path scheme: a node's children live at `parent_path + [node.id]`, and
    /// exactly the nodes that carry children end up in `expanded`.
    #[test]
    fn ingest_subtree_matches_cascade_path_scheme() {
        // root → A → {A1 → A1a(leaf), A2(leaf)}, B(leaf)
        let subtree = Subtree {
            items: vec![
                sub_node(
                    "A",
                    vec![
                        sub_node("A1", vec![sub_node("A1a", vec![])]),
                        sub_node("A2", vec![]),
                    ],
                ),
                sub_node("B", vec![]),
            ],
            page: None,
        };

        let mut eager = TreeState::new();
        ingest_subtree_level(&mut eager, Vec::new(), subtree, false);

        // Cache key = full id-chain from root, identical to flatten_into.
        assert_eq!(
            eager.cache[&Vec::<String>::new()]
                .children
                .iter()
                .map(|n| n.id.clone())
                .collect::<Vec<_>>(),
            vec!["A".to_string(), "B".to_string()],
        );
        assert_eq!(
            eager.cache[&vec!["A".to_string()]]
                .children
                .iter()
                .map(|n| n.id.clone())
                .collect::<Vec<_>>(),
            vec!["A1".to_string(), "A2".to_string()],
        );
        assert_eq!(
            eager.cache[&vec!["A".to_string(), "A1".to_string()]]
                .children
                .iter()
                .map(|n| n.id.clone())
                .collect::<Vec<_>>(),
            vec!["A1a".to_string()],
        );
        // Leaves get no cache slot (no children laid down).
        assert!(!eager.cache.contains_key(&vec!["B".to_string()]));
        assert!(
            !eager
                .cache
                .contains_key(&vec!["A".to_string(), "A2".to_string()])
        );
        assert!(!eager.cache.contains_key(&vec![
            "A".to_string(),
            "A1".to_string(),
            "A1a".to_string()
        ]));

        // Exactly the parents-with-children are expanded.
        let expected_expanded: std::collections::HashSet<Vec<String>> = [
            vec!["A".to_string()],
            vec!["A".to_string(), "A1".to_string()],
        ]
        .into_iter()
        .collect();
        assert_eq!(eager.expanded, expected_expanded);

        // Now build the same shape the way the cascade does — one
        // set_cached_children per parent path + one expanded.insert per
        // opened node — and assert the two states are indistinguishable.
        let mut cascade = TreeState::new();
        cascade.set_cached_children(Vec::new(), vec![sub_summary("A"), sub_summary("B")], None);
        cascade.expanded.insert(vec!["A".to_string()]);
        cascade.set_cached_children(
            vec!["A".to_string()],
            vec![sub_summary("A1"), sub_summary("A2")],
            None,
        );
        cascade
            .expanded
            .insert(vec!["A".to_string(), "A1".to_string()]);
        cascade.set_cached_children(
            vec!["A".to_string(), "A1".to_string()],
            vec![sub_summary("A1a")],
            None,
        );

        assert_eq!(eager.expanded, cascade.expanded);
        assert_eq!(
            eager.cache.keys().collect::<std::collections::HashSet<_>>(),
            cascade
                .cache
                .keys()
                .collect::<std::collections::HashSet<_>>(),
        );
        for (key, ec) in &eager.cache {
            let cc = &cascade.cache[key];
            assert_eq!(
                ec.children.iter().map(|n| n.id.clone()).collect::<Vec<_>>(),
                cc.children.iter().map(|n| n.id.clone()).collect::<Vec<_>>(),
                "children mismatch at {key:?}",
            );
            assert_eq!(ec.loaded, cc.loaded, "loaded mismatch at {key:?}");
            assert_eq!(ec.next_page, cc.next_page, "next_page mismatch at {key:?}");
        }
    }

    /// Build a `SubtreeNode` carrying an explicit `has_children` so the
    /// genuine-leaf (`Some(false)`) vs. depth-frontier (`Some(true)`)
    /// distinction can be exercised — `sub_node` leaves it `None`.
    fn sub_node_hc(
        id: &str,
        has_children: Option<bool>,
        children: Vec<not_yet_done_content::SubtreeNode>,
    ) -> not_yet_done_content::SubtreeNode {
        let mut summary = sub_summary(id);
        summary.has_children = has_children;
        not_yet_done_content::SubtreeNode {
            summary,
            children: Subtree {
                items: children,
                page: None,
            },
        }
    }

    /// Regression (Tasks delete-last-child): a reload whose fresh subtree
    /// reports a node as a genuine leaf (`has_children == Some(false)`, empty
    /// `children`) must positively clear that node's previously-cached
    /// children and drop it from `expanded`. Otherwise the stale rows keep
    /// rendering under it (the deleted task lingers even though its parent's
    /// expand marker is already gone). A depth-frontier node
    /// (`has_children == Some(true)`, empty `children`) must be left intact
    /// so a later lazy expand can still fill it.
    #[test]
    fn ingest_subtree_clears_children_of_emptied_leaf() {
        // First load: P (root) → C (which itself had a child GC).
        let first = Subtree {
            items: vec![sub_node_hc(
                "P",
                Some(true),
                vec![sub_node_hc(
                    "C",
                    Some(true),
                    vec![sub_node_hc("GC", Some(false), vec![])],
                )],
            )],
            page: None,
        };
        let mut state = TreeState::new();
        ingest_subtree_level(&mut state, Vec::new(), first, false);
        // Pre-condition: C's children are cached and C is expanded.
        assert_eq!(
            state.cache[&vec!["P".to_string(), "C".to_string()]]
                .children
                .iter()
                .map(|n| n.id.clone())
                .collect::<Vec<_>>(),
            vec!["GC".to_string()],
        );
        assert!(
            state
                .expanded
                .contains(&vec!["P".to_string(), "C".to_string()])
        );

        // Reload after GC was deleted: C is now a genuine leaf.
        let reloaded = Subtree {
            items: vec![sub_node_hc(
                "P",
                Some(true),
                vec![sub_node_hc("C", Some(false), vec![])],
            )],
            page: None,
        };
        ingest_subtree_level(&mut state, Vec::new(), reloaded, false);

        // The stale GC row is gone: C's cache slot is now empty, and C is no
        // longer treated as expanded.
        assert!(
            state.cache[&vec!["P".to_string(), "C".to_string()]]
                .children
                .is_empty(),
            "emptied leaf must have its cached children cleared",
        );
        assert!(
            !state
                .expanded
                .contains(&vec!["P".to_string(), "C".to_string()]),
            "emptied leaf must be dropped from `expanded`",
        );
    }

    /// Counterpart to the above: an empty `children` at the *depth frontier*
    /// (`has_children == Some(true)`) must NOT wipe whatever the node already
    /// has cached — that cache is its lazily-loaded deeper level.
    #[test]
    fn ingest_subtree_keeps_frontier_children() {
        // Frontier node F already has a cached child (lazy-loaded earlier).
        let mut state = TreeState::new();
        state.set_cached_children(vec!["F".to_string()], vec![sub_summary("deep")], None);
        state.expanded.insert(vec!["F".to_string()]);

        // A shallow reload that stops at F (frontier: has_children true but
        // no `children` in the payload) must leave F's cache untouched.
        let shallow = Subtree {
            items: vec![sub_node_hc("F", Some(true), vec![])],
            page: None,
        };
        ingest_subtree_level(&mut state, Vec::new(), shallow, false);

        assert_eq!(
            state.cache[&vec!["F".to_string()]]
                .children
                .iter()
                .map(|n| n.id.clone())
                .collect::<Vec<_>>(),
            vec!["deep".to_string()],
            "frontier node's lazily-loaded children must survive a shallow reload",
        );
        assert!(state.expanded.contains(&vec!["F".to_string()]));
    }

    /// Reload fold preservation (`r` on an eager Tasks/Trackings tree): the
    /// first load (`preserve == false`) force-expands to build the initial
    /// shape; after the user collapses a branch, a reload
    /// (`preserve == true`) must (a) leave that branch collapsed, (b) keep the
    /// still-expanded branches expanded, (c) refresh cached children (external
    /// edits show), and (d) surface externally-added siblings — collapsed,
    /// since they were never in `expanded`.
    #[test]
    fn ingest_subtree_reload_preserves_fold_state() {
        // First load: root → A → {A1(leaf)}, B → {B1(leaf)} — force-expanded.
        let first = Subtree {
            items: vec![
                sub_node("A", vec![sub_node("A1", vec![])]),
                sub_node("B", vec![sub_node("B1", vec![])]),
            ],
            page: None,
        };
        let mut state = TreeState::new();
        ingest_subtree_level(&mut state, Vec::new(), first, false);
        assert!(state.expanded.contains(&vec!["A".to_string()]));
        assert!(state.expanded.contains(&vec!["B".to_string()]));

        // User collapses A.
        state.expanded.remove(&vec!["A".to_string()]);

        // Reload: A still has its child (renamed externally), B unchanged, and
        // a new top-level node C (externally added) with a child C1.
        let reloaded = Subtree {
            items: vec![
                sub_node("A", vec![sub_node("A1", vec![])]),
                sub_node("B", vec![sub_node("B1", vec![])]),
                sub_node("C", vec![sub_node("C1", vec![])]),
            ],
            page: None,
        };
        ingest_subtree_level(&mut state, Vec::new(), reloaded, true);

        // (a) A stays collapsed — the reload did not re-expand it.
        assert!(
            !state.expanded.contains(&vec!["A".to_string()]),
            "reload must not re-expand a branch the user collapsed",
        );
        // (b) B stays expanded.
        assert!(
            state.expanded.contains(&vec!["B".to_string()]),
            "reload must keep an already-expanded branch expanded",
        );
        // (c) A's children are still cached beneath it (fresh), ready for a
        // zero-round-trip re-expand.
        assert_eq!(
            state.cache[&vec!["A".to_string()]]
                .children
                .iter()
                .map(|n| n.id.clone())
                .collect::<Vec<_>>(),
            vec!["A1".to_string()],
        );
        // (d) The externally-added C is present but collapsed (never in
        // `expanded`), even though its child arrived in the eager payload.
        assert_eq!(
            state.cache[&Vec::<String>::new()]
                .children
                .iter()
                .map(|n| n.id.clone())
                .collect::<Vec<_>>(),
            vec!["A".to_string(), "B".to_string(), "C".to_string()],
        );
        assert!(
            !state.expanded.contains(&vec!["C".to_string()]),
            "an externally-added node must appear collapsed on reload",
        );
    }

    /// M9 now-bucket refresh: `reload_now_bucket` swaps one bucket's header
    /// row + re-folds its subtree, and leaves every sibling bucket's cache
    /// untouched — the whole point of the targeted path. A header id absent
    /// from the root level returns `false` (brand-new bucket → caller
    /// full-reloads).
    #[test]
    fn reload_now_bucket_splices_one_bucket_and_leaves_siblings() {
        let config = test_config_with_tree();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        // Root level = two buckets (db1, db2), each with one schema child.
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        let view_defs = view.view_defs.clone();
        {
            let pane = view.active_pane_mut();
            let tree = pane.tree.as_mut().expect("tree mode");
            tree.set_cached_children(
                vec!["db1".into()],
                vec![tnode("s1_old", "s1_old", "mock:schema")],
                None,
            );
            tree.expanded.insert(vec!["db1".into()]);
            tree.set_cached_children(
                vec!["db2".into()],
                vec![tnode("s2", "s2", "mock:schema")],
                None,
            );
            tree.expanded.insert(vec!["db2".into()]);
            tree.rebuild_entries(&view_defs[0]);
        }
        let pane_id = view.active_pane_id();

        // Reload db1: a relabelled header + a fresh single-child subtree.
        let mut new_header = tnode("db1", "db1*", "mock:db");
        new_header.has_children = Some(true);
        let new_subtree = Subtree {
            items: vec![sub_node("s1_new", vec![])],
            page: None,
        };
        assert!(view.reload_now_bucket(pane_id, new_header, new_subtree));

        let tree = view.find_pane(pane_id).unwrap().tree.as_ref().unwrap();
        // db1's header row picked up the new label; db2 stayed put.
        let roots: Vec<(&str, &str)> = tree.cache[&Vec::<String>::new()]
            .children
            .iter()
            .map(|c| (c.id.as_str(), c.label.as_str()))
            .collect();
        assert_eq!(roots, vec![("db1", "db1*"), ("db2", "db2")]);
        // db1's subtree was replaced; db2's was not.
        assert_eq!(
            tree.cache[&vec!["db1".to_string()]]
                .children
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            vec!["s1_new"],
        );
        assert_eq!(
            tree.cache[&vec!["db2".to_string()]]
                .children
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            vec!["s2"],
        );

        // A header id that isn't a visible bucket → no splice, caller falls
        // back to a full reload.
        let orphan = tnode("db_new", "db_new", "mock:db");
        assert!(!view.reload_now_bucket(pane_id, orphan, Subtree::default()));
        let tree = view.find_pane(pane_id).unwrap().tree.as_ref().unwrap();
        assert!(!tree.cache.contains_key(&vec!["db_new".to_string()]));
    }

    /// `patch_row` (the live-tick path) must reach a row wherever it lives in
    /// the tree cache — a deep tree-item *and* its bucket header, neither of
    /// which sits in `pane.items`. A miss leaves every row untouched.
    #[test]
    fn patch_row_swaps_deep_tree_rows_and_bucket_headers() {
        let config = test_config_with_tree();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        // Root level = two buckets (db1, db2); db1 has one (deep) child.
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        let view_defs = view.view_defs.clone();
        {
            let pane = view.active_pane_mut();
            let tree = pane.tree.as_mut().expect("tree mode");
            tree.set_cached_children(
                vec!["db1".into()],
                vec![tnode("deep", "deep old", "mock:schema")],
                None,
            );
            tree.expanded.insert(vec!["db1".into()]);
            tree.rebuild_entries(&view_defs[0]);
        }

        // Patch the deep tree-item (lives in cache[["db1"]], not pane.items).
        assert!(view.patch_row(&tnode("deep", "deep ticked", "mock:schema")));
        // Patch the bucket header (lives in cache[[]], the root level).
        assert!(view.patch_row(&tnode("db1", "db1 ticked", "mock:db")));

        let tree = view.active_pane().tree.as_ref().unwrap();
        assert_eq!(
            tree.cache[&vec!["db1".to_string()]].children[0].label,
            "deep ticked",
        );
        let root_db1 = tree.cache[&Vec::<String>::new()]
            .children
            .iter()
            .find(|c| c.id == "db1")
            .unwrap();
        assert_eq!(root_db1.label, "db1 ticked");

        // An id present in no level patches nothing.
        assert!(!view.patch_row(&tnode("ghost", "ghost", "mock:schema")));
    }

    #[test]
    fn tree_open_on_placeholder_requests_append_next_page() {
        use not_yet_done_content::PageRequest;
        let config = test_config_with_tree();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        let view_defs = view.view_defs.clone();
        let pane_id = view.active_pane_id();
        let view_index = view.view_index;

        // Pre-seed: db1 is expanded, schemas loaded with a pending
        // next_page; rebuild entries → placeholder appears.
        {
            let tree = view.active_pane_mut().tree.as_mut().unwrap();
            tree.set_cached_children(
                vec!["db1".into()],
                mock_schemas(),
                Some(PageRequest {
                    offset: 2,
                    limit: 2,
                }),
            );
            tree.expanded.insert(vec!["db1".into()]);
            tree.rebuild_entries(&view_defs[0]);
        }
        view.active_pane_mut().rebuild_table(&view_defs);

        // Entries: db1 (0), public (1), private (2), <more> (3).
        // Cursor onto the placeholder.
        view.active_pane_mut().table.set_selected(3);

        let msg = view
            .active_pane_mut()
            .try_tree_open(view_index, pane_id, &view_defs)
            .expect("placeholder yields a request");
        match msg {
            SubViewMessage::Request(ViewRequest::ExpandTreeNode {
                parent_path,
                parent_node_id,
                page,
                append,
                ..
            }) => {
                assert_eq!(parent_path, vec!["db1".to_string()]);
                assert_eq!(parent_node_id, "db1");
                assert!(append, "placeholder activation must append");
                let p = page.expect("page request carried through");
                assert_eq!(p.offset, 2);
                assert_eq!(p.limit, 2);
            }
            other => panic!("unexpected message {other:?}"),
        }
    }

    #[test]
    fn tree_smart_collapse_on_expanded_collapses_self_keeps_cursor() {
        let config = test_config_with_tree();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        let view_defs = view.view_defs.clone();

        // Expand db1 so the cursor sits on an *expanded* node at depth 0.
        {
            let pane = view.active_pane_mut();
            let tree = pane.tree.as_mut().expect("tree mode");
            tree.set_cached_children(vec!["db1".into()], mock_schemas(), None);
            tree.expanded.insert(vec!["db1".into()]);
            tree.rebuild_entries(&view_defs[0]);
        }
        view.active_pane_mut().rebuild_table(&view_defs);
        view.active_pane_mut().table.set_selected(0);

        match view.active_pane_mut().try_tree_smart_collapse(&view_defs) {
            Some(SubViewMessage::SelectionChanged(_)) => {}
            other => panic!("expected SelectionChanged, got {other:?}"),
        }
        let pane = view.active_pane();
        let tree = pane.tree.as_ref().unwrap();
        assert!(
            !tree.expanded.contains(&vec!["db1".to_string()]),
            "db1 stayed expanded after smart-collapse on self",
        );
        // Cursor stays put on db1 (still row 0); entries shrunk back to roots.
        assert_eq!(tree.entries.len(), 2);
        assert_eq!(pane.table.selected_row(), 0);
        assert_eq!(tree.entries[0].node.id, "db1");
    }

    #[test]
    fn tree_smart_collapse_on_unexpanded_child_collapses_parent() {
        let config = test_config_with_tree();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        let view_defs = view.view_defs.clone();

        // db1 expanded, schemas loaded; cursor on "public" (depth 1, unexpanded).
        {
            let pane = view.active_pane_mut();
            let tree = pane.tree.as_mut().expect("tree mode");
            tree.set_cached_children(vec!["db1".into()], mock_schemas(), None);
            tree.expanded.insert(vec!["db1".into()]);
            tree.rebuild_entries(&view_defs[0]);
        }
        view.active_pane_mut().rebuild_table(&view_defs);
        view.active_pane_mut().table.set_selected(1);

        match view.active_pane_mut().try_tree_smart_collapse(&view_defs) {
            Some(SubViewMessage::SelectionChanged(_)) => {}
            other => panic!("expected SelectionChanged, got {other:?}"),
        }
        let pane = view.active_pane();
        let tree = pane.tree.as_ref().unwrap();
        assert!(
            !tree.expanded.contains(&vec!["db1".to_string()]),
            "parent db1 should have collapsed",
        );
        assert_eq!(pane.table.selected_row(), 0, "cursor jumped up to db1");
    }

    #[test]
    fn tree_collapse_all_drops_every_expanded_path() {
        let config = test_config_with_tree();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        let view_defs = view.view_defs.clone();

        // Expand both top-level rows so we have something to collapse.
        {
            let pane = view.active_pane_mut();
            let tree = pane.tree.as_mut().expect("tree mode");
            tree.set_cached_children(vec!["db1".into()], mock_schemas(), None);
            tree.set_cached_children(vec!["db2".into()], mock_schemas(), None);
            tree.expanded.insert(vec!["db1".into()]);
            tree.expanded.insert(vec!["db2".into()]);
            tree.rebuild_entries(&view_defs[0]);
        }
        view.active_pane_mut().rebuild_table(&view_defs);
        view.active_pane_mut().table.set_selected(2);

        match view.active_pane_mut().try_tree_collapse_all(&view_defs) {
            Some(SubViewMessage::SelectionChanged(_)) => {}
            other => panic!("expected SelectionChanged, got {other:?}"),
        }
        let pane = view.active_pane();
        let tree = pane.tree.as_ref().unwrap();
        assert!(tree.expanded.is_empty(), "all expanded paths cleared");
        assert!(
            tree.cache.contains_key(&vec!["db1".to_string()]),
            "cached children kept (no refetch needed on next expand)",
        );
        assert_eq!(pane.table.selected_row(), 0, "cursor reset to first row");
    }

    #[test]
    fn tree_smart_collapse_at_depth_zero_unexpanded_is_unhandled() {
        let config = test_config_with_tree();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        let view_defs = view.view_defs.clone();
        view.active_pane_mut().table.set_selected(0);
        assert!(
            view.active_pane_mut()
                .try_tree_smart_collapse(&view_defs)
                .is_none()
        );
    }

    #[test]
    fn tree_back_collapses_parent_and_moves_cursor() {
        let config = test_config_with_tree();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        let view_defs = view.view_defs.clone();

        // Pre-populate db1's children + mark expanded, then rebuild entries
        // so the cursor can land on a depth-1 row.
        {
            let pane = view.active_pane_mut();
            let tree = pane.tree.as_mut().expect("tree mode enabled");
            tree.set_cached_children(vec!["db1".into()], mock_schemas(), None);
            tree.expanded.insert(vec!["db1".into()]);
            let vd = &view_defs[0];
            tree.rebuild_entries(vd);
        }
        view.active_pane_mut().rebuild_table(&view_defs);
        // Entries: [db1, public, private, db2]; cursor on "public" (depth 1).
        view.active_pane_mut().table.set_selected(1);

        match view.active_pane_mut().try_back(&view_defs) {
            SubViewMessage::SelectionChanged(_) => {}
            other => panic!("expected SelectionChanged, got {other:?}"),
        }
        let pane = view.active_pane();
        let tree = pane.tree.as_ref().unwrap();
        assert!(
            !tree.expanded.contains(&vec!["db1".to_string()]),
            "db1 stayed expanded"
        );
        assert_eq!(tree.entries.len(), 2, "only db1 + db2 remain");
        assert_eq!(tree.entries[0].node.id, "db1");
        assert_eq!(tree.entries[1].node.id, "db2");
        assert_eq!(pane.table.selected_row(), 0, "cursor moved to db1");
    }

    /// Regression: applying a saved query while the cursor sits on a row
    /// deeper than the (much smaller) filtered tree used to abort the
    /// table rebuild — `current_columns` resolved the column set via the
    /// now out-of-range cursor row, got nothing, and `rebuild_table`
    /// early-returned before `set_data`, leaving the widget painting the
    /// stale pre-query rows.
    #[test]
    fn tree_query_apply_shrink_clamps_cursor_and_keeps_columns() {
        let config = test_config_with_tree();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        let view_defs = view.view_defs.clone();
        {
            let pane = view.active_pane_mut();
            let tree = pane.tree.as_mut().unwrap();
            tree.set_cached_children(vec!["db1".into()], mock_schemas(), None);
            tree.expanded.insert(vec!["db1".into()]);
            tree.rebuild_entries(&view_defs[0]);
        }
        view.active_pane_mut().rebuild_table(&view_defs);
        // Entries: [db1, public, private, db2]; cursor on the last row.
        view.active_pane_mut().table.set_selected(3);

        // The query-apply path: set_query clears the tree, set_items
        // feeds the (single) filtered root back in.
        view.active_pane_mut()
            .set_query("filtered".into(), Some("sq".into()));
        let filtered_root = vec![mock_dbs().remove(0)];
        view.set_items(filtered_root, Vec::new(), None, Vec::new(), None);

        let pane = view.active_pane();
        assert_eq!(
            pane.tree_visible_indices.len(),
            1,
            "one filtered root entry"
        );
        assert_eq!(
            pane.table.selected_row(),
            0,
            "stale cursor clamped onto the shrunk tree"
        );
        assert!(
            !pane.current_columns(&view.view_defs).is_empty(),
            "columns resolve (root-level fallback) so rebuild_table reaches set_data"
        );
    }

    /// Tree config like `test_config_with_tree`, plus a `fuzzy_filter`
    /// action mounted at the requested depth (`0` = ViewDef root, `1` =
    /// the schemas ChildDef). Used by Phase 6 filter tests so each can
    /// place the filter at a single level and assert depth-scoped
    /// behavior.
    fn test_config_with_tree_filter_at(depth: usize) -> ViewFileConfig {
        let filter_action = crate::config::view_config::ActionDef {
            name: "filter".into(),
            key: Some("f".into()),
            action_type: "fuzzy_filter".into(),
            id: None,
            node_id_from: None,
            navigate_to: None,
            fuzzy_filter: Some(crate::config::view_config::FuzzyFilterConfig {
                fields: Vec::new(),
            }),
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
        let mut config = test_config_with_tree();
        match depth {
            0 => config.views[0].actions.push(filter_action),
            1 => config.views[0].children[0].actions.push(filter_action),
            _ => panic!("test helper only places filter at depth 0 or 1"),
        }
        config
    }

    #[test]
    fn tree_resolve_filter_depth_finds_root_action() {
        let config = test_config_with_tree_filter_at(0);
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let pane = view.active_pane();
        assert_eq!(pane.resolve_tree_filter_depth(&view.view_defs), Some(0));
    }

    #[test]
    fn tree_resolve_filter_depth_finds_child_action() {
        let config = test_config_with_tree_filter_at(1);
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let pane = view.active_pane();
        assert_eq!(pane.resolve_tree_filter_depth(&view.view_defs), Some(1));
    }

    #[test]
    fn tree_resolve_filter_depth_none_when_unset() {
        let config = test_config_with_tree();
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let pane = view.active_pane();
        assert_eq!(pane.resolve_tree_filter_depth(&view.view_defs), None);
    }

    #[test]
    fn tree_visible_indices_all_when_filter_empty() {
        let config = test_config_with_tree_filter_at(0);
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        let pane = view.active_pane();
        assert_eq!(pane.tree_visible_indices, vec![0, 1]);
    }

    #[test]
    fn tree_filter_surfaces_deep_match_above_armed_depth() {
        // Regression: filter armed at depth 0 (the root level carries the
        // `fuzzy_filter` action), but the match — "public" — sits at depth 1.
        // The old single-depth filter only tested depth-0 rows, so a non-
        // matching parent (db1) hid its whole subtree and the subtask match
        // was unreachable. Path-pruning must surface db1 because a descendant
        // matches.
        let config = test_config_with_tree_filter_at(0);
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        let view_defs = view.view_defs.clone();
        {
            let pane = view.active_pane_mut();
            let tree = pane.tree.as_mut().unwrap();
            tree.set_cached_children(vec!["db1".into()], mock_schemas(), None);
            tree.expanded.insert(vec!["db1".into()]);
            tree.rebuild_entries(&view_defs[0]);
        }
        view.active_pane_mut().tree_filter_depth = Some(0);
        view.active_pane_mut().table.filter_text = "public".into();
        view.active_pane_mut().rebuild_table(&view_defs);

        let pane = view.active_pane();
        let tree = pane.tree.as_ref().unwrap();
        let visible_ids: Vec<&str> = pane
            .tree_visible_indices
            .iter()
            .map(|&i| tree.entries[i].node.id.as_str())
            .collect();
        // db1 kept as ancestor of the match; public matches; private and db2
        // pruned.
        assert_eq!(visible_ids, vec!["db1", "public"]);
    }

    #[test]
    fn restore_tree_filter_expand_recollapses_to_stashed_shape() {
        // Opening a fuzzy filter on an eager tree blows the whole subtree open
        // and stashes the pre-filter `expanded` set; clearing the filter must
        // restore exactly that set, re-collapsing the branches the filter
        // expanded. Here the stash is the collapsed (empty) pre-filter state.
        let config = test_config_with_tree_filter_at(0);
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        let view_defs = view.view_defs.clone();
        {
            let pane = view.active_pane_mut();
            let tree = pane.tree.as_mut().unwrap();
            tree.set_cached_children(vec!["db1".into()], mock_schemas(), None);
            // Pre-filter shape: db1 collapsed (nothing expanded).
            let stash: std::collections::HashSet<Vec<String>> = tree.expanded.clone();
            // Filter expands db1 to surface its schemas.
            tree.expanded.insert(vec!["db1".into()]);
            tree.rebuild_entries(&view_defs[0]);
            pane.tree_filter_expand_stash = Some(stash);
        }
        // Expanded while the filter is "open": db1 + its two schemas + db2.
        assert_eq!(view.active_pane().tree.as_ref().unwrap().entries.len(), 4);

        view.active_pane_mut()
            .restore_tree_filter_expand(&view_defs);

        let pane = view.active_pane();
        assert!(pane.tree_filter_expand_stash.is_none());
        let tree = pane.tree.as_ref().unwrap();
        assert!(tree.expanded.is_empty());
        // Re-collapsed back to the two root rows.
        assert_eq!(tree.entries.len(), 2);
    }

    #[test]
    fn tree_filter_hides_non_matching_at_filter_depth() {
        // Filter at depth 1 (schemas). db1 expanded with [public, private];
        // db2 collapsed. Filter "pub" matches "public" → keep public and its
        // ancestor db1; "private" is pruned, and db2 (no match, no matching
        // descendant) is pruned too — path-pruning hides irrelevant siblings.
        let config = test_config_with_tree_filter_at(1);
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        let view_defs = view.view_defs.clone();
        {
            let pane = view.active_pane_mut();
            let tree = pane.tree.as_mut().unwrap();
            tree.set_cached_children(vec!["db1".into()], mock_schemas(), None);
            tree.expanded.insert(vec!["db1".into()]);
            tree.rebuild_entries(&view_defs[0]);
        }
        view.active_pane_mut().tree_filter_depth = Some(1);
        view.active_pane_mut().table.filter_text = "pub".into();
        view.active_pane_mut().rebuild_table(&view_defs);

        let pane = view.active_pane();
        let tree = pane.tree.as_ref().unwrap();
        // Full entries: [db1, public, private, db2]; visible:
        // [db1 (kept as ancestor of a match), public (matches)]. private and
        // db2 are pruned.
        let visible_ids: Vec<&str> = pane
            .tree_visible_indices
            .iter()
            .map(|&i| tree.entries[i].node.id.as_str())
            .collect();
        assert_eq!(visible_ids, vec!["db1", "public"]);
    }

    #[test]
    fn tree_filter_hides_subtree_of_non_matching_parent() {
        // Filter at depth 0. db1 expanded — when db1 is filtered out,
        // its expanded children must also disappear.
        let config = test_config_with_tree_filter_at(0);
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        let view_defs = view.view_defs.clone();
        {
            let pane = view.active_pane_mut();
            let tree = pane.tree.as_mut().unwrap();
            tree.set_cached_children(vec!["db1".into()], mock_schemas(), None);
            tree.expanded.insert(vec!["db1".into()]);
            tree.rebuild_entries(&view_defs[0]);
        }
        view.active_pane_mut().tree_filter_depth = Some(0);
        view.active_pane_mut().table.filter_text = "db2".into();
        view.active_pane_mut().rebuild_table(&view_defs);

        let pane = view.active_pane();
        let tree = pane.tree.as_ref().unwrap();
        let visible_ids: Vec<&str> = pane
            .tree_visible_indices
            .iter()
            .map(|&i| tree.entries[i].node.id.as_str())
            .collect();
        // db1 hidden → public/private vanish with it; only db2 stays.
        assert_eq!(visible_ids, vec!["db2"]);
    }

    #[test]
    fn tree_filter_keeps_pagination_placeholder_visible() {
        // Placeholder rows must stay visible even when the filter is
        // active — they belong to the parent and are the user's only
        // path to the next page.
        use not_yet_done_content::PageRequest;
        let config = test_config_with_tree_filter_at(1);
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        let view_defs = view.view_defs.clone();
        {
            let pane = view.active_pane_mut();
            let tree = pane.tree.as_mut().unwrap();
            tree.set_cached_children(
                vec!["db1".into()],
                mock_schemas(),
                Some(PageRequest {
                    offset: 2,
                    limit: 2,
                }),
            );
            tree.expanded.insert(vec!["db1".into()]);
            tree.rebuild_entries(&view_defs[0]);
        }
        view.active_pane_mut().tree_filter_depth = Some(1);
        view.active_pane_mut().table.filter_text = "pub".into();
        view.active_pane_mut().rebuild_table(&view_defs);

        let pane = view.active_pane();
        let tree = pane.tree.as_ref().unwrap();
        // Full: [db1, public, private, <more>, db2]; with `pub` filter:
        // public matches → db1 kept; private pruned; the placeholder rides
        // along because its parent db1 survives; db2 (no match) is pruned.
        let visible: Vec<(&str, bool)> = pane
            .tree_visible_indices
            .iter()
            .map(|&i| {
                (
                    tree.entries[i].node.id.as_str(),
                    tree.entries[i].is_more_placeholder,
                )
            })
            .collect();
        assert_eq!(visible.len(), 3);
        assert_eq!(visible[0].0, "db1");
        assert_eq!(visible[1].0, "public");
        assert!(visible[2].1, "placeholder kept in the schemas group");
    }

    #[test]
    fn tree_search_descriptions_iterates_visible_entries() {
        // `/`-search must see exactly the rows the user can see, after
        // fuzzy filter has narrowed them — and must skip pagination
        // placeholders so cursor never lands on the loader row.
        use not_yet_done_content::PageRequest;
        let config = test_config_with_tree_filter_at(1);
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        let view_defs = view.view_defs.clone();
        {
            let pane = view.active_pane_mut();
            let tree = pane.tree.as_mut().unwrap();
            tree.set_cached_children(
                vec!["db1".into()],
                mock_schemas(),
                Some(PageRequest {
                    offset: 2,
                    limit: 2,
                }),
            );
            tree.expanded.insert(vec!["db1".into()]);
            tree.rebuild_entries(&view_defs[0]);
        }
        view.active_pane_mut().tree_filter_depth = Some(1);
        view.active_pane_mut().table.filter_text = "pub".into();
        view.active_pane_mut().rebuild_table(&view_defs);

        let pane = view.active_pane();
        let descs = pane.search_descriptions(&view_defs);
        // Visible rows are db1 (row 0), public (row 1), <more> (row 2);
        // db2 is pruned (no match). search_descriptions drops the
        // placeholder, so /-search sees two rows — and the surviving row
        // indices map back into the visible-row list, not raw tree.entries.
        assert_eq!(descs.len(), 2);
        assert_eq!(descs[0].0, 0);
        assert!(descs[0].1.contains("db1"));
        assert_eq!(descs[1].0, 1);
        assert!(descs[1].1.contains("public"));
    }

    #[test]
    fn tree_apply_children_respects_active_filter_at_load_time() {
        // Fuzzy filter at depth 1 with text "pub" is active before db1's
        // schemas finish loading. When apply_tree_children lands them,
        // the depth-1 filter must apply to the freshly cached rows too
        // (not just to pre-loaded ones).
        let config = test_config_with_tree_filter_at(1);
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        let view_defs = view.view_defs.clone();
        let pane_id = view.active_pane_id();

        // Filter armed; db1 is marked expanded but has no cached children
        // yet — simulates the gap between Enter on db1 and the async load
        // returning.
        view.active_pane_mut().tree_filter_depth = Some(1);
        view.active_pane_mut().table.filter_text = "pub".into();
        {
            let tree = view.active_pane_mut().tree.as_mut().unwrap();
            tree.expanded.insert(vec!["db1".into()]);
            tree.rebuild_entries(&view_defs[0]);
        }
        view.active_pane_mut().rebuild_table(&view_defs);

        // Now the async load lands. apply_tree_children re-flattens
        // entries and rebuilds the table — refresh_tree_visible_indices
        // runs against the new rows.
        view.apply_tree_children(
            pane_id,
            vec!["db1".into()],
            mock_schemas(),
            None,
            false,
            "mock:schema".into(),
        );

        let pane = view.active_pane();
        let tree = pane.tree.as_ref().unwrap();
        let visible_ids: Vec<&str> = pane
            .tree_visible_indices
            .iter()
            .map(|&i| tree.entries[i].node.id.as_str())
            .collect();
        assert_eq!(
            visible_ids,
            vec!["db1", "public"],
            "freshly loaded schemas must be filtered: private is hidden, \
             and db2 (no match) is pruned with path-pruning semantics"
        );
    }

    #[test]
    fn tree_split_drill_into_leaf_creates_flat_pane() {
        // Cursor on a tree row whose level only has a non-tree child (a
        // leaf with `split: right`) — Enter falls through to
        // `dispatch_content_drill`. The new split pane represents the
        // leaf level and MUST be flat; carrying tree state into it would
        // render the leaf rows through the tree-pane code paths.
        let mut config = test_config_with_tree();
        // test_config_with_tree gives databases(tree) → Schemas(tree).
        // Add a "Rows" leaf as Schemas' only child, with split: right.
        config.views[0].children[0].children.push(ChildDef {
            card: None,
            row_layout: None,
            smooth_scroll: false,
            name: "Rows".into(),
            node_type: "mock:row".into(),
            columns: vec![ColumnDef {
                key: "id".into(),
                label: Some("Id".into()),
                source: Some("label".into()),
                collapsed_source: None,
                long_source: None,
                style: None,
                sizing: "max".into(),
                markdown: false,
                kind: ColumnKind::Text,
                format: None,
                separator: None,
                elapsed_from: None,
                tree_aggregate: None,
                hidden: false,
            }],
            preview: None,
            actions: Vec::new(),
            children: Vec::new(),
            split: Some(SplitDef {
                direction: SplitDirection::Right,
                ratio: 0.5,
                coupled: false,
            }),
            pagination: None,
            keybindings: HashMap::new(),
            action_chains: Default::default(),
            column_cursor: false,
            record_detail: false,
            node_scripts: false,
            tree_label: None,
            shortcuts: HashMap::new(),
            enter_action: None,
            recursive: false,
            editor_in_place: false,
            leaf_glyph: None,
            icon: None,
            group_by: None,
            aggregates: Vec::new(),
            mark_read_on_reach_end: None,
            cursor_on_open: None,
        });

        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        let view_defs = view.view_defs.clone();
        let source_pane_id = view.active_pane_id();
        let view_index = view.view_index;

        // Expand db1, cache schemas, cursor onto a schema row at depth 1.
        {
            let tree = view.active_pane_mut().tree.as_mut().unwrap();
            tree.set_cached_children(vec!["db1".into()], mock_schemas(), None);
            tree.expanded.insert(vec!["db1".into()]);
            tree.rebuild_entries(&view_defs[0]);
        }
        view.active_pane_mut().rebuild_table(&view_defs);
        view.active_pane_mut().table.set_selected(1);

        // Branch 2 of try_tree_open: no tree-continuing child at depth 1,
        // so it returns ContentDrill for the Rows leaf.
        let msg = view
            .active_pane_mut()
            .try_tree_open(view_index, source_pane_id, &view_defs)
            .expect("schema row yields a drill message");
        let (item_id, item_label, child_def) = match msg {
            SubViewMessage::ContentDrill {
                item_id,
                item_label,
                child_def,
            } => (item_id, item_label, *child_def),
            other => panic!("expected ContentDrill, got {other:?}"),
        };
        assert!(child_def.tree_label.is_none(), "leaf has no tree_label");
        assert!(child_def.split.is_some(), "leaf uses split");

        // Dispatch the drill → split pane spawned for the leaf.
        let result = view.dispatch_content_drill(item_id, item_label, child_def);
        let new_pane_id = match result {
            SubViewMessage::Request(ViewRequest::DrillDown { pane_id, .. }) => pane_id,
            other => panic!("expected DrillDown request, got {other:?}"),
        };
        assert_ne!(new_pane_id, source_pane_id);

        let new_pane = view.find_pane(new_pane_id).expect("split pane exists");
        assert!(
            new_pane.tree.is_none(),
            "split-drill into a leaf must yield a flat pane, not tree-mode"
        );
        let source = view.find_pane(source_pane_id).expect("source alive");
        assert!(source.tree.is_some(), "source pane keeps its tree state");
    }

    #[test]
    fn tree_in_place_drill_into_leaf_disables_tree_and_nav_back_restores() {
        // In-place drill (leaf without `split`) must terminate the tree
        // chain on the same pane: `self.tree` goes to None for the
        // duration, and `nav_back` resurrects it (with the same expanded
        // set and entries) when leaving the leaf.
        let mut config = test_config_with_tree();
        config.views[0].children[0].children.push(ChildDef {
            card: None,
            row_layout: None,
            smooth_scroll: false,
            name: "Rows".into(),
            node_type: "mock:row".into(),
            columns: vec![ColumnDef {
                key: "id".into(),
                label: Some("Id".into()),
                source: Some("label".into()),
                collapsed_source: None,
                long_source: None,
                style: None,
                sizing: "max".into(),
                markdown: false,
                kind: ColumnKind::Text,
                format: None,
                separator: None,
                elapsed_from: None,
                tree_aggregate: None,
                hidden: false,
            }],
            preview: None,
            actions: Vec::new(),
            children: Vec::new(),
            split: None,
            pagination: None,
            keybindings: HashMap::new(),
            action_chains: Default::default(),
            column_cursor: false,
            record_detail: false,
            node_scripts: false,
            tree_label: None,
            shortcuts: HashMap::new(),
            enter_action: None,
            recursive: false,
            editor_in_place: false,
            leaf_glyph: None,
            icon: None,
            group_by: None,
            aggregates: Vec::new(),
            mark_read_on_reach_end: None,
            cursor_on_open: None,
        });

        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        let view_defs = view.view_defs.clone();
        let pane_id = view.active_pane_id();
        let view_index = view.view_index;

        // Expand db1, cache schemas, cursor on the public row (depth 1).
        {
            let tree = view.active_pane_mut().tree.as_mut().unwrap();
            tree.set_cached_children(vec!["db1".into()], mock_schemas(), None);
            tree.expanded.insert(vec!["db1".into()]);
            tree.rebuild_entries(&view_defs[0]);
        }
        view.active_pane_mut().rebuild_table(&view_defs);
        view.active_pane_mut().table.set_selected(1);

        let msg = view
            .active_pane_mut()
            .try_tree_open(view_index, pane_id, &view_defs)
            .expect("depth-1 row yields drill");
        let (item_id, item_label, child_def) = match msg {
            SubViewMessage::ContentDrill {
                item_id,
                item_label,
                child_def,
            } => (item_id, item_label, *child_def),
            other => panic!("expected ContentDrill, got {other:?}"),
        };
        assert!(child_def.split.is_none(), "leaf is in-place");

        // In-place drill mutates the SAME pane.
        let result = view.dispatch_content_drill(item_id, item_label, child_def);
        match result {
            SubViewMessage::Request(ViewRequest::DrillDown { pane_id: p, .. }) => {
                assert_eq!(p, pane_id, "in-place drill keeps the source pane id");
            }
            other => panic!("expected DrillDown, got {other:?}"),
        }
        let pane = view.active_pane();
        assert!(
            pane.tree.is_none(),
            "in-place drill into a leaf must turn off tree mode"
        );
        assert!(
            pane.active_child
                .as_ref()
                .map(|c| c.name == "Rows")
                .unwrap_or(false),
            "active_child set to leaf"
        );

        // try_back: tree is None → falls to nav_back, which restores tree.
        let _ = view.active_pane_mut().try_back(&view_defs);
        let pane = view.active_pane();
        assert!(pane.tree.is_some(), "nav_back resurrects the tree state");
        let tree = pane.tree.as_ref().unwrap();
        assert!(
            tree.expanded.contains(&vec!["db1".to_string()]),
            "expanded set survives drill+back"
        );
        assert!(
            pane.active_child.is_none(),
            "active_child cleared back to tree-root scope"
        );
    }

    // ── Phase 7: per-level keymap ────────────────────────────────────

    /// Tree config with an `edit` action on the root view and an `inspect`
    /// action on the schemas ChildDef. Some tests also add a `fuzzy_filter`
    /// at root to verify global-action discovery.
    fn test_config_with_tree_per_level_actions() -> ViewFileConfig {
        let mut config = test_config_with_tree();
        // Root view: edit + global fuzzy_filter.
        config.views[0].actions.push(ActionDef {
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
        });
        config.views[0].actions.push(ActionDef {
            name: "filter".into(),
            key: Some("f".into()),
            action_type: "fuzzy_filter".into(),
            id: None,
            node_id_from: None,
            navigate_to: None,
            fuzzy_filter: Some(crate::config::view_config::FuzzyFilterConfig {
                fields: Vec::new(),
            }),
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
        });
        // Root view: tree_find (`/`). Like fuzzy_filter it is declared only
        // here yet must reach every cursor depth (it is in the GLOBAL set).
        config.views[0].actions.push(ActionDef {
            name: "treefind".into(),
            key: Some("/".into()),
            action_type: "tree_find".into(),
            id: None,
            node_id_from: None,
            navigate_to: None,
            fuzzy_filter: None,
            search: None,
            text_search: None,
            tree_find: Some(crate::config::view_config::TreeFindActionConfig { prompt: None }),
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
        });
        // Schemas child: inspect (level-only) action.
        config.views[0].children[0].actions.push(ActionDef {
            name: "inspect".into(),
            key: Some("i".into()),
            action_type: "custom".into(),
            id: Some("inspect_schema".into()),
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
        });
        config
    }

    /// Expand `db1` so the entry list is `[db1, public, private, db2]` and
    /// land the cursor on the requested row. Used by the depth-aware tests.
    fn expand_db1_and_select(view: &mut ContentView, row: usize) {
        let view_defs = view.view_defs.clone();
        {
            let pane = view.active_pane_mut();
            let tree = pane.tree.as_mut().expect("tree mode enabled");
            tree.set_cached_children(vec!["db1".into()], mock_schemas(), None);
            tree.expanded.insert(vec!["db1".into()]);
            tree.rebuild_entries(&view_defs[0]);
        }
        view.active_pane_mut().rebuild_table(&view_defs);
        view.active_pane_mut().table.set_selected(row);
    }

    #[test]
    fn tree_current_actions_at_root_depth_returns_view_actions() {
        let config = test_config_with_tree_per_level_actions();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        // Cursor on db1 (depth 0).
        let actions = view.active_pane().current_actions(&view.view_defs);
        let names: Vec<&str> = actions.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["edit", "filter", "treefind"]);
    }

    #[test]
    fn tree_current_actions_on_empty_tree_falls_back_to_root_actions() {
        // Regression: a `tree_label` view is in tree mode from
        // construction (TreeState::new), so before any successful load
        // the tree is empty and has no cursor row. `tree_current_actions`
        // must still surface the root-level actions (notably `reload`) so
        // the user can retry a failed initial load — otherwise pressing
        // the refresh key is silently Unhandled.
        let config = test_config_with_tree_per_level_actions();
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        // No set_items: tree is Some but empty, cursor resolves to nothing.
        let actions = view.active_pane().current_actions(&view.view_defs);
        let names: Vec<&str> = actions.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["edit", "filter", "treefind"],
            "empty tree must still expose the root level's actions"
        );
    }

    #[test]
    fn tree_current_actions_at_child_depth_returns_level_plus_globals() {
        let config = test_config_with_tree_per_level_actions();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        expand_db1_and_select(&mut view, 1); // cursor on "public" (depth 1)

        let actions = view.active_pane().current_actions(&view.view_defs);
        let names: Vec<&str> = actions.iter().map(|a| a.name.as_str()).collect();
        // depth-1 own action first, then the root globals (fuzzy_filter +
        // tree_find) appended. root `edit` is not global → stays hidden at
        // depth 1. The `treefind` entry is the regression guard: before the
        // fix `/` dispatched only on a top-level node.
        assert_eq!(names, vec!["inspect", "filter", "treefind"]);
    }

    #[test]
    fn tree_current_actions_prefers_active_level_on_key_collision() {
        let mut config = test_config_with_tree();
        // Both levels define an action under key "x"; only the active
        // level's should win.
        config.views[0].actions.push(ActionDef {
            name: "root_x".into(),
            key: Some("x".into()),
            action_type: "custom".into(),
            id: Some("root_x".into()),
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
        });
        // root_x is NOT a global type, so without the dedup rule it
        // wouldn't show up at depth 1 anyway. Use search (global) to
        // force the collision path: same key "x" exists at child.
        config.views[0].actions.push(ActionDef {
            name: "root_search".into(),
            key: Some("x".into()),
            action_type: "search".into(),
            id: None,
            node_id_from: None,
            navigate_to: None,
            fuzzy_filter: None,
            search: Some(crate::config::view_config::SearchConfig {
                fields: Vec::new(),
                next_key: None,
                prev_key: None,
            }),
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
        });
        config.views[0].children[0].actions.push(ActionDef {
            name: "child_x".into(),
            key: Some("x".into()),
            action_type: "custom".into(),
            id: Some("child_x".into()),
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
        });
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        expand_db1_and_select(&mut view, 1);

        let actions = view.active_pane().current_actions(&view.view_defs);
        let names: Vec<&str> = actions.iter().map(|a| a.name.as_str()).collect();
        // Active level wins for key "x"; root's global search is skipped.
        assert_eq!(names, vec!["child_x"]);
    }

    #[test]
    fn tree_current_children_switches_with_depth() {
        let config = test_config_with_tree();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        // Depth 0: view has one tree-continuing child (schemas).
        assert_eq!(
            view.active_pane().current_children(&view.view_defs).len(),
            1
        );
        expand_db1_and_select(&mut view, 1);
        // Depth 1: schemas has no children at all.
        assert_eq!(
            view.active_pane().current_children(&view.view_defs).len(),
            0
        );
    }

    #[test]
    fn tree_level_binding_uses_child_overrides() {
        let mut config = test_config_with_tree();
        // Override Back on the schemas level: at depth 1 the user must
        // press X instead of the global back keybinding.
        config.views[0].children[0]
            .keybindings
            .insert(ContentAction::Back, Some(KeyBinding::new("X")));
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);

        let content_kb = KeyBindingConfig::default().content;
        let view_defs = view.view_defs.clone();
        let pane = view.active_pane();
        // Depth 0: falls through to global content_kb (no per-level
        // override on the ViewDef — that field doesn't exist there).
        let root = pane.level_binding(&ContentAction::Back, &content_kb, &view_defs);
        assert!(root.is_some(), "root depth falls through to global");
        assert!(!root.unwrap().matches("X"));

        expand_db1_and_select(&mut view, 1);
        let content_kb = KeyBindingConfig::default().content;
        let view_defs = view.view_defs.clone();
        let pane = view.active_pane();
        let depth1 = pane
            .level_binding(&ContentAction::Back, &content_kb, &view_defs)
            .expect("depth-1 override present");
        assert!(depth1.matches("X"));
    }

    #[test]
    fn tree_action_chain_scopes_pushes_active_child_first() {
        let mut config = test_config_with_tree();
        config.views[0].children[0]
            .action_chains
            .0
            .insert("ctrl+x".into(), None);
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);

        // Depth 0: no active child → only the view's chains scope.
        let scopes = view.action_chain_scopes();
        assert_eq!(scopes.len(), 1);
        assert!(scopes[0].lookup("ctrl+x").is_none());

        expand_db1_and_select(&mut view, 1);
        let scopes = view.action_chain_scopes();
        assert_eq!(scopes.len(), 2, "child + view in tree mode at depth 1");
        // Innermost first: the depth-1 ChildDef's `ctrl+x` override.
        assert!(scopes[0].lookup("ctrl+x").is_some());
    }

    /// A saved (not extended) query as the reload hands them to the view.
    fn merged_query(name: &str, query: &str, shortcut: Option<&str>) -> MergedSavedQuery {
        MergedSavedQuery {
            name: name.into(),
            query: query.into(),
            shortcut: shortcut.map(str::to_string),
            kind: QueryKind::Saved,
        }
    }

    fn test_config_with_query() -> ViewFileConfig {
        ViewFileConfig {
            reminder: None,
            tab: TabConfig {
                name: "Test".into(),
                order: 0,
                icon: None,
                key: None,
                unread_marker: None,
                unread_style: None,
                load_banner: None,
            },
            adapter: AdapterConfig {
                adapter_type: "mock".into(),
                id: None,
                config: None,
                config_inline: None,
                manual_connect: false,
            },
            views: vec![ViewDef {
                card: None,
                row_layout: None,
                smooth_scroll: false,
                name: "issues".into(),
                node_type: "mock:issue".into(),
                default: true,
                window_ops: false,
                key: None,
                query: Some(QueryConfig {
                    default: Some("assignee = me".into()),
                    template: None,
                    editable: true,
                    menu_key: Some("q".into()),
                    inherit_default: false,
                }),
                columns: vec![ColumnDef {
                    key: "key".into(),
                    label: Some("Key".into()),
                    source: None,
                    style: None,
                    sizing: "max".into(),
                    markdown: false,
                    kind: ColumnKind::Text,
                    format: None,
                    separator: None,
                    elapsed_from: None,
                    tree_aggregate: None,
                    hidden: false,
                    collapsed_source: None,
                    long_source: None,
                }],
                preview: None,
                actions: vec![],
                children: vec![],
                pagination: None,
                action_chains: Default::default(),
                column_cursor: false,
                record_detail: false,
                node_scripts: false,
                tree_label: None,
                retries: 0,
                script_template: None,
                script_source: None,
                shortcuts: HashMap::new(),
                leaf_glyph: None,
                icon: None,
                group_by: None,
                aggregates: Vec::new(),
                tree_connector_style: None,
                unread_style: None,
                unread_marker: None,
                tree_lines: None,
                tree_markers: None,
                expand_depth: None,
                group_headers: None,
                event_actions: Vec::new(),
            }],
        }
    }

    #[test]
    fn root_load_request_includes_sort_and_page_state() {
        use not_yet_done_content::SortDirection;
        let config = test_config_with_query();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let changed = view.set_current_sort(vec![SortKey {
            column: "modified".into(),
            direction: SortDirection::Desc,
        }]);
        assert!(changed);
        let changed = view.set_current_page(Some(PageRequest {
            offset: 50,
            limit: 50,
        }));
        assert!(changed);
        let req = view.root_load_request().unwrap();
        assert_eq!(req.sort.len(), 1);
        assert_eq!(req.sort[0].column, "modified");
        assert_eq!(
            req.page,
            Some(PageRequest {
                offset: 50,
                limit: 50
            })
        );
    }

    #[test]
    fn root_load_request_seeds_page_from_pagination_config() {
        use crate::config::view_config::PaginationConfig;
        let mut config = test_config_with_query();
        config.views[0].pagination = Some(PaginationConfig {
            mode: PaginationMode::Server,
            page_size: Some(30),
        });
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let req = view.root_load_request().unwrap();
        assert_eq!(
            req.page,
            Some(PageRequest {
                offset: 0,
                limit: 30
            })
        );
    }

    #[test]
    fn root_load_request_server_pagination_without_page_size_uses_zero_sentinel() {
        use crate::config::view_config::PaginationConfig;
        let mut config = test_config_with_query();
        config.views[0].pagination = Some(PaginationConfig {
            mode: PaginationMode::Server,
            page_size: None,
        });
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let req = view.root_load_request().unwrap();
        assert_eq!(
            req.page,
            Some(PageRequest {
                offset: 0,
                limit: 0
            })
        );
    }

    #[test]
    fn root_load_request_pagination_all_leaves_page_none() {
        use crate::config::view_config::PaginationConfig;
        let mut config = test_config_with_query();
        config.views[0].pagination = Some(PaginationConfig {
            mode: PaginationMode::All,
            page_size: None,
        });
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let req = view.root_load_request().unwrap();
        assert_eq!(req.page, None);
    }

    #[test]
    fn changing_sort_resets_page_offset_to_zero() {
        use not_yet_done_content::SortDirection;
        let config = test_config_with_query();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_current_page(Some(PageRequest {
            offset: 100,
            limit: 50,
        }));
        view.set_current_sort(vec![SortKey {
            column: "modified".into(),
            direction: SortDirection::Desc,
        }]);
        assert_eq!(
            view.current_page(),
            Some(PageRequest {
                offset: 0,
                limit: 50
            })
        );
    }

    #[test]
    fn format_page_footer_with_total_and_pages() {
        let info = PageInfo {
            offset: 50,
            limit: 25,
            total: Some(100),
            has_next: true,
            has_prev: true,
        };
        let text = format_page_footer(info, &[]);
        assert!(text.contains("Items 51\u{2013}75 of 100"), "{text}");
        assert!(text.contains("Page 3/4"), "{text}");
    }

    #[test]
    fn format_page_footer_collapses_single_page() {
        let info = PageInfo {
            offset: 0,
            limit: 50,
            total: Some(12),
            has_next: false,
            has_prev: false,
        };
        let text = format_page_footer(info, &[]);
        assert_eq!(text, "12 items");
    }

    #[test]
    fn format_page_footer_appends_sort_indicator() {
        use not_yet_done_content::SortDirection;
        let info = PageInfo {
            offset: 0,
            limit: 50,
            total: Some(12),
            has_next: false,
            has_prev: false,
        };
        let sort = vec![SortKey {
            column: "modified".into(),
            direction: SortDirection::Desc,
        }];
        let text = format_page_footer(info, &sort);
        assert!(text.contains("Sort: modified\u{25BC}"), "{text}");
    }

    #[test]
    fn next_page_request_uses_last_page_info() {
        let config = test_config_with_query();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let info = PageInfo {
            offset: 50,
            limit: 25,
            total: Some(100),
            has_next: true,
            has_prev: true,
        };
        view.set_items(vec![], Vec::new(), Some(info), Vec::new(), None);
        let next = view.next_page_request().unwrap();
        assert_eq!(
            next,
            PageRequest {
                offset: 75,
                limit: 25
            }
        );
        let prev = view.prev_page_request().unwrap();
        assert_eq!(
            prev,
            PageRequest {
                offset: 25,
                limit: 25
            }
        );
    }

    #[test]
    fn next_page_request_none_at_last_page() {
        let config = test_config_with_query();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let info = PageInfo {
            offset: 75,
            limit: 25,
            total: Some(100),
            has_next: false,
            has_prev: true,
        };
        view.set_items(vec![], Vec::new(), Some(info), Vec::new(), None);
        assert!(view.next_page_request().is_none());
        assert!(view.prev_page_request().is_some());
    }

    fn test_config_with_cursor_pagination() -> ViewFileConfig {
        use crate::config::view_config::PaginationConfig;
        let mut config = test_config_with_query();
        config.views[0].pagination = Some(PaginationConfig {
            mode: PaginationMode::Cursor,
            page_size: Some(100),
        });
        config
    }

    fn cursor_pane_with_state(view: &mut ContentView, cursor_id: Option<String>) {
        let info = PageInfo {
            offset: 0,
            limit: 100,
            total: None,
            has_next: true,
            has_prev: true,
        };
        let pane_id = view.active_pane_id();
        view.apply_custom_query_result(
            pane_id,
            Vec::new(),
            Some(info),
            Some(CustomQueryRunState {
                query: "SELECT 1".into(),
                node_id: "db/schemas/public/tables/t".into(),
                // Placeholder — apply_custom_query_result patches it.
                mode: PaginationMode::Server,
                cursor_id,
            }),
        );
    }

    /// The App side reads a pane's mode before the first query runs, to
    /// decide whether the adapter may be asked for a cursor at all. An
    /// unknown pane falls back to the mode every adapter supports.
    #[test]
    fn pane_pagination_mode_comes_from_the_view_config() {
        let config = test_config_with_cursor_pagination();
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        assert_eq!(
            view.pane_pagination_mode(view.active_pane_id()),
            PaginationMode::Cursor
        );
        assert_eq!(view.pane_pagination_mode(9999), PaginationMode::Server);

        let plain = test_config_with_query();
        let view = ContentView::new(test_theme(), &plain, None, &KeyBindingConfig::default());
        assert_eq!(
            view.pane_pagination_mode(view.active_pane_id()),
            PaginationMode::Server
        );
    }

    #[test]
    fn apply_custom_query_result_patches_mode_from_view_config() {
        let config = test_config_with_cursor_pagination();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        cursor_pane_with_state(&mut view, Some("c1".into()));
        let cq = view
            .active_pane()
            .active_custom_query
            .as_ref()
            .expect("custom query state restored");
        assert_eq!(cq.mode, PaginationMode::Cursor);
        assert_eq!(cq.cursor_id.as_deref(), Some("c1"));
    }

    #[test]
    fn try_next_page_emits_continue_when_cursor_id_present() {
        let config = test_config_with_cursor_pagination();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        cursor_pane_with_state(&mut view, Some("c1".into()));
        let view_index = view.view_index;
        let pane_id = view.active_pane_id();
        let msg = view.active_pane_mut().try_next_page(view_index, pane_id);
        match msg {
            SubViewMessage::Request(ViewRequest::RunAdapterQuery {
                cursor: Some(CursorIntent::Continue { cursor_id }),
                ..
            }) => assert_eq!(cursor_id, "c1"),
            other => panic!("expected Continue, got {other:?}"),
        }
    }

    #[test]
    fn try_next_page_emits_open_when_cursor_id_missing() {
        let config = test_config_with_cursor_pagination();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        cursor_pane_with_state(&mut view, None);
        let view_index = view.view_index;
        let pane_id = view.active_pane_id();
        let msg = view.active_pane_mut().try_next_page(view_index, pane_id);
        assert!(matches!(
            msg,
            SubViewMessage::Request(ViewRequest::RunAdapterQuery {
                cursor: Some(CursorIntent::Open),
                ..
            })
        ));
    }

    #[test]
    fn try_prev_page_reissues_open_in_cursor_mode() {
        let config = test_config_with_cursor_pagination();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        cursor_pane_with_state(&mut view, Some("c1".into()));
        let view_index = view.view_index;
        let pane_id = view.active_pane_id();
        let msg = view.active_pane_mut().try_prev_page(view_index, pane_id);
        assert!(matches!(
            msg,
            SubViewMessage::Request(ViewRequest::RunAdapterQuery {
                cursor: Some(CursorIntent::Open),
                ..
            })
        ));
    }

    #[test]
    fn close_focused_harvests_cursor_id_into_pending_queue() {
        // Two-pane setup so close_focused doesn't refuse the close as
        // "would empty the tree". Focused pane has a cursor; the other
        // pane is plain. Expect exactly one harvested id.
        let config = test_config_with_cursor_pagination();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.split_focused(SplitOrientation::Horizontal);
        // After split, focus stays on the source pane (first leaf).
        cursor_pane_with_state(&mut view, Some("c-harvest".into()));

        view.execute_window_action(WindowAction::Close);

        let drained = view.take_pending_cursor_closes();
        assert_eq!(drained, vec!["c-harvest".to_string()]);
        // Second drain yields empty — no stale state.
        assert!(view.take_pending_cursor_closes().is_empty());
    }

    #[test]
    fn close_focused_no_cursor_yields_empty_queue() {
        let config = test_config_with_cursor_pagination();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.split_focused(SplitOrientation::Horizontal);
        // Focused pane has no active custom-query — nothing to harvest.

        view.execute_window_action(WindowAction::Close);

        assert!(view.take_pending_cursor_closes().is_empty());
    }

    #[test]
    fn close_focused_refused_when_single_pane_does_not_harvest() {
        // close_focused refuses the close when the cascade would empty
        // the tree. The cursor must NOT be harvested in that case — the
        // pane is still alive and still owns the cursor.
        let config = test_config_with_cursor_pagination();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        cursor_pane_with_state(&mut view, Some("c-keep".into()));

        view.execute_window_action(WindowAction::Close);

        assert!(view.take_pending_cursor_closes().is_empty());
        // Pane still holds the cursor.
        let cq = view
            .active_pane()
            .active_custom_query
            .as_ref()
            .expect("pane still alive");
        assert_eq!(cq.cursor_id.as_deref(), Some("c-keep"));
    }

    // ── Record-detail split (`record_detail: true`) ──────────────────

    /// Minimal column def for the record-detail tests.
    fn col(key: &str, label: &str) -> ColumnDef {
        ColumnDef {
            key: key.into(),
            label: Some(label.into()),
            source: None,
            style: None,
            sizing: "max".into(),
            markdown: false,
            kind: ColumnKind::Text,
            format: None,
            separator: None,
            elapsed_from: None,
            tree_aggregate: None,
            hidden: false,
            collapsed_source: None,
            long_source: None,
        }
    }

    /// `test_config_with_query` with the root view opted into the
    /// record-detail split. The follower mirrors the view's columns, so the
    /// view is given the record's two fields (the order + labels the
    /// transpose is asserted against).
    fn record_detail_config() -> ViewFileConfig {
        let mut config = test_config_with_query();
        config.views[0].record_detail = true;
        config.views[0].columns = vec![col("name", "Name"), col("status", "Status")];
        config
    }

    /// A flat record carrying two metadata fields — the source row a
    /// follower transposes.
    fn record_item(id: &str) -> NodeSummary {
        use not_yet_done_content::{Metadata, MetadataField};
        let mut n = tnode(id, id, "mock:issue");
        n.metadata = Metadata {
            fields: vec![
                MetadataField {
                    key: "name".into(),
                    value: format!("{id}-name"),
                    display_label: "Name".into(),
                    editable: false,
                    allowed_values: None,
                },
                MetadataField {
                    key: "status".into(),
                    value: format!("{id}-status"),
                    display_label: "Status".into(),
                    editable: false,
                    allowed_values: None,
                },
            ],
        };
        n
    }

    /// Value of `key` in the follower's `row`-th synthetic item.
    fn follower_cell(view: &ContentView, follower_id: PaneId, row: usize, key: &str) -> String {
        let pane = view.find_pane(follower_id).expect("follower alive");
        pane.items[row]
            .metadata
            .fields
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.value.clone())
            .unwrap_or_default()
    }

    #[test]
    fn toggle_record_detail_opens_follower_split() {
        let config = record_detail_config();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(vec![record_item("a")], Vec::new(), None, Vec::new(), None);
        let source_id = view.active_pane_id();

        let msg = view.toggle_record_detail();
        assert!(matches!(msg, SubViewMessage::SelectionChanged(None)));
        // A second leaf now exists and focus stays on the source.
        assert_eq!(view.pane_trees[view.active_subtab].leaf_count(), 2);
        assert_eq!(view.active_pane_id(), source_id);
        // Backlinks wired both ways.
        let follower_id = view.find_pane(source_id).unwrap().detail_child.unwrap();
        assert_ne!(follower_id, source_id);
        assert_eq!(
            view.find_pane(follower_id).unwrap().detail_source,
            Some(source_id)
        );
        assert!(view.find_pane(follower_id).unwrap().is_detail_pane());
    }

    #[test]
    fn toggle_record_detail_refused_without_opt_in() {
        // Plain config (record_detail = false) → toggle is a no-op.
        let config = test_config_with_query();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(vec![record_item("a")], Vec::new(), None, Vec::new(), None);

        let msg = view.toggle_record_detail();
        assert!(matches!(msg, SubViewMessage::Unhandled));
        assert_eq!(view.pane_trees[view.active_subtab].leaf_count(), 1);
    }

    #[test]
    fn toggle_record_detail_again_closes_follower() {
        let config = record_detail_config();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(vec![record_item("a")], Vec::new(), None, Vec::new(), None);
        let source_id = view.active_pane_id();

        view.toggle_record_detail();
        assert_eq!(view.pane_trees[view.active_subtab].leaf_count(), 2);

        // Pressed again from the source — the follower closes, source stays.
        view.toggle_record_detail();
        assert_eq!(view.pane_trees[view.active_subtab].leaf_count(), 1);
        assert_eq!(view.active_pane_id(), source_id);
        assert!(view.find_pane(source_id).unwrap().detail_child.is_none());
    }

    #[test]
    fn sync_detail_panes_transposes_selected_record() {
        let config = record_detail_config();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(vec![record_item("a")], Vec::new(), None, Vec::new(), None);
        let source_id = view.active_pane_id();
        view.toggle_record_detail();
        let follower_id = view.find_pane(source_id).unwrap().detail_child.unwrap();

        // rebuild_table runs sync_detail_panes; the follower now holds one
        // synthetic row per source field, in source order.
        view.rebuild_table();
        let follower = view.find_pane(follower_id).unwrap();
        assert_eq!(follower.items.len(), 2);
        assert_eq!(
            follower_cell(&view, follower_id, 0, content_detail::FIELD_KEY),
            "Name"
        );
        assert_eq!(
            follower_cell(&view, follower_id, 0, content_detail::VALUE_KEY),
            "a-name"
        );
        assert_eq!(
            follower_cell(&view, follower_id, 1, content_detail::FIELD_KEY),
            "Status"
        );
        assert_eq!(
            follower_cell(&view, follower_id, 1, content_detail::VALUE_KEY),
            "a-status"
        );
    }

    #[test]
    fn sync_detail_panes_follows_column_config() {
        // A relabelled, reordered subset: the follower shows exactly these
        // columns (State before the name field, with custom labels), proving
        // it follows the column config rather than the raw record fields.
        let mut config = record_detail_config();
        config.views[0].columns = vec![col("status", "State"), col("name", "Full name")];
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(vec![record_item("a")], Vec::new(), None, Vec::new(), None);
        let source_id = view.active_pane_id();
        view.toggle_record_detail();
        let follower_id = view.find_pane(source_id).unwrap().detail_child.unwrap();

        view.rebuild_table();
        assert_eq!(view.find_pane(follower_id).unwrap().items.len(), 2);
        assert_eq!(
            follower_cell(&view, follower_id, 0, content_detail::FIELD_KEY),
            "State"
        );
        assert_eq!(
            follower_cell(&view, follower_id, 0, content_detail::VALUE_KEY),
            "a-status"
        );
        assert_eq!(
            follower_cell(&view, follower_id, 1, content_detail::FIELD_KEY),
            "Full name"
        );
        assert_eq!(
            follower_cell(&view, follower_id, 1, content_detail::VALUE_KEY),
            "a-name"
        );
    }

    #[test]
    fn sync_detail_panes_without_columns_shows_all_fields() {
        // No configured columns (postgres and other dynamic-schema views):
        // `current_columns` auto-derives one per record field, so the follower
        // still shows the whole record — unchanged from before this feature.
        let mut config = record_detail_config();
        config.views[0].columns = vec![];
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(vec![record_item("a")], Vec::new(), None, Vec::new(), None);
        let source_id = view.active_pane_id();
        view.toggle_record_detail();
        let follower_id = view.find_pane(source_id).unwrap().detail_child.unwrap();

        view.rebuild_table();
        assert_eq!(view.find_pane(follower_id).unwrap().items.len(), 2);
        assert_eq!(
            follower_cell(&view, follower_id, 0, content_detail::FIELD_KEY),
            "Name"
        );
        assert_eq!(
            follower_cell(&view, follower_id, 1, content_detail::FIELD_KEY),
            "Status"
        );
    }

    #[test]
    fn closing_source_cascades_the_detail_follower() {
        // A coupled-drill sibling keeps the tree non-empty so close_focused
        // doesn't refuse; closing the source must still take its detail
        // follower with it.
        let config = record_detail_config();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(vec![record_item("a")], Vec::new(), None, Vec::new(), None);
        let source_id = view.active_pane_id();
        // A plain split gives us a 2nd unrelated leaf; then open the detail
        // follower off the (still-focused) source → 3 leaves.
        view.split_focused(SplitOrientation::Horizontal);
        // split_focused focuses the new pane; refocus the source.
        view.pane_trees[view.active_subtab].focus = source_id;
        view.toggle_record_detail();
        assert_eq!(view.pane_trees[view.active_subtab].leaf_count(), 3);

        // Close the source → cascade removes its detail follower too,
        // leaving only the unrelated split pane.
        view.pane_trees[view.active_subtab].focus = source_id;
        view.close_focused();
        assert_eq!(view.pane_trees[view.active_subtab].leaf_count(), 1);
    }

    #[test]
    fn try_next_page_server_mode_keeps_limit_offset_path() {
        let config = test_config_with_query();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let info = PageInfo {
            offset: 0,
            limit: 100,
            total: None,
            has_next: true,
            has_prev: false,
        };
        let pane_id = view.active_pane_id();
        view.apply_custom_query_result(
            pane_id,
            Vec::new(),
            Some(info),
            Some(CustomQueryRunState {
                query: "SELECT 1".into(),
                node_id: "db/schemas/public/tables/t".into(),
                mode: PaginationMode::Server,
                cursor_id: None,
            }),
        );
        let view_index = view.view_index;
        let msg = view.active_pane_mut().try_next_page(view_index, pane_id);
        assert!(matches!(
            msg,
            SubViewMessage::Request(ViewRequest::RunAdapterQuery { cursor: None, .. })
        ));
    }

    #[test]
    fn set_items_persists_applied_sort_and_page_info() {
        use not_yet_done_content::SortDirection;
        let config = test_config_with_query();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let applied = vec![SortKey {
            column: "ref".into(),
            direction: SortDirection::Asc,
        }];
        let page = PageInfo {
            offset: 0,
            limit: 25,
            total: Some(83),
            has_next: true,
            has_prev: false,
        };
        view.set_items(mock_issues(), applied.clone(), Some(page), Vec::new(), None);
        assert_eq!(view.last_applied_sort(), applied.as_slice());
        assert_eq!(view.last_page_info(), Some(page));
    }

    #[test]
    fn root_load_request_uses_default_query() {
        let config = test_config_with_query();
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let req = view.root_load_request().unwrap();
        assert_eq!(req.node_type_id, "mock:issue");
        assert_eq!(req.query.as_deref(), Some("assignee = me"));
        assert!(req.sort.is_empty());
        assert!(req.page.is_none());
    }

    #[test]
    fn set_query_overrides_default() {
        let config = test_config_with_query();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_query("type = Bug".into(), Some("My Bugs".into()));
        let req = view.root_load_request().unwrap();
        assert_eq!(req.query.as_deref(), Some("type = Bug"));
        assert_eq!(
            view.active_pane().active_query_name.as_deref(),
            Some("My Bugs")
        );
    }

    #[test]
    fn text_search_hint_stays_lit_while_its_query_is_shown() {
        // The `text_search` hint must light up while the term is typed *and*
        // for as long as the pane still shows what the search produced — the
        // local `/`-search hint must stay dark throughout.
        let config = test_config_with_query();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let view_defs = config.views.clone();
        let pane_id = view.active_pane_id();
        {
            let pane = view.active_pane_mut();
            pane.search_mode = SearchMode::Adapter {
                template: "queries:\n  - { type: task, q: \"<input>\" }".into(),
                prompt: None,
            };
            pane.search.open();
        }
        assert!(view.resolve_active(&ActiveSurface::TextSearch));
        assert!(!view.resolve_active(&ActiveSurface::Search));

        for key in ["1", "1", "2", "enter"] {
            view.active_pane_mut()
                .handle_search_key(key, 0, pane_id, &view_defs);
        }
        assert!(
            view.resolve_active(&ActiveSurface::TextSearch),
            "hint must stay lit after the input closes — the result is still on screen"
        );
        assert!(!view.resolve_active(&ActiveSurface::Search));
        assert_eq!(
            view.active_pane().active_query.as_deref(),
            Some("queries:\n  - { type: task, q: \"112\" }")
        );

        // Any other query ends the search — no explicit reset needed.
        view.set_query("assignee = me".into(), None);
        assert!(!view.resolve_active(&ActiveSurface::TextSearch));
    }

    #[test]
    fn apply_default_query_stamps_inheriting_subtab_panes() {
        // The startup default-query apply stamps the active (default
        // view) pane plus every subtab opting in via
        // `query.inherit_default`; plain sibling views keep their own
        // query semantics untouched.
        let mut config = test_config_with_query();
        let mut inheriting = config.views[0].clone();
        inheriting.name = "condensed".into();
        inheriting.default = false;
        inheriting.query.as_mut().unwrap().inherit_default = true;
        let mut plain = config.views[0].clone();
        plain.name = "other".into();
        plain.default = false;
        config.views.push(inheriting);
        config.views.push(plain);

        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.apply_default_query(
            "type = Bug".into(),
            Some("My Bugs".into()),
            QueryKind::Saved,
        );

        assert_eq!(
            view.active_pane().active_query.as_deref(),
            Some("type = Bug"),
            "default view pane is stamped as before"
        );
        let PaneNode::Leaf(inheriting_leaf) = &view.pane_trees[1].root else {
            panic!("expected single-leaf pane tree");
        };
        assert_eq!(
            inheriting_leaf.pane.active_query.as_deref(),
            Some("type = Bug"),
            "inherit_default subtab pane is stamped too"
        );
        assert_eq!(
            inheriting_leaf.pane.active_query_name.as_deref(),
            Some("My Bugs")
        );
        let PaneNode::Leaf(plain_leaf) = &view.pane_trees[2].root else {
            panic!("expected single-leaf pane tree");
        };
        assert!(
            plain_leaf.pane.active_query.is_none(),
            "non-inheriting sibling view stays untouched"
        );
    }

    #[test]
    fn current_query_text_returns_active_or_default() {
        let config = test_config_with_query();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        assert_eq!(view.current_query_text(), "assignee = me");
        view.set_query("custom query".into(), None);
        assert_eq!(view.current_query_text(), "custom query");
    }

    #[test]
    fn an_extended_query_keeps_its_kind_from_the_store_to_the_load_request() {
        // Nothing about the body says which store it came from, so the kind
        // has to survive every hop — merge, menu lookup, stamp, load — or the
        // loader hands a Markdown document straight to the adapter.
        let config = test_config_with_query();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.merge_saved_queries(vec![
            merged_query("Native", "type = Bug", None),
            MergedSavedQuery {
                name: "Combined".into(),
                query: "```yaml\nquery: type = Bug\n```\n".into(),
                shortcut: None,
                kind: QueryKind::Extended,
            },
        ]);
        assert_eq!(view.query_kind_of("Combined"), QueryKind::Extended);
        assert_eq!(view.query_kind_of("Native"), QueryKind::Saved);
        assert_eq!(
            view.query_kind_of("typed into the editor"),
            QueryKind::Saved,
            "a body with no entry in either store is adapter-native"
        );

        let pane_id = view.active_pane_id();
        view.set_query_for_pane_with_vars(
            pane_id,
            "```yaml\nquery: type = Bug\n```\n".into(),
            Some("Combined".into()),
            std::collections::HashMap::new(),
            QueryKind::Extended,
        );
        let req = view.root_load_request().expect("load request");
        assert_eq!(req.kind, QueryKind::Extended);

        // Clearing back to the view's YAML default drops the kind with it —
        // that default is always a literal query in the adapter's language.
        view.set_query("type = Bug".into(), None);
        assert_eq!(view.root_load_request().unwrap().kind, QueryKind::Saved);
    }

    #[test]
    fn is_query_editable_from_config() {
        let config = test_config_with_query();
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        assert!(view.is_query_editable());

        let config2 = test_config_with_children();
        let view2 = ContentView::new(test_theme(), &config2, None, &KeyBindingConfig::default());
        assert!(!view2.is_query_editable());
    }

    #[test]
    fn saved_query_shortcut_dispatches_apply_request() {
        // A DB-stored shortcut bound to a saved query defers the
        // actual set_query to the App-side dispatcher
        // (App::start_query_apply) so the adapter can introduce a
        // variable-input popup before the load. The view's only job
        // here is to surface the `ApplyContentSavedQuery` request
        // with the right query/name.
        let config = test_config_with_query();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.merge_saved_queries(vec![merged_query("My Bugs", "type = Bug", Some("1"))]);
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);
        let msg = view.handle_key("1");
        match msg {
            SubViewMessage::Request(ViewRequest::ApplyContentSavedQuery {
                query, name, ..
            }) => {
                assert_eq!(query, "type = Bug");
                assert_eq!(name, "My Bugs");
            }
            other => panic!("Expected ApplyContentSavedQuery, got {other:?}"),
        }
    }

    #[test]
    fn query_menu_set_default_dispatches_request() {
        // ctrl+t on a query-menu entry surfaces SetDefaultContentQuery
        // so the App can toggle + persist the per-scope default.
        let config = test_config_with_query();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.merge_saved_queries(vec![merged_query("My Bugs", "type = Bug", None)]);
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);
        view.open_query_popup();
        let msg = view.handle_query_popup_key("ctrl+t");
        match msg {
            Some(SubViewMessage::Request(ViewRequest::SetDefaultContentQuery { name, .. })) => {
                assert_eq!(name, "My Bugs")
            }
            other => panic!("Expected SetDefaultContentQuery, got {other:?}"),
        }
        assert!(!view.has_query_popup());
    }

    #[test]
    fn a_double_plus_name_opens_the_editor_for_a_new_extended_query() {
        // The kind is decided when the entry is created — there is no
        // entry to look it up from yet, and the body the user is about to
        // write is what will tell the two apart.
        let config = test_config_with_query();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.merge_saved_queries(vec![merged_query("My Bugs", "type = Bug", None)]);
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);
        view.open_query_popup();
        for c in "++Combined".chars() {
            view.handle_query_popup_key(&c.to_string());
        }
        match view.handle_query_popup_key("enter") {
            Some(SubViewMessage::Request(ViewRequest::OpenContentQueryEditor {
                save_name,
                is_new,
                kind,
                ..
            })) => {
                assert_eq!(save_name.as_deref(), Some("Combined"));
                assert!(is_new);
                assert_eq!(kind, QueryKind::Extended);
            }
            other => panic!("Expected OpenContentQueryEditor, got {other:?}"),
        }
    }

    #[test]
    fn editing_an_extended_entry_opens_the_editor_on_its_own_kind() {
        // ctrl+e stamps the body onto the pane and opens the editor; both
        // have to hear that this is a document, or it is saved back into
        // the wrong store in the wrong language.
        let config = test_config_with_query();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.merge_saved_queries(vec![MergedSavedQuery {
            kind: QueryKind::Extended,
            ..merged_query("Combined", "```yaml\nquery-ref: a\n```\n", None)
        }]);
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);
        view.open_query_popup();
        match view.handle_query_popup_key("ctrl+e") {
            Some(SubViewMessage::Request(ViewRequest::OpenContentQueryEditor {
                kind,
                is_new,
                ..
            })) => {
                assert_eq!(kind, QueryKind::Extended);
                assert!(!is_new);
            }
            other => panic!("Expected OpenContentQueryEditor, got {other:?}"),
        }
    }

    #[test]
    fn q_key_triggers_query_editor() {
        let config = test_config_with_query();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);
        let msg = view.handle_key("Q");
        match msg {
            SubViewMessage::Request(ViewRequest::OpenContentQueryEditor { .. }) => {}
            other => panic!("Expected OpenContentQueryEditor, got {other:?}"),
        }
    }

    #[test]
    fn script_action_dispatches_open_script_menu_for_node() {
        // Regression check: a YAML `type: script` action with key `x`
        // must reach `OpenScriptMenuForNode` via the per-view action
        // dispatcher. Mirrors the user's taiga.yaml configuration.
        let mut config = test_config_with_children();
        config.views[0].actions.push(ActionDef {
            name: "script".into(),
            key: Some("x".into()),
            action_type: "script".into(),
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
        });
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);
        let msg = view.handle_key("x");
        match msg {
            SubViewMessage::Request(ViewRequest::OpenScriptMenuForNode { .. }) => {}
            other => panic!("Expected OpenScriptMenuForNode, got {other:?}"),
        }
    }

    #[test]
    fn create_under_selection_targets_selected_node() {
        // `under_selection: true` re-targets a `create` onto the highlighted
        // row instead of the drilled-into container. Backs the task tree's
        // `A` (add-child-under-cursor) so nesting works without drilling in.
        let mut config = test_config_with_children();
        config.views[0].actions.push(ActionDef {
            name: "add child".into(),
            key: Some("A".into()),
            action_type: "create".into(),
            id: Some("add".into()),
            node_id_from: None,
            navigate_to: None,
            fuzzy_filter: None,
            search: None,
            text_search: None,
            tree_find: None,
            hide_from_bar: false,
            in_action_bar: false,
            editor: None,
            under_selection: true,
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
        });
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let issues = mock_issues();
        let first_id = issues[0].id.clone();
        view.set_items(issues, Vec::new(), None, Vec::new(), None);
        match view.handle_key("A") {
            SubViewMessage::Request(ViewRequest::CreateContentChild { parent_node_id, .. }) => {
                assert_eq!(
                    parent_node_id,
                    Some(first_id),
                    "under_selection create should parent on the selected row"
                );
            }
            other => panic!("Expected CreateContentChild on selection, got {other:?}"),
        }
    }

    #[test]
    fn script_action_shows_in_action_bar() {
        // shows_in_action_bar() must return true for `type: script` so the
        // top action bar lists it alongside `edit` / `create` / etc.
        let mut config = test_config_with_children();
        config.views[0].actions.push(ActionDef {
            name: "script".into(),
            key: Some("x".into()),
            action_type: "script".into(),
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
        });
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let hints = view.action_bar_hints();
        assert!(
            hints.iter().any(|h| h.key == "x" && h.desc == "script"),
            "expected [x] script in action bar, got: {hints:?}"
        );
    }

    #[test]
    fn action_bar_shows_query_hints() {
        let config = test_config_with_query();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let hints = view.action_bar_hints();
        assert!(hints.iter().any(|h| h.key == "Q"));
        assert!(hints.iter().any(|h| h.key == "q" && h.desc == "queries"));

        view.set_query("type = Bug".into(), Some("My Bugs".into()));
        assert_eq!(
            view.active_pane().active_query_name.as_deref(),
            Some("My Bugs")
        );

        view.merge_saved_queries(vec![
            merged_query("My Bugs", "type = Bug", Some("1")),
            merged_query("Sprint", "sprint in open", Some("2")),
        ]);
        let favs: Vec<(&str, &str)> = view
            .db_saved_queries
            .iter()
            .filter_map(|sq| sq.shortcut.as_deref().map(|s| (sq.name.as_str(), s)))
            .collect();
        assert!(favs.contains(&("My Bugs", "1")));
        assert!(favs.contains(&("Sprint", "2")));
    }

    #[test]
    fn status_bar_hints_include_back_when_drilled() {
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);

        let hints = view.status_bar_hints();
        assert!(!hints.iter().any(|(_, v)| v == "back"));
        assert!(hints.iter().any(|(_, v)| v == "open"));

        let child_def = config.views[0].children[0].clone();
        let view_defs = view.view_defs.clone();
        view.active_pane_mut()
            .drill_down_prepare("ISS-1", "First issue", &child_def, &view_defs);

        let hints = view.status_bar_hints();
        assert!(hints.iter().any(|(k, v)| v == "back" && k == "⌫"));
        assert!(!hints.iter().any(|(_, v)| v == "open"));
    }

    #[test]
    fn status_bar_hints_include_fold_chords_on_tree_pane() {
        // The regression that motivated the claim-derived bar: `zm`/`zr`
        // (and backspace smart-collapse) must surface in the status bar on a
        // tree pane *automatically*, because `build_claims` now claims them
        // and the bar derives its nav hints from that same claim set — no
        // hand-listed hint entry.
        let config = uniform_recursive_config();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(
            vec![tnode_val("work", "Work", "W")],
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        assert!(
            view.active_pane().tree.is_some(),
            "fixture must be tree mode"
        );

        let hints = view.status_bar_hints();
        assert!(
            hints.iter().any(|(k, v)| v == "collapse all" && k == "zm"),
            "expected [zm] collapse all, got: {hints:?}"
        );
        assert!(
            hints.iter().any(|(k, v)| v == "expand all" && k == "zr"),
            "expected [zr] expand all, got: {hints:?}"
        );
        assert!(
            hints.iter().any(|(_, v)| v == "collapse"),
            "expected smart-collapse hint, got: {hints:?}"
        );
    }

    #[test]
    fn status_bar_hints_list_window_chords_when_view_opts_in() {
        // `w*` used to be invisible until the leader was already pressed. On a
        // `window_ops: true` view every window chord now sits in the status
        // bar with its full chord, exactly as `zm`/`zr` do.
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.view_defs[0].window_ops = true;
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);

        let hints = view.status_bar_hints();
        for (key, label) in [
            ("wv", "split right"),
            ("ws", "split down"),
            ("wq", "close pane"),
            ("wh", "focus parent"),
            ("wl", "focus child"),
        ] {
            assert!(
                hints.iter().any(|(k, v)| k == key && v == label),
                "expected [{key}] {label}, got: {hints:?}"
            );
        }
        // The pane-tag switch (`w<tag>`) is layout-derived, not a binding —
        // it must stay out of the bar and live only in the WINDOW-mode prompt.
        assert!(
            !hints.iter().any(|(_, v)| v == "switch pane"),
            "pane-tag switch must not reach the status bar, got: {hints:?}"
        );
    }

    #[test]
    fn status_bar_hints_omit_window_chords_without_window_ops() {
        // Window ops are opt-in per view; where the leader never engages the
        // bar must not advertise the chords.
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        assert!(!view.view_defs[0].window_ops, "fixture must default to off");
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);

        let hints = view.status_bar_hints();
        assert!(
            !hints.iter().any(|(k, _)| k.starts_with('w')),
            "no window chord may show without window_ops, got: {hints:?}"
        );
    }

    // -- Auth-status banner --

    #[test]
    fn auth_status_banner_hidden_when_ready() {
        let config = test_config_with_children();
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        assert!(view.auth_status_banner().is_none());
    }

    #[test]
    fn is_busy_tracks_adapter_status() {
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        assert!(!view.is_busy(), "fresh view is Ready, not Busy");
        view.set_auth_status(AdapterStatus::Busy {
            label: "Running query".into(),
            started_at_unix_ms: 0,
            timeout_secs: 7,
            progress: None,
        });
        assert!(
            view.is_busy(),
            "Busy drives the ~1 Hz live-banner redraw nudge"
        );
        // Connecting counts as a static banner, not a wall-clock one.
        view.set_auth_status(AdapterStatus::Connecting {
            retry: 1,
            max_retries: 3,
            timeout_secs: 30,
        });
        assert!(
            !view.is_busy(),
            "Connecting banner text is static, not live"
        );
        view.set_auth_status(AdapterStatus::Ready);
        assert!(!view.is_busy());
    }

    #[test]
    fn busy_banner_renders_progress_as_percentage() {
        // No progress → no percentage, just elapsed.
        let plain = busy_banner("Loading calendar", 0, 0, None);
        assert!(!plain.contains('%'), "no progress → no %, got: {plain}");

        // A fraction renders as a rounded whole percent between label and timer.
        let with = busy_banner("Loading calendar", 0, 0, Some(0.5));
        assert!(with.contains("50 %"), "expected '50 %', got: {with}");

        // Out-of-range fractions are clamped, not shown raw.
        assert!(busy_banner("x", 0, 0, Some(1.5)).contains("100 %"));
        assert!(busy_banner("x", 0, 0, Some(-0.3)).contains("0 %"));
    }

    /// Helper: a view that is loading, routed as given.
    fn busy_view(route: LoadBannerRoute) -> ContentView {
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_load_banner_default(route);
        view.set_auth_status(AdapterStatus::Busy {
            label: "Loading".into(),
            started_at_unix_ms: 0,
            timeout_secs: 0,
            progress: None,
        });
        view
    }

    #[test]
    fn the_tab_route_keeps_the_load_banner_on_the_tabs_own_line() {
        let view = busy_view(LoadBannerRoute::Tab);
        assert!(
            view.auth_status_banner()
                .is_some_and(|b| b.contains("Loading")),
            "the default route draws the counter locally"
        );
        assert!(
            view.global_load_banner().is_none(),
            "and hands nothing to the global surface"
        );
    }

    #[test]
    fn routing_a_load_banner_away_takes_it_off_the_tabs_line() {
        for route in [LoadBannerRoute::Global, LoadBannerRoute::Off] {
            let view = busy_view(route);
            assert!(
                view.auth_status_banner().is_none(),
                "{route:?} must not also draw the counter in the tab"
            );
        }
        // Only `global` offers it to the App; `off` means nowhere at all.
        assert!(
            busy_view(LoadBannerRoute::Global)
                .global_load_banner()
                .is_some()
        );
        assert!(
            busy_view(LoadBannerRoute::Off)
                .global_load_banner()
                .is_none()
        );
    }

    #[test]
    fn a_tab_override_beats_the_global_default() {
        let mut config = test_config_with_children();
        config.tab.load_banner = Some(LoadBannerRoute::Global);
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        // App applies the global setting to every view, override or not.
        view.set_load_banner_default(LoadBannerRoute::Off);
        assert_eq!(view.load_banner_route(), LoadBannerRoute::Global);
    }

    #[test]
    fn a_retry_stays_in_its_tab_even_while_the_load_banner_is_routed_away() {
        let mut view = busy_view(LoadBannerRoute::Global);
        view.active_pane_mut().retry_state = Some(RetryState {
            attempt: 2,
            max_attempts: 5,
            last_error: "connection reset".into(),
        });
        let banner = view
            .auth_status_banner()
            .expect("a retry is a fault, not progress — it belongs where it happened");
        assert!(banner.contains("(2/5)"), "got: {banner}");
        assert!(banner.contains("connection reset"), "got: {banner}");
        assert!(
            !banner.contains("Loading"),
            "the routed-away counter must not sneak back in: {banner}"
        );
    }

    #[test]
    fn several_loading_tabs_collapse_into_one_counter() {
        let line = collapsed_load_banner(3, 0);
        assert!(line.starts_with("3 tabs loading…"), "got: {line}");
        assert!(line.contains('s'), "elapsed seconds expected, got: {line}");
    }

    #[test]
    fn auth_status_banner_shows_connecting_with_retry_and_timeout() {
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_auth_status(AdapterStatus::Connecting {
            retry: 2,
            max_retries: 5,
            timeout_secs: 30,
        });
        let banner = view.auth_status_banner().unwrap();
        assert!(banner.contains("(2/5)"), "got: {banner}");
        assert!(banner.contains("30s"), "got: {banner}");
        assert!(banner.starts_with("Connecting"), "got: {banner}");
    }

    #[test]
    fn auth_status_banner_shows_failure_reason() {
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_auth_status(AdapterStatus::Failed {
            reason: "cookie script failed after 3 attempt(s)".into(),
        });
        let banner = view.auth_status_banner().unwrap();
        assert!(banner.starts_with("Connection failed"), "got: {banner}");
        assert!(banner.contains("3 attempt(s)"), "got: {banner}");
    }

    #[test]
    fn auth_status_banner_shows_init_error() {
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_adapter_init_error("unknown field `cache`".into());
        let banner = view.auth_status_banner().unwrap();
        assert!(banner.starts_with("Configuration error"), "got: {banner}");
        assert!(banner.contains("unknown field `cache`"), "got: {banner}");
    }

    #[test]
    fn auth_status_banner_shows_fetch_error_when_idle_or_ready() {
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(
            Vec::new(),
            Vec::new(),
            None,
            Vec::new(),
            Some("web-notifications 401: token expired".into()),
        );
        let banner = view.auth_status_banner().unwrap();
        assert!(banner.starts_with("Fetch failed"), "got: {banner}");
        assert!(banner.contains("401"), "got: {banner}");
    }

    #[test]
    fn fetch_error_yields_to_auth_status_when_connecting() {
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(
            Vec::new(),
            Vec::new(),
            None,
            Vec::new(),
            Some("stale".into()),
        );
        view.set_auth_status(AdapterStatus::Connecting {
            retry: 1,
            max_retries: 3,
            timeout_secs: 30,
        });
        let banner = view.auth_status_banner().unwrap();
        assert!(banner.starts_with("Connecting"), "got: {banner}");
    }

    #[test]
    fn init_error_takes_precedence_over_auth_status() {
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_adapter_init_error("bad yaml".into());
        view.set_auth_status(AdapterStatus::Connecting {
            retry: 1,
            max_retries: 3,
            timeout_secs: 30,
        });
        let banner = view.auth_status_banner().unwrap();
        assert!(banner.starts_with("Configuration error"), "got: {banner}");
    }

    fn manual_connect_config_with_reload_key(reload_key: Option<&str>) -> ViewFileConfig {
        let mut config = test_config_with_children();
        config.adapter.manual_connect = true;
        if let Some(k) = reload_key {
            config.views[0].actions.push(ActionDef {
                name: "refresh".into(),
                key: Some(k.into()),
                action_type: "reload".into(),
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
            });
        }
        config
    }

    #[test]
    fn manual_connect_banner_names_reload_key_when_pane_unloaded() {
        let config = manual_connect_config_with_reload_key(Some("r"));
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let banner = view.auth_status_banner().expect("banner expected");
        assert!(banner.contains("Auto-connect disabled"), "got: {banner}");
        assert!(banner.contains("`r`"), "got: {banner}");
    }

    #[test]
    fn manual_connect_banner_falls_back_when_no_reload_action() {
        let config = manual_connect_config_with_reload_key(None);
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let banner = view.auth_status_banner().expect("banner expected");
        assert!(banner.contains("no `reload` action"), "got: {banner}");
    }

    #[test]
    fn manual_connect_banner_disappears_once_loaded() {
        let config = manual_connect_config_with_reload_key(Some("r"));
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(Vec::new(), Vec::new(), None, Vec::new(), None);
        assert!(view.auth_status_banner().is_none());
    }

    #[test]
    fn manual_connect_off_keeps_legacy_behaviour() {
        let config = test_config_with_children();
        assert!(!config.adapter.manual_connect, "fixture sanity");
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        assert!(view.auth_status_banner().is_none());
    }

    // ── Shortcut Hints (SH-1..SH-7) ─────────────────────────────────

    use not_yet_done_content::{InputSpec, NodeAction};

    fn shortcut_config_flat(
        view_shortcuts: HashMap<char, String>,
        child_shortcuts: HashMap<char, String>,
    ) -> ViewFileConfig {
        let view_shortcuts: HashMap<char, ShortcutDef> = view_shortcuts
            .into_iter()
            .map(|(k, v)| (k, ShortcutDef::Action(v)))
            .collect();
        let child_shortcuts: HashMap<char, ShortcutDef> = child_shortcuts
            .into_iter()
            .map(|(k, v)| (k, ShortcutDef::Action(v)))
            .collect();
        ViewFileConfig {
            reminder: None,
            tab: TabConfig {
                name: "Test".into(),
                order: 0,
                icon: None,
                key: None,
                unread_marker: None,
                unread_style: None,
                load_banner: None,
            },
            adapter: AdapterConfig {
                adapter_type: "mock".into(),
                id: None,
                config: None,
                config_inline: None,
                manual_connect: false,
            },
            views: vec![ViewDef {
                card: None,
                row_layout: None,
                smooth_scroll: false,
                name: "issues".into(),
                node_type: "mock:issue".into(),
                default: true,
                window_ops: false,
                key: None,
                query: None,
                columns: vec![ColumnDef {
                    key: "label".into(),
                    label: Some("Label".into()),
                    source: Some("label".into()),
                    style: None,
                    sizing: "max".into(),
                    markdown: false,
                    kind: ColumnKind::Text,
                    format: None,
                    separator: None,
                    elapsed_from: None,
                    tree_aggregate: None,
                    hidden: false,
                    collapsed_source: None,
                    long_source: None,
                }],
                preview: None,
                actions: vec![],
                children: vec![ChildDef {
                    card: None,
                    row_layout: None,
                    smooth_scroll: false,
                    name: "Comments".into(),
                    node_type: "mock:comment".into(),
                    columns: vec![ColumnDef {
                        key: "label".into(),
                        label: Some("Comment".into()),
                        source: Some("label".into()),
                        style: None,
                        sizing: "flex(1)".into(),
                        markdown: false,
                        kind: ColumnKind::Text,
                        format: None,
                        separator: None,
                        elapsed_from: None,
                        tree_aggregate: None,
                        hidden: false,
                        collapsed_source: None,
                        long_source: None,
                    }],
                    preview: None,
                    actions: vec![],
                    children: vec![],
                    split: None,
                    pagination: None,
                    keybindings: HashMap::new(),
                    action_chains: Default::default(),
                    column_cursor: false,
                    record_detail: false,
                    node_scripts: false,
                    tree_label: None,
                    shortcuts: child_shortcuts,
                    enter_action: None,
                    recursive: false,
                    editor_in_place: false,
                    leaf_glyph: None,
                    icon: None,
                    group_by: None,
                    aggregates: Vec::new(),
                    mark_read_on_reach_end: None,
                    cursor_on_open: None,
                }],
                pagination: None,
                action_chains: Default::default(),
                column_cursor: false,
                record_detail: false,
                node_scripts: false,
                tree_label: None,
                retries: 0,
                script_template: None,
                script_source: None,
                shortcuts: view_shortcuts,
                leaf_glyph: None,
                icon: None,
                group_by: None,
                aggregates: Vec::new(),
                tree_connector_style: None,
                unread_style: None,
                unread_marker: None,
                tree_lines: None,
                tree_markers: None,
                expand_depth: None,
                group_headers: None,
                event_actions: Vec::new(),
            }],
        }
    }

    /// Build a test action. The `input` shape drives everything the bar
    /// builders now derive: an activatable spec (Editor/Form/Picker/…) makes
    /// the hint action-bar-placed, `InputSpec::None` (with a non-whitelisted
    /// id) makes it status-bar-placed.
    fn make_action(id: &str, label: &str, input: InputSpec) -> NodeAction {
        NodeAction::new(id, label, input)
    }

    #[test]
    fn current_shortcuts_at_root_returns_view_def_entries() {
        let mut sc = HashMap::new();
        sc.insert('a', "do_alpha".to_string());
        sc.insert('b', "parent:do_beta".to_string());
        let config = shortcut_config_flat(sc, HashMap::new());
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());

        let mut entries = view.active_pane().current_shortcuts(&view.view_defs);
        entries.sort_by_key(|(k, _)| *k);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], ('a', "do_alpha".to_string()));
        assert_eq!(entries[1], ('b', "parent:do_beta".to_string()));
    }

    #[test]
    fn current_shortcuts_after_drill_child_overrides_view() {
        let mut view_sc = HashMap::new();
        view_sc.insert('a', "view_alpha".to_string());
        view_sc.insert('c', "view_charlie".to_string());
        let mut child_sc = HashMap::new();
        // 'a' clashes with view-level — child wins; 'b' is child-only.
        child_sc.insert('a', "child_alpha".to_string());
        child_sc.insert('b', "child_bravo".to_string());

        let config = shortcut_config_flat(view_sc, child_sc);
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);

        let child_def = config.views[0].children[0].clone();
        let view_defs = view.view_defs.clone();
        view.active_pane_mut()
            .drill_down_prepare("ISS-1", "First issue", &child_def, &view_defs);

        let mut entries = view.active_pane().current_shortcuts(&view.view_defs);
        entries.sort_by_key(|(k, _)| *k);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0], ('a', "child_alpha".to_string())); // child wins
        assert_eq!(entries[1], ('b', "child_bravo".to_string()));
        assert_eq!(entries[2], ('c', "view_charlie".to_string())); // inherited
    }

    /// Tiny `ContentAdapter` stub for the shortcut-hint tests below.
    /// Mirrors `MockAdapterBuilder::actions_for` (which lives in
    /// not-yet-done-content) but stays local — the tests need a
    /// `&dyn ContentAdapter` and the production stub requires more
    /// boilerplate than is justified here.
    struct ShortcutTestAdapter {
        actions: HashMap<String, Vec<NodeAction>>,
    }

    #[async_trait::async_trait]
    impl not_yet_done_content::ContentAdapter for ShortcutTestAdapter {
        fn adapter_type(&self) -> &str {
            "mock"
        }
        fn instance_id(&self) -> &str {
            "mock"
        }
        async fn root(&self) -> not_yet_done_content::Result<Box<dyn not_yet_done_content::Node>> {
            Err(not_yet_done_content::ContentError::NotSupported(
                "test stub".into(),
            ))
        }
        async fn get_by_id(
            &self,
            _id: &str,
        ) -> not_yet_done_content::Result<Box<dyn not_yet_done_content::Node>> {
            Err(not_yet_done_content::ContentError::NotSupported(
                "test stub".into(),
            ))
        }
        fn capabilities(&self) -> not_yet_done_content::AdapterCapabilities {
            Default::default()
        }
        fn actions_for_type(&self, nt: &not_yet_done_content::NodeType) -> Vec<NodeAction> {
            self.actions.get(&nt.type_id).cloned().unwrap_or_default()
        }
        fn childs<'a>(
            &'a self,
            _node: &'a dyn not_yet_done_content::Node,
        ) -> Vec<not_yet_done_content::Child<'a>> {
            // Stub: the shortcut-hint tests never list children.
            Vec::new()
        }
    }

    fn test_adapter(map: &[(&str, Vec<NodeAction>)]) -> ShortcutTestAdapter {
        let mut actions = HashMap::new();
        for (k, v) in map {
            actions.insert((*k).to_string(), v.clone());
        }
        ShortcutTestAdapter { actions }
    }

    #[test]
    fn collect_shortcut_hints_emits_adapter_action_label_and_source() {
        let mut sc = HashMap::new();
        sc.insert('a', "do_alpha".to_string());
        sc.insert('s', "do_sigma".to_string());
        let config = shortcut_config_flat(sc, HashMap::new());
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);

        let adapter = test_adapter(&[(
            "mock:issue",
            vec![
                // Editor input → activatable → carries a source (action bar).
                make_action("do_alpha", "Alpha", InputSpec::Editor),
                // No input, non-whitelisted id → no source (status bar).
                make_action("do_sigma", "Sigma", InputSpec::None),
                make_action("ignored", "Unused", InputSpec::Editor),
            ],
        )]);

        let mut hints = view
            .active_pane()
            .collect_shortcut_hints(&view.view_defs, Some(&adapter));
        hints.sort_by(|a, b| a.key.cmp(&b.key));
        assert_eq!(hints.len(), 2, "exactly the two configured shortcuts");
        assert_eq!(hints[0].key, "a");
        assert_eq!(hints[0].label, "Alpha");
        assert!(hints[0].source.is_some(), "editor action is activatable");
        assert_eq!(hints[1].key, "s");
        assert_eq!(hints[1].label, "Sigma");
        assert!(
            hints[1].source.is_none(),
            "fire-and-forget action has no source"
        );
    }

    #[test]
    fn collect_shortcut_hints_drops_when_action_id_not_in_adapter_set() {
        // Adapter returns a different action_id than the YAML shortcut
        // references — mirrors the "a:add on a leaf row whose
        // actions_for_type omits add" case.
        let mut sc = HashMap::new();
        sc.insert('a', "add".to_string());
        let config = shortcut_config_flat(sc, HashMap::new());
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);

        let adapter = test_adapter(&[(
            "mock:issue",
            vec![make_action("delete", "Delete", InputSpec::None)],
        )]);

        let hints = view
            .active_pane()
            .collect_shortcut_hints(&view.view_defs, Some(&adapter));
        assert!(
            hints.is_empty(),
            "missing action_id → hint dropped silently"
        );
    }

    #[test]
    fn collect_shortcut_hints_empty_without_adapter() {
        let mut sc = HashMap::new();
        sc.insert('a', "do_alpha".to_string());
        let config = shortcut_config_flat(sc, HashMap::new());
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);

        let hints = view
            .active_pane()
            .collect_shortcut_hints(&view.view_defs, None);
        assert!(hints.is_empty(), "no adapter → no hints");
    }

    #[test]
    fn collect_shortcut_hints_parent_target_uses_parent_node_type() {
        // 'p' on the child references the parent's action — resolver
        // must look it up under the parent's node_type, not the
        // currently-selected child's.
        let mut child_sc = HashMap::new();
        child_sc.insert('p', "parent:promote".to_string());
        let config = shortcut_config_flat(HashMap::new(), child_sc);
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);

        let child_def = config.views[0].children[0].clone();
        let view_defs = view.view_defs.clone();
        view.active_pane_mut()
            .drill_down_prepare("ISS-1", "First issue", &child_def, &view_defs);
        view.set_items(mock_comments(), Vec::new(), None, Vec::new(), None);

        // Adapter advertises `promote` for `mock:issue` (the parent's
        // type) but nothing for `mock:comment` (the selected child).
        // A correct lookup honours the `parent:` prefix and finds the
        // action under the parent type.
        let adapter = test_adapter(&[(
            "mock:issue",
            vec![make_action("promote", "Promote Parent", InputSpec::Editor)],
        )]);

        let hints = view
            .active_pane()
            .collect_shortcut_hints(&view.view_defs, Some(&adapter));
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].key, "p");
        assert_eq!(hints[0].label, "Promote Parent");
    }

    // ── CT-5: TreeFindState lifecycle ────────────────────────────────

    fn make_hit(path: &[&str], label: &str, space: &str) -> TreeFindHit {
        TreeFindHit {
            path: path.iter().map(|s| s.to_string()).collect(),
            label: label.to_string(),
            space_key: space.to_string(),
        }
    }

    fn empty_pane() -> ContentPane {
        ContentPane::new(
            test_theme(),
            0,
            true,
            not_yet_done_content::AdapterCapabilities::default(),
        )
    }

    #[test]
    fn tree_find_begin_seeds_loading_state() {
        let mut pane = empty_pane();
        assert!(!pane.tree_find_active(), "fresh pane has no tree-find");
        pane.tree_find_begin("design".to_string());
        let state = pane.tree_find.as_ref().expect("state set");
        assert_eq!(state.query, "design");
        assert!(state.loading);
        assert!(state.hits.is_empty());
        assert!(!state.truncated);
        assert_eq!(state.current, 0);
        assert!(pane.tree_find_active());
    }

    #[test]
    fn tree_find_complete_lands_hits_and_drops_loading() {
        let mut pane = empty_pane();
        pane.tree_find_begin("q".to_string());
        let hits = vec![
            make_hit(&["DEMO", "100", "200"], "Design", "DEMO"),
            make_hit(&["DEMO", "300"], "Other", "DEMO"),
        ];
        pane.tree_find_complete(hits, true);
        let state = pane.tree_find.as_ref().unwrap();
        assert!(!state.loading);
        assert_eq!(state.hits.len(), 2);
        assert_eq!(state.current, 0);
        assert!(state.truncated);
        // current_hit points at the first one.
        assert_eq!(pane.tree_find_current().unwrap().label, "Design");
    }

    #[test]
    fn tree_find_next_and_prev_wrap_around() {
        let mut pane = empty_pane();
        pane.tree_find_begin("q".to_string());
        pane.tree_find_complete(
            vec![
                make_hit(&["DEMO", "100"], "A", "DEMO"),
                make_hit(&["DEMO", "200"], "B", "DEMO"),
                make_hit(&["DEMO", "300"], "C", "DEMO"),
            ],
            false,
        );

        assert_eq!(pane.tree_find_next().unwrap().label, "B");
        assert_eq!(pane.tree_find_next().unwrap().label, "C");
        // Wrap forward.
        assert_eq!(pane.tree_find_next().unwrap().label, "A");
        // Wrap backward.
        assert_eq!(pane.tree_find_prev().unwrap().label, "C");
        assert_eq!(pane.tree_find_prev().unwrap().label, "B");
    }

    #[test]
    fn tree_find_next_on_empty_hits_returns_none() {
        let mut pane = empty_pane();
        pane.tree_find_begin("q".to_string());
        pane.tree_find_complete(Vec::new(), false);
        assert!(pane.tree_find_next().is_none());
        assert!(pane.tree_find_prev().is_none());
        assert!(pane.tree_find_current().is_none());
        // State is still active — the empty-hits hint sits in the
        // status bar until cleared.
        assert!(pane.tree_find_active());
    }

    #[test]
    fn tree_find_fail_clears_loading_keeps_query() {
        let mut pane = empty_pane();
        pane.tree_find_begin("design".to_string());
        pane.tree_find_fail();
        let state = pane.tree_find.as_ref().unwrap();
        assert!(!state.loading);
        assert!(state.hits.is_empty());
        assert_eq!(state.query, "design", "query stays for status-bar feedback");
    }

    // ── Column-config overrides (popup `c` on content tabs) ─────────

    #[test]
    fn apply_column_override_orders_filters_and_keeps_label() {
        let cols = vec![hcol("a"), hcol("b"), hcol("c")];
        // Reorder + hide.
        let out = apply_column_override(cols.clone(), &["c".into(), "a".into()], None);
        let keys: Vec<&str> = out.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(keys, ["c", "a"]);
        // Keys the config no longer knows (stale persisted override) are skipped.
        let out = apply_column_override(cols.clone(), &["gone".into(), "b".into()], None);
        let keys: Vec<&str> = out.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(keys, ["b"]);
        // The tree-label column survives an override that dropped it,
        // re-inserted at its configured position.
        let out = apply_column_override(cols, &["c".into()], Some("a"));
        let keys: Vec<&str> = out.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(keys, ["a", "c"]);
    }

    #[test]
    fn column_config_entries_and_apply_on_view_root() {
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);

        let (current, entries) = view
            .column_config_entries()
            .expect("root level configurable");
        assert_eq!(current, vec!["key".to_string(), "summary".to_string()]);
        assert_eq!(entries.len(), 2);
        assert!(
            entries.iter().all(|e| e.hideable),
            "flat levels have no fixed column"
        );
        assert_eq!(entries[0].display_name, "Key", "label wins as display name");
        assert_eq!(entries[1].display_name, "summary", "no label → key");

        // Hide `key` → override stored under the view-root level key and
        // applied by current_columns.
        assert!(view.apply_column_config(vec!["summary".into()]));
        assert_eq!(
            view.column_overrides().get("view:issues"),
            Some(&vec!["summary".to_string()]),
        );
        let view_defs = view.view_defs.clone();
        let cols = view.active_pane().current_columns(&view_defs);
        let keys: Vec<&str> = cols.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(keys, ["summary"]);
        // Re-opening the popup shows the overridden selection.
        let (current, _) = view.column_config_entries().unwrap();
        assert_eq!(current, vec!["summary".to_string()]);

        // Restoring the raw YAML layout removes the override entirely
        // (clean reset, nothing stale persists).
        assert!(view.apply_column_config(vec!["key".into(), "summary".into()]));
        assert!(view.column_overrides().is_empty());
        let cols = view.active_pane().current_columns(&view_defs);
        let keys: Vec<&str> = cols.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(keys, ["key", "summary"]);
    }

    #[test]
    fn hidden_column_excluded_by_default_but_offered_in_config() {
        let mut config = test_config_with_children();
        // Flag `summary` hidden: it must drop from the default layout but
        // still be offered (unchecked) in the `c` column-config popup.
        config.views[0].columns[1].hidden = true;
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);
        let view_defs = view.view_defs.clone();

        // Default layout omits the hidden column.
        let cols = view.active_pane().current_columns(&view_defs);
        let keys: Vec<&str> = cols.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(keys, ["key"], "hidden column not shown by default");

        // The popup still lists it; `current` pre-checks only the default set.
        let (current, entries) = view.column_config_entries().unwrap();
        assert_eq!(
            current,
            vec!["key".to_string()],
            "hidden column starts unchecked"
        );
        assert_eq!(entries.len(), 2, "hidden column still configurable");

        // Enabling it stores an override and shows it.
        assert!(view.apply_column_config(vec!["key".into(), "summary".into()]));
        assert_eq!(
            view.column_overrides().get("view:issues"),
            Some(&vec!["key".to_string(), "summary".to_string()]),
        );
        let cols = view.active_pane().current_columns(&view_defs);
        let keys: Vec<&str> = cols.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(keys, ["key", "summary"]);

        // Re-hiding it (back to the default visible set) clears the override
        // entirely rather than persisting an explicit "key only" override.
        assert!(view.apply_column_config(vec!["key".into()]));
        assert!(
            view.column_overrides().is_empty(),
            "default visible set removes the override"
        );
    }

    #[test]
    fn column_level_keys_distinguish_root_child_and_tree() {
        // Flat root vs drilled child get distinct keys, so each level
        // keeps its own layout.
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);
        let view_defs = view.view_defs.clone();
        assert_eq!(
            view.active_pane().column_level_key(&view_defs).as_deref(),
            Some("view:issues"),
        );
        let child_def = config.views[0].children[0].clone();
        view.active_pane_mut()
            .drill_down_prepare("ISS-1", "First issue", &child_def, &view_defs);
        assert_eq!(
            view.active_pane().column_level_key(&view_defs).as_deref(),
            Some("child:issues/Comments"),
        );

        // Tree mode keys off the cursor row's node_type_chain.
        let config = test_config_with_tree();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        let view_defs = view.view_defs.clone();
        assert_eq!(
            view.active_pane().column_level_key(&view_defs).as_deref(),
            Some("tree:databases/mock:db"),
        );
    }

    #[test]
    fn tree_column_owner_collapses_inherited_and_recursive_levels() {
        use crate::config::view_config::ViewFileConfig;
        // Root declares columns; the recursive child omits `columns:` and
        // inherits them. Every depth — including each recursion level, which
        // resolves to the one recursive ChildDef — must collapse onto the
        // root's single owning coordinate (prefix length 1).
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: tasks
    node_type: "task:item"
    tree_label: title
    columns:
      - { key: title, source: label }
      - { key: status }
    children:
      - name: subtasks
        node_type: "task:item"
        tree_label: title
        recursive: true
"#;
        let mut cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        cfg.inherit_tree_columns();
        let vd = &cfg.views[0];
        let cols = &vd.columns; // child inherited exactly this set
        let t = "task:item".to_string();
        assert_eq!(column_owner_chain_len(vd, &[t.clone()], cols), 1);
        assert_eq!(column_owner_chain_len(vd, &[t.clone(), t.clone()], cols), 1);
        assert_eq!(
            column_owner_chain_len(vd, &[t.clone(), t.clone(), t.clone()], cols),
            1
        );
    }

    #[test]
    fn tree_column_owner_keeps_diverging_level_independent() {
        use crate::config::view_config::ViewFileConfig;
        // The child DECLARES its own, different columns → the walk stops at
        // the divergence and the child keeps its own per-level key (prefix
        // length 2), so a deliberately-diverging tree stays configurable
        // per level.
        let yaml = r#"
tab: { name: T }
adapter: { type: x }
views:
  - name: tasks
    node_type: "root"
    tree_label: title
    columns:
      - { key: title, source: label }
      - { key: status }
    children:
      - name: subtasks
        node_type: "leaf"
        tree_label: title
        columns:
          - { key: title, source: label }
"#;
        let mut cfg: ViewFileConfig = serde_yaml::from_str(yaml).unwrap();
        cfg.inherit_tree_columns();
        let vd = &cfg.views[0];
        let child_cols = &vd.children[0].columns; // its own, shorter set
        let chain = vec!["root".to_string(), "leaf".to_string()];
        assert_eq!(column_owner_chain_len(vd, &chain, child_cols), 2);
    }

    #[test]
    fn tree_override_never_drops_label_column() {
        // A (stale/corrupt) override without the tree-label column still
        // renders it — the column carries the tree itself.
        let config = test_config_with_tree();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_dbs(), Vec::new(), None, Vec::new(), None);
        let mut overrides = std::collections::HashMap::new();
        overrides.insert("tree:databases/mock:db".to_string(), Vec::new());
        view.set_column_overrides(overrides);
        let view_defs = view.view_defs.clone();
        let cols = view.active_pane().current_columns(&view_defs);
        let keys: Vec<&str> = cols.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(
            keys,
            ["name"],
            "tree-label column re-inserted despite empty override"
        );
    }

    #[test]
    fn column_config_unavailable_on_auto_fallback_level() {
        // No YAML columns → schema is derived from item metadata
        // (postgres rows); there is nothing stable to configure.
        let mut config = test_config_with_children();
        config.views[0].columns = Vec::new();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);
        assert!(view.column_config_entries().is_none());
        assert!(!view.apply_column_config(vec!["key".into()]));
    }

    #[test]
    fn split_pane_inherits_column_overrides() {
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(mock_issues(), Vec::new(), None, Vec::new(), None);
        assert!(view.apply_column_config(vec!["summary".into()]));

        view.split_focused(SplitOrientation::Horizontal);
        let view_defs = view.view_defs.clone();
        // The freshly split pane (now focused) sees the same override.
        let cols = view.active_pane().current_columns(&view_defs);
        let keys: Vec<&str> = cols.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(keys, ["summary"], "new split pane mirrors the override map");
    }

    #[test]
    fn tree_find_clear_drops_state_entirely() {
        let mut pane = empty_pane();
        pane.tree_find_begin("q".to_string());
        pane.tree_find_complete(vec![make_hit(&["X"], "X", "X")], false);
        pane.tree_find_clear();
        assert!(!pane.tree_find_active());
        assert!(pane.tree_find.is_none());
    }

    #[test]
    fn tree_find_complete_after_clear_is_noop() {
        // Mirrors CT-9: user hit Esc / reloaded between the spawn and
        // the response. The late response must not resurrect state.
        let mut pane = empty_pane();
        pane.tree_find_begin("q".to_string());
        pane.tree_find_clear();
        pane.tree_find_complete(vec![make_hit(&["X"], "X", "X")], false);
        assert!(!pane.tree_find_active());
    }

    // ── CT-7: advance_tree_find walker ───────────────────────────────

    fn mock_space_nt() -> not_yet_done_content::NodeType {
        not_yet_done_content::NodeType {
            type_id: "mock:space".into(),
            mime_type: "".into(),
            syntax: None,
            file_extension: "".into(),
            display_name: "Space".into(),
        }
    }

    fn mock_page_nt() -> not_yet_done_content::NodeType {
        not_yet_done_content::NodeType {
            type_id: "mock:page".into(),
            mime_type: "".into(),
            syntax: None,
            file_extension: "".into(),
            display_name: "Page".into(),
        }
    }

    /// Regression (tasks adapter): Enter on a depth-1 node in a *uniform*
    /// recursive tree (`mock:task`/`mock:task`, root type == child type) must
    /// request its children, and they must then render at depth 2. The
    /// pre-existing `uniform_recursive_*` tests only ever went root→child
    /// (cursor on the root), so the deeper-than-one expansion path was
    /// untested — that's where "sub-levels past the 2nd don't unfold" lived.
    #[test]
    fn uniform_recursive_tree_expands_below_depth_one() {
        let config = uniform_recursive_config();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(
            vec![tnode_val("work", "Work", "W")],
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        let view_defs = view.view_defs.clone();
        let pane_id = view.active_pane_id();
        let view_index = view.view_index;

        // Drive the whole chain through `try_tree_open` (the real Enter
        // path) at each depth, feeding the adapter response back via
        // `apply_tree_children`, exactly like the live LoadMsg loop.
        let expand = |view: &mut ContentView, row: usize, expect_parent: &str, child_id: &str| {
            view.active_pane_mut().table.set_selected(row);
            let msg = view
                .active_pane_mut()
                .try_tree_open(view_index, pane_id, &view_defs);
            match msg {
                Some(SubViewMessage::Request(ViewRequest::ExpandTreeNode {
                    child_node_type,
                    parent_node_id,
                    parent_path,
                    ..
                })) => {
                    assert_eq!(child_node_type, "mock:task");
                    assert_eq!(parent_node_id, expect_parent, "expanding {expect_parent}");
                    view.apply_tree_children(
                        pane_id,
                        parent_path,
                        vec![tnode_val(child_id, child_id, "V")],
                        None,
                        false,
                        "mock:task".into(),
                    );
                }
                other => {
                    panic!("Enter on `{expect_parent}` must yield ExpandTreeNode, got {other:?}")
                }
            }
        };

        // depth 0 → 1 (Work → h): known to work.
        expand(&mut view, 0, "work", "h");
        // depth 1 → 2 (h → g): the regressed level ("nach der 2. Ebene").
        expand(&mut view, 1, "h", "g");
        // depth 2 → 3 (g → gg): self-similar tree keeps going.
        expand(&mut view, 2, "g", "gg");

        // All four levels present in the entry model …
        let depths: Vec<(String, usize)> = view
            .active_pane()
            .tree
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .map(|e| (e.node.id.clone(), e.depth))
            .collect();
        assert!(
            depths.contains(&("g".to_string(), 2)),
            "grandchild at depth 2: {depths:?}"
        );
        assert!(
            depths.contains(&("gg".to_string(), 3)),
            "great-grandchild at depth 3: {depths:?}"
        );

        // … and they actually RENDER as rows (not blanked out).
        let pane = view.active_pane();
        let columns = pane.current_columns(&view_defs);
        let rows =
            pane.build_tree_data_rows(&columns, &view_defs, chrono::Local::now(), false, None);
        assert_eq!(rows.len(), 4, "work + h + g + gg all render");
    }

    #[test]
    fn cursor_can_open_recursive_node_below_root() {
        // Regression: the `Open`/Enter key claim gates on `cursor_can_open`.
        // `current_children` returns the *raw* declared children of the
        // cursor's ChildDef — empty for a recursive node (its only child is
        // itself, added by `effective_child_children`). The earlier gate on
        // `current_children` left Enter unbound on every node below the root:
        // the expand glyph showed but pressing Return did nothing.
        let config = uniform_recursive_config();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(
            vec![tnode_val("work", "Work", "W")],
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        let view_defs = view.view_defs.clone();
        let pane_id = view.active_pane_id();
        let view_index = view.view_index;

        // Root can open.
        view.active_pane_mut().table.set_selected(0);
        assert!(view.active_pane().cursor_can_open(&view_defs), "root opens");

        // Expand Work → child h, then put the cursor on h (depth 1).
        if let Some(SubViewMessage::Request(ViewRequest::ExpandTreeNode { parent_path, .. })) = view
            .active_pane_mut()
            .try_tree_open(view_index, pane_id, &view_defs)
        {
            view.apply_tree_children(
                pane_id,
                parent_path,
                vec![tnode_val("h", "h", "V")],
                None,
                false,
                "mock:task".into(),
            );
        } else {
            panic!("Work must expand");
        }
        view.active_pane_mut().table.set_selected(1);
        assert!(
            view.active_pane().cursor_can_open(&view_defs),
            "recursive node below the root must be openable (the regressed gate)"
        );
    }

    #[test]
    fn expanded_refresh_requests_refetch_expanded_loaded_paths_only() {
        // After a root reload only the depth-0 rows are fresh; the
        // refresh pass re-fetches every *expanded* node's children (so
        // e.g. a tracking-marker change shows on nested rows too) while
        // leaving collapsed siblings and in-flight loads alone.
        let config = uniform_recursive_config();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(
            vec![
                tnode_val("work", "Work", "W"),
                tnode_val("priv", "Priv", "P"),
            ],
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        let pane_id = view.active_pane_id();
        let view_index = view.view_index;

        // Expand work → h → (children loaded for both); `priv` stays
        // collapsed.
        {
            let tree = view.active_pane_mut().tree.as_mut().unwrap();
            tree.expanded.insert(vec!["work".to_string()]);
            tree.expanded
                .insert(vec!["work".to_string(), "h".to_string()]);
        }
        view.apply_tree_children(
            pane_id,
            vec!["work".to_string()],
            vec![tnode_val("h", "h", "V")],
            None,
            false,
            "mock:task".into(),
        );
        view.apply_tree_children(
            pane_id,
            vec!["work".to_string(), "h".to_string()],
            vec![tnode_val("g", "g", "V")],
            None,
            false,
            "mock:task".into(),
        );

        // A reload lands: depth-0 replaced, deeper caches untouched.
        view.set_items(
            vec![
                tnode_val("work", "Work", "W2"),
                tnode_val("priv", "Priv", "P"),
            ],
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        let reqs = view.pending_expanded_refresh_requests(view_index, pane_id);
        let parents: Vec<&str> = reqs
            .iter()
            .map(|r| match r {
                ViewRequest::ExpandTreeNode { parent_node_id, .. } => parent_node_id.as_str(),
                other => panic!("expected ExpandTreeNode, got {other:?}"),
            })
            .collect();
        assert_eq!(
            parents,
            vec!["work", "h"],
            "every expanded path refreshes: {reqs:?}"
        );

        // An expanded path whose children are still in flight (cache not
        // `loaded`) is skipped — its landing already brings fresh data.
        view.active_pane_mut()
            .tree
            .as_mut()
            .unwrap()
            .cache
            .get_mut(&vec!["work".to_string(), "h".to_string()])
            .unwrap()
            .loaded = false;
        let reqs = view.pending_expanded_refresh_requests(view_index, pane_id);
        assert_eq!(reqs.len(), 1, "in-flight path skipped: {reqs:?}");
    }

    #[test]
    fn selected_item_resolves_nested_tree_rows() {
        // `selected_item` must read the summary off the tree entry —
        // `items` only holds the depth-0 rows, so an id lookup there
        // misses every nested node (the `:script` "No row selected" bug).
        let config = uniform_recursive_config();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(
            vec![tnode_val("work", "Work", "W")],
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        let pane_id = view.active_pane_id();
        view.active_pane_mut()
            .tree
            .as_mut()
            .unwrap()
            .expanded
            .insert(vec!["work".to_string()]);
        view.apply_tree_children(
            pane_id,
            vec!["work".to_string()],
            vec![tnode_val("h", "Nested", "V")],
            None,
            false,
            "mock:task".into(),
        );

        assert!(view.active_pane_mut().focus_item_by_id("h"));
        let pane = view.find_pane(pane_id).unwrap();
        let item = pane
            .selected_item()
            .expect("nested row resolves to its summary");
        assert_eq!(item.id, "h");
        assert_eq!(item.label, "Nested");
        assert!(
            pane.items.iter().all(|n| n.id != "h"),
            "nested summary is absent from items — an items lookup would miss it"
        );
    }

    #[test]
    fn expand_depth_auto_expands_to_configured_depth_then_disarms() {
        // `expand_depth: 2` → depth-0 and depth-1 rows auto-expand after
        // load (three levels visible), then the one-shot cascade disarms
        // so it never overrides manual collapse afterwards.
        let mut config = uniform_recursive_config();
        config.views[0].expand_depth = Some(ExpandDepth::Levels(2));
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(
            vec![tnode_val("work", "Work", "W")],
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        let pane_id = view.active_pane_id();
        let view_index = view.view_index;

        // Pass 1 (root rows landed): depth-0 `work` auto-expands.
        let reqs = view.pending_auto_expand_requests(view_index, pane_id);
        assert_eq!(reqs.len(), 1, "one depth-0 expand request: {reqs:?}");
        let ViewRequest::ExpandTreeNode {
            parent_path,
            parent_node_id,
            ..
        } = &reqs[0]
        else {
            panic!("expected ExpandTreeNode, got {:?}", reqs[0]);
        };
        assert_eq!(parent_node_id, "work");
        view.apply_tree_children(
            pane_id,
            parent_path.clone(),
            vec![tnode_val("h", "h", "V")],
            None,
            false,
            "mock:task".into(),
        );

        // Pass 2 (depth-1 children landed): `h` (depth 1 < 2) expands too.
        let reqs = view.pending_auto_expand_requests(view_index, pane_id);
        assert_eq!(reqs.len(), 1, "one depth-1 expand request: {reqs:?}");
        let ViewRequest::ExpandTreeNode {
            parent_path,
            parent_node_id,
            ..
        } = &reqs[0]
        else {
            panic!("expected ExpandTreeNode, got {:?}", reqs[0]);
        };
        assert_eq!(parent_node_id, "h");
        view.apply_tree_children(
            pane_id,
            parent_path.clone(),
            vec![tnode_val("g", "g", "V")],
            None,
            false,
            "mock:task".into(),
        );

        // Pass 3: `g` sits AT the target depth → nothing further, disarm.
        let reqs = view.pending_auto_expand_requests(view_index, pane_id);
        assert!(reqs.is_empty(), "cascade complete: {reqs:?}");
        assert!(
            !view
                .active_pane()
                .tree
                .as_ref()
                .unwrap()
                .auto_expand_pending,
            "cascade must disarm once nothing is left to load"
        );
        let depths: Vec<(String, usize)> = view
            .active_pane()
            .tree
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .map(|e| (e.node.id.clone(), e.depth))
            .collect();
        assert!(depths.contains(&("h".to_string(), 1)), "{depths:?}");
        assert!(depths.contains(&("g".to_string(), 2)), "{depths:?}");

        // Disarmed: a manual collapse survives later data landings.
        view.active_pane_mut()
            .tree
            .as_mut()
            .unwrap()
            .expanded
            .remove(&vec!["work".to_string()]);
        let reqs = view.pending_auto_expand_requests(view_index, pane_id);
        assert!(reqs.is_empty(), "disarmed cascade must not re-expand");
        assert!(
            !view
                .active_pane()
                .tree
                .as_ref()
                .unwrap()
                .expanded
                .contains(&vec!["work".to_string()]),
            "manual collapse stays collapsed"
        );
    }

    #[test]
    fn expand_depth_all_expands_until_nothing_is_left() {
        // `expand_depth: all` → no depth ceiling; the cascade keeps
        // requesting children level by level and only disarms once a
        // pass finds nothing expandable (here: a leaf level lands).
        let mut config = uniform_recursive_config();
        config.views[0].expand_depth = Some(ExpandDepth::All);
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(
            vec![tnode_val("work", "Work", "W")],
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        let pane_id = view.active_pane_id();
        let view_index = view.view_index;

        // Walk three levels deep — each landing triggers the next request.
        for (parent, child) in [("work", "h"), ("h", "g"), ("g", "leaf")] {
            let reqs = view.pending_auto_expand_requests(view_index, pane_id);
            assert_eq!(reqs.len(), 1, "expand request for `{parent}`: {reqs:?}");
            let ViewRequest::ExpandTreeNode {
                parent_path,
                parent_node_id,
                ..
            } = &reqs[0]
            else {
                panic!("expected ExpandTreeNode, got {:?}", reqs[0]);
            };
            assert_eq!(parent_node_id, parent);
            view.apply_tree_children(
                pane_id,
                parent_path.clone(),
                vec![tnode_val(child, child, "V")],
                None,
                false,
                "mock:task".into(),
            );
        }

        // `leaf` expands too (depth 3 — beyond any fixed default), comes
        // back empty → the next pass has nothing left and disarms.
        let reqs = view.pending_auto_expand_requests(view_index, pane_id);
        assert_eq!(reqs.len(), 1, "leaf still probed under `all`: {reqs:?}");
        let ViewRequest::ExpandTreeNode {
            parent_path,
            parent_node_id,
            ..
        } = &reqs[0]
        else {
            panic!("expected ExpandTreeNode, got {:?}", reqs[0]);
        };
        assert_eq!(parent_node_id, "leaf");
        view.apply_tree_children(
            pane_id,
            parent_path.clone(),
            Vec::new(),
            None,
            false,
            "mock:task".into(),
        );
        let reqs = view.pending_auto_expand_requests(view_index, pane_id);
        assert!(reqs.is_empty(), "cascade complete: {reqs:?}");
        assert!(
            !view
                .active_pane()
                .tree
                .as_ref()
                .unwrap()
                .auto_expand_pending,
            "cascade must disarm once a pass finds nothing expandable"
        );
        let depths: Vec<(String, usize)> = view
            .active_pane()
            .tree
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .map(|e| (e.node.id.clone(), e.depth))
            .collect();
        assert!(depths.contains(&("leaf".to_string(), 3)), "{depths:?}");
    }

    #[test]
    fn tree_collapse_all_retains_paths_up_to_expand_depth() {
        // `zm` on a view with `expand_depth: 2` must fold back to the
        // initial depth (depth-0 + depth-1 expanded → three visible
        // levels), NOT all the way to the root. Deeper manual expansions
        // are dropped; `own_path.len() <= 2` survives.
        let mut config = uniform_recursive_config();
        config.views[0].expand_depth = Some(ExpandDepth::Levels(2));
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(
            vec![tnode_val("work", "Work", "W")],
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        let view_defs = view.view_defs.clone();

        // Hand-build a four-level tree fully expanded down to `leaf`
        // (deeper than `expand_depth`).
        {
            let pane = view.active_pane_mut();
            let tree = pane.tree.as_mut().expect("tree mode");
            tree.set_cached_children(vec!["work".into()], vec![tnode_val("h", "h", "V")], None);
            tree.set_cached_children(
                vec!["work".into(), "h".into()],
                vec![tnode_val("g", "g", "V")],
                None,
            );
            tree.set_cached_children(
                vec!["work".into(), "h".into(), "g".into()],
                vec![tnode_val("leaf", "leaf", "V")],
                None,
            );
            tree.expanded.insert(vec!["work".into()]);
            tree.expanded.insert(vec!["work".into(), "h".into()]);
            tree.expanded
                .insert(vec!["work".into(), "h".into(), "g".into()]);
            tree.rebuild_entries(&view_defs[0]);
        }
        view.active_pane_mut().rebuild_table(&view_defs);

        match view.active_pane_mut().try_tree_collapse_all(&view_defs) {
            Some(SubViewMessage::SelectionChanged(_)) => {}
            other => panic!("expected SelectionChanged, got {other:?}"),
        }
        let tree = view.active_pane().tree.as_ref().unwrap();
        assert!(
            tree.expanded.contains(&vec!["work".to_string()]),
            "depth-0 stays expanded: {:?}",
            tree.expanded
        );
        assert!(
            tree.expanded
                .contains(&vec!["work".to_string(), "h".to_string()]),
            "depth-1 stays expanded: {:?}",
            tree.expanded
        );
        assert!(
            !tree
                .expanded
                .contains(&vec!["work".to_string(), "h".to_string(), "g".to_string()]),
            "depth-2 folded away: {:?}",
            tree.expanded
        );
        assert!(
            tree.cache
                .contains_key(&vec!["work".to_string(), "h".to_string(), "g".to_string()]),
            "cached children kept for a cheap re-expand",
        );
    }

    #[test]
    fn tree_expand_all_arms_unbounded_cascade_past_expand_depth() {
        // `zr` on a view with `expand_depth: 2` must blow past the initial
        // ceiling: after the natural cascade settles at depth 2, expand-all
        // re-arms with an unbounded target and the node sitting AT the old
        // ceiling (`g`, depth 2) now gets an expand request.
        let mut config = uniform_recursive_config();
        config.views[0].expand_depth = Some(ExpandDepth::Levels(2));
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(
            vec![tnode_val("work", "Work", "W")],
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        let pane_id = view.active_pane_id();
        let view_index = view.view_index;
        let view_defs = view.view_defs.clone();

        // Drive the natural `expand_depth: 2` cascade to its ceiling.
        for (parent, child) in [("work", "h"), ("h", "g")] {
            let reqs = view.pending_auto_expand_requests(view_index, pane_id);
            let ViewRequest::ExpandTreeNode {
                parent_path,
                parent_node_id,
                ..
            } = &reqs[0]
            else {
                panic!("expected ExpandTreeNode, got {:?}", reqs[0]);
            };
            assert_eq!(parent_node_id, parent);
            view.apply_tree_children(
                pane_id,
                parent_path.clone(),
                vec![tnode_val(child, child, "V")],
                None,
                false,
                "mock:task".into(),
            );
        }
        // Ceiling reached: `g` (depth 2) is NOT expanded and the cascade is disarmed.
        assert!(
            view.pending_auto_expand_requests(view_index, pane_id)
                .is_empty()
        );
        assert!(
            !view
                .active_pane()
                .tree
                .as_ref()
                .unwrap()
                .auto_expand_pending
        );

        // `zr`: arm expand-all and ask the App to drive the cascade.
        match view
            .active_pane_mut()
            .try_tree_expand_all(view_index, pane_id, &view_defs)
        {
            Some(SubViewMessage::Request(ViewRequest::DriveTreeAutoExpand {
                view_index: vi,
                pane_id: pid,
            })) => {
                assert_eq!(vi, view_index);
                assert_eq!(pid, pane_id);
            }
            other => panic!("expected DriveTreeAutoExpand request, got {other:?}"),
        }
        {
            let tree = view.active_pane().tree.as_ref().unwrap();
            assert!(tree.expand_all_armed, "expand-all override raised");
            assert!(tree.auto_expand_pending, "cascade re-armed");
        }

        // Pump: `g`, which sat at the old depth-2 ceiling, now expands.
        let reqs = view.pending_auto_expand_requests(view_index, pane_id);
        let ViewRequest::ExpandTreeNode { parent_node_id, .. } = &reqs[0] else {
            panic!("expected ExpandTreeNode, got {:?}", reqs[0]);
        };
        assert_eq!(
            parent_node_id, "g",
            "expand-all blew past the configured expand_depth"
        );
    }

    /// Mirror trackings.yaml `tree`: a heterogeneous group-bucket root
    /// (`tracking:tree-group`) over a recursive task forest
    /// (`tracking:tree-item`/`tracking:tree-item`), with `group_headers`
    /// and `expand_depth: all`. Differs from `uniform_recursive_config`
    /// in that the ROOT type is NOT the recursive child type.
    fn grouped_recursive_tree_config() -> ViewFileConfig {
        let mut sub = hchild(
            "subtasks",
            "tracking:tree-item",
            Some("name"),
            vec![hcol("name"), dcol("val")],
            vec![],
        );
        sub.recursive = true;
        let mut cfg = uniform_recursive_config();
        cfg.views[0].node_type = "tracking:tree-group".into();
        cfg.views[0].children = vec![sub];
        cfg.views[0].expand_depth = Some(ExpandDepth::All);
        cfg.views[0].group_headers = Some(GroupHeadersDef::default());
        cfg
    }

    /// A tree node with an explicit `has_children` flag (the trackings
    /// adapter always sets it, unlike the `tnode_val` default of `None`).
    fn hc_node(id: &str, ty: &str, has_children: bool) -> NodeSummary {
        let mut n = tnode_val(id, id, "V");
        n.node_type.type_id = ty.into();
        n.has_children = Some(has_children);
        n
    }

    #[test]
    fn grouped_recursive_tree_cascade_expands_below_depth_two() {
        // Repro for "Trackings tree only opens the top two levels":
        // bucket(0) → t1(1) → t2(2) → t3(3, leaf). The cascade must keep
        // descending past depth 2 exactly like the uniform tree does.
        let config = grouped_recursive_tree_config();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(
            vec![hc_node(
                "treegrp:started:day:2026-06-09",
                "tracking:tree-group",
                true,
            )],
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        let pane_id = view.active_pane_id();
        let view_index = view.view_index;

        let chain = [
            ("treegrp:started:day:2026-06-09", "tree:s:t1", true),
            ("tree:s:t1", "tree:s:t2", true),
            ("tree:s:t2", "tree:s:t3", false),
        ];
        for (parent, child, child_has) in chain {
            let reqs = view.pending_auto_expand_requests(view_index, pane_id);
            assert_eq!(reqs.len(), 1, "expand request for `{parent}`: {reqs:?}");
            let ViewRequest::ExpandTreeNode {
                parent_path,
                parent_node_id,
                child_node_type,
                ..
            } = &reqs[0]
            else {
                panic!("expected ExpandTreeNode, got {:?}", reqs[0]);
            };
            assert_eq!(parent_node_id, parent, "cascade expanding {parent}");
            assert_eq!(child_node_type, "tracking:tree-item");
            view.apply_tree_children(
                pane_id,
                parent_path.clone(),
                vec![hc_node(child, "tracking:tree-item", child_has)],
                None,
                false,
                "tracking:tree-item".into(),
            );
        }

        let depths: Vec<(String, usize)> = view
            .active_pane()
            .tree
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .map(|e| (e.node.id.clone(), e.depth))
            .collect();
        assert!(
            depths.contains(&("tree:s:t2".to_string(), 2)),
            "depth-2 task: {depths:?}"
        );
        assert!(
            depths.contains(&("tree:s:t3".to_string(), 3)),
            "depth-3 task: {depths:?}"
        );

        // … and they RENDER (group_headers maps the bucket to a header row;
        // the three task rows must all survive into the table).
        let view_defs = view.view_defs.clone();
        let pane = view.active_pane();
        let columns = pane.current_columns(&view_defs);
        let rows =
            pane.build_tree_data_rows(&columns, &view_defs, chrono::Local::now(), false, None);
        assert_eq!(
            rows.len(),
            4,
            "bucket header + t1 + t2 + t3 all render: {} rows",
            rows.len()
        );
    }

    #[test]
    fn cascade_stays_armed_while_a_sibling_branch_is_still_in_flight() {
        // Live repro for "Trackings tree only opens the top two levels":
        // the `expand_depth: all` cascade is pumped once per async
        // `TreeChildren` arrival (drive_tree_auto_expand). With two sibling
        // branches expanding concurrently, one branch can bottom out (a leaf
        // lands) WHILE the other is still in flight. The pump for that leaf
        // finds no new candidate that round — but it must NOT disarm: the
        // in-flight sibling will reveal deeper levels once its children land.
        //
        //   root → t1 → t1a → L1(leaf)
        //        → t2 → t2a → t2b → L2(leaf)
        let mut config = uniform_recursive_config();
        config.views[0].expand_depth = Some(ExpandDepth::All);
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(
            vec![hc_node("root", "mock:task", true)],
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        let pane_id = view.active_pane_id();
        let view_index = view.view_index;

        // The live loop calls pending_auto_expand_requests once per arrival,
        // then dispatches the children of exactly one expand request.
        let pump = |view: &mut ContentView, parent_path: Vec<String>, kids: Vec<NodeSummary>| {
            let _ = view.pending_auto_expand_requests(view_index, pane_id);
            view.apply_tree_children(pane_id, parent_path, kids, None, false, "mock:task".into());
        };

        // root → [t1, t2]
        pump(
            &mut view,
            vec!["root".into()],
            vec![
                hc_node("t1", "mock:task", true),
                hc_node("t2", "mock:task", true),
            ],
        );
        // t1 → [t1a]
        pump(
            &mut view,
            vec!["root".into(), "t1".into()],
            vec![hc_node("t1a", "mock:task", true)],
        );
        // t2 → [t2a]   (t1a now queued for expansion, t2a about to be)
        pump(
            &mut view,
            vec!["root".into(), "t2".into()],
            vec![hc_node("t2a", "mock:task", true)],
        );
        // t1a → [L1 leaf]   ← this pump finds no NEW candidate (t2a is
        // expanded-but-in-flight, L1 is a leaf). The cascade must stay armed.
        pump(
            &mut view,
            vec!["root".into(), "t1".into(), "t1a".into()],
            vec![hc_node("L1", "mock:task", false)],
        );

        assert!(
            view.active_pane()
                .tree
                .as_ref()
                .unwrap()
                .auto_expand_pending,
            "cascade disarmed while the t2 branch is still in flight — deeper \
             levels of t2 will never auto-expand",
        );

        // t2a → [t2b]   if still armed, the next pump expands t2b.
        pump(
            &mut view,
            vec!["root".into(), "t2".into(), "t2a".into()],
            vec![hc_node("t2b", "mock:task", true)],
        );
        let reqs = view.pending_auto_expand_requests(view_index, pane_id);
        assert!(
            reqs.iter().any(|r| matches!(r, ViewRequest::ExpandTreeNode { parent_node_id, .. } if parent_node_id == "t2b")),
            "t2b must still auto-expand: {reqs:?}",
        );
    }

    #[test]
    fn no_expand_depth_means_no_auto_expansion() {
        let config = uniform_recursive_config();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.set_items(
            vec![tnode_val("work", "Work", "W")],
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        let pane_id = view.active_pane_id();
        let view_index = view.view_index;
        let reqs = view.pending_auto_expand_requests(view_index, pane_id);
        assert!(reqs.is_empty(), "no expand_depth → no requests");
        let tree_state = view.active_pane().tree.as_ref().unwrap();
        assert!(tree_state.expanded.is_empty(), "tree stays collapsed");
        assert!(!tree_state.auto_expand_pending, "disarmed immediately");
    }

    fn tree_view_def() -> ViewDef {
        // Minimal tree-enabled view: root → recursive page child.
        // Mirrors confluence.yaml's spaces tree (one space root that
        // recurses through pages).
        ViewDef {
            card: None,
            row_layout: None,
            smooth_scroll: false,
            name: "tree-view".into(),
            node_type: "mock:space".into(),
            default: true,
            window_ops: false,
            key: None,
            query: None,
            columns: vec![ColumnDef {
                key: "label".into(),
                label: Some("Label".into()),
                source: Some("label".into()),
                style: None,
                sizing: "max".into(),
                markdown: false,
                kind: ColumnKind::Text,
                format: None,
                separator: None,
                elapsed_from: None,
                tree_aggregate: None,
                hidden: false,
                collapsed_source: None,
                long_source: None,
            }],
            preview: None,
            actions: vec![],
            children: vec![ChildDef {
                card: None,
                row_layout: None,
                smooth_scroll: false,
                name: "pages".into(),
                node_type: "mock:page".into(),
                columns: vec![ColumnDef {
                    key: "label".into(),
                    label: Some("Page".into()),
                    source: Some("label".into()),
                    style: None,
                    sizing: "max".into(),
                    markdown: false,
                    kind: ColumnKind::Text,
                    format: None,
                    separator: None,
                    elapsed_from: None,
                    tree_aggregate: None,
                    hidden: false,
                    collapsed_source: None,
                    long_source: None,
                }],
                preview: None,
                actions: vec![],
                children: vec![],
                split: None,
                pagination: None,
                keybindings: HashMap::new(),
                action_chains: Default::default(),
                column_cursor: false,
                record_detail: false,
                node_scripts: false,
                tree_label: Some("label".into()),
                shortcuts: HashMap::new(),
                enter_action: None,
                recursive: true,
                editor_in_place: false,
                leaf_glyph: None,
                icon: None,
                group_by: None,
                aggregates: Vec::new(),
                mark_read_on_reach_end: None,
                cursor_on_open: None,
            }],
            pagination: None,
            action_chains: Default::default(),
            column_cursor: false,
            record_detail: false,
            node_scripts: false,
            tree_label: Some("label".into()),
            retries: 0,
            script_template: None,
            script_source: None,
            shortcuts: HashMap::new(),
            leaf_glyph: None,
            icon: None,
            group_by: None,
            aggregates: Vec::new(),
            tree_connector_style: None,
            unread_style: None,
            unread_marker: None,
            tree_lines: None,
            tree_markers: None,
            expand_depth: None,
            group_headers: None,
            event_actions: Vec::new(),
        }
    }

    fn space_node(id: &str, label: &str) -> NodeSummary {
        NodeSummary {
            id: id.into(),
            label: label.into(),
            node_type: mock_space_nt(),
            metadata: not_yet_done_content::Metadata::default(),
            has_children: None,
        }
    }

    fn page_node(id: &str, label: &str) -> NodeSummary {
        NodeSummary {
            id: id.into(),
            label: label.into(),
            node_type: mock_page_nt(),
            metadata: not_yet_done_content::Metadata::default(),
            has_children: None,
        }
    }

    #[test]
    fn advance_tree_find_returns_idle_without_state() {
        let mut pane = empty_pane();
        let vds = vec![tree_view_def()];
        match pane.advance_tree_find(0, 0, &vds) {
            TreeFindAdvance::Idle => {}
            other => panic!("expected Idle, got {other:?}"),
        }
    }

    #[test]
    fn advance_tree_find_needs_root_load_when_cache_empty() {
        let mut pane = empty_pane();
        let vds = vec![tree_view_def()];
        pane.tree_find_begin("q".into());
        pane.tree_find_complete(vec![make_hit(&["SPACE", "p1"], "P1", "SPACE")], false);
        match pane.advance_tree_find(0, 0, &vds) {
            TreeFindAdvance::NeedRootLoad => {}
            other => panic!("expected NeedRootLoad, got {other:?}"),
        }
    }

    #[test]
    fn advance_tree_find_needs_tree_expand_after_root_loaded() {
        let mut pane = empty_pane();
        let vds = vec![tree_view_def()];
        pane.tree_find_begin("q".into());
        pane.tree_find_complete(vec![make_hit(&["SPACE", "p1"], "P1", "SPACE")], false);
        // Land the root (depth 0) but not the page level.
        pane.tree.as_mut().unwrap().set_cached_children(
            Vec::new(),
            vec![space_node("SPACE", "Space")],
            None,
        );
        match pane.advance_tree_find(0, 0, &vds) {
            TreeFindAdvance::NeedTreeExpand {
                parent_path,
                parent_node_id,
                child_node_type,
                ..
            } => {
                assert_eq!(parent_path, vec!["SPACE".to_string()]);
                assert_eq!(parent_node_id, "SPACE");
                assert_eq!(child_node_type, "mock:page");
            }
            other => panic!("expected NeedTreeExpand, got {other:?}"),
        }
    }

    #[test]
    fn advance_tree_find_refreshes_once_then_reports_not_in_tree_when_ancestor_missing() {
        let mut pane = empty_pane();
        let vds = vec![tree_view_def()];
        pane.tree_find_begin("q".into());
        // Hit references SPACE but the loaded root has only OTHER.
        pane.tree_find_complete(vec![make_hit(&["SPACE", "p1"], "P1", "SPACE")], false);
        pane.tree.as_mut().unwrap().set_cached_children(
            Vec::new(),
            vec![space_node("OTHER", "Other")],
            None,
        );
        // A loaded-but-stale level is re-fetched once (it may be missing the
        // hit only because the cache predates a cross-process insert).
        match pane.advance_tree_find(0, 0, &vds) {
            TreeFindAdvance::NeedRootLoad => {}
            other => panic!("expected NeedRootLoad (refresh), got {other:?}"),
        }
        // Once that refresh budget is spent, a still-missing ancestor is
        // reported as genuinely absent instead of looping forever.
        match pane.advance_tree_find(0, 0, &vds) {
            TreeFindAdvance::NotInTree(_) => {}
            other => panic!("expected NotInTree after refresh, got {other:?}"),
        }
    }

    #[test]
    fn advance_tree_find_ready_when_all_levels_cached() {
        let mut pane = empty_pane();
        let vds = vec![tree_view_def()];
        pane.tree_find_begin("q".into());
        pane.tree_find_complete(vec![make_hit(&["SPACE", "p1"], "P1", "SPACE")], false);
        // Land both levels. The space is at depth 0, then its
        // children at parent_path=[SPACE] are the pages.
        let tree = pane.tree.as_mut().unwrap();
        tree.set_cached_children(Vec::new(), vec![space_node("SPACE", "Space")], None);
        tree.set_cached_children(vec!["SPACE".into()], vec![page_node("p1", "P1")], None);
        match pane.advance_tree_find(0, 0, &vds) {
            TreeFindAdvance::Ready(_row) => {
                // SPACE prefix is marked expanded so the page surfaces.
                assert!(
                    pane.tree
                        .as_ref()
                        .unwrap()
                        .expanded
                        .contains(&vec!["SPACE".to_string()])
                );
            }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn advance_tree_find_idle_after_ready_until_next_or_prev() {
        // Regression: after the walker lands the cursor on the hit, a
        // subsequent TreeChildren (e.g. user expanded a node by hand
        // via Enter) re-ran the walker and snapped the cursor back to
        // the hit. `settled` blocks that re-run; n/N re-arms.
        let mut pane = empty_pane();
        let vds = vec![tree_view_def()];
        pane.tree_find_begin("q".into());
        pane.tree_find_complete(
            vec![
                make_hit(&["SPACE", "p1"], "P1", "SPACE"),
                make_hit(&["SPACE", "p2"], "P2", "SPACE"),
            ],
            false,
        );
        let tree = pane.tree.as_mut().unwrap();
        tree.set_cached_children(Vec::new(), vec![space_node("SPACE", "Space")], None);
        tree.set_cached_children(
            vec!["SPACE".into()],
            vec![page_node("p1", "P1"), page_node("p2", "P2")],
            None,
        );

        // First call: walker lands the first hit.
        match pane.advance_tree_find(0, 0, &vds) {
            TreeFindAdvance::Ready(_) => {}
            other => panic!("expected Ready, got {other:?}"),
        }
        assert!(pane.tree_find.as_ref().unwrap().settled);

        // Second call (simulates a TreeChildren landing later, e.g.
        // because the user expanded another branch): walker must NOT
        // re-position — it returns Idle.
        match pane.advance_tree_find(0, 0, &vds) {
            TreeFindAdvance::Idle => {}
            other => panic!("expected Idle after settled, got {other:?}"),
        }

        // n re-arms: settled cleared, walker drives again.
        pane.tree_find_next();
        assert!(!pane.tree_find.as_ref().unwrap().settled);
        match pane.advance_tree_find(0, 0, &vds) {
            TreeFindAdvance::Ready(_) => {}
            other => panic!("expected Ready after next, got {other:?}"),
        }
    }

    // ── Grouping + aggregation (M3) render path ──────────────────────────

    fn group_columns() -> Vec<ColumnDef> {
        vec![
            ColumnDef {
                key: "category".into(),
                label: None,
                source: Some("label".into()),
                style: None,
                sizing: "max".into(),
                markdown: false,
                kind: ColumnKind::Text,
                format: None,
                separator: None,
                elapsed_from: None,
                tree_aggregate: None,
                hidden: false,
                collapsed_source: None,
                long_source: None,
            },
            ColumnDef {
                key: "dur".into(),
                label: None,
                source: None,
                style: None,
                sizing: "max".into(),
                markdown: false,
                kind: ColumnKind::Number,
                format: None,
                separator: None,
                elapsed_from: None,
                tree_aggregate: None,
                hidden: false,
                collapsed_source: None,
                long_source: None,
            },
        ]
    }

    /// `label` is the group key (read via `source: label`); `dur` is the
    /// aggregated `kind: number` metadata field.
    fn group_item(id: &str, label: &str, dur: &str) -> NodeSummary {
        use not_yet_done_content::{Metadata, MetadataField};
        NodeSummary {
            id: id.into(),
            label: label.into(),
            node_type: not_yet_done_content::mock::default_node_type(),
            metadata: Metadata {
                fields: vec![MetadataField {
                    key: "dur".into(),
                    value: dur.into(),
                    display_label: "dur".into(),
                    editable: false,
                    allowed_values: None,
                }],
            },
            has_children: None,
        }
    }

    /// Drive [`build_grouped_table`] over a small fixture and return the
    /// build plus the right-aligned total text per summary row (trimmed).
    fn run_grouped() -> (GroupedBuild, Vec<String>, Vec<String>) {
        let items = vec![
            group_item("0", "B", "30"),
            group_item("1", "A", "10"),
            group_item("2", "A", "20"),
            group_item("3", "B", "40"),
        ];
        let columns = group_columns();
        let col_ids: Vec<TColumnId> = columns.iter().map(|c| TColumnId::new(&c.key)).collect();
        let mut strategies = std::collections::HashMap::new();
        for c in &columns {
            strategies.insert(TColumnId::new(&c.key), parse_sizing(&c.sizing));
        }
        let config = TableConfig {
            max_width: 300,
            separator: "  ".into(),
            sizer: Box::new(MixedColSizer { strategies }),
        };
        let mut header = TRow::new(0u32).not_selectable();
        for c in &columns {
            header = header.cell(&c.key, c.key.clone());
        }
        let levels = vec![GroupBy {
            column: "category".into(),
            bucket: None,
            order: GroupOrder::Asc,
        }];
        let aggregates = vec![AggregateDef {
            column: "dur".into(),
            op: AggregateOp::Sum,
            total_column: None,
        }];
        let no_links = |_: &str| false;

        let build = build_grouped_table(
            &items,
            &[0, 1, 2, 3],
            &columns,
            &levels,
            &aggregates,
            chrono::Local::now(),
            &no_links,
            &config,
            &col_ids,
            &header,
            false,
        );

        // Total = the trimmed text of the last cell on each summary row.
        let total_of = |r: &TableWidgetRow| {
            r.primary_line()
                .last()
                .map(|c| c.text.trim().to_string())
                .unwrap_or_default()
        };
        let header_totals: Vec<String> = build
            .widget_rows
            .iter()
            .filter(|r| !r.selectable)
            .map(total_of)
            .collect();
        let footer_totals: Vec<String> = build.footers.iter().map(total_of).collect();
        (build, header_totals, footer_totals)
    }

    #[test]
    fn grouped_table_interleaves_headers_items_and_footer() {
        let (build, header_totals, footer_totals) = run_grouped();

        // 2 group headers + 4 items, grand total pinned as footer.
        assert_eq!(build.widget_rows.len(), 6);
        assert_eq!(build.footers.len(), 1);

        // Rows: headerA, item, item, headerB, item, item — headers are the
        // non-selectable rows at positions 0 and 3.
        let selectable: Vec<bool> = build.widget_rows.iter().map(|r| r.selectable).collect();
        assert_eq!(selectable, vec![false, true, true, false, true, true]);

        // filtered_indices align 1:1 with widget_rows; header rows carry the
        // sentinel `usize::MAX`. Items are ordered by group label, so the "A"
        // group (original indices 1, 2) precedes "B" (indices 0, 3).
        assert_eq!(
            build.filtered_indices,
            vec![usize::MAX, 1, 2, usize::MAX, 0, 3]
        );

        // Per-group totals: A = 10+20 = 30, B = 30+40 = 70; grand = 100.
        assert_eq!(header_totals, vec!["30".to_string(), "70".to_string()]);
        assert_eq!(footer_totals, vec!["100".to_string()]);

        // Group-header labels carry the bucket/category text and are styled.
        let header_a = &build.widget_rows[0];
        assert!(header_a.primary_line()[0].text.contains('A'));
        assert_eq!(
            header_a.primary_line()[0].style_id,
            Some(GROUP_HEADER_STYLE_ID)
        );
    }

    /// `fixed(n)` maps to the engine's budget-counted constant width;
    /// malformed input falls back to `Max` like every other sizing string.
    #[test]
    fn parse_sizing_fixed() {
        assert!(matches!(parse_sizing("fixed(30)"), ColStrategy::Fixed(30)));
        assert!(matches!(parse_sizing("fixed(x)"), ColStrategy::Max));
    }

    /// `order: desc` reverses the group order; an aggregate's `total_column`
    /// moves the per-group totals off the header rows onto the closing data
    /// row of each outermost group (and the grand total into the same column
    /// on the Σ footer) — the native trackings layout.
    #[test]
    fn grouped_table_desc_order_and_total_column() {
        let mut columns = group_columns();
        columns.push(ColumnDef {
            key: "total".into(),
            label: Some("Total".into()),
            source: None,
            style: None,
            sizing: "max".into(),
            markdown: false,
            kind: ColumnKind::Number,
            format: None,
            separator: None,
            elapsed_from: None,
            tree_aggregate: None,
            hidden: false,
            collapsed_source: None,
            long_source: None,
        });
        let items = vec![
            group_item("0", "B", "30"),
            group_item("1", "A", "10"),
            group_item("2", "A", "20"),
            group_item("3", "B", "40"),
        ];
        let col_ids: Vec<TColumnId> = columns.iter().map(|c| TColumnId::new(&c.key)).collect();
        let mut strategies = std::collections::HashMap::new();
        for c in &columns {
            strategies.insert(TColumnId::new(&c.key), parse_sizing(&c.sizing));
        }
        let config = TableConfig {
            max_width: 300,
            separator: "  ".into(),
            sizer: Box::new(MixedColSizer { strategies }),
        };
        let mut header = TRow::new(0u32).not_selectable();
        for c in &columns {
            header = header.cell(&c.key, c.key.clone());
        }
        let levels = vec![GroupBy {
            column: "category".into(),
            bucket: None,
            order: GroupOrder::Desc,
        }];
        let aggregates = vec![AggregateDef {
            column: "dur".into(),
            op: AggregateOp::Sum,
            total_column: Some("total".into()),
        }];
        let no_links = |_: &str| false;

        let build = build_grouped_table(
            &items,
            &[0, 1, 2, 3],
            &columns,
            &levels,
            &aggregates,
            chrono::Local::now(),
            &no_links,
            &config,
            &col_ids,
            &header,
            false,
        );

        // Desc: group B first (original indices 0, 3), then A (1, 2).
        assert_eq!(
            build.filtered_indices,
            vec![usize::MAX, 0, 3, usize::MAX, 1, 2]
        );

        let last_text = |r: &TableWidgetRow| {
            r.primary_line()
                .last()
                .map(|c| c.text.trim().to_string())
                .unwrap_or_default()
        };
        // Header rows carry NO total any more (it moved to the data rows):
        // with nothing to right-align, the label spans the whole row.
        assert_eq!(build.widget_rows[0].primary_line().len(), 1);
        assert_eq!(last_text(&build.widget_rows[0]), "── B");
        assert_eq!(last_text(&build.widget_rows[3]), "── A");
        // The total column is blank except on each group's last data row:
        // B closes with 30+40 = 70, A with 10+20 = 30.
        assert_eq!(last_text(&build.widget_rows[1]), "");
        assert_eq!(last_text(&build.widget_rows[2]), "70");
        assert_eq!(last_text(&build.widget_rows[4]), "");
        assert_eq!(last_text(&build.widget_rows[5]), "30");
        // Grand total lands in the total column of the Σ footer.
        assert_eq!(last_text(&build.footers[0]), "100");
    }

    /// `test_config_with_children` with a `group_by` on the root view —
    /// the minimal shape that arms the group-by menu (`u`).
    fn test_config_with_group_by() -> ViewFileConfig {
        let mut config = test_config_with_children();
        config.views[0].group_by = Some(GroupBy {
            column: "key".into(),
            bucket: Some(DateBucket::Day),
            order: GroupOrder::Asc,
        });
        config
    }

    #[test]
    fn jump_mode_action_opens_hop_on_active_table() {
        // Native Tasks-tab parity: the jump action arms the table's hop
        // overlay (phase 1). The App-level interceptor drives the rest.
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        // The App sets this from `navigation.jump_chars`; without an
        // alphabet the table refuses to open (no labels to hand out).
        view.set_nav_chars(&['a', 'b', 'c']);
        assert!(!view.active_pane().table.jump_active());

        // Before arming, the jump hint exists but is not active.
        let jump_key = view
            .content_kb
            .hint_label(&ContentAction::JumpMode, &view.key_icons);
        let jump_hint = |v: &ContentView| -> bool {
            v.action_bar_hints()
                .into_iter()
                .find(|h| h.key == jump_key && h.desc == "jump")
                .map(|h| h.active)
                .expect("jump hint present")
        };
        assert!(!jump_hint(&view));

        let msg = view.dispatch_content_action(ContentAction::JumpMode);
        assert!(matches!(msg, SubViewMessage::SelectionChanged(None)));
        assert!(view.active_pane().table.jump_active());
        assert!(view.active_pane().table.jump_waiting_for_char());

        // Arming jump mode flips the hint's `active` flag (key-identity,
        // configurable key, read live from the pane).
        assert!(jump_hint(&view));
    }

    #[test]
    fn jump_mode_default_key_is_shift_j() {
        // `J` is the shipped default and must not collide with the native
        // tab's `p` (CommonAction::JumpMode), which stays on `p`.
        let kb = KeyBindingConfig::default();
        let binding = kb.content.get(&ContentAction::JumpMode).unwrap();
        assert!(binding.matches("J"));
        assert!(!binding.matches("p"));
    }

    #[test]
    fn group_menu_requires_group_by_level() {
        // No `group_by` on the level → the menu must not open (same gate
        // as the keybinding claim; this covers the action-chain path).
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let msg = view.dispatch_content_action(ContentAction::GroupMenu);
        assert!(matches!(msg, SubViewMessage::Unhandled));
        assert!(!view.group_menu.is_open());
    }

    #[test]
    fn group_menu_jumps_grouping_state() {
        let config = test_config_with_group_by();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());

        // Open the menu and jump straight to Week via its hotkey.
        let msg = view.dispatch_content_action(ContentAction::GroupMenu);
        assert!(matches!(msg, SubViewMessage::SelectionChanged(None)));
        assert!(view.group_menu.is_open());
        view.handle_key("w");
        assert!(!view.group_menu.is_open());
        let gb = view
            .active_pane()
            .current_group_by(&view.view_defs)
            .expect("override grouping");
        assert_eq!(gb.bucket, Some(DateBucket::Week));
        assert_eq!(gb.column, "key");

        // Reopen and pick "No grouping" — runtime state goes ungrouped.
        view.dispatch_content_action(ContentAction::GroupMenu);
        view.handle_key("n");
        assert!(!view.group_menu.is_open());
        assert!(
            view.active_pane()
                .current_group_by(&view.view_defs)
                .is_none()
        );
    }

    #[test]
    fn toggle_group_order_requires_group_by_level() {
        // No `group_by` on the level → nothing to flip (same gate as the
        // view-level claim; this covers the action-chain dispatch path).
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let msg = view.dispatch_content_action(ContentAction::ToggleGroupOrder);
        assert!(matches!(msg, SubViewMessage::Unhandled));
    }

    #[test]
    fn toggle_group_order_flips_order_preserving_bucket() {
        // Configured grouping is `key`/Day/Asc. `o` flips only the order,
        // keeping the column and bucket; pressing it again flips back.
        let config = test_config_with_group_by();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());

        let msg = view.dispatch_content_action(ContentAction::ToggleGroupOrder);
        assert!(matches!(msg, SubViewMessage::SelectionChanged(None)));
        let gb = view
            .active_pane()
            .current_group_by(&view.view_defs)
            .expect("override grouping");
        assert_eq!(gb.order, GroupOrder::Desc); // flipped from Asc
        assert_eq!(gb.bucket, Some(DateBucket::Day)); // preserved
        assert_eq!(gb.column, "key"); // preserved

        // Second toggle returns to ascending.
        view.dispatch_content_action(ContentAction::ToggleGroupOrder);
        let gb = view
            .active_pane()
            .current_group_by(&view.view_defs)
            .expect("override grouping");
        assert_eq!(gb.order, GroupOrder::Asc);
        assert_eq!(gb.bucket, Some(DateBucket::Day));
    }

    #[test]
    fn toggle_group_order_no_op_when_grouping_cycled_off() {
        // Grouping turned off at runtime (Some(None) override) → `o` has no
        // bucket order to flip and reports Unhandled.
        let config = test_config_with_group_by();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.dispatch_content_action(ContentAction::GroupMenu);
        view.handle_key("n"); // No grouping
        assert!(
            view.active_pane()
                .current_group_by(&view.view_defs)
                .is_none()
        );

        let msg = view.dispatch_content_action(ContentAction::ToggleGroupOrder);
        assert!(matches!(msg, SubViewMessage::Unhandled));
    }

    // ── Adapter-grouped tree (group_by_via_adapter) ──────────────────

    /// A pane in tree mode, optionally with the `group_by_via_adapter`
    /// capability — the shape a tracking-style adapter's tree view has.
    fn tree_pane(group_by_via_adapter: bool) -> ContentPane {
        let mut caps = not_yet_done_content::AdapterCapabilities::default();
        caps.group_by_via_adapter = group_by_via_adapter;
        ContentPane::new(test_theme(), 0, true, caps)
    }

    #[test]
    fn tree_grouping_gates_on_adapter_capability() {
        let view_defs = test_config_with_group_by().views;

        // Capability missing → a tree never groups, even with a root
        // `group_by` in the config: keys stay unclaimable, the effective
        // grouping is `None`, nothing is handed to the adapter, and the
        // action-chain path (which bypasses the key-claim gate) no-ops.
        let mut pane = tree_pane(false);
        assert!(!pane.level_has_group_by(&view_defs));
        assert!(pane.current_group_by(&view_defs).is_none());
        assert!(pane.adapter_group_spec(&view_defs).is_none());
        assert!(matches!(
            pane.try_cycle_grouping(&view_defs, 0, 0),
            SubViewMessage::Unhandled
        ));

        // Capability present → the root `group_by` arms the tree.
        let pane = tree_pane(true);
        assert!(pane.level_has_group_by(&view_defs));
        assert_eq!(
            pane.current_group_by(&view_defs).and_then(|gb| gb.bucket),
            Some(DateBucket::Day)
        );
    }

    // ── Shared script source (`script_source`) ───────────────────────

    fn two_view_config(bookmarks_source: Option<&str>) -> ViewFileConfig {
        let src = bookmarks_source
            .map(|s| format!("    script_source: {s}\n"))
            .unwrap_or_default();
        let yaml = format!(
            "tab:\n  name: Jira\n  order: 0\nadapter:\n  type: jira\nviews:\n  - name: tickets\n    node_type: jira:issue\n    default: true\n  - name: bookmarks\n    node_type: jira:bookmark\n    key: m\n{src}"
        );
        serde_yaml::from_str(&yaml).expect("yaml parses")
    }

    #[test]
    fn script_scope_path_swaps_root_for_referenced_source() {
        let view_defs = two_view_config(Some("tickets")).views;
        let caps = not_yet_done_content::AdapterCapabilities::default();

        // tickets (index 0) has no `script_source` → own scope.
        let tickets = ContentPane::new(test_theme(), 0, false, caps.clone());
        assert_eq!(tickets.view_path_node_types(&view_defs), vec!["jira:issue"]);
        assert_eq!(tickets.script_scope_path(&view_defs), vec!["jira:issue"]);

        // bookmarks (index 1) keeps its own identity path, but its script
        // scope borrows tickets' root node_type — so both share scripts.
        let bookmarks = ContentPane::new(test_theme(), 1, false, caps);
        assert_eq!(
            bookmarks.view_path_node_types(&view_defs),
            vec!["jira:bookmark"]
        );
        assert_eq!(bookmarks.script_scope_path(&view_defs), vec!["jira:issue"]);
    }

    #[test]
    fn script_scope_path_unknown_source_falls_back_to_self() {
        // A name matching no sibling view is a silent no-op (validation
        // catches the typo separately); the scope stays the view's own.
        let view_defs = two_view_config(Some("nope")).views;
        let bookmarks = ContentPane::new(
            test_theme(),
            1,
            false,
            not_yet_done_content::AdapterCapabilities::default(),
        );
        assert_eq!(
            bookmarks.script_scope_path(&view_defs),
            vec!["jira:bookmark"]
        );
    }

    #[test]
    fn script_scope_path_without_source_equals_view_path() {
        let view_defs = two_view_config(None).views;
        let bookmarks = ContentPane::new(
            test_theme(),
            1,
            false,
            not_yet_done_content::AdapterCapabilities::default(),
        );
        assert_eq!(
            bookmarks.script_scope_path(&view_defs),
            bookmarks.view_path_node_types(&view_defs)
        );
    }

    #[test]
    fn adapter_group_spec_maps_config_grouping() {
        let mut config = test_config_with_group_by();
        config.views[0].group_by.as_mut().unwrap().order = GroupOrder::Desc;
        let view_defs = config.views;

        let spec = tree_pane(true)
            .adapter_group_spec(&view_defs)
            .expect("tree + capability + root group_by yields a spec");
        assert_eq!(spec.column, "key");
        assert_eq!(spec.bucket, Some(not_yet_done_content::GroupBucket::Day));
        assert_eq!(spec.order, SortDirection::Desc);

        // Flat mode never hands the adapter a grouping — flat lists group
        // engine-side.
        let flat = ContentPane::new(test_theme(), 0, false, tree_pane(true).capabilities.clone());
        assert!(flat.adapter_group_spec(&view_defs).is_none());
    }

    #[test]
    fn tree_cycle_grouping_requests_reload() {
        let view_defs = test_config_with_group_by().views;
        let mut pane = tree_pane(true);

        // Cycling changes the override (Day → Week) and, because the
        // adapter owns the fold, asks for a root reload instead of an
        // engine rebuild.
        let msg = pane.try_cycle_grouping(&view_defs, 3, 7);
        assert!(matches!(
            msg,
            SubViewMessage::Request(ViewRequest::SpawnContentLoad {
                view_index: 3,
                pane_id: 7
            })
        ));
        assert_eq!(
            pane.current_group_by(&view_defs).and_then(|gb| gb.bucket),
            Some(DateBucket::Week)
        );
        assert_eq!(
            pane.adapter_group_spec(&view_defs)
                .expect("spec follows the override")
                .bucket,
            Some(not_yet_done_content::GroupBucket::Week)
        );

        // Cycled off → the adapter gets no grouping (plain tree)...
        for _ in 0..3 {
            pane.try_cycle_grouping(&view_defs, 3, 7);
        }
        assert!(pane.current_group_by(&view_defs).is_none());
        assert!(pane.adapter_group_spec(&view_defs).is_none());
        // ...but the key stays claimable (configured default still set).
        assert!(pane.level_has_group_by(&view_defs));
    }

    #[test]
    fn current_levels_stays_empty_in_adapter_grouped_tree() {
        // The engine's grouped render path is flat-only: even an armed
        // adapter-grouped tree must not produce engine grouping levels
        // (its groups arrive as tree nodes from the adapter).
        let view_defs = test_config_with_group_by().views;
        let pane = tree_pane(true);
        assert!(pane.current_group_by(&view_defs).is_some());
        assert!(pane.current_levels(&view_defs).is_empty());
    }

    #[test]
    fn tree_group_headers_def_gates_on_capability_and_active_grouping() {
        let mut config = test_config_with_group_by();
        config.views[0].group_headers = Some(GroupHeadersDef::default());
        let view_defs = config.views;

        // Armed: tree + capability + config + grouping active.
        let mut pane = tree_pane(true);
        assert!(pane.tree_group_headers_def(&view_defs).is_some());

        // Capability missing → buckets never arrive, headers stay off.
        assert!(
            tree_pane(false)
                .tree_group_headers_def(&view_defs)
                .is_none()
        );

        // Grouping cycled off → the adapter returns plain rows at depth 0,
        // so the header rendering must switch off with it.
        for _ in 0..4 {
            pane.try_cycle_grouping(&view_defs, 0, 0);
        }
        assert!(pane.current_group_by(&view_defs).is_none());
        assert!(pane.tree_group_headers_def(&view_defs).is_none());
    }

    fn group_headers_tree_config() -> ViewFileConfig {
        // Trackings-tree shape: bucket root level + recursive item level.
        let mut config = uniform_recursive_config();
        config.views[0].node_type = "mock:bucket".into();
        config.views[0].group_by = Some(GroupBy {
            column: "key".into(),
            bucket: Some(DateBucket::Day),
            order: GroupOrder::Desc,
        });
        let mut total = dcol("total");
        total.source = Some("dur".into());
        config.views[0].group_headers = Some(GroupHeadersDef { total: Some(total) });
        config
    }

    fn bucket_node(id: &str, label: &str, total: &str) -> NodeSummary {
        use not_yet_done_content::{Metadata, MetadataField};
        let mut n = tnode(id, label, "mock:bucket");
        n.metadata = Metadata {
            fields: vec![MetadataField {
                key: "dur".into(),
                value: total.into(),
                display_label: "dur".into(),
                editable: false,
                allowed_values: None,
            }],
        };
        n
    }

    #[test]
    fn tree_group_headers_blank_bucket_rows_shift_indent_and_close_totals() {
        // Two day buckets; the first holds a task with a subtask, the
        // second a single task. With `group_headers:` active the bucket
        // rows turn into non-selectable placeholders (the widget stage
        // swaps in the `── label` summary row), the items shed the
        // bucket's indentation level, and the appended total column
        // carries each bucket's total on the bucket's LAST row only.
        let config = group_headers_tree_config();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        view.active_pane_mut().capabilities.group_by_via_adapter = true;
        view.set_items(
            vec![
                bucket_node("b1", "Mon", "1:00"),
                bucket_node("b2", "Tue", "0:30"),
            ],
            Vec::new(),
            None,
            Vec::new(),
            None,
        );
        let view_defs = view.view_defs.clone();
        {
            let pane = view.active_pane_mut();
            let tree = pane.tree.as_mut().expect("tree mode");
            tree.set_cached_children(
                vec!["b1".into()],
                vec![tnode("t1", "T1", "mock:task")],
                None,
            );
            tree.expanded.insert(vec!["b1".into()]);
            tree.set_cached_children(
                vec!["b1".into(), "t1".into()],
                vec![tnode("t1a", "T1a", "mock:task")],
                None,
            );
            tree.expanded.insert(vec!["b1".into(), "t1".into()]);
            tree.set_cached_children(
                vec!["b2".into()],
                vec![tnode("t2", "T2", "mock:task")],
                None,
            );
            tree.expanded.insert(vec!["b2".into()]);
            tree.rebuild_entries(&view_defs[0]);
        }
        view.active_pane_mut().rebuild_table(&view_defs);

        let pane = view.active_pane();
        assert!(pane.tree_group_headers_def(&view_defs).is_some());
        // DFS order: b1, t1, t1a, b2, t2.
        let mut columns = pane.current_columns(&view_defs);
        let gh = view_defs[0].group_headers.as_ref().unwrap();
        columns.push(gh.total.clone().unwrap());
        let total_idx = columns.len() - 1;
        let rows = pane.build_tree_data_rows(
            &columns,
            &view_defs,
            chrono::Local::now(),
            true,
            Some(total_idx),
        );
        assert_eq!(rows.len(), 5);

        // Bucket rows: non-selectable placeholders with an empty label.
        let label_key = TColumnId::new("name");
        let cell = |r: usize, key: &TColumnId| {
            rows[r]
                .cells
                .get(key)
                .map(|c| c.text.clone())
                .unwrap_or_default()
        };
        for b in [0usize, 3] {
            assert!(!rows[b].selectable, "bucket row {b} must not be selectable");
            assert_eq!(
                cell(b, &label_key),
                "",
                "bucket row {b} renders at the widget stage"
            );
        }
        // Items shed the bucket's indent level: the first-level task has no
        // box connector (it is a forest root now), its subtask has one.
        let t1 = cell(1, &label_key);
        assert!(
            !t1.contains('├') && !t1.contains('└'),
            "depth-1 item must render at indent 0: {t1:?}"
        );
        let t1a = cell(2, &label_key);
        assert!(
            t1a.contains('└') || t1a.contains('├'),
            "depth-2 item keeps its (shifted) connector: {t1a:?}"
        );
        // The total column closes each bucket on its last row.
        let total_key = TColumnId::new("total");
        assert_eq!(cell(0, &total_key), "");
        assert_eq!(cell(1, &total_key), "");
        assert_eq!(
            cell(2, &total_key),
            "1:00",
            "b1's total sits on its last row"
        );
        assert_eq!(cell(3, &total_key), "");
        assert_eq!(
            cell(4, &total_key),
            "0:30",
            "b2's total sits on its last row"
        );
    }

    #[test]
    fn group_menu_in_adapter_tree_requests_reload() {
        let config = test_config_with_group_by();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        {
            let pane = view.active_pane_mut();
            pane.tree = Some(TreeState::new());
            pane.capabilities.group_by_via_adapter = true;
        }

        view.dispatch_content_action(ContentAction::GroupMenu);
        assert!(view.group_menu.is_open());
        let msg = view.handle_key("m");
        assert!(matches!(
            msg,
            SubViewMessage::Request(ViewRequest::SpawnContentLoad { view_index: 0, .. })
        ));
        assert_eq!(
            view.active_pane()
                .adapter_group_spec(&view.view_defs)
                .expect("menu selection lands in the override")
                .bucket,
            Some(not_yet_done_content::GroupBucket::Month)
        );
    }

    fn long_text_columns() -> Vec<ColumnDef> {
        // A `source: label` column plus a description column whose full body is
        // read from `long_source` while long-text mode is on.
        let plain = |key: &str, source: &str, long_source: Option<&str>| ColumnDef {
            key: key.into(),
            label: None,
            source: Some(source.into()),
            style: None,
            sizing: "max".into(),
            markdown: false,
            kind: ColumnKind::Text,
            format: None,
            separator: None,
            elapsed_from: None,
            tree_aggregate: None,
            hidden: false,
            collapsed_source: None,
            long_source: long_source.map(|s| s.into()),
        };
        vec![
            plain("project", "project", None),
            plain("description", "label", Some("description")),
        ]
    }

    fn summary_with_description(label: &str, description: &str) -> NodeSummary {
        NodeSummary {
            id: "ts-1".into(),
            label: label.into(),
            node_type: not_yet_done_content::NodeType {
                type_id: "mock:timesheet".into(),
                mime_type: "text/plain".into(),
                syntax: None,
                file_extension: ".txt".into(),
                display_name: "Timesheet".into(),
            },
            metadata: not_yet_done_content::Metadata {
                fields: vec![not_yet_done_content::MetadataField {
                    key: "description".into(),
                    value: description.into(),
                    display_label: "Description".into(),
                    editable: false,
                    allowed_values: None,
                }],
            },
            has_children: None,
        }
    }

    fn base_row(cells: &[&str]) -> TableWidgetRow {
        TableWidgetRow::new(
            cells
                .iter()
                .map(|c| TableWidgetCell::plain((*c).to_string()))
                .collect(),
        )
    }

    #[test]
    fn wrap_long_field_splits_on_newlines_and_width() {
        assert_eq!(wrap_long_field("", 5), vec![String::new()]);
        assert_eq!(wrap_long_field("abc", 5), vec!["abc".to_string()]);
        assert_eq!(
            wrap_long_field("abcdefg", 3),
            vec!["abc".to_string(), "def".to_string(), "g".to_string()]
        );
        // Hard line breaks are preserved, then each line is width-chunked.
        assert_eq!(
            wrap_long_field("ab\ncdef", 3),
            vec!["ab".to_string(), "cde".to_string(), "f".to_string()]
        );
        // A zero width is clamped to 1.
        assert_eq!(
            wrap_long_field("ab", 0),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn expand_long_text_row_grows_only_the_long_column() {
        let columns = long_text_columns();
        let col_widths = vec![7, 5];
        let item = summary_with_description("First line", "abcdefghij");
        let base = base_row(&["Acme   ", "First"]);
        let row = expand_long_text_row(base, &item, &columns, &col_widths);

        // "abcdefghij" wraps to 5-wide chunks: "abcde" / "fghij" -> 2 lines.
        assert_eq!(row.lines.len(), 2);
        // First physical line keeps the project cell, description = first chunk
        // padded to width 5.
        assert_eq!(row.lines[0].cells[0].text, "Acme   ");
        assert_eq!(row.lines[0].cells[1].text, "abcde");
        // Continuation line blank-pads the project column to its width, then the
        // second wrapped chunk under the description column.
        assert_eq!(row.lines[1].cells[0].text, " ".repeat(7));
        assert_eq!(row.lines[1].cells[1].text, "fghij");
        // Still one selectable logical row.
        assert!(row.selectable);
    }

    #[test]
    fn expand_long_text_row_single_line_is_untouched() {
        let columns = long_text_columns();
        let col_widths = vec![7, 10];
        let item = summary_with_description("Short", "short");
        let base = base_row(&["Acme   ", "Short"]);
        let row = expand_long_text_row(base, &item, &columns, &col_widths);
        // Fits in one line -> base row returned unchanged.
        assert_eq!(row.lines.len(), 1);
    }

    #[test]
    fn expand_long_text_row_no_long_column_is_noop() {
        let mut columns = long_text_columns();
        columns[1].long_source = None;
        let col_widths = vec![7, 5];
        let item = summary_with_description("First", "abcdefghij");
        let base = base_row(&["Acme   ", "First"]);
        let row = expand_long_text_row(base, &item, &columns, &col_widths);
        assert_eq!(row.lines.len(), 1);
    }

    #[test]
    fn toggle_long_text_key_flips_pane_flag() {
        // Regression: the single-key path routes through `dispatch_view_claim`,
        // so `v` only works if that dispatcher has an explicit arm (like
        // `ToggleDetailWrap`). Without it the key falls through to the pane,
        // which never claims `v`, and nothing happens.
        let mut config = test_config_with_group_by();
        config.views[0].columns[1].long_source = Some("description".into());
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());

        assert!(!view.active_pane().long_text);
        let msg = view.handle_key("v");
        assert!(matches!(msg, SubViewMessage::SelectionChanged(None)));
        assert!(view.active_pane().long_text);
        // Pressing `v` again collapses back.
        view.handle_key("v");
        assert!(!view.active_pane().long_text);
    }

    #[test]
    fn toggle_long_text_key_ignored_without_long_source() {
        // No column opts in → `v` stays free (Unhandled) and the flag never
        // flips, so the key remains available to other views.
        let config = test_config_with_group_by();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let msg = view.handle_key("v");
        assert!(matches!(msg, SubViewMessage::Unhandled));
        assert!(!view.active_pane().long_text);
    }

    /// A `card:` block over the two columns of the test view: `key` and
    /// `summary` side by side, toggled with `C`.
    fn card_config(columns: usize, default_on: bool) -> CardConfig {
        CardConfig {
            fields: vec![
                CardFieldDef {
                    column: "key".into(),
                    label: None,
                },
                CardFieldDef {
                    column: "summary".into(),
                    label: Some("Title".into()),
                },
            ],
            columns,
            weights: Vec::new(),
            labels: CardLabelMode::Inline,
            border: CardBorderMode::Rounded,
            border_style: None,
            label_style: None,
            padding: 1,
            gap: 0,
            separator: "  ".to_string(),
            divider: String::new(),
            key: Some(KeyBinding::new("C")),
            default: default_on,
        }
    }

    #[test]
    fn card_key_toggles_mode_and_asks_the_app_to_persist() {
        let mut config = test_config_with_children();
        config.views[0].card = Some(card_config(2, false));
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());

        assert!(!view.active_pane().card_mode_active(&view.view_defs));
        let msg = view.handle_key("C");
        assert!(
            matches!(
                msg,
                SubViewMessage::Request(ViewRequest::PersistCardMode { .. })
            ),
            "the toggle must ask the App to write the choice so it survives a restart"
        );
        assert!(view.active_pane().card_mode_active(&view.view_defs));
        assert_eq!(
            view.card_mode_overrides().get("view:issues").copied(),
            Some(true)
        );

        // Back to the configured default → the entry is dropped, so nothing
        // stale is persisted after a full round trip.
        view.handle_key("C");
        assert!(!view.active_pane().card_mode_active(&view.view_defs));
        assert!(view.card_mode_overrides().is_empty());
    }

    #[test]
    fn card_key_stays_free_without_a_card_block() {
        // No `card:` on the level → `C` is never claimed and stays available
        // to other views (same contract as `v` without `long_source`).
        let config = test_config_with_children();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        assert!(matches!(view.handle_key("C"), SubViewMessage::Unhandled));
        assert!(view.card_mode_overrides().is_empty());
    }

    #[test]
    fn card_default_true_opens_in_card_mode_and_toggles_off() {
        let mut config = test_config_with_children();
        config.views[0].card = Some(card_config(2, true));
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());

        assert!(view.active_pane().card_mode_active(&view.view_defs));
        view.handle_key("C");
        assert!(!view.active_pane().card_mode_active(&view.view_defs));
        // Deviating from the default is what gets stored.
        assert_eq!(
            view.card_mode_overrides().get("view:issues").copied(),
            Some(false)
        );
    }

    #[test]
    fn stored_card_mode_is_restored_like_a_restart() {
        // What `App::load_card_mode_for` does at startup: push the persisted
        // map in, and the level renders as cards without any key press.
        let mut config = test_config_with_children();
        config.views[0].card = Some(card_config(2, false));
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        assert!(!view.active_pane().card_mode_active(&view.view_defs));

        let mut stored = std::collections::HashMap::new();
        stored.insert("view:issues".to_string(), true);
        view.set_card_mode_overrides(stored);
        assert!(view.active_pane().card_mode_active(&view.view_defs));
    }

    #[test]
    fn card_hint_names_the_mode_the_key_switches_to() {
        let mut config = test_config_with_children();
        config.views[0].card = Some(card_config(2, false));
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let has = |v: &ContentView, label: &str| {
            v.status_bar_hints().into_iter().any(|(_, d)| d == label)
        };
        assert!(has(&view, "cards"), "table mode offers switching to cards");
        view.handle_key("C");
        assert!(has(&view, "table"), "card mode offers switching back");
    }

    #[test]
    fn card_spec_drops_fields_whose_column_is_hidden() {
        // `columns` is the *visible* set, so hiding a column in the
        // column-config popup also takes it out of the card instead of
        // leaving an empty slot behind.
        let config = test_config_with_children();
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let card = card_config(2, false);
        let visible = vec![config.views[0].columns[0].clone()];
        let spec = view.active_pane().card_spec(&card, &visible);
        assert_eq!(spec.fields.len(), 1);
        assert_eq!(spec.fields[0].column.0, "key");
        // The label falls back to the column's own `label:`.
        assert_eq!(spec.fields[0].label, "Key");
    }

    #[test]
    fn card_spec_without_fields_uses_every_visible_column() {
        // The common case: `card:` with no `fields:` shows the whole table,
        // in the effective column order — no second list to keep in sync.
        let config = test_config_with_children();
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let mut card = card_config(2, false);
        card.fields.clear();
        let spec = view
            .active_pane()
            .card_spec(&card, &config.views[0].columns);
        assert_eq!(spec.fields.len(), 2);
        assert_eq!(spec.fields[0].column.0, "key");
        assert_eq!(spec.fields[1].column.0, "summary");
        // Labels come from the columns; one without `label:` falls back to
        // its key.
        assert_eq!(spec.fields[0].label, "Key");
        assert_eq!(spec.fields[1].label, "summary");
    }

    #[test]
    fn card_spec_without_fields_skips_markdown_columns() {
        // A `markdown:` column soft-wraps into N lines, so it has no
        // fixed-height grid slot — an explicit `fields:` entry is rejected at
        // config load, and the implicit "all columns" list drops it here.
        let config = test_config_with_children();
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let mut card = card_config(2, false);
        card.fields.clear();
        let mut columns = config.views[0].columns.clone();
        columns[1].markdown = true;
        let spec = view.active_pane().card_spec(&card, &columns);
        assert_eq!(spec.fields.len(), 1);
        assert_eq!(spec.fields[0].column.0, "key");
    }

    #[test]
    fn card_spec_prefers_the_fields_own_label() {
        let config = test_config_with_children();
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let card = card_config(2, false);
        let spec = view
            .active_pane()
            .card_spec(&card, &config.views[0].columns);
        assert_eq!(spec.fields[1].label, "Title");
        assert_eq!(spec.columns, 2);
    }

    #[test]
    fn card_widget_rows_style_border_label_and_value_separately() {
        let config = test_config_with_children();
        let view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());
        let card = card_config(2, false);
        let spec = view
            .active_pane()
            .card_spec(&card, &config.views[0].columns);
        let rows = vec![
            TRow::new(0u32)
                .cell("key", "ABC-1")
                .cell("summary", "First"),
            TRow::new(1u32)
                .cell("key", "ABC-2")
                .cell("summary", "Second"),
        ];
        let t = test_theme();
        let (widget_rows, _map) =
            build_card_widget_rows(&rows, &config.views[0].columns, &card, &spec, 60, &t);

        assert_eq!(widget_rows.len(), 2, "one card per row");
        // Rounded frame: top, one content line (two fields at columns: 2),
        // bottom.
        let first = &widget_rows[0];
        assert_eq!(first.lines.len(), 3);
        assert!(first.selectable);
        // The frame line is one span, and every line is exactly as wide as
        // the card — that is what keeps the right edge aligned.
        let width = |line: &TableWidgetLine| -> usize {
            line.cells
                .iter()
                .map(|c| c.text.chars().count())
                .sum::<usize>()
        };
        assert_eq!(width(&first.lines[0]), 60);
        assert_eq!(width(&first.lines[1]), 60);
        assert_eq!(width(&first.lines[2]), 60);
        // Content line: the label and its value are separate cells, so they
        // can carry different styles.
        let text: String = first.lines[1]
            .cells
            .iter()
            .map(|c| c.text.as_str())
            .collect();
        assert!(text.contains("Key: ABC-1"), "got: {text:?}");
        assert!(text.contains("Title: First"), "got: {text:?}");
        // Border and label cells get their own style slots, and they differ.
        let border_style = first.lines[0].cells[0].style_id;
        let label_style = first.lines[1]
            .cells
            .iter()
            .find(|c| c.text.starts_with("Key:"))
            .and_then(|c| c.style_id);
        assert!(border_style.is_some());
        assert_ne!(border_style, label_style);
    }

    #[test]
    fn toggle_group_order_via_key_flips_order() {
        // Same regression class as `v`: `o` reaches `dispatch_view_claim` on
        // the single-key path, which needs an explicit arm. Configured order
        // is Asc; `o` flips it to Desc, preserving the bucket.
        let config = test_config_with_group_by();
        let mut view = ContentView::new(test_theme(), &config, None, &KeyBindingConfig::default());

        let msg = view.handle_key("o");
        assert!(matches!(msg, SubViewMessage::SelectionChanged(None)));
        let gb = view
            .active_pane()
            .current_group_by(&view.view_defs)
            .expect("override grouping");
        assert_eq!(gb.order, GroupOrder::Desc);
        assert_eq!(gb.bucket, Some(DateBucket::Day));
    }
}

/// Create a hardcoded Jira view config (used when no YAML file exists).
/// Will be replaced by YAML loading in Phase 6.
pub fn default_jira_view_config() -> ViewFileConfig {
    use crate::config::view_config::*;
    ViewFileConfig {
        reminder: None,
        tab: TabConfig {
            name: "Jira".to_string(),
            order: 3,
            icon: Some("󰌃".to_string()),
            key: None,
            unread_marker: None,
            unread_style: None,
            load_banner: None,
        },
        adapter: AdapterConfig {
            adapter_type: "jira".to_string(),
            id: None,
            config: None,
            config_inline: None,
            manual_connect: false,
        },
        views: vec![ViewDef {
            card: None,
            row_layout: None,
            smooth_scroll: false,
            name: "tickets".to_string(),
            node_type: "jira:issue".to_string(),
            default: true,
            window_ops: false,
            key: None,
            query: None,
            columns: vec![
                ColumnDef {
                    key: "key".into(),
                    label: Some("Key".into()),
                    source: None,
                    style: Some("accent".into()),
                    sizing: "max".into(),
                    markdown: false,
                    kind: ColumnKind::Text,
                    format: None,
                    separator: None,
                    elapsed_from: None,
                    tree_aggregate: None,
                    hidden: false,
                    collapsed_source: None,
                    long_source: None,
                },
                ColumnDef {
                    key: "type".into(),
                    label: Some("Type".into()),
                    source: None,
                    style: None,
                    sizing: "max".into(),
                    markdown: false,
                    kind: ColumnKind::Text,
                    format: None,
                    separator: None,
                    elapsed_from: None,
                    tree_aggregate: None,
                    hidden: false,
                    collapsed_source: None,
                    long_source: None,
                },
                ColumnDef {
                    key: "status".into(),
                    label: Some("Status".into()),
                    source: None,
                    style: Some("success".into()),
                    sizing: "max".into(),
                    markdown: false,
                    kind: ColumnKind::Text,
                    format: None,
                    separator: None,
                    elapsed_from: None,
                    tree_aggregate: None,
                    hidden: false,
                    collapsed_source: None,
                    long_source: None,
                },
                ColumnDef {
                    key: "priority".into(),
                    label: Some("Priority".into()),
                    source: None,
                    style: Some("warning".into()),
                    sizing: "max".into(),
                    markdown: false,
                    kind: ColumnKind::Text,
                    format: None,
                    separator: None,
                    elapsed_from: None,
                    tree_aggregate: None,
                    hidden: false,
                    collapsed_source: None,
                    long_source: None,
                },
                ColumnDef {
                    key: "summary".into(),
                    label: Some("Summary".into()),
                    source: Some("label".into()),
                    style: Some("text_high".into()),
                    sizing: "flex(1)".into(),
                    markdown: false,
                    kind: ColumnKind::Text,
                    format: None,
                    separator: None,
                    elapsed_from: None,
                    tree_aggregate: None,
                    hidden: false,
                    collapsed_source: None,
                    long_source: None,
                },
            ],
            preview: Some(PreviewConfig {
                enabled: true,
                source: "content".to_string(),
                action: None,
                node_id_from: None,
                split: "horizontal".to_string(),
                ratio: 50,
                keybinding: Some("p".to_string()),
                markdown: false,
            }),
            actions: vec![
                ActionDef {
                    name: "edit".to_string(),
                    key: Some("e".into()),
                    action_type: "edit".to_string(),
                    id: Some("edit_full".into()),
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
                },
                ActionDef {
                    name: "refresh".to_string(),
                    key: Some("r".into()),
                    action_type: "reload".to_string(),
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
                },
            ],
            children: vec![],
            pagination: None,
            action_chains: Default::default(),
            column_cursor: false,
            record_detail: false,
            node_scripts: false,
            tree_label: None,
            retries: 0,
            script_template: None,
            script_source: None,
            shortcuts: HashMap::new(),
            leaf_glyph: None,
            icon: None,
            group_by: None,
            aggregates: Vec::new(),
            tree_connector_style: None,
            unread_style: None,
            unread_marker: None,
            tree_lines: None,
            tree_markers: None,
            expand_depth: None,
            group_headers: None,
            event_actions: Vec::new(),
        }],
    }
}
