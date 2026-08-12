use std::sync::Arc;
use std::time::Instant;

use std::collections::HashSet;

use not_yet_done_content::{DefaultQuery, QueryKind};
use not_yet_done_core::repository::{LinkRepository, QueryShortcutRepository, SettingsRepository};
use not_yet_done_ratatui::{DetachedEditor, FilePicker, FilePickerEvent};

use uuid::Uuid;

use crate::action::{self, Action};
use crate::components::adapter_prompt_popup::{AdapterPromptPopup, PromptKeyOutcome};
use crate::components::content_form_popup::{ContentFormEvent, ContentFormPopup};
use crate::components::data_table::DataTable;
use crate::components::notification_bar::NotificationBarComponent;
use crate::components::query_error_bar::QueryErrorBarComponent;
use crate::components::searchable_popup::{PopupItem, SearchablePopup};
use crate::components::sort_menu::SortMenuOutcome;
use crate::components::status_bar::StatusBarComponent;
use crate::components::tab_bar::TabBarComponent;
use crate::config::keybindings::binding_steps;
use crate::config::tui_config::LoadBannerRoute;
use crate::config::{CommonAction, GlobalAction, KeyBindingConfig, TuiConfig};
use crate::tabs::{Tab, TabLayout};
use crate::ui::theme::Theme;
use crate::views::content_view::{ContentView, LoadBanner, collapsed_load_banner};
use crate::views::{SubViewMessage, ViewRequest};

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
    /// Async-loaded items for a content view.
    ContentItems {
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        items: Vec<not_yet_done_content::NodeSummary>,
        applied_sort: Vec<not_yet_done_content::SortKey>,
        page: Option<not_yet_done_content::PageInfo>,
        columns: Vec<not_yet_done_content::ColumnSchema>,
        error: Option<String>,
    },
    /// The columns the adapter *describes* for a node type
    /// ([`ContentAdapter::describe_columns`]), fetched off-thread after a load
    /// so the backend-authoritative column types can be merged into rendering
    /// without blocking the render thread. Additive: a view with no described
    /// columns simply never receives one.
    ContentColumnSchema {
        view_index: usize,
        node_type: String,
        schema: Vec<not_yet_done_content::ColumnSchema>,
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
    /// An inline picture referenced by a `markdown: true` body finished
    /// downloading and decoding. `image` is `None` when either step failed —
    /// the pane retires the URL so a broken attachment costs one request, not
    /// one per rebuild. See [`crate::views::images`].
    ImageDecoded {
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        url: String,
        image: Option<crate::views::images::DecodedImage>,
    },
    /// A menu-step action returned [`ActionOutcome::OpenEditor`]: the user
    /// picked a target from a `Picker` (e.g. Taiga's convert target menu) and
    /// the adapter asked to open a type-specific editor for `action_id` on the
    /// same node. Routed back to the main thread so `open_content_editor` can
    /// build the `NodeActionEditSession` (network-heavy prepare stays
    /// off-thread). `commit_on_save` is false / `editor_profile` is default —
    /// these editors reuse the node's standard edit plumbing.
    OpenContentEditorForAction {
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        node_id: String,
        action_id: String,
        label: String,
    },
    /// An `InputSpec::None` action returned [`ActionOutcome::OpenExternal`]:
    /// it produced a local file (e.g. a downloaded attachment) the frontend
    /// should hand to the OS viewer via the configured link opener. `message`
    /// is the adapter's status line (e.g. "Downloaded 3 images"). The pane is
    /// not reloaded — opening a file changes nothing in the list.
    ContentOpenExternal {
        target: String,
        message: Option<String>,
    },
    /// Async-loaded options for a `type: option_menu` popup. `items` are the
    /// adapter's selectable values (`list_values(source)`); `selected_values`
    /// are the stable ids currently set on the node (parsed from its marker
    /// metadata field) so the menu can pre-mark them. `error` short-circuits
    /// the open with a notification.
    OptionMenuItems {
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        items: Vec<not_yet_done_content::ValueOption>,
        selected_values: Vec<String>,
        /// Fatal load failure — surfaced and the menu stays closed.
        error: Option<String>,
        /// Non-fatal note (e.g. an adapter-rejected create/rename/delete) —
        /// surfaced while the menu stays open.
        notice: Option<String>,
        /// Set when a create/rename/delete changed the underlying data, so the
        /// pane reloads alongside the menu rebuild.
        reload_pane: bool,
    },
    /// Result of a generic content delete (`Node::execute("delete")`).
    /// On success the App removes the row from the pane's tree *in place*
    /// ([`ContentView::remove_tree_node`]) rather than full-reloading —
    /// reload stays reserved for external changes. Falls back to a reload
    /// for non-tree panes or rows the tree can't locate. `node_id` is the
    /// deleted node so the local removal can find it.
    ContentNodeDeleted {
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        node_id: String,
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
    /// Open the generic action popup for an action that collects input via a
    /// form. Emitted by [`App::spawn_invoke_container_action`] (root-scoped)
    /// and [`App::spawn_invoke_node_action`] (per-row) once the resolved
    /// node's action `InputSpec` is seen to be a form: `invoke_action` has no
    /// form dispatch, so the popup / `execute` path drives it instead,
    /// targeted at the resolved node id.
    OpenContentActionPopup {
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        node_id: String,
        action_id: String,
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
    /// A reminder fired by an adapter's reminder stream. Pushed by
    /// `spawn_content_reminder_watcher` only for tabs whose `reminder:`
    /// block is present and `enabled`; `poll_load` runs that tab's
    /// configured `command`. See [`not_yet_done_content::Reminder`].
    AdapterReminder {
        view_index: usize,
        reminder: not_yet_done_content::Reminder,
    },
    /// A backend-initiated request for user input, raised mid-operation (e.g.
    /// an MFA challenge during an interactive browser sign-in). Pushed by
    /// `spawn_content_prompt_watcher` for tabs whose adapter exposes a prompt
    /// stream; `poll_load` opens the global [`AdapterPromptPopup`] overlay (or
    /// queues it behind one already shown). The overlay is tab-agnostic — the
    /// request's `source` label carries the context — so no view index is
    /// threaded through. See [`not_yet_done_content::PromptRequest`].
    AdapterPrompt {
        request: not_yet_done_content::PromptRequest,
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
    /// Targeted reload of a single grouped-tree **bucket** (M9 now-bucket
    /// refresh), spawned by [`App::spawn_now_bucket_reload`] in response to
    /// [`Invalidation::NowAnchored`]. `header` is the bucket node's refreshed
    /// summary (its shifted total / `⏱` marker) and `subtree` its re-folded
    /// forest; [`ContentView::reload_now_bucket`] splices both in place,
    /// leaving every other bucket untouched. A `None` result means the
    /// now-bucket couldn't be resolved (no trackings / load error) — dropped.
    NowBucketReload {
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        result: Option<NowBucketPayload>,
    },
    /// One live-refresh tick for `view_index` (M9 live rows), fired by the
    /// per-view timer set in [`App::set_live_refresh_timer`]. The timer carries
    /// no data — it only paces; the actual fold runs in
    /// [`App::spawn_live_refresh`]. Crucially, a tick for a **background** tab
    /// must not touch the visible tab: the handler only evaluates it when its
    /// view is active, otherwise it marks the view due in
    /// `pending_live_refresh` for a single coalesced refresh on switch-back.
    LiveTick { view_index: usize },
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
    /// A [`BusEvent`](not_yet_done_content::BusEvent) received on the
    /// well-known [`EVENT_CHANNEL`](not_yet_done_content::EVENT_CHANNEL),
    /// forwarded by [`App::spawn_event_rule_watcher`]. The rule engine
    /// ([`App::handle_bus_event`]) matches its `topic` against every content
    /// view's `event_actions` bindings and runs the bound action; an unmatched
    /// *request* (one carrying a `correlation_id`) is NACKed so the emitter
    /// unblocks. Events the host itself emits (`source == "host"`) are dropped
    /// in the watcher to avoid a self-trigger loop.
    BusEvent {
        event: not_yet_done_content::BusEvent,
    },
    /// A plain note from an off-thread load, surfaced through
    /// [`App::notify`]. Loads that *succeed with a caveat* have no other way
    /// back: their result rides in `ContentItems`, whose `error` field would
    /// paint the load as failed. Extended queries produce exactly that —
    /// "truncated at the limit", "the document's own order replaced the
    /// adapter's" — and the rows are still worth showing.
    Notify { text: String },
}

/// Distinct `node_type.type_id`s present in a batch of rows, in first-seen
/// order. Used to fetch the backend-described column schema (3b) once per node
/// type after a load.
fn distinct_node_types(items: &[not_yet_done_content::NodeSummary]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    items
        .iter()
        .filter_map(|i| {
            let t = &i.node_type.type_id;
            seen.insert(t.clone()).then(|| t.clone())
        })
        .collect()
}

/// Execute an extended query document for one level of a content view.
///
/// Nothing about such a document can be handed to `children::list`: it
/// combines several adapter-native queries, and only the executor knows how to
/// render, fetch and merge them — via an [`AdapterBackend`] that lists this
/// very `parent`/`node_type` once per branch. The merged set is therefore
/// always complete, which is why the result carries no [`PageInfo`]: there is
/// no server-side page to continue from.
///
/// `sort` is the pane's own sort (the `s` action). It wins over the document's
/// `order_by` when set, so sorting keeps working in an extended view; with no
/// pane sort the document's order — or, absent an `order_by`, the merge order
/// — survives untouched.
///
/// Warnings are notes on a *successful* load ("truncated at the limit", "no
/// backend order survived"), so they travel as [`LoadMsg::Notify`] rather than
/// through the `error` field, which would paint the load as failed.
#[allow(clippy::too_many_arguments)]
async fn run_extended_query(
    adapter: &dyn not_yet_done_content::ContentAdapter,
    parent: &dyn not_yet_done_content::Node,
    node_type: not_yet_done_content::NodeType,
    document: &str,
    vars: &std::collections::HashMap<String, String>,
    sort: &[not_yet_done_content::SortKey],
    columns: &[not_yet_done_content::ColumnSchema],
    group_by: Option<not_yet_done_content::GroupSpec>,
    tx: &tokio::sync::mpsc::UnboundedSender<LoadMsg>,
) -> Result<not_yet_done_content::ListResult, String> {
    let backend = not_yet_done_extended_query::AdapterBackend::new(adapter, parent, node_type)
        .with_group_by(group_by);
    let types = backend.column_types().await;
    let execution = not_yet_done_extended_query::run(document, &backend, &types, vars, None)
        .await
        .map_err(|e| e.to_string())?;
    for warning in &execution.warnings {
        let _ = tx.send(LoadMsg::Notify {
            text: warning.to_string(),
        });
    }
    let mut items = execution.items;
    let applied_sort = if sort.is_empty() {
        execution.applied_sort
    } else {
        not_yet_done_content::apply_sort(&mut items, sort, columns)
    };
    Ok(not_yet_done_content::ListResult {
        items,
        applied_sort,
        page: None,
        batch_download_available: false,
        downloaded: Vec::new(),
    })
}

/// The pane's active query as a child level should run it.
///
/// A saved query arrives rendered — there is one body, so its variables are
/// substituted once. An extended document arrives verbatim with its bindings
/// instead, because each of its branches renders separately at execution time.
#[derive(Clone, Debug)]
struct SubtreeQuery {
    text: String,
    kind: QueryKind,
    /// Bindings for the branches; empty for an already-rendered saved query.
    vars: std::collections::HashMap<String, String>,
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

/// Successful payload for [`LoadMsg::NowBucketReload`]: the refreshed
/// grouped-tree bucket header and its re-folded subtree. `parent_path` is
/// `vec![header.id]` — the path the subtree hangs under in the pane's tree
/// cache (carried explicitly so the splice doesn't re-derive it).
pub struct NowBucketPayload {
    pub header: not_yet_done_content::NodeSummary,
    pub subtree: not_yet_done_content::Subtree,
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
pub mod option_menu;
pub mod script;

pub use editor::EditorRequest;

/// Generate one label per sortable column. Single letters first, then
/// two-letter combos. Capacity 26 + 26·26 = 702 — far more than any
/// realistic column count.
/// Opt-in reminder-pipeline tracing (`NYD_DEBUG_REMINDER=1`), the TUI half of
/// the calendar-adapter's `rem_trace`. Appends to the same
/// `$TMPDIR/nyd-reminder-debug.log` so a single run shows the whole chain:
/// adapter fires → TUI receives → command runs. No-op unless the env var is set.
fn reminder_trace(args: std::fmt::Arguments<'_>) {
    if std::env::var_os("NYD_DEBUG_REMINDER").is_none() {
        return;
    }
    let path = std::env::temp_dir().join("nyd-reminder-debug.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let ts = chrono::Local::now().to_rfc3339();
        let _ = writeln!(f, "[reminder {ts}] {args}");
    }
}

fn generate_sort_labels(count: usize) -> Vec<String> {
    let alphabet: Vec<char> = ('a'..='z').collect();
    let n = alphabet.len();
    if count <= n {
        return alphabet.iter().take(count).map(|c| c.to_string()).collect();
    }
    let mut out = Vec::with_capacity(count);
    'outer: for first in &alphabet {
        for second in &alphabet {
            if out.len() >= count {
                break 'outer;
            }
            out.push(format!("{}{}", first, second));
        }
    }
    out
}

/// Download every image in `urls` through `adapter` into one fresh temp
/// directory and return the path of the file for `picked` plus the number of
/// files written. Files are named `NN_<basename>` (index-prefixed so order and
/// uniqueness hold even when two URLs share a filename); the picked URL is
/// downloaded first so its file always exists on success. Individual sibling
/// failures are skipped, but if the picked image itself can't be fetched the
/// whole call errors so the caller can fall back to the browser.
///
/// The directory is a single wiped-and-recreated slot under the temp dir, so a
/// later link-hop reuses it (only one viewer session matters at a time). The
/// OS viewer opened on the returned file pages through the siblings written
/// beside it.
async fn download_images_to_temp(
    adapter: &dyn not_yet_done_content::ContentAdapter,
    urls: &[String],
    picked: &str,
) -> Result<(std::path::PathBuf, usize), String> {
    use tokio::fs;

    let dir = std::env::temp_dir().join("not_yet_done_images");
    // Fresh slot: drop whatever a previous hop left behind.
    let _ = fs::remove_dir_all(&dir).await;
    fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("create temp dir: {e}"))?;

    // Picked first, then the rest in first-seen order (deduped).
    let mut ordered: Vec<&str> = vec![picked];
    for u in urls {
        if u != picked && !ordered.contains(&u.as_str()) {
            ordered.push(u);
        }
    }

    let mut picked_path: Option<std::path::PathBuf> = None;
    let mut written = 0usize;
    for (idx, url) in ordered.iter().enumerate() {
        let bytes = match adapter.download_asset(url).await {
            Ok(b) => b,
            Err(e) => {
                if *url == picked {
                    return Err(format!("download image: {e}"));
                }
                continue;
            }
        };
        let path = dir.join(image_temp_filename(idx, url));
        if let Err(e) = fs::write(&path, &bytes).await {
            if *url == picked {
                return Err(format!("write image: {e}"));
            }
            continue;
        }
        written += 1;
        if *url == picked {
            picked_path = Some(path);
        }
    }

    match picked_path {
        Some(p) => Ok((p, written)),
        None => Err("picked image was not downloaded".to_string()),
    }
}

/// Build a safe, index-prefixed local filename for a downloaded image URL.
/// The basename is taken from the URL path (query/fragment dropped), reduced
/// to its last path segment, and stripped of characters that aren't safe in a
/// filename; an image extension is appended if the derived name lacks one.
fn image_temp_filename(idx: usize, url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let raw = path.rsplit(['/', '\\']).next().unwrap_or("image");
    let mut cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        cleaned = "image".to_string();
    }
    if !cleaned.contains('.') {
        cleaned.push_str(".img");
    }
    format!("{idx:02}_{cleaned}")
}

/// A pending confirmation dialog: shows a message, executes on y/Enter, cancels on n/Esc.
pub enum PendingConfirmation {
    /// Drop a stale link row whose target ref can no longer be resolved
    /// (Stale / UnknownRoute / parse failure from [`crate::app::link`]).
    DeleteStaleLink(Uuid),
    /// Bulk-delete every link row whose source or target ref no longer
    /// resolves. Triggered by `:linkprune` after the user accepts the
    /// preview count. The Vec holds the link table IDs to remove.
    BulkDeleteStaleLinks(Vec<Uuid>),
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
        /// The confirmed delete action, re-run verbatim via `Node::execute`
        /// (e.g. `delete` vs the flat list's non-recursive `delete-single`).
        action_name: String,
    },
    /// Generic confirm-then-invoke (from `ActionDispatch::Confirm`). On
    /// accept the App re-invokes the *same* action on the *same* node with
    /// `ActionContext::confirmed = true` via `spawn_invoke_node_action`, so
    /// the adapter performs the work instead of asking again. Used by the
    /// trackings adapter's `restore` / `restore-all`.
    InvokeNodeAction {
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        node_id: String,
        action_name: String,
    },
}

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
    /// Temp-buffer file extension for the captured-output viewer (from the
    /// script's `# output:` header; `.txt` by default). Unused when
    /// `emits_commands` — the output is parsed as JSON, not displayed.
    pub output_suffix: String,
}

impl DetachedScript {
    pub fn is_done(&self) -> bool {
        self.output_path.exists()
    }

    pub fn read_output(&self) -> Option<String> {
        std::fs::read_to_string(&self.output_path).ok()
    }
}

// ---------------------------------------------------------------------------
// Content action popup state
// ---------------------------------------------------------------------------

/// Notification-bar slot holding the "opening editor" line while a session is
/// prepared off-thread. One slot, because only one such load runs at a time
/// ([`App::editor_busy`] rejects a second).
const EDITOR_LOADING_SLOT: &str = "editor:loading";

/// The one slot every globally routed load banner shares
/// ([`App::sync_load_banners`]) — one tab names itself in it, several collapse
/// into a single counter, so loads never cost the bar more than one line.
const LOAD_BANNER_SLOT: &str = "load";

/// A live notification-bar message opened by a `type: notify` event action,
/// retained so an `on_event: { <topic>: close }` binding can retract exactly
/// this message when the topic fires for the same `source`. See
/// [`App::event_notices`].
struct EventNotice {
    /// The event `source` the notice was opened for (e.g. the calendar
    /// connection id). A close event must match it, so one connection's
    /// `resolved` doesn't dismiss another's notice.
    source: String,
    /// Bus topics that retract this notice (the `on_event: close` keys).
    close_topics: Vec<String>,
    /// The bar slot this notice owns. Unique per notice, so retracting one
    /// cannot hit another whose text happens to be identical — two
    /// connections of the same adapter word their prompts the same way.
    key: String,
    /// Which bar the message landed on, so the retract targets the same one:
    /// `true` = the prominent top alert bar, `false` = the bottom bar.
    prominent: bool,
}

/// Substitute `{field}` placeholders in `template` with the matching top-level
/// keys of a bus event's JSON `payload`. A string value is inserted verbatim;
/// any other JSON value uses its compact `to_string()` form (so `{n}` against
/// `{"n": 42}` yields `42`, not `"42"`). Unknown placeholders are left as-is.
/// Used by `type: notify` event actions (e.g. `"tap number {number}"`).
fn render_payload_template(template: &str, payload: &serde_json::Value) -> String {
    let mut out = template.to_string();
    if let Some(obj) = payload.as_object() {
        for (key, value) in obj {
            let replacement = match value {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            out = out.replace(&format!("{{{key}}}"), &replacement);
        }
    }
    out
}

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
    /// Set for an `InputSpec::ColumnForm` (custom columns): maps each field key
    /// to the `value_type` the front-end knows from its own column config
    /// (YAML `kind:`). On submit the values are delivered as a typed
    /// [`ActionInput::ColumnForm`](not_yet_done_content::ActionInput::ColumnForm)
    /// so the store can bootstrap a column on first write. `None` for a plain
    /// [`InputSpec::Form`], which submits the untyped
    /// [`ActionInput::Form`](not_yet_done_content::ActionInput::Form).
    pub column_types: Option<std::collections::HashMap<String, String>>,
}

// ---------------------------------------------------------------------------
// Sort-hint mode
// ---------------------------------------------------------------------------

/// Where a sort change should land. `Content(idx)` updates the indexed
/// `ContentView` + persists in the adapter's own DB.
#[derive(Debug, Clone, Copy)]
pub enum SortTarget {
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
        columns: Vec<not_yet_done_content::ColumnSchema>,
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
/// Address of a single per-node script. Carried in the shortcut-capture
/// state so the captured key chord is persisted under the right node
/// scope. `node_id` is the adapter's own id, passed through opaquely.
#[derive(Debug, Clone)]
pub struct NodeScriptCoords {
    pub view_index: usize,
    pub node_id: String,
    pub script: String,
}

/// A query waiting for the keypress that becomes its shortcut. Carries
/// the body and its [`QueryKind`] because binding a shortcut also (re-)saves
/// the body, and the two stores share one namespace — writing an extended
/// document to the saved-query store would leave a second entry under the
/// same name, in the wrong language.
#[derive(Debug, Clone)]
pub struct PendingFavorite {
    pub scope: String,
    pub name: String,
    pub query: String,
    pub kind: QueryKind,
}

/// Addressing for a `:script`-menu shortcut binding. Carried in the
/// shortcut-capture state so the captured key chord is persisted under
/// the right script scope (`script:<tab>/<view_path…>`) for the named
/// script file.
#[derive(Debug, Clone)]
pub struct ScriptShortcutCoords {
    pub view_index: usize,
    pub scope: String,
    pub name: String,
}

pub enum ContentSlot {
    Working(ContentView),
    Broken {
        name: String,
        path: std::path::PathBuf,
        errors: Vec<String>,
    },
}

/// A DB-stored shortcut resolved back to the `query_shortcut` row it edits.
/// Saved-query / `:script`-menu / Postgres-table script shortcuts keep their
/// chord in the adapter database (`query_shortcut` table), not in any YAML
/// file — so the shortcut editor edits them through the repository, and the
/// owning content view's chord cache is dropped so the next keypress refetches
/// and the new claim goes live.
struct DbShortcutTarget {
    /// `query_shortcut.scope` for the row.
    scope: String,
    /// `query_shortcut.name` for the row (the query/script name).
    name: String,
    /// Content view that owns the chord cache to invalidate.
    view_index: usize,
    invalidate: DbShortcutInvalidate,
}

/// How to drop the cached chord-claim for a [`DbShortcutTarget`] after its DB
/// row changes, so the rebuilt keymap reflects the new state immediately.
enum DbShortcutInvalidate {
    /// Reload the view's saved queries (`reload_content_saved_queries`).
    SavedQuery,
    /// Drop `ContentView::script_shortcuts[scope]`.
    ScriptScope(String),
    /// Drop `ContentView::node_script_shortcuts[node_id]`.
    NodeScript(String),
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
    /// Visible, ordered tabs (from `tabs.order`, or all tabs in slot
    /// order when none is configured). Drives tab switching,
    /// `Tab`/`Shift+Tab` cycling, the digit keys and which tabs the bar
    /// renders. Rebuilt on config reload.
    pub tab_layout: TabLayout,
    pub keybindings: KeyBindingConfig,
    pub theme: Theme,
    pub shared_theme: Arc<Theme>,
    pub config: TuiConfig,
    pub should_quit: bool,

    /// Fullscreen mode: when set, the chrome bars (tab bar, the active
    /// view's action/shortcut bar and the bottom status bar) are hidden so
    /// the content view fills the terminal. Message bars (alerts,
    /// notifications, inline query errors) remain visible. Toggled by
    /// [`GlobalAction::ToggleFullscreen`].
    pub fullscreen: bool,

    pub query_shortcut_repo: Arc<dyn QueryShortcutRepository>,
    settings_repo: Arc<dyn SettingsRepository>,
    pub link_repo: Arc<dyn LinkRepository>,

    pub load_rx: tokio::sync::mpsc::UnboundedReceiver<LoadMsg>,
    load_tx: tokio::sync::mpsc::UnboundedSender<LoadMsg>,

    /// Per-view live-refresh timers (M9 — adapter-driven live rows). Key =
    /// `view_index`. Each handle drives a `tokio::time::interval` that pulls
    /// the view's adapter `live_rows()` and republishes each as a
    /// `LoadMsg::AdapterInvalidation { Invalidation::Row }` patch. (Re)paced
    /// by `Invalidation::RefreshInterval`; at most one timer per view (a
    /// respawn aborts the previous handle, `None` stops it).
    live_refresh_timers: std::collections::HashMap<usize, tokio::task::JoinHandle<()>>,

    /// Views whose live-refresh tick fired while they were **not** the active
    /// tab (M9 — adapter-driven live rows). A background tick must not touch
    /// the visible tab, so it only records the view here; switching *to* the
    /// view runs one coalesced [`Self::spawn_live_refresh`] against the current
    /// state (so any number of missed ticks collapse to a single up-to-date
    /// fold) and clears the flag.
    pending_live_refresh: std::collections::HashSet<usize>,

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

    /// Mounted builtin-editor pane (an editor profile with `builtin: true`).
    /// Mutually exclusive with [`Self::detached_editor`]: both are gated by
    /// [`Self::editor_busy`], so only one editor is ever live. While it is
    /// `Some` the pane owns the keyboard and is laid out above the message
    /// bars.
    pub builtin_editor: Option<crate::components::builtin_editor::BuiltinEditorPane>,

    /// Whether a `NodeActionEditSession` is being prepared off-thread (the
    /// network-heavy fetch behind `OpenContentEditor`). `true` ⇒ a load is in
    /// flight, which counts toward [`Self::editor_busy`] so a second open is
    /// rejected. Its notification-bar line lives in the keyed slot
    /// [`EDITOR_LOADING_SLOT`], which the completion handler clears without
    /// having to remember the exact text it showed.
    editor_loading: bool,
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

    /// Last time a live `Busy` banner was nudged to repaint (~1 Hz).
    last_anim_tick: Instant,

    /// Column configuration popup.
    pub column_config_popup: Option<crate::components::column_config_popup::ColumnConfigPopup>,

    /// Sort menu popup — the whole sort spec at once (`c s`). The second UI
    /// path onto the state the `S` sort-hint mode edits column-by-column.
    pub sort_menu_popup: Option<crate::components::sort_menu::SortMenu>,

    /// Generic, adapter-driven option menu (a `type: option_menu` action).
    /// Stays alive across opens; the inner popup toggles per session. Unlike
    /// the tag menu it knows nothing about tags — it lists whatever values the
    /// adapter exposes via `list_values(source)` and toggles them through a
    /// configured adapter action. See [`crate::app::option_menu`].
    pub option_menu: crate::components::option_menu::OptionMenuComponent,

    /// Dispatch context for the currently open [`Self::option_menu`]: which
    /// node the toggle acts on, the adapter action to invoke, and the pane to
    /// refresh afterwards. Set when the menu opens, consulted on each toggle.
    pub option_menu_target: Option<crate::app::option_menu::OptionMenuTarget>,

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

    /// Shortcut/action menu (`global.shortcut_menu`, default `ctrl+y`).
    /// Lists every configurable keyboard shortcut as name → keys, toggling
    /// between the current context and every tab.
    pub shortcut_menu: crate::components::shortcut_menu::ShortcutMenu,

    /// Adapter credentials popup (login form for adapters that surface
    /// `AdapterStatus::NeedsCreds`).
    pub adapter_creds_popup: Option<crate::components::adapter_creds_popup::AdapterCredsPopup>,

    /// Query-variable input popup. Set when applying a saved query that
    /// the adapter reports as having `${var}` placeholders; cleared on
    /// submit (after the load) or cancel.
    pub query_var_popup: Option<crate::components::query_var_popup::QueryVarPopup>,

    /// `:config` picker popup — lists YAML files under the config dir.
    /// Activating a row opens it in a [`crate::edit_session::FileEditSession`].
    pub config_picker_popup: Option<SearchablePopup>,

    /// Cached set of every `source_ref` + `target_ref` string in the link
    /// table. Drives the "has-links" indicator column without hitting the
    /// DB per row. Refreshed on link create/delete and on startup.
    pub link_refs: HashSet<String>,

    /// Pending key for chord sequences (e.g. "g" waiting for "g" to form "gg").
    pub pending_key: Option<String>,

    /// Which-key chord-completion preview popup (see
    /// [`crate::components::which_key::WhichKeyMenu`]). Purely informational;
    /// mirrors [`Self::pending_key`] via [`Self::reconcile_which_key`].
    pub which_key: crate::components::which_key::WhichKeyMenu,

    /// When the which-key popup should reveal itself: set to
    /// `now + which_key.delay_ms` when a chord prefix is first stashed, taken
    /// by the main loop's timer branch. `None` while nothing is pending or
    /// the popup is already shown.
    which_key_deadline: Option<std::time::Instant>,

    /// When set, the next keypress is captured as a shortcut for a new favorite.
    pub awaiting_favorite_shortcut: Option<PendingFavorite>,
    /// Saved-query shortcut conflicts already surfaced as notifications
    /// this session. Saved queries reload on every tab switch and
    /// q-menu mutation, so without this an unresolved conflict would
    /// re-notify dozens of times per session instead of once.
    warned_saved_query_conflicts: std::collections::HashSet<String>,
    /// Pending shortcut capture for a Postgres per-table script. Carries
    /// the addressing tuple so the captured key chord lands in the right
    /// `<table_dir>/.shortcuts.yaml`. Reset on capture or Esc.
    pub awaiting_node_script_shortcut: Option<NodeScriptCoords>,
    /// Pending shortcut capture for a `:script`-menu script. Carries the
    /// script scope + filename so the captured key chord is persisted via
    /// the `query_shortcut` table. Reset on capture or Esc.
    pub awaiting_script_shortcut: Option<ScriptShortcutCoords>,
    /// Modal message popup — blocks input until dismissed.
    pub modal_message: Option<String>,

    /// Pending confirmation dialog — blocks input until y/n.
    pub pending_confirmation: Option<(String, PendingConfirmation)>,

    /// tuirealm components.
    pub tab_bar: TabBarComponent,
    pub status_bar: StatusBarComponent,
    pub notification_bar: NotificationBarComponent,
    /// Prominent notification strip just beneath the top chrome (tab bar +
    /// action bar) for important messages (e.g. an MFA number to tap).
    /// Fed only by `type: notify` actions flagged `prominent: true`, and only
    /// while `notifications.alert_enabled` is set. Same component as
    /// [`Self::notification_bar`], switched into its alert presentation.
    pub alert_bar: NotificationBarComponent,
    pub query_error_bar: QueryErrorBarComponent,
    pub content_views: Vec<ContentSlot>,

    /// Content action popup (e.g. Jira transitions).
    pub content_action_popup: Option<ContentActionPopupState>,
    pub content_file_picker_popup: Option<ContentFilePickerPopupState>,
    pub content_form_popup: Option<ContentFormPopupState>,

    /// Global overlay for an adapter-initiated mid-operation prompt (e.g. an
    /// MFA challenge). At most one is shown at a time; concurrent requests wait
    /// in `adapter_prompt_queue` and are promoted when the current one closes.
    pub adapter_prompt_popup: Option<AdapterPromptPopup>,
    /// Prompts that arrived while another was already on screen (e.g. two
    /// calendars needing MFA at startup). FIFO; drained by `poll_load`.
    pub adapter_prompt_queue: std::collections::VecDeque<not_yet_done_content::PromptRequest>,

    /// Live notification-bar messages opened by a `type: notify` event action
    /// that declared `on_event: { <topic>: close }`. Each remembers the exact
    /// message pushed, the topics that retract it, and the event `source` it
    /// was opened for — so a later event (e.g. `…:mfa:resolved`) closes only
    /// the notice raised for that same connection, not another's.
    event_notices: Vec<EventNotice>,
    /// Counter behind [`EventNotice::key`] — one slot per opened notice.
    event_notice_seq: u64,

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

    /// Host-owned cross-adapter context (Phase C4): carries the
    /// [`HostEventBus`](not_yet_done_content::HostEventBus) every adapter is
    /// handed at construction. Held here so each `factory.create` call site
    /// (startup load, per-view reload) can pass `&self.host_ctx`.
    pub host_ctx: not_yet_done_content::HostContext,

    /// Active sort-hint mode (column-pick → direction-pick). `Off` when idle.
    pub sort_hint_phase: SortHintPhase,

    /// Guard for [`Self::spawn_event_rule_watcher`]. The rule watcher
    /// subscribes to the host event bus, which outlives every config
    /// reload — unlike the per-adapter watchers it would *not* die when
    /// the views are rebuilt, so a second subscription would deliver each
    /// bus event twice. Set on the first spawn, never cleared.
    event_rule_watcher_started: bool,
}

impl App {
    /// Borrow the working `ContentView` for slot `idx`, or `None` if the
    /// slot is broken or out of range. Most callers want this — only
    /// the render and key-dispatch paths inspect the broken variant.
    pub fn content_view(&self, idx: usize) -> Option<&ContentView> {
        self.content_views.get(idx).and_then(|s| s.as_view())
    }
    pub fn content_view_mut(&mut self, idx: usize) -> Option<&mut ContentView> {
        self.content_views
            .get_mut(idx)
            .and_then(|s| s.as_view_mut())
    }
    /// Iterate working content views (skips broken slots).
    pub fn content_views_iter(&self) -> impl Iterator<Item = &ContentView> {
        self.content_views.iter().filter_map(|s| s.as_view())
    }
    pub fn content_views_iter_mut(&mut self) -> impl Iterator<Item = &mut ContentView> {
        self.content_views
            .iter_mut()
            .filter_map(|s| s.as_view_mut())
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
        query_shortcut_repo: Arc<dyn QueryShortcutRepository>,
        settings_repo: Arc<dyn SettingsRepository>,
        link_repo: Arc<dyn LinkRepository>,
        adapter_factory_builder: Box<
            dyn Fn() -> std::collections::HashMap<
                    String,
                    Box<dyn not_yet_done_content::AdapterFactory>,
                > + Send
                + Sync,
        >,
        host_ctx: not_yet_done_content::HostContext,
    ) -> Self {
        let keybindings = config.keybindings.clone();
        let shared_theme = Arc::new(Theme::new(config.theme.clone()));
        // Pulled out before `config` is moved into the struct literal below.
        let shortcut_menu_execute = config.shortcut_menu.execute_on_enter;
        let shortcut_menu_toggle = config.shortcut_menu.toggle_key.clone();
        // Load content views from YAML config files (must happen before tab_bar).
        let content_views = load_content_views(
            &shared_theme,
            &config.keybindings,
            &config.editors,
            adapter_factory_builder(),
            &host_ctx,
        );
        let content_tab_infos: Vec<crate::components::tab_bar::ContentTabInfo> = content_views
            .iter()
            .map(|slot| crate::components::tab_bar::ContentTabInfo {
                name: slot.tab_name().to_string(),
                icon: slot.tab_icon().unwrap_or_default().to_string(),
            })
            .collect();
        let (tab_layout, tab_layout_error) = build_tab_layout(&config.tabs, &content_views);
        let initial_tab = tab_layout.first();
        let tab_bar = TabBarComponent::new(
            Arc::clone(&shared_theme),
            &content_tab_infos,
            config.tabs.subtabs_own_line,
        );
        let status_bar = StatusBarComponent::new(Arc::clone(&shared_theme), &config.keybindings);

        let bar_hint = notification_bar_hint(&config.keybindings.global);
        let mut notification_bar = NotificationBarComponent::new(Arc::clone(&shared_theme));
        notification_bar.set_max_lines(config.notifications.max_lines);
        notification_bar.set_max_messages(config.notifications.max_messages);
        notification_bar.set_history_limit(config.notifications.history_limit);
        notification_bar.set_hint(bar_hint.clone());
        let mut alert_bar = NotificationBarComponent::new(Arc::clone(&shared_theme));
        alert_bar.set_prominent(true);
        alert_bar.set_max_lines(config.notifications.alert_max_lines);
        alert_bar.set_hint(bar_hint);
        // The top bar carries the messages the user must not miss — it keeps
        // every one of them on screen, so `max_messages` deliberately does not
        // apply here. Only the log limit is shared.
        alert_bar.set_history_limit(config.notifications.history_limit);
        let query_error_bar = QueryErrorBarComponent::new(Arc::clone(&shared_theme));
        let (load_tx, load_rx) = tokio::sync::mpsc::unbounded_channel();
        let (commit_tx, commit_rx) = tokio::sync::mpsc::unbounded_channel();

        // Pre-clone the popup-intrinsic kb + icons so they can be passed
        // into the option/script menus without colliding with the move of
        // `keybindings` into the struct literal below.
        let popup_kb = keybindings.popup.clone();
        let popup_icons = keybindings.key_icons.clone();

        let mut app = Self {
            active_tab: initial_tab,
            tab_layout,
            keybindings,
            theme,
            shared_theme: Arc::clone(&shared_theme),
            config,
            should_quit: false,
            fullscreen: false,
            query_shortcut_repo,
            settings_repo,
            link_repo,
            load_rx,
            load_tx,
            live_refresh_timers: std::collections::HashMap::new(),
            pending_live_refresh: std::collections::HashSet::new(),
            commit_rx,
            commit_tx,
            commit_in_flight: false,
            detached_editor: None,
            detached_script: None,
            pending_session: None,
            builtin_editor: None,
            editor_loading: false,
            editor_load_token: 0,
            pending_editor_request: None,
            last_editor_buffer: None,
            notification: None,
            query_error: None,
            last_error: None,
            last_anim_tick: Instant::now(),
            column_config_popup: None,
            sort_menu_popup: None,
            option_menu: crate::components::option_menu::OptionMenuComponent::new(Arc::clone(
                &shared_theme,
            ))
            .with_popup_kb(popup_kb.clone(), popup_icons.clone()),
            option_menu_target: None,
            script_menu: crate::components::script_menu::ScriptMenuComponent::new(
                Arc::clone(&shared_theme),
                "Scripts",
            )
            .with_popup_kb(popup_kb, popup_icons),
            script_menu_ctx: None,
            shortcut_menu: crate::components::shortcut_menu::ShortcutMenu::new(
                Arc::clone(&shared_theme),
                shortcut_menu_execute,
                shortcut_menu_toggle,
            ),
            adapter_creds_popup: None,
            adapter_prompt_popup: None,
            adapter_prompt_queue: std::collections::VecDeque::new(),
            event_notices: Vec::new(),
            event_notice_seq: 0,
            query_var_popup: None,
            config_picker_popup: None,
            link_refs: HashSet::new(),
            pending_key: None,
            which_key: crate::components::which_key::WhichKeyMenu::new(Arc::clone(&shared_theme)),
            which_key_deadline: None,
            awaiting_favorite_shortcut: None,
            warned_saved_query_conflicts: std::collections::HashSet::new(),
            awaiting_node_script_shortcut: None,
            awaiting_script_shortcut: None,
            modal_message: None,
            pending_confirmation: None,
            content_views,
            content_form_popup: None,
            content_action_popup: None,
            content_file_picker_popup: None,
            tab_bar,
            status_bar,
            notification_bar,
            alert_bar,
            query_error_bar,
            sort_hint_phase: SortHintPhase::Off,
            marked_link: None,
            marked_db_script_for_move: None,
            content_marked_node: None,
            cut_node_id: None,
            link_popup: None,
            jump_history: crate::app::link::JumpHistory::new(),
            adapter_factory_builder,
            host_ctx,
            event_rule_watcher_started: false,
        };
        // A duplicate tab name is a hard config error — show it up front
        // (the layout already fell back to legacy so the app still runs).
        if let Some(err) = tab_layout_error {
            app.modal_message = Some(format!("Tab configuration error:\n\n{err}"));
        }

        // Configure nav chars on all tables.
        app.apply_nav_chars();

        app.reload_link_refs();

        app
    }

    /// Apply the jump-mode label alphabet (`navigation.jump_chars`) to
    /// every content view. A view without it opens jump mode with no
    /// labels, so this has to run for freshly built views — at startup
    /// and again after every config reload that replaces them.
    pub fn apply_nav_chars(&mut self) {
        let nav_chars: Vec<char> = self.config.navigation.jump_chars.chars().collect();
        for cv in self.content_views_iter_mut() {
            cv.set_nav_chars(&nav_chars);
        }
    }

    /// [`Self::apply_nav_chars`] for a single slot.
    pub fn apply_nav_chars_to(&mut self, view_index: usize) {
        let nav_chars: Vec<char> = self.config.navigation.jump_chars.chars().collect();
        if let Some(cv) = self.content_view_mut(view_index) {
            cv.set_nav_chars(&nav_chars);
        }
    }

    /// Everything a freshly constructed [`ContentView`] needs before it can
    /// render and fetch: the jump alphabet, its DB-persisted state (column
    /// overrides, saved queries, default query, sort spec) and the async
    /// watchers plus the initial load.
    ///
    /// The single source for "what startup does to a view after building
    /// it" — every in-process config reload that rebuilds a slot must run
    /// this for it, or the replacement view stays empty and unwired (no
    /// data, no jump labels, no adapter watchers) until the next restart.
    pub fn wire_content_view(&mut self, view_index: usize) {
        self.apply_load_banner_route(view_index);
        self.apply_nav_chars_to(view_index);
        self.load_column_config_for(view_index);
        self.load_card_mode_for(view_index);
        self.reload_content_saved_queries(view_index);
        self.apply_default_content_query(view_index);
        self.load_content_sort_state(view_index);
        self.start_content_load(view_index);
    }

    /// [`Self::wire_content_view`] for every slot, plus the one global
    /// subscription that is not per-view. Used by startup and by the
    /// reload paths that rebuild the whole view set.
    pub fn wire_content_views(&mut self) {
        for idx in 0..self.content_views.len() {
            self.wire_content_view(idx);
        }
        // Rule engine: one global subscription to the host event bus, routed
        // by topic to whichever view declares a matching `event_actions`
        // binding. Independent of the per-view watchers above.
        self.spawn_event_rule_watcher();
    }

    /// Auto-load the content view in `view_index` if it has an adapter from
    /// YAML config. The watchers are always spawned (they only subscribe to
    /// a channel, no I/O); the load itself is skipped for tabs flagged
    /// `adapter.manual_connect: true` so they wait for an explicit
    /// user-triggered `reload` action.
    ///
    /// Called from [`Self::wire_content_view`] *after* the DB-persisted
    /// default query has been stamped onto the pane, so the first fetch
    /// already uses it — that ordering is why the wiring lives outside
    /// [`App::new`], which has no repositories to read yet.
    pub fn start_content_load(&mut self, view_index: usize) {
        let Some((pane_id, manual)) = self
            .content_view(view_index)
            .filter(|cv| cv.adapter.is_some())
            .map(|cv| (cv.active_pane_id(), cv.manual_connect))
        else {
            return;
        };
        self.spawn_content_status_watcher(view_index);
        self.spawn_content_invalidation_watcher(view_index);
        self.spawn_content_reminder_watcher(view_index);
        self.spawn_content_prompt_watcher(view_index);
        if !manual {
            self.spawn_content_load(view_index, pane_id);
        }
    }

    /// Stamp a content view's default saved query (if any) onto its active
    /// pane — plus every subtab pane opting in via `query.inherit_default`
    /// — so the initial load already uses it. Runs from
    /// [`Self::wire_content_view`] after the saved queries were read from
    /// the DB; a default whose name no longer exists in the store is
    /// skipped silently (the view falls back to its YAML `query.default`).
    pub fn apply_default_content_query(&mut self, view_index: usize) {
        let Some(cv) = self.content_view_mut(view_index) else {
            return;
        };
        let Some(name) = cv.default_saved_query.clone() else {
            return;
        };
        let Some((body, kind)) = cv
            .db_saved_queries
            .iter()
            .find(|q| q.name == name)
            .map(|q| (q.query.clone(), q.kind))
        else {
            return;
        };
        cv.apply_default_query(body, Some(name), kind);
    }

    // -----------------------------------------------------------------------
    // Async task loading
    // -----------------------------------------------------------------------

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
        let Some(cv) = self.content_view(view_index) else {
            return;
        };
        let Some(adapter) = cv.adapter.as_ref() else {
            return;
        };
        let adapter = Arc::clone(adapter);
        let tx = self.load_tx.clone();
        tokio::spawn(async move {
            let result = adapter.submit_credentials(values).await;
            let error = result.err().map(|e| e.to_string());
            let _ = tx.send(LoadMsg::CredentialSubmitResult { view_index, error });
        });
    }

    /// Tell the adapter behind `view_index` that the user dismissed the
    /// credential form, so the login it is waiting on gives up.
    ///
    /// Deliberately fire-and-forget: the popup is already gone, and an
    /// adapter that does not support interactive auth cannot have been
    /// waiting on one.
    pub fn spawn_cancel_credentials(&self, view_index: usize) {
        let Some(cv) = self.content_view(view_index) else {
            return;
        };
        let Some(adapter) = cv.adapter.as_ref() else {
            return;
        };
        let adapter = Arc::clone(adapter);
        tokio::spawn(async move {
            let _ = adapter.cancel_credentials().await;
        });
    }

    pub fn spawn_content_status_watcher(&self, view_index: usize) {
        let Some(cv) = self.content_view(view_index) else {
            return;
        };
        let Some(adapter) = cv.adapter.as_ref() else {
            return;
        };
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
        let Some(cv) = self.content_view(view_index) else {
            return;
        };
        let Some(adapter) = cv.adapter.as_ref() else {
            return;
        };
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

    /// Forward an adapter's [`Reminder`] stream into `poll_load` as
    /// [`LoadMsg::AdapterReminder`], but *only* for tabs that opted in with a
    /// present-and-`enabled` `reminder:` block — a tab without one never
    /// subscribes, so it ignores reminders entirely (the adapter still owns
    /// *when* they fire; this owns *whether* and *what runs*). On `Lagged` we
    /// simply skip the dropped reminders: firing a stale command late is worse
    /// than missing one, and the next reminder arrives on schedule.
    ///
    /// [`Reminder`]: not_yet_done_content::Reminder
    pub fn spawn_content_reminder_watcher(&self, view_index: usize) {
        let Some(cv) = self.content_view(view_index) else {
            return;
        };
        // Opt-in gate: no `reminder:` block, or `enabled: false` → no watcher.
        match cv.reminder.as_ref() {
            Some(r) if r.enabled => {}
            _ => return,
        }
        let Some(adapter) = cv.adapter.as_ref() else {
            return;
        };
        let mut rx = adapter.subscribe_reminders();
        let tx = self.load_tx.clone();
        tokio::spawn(async move {
            use tokio::sync::broadcast::error::RecvError;
            loop {
                match rx.recv().await {
                    Ok(reminder) => {
                        if tx
                            .send(LoadMsg::AdapterReminder {
                                view_index,
                                reminder,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
        });
    }

    /// Forward an adapter's mid-operation [`PromptRequest`] stream into
    /// `poll_load` as [`LoadMsg::AdapterPrompt`]. Take-once (`mpsc`, not
    /// broadcast): each request carries a non-cloneable one-shot responder, so
    /// exactly one frontend services it. Harmless for adapters that never
    /// prompt — [`ContentAdapter::take_prompt_requests`] defaults to `None`, so
    /// the watcher simply doesn't spawn.
    ///
    /// [`PromptRequest`]: not_yet_done_content::PromptRequest
    /// [`ContentAdapter::take_prompt_requests`]: not_yet_done_content::ContentAdapter::take_prompt_requests
    pub fn spawn_content_prompt_watcher(&self, view_index: usize) {
        let Some(cv) = self.content_view(view_index) else {
            return;
        };
        let Some(adapter) = cv.adapter.as_ref() else {
            return;
        };
        let Some(mut rx) = adapter.take_prompt_requests() else {
            return;
        };
        let tx = self.load_tx.clone();
        tokio::spawn(async move {
            while let Some(request) = rx.recv().await {
                if tx.send(LoadMsg::AdapterPrompt { request }).is_err() {
                    break;
                }
            }
        });
    }

    /// Subscribe to the host [`EVENT_CHANNEL`] and forward each
    /// [`BusEvent`] into `poll_load` as [`LoadMsg::BusEvent`]. Spawned **once**
    /// at startup (not per-view): topic-routing to the owning view happens in
    /// [`Self::handle_bus_event`], and events such as an MFA challenge arrive
    /// independent of which tab is active. Host-sourced events
    /// (`source == "host"`) are dropped here so an action's own `emit` — or a
    /// NACK the engine itself publishes — never re-enters the engine. On
    /// `Lagged` we skip the dropped events: a missed prompt is re-raised by the
    /// backend's own retry, while replaying a stale one is worse.
    ///
    /// [`EVENT_CHANNEL`]: not_yet_done_content::EVENT_CHANNEL
    /// [`BusEvent`]: not_yet_done_content::BusEvent
    pub fn spawn_event_rule_watcher(&mut self) {
        // Once only: the host event bus outlives every config reload, so a
        // second subscription would deliver each event twice.
        if self.event_rule_watcher_started {
            return;
        }
        self.event_rule_watcher_started = true;
        let mut rx = not_yet_done_content::subscribe_events(&*self.host_ctx.event_bus);
        let tx = self.load_tx.clone();
        tokio::spawn(async move {
            use tokio::sync::broadcast::error::RecvError;
            loop {
                match rx.recv().await {
                    Ok(raw) => {
                        let Some(event) = not_yet_done_content::BusEvent::from_host_event(&raw)
                        else {
                            continue;
                        };
                        if event.source == "host" {
                            continue;
                        }
                        if tx.send(LoadMsg::BusEvent { event }).is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
        });
    }

    /// Open the global adapter-prompt overlay for `request`, or queue it behind
    /// one already on screen. An input shape the overlay can't render inline is
    /// cancelled cleanly (the raising op unwinds) with a user-facing note,
    /// rather than left hanging.
    fn open_adapter_prompt(&mut self, request: not_yet_done_content::PromptRequest) {
        if self.adapter_prompt_popup.is_some() {
            self.adapter_prompt_queue.push_back(request);
            return;
        }
        let mut popup = AdapterPromptPopup::new(request, &self.theme);
        if let Some(what) = popup.take_unsupported() {
            self.notify_error(format!("Adapter prompt input '{what}' is not supported"));
            return;
        }
        self.adapter_prompt_popup = Some(popup);
        self.sync_components();
    }

    /// Promote the next queued adapter prompt, if any, once the current overlay
    /// has closed. Called from the key interceptor after a prompt resolves.
    fn advance_adapter_prompt_queue(&mut self) {
        if self.adapter_prompt_popup.is_some() {
            return;
        }
        if let Some(next) = self.adapter_prompt_queue.pop_front() {
            self.open_adapter_prompt(next);
        }
    }

    /// Rule engine: react to one [`BusEvent`](not_yet_done_content::BusEvent)
    /// received on the host [`EVENT_CHANNEL`](not_yet_done_content::EVENT_CHANNEL).
    ///
    /// Scans **every** content view (not just the active tab — an MFA challenge
    /// arrives whichever tab is focused) for an `event_actions` binding whose
    /// `on:` topic equals the event's, and runs each bound action on its owning
    /// view. When no binding matches and the event is a *request* (carries a
    /// `correlation_id`), it is NACKed: a reply with the same id but no payload
    /// goes back on the bus so the waiting emitter (e.g. an OTC prompt) unblocks
    /// and cancels rather than hanging forever.
    fn handle_bus_event(&mut self, event: not_yet_done_content::BusEvent) {
        // A close/resolve event retracts any live notice it targets first —
        // independent of whether it also triggers an action (a `resolved`
        // event typically has no `event_actions` binding, only closes).
        self.sweep_event_notices(&event);

        // Collect (view_index, action_name) up front so the mutable dispatch
        // below doesn't overlap the immutable view scan.
        let targets: Vec<(usize, String)> = self
            .content_views_indexed()
            .flat_map(|(i, cv)| {
                cv.event_action_targets(&event.topic)
                    .into_iter()
                    .map(move |name| (i, name))
            })
            .collect();

        if targets.is_empty() {
            if let Some(correlation_id) = event.correlation_id.clone() {
                // Unhandled request → NACK so the emitter stops waiting. Same
                // correlation id, empty payload, `source == "host"` (the
                // watcher drops it, so this never re-enters the engine).
                not_yet_done_content::publish_event(
                    &*self.host_ctx.event_bus,
                    not_yet_done_content::BusEvent::new(
                        "host:nack",
                        "host",
                        serde_json::Value::Null,
                    )
                    .with_correlation(correlation_id),
                );
            }
            return;
        }

        for (view_index, name) in targets {
            self.dispatch_event_action(view_index, &name, &event);
        }
    }

    /// Run the action named `name` on content view `view_index` in response to
    /// bus `event`. A `type: notify` action is handled here — its message is
    /// templated from the event payload and pushed to the notification bar
    /// (retained for `on_event: close` when declared). Every other action type
    /// runs through the view's normal execute path and its resulting
    /// [`SubViewMessage`] is routed exactly like the key path.
    fn dispatch_event_action(
        &mut self,
        view_index: usize,
        name: &str,
        event: &not_yet_done_content::BusEvent,
    ) {
        let Some(action) = self
            .content_view(view_index)
            .and_then(|cv| cv.find_action_by_name(name))
        else {
            return;
        };
        if action.action_type == "notify" {
            self.open_event_notice(&action, event);
            return;
        }
        let msg = {
            let Some(cv) = self.content_view_mut(view_index) else {
                return;
            };
            cv.dispatch_event_action(name)
        };
        let _ = self.process_sub_view_message(msg);
    }

    /// Push a `type: notify` action's message onto the notification bar,
    /// substituting `{field}` placeholders from the triggering event's JSON
    /// payload. When the action declares `on_event: { <topic>: close }`, the
    /// pushed message is remembered as an [`EventNotice`] so a later event on
    /// one of those topics (for the same `source`) retracts it.
    fn open_event_notice(
        &mut self,
        action: &crate::config::view_config::ActionDef,
        event: &not_yet_done_content::BusEvent,
    ) {
        let template = action.message.as_deref().unwrap_or_default();
        let message = render_payload_template(template, &event.payload);
        if message.is_empty() {
            return;
        }
        // Prominent messages go to the loud top bar — but only when it is
        // enabled; otherwise they fall back to the ordinary bottom bar, so the
        // message is never silently dropped.
        let prominent = action.prominent && self.config.notifications.alert_enabled;
        // Every notice owns a slot of its own, numbered rather than named
        // after its text: two connections of one adapter phrase their prompts
        // identically, and a retract must not be able to pick the wrong one.
        self.event_notice_seq = self.event_notice_seq.wrapping_add(1);
        let key = format!("event:{}", self.event_notice_seq);
        let class = crate::components::notification_bar::NoticeClass::Message;
        if prominent {
            self.alert_bar.set_keyed(&key, class, message.clone());
        } else {
            self.notification_bar
                .set_keyed(&key, class, message.clone());
        }
        let close_topics: Vec<String> = action
            .on_event
            .as_ref()
            .map(|m| {
                m.iter()
                    .filter(|(_, r)| {
                        matches!(r, crate::config::view_config::OnEventReaction::Close)
                    })
                    .map(|(t, _)| t.clone())
                    .collect()
            })
            .unwrap_or_default();
        if !close_topics.is_empty() {
            self.event_notices.push(EventNotice {
                source: event.source.clone(),
                close_topics,
                key,
                prominent,
            });
        }
    }

    /// Retract every live [`EventNotice`] whose `close_topics` include this
    /// event's topic and whose `source` matches — scoped so one connection's
    /// resolve doesn't dismiss another's notice.
    fn sweep_event_notices(&mut self, event: &not_yet_done_content::BusEvent) {
        let mut i = 0;
        while i < self.event_notices.len() {
            let n = &self.event_notices[i];
            if n.source == event.source && n.close_topics.iter().any(|t| t == &event.topic) {
                let removed = self.event_notices.remove(i);
                if removed.prominent {
                    self.alert_bar.clear_keyed(&removed.key);
                } else {
                    self.notification_bar.clear_keyed(&removed.key);
                }
            } else {
                i += 1;
            }
        }
    }

    /// Start, re-pace, or stop the live-refresh timer for `view_index`
    /// (M9 — adapter-driven live rows). `Some(interval)` (re)spawns a
    /// `tokio::time::interval` that, on each tick, sends a data-free
    /// [`LoadMsg::LiveTick`] through the load channel; `None` stops it. The
    /// timer only *paces* — the actual fold runs in
    /// [`Self::spawn_live_refresh`], gated on whether the view is the active
    /// tab (a background tick must not touch the visible tab). A respawn aborts
    /// the existing handle first, so the cadence the adapter last declared
    /// always wins and timers never accumulate across re-pacings.
    fn set_live_refresh_timer(&mut self, view_index: usize, interval: Option<std::time::Duration>) {
        // Re-pacing replaces the running timer; `None` leaves it stopped.
        if let Some(handle) = self.live_refresh_timers.remove(&view_index) {
            handle.abort();
        }
        let Some(interval) = interval else { return };
        if interval.is_zero() {
            return; // a zero interval would busy-loop
        }
        let tx = self.load_tx.clone();
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // `interval()` fires immediately at t=0; skip that tick so the
            // first refresh lands one interval out, not on the same frame
            // as the load that declared the cadence.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if tx.send(LoadMsg::LiveTick { view_index }).is_err() {
                    return; // app gone
                }
            }
        });
        self.live_refresh_timers.insert(view_index, handle);
    }

    /// Run one live-refresh fold for `view_index` (M9 — adapter-driven live
    /// rows): pull the adapter's [`live_rows`] (the flat list / cross-tab
    /// markers — this also re-paces the adaptive interval) and, for each
    /// grouped-tree pane, the now-bucket's chain folded against live `now`
    /// ([`live_group_rows`]), forwarding every refreshed row as an
    /// [`Invalidation::Row`] patch. Per-pane [`GroupSpec`] + saved query are
    /// resolved up front (frontend state the adapter can't see), then the
    /// async folds run off the main loop. Called for the active tab on every
    /// tick, and once per tab on switch-back (see `pending_live_refresh`).
    ///
    /// [`live_rows`]: not_yet_done_content::ContentAdapter::live_rows
    /// [`live_group_rows`]: not_yet_done_content::ContentAdapter::live_group_rows
    /// [`Invalidation::Row`]: not_yet_done_content::Invalidation::Row
    /// [`GroupSpec`]: not_yet_done_content::GroupSpec
    pub fn spawn_live_refresh(&self, view_index: usize) {
        let Some(cv) = self.content_view(view_index) else {
            return;
        };
        let Some(adapter) = cv.adapter.as_ref() else {
            return;
        };
        let adapter = Arc::clone(adapter);
        // Each grouped *eager* tree pane's (spec, saved query). A flat /
        // condensed / ungrouped pane has no spec and is served by `live_rows`
        // alone; a pane with a spec but no eager depth isn't a tree we fold.
        let grouped: Vec<(not_yet_done_content::GroupSpec, Option<String>)> = cv
            .all_pane_ids()
            .into_iter()
            .filter_map(|pid| {
                let pane = cv.find_pane(pid)?;
                let spec = pane.adapter_group_spec(&cv.view_defs)?;
                pane.eager_subtree_depth(&cv.view_defs)?;
                let query = Self::subtree_query_for_pane(cv, pane, &adapter);
                // `live_group_rows` is the adapter recomputing its own bucket
                // under one native query; an extended document has no such
                // form. Such a pane sits out the live tick rather than being
                // refreshed against a query it isn't showing — its rows still
                // update on the next reload.
                match query {
                    Some(q) if q.kind == QueryKind::Extended => None,
                    other => Some((spec, other.map(|q| q.text))),
                }
            })
            .collect();
        let tx = self.load_tx.clone();
        tokio::spawn(async move {
            let send = |summary| {
                tx.send(LoadMsg::AdapterInvalidation {
                    view_index,
                    inv: not_yet_done_content::Invalidation::Row(summary),
                })
                .is_ok()
            };
            // Flat list / cross-tab markers (and adaptive re-pacing).
            for summary in adapter.live_rows().await {
                if !send(summary) {
                    return; // app gone
                }
            }
            // Grouped trees: the now-bucket's ticking chain, keyed to the
            // rendered tree rows so `patch_row` swaps them in place.
            for (spec, query) in grouped {
                for summary in adapter.live_group_rows(&spec, query.as_deref()).await {
                    if !send(summary) {
                        return;
                    }
                }
            }
        });
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
        // M9 now-bucket refresh — now-anchored data shifted: reload only the
        // bucket the current instant falls into, per grouped-tree pane, rather
        // than rebuilding every bucket. `spawn_now_bucket_reload` self-filters
        // (it no-ops on panes that aren't grouped trees), so we can offer it
        // every pane in the view.
        if matches!(inv, Invalidation::NowAnchored) {
            let panes = self
                .content_view(view_index)
                .map(|cv| cv.all_pane_ids())
                .unwrap_or_default();
            for pid in panes {
                if self.pane_is_grouped_eager_tree(view_index, pid) {
                    self.spawn_now_bucket_reload(view_index, pid);
                } else {
                    // Flat / grouped-but-not-tree / condensed pane: a
                    // now-anchored shift can *add or drop a row* (a freshly
                    // started tracking appears, a deleted one vanishes) and
                    // move group totals — none of which a row patch or a
                    // localized bucket reload can express. Reload the pane
                    // at its current level so the new row shows up (the bug
                    // where a tracking started elsewhere was missing from
                    // the flat Trackings tab until a manual `r`).
                    self.reload_content_pane_current_level(view_index, pid);
                }
            }
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
                    // Row / RefreshInterval / NowAnchored are handled by the
                    // early returns above and never reach this filter.
                    Invalidation::Repaint
                    | Invalidation::Row(_)
                    | Invalidation::RefreshInterval(_)
                    | Invalidation::NowAnchored => false,
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
        // A record-detail follower has no fetchable data of its own — its
        // rows are the transposed fields of the source's selected record.
        // Reloading it directly would fetch the source level's rows into the
        // synthetic pane and blank it. Redirect to the source pane so the
        // record reloads and `sync_detail_panes` re-transposes it.
        let pane_id = self
            .content_view(view_index)
            .and_then(|cv| cv.find_pane(pane_id))
            .and_then(|pane| pane.detail_source())
            .unwrap_or(pane_id);
        let drill = self
            .content_view(view_index)
            .and_then(|cv| cv.find_pane(pane_id))
            .and_then(|pane| {
                let parent = pane.parent_node_id()?.to_string();
                let child = pane.current_child_node_type()?.to_string();
                Some((parent, child))
            });
        match drill {
            Some((parent, child)) => {
                self.spawn_content_drill_down(view_index, pane_id, parent, child)
            }
            None => self.spawn_content_load(view_index, pane_id),
        }
    }

    /// Soft (re)load: list from the adapter, serving any warm cache. This is
    /// the default used by the ~16 ordinary load call sites.
    pub fn spawn_content_load(
        &self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
    ) {
        self.spawn_content_load_inner(view_index, pane_id, false);
    }

    /// Hard reload (the `r` action): ask the adapter to `refresh()` — abort
    /// every in-flight load and drop caches — *before* re-listing, so the load
    /// always fetches fresh instead of re-serving a warm cache.
    pub fn spawn_content_reload(
        &self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
    ) {
        self.spawn_content_load_inner(view_index, pane_id, true);
    }

    fn spawn_content_load_inner(
        &self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        force: bool,
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
            kind,
        } = req;
        // A saved query is one adapter-native body, so its variables are
        // rendered once here. An extended document is rendered per branch
        // inside the executor and must reach it verbatim.
        let query = match kind {
            QueryKind::Saved => query.map(|raw| adapter.render_query(&raw, &vars)),
            QueryKind::Extended => query,
        };
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
            // Hard reload: let the adapter tear down in-flight work and caches
            // first, so the list below runs against a clean slate. No-op for
            // adapters that don't override `refresh()`.
            if force {
                let _ = adapter.refresh().await;
            }
            let result = run_with_retries(retries, &tx, view_index, pane_id, || {
                let adapter = Arc::clone(&adapter);
                let node_type_id = node_type_id.clone();
                let query = query.clone();
                let sort = sort.clone();
                let vars = vars.clone();
                let group_by = group_by.clone();
                let tx = tx.clone();
                async move {
                    let root = adapter.root().await.map_err(|e| e.to_string())?;
                    let node_type =
                        not_yet_done_content::children::child_types(&*adapter, root.as_ref())
                            .into_iter()
                            .find(|t| t.type_id == node_type_id)
                            .ok_or_else(|| format!("Node type '{node_type_id}' not found"))?;
                    let columns = not_yet_done_content::children::columns_for(
                        &*adapter,
                        root.as_ref(),
                        &node_type,
                    )
                    .await;
                    let list = match (kind, query) {
                        (QueryKind::Extended, Some(document)) => {
                            run_extended_query(
                                &*adapter,
                                root.as_ref(),
                                node_type,
                                &document,
                                &vars,
                                &sort,
                                &columns,
                                group_by,
                                &tx,
                            )
                            .await?
                        }
                        (_, query) => {
                            let params = not_yet_done_content::ListParams {
                                node_type,
                                query,
                                sort,
                                page,
                                download: false,
                                group_by,
                            };
                            not_yet_done_content::children::list(&*adapter, root.as_ref(), params)
                                .await
                                .map_err(|e| e.to_string())?
                        }
                    };
                    Ok((list, columns))
                }
            })
            .await;
            match result {
                Ok((list, columns)) => {
                    let _ = tx.send(LoadMsg::ContentItems {
                        view_index,
                        pane_id,
                        items: list.items,
                        applied_sort: list.applied_sort,
                        page: list.page,
                        columns,
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
                        columns: Vec::new(),
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
            kind,
        } = req;
        // Eager pre-expansion asks the adapter for the *whole* tree in one
        // `list_subtree` call, which an extended document cannot be split
        // across: it is executed per level, against one parent at a time.
        // Skipping the pre-expansion costs only the eagerness — the tree still
        // opens level by level, and every level runs the document — whereas
        // passing the document down would have the adapter reject it, or worse,
        // load the subtree unfiltered.
        if kind == QueryKind::Extended && query.is_some() {
            return;
        }
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
                    let node_type =
                        not_yet_done_content::children::child_types(&*adapter, root.as_ref())
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
                    not_yet_done_content::children::list_subtree(
                        &*adapter,
                        root.as_ref(),
                        params,
                        depth,
                    )
                    .await
                    .map_err(|e| e.to_string())
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

    /// M9 now-bucket refresh: reload just the bucket the current instant falls
    /// into for one grouped-tree pane, in response to
    /// [`Invalidation::NowAnchored`](not_yet_done_content::Invalidation::NowAnchored).
    /// Returns without spawning unless the pane is a grouped *eager* tree — it
    /// needs both a `TreeState` (an active `group_by`) and an eager subtree
    /// depth; a flat/condensed pane has no per-bucket fold to localise and
    /// keeps the cheap full `Reload` on its toggle. Asks the adapter
    /// [`bucket_for_now`](not_yet_done_content::ContentAdapter::bucket_for_now)
    /// for the bucket id, fetches that one bucket node's refreshed header +
    /// re-folded subtree, and lands them via [`LoadMsg::NowBucketReload`] →
    /// [`ContentView::reload_now_bucket`]. Folds ONE bucket instead of the
    /// per-bucket fold a whole-forest reload would run for every group.
    /// Whether `pane` is a *grouped eager tree* — the only pane shape
    /// [`spawn_now_bucket_reload`] can localize (it needs both an active
    /// `group_by` and an eager subtree depth). Mirrors the early-return
    /// guards there so [`handle_adapter_invalidation`] can pick the
    /// localized bucket reload for trees and a full reload for every other
    /// shape on [`Invalidation::NowAnchored`].
    fn pane_is_grouped_eager_tree(
        &self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
    ) -> bool {
        let Some(cv) = self.content_view(view_index) else {
            return false;
        };
        let Some(pane) = cv.find_pane(pane_id) else {
            return false;
        };
        pane.adapter_group_spec(&cv.view_defs).is_some()
            && pane.eager_subtree_depth(&cv.view_defs).is_some()
    }

    pub fn spawn_now_bucket_reload(
        &self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
    ) {
        let Some(cv) = self.content_view(view_index) else {
            return;
        };
        let Some(adapter) = cv.adapter.as_ref() else {
            return;
        };
        let Some(pane) = cv.find_pane(pane_id) else {
            return;
        };
        let Some(spec) = pane.adapter_group_spec(&cv.view_defs) else {
            return;
        };
        let Some(depth) = pane.eager_subtree_depth(&cv.view_defs) else {
            return;
        };
        let adapter = Arc::clone(adapter);
        let subtree_query = Self::subtree_query_for_pane(cv, pane, &adapter);
        // The refresh re-reads the bucket's whole subtree in one
        // `list_subtree`, which an extended document cannot be split across —
        // the same reason the eager pre-expansion skips it. The bucket keeps
        // the rows it has until the user reloads.
        if subtree_query
            .as_ref()
            .is_some_and(|q| q.kind == QueryKind::Extended)
        {
            return;
        }
        let subtree_query = subtree_query.map(|q| q.text);
        let tx = self.load_tx.clone();
        tokio::spawn(async move {
            let payload = async {
                let bucket_id = adapter.bucket_for_now(&spec).await?;
                let node = adapter.get_by_id(&bucket_id).await.ok()?;
                // The bucket node's metadata already carries the refreshed
                // total + `⏱` marker (recomputed in `get_by_id`); lift it
                // straight into the header the splice swaps the bucket row for.
                let header = not_yet_done_content::NodeSummary {
                    id: node.id().to_string(),
                    label: node.label().to_string(),
                    node_type: node.node_type().clone(),
                    metadata: node.metadata().clone(),
                    has_children: Some(true),
                };
                let node_type =
                    not_yet_done_content::children::child_types(&*adapter, node.as_ref())
                        .into_iter()
                        .next()?;
                let params = not_yet_done_content::ListParams {
                    node_type,
                    query: subtree_query,
                    sort: Vec::new(),
                    page: None,
                    download: false,
                    group_by: None,
                };
                let subtree = not_yet_done_content::children::list_subtree(
                    &*adapter,
                    node.as_ref(),
                    params,
                    depth,
                )
                .await
                .ok()?;
                Some(NowBucketPayload { header, subtree })
            }
            .await;
            let _ = tx.send(LoadMsg::NowBucketReload {
                view_index,
                pane_id,
                result: payload,
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
    pub fn spawn_adapter_query(
        &self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        node_id: String,
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
            // The adapter derives its own routing keys from the node id
            // (for Postgres: the target database), so the host stays out
            // of the id's shape.
            let mut ctx = adapter.custom_query_context(&node_id);
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
                        node_id: node_id.clone(),
                        // Placeholder — the pane overrides this with its
                        // own view-config-derived mode in
                        // `apply_custom_query_result`.
                        mode: crate::config::view_config::PaginationMode::Server,
                        cursor_id: res.cursor_id.clone(),
                    },
                    status,
                }
            });
            let _ = tx.send(LoadMsg::CustomQueryItems {
                view_index,
                pane_id,
                result,
            });
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

    /// CP-8 entry point: a `<adapter>:db_script` row's `x` shortcut
    /// dispatched `ActionDispatch::ExecuteQuery { paged: true }`. Allocates
    /// (or reuses) the result pane child via the active level's first
    /// `ChildDef` (typically a synthetic `<adapter>:db_script_result` with
    /// `split: right` + a `pagination:` block), then spawns a paginated
    /// custom query against it — cursor-based or LIMIT/OFFSET, whichever
    /// that pane's `pagination: mode:` asks for.
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
        // Open a cursor only where the result pane asks for one. The
        // next/prev-page keys already derive this from the pane's
        // `pagination:` block; deciding it here the same way keeps the first
        // page consistent with the following ones — and keeps the host from
        // assuming every SQL backend has server-side cursors (SQLite has
        // none and rejects the intent outright).
        let cursor = match cv.pane_pagination_mode(target_pane_id) {
            crate::config::view_config::PaginationMode::Cursor => {
                Some(not_yet_done_content::CursorIntent::Open)
            }
            crate::config::view_config::PaginationMode::Server
            | crate::config::view_config::PaginationMode::All => None,
        };
        self.spawn_adapter_query(
            view_index,
            target_pane_id,
            database,
            sql,
            Some(not_yet_done_content::PageRequest {
                offset: 0,
                limit: crate::edit_session::ADAPTER_QUERY_DEFAULT_PAGE_SIZE,
            }),
            cursor,
        );
    }

    /// CP-8 entry point: a `postgres:db_script` row's `e` shortcut
    /// dispatched `ActionDispatch::OpenEditor { session_kind: "script_editor" }`.
    /// Opens [`AdapterDbScriptSession`] which writes the buffer back to
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
                crate::edit_session::AdapterDbScriptSession::open(
                    adapter, database, script, in_place,
                )
                .await
            })
        });
        self.open_session(Box::new(session))
    }

    /// DSF-4: stash the marked source for a subsequent move. Mirrors
    /// [`Self::marked_link`] UX — the status bar shows the indicator
    /// until paste or Esc clears it.
    fn mark_db_script_for_move(&mut self, node_id: String) {
        self.notify(format!(
            "Marked '{node_id}' for move — paste with `p` on the target dir"
        ));
        self.marked_db_script_for_move = Some(node_id);
    }

    /// DSF-4: paste the marked source into the target dir (or root
    /// group). Validates same-database, calls `move_db_script_entry`,
    /// reloads, and clears the mark.
    fn paste_db_script_move(&mut self, target_node_id: String) {
        use crate::app::node_actions::{db_script_rel_path_str, parse_db_script_node_id};
        let Some(source_node_id) = self.marked_db_script_for_move.clone() else {
            self.notify("No DB-script marked for move (use `m` first)".to_string());
            return;
        };
        let Some((src_db, src_segs)) = parse_db_script_node_id(&source_node_id) else {
            self.notify_error(format!(
                "Marked source '{source_node_id}' is not a DB-script id"
            ));
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
            self.notify_error(format!(
                "Marked source '{source_node_id}' has no name segment"
            ));
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
        let result: not_yet_done_content::Result<()> = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                match adapter.script_store() {
                    Some(store) => store.move_db_entry(&src_db, &src_rel, &dst_rel).await,
                    None => Err(not_yet_done_content::ContentError::NotSupported(
                        "adapter has no script store".into(),
                    )),
                }
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
                    self.modal_message = Some(":db-script new expects <name>".to_string());
                    return;
                }
                self.db_script_new_in_current_dir(rest);
            }
            "new-dir" => {
                if rest.is_empty() {
                    self.modal_message = Some(":db-script new-dir expects <name>".to_string());
                    return;
                }
                self.db_script_new_dir_in_current_dir(rest);
            }
            "rename" => {
                if rest.is_empty() {
                    self.modal_message = Some(":db-script rename expects <new-name>".to_string());
                    return;
                }
                self.db_script_rename_selected(rest);
            }
            "move" => {
                if rest.is_empty() {
                    self.modal_message =
                        Some(":db-script move expects <dest-dir> (use '/' for root)".to_string());
                    return;
                }
                self.db_script_move_selected_or_marked(rest);
            }
            "delete" => {
                if !rest.is_empty() {
                    self.modal_message = Some(":db-script delete takes no arguments".to_string());
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
            self.modal_message = Some(format!(":db-script {sub} — no row selected"));
            return None;
        };
        // Parse the id. If it doesn't look like a db-script id, bail.
        let Some((database, segments)) =
            crate::app::node_actions::parse_db_script_node_id(&selected_id)
        else {
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
        let rel_path = crate::app::node_actions::db_script_rel_path_str(&segments);
        // Filesystem probe to disambiguate dir vs script.
        let is_dir = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                match adapter.script_store() {
                    Some(store) => store.db_entry_is_dir(&database, &rel_path).await,
                    None => false,
                }
            })
        });
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

    /// Resolve the container node the cmdline create should act on: the
    /// db_scripts group node (`<db>/db_scripts`) when the cursor sits at the
    /// root, else the current directory node (`<db>/db_scripts/<rel>`). The
    /// `.sql` default, rel_path composition and name validation all live in
    /// the adapter's `execute`, so both the keyboard form and the cmdline
    /// share one write path (Phase 4.2).
    fn db_script_create_via_execute(&mut self, sub: &str, action_id: &str, name: &str) {
        let Some((view_index, pane_id, _adapter, database, current_dir_rel, _selected)) =
            self.resolve_db_script_context(sub)
        else {
            return;
        };
        let node_id = if current_dir_rel.is_empty() {
            format!("{database}/db_scripts")
        } else {
            format!("{database}/db_scripts/{current_dir_rel}")
        };
        let values = std::collections::HashMap::from([("name".to_string(), name.to_string())]);
        self.execute_content_action_form(
            view_index,
            pane_id,
            node_id,
            action_id.to_string(),
            values,
            None,
        );
    }

    fn db_script_new_in_current_dir(&mut self, name: &str) {
        self.db_script_create_via_execute("new", "add-script", name);
    }

    fn db_script_new_dir_in_current_dir(&mut self, name: &str) {
        self.db_script_create_via_execute("new-dir", "add-dir", name);
    }

    /// Rename the selected script/dir through the adapter's `rename` form
    /// action — the same `execute` write path the keyboard `r` and the CLI
    /// `--field name=…` drive (Phase 4.3). Name validation, extension
    /// preservation and the folder-vs-file distinction all live in the
    /// adapter, so the cmdline just resolves the selected node id and hands
    /// over the new name.
    fn db_script_rename_selected(&mut self, new_name: &str) {
        let Some((view_index, pane_id, _adapter, database, _current_dir, selected)) =
            self.resolve_db_script_context("rename")
        else {
            return;
        };
        let Some((rel_path, _is_dir)) = selected else {
            self.modal_message = Some(
                ":db-script rename — selected row is the group node, not an entry".to_string(),
            );
            return;
        };
        let node_id = format!("{database}/db_scripts/{rel_path}");
        let values = std::collections::HashMap::from([("name".to_string(), new_name.to_string())]);
        self.execute_content_action_form(
            view_index,
            pane_id,
            node_id,
            "rename".to_string(),
            values,
            None,
        );
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
            let Some((db, segs)) = crate::app::node_actions::parse_db_script_node_id(&marked)
            else {
                self.notify_error(format!("Marked source '{marked}' is not a DB-script id"));
                self.marked_db_script_for_move = None;
                return;
            };
            (db, crate::app::node_actions::db_script_rel_path_str(&segs))
        } else {
            let Some((rel, _)) = selected else {
                self.modal_message =
                    Some(":db-script move — no marked source and no entry selected".to_string());
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
        let result: not_yet_done_content::Result<()> = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                match adapter.script_store() {
                    Some(store) => store.move_db_entry(&database, &src_rel, &dst_rel).await,
                    None => Err(not_yet_done_content::ContentError::NotSupported(
                        "adapter has no script store".into(),
                    )),
                }
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
        let Some((view_index, pane_id, _adapter, _database, _current_dir, selected)) =
            self.resolve_db_script_context("delete")
        else {
            return;
        };
        let Some((_rel_path, is_dir)) = selected else {
            self.modal_message = Some(
                ":db-script delete — selected row is the group node, not an entry".to_string(),
            );
            return;
        };
        // The db_script(/_dir) nodes carry their own delete logic
        // (`Node::execute("delete" | "delete-dir")`), so route through the
        // generic adapter-executed path — identical to the keyboard `d`
        // action. No TUI-owned filesystem reach-in remains.
        let Some(node_id) = self
            .content_view(view_index)
            .and_then(|cv| cv.selected_item_id().map(str::to_string))
        else {
            return;
        };
        let action_name = if is_dir { "delete-dir" } else { "delete" };
        self.confirm_delete_content_node(
            view_index,
            pane_id,
            node_id,
            action_name.to_string(),
            None,
        );
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
        action_name: String,
        confirm: Option<String>,
    ) {
        // The adapter may supply its own prompt (e.g. a recursive-delete
        // warning when the node has children). Otherwise pull a
        // user-friendly label from the selected row — falls back to the
        // raw id if the pane / cursor moved. The pane label is set from
        // the adapter's `NodeSummary.label`, so confluence pages show as
        // the page title; postgres rows show their last segment; etc.
        let msg = confirm.unwrap_or_else(|| {
            let label = self
                .content_view(view_index)
                .and_then(|cv| cv.find_pane(pane_id))
                .and_then(|pane| pane.selected_item_label().map(str::to_string))
                .unwrap_or_else(|| node_id.clone());
            format!("Delete '{label}'? (y/n)")
        });
        self.modal_message = Some(msg.clone());
        self.pending_confirmation = Some((
            msg,
            PendingConfirmation::DeleteContentNode {
                view_index,
                pane_id,
                node_id,
                action_name,
            },
        ));
    }

    /// Stage the confirm popup for a generic confirm-then-invoke
    /// (`ActionDispatch::Confirm`). The prompt is adapter-authored — only
    /// the adapter knows what the action will do (e.g. how many successor
    /// intervals a restore purges). On accept the App re-invokes the same
    /// action on the same node with `confirmed: true`.
    fn confirm_invoke_node_action(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        node_id: String,
        action_name: String,
        prompt: String,
    ) {
        self.modal_message = Some(prompt.clone());
        self.pending_confirmation = Some((
            prompt,
            PendingConfirmation::InvokeNodeAction {
                view_index,
                pane_id,
                node_id,
                action_name,
            },
        ));
    }

    /// CF-11: spawn the actual `Node::execute(<action>, ActionInput::None)`
    /// roundtrip on the current pane's adapter. `action_name` is the delete
    /// action the user confirmed — usually `delete`, but the tasks flat list
    /// sends `delete-single` so it deletes one task non-recursively. On
    /// `ActionOutcome::Done` the result lands in `ContentActionDone`, which
    /// already notifies + reloads the pane.
    fn delete_content_node_now(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        node_id: String,
        action_name: String,
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
        let action_id = action_name;
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
            let _ = tx.send(LoadMsg::ContentNodeDeleted {
                view_index,
                pane_id,
                node_id,
                result,
            });
        });
    }

    /// Re-issue a tree-expand for every cached subtree whose immediate
    /// parent is either the `<database>/db_scripts` group node OR any
    /// script-directory node under it. Used by the create/delete/
    /// rename/move paths so a tree-mode pane that currently shows the
    /// scripts/folders under an expanded DB-Scripts row picks up the
    /// new on-disk state without a full reload.
    ///
    /// Multi-tree-continuation (MT-1, DSF-3): both the `db_script_dir`
    /// AND the `db_script` level are fanned out per parent so newly
    /// created folders show up alongside scripts — refreshing only the
    /// script bucket would leave new folders invisible until restart.
    ///
    /// The two node types are derived from the adapter's own
    /// `adapter_type()`: the `:db_script` / `:db_script_dir` suffixes are
    /// the shared contract of the script-store shape, so every adapter
    /// offering one gets this refresh without being named here.
    fn refresh_db_scripts_tree_children(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        database: &str,
    ) {
        let Some(adapter_type) = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(|a| a.adapter_type().to_string())
        else {
            return;
        };
        let group_id = format!("{database}/db_scripts");
        let sub_prefix = format!("{group_id}/");
        let paths: Vec<(Vec<String>, String)> = {
            let Some(cv) = self.content_view(view_index) else {
                return;
            };
            let Some(pane) = cv.find_pane(pane_id) else {
                return;
            };
            let Some(tree) = pane.tree.as_ref() else {
                return;
            };
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
            format!("{adapter_type}:db_script_dir"),
            format!("{adapter_type}:db_script"),
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
    ) -> Option<SubtreeQuery> {
        if !adapter.capabilities().propagates_query_to_subtree {
            return None;
        }
        let req = pane.root_load_request(&cv.view_defs)?;
        let text = req.query?;
        Some(match req.kind {
            QueryKind::Saved => SubtreeQuery {
                text: adapter.render_query(&text, &req.vars),
                kind: QueryKind::Saved,
                vars: std::collections::HashMap::new(),
            },
            QueryKind::Extended => SubtreeQuery {
                text,
                kind: QueryKind::Extended,
                vars: req.vars,
            },
        })
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
            .unwrap_or(not_yet_done_content::PageRequest {
                offset: 0,
                limit: 50,
            });
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
                let tx = tx.clone();
                async move {
                    let parent = adapter
                        .get_by_id(&node_id)
                        .await
                        .map_err(|e| e.to_string())?;
                    let node_type =
                        not_yet_done_content::children::child_types(&*adapter, parent.as_ref())
                            .into_iter()
                            .find(|t| t.type_id == child_node_type)
                            .ok_or_else(|| {
                                format!(
                                    "Node type '{child_node_type}' not available on '{node_id}'"
                                )
                            })?;
                    let columns = not_yet_done_content::children::columns_for(
                        &*adapter,
                        parent.as_ref(),
                        &node_type,
                    )
                    .await;
                    // An extended document re-runs whole, one level down: its
                    // branches list this parent instead of the root, which is
                    // what makes a drilled level filter by the same document
                    // as the level above it.
                    let list = match subtree_query {
                        Some(sq) if sq.kind == QueryKind::Extended => {
                            run_extended_query(
                                &*adapter,
                                parent.as_ref(),
                                node_type,
                                &sq.text,
                                &sq.vars,
                                &[],
                                &columns,
                                None,
                                &tx,
                            )
                            .await?
                        }
                        subtree_query => {
                            let params = not_yet_done_content::ListParams {
                                node_type,
                                query: subtree_query.map(|sq| sq.text),
                                sort: Vec::new(),
                                page: Some(page),
                                download: false,
                                group_by: None,
                            };
                            not_yet_done_content::children::list(&*adapter, parent.as_ref(), params)
                                .await
                                .map_err(|e| e.to_string())?
                        }
                    };
                    Ok((list, columns))
                }
            })
            .await;
            match result {
                Ok((list, columns)) => {
                    let _ = tx.send(LoadMsg::ContentItems {
                        view_index,
                        pane_id,
                        items: list.items,
                        applied_sort: list.applied_sort,
                        page: list.page,
                        columns,
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
                        columns: Vec::new(),
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
        // Carry the pane's active sort so a per-node (lazy / paginated)
        // expansion orders the freshly-fetched children the same way the
        // eager subtree does — each subtree's siblings sorted among
        // themselves. Empty = the adapter's stored order.
        let sort = cv
            .find_pane(pane_id)
            .and_then(|p| p.root_load_request(&cv.view_defs))
            .map(|r| r.sort)
            .unwrap_or_default();
        let tx = self.load_tx.clone();
        tokio::spawn(async move {
            let payload = run_with_retries(retries, &tx, view_index, pane_id, || {
                let adapter = Arc::clone(&adapter);
                let parent_node_id = parent_node_id.clone();
                let child_node_type = child_node_type.clone();
                let subtree_query = subtree_query.clone();
                let sort = sort.clone();
                let tx = tx.clone();
                async move {
                    let parent = adapter
                        .get_by_id(&parent_node_id)
                        .await
                        .map_err(|e| e.to_string())?;
                    let node_type = not_yet_done_content::children::child_types(
                        &*adapter,
                        parent.as_ref(),
                    )
                    .into_iter()
                    .find(|t| t.type_id == child_node_type)
                    .ok_or_else(|| {
                        format!("Node type '{child_node_type}' not available on '{parent_node_id}'")
                    })?;
                    let page_request = page.unwrap_or(not_yet_done_content::PageRequest {
                        offset: 0,
                        limit: page_size,
                    });
                    // See `spawn_content_drill_down`: an extended document
                    // runs again for this level, against this parent.
                    let list = match subtree_query {
                        Some(sq) if sq.kind == QueryKind::Extended => {
                            let columns = not_yet_done_content::children::columns_for(
                                &*adapter,
                                parent.as_ref(),
                                &node_type,
                            )
                            .await;
                            run_extended_query(
                                &*adapter,
                                parent.as_ref(),
                                node_type,
                                &sq.text,
                                &sq.vars,
                                &sort,
                                &columns,
                                None,
                                &tx,
                            )
                            .await?
                        }
                        subtree_query => {
                            let params = not_yet_done_content::ListParams {
                                node_type,
                                query: subtree_query.map(|sq| sq.text),
                                sort,
                                page: Some(page_request),
                                download: false,
                                group_by: None,
                            };
                            not_yet_done_content::children::list(&*adapter, parent.as_ref(), params)
                                .await
                                .map_err(|e| e.to_string())?
                        }
                    };
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

    /// A child-create action (`add`/`A`) succeeded; splice the new child into
    /// the pane *locally*, never via a full reload (reload is reserved for
    /// external changes — the user's `r`).
    ///
    /// **Tree pane:** resolve the parent's tree path, arm its expansion, and
    /// re-fetch ONLY that parent's children ([`Self::spawn_tree_expand`],
    /// `append=false`). The single `cache[parent_path]` slot is replaced;
    /// `expanded` and every sibling/descendant cache stay untouched, so the
    /// tree does not collapse and the cursor — selection is by row index —
    /// stays on the parent (children render below it). A parent that isn't a
    /// visible row is the adapter's synthetic root container (e.g.
    /// `task:root`), whose children are the root level → re-fetch under the
    /// empty path.
    ///
    /// **Flat/drill pane:** re-run the drill-down at the parent level — the
    /// historical create-refresh behaviour.
    pub fn insert_content_child(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        parent_node_id: String,
        child_node_type: String,
    ) {
        // Arm the parent's expansion and resolve its cache path while we hold
        // the mutable borrow; drop it before spawning the async load. `None`
        // marks a non-tree pane → drill-down fallback below.
        let parent_path: Option<Vec<String>> = {
            let Some(cv) = self.content_view_mut(view_index) else {
                return;
            };
            let Some(pane) = cv.find_pane_mut(pane_id) else {
                return;
            };
            match pane.tree.as_mut() {
                Some(tree) => {
                    let path = tree
                        .entries
                        .iter()
                        .find(|e| e.node.id == parent_node_id)
                        .map(|e| {
                            let mut p = e.parent_path.clone();
                            p.push(parent_node_id.clone());
                            p
                        })
                        // Parent isn't a rendered row → synthetic root
                        // container; its children are the root level.
                        .unwrap_or_default();
                    tree.expanded.insert(path.clone());
                    Some(path)
                }
                None => None,
            }
        };
        match parent_path {
            Some(parent_path) => self.spawn_tree_expand(
                view_index,
                pane_id,
                parent_path,
                parent_node_id,
                child_node_type,
                50,
                None,
                false,
            ),
            None => {
                self.spawn_content_drill_down(view_index, pane_id, parent_node_id, child_node_type)
            }
        }
    }

    /// Patch the edited node's row in place ([`ContentView::patch_row`])
    /// instead of full-reloading after an `edit`/notes save — reload stays
    /// reserved for external changes.
    ///
    /// Re-fetches the node's fresh `label`/`metadata`/`node_type` but keeps
    /// the currently-displayed row's `has_children`: an edit changes content,
    /// not structure, and a bare `Node` can't report its child count. Falls
    /// back to a pane reload when the row isn't visible (non-tree pane,
    /// scrolled-away) or the fetch fails — so non-tree adapters behave as
    /// before.
    pub async fn patch_content_row(
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
            self.reload_content_pane_current_level(view_index, pane_id);
            return;
        };
        // Base on the visible row so structural fields (has_children) survive;
        // overlay fresh content from the re-fetched node.
        let base = self
            .content_view(view_index)
            .and_then(|cv| cv.visible_summary(pane_id, &node_id));
        let fetched = adapter.get_by_id(&node_id).await;
        let (Some(mut summary), Ok(mut node)) = (base, fetched) else {
            self.reload_content_pane_current_level(view_index, pane_id);
            return;
        };
        // Lazy adapters (e.g. Jira) re-resolve to a stub whose label is the id
        // and whose metadata is sparse; fill in the real display fields before
        // copying them onto the row. No-op for eager adapters (Taiga, local).
        node.hydrate().await;
        // Refresh from the node's *list-row* projection, not its `metadata()`:
        // the latter is a detail projection whose key set diverges from the
        // list row (Jira) or is empty (Taiga), which would blank columns. Merge
        // by key so columns the detail can't carry (e.g. attachment counts) keep
        // the row's last-known value instead of clearing. See Node::row_summary.
        let fresh = node.row_summary();
        summary.label = fresh.label;
        summary.node_type = fresh.node_type;
        for field in &fresh.metadata.fields {
            summary.metadata.set_field(&field.key, field.value.clone());
        }
        let patched = self
            .content_view_mut(view_index)
            .map(|cv| cv.patch_row(&summary))
            .unwrap_or(false);
        if !patched {
            self.reload_content_pane_current_level(view_index, pane_id);
        }
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
    /// Consume a pane's queued `:tree-find` query (if any) and start it:
    /// stamp the loading state synchronously, then spawn the adapter
    /// search. No-op when nothing is queued.
    ///
    /// Fired from the `LoadMsg::ContentItems` handler for ordinary tree
    /// panes, but deferred to the `LoadMsg::Subtree` handler for
    /// eager-subtree panes so the expand-to-hit walk runs against the
    /// fully-ingested tree rather than racing the parallel subtree load
    /// (see the call sites for the full rationale).
    /// Opt-in tree-find pipeline tracing. Enable with `NYD_DEBUG_TREEFIND=1`;
    /// each stage appends one line to `$TMPDIR/nyd-treefind-debug.log`. Zero
    /// cost when the env var is unset.
    ///
    /// Added to diagnose the intermittent "a task created by an external
    /// process (`nyd-t task add` from a Jira script) isn't visible in the
    /// Tasks tree until the app restarts" report. The DB and reload paths are
    /// provably fresh (a cross-process visibility test confirms a long-lived
    /// connection sees the external insert), so the drop must be downstream —
    /// this trace pins *which* stage loses the node in a live occurrence:
    /// the fresh root reload (item count), the eager subtree ingest, or the
    /// expand-to-hit walk (`hit count == 0` ⇒ search/data; `> 0` but invisible
    /// ⇒ render/navigation).
    fn treefind_trace(stage: &str, detail: impl std::fmt::Display) {
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
            let _ = writeln!(f, "[treefind] {stage}: {detail}");
        }
    }

    fn fire_pending_tree_find(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
    ) {
        let pending = self
            .content_view_mut(view_index)
            .and_then(|cv| cv.find_pane_mut(pane_id))
            .and_then(|pane| pane.take_pending_tree_find());
        let Some(query) = pending else {
            Self::treefind_trace(
                "fire_pending",
                format!("view={view_index} — nothing queued (no-op)"),
            );
            return;
        };
        Self::treefind_trace("fire_pending", format!("view={view_index} query={query:?}"));
        if let Some(pane) = self
            .content_view_mut(view_index)
            .and_then(|cv| cv.find_pane_mut(pane_id))
        {
            pane.tree_find_begin(query.clone());
        }
        self.spawn_tree_find(view_index, pane_id, query, Self::TREE_FIND_DEFAULT_LIMIT);
    }

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
        let Some(cv) = self.content_view_mut(view_index) else {
            return;
        };
        let Some(msg) = cv.drive_tree_find(view_index, pane_id) else {
            return;
        };
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

    /// Fetch the inline pictures the last markdown render asked for.
    ///
    /// Every pane collects the image URLs it couldn't resolve from its cache
    /// (see [`crate::views::images::ImageStore`]); this drains those lists and
    /// starts one task per URL. The download goes through the *view's*
    /// adapter — an attachment usually sits behind the same authentication as
    /// the messages — and the decode+downscale runs on the blocking pool
    /// because it is pure CPU. The result comes back as
    /// [`LoadMsg::ImageDecoded`].
    ///
    /// Cheap to call every loop iteration: with nothing queued it is a walk
    /// over the panes draining empty vectors.
    pub fn pump_image_downloads(&mut self) {
        struct Job {
            view_index: usize,
            pane_id: crate::views::content_view::PaneId,
            adapter: std::sync::Arc<dyn not_yet_done_content::ContentAdapter>,
            url: String,
            max_height: u16,
            font: (u16, u16),
        }

        let mut jobs: Vec<Job> = Vec::new();
        let view_indices: Vec<usize> = self.content_views_indexed().map(|(i, _)| i).collect();
        for view_index in view_indices {
            let Some(cv) = self.content_view(view_index) else {
                continue;
            };
            let Some(adapter) = cv.adapter.clone() else {
                continue;
            };
            for pane_id in cv.all_pane_ids() {
                let Some(pane) = self
                    .content_view_mut(view_index)
                    .and_then(|cv| cv.find_pane_mut(pane_id))
                else {
                    continue;
                };
                let (max_height, font) = pane.image_decode_params();
                for url in pane.take_wanted_images() {
                    jobs.push(Job {
                        view_index,
                        pane_id,
                        adapter: adapter.clone(),
                        url,
                        max_height,
                        font,
                    });
                }
            }
        }

        for job in jobs {
            let tx = self.load_tx.clone();
            tokio::spawn(async move {
                let bytes = job.adapter.download_asset(&job.url).await.ok();
                let image = match bytes {
                    Some(bytes) => tokio::task::spawn_blocking(move || {
                        crate::views::images::ImageStore::decode_bytes(
                            &bytes,
                            job.max_height,
                            job.font,
                        )
                    })
                    .await
                    .ok()
                    .flatten(),
                    None => None,
                };
                let _ = tx.send(LoadMsg::ImageDecoded {
                    view_index: job.view_index,
                    pane_id: job.pane_id,
                    url: job.url,
                    image,
                });
            });
        }
    }

    /// Apply a single [`LoadMsg`] to App state, returning `true` when
    /// visible state may have changed. Split out from [`Self::poll_load`]
    /// so the event-driven (1b) `select!` loop can handle the one message
    /// its `load_rx.recv()` consumed before draining the rest with
    /// `poll_load`. See docs/decisions/0001-render-loop-dirty-gating.md.
    pub fn handle_load_msg(&mut self, msg: LoadMsg) -> bool {
        {
            match msg {
                LoadMsg::ContentColumnSchema {
                    view_index,
                    node_type,
                    schema,
                } => {
                    if let Some(cv) = self.content_view_mut(view_index) {
                        cv.record_column_schema(node_type, schema);
                        return true;
                    }
                    return false;
                }
                LoadMsg::ImageDecoded {
                    view_index,
                    pane_id,
                    url,
                    image,
                } => {
                    let Some(cv) = self.content_view_mut(view_index) else {
                        return false;
                    };
                    let Some(pane) = cv.find_pane_mut(pane_id) else {
                        return false;
                    };
                    if !pane.insert_decoded_image(&url, image) {
                        // Failed download: nothing new to show, and the URL
                        // is retired — no repaint needed.
                        return false;
                    }
                    // The picture now needs its reserved lines, which only a
                    // rebuild can produce.
                    cv.rebuild_pane_table(pane_id);
                    return true;
                }
                LoadMsg::ContentItems {
                    view_index,
                    pane_id,
                    items,
                    applied_sort,
                    page,
                    columns,
                    error,
                } => {
                    if let Some(err) = error.as_ref() {
                        not_yet_done_content::http_log::log_error("content_load", err);
                        self.last_error = Some(err.clone());
                    }
                    let item_count = items.len();
                    // Distinct node types present, captured before `items` is
                    // moved — used to fetch the backend-described column schema
                    // (3b) off-thread so its types can be merged into rendering.
                    let node_types = distinct_node_types(&items);
                    if let Some(cv) = self.content_view_mut(view_index) {
                        cv.set_items_for_pane(pane_id, items, applied_sort, page, columns, error);
                    }
                    self.refresh_column_schema(view_index, node_types);
                    // Eager tree (capability `supports_eager_subtree`): the
                    // root rows are in; pull the WHOLE expanded subtree in one
                    // `list_subtree` call instead of running the per-node
                    // cascade. This covers reload (r / Invalidation::All) too —
                    // the single eager load renews every level.
                    let eager_depth = self.content_view(view_index).and_then(|cv| {
                        cv.find_pane(pane_id)
                            .and_then(|p| p.eager_subtree_depth(&cv.view_defs))
                    });
                    Self::treefind_trace(
                        "content_items",
                        format!(
                            "view={view_index} root_rows={item_count} eager_depth={eager_depth:?}"
                        ),
                    );
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
                    //
                    // EXCEPTION — eager-subtree panes: the whole tree is
                    // still being ingested by the parallel `spawn_subtree_load`
                    // above. Firing the find here races that load: if the
                    // find's cursor-park (`advance_tree_find` →
                    // `set_selected`) lands *before* the subtree's
                    // `apply_subtree` → `rebuild_table`, that rebuild only
                    // re-clamps an out-of-bounds selection — it never
                    // re-anchors to the hit's node id — so the parked cursor
                    // ends up on a shifted row and the jump is silently lost.
                    // Intermittent because it hinges on which async load lands
                    // first. Defer the find to the `LoadMsg::Subtree` handler
                    // (after `apply_subtree`), so the walk always runs last
                    // against the fully-populated tree.
                    if eager_depth.is_none() {
                        self.fire_pending_tree_find(view_index, pane_id);
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
                LoadMsg::TreeChildren {
                    view_index,
                    pane_id,
                    parent_path,
                    result,
                    append,
                } => {
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
                LoadMsg::Subtree {
                    view_index,
                    pane_id,
                    parent_path,
                    result,
                } => {
                    if let Some(cv) = self.content_view_mut(view_index) {
                        if let Some(pane) = cv.find_pane_mut(pane_id) {
                            pane.retry_state = None;
                        }
                    }
                    match result {
                        Ok(subtree) => {
                            fn count_subtree(st: &not_yet_done_content::Subtree) -> usize {
                                st.items.len()
                                    + st.items
                                        .iter()
                                        .map(|n| count_subtree(&n.children))
                                        .sum::<usize>()
                            }
                            Self::treefind_trace(
                                "subtree",
                                format!(
                                    "view={view_index} top_level={} total_nodes={}",
                                    subtree.items.len(),
                                    count_subtree(&subtree)
                                ),
                            );
                            if let Some(cv) = self.content_view_mut(view_index) {
                                cv.apply_subtree(pane_id, parent_path, subtree);
                            }
                            // A `:tree-find` queued on this eager pane was held
                            // back in the `ContentItems` handler so it wouldn't
                            // race this subtree load — now that the whole tree
                            // is ingested, fire it against the settled cache.
                            self.fire_pending_tree_find(view_index, pane_id);
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
                            // load failing. The deferred tree-find still needs
                            // to fire — the walk drives the lazy expansion
                            // itself from here.
                            self.fire_pending_tree_find(view_index, pane_id);
                            self.drive_tree_auto_expand(view_index, pane_id);
                        }
                    }
                }
                LoadMsg::NowBucketReload {
                    view_index,
                    pane_id,
                    result,
                } => {
                    // `None` = the now-bucket couldn't be resolved (no
                    // trackings / a load error) — leave the pane as-is.
                    if let Some(payload) = result {
                        let spliced = self
                            .content_view_mut(view_index)
                            .map(|cv| {
                                cv.reload_now_bucket(pane_id, payload.header, payload.subtree)
                            })
                            .unwrap_or(false);
                        // A *start* can mint a brand-new bucket (the task's
                        // first booking of the period) that isn't a visible
                        // row yet — the splice can't graft onto a row that
                        // doesn't exist, so fall back to a full pane reload to
                        // surface the new bucket in sorted position.
                        if !spliced {
                            self.reload_content_pane_current_level(view_index, pane_id);
                        }
                    }
                }
                LoadMsg::LiveTick { view_index } => {
                    // A background tab's tick must not touch the visible tab:
                    // only the active view folds now; others record themselves
                    // as due and run one coalesced refresh on switch-back. The
                    // fold itself spawns async and produces no immediate visible
                    // change (its row patches arrive as later messages), so this
                    // arm never marks the frame dirty — returning `false` keeps a
                    // background tick from redrawing the active tab.
                    if self.active_tab == Tab::Content(view_index) {
                        self.spawn_live_refresh(view_index);
                    } else {
                        self.pending_live_refresh.insert(view_index);
                    }
                    return false;
                }
                LoadMsg::CustomQueryItems {
                    view_index,
                    pane_id,
                    result,
                } => match result {
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
                },
                LoadMsg::ContentPreview {
                    view_index,
                    pane_id,
                    cache_key,
                    text,
                } => {
                    if let Some(cv) = self.content_view_mut(view_index) {
                        cv.set_preview_description_for_pane(pane_id, &cache_key, text);
                    }
                }
                LoadMsg::EditorSessionReady {
                    node_id,
                    token,
                    result,
                } => {
                    // A newer open (higher token) or a cancel superseded this
                    // one — it owns the loading state now, so drop this stale
                    // session without touching the indicator.
                    if token != self.editor_load_token {
                        return true;
                    }
                    if std::mem::take(&mut self.editor_loading) {
                        self.notification_bar.clear_keyed(EDITOR_LOADING_SLOT);
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
                LoadMsg::ContentActionDone {
                    view_index,
                    pane_id,
                    result,
                } => {
                    let ok = result.is_ok();
                    match result {
                        Ok(msg) => self.notify(msg),
                        Err(msg) => {
                            self.set_query_error(Some(msg.clone()));
                            self.notification_bar.push(msg);
                        }
                    }
                    self.reload_content_pane_current_level(view_index, pane_id);
                    // A successful mutation in one subtab can change what a
                    // sibling subtab lists (e.g. bookmarking here vs. the
                    // bookmarks subtab) — invalidate the siblings so they
                    // reload on next switch instead of showing a stale row.
                    if ok {
                        if let Some(cv) = self.content_view_mut(view_index) {
                            cv.invalidate_sibling_subtabs();
                        }
                    }
                }
                LoadMsg::OpenContentEditorForAction {
                    view_index,
                    pane_id,
                    node_id,
                    action_id,
                    label,
                } => {
                    self.open_content_editor(
                        view_index, pane_id, node_id, action_id, label, None, false,
                    );
                }
                LoadMsg::ContentOpenExternal { target, message } => {
                    if let Some(msg) = message {
                        self.notify(msg);
                    }
                    self.open_external(&target);
                }
                LoadMsg::OptionMenuItems {
                    view_index,
                    pane_id,
                    items,
                    selected_values,
                    error,
                    notice,
                    reload_pane,
                } => {
                    self.open_option_menu_popup(
                        view_index,
                        pane_id,
                        items,
                        selected_values,
                        error,
                        notice,
                        reload_pane,
                    );
                }
                LoadMsg::ContentNodeDeleted {
                    view_index,
                    pane_id,
                    node_id,
                    result,
                } => {
                    match result {
                        Ok(msg) => {
                            self.notify(msg);
                            // Remove the row locally; only full-reload when
                            // the pane has no tree (flat/drill) or can't
                            // locate the row — never for a successful tree
                            // delete (reload is for external changes).
                            let removed = self
                                .content_view_mut(view_index)
                                .map(|cv| cv.remove_tree_node(pane_id, &node_id))
                                .unwrap_or(false);
                            if !removed {
                                self.reload_content_pane_current_level(view_index, pane_id);
                            }
                        }
                        Err(msg) => {
                            // Delete failed → nothing changed; surface the
                            // error but leave the tree (and selection) as-is.
                            self.set_query_error(Some(msg.clone()));
                            self.notification_bar.push(msg);
                        }
                    }
                }
                LoadMsg::NodeActionDispatched {
                    view_index,
                    pane_id,
                    node_id,
                    action_name,
                    result,
                    node_label,
                    node_type,
                } => {
                    self.handle_node_action_dispatched(
                        view_index,
                        pane_id,
                        node_id,
                        action_name,
                        result,
                        node_label,
                        node_type,
                    );
                }
                LoadMsg::OpenContentActionPopup {
                    view_index,
                    pane_id,
                    node_id,
                    action_id,
                } => {
                    self.open_content_action_popup(view_index, pane_id, node_id, action_id);
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
                LoadMsg::AdapterReminder {
                    view_index,
                    reminder,
                } => {
                    self.handle_adapter_reminder(view_index, reminder);
                }
                LoadMsg::AdapterPrompt { request } => {
                    self.open_adapter_prompt(request);
                }
                LoadMsg::BusEvent { event } => {
                    self.handle_bus_event(event);
                }
                LoadMsg::Notify { text } => {
                    self.notify(text);
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
                LoadMsg::TreeFindResult {
                    view_index,
                    pane_id,
                    query,
                    result,
                } => {
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
                        let Some(cv) = self.content_view_mut(view_index) else {
                            return true;
                        };
                        let Some(pane) = cv.find_pane_mut(pane_id) else {
                            return true;
                        };
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
                    Self::treefind_trace(
                        "find_result",
                        format!(
                            "query={query:?} {outcome_summary}",
                            outcome_summary = match &outcome {
                                Outcome::Stale => "stale (query changed, dropped)".to_string(),
                                Outcome::Landed { count, truncated } =>
                                    format!("landed hits={count} truncated={truncated}"),
                                Outcome::Unsupported =>
                                    "unsupported (adapter has no tree search)".to_string(),
                                Outcome::Failed(e) => format!("failed: {e}"),
                            }
                        ),
                    );
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
                            self.notify_error("Adapter doesn't support tree search.".to_string());
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
        // The builtin editor pane owns the keyboard while it is open —
        // ahead of even the global quit binding, which would otherwise end
        // the app on a plain `q` typed into the buffer. Leaving is `:q!`,
        // and then quit works again. A modal message is the one exception:
        // it is drawn on top of the pane, so its dismissing keypress is an
        // answer to the modal rather than input for the buffer.
        if self.builtin_editor.is_some() && self.modal_message.is_none() {
            let req = self.handle_builtin_editor_key(key);
            self.sync_components();
            return req;
        }

        // Quit always works, regardless of mode/popups.
        if self
            .keybindings
            .global
            .bindings
            .get(&GlobalAction::Quit)
            .map_or(false, |b| b.matches(key))
        {
            self.should_quit = true;
            return EditorRequest::None;
        }

        // Modal message: dismiss on any key (but not when awaiting shortcut/confirm).
        if self.modal_message.is_some()
            && self.awaiting_favorite_shortcut.is_none()
            && self.awaiting_node_script_shortcut.is_none()
            && self.awaiting_script_shortcut.is_none()
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

        // Adapter prompt overlay (e.g. MFA during an interactive sign-in) —
        // global and tab-agnostic: while shown it intercepts every key. The
        // popup owns its own input collection (acknowledge / embedded form) and
        // sends the answer back to the raising operation on close; we then
        // promote any queued prompt.
        if self.adapter_prompt_popup.is_some() {
            let popup = self.adapter_prompt_popup.as_mut().unwrap();
            if let PromptKeyOutcome::Closed = popup.handle_key(key) {
                self.adapter_prompt_popup = None;
                self.advance_adapter_prompt_queue();
            }
            self.sync_components();
            return EditorRequest::None;
        }

        // Postgres script shortcut capture mode.
        if let Some(coords) = self.awaiting_node_script_shortcut.take() {
            self.modal_message = None;
            if key == "esc" {
                // Cancelled.
            } else if self.is_shortcut_taken(key) {
                self.modal_message = Some(format!(
                    "Shortcut '{}' is already taken!\n\nPress another key for '{}'\nEsc to cancel",
                    key, coords.script
                ));
                self.awaiting_node_script_shortcut = Some(coords);
            } else {
                let chord = key.to_string();
                let script_label = coords.script.clone();
                self.bind_node_script_shortcut(coords, &chord);
                self.modal_message =
                    Some(format!("Script '{}' bound to [{}]", script_label, chord));
            }
            self.sync_components();
            return EditorRequest::None;
        }

        // `:script`-menu shortcut capture mode.
        if let Some(coords) = self.awaiting_script_shortcut.take() {
            self.modal_message = None;
            if key == "esc" {
                // Cancelled.
            } else if let Some(conflict) = self
                .content_view(coords.view_index)
                .and_then(|cv| cv.script_shortcut_conflict(&self.keybindings, &coords.name, key))
            {
                self.modal_message = Some(format!(
                    "Shortcut '{}' is already taken by {}!\n\nPress another key for '{}'\nEsc to cancel",
                    key, conflict, coords.name
                ));
                self.awaiting_script_shortcut = Some(coords);
            } else {
                let chord = key.to_string();
                let script_label = coords.name.clone();
                self.bind_script_shortcut(coords, &chord);
                self.modal_message =
                    Some(format!("Script '{}' bound to [{}]", script_label, chord));
            }
            self.sync_components();
            return EditorRequest::None;
        }

        // Favorite shortcut capture mode.
        if let Some(pending) = self.awaiting_favorite_shortcut.take() {
            self.modal_message = None;
            if key == "esc" {
                // Cancelled — no modal needed.
            } else if let Some(conflict) =
                self.favorite_shortcut_conflict(&pending.scope, &pending.name, key)
            {
                // Show error and re-prompt.
                self.modal_message = Some(format!(
                    "Shortcut '{}' is already taken by {}!\n\nPress another key for '{}'\nEsc to cancel",
                    key, conflict, pending.name
                ));
                self.awaiting_favorite_shortcut = Some(pending);
            } else {
                let name = pending.name.clone();
                match self.add_favorite(pending, key.to_string()) {
                    Ok(()) => {
                        self.modal_message =
                            Some(format!("Favorite '{}' added with shortcut [{}]", name, key));
                    }
                    Err(e) => {
                        self.modal_message =
                            Some(format!("Could not add favorite '{}': {}", name, e));
                    }
                }
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

        // Link-hop: intercept all keys while link labels are showing. Esc
        // cancels; a single label char either narrows (Pending), resolves to
        // a URL (Picked → open in browser, detached), or misses (NoMatch).
        if self.active_table_mut().is_some_and(|t| t.link_hop_active()) {
            if key == "esc" {
                if let Some(table) = self.active_table_mut() {
                    table.link_hop_close();
                }
            } else if key.chars().count() == 1 && !key.chars().next().unwrap().is_control() {
                let ch = key.chars().next().unwrap();
                let outcome = self
                    .active_table_mut()
                    .map(|t| t.link_hop_input(ch))
                    .unwrap_or(not_yet_done_ratatui::LinkHopOutcome::NoMatch);
                match outcome {
                    not_yet_done_ratatui::LinkHopOutcome::Picked(url) => {
                        if let Some(table) = self.active_table_mut() {
                            table.link_hop_close();
                        }
                        if crate::views::link_extract::is_image_url(&url) {
                            self.open_image_link(&url);
                        } else {
                            self.open_link_in_browser(&url);
                        }
                    }
                    not_yet_done_ratatui::LinkHopOutcome::NoMatch => {
                        if let Some(table) = self.active_table_mut() {
                            table.link_hop_close();
                        }
                    }
                    not_yet_done_ratatui::LinkHopOutcome::Pending => {}
                }
            }
            self.sync_components();
            return EditorRequest::None;
        }

        // Command line mode: delegate to active view's CmdlineComponent.
        {
            use crate::views::{CmdlineKeyResult, HasCmdline};
            let cmdline_active = match self.active_tab {
                Tab::Content(idx) => self
                    .content_view(idx)
                    .map(|cv| cv.cmdline_active())
                    .unwrap_or(false),
            };
            if cmdline_active {
                let result = match self.active_tab {
                    Tab::Content(idx) => self
                        .content_view_mut(idx)
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
            use crate::views::SearchKeyResult;
            let Tab::Content(_) = self.active_tab;
            let search_active = false;
            if search_active {
                let result = match self.active_tab {
                    Tab::Content(_) => SearchKeyResult::Cancelled,
                };
                match result {
                    SearchKeyResult::Accepted
                    | SearchKeyResult::Cancelled
                    | SearchKeyResult::QueryChanged
                    | SearchKeyResult::Handled => {}
                }
                self.sync_components();
                return EditorRequest::None;
            }
        }

        // Chord handling: if a pending key exists, try to complete the chord.
        if let Some(pending) = self.pending_key.take() {
            // Steps are joined with a space, not concatenated: this is the
            // canonical multi-step surface form, so a modifier-bearing step
            // (`ctrl+k l`) reassembles correctly. Legacy single-char chords
            // are unaffected — `binding_steps` normalises `"z r"` and `"zr"`
            // to the same `[z, r]`.
            let chord = format!("{pending} {key}");
            // Check if the chord matches any binding.
            // Global comes first so chords like `gl` land here regardless
            // of the active tab.
            let global_chord = self
                .keybindings
                .global
                .bindings
                .iter()
                .find(|(_, b)| b.matches(&chord))
                .map(|(a, _)| a.clone());
            if let Some(action) = global_chord {
                let _ = self.handle_global_action(action);
                self.sync_components();
                return EditorRequest::None;
            }
            let common_chord = self
                .keybindings
                .common
                .bindings
                .iter()
                .find(|(_, b)| b.matches(&chord))
                .map(|(a, _)| a.clone());
            if let Some(action) = common_chord {
                let _ = self.handle_common_action(action);
                self.sync_components();
                return EditorRequest::None;
            }
            // Content-tab chords (e.g. `zm` → TreeCollapseAll). Route
            // through the active ContentView's central dispatcher so the
            // tree-mode guard + drill post-processing match the single-
            // key path.
            let content_chord = self
                .keybindings
                .content
                .bindings
                .iter()
                .find(|(_, b)| b.matches(&chord))
                .map(|(a, _)| a.clone());
            if let Some(action) = content_chord {
                let Tab::Content(idx) = self.active_tab;
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
                self.sync_components();
                return EditorRequest::None;
            }
            // Content-tab YAML `actions:` chords (e.g. `al` → new
            // channel). These keys live in the ContentView's pane/view
            // keymaps, not the typed `content.*` section above, so route
            // the assembled chord through the active ContentView. A clean
            // miss returns `Unhandled` and falls through to the remaining
            // chord resolution below.
            {
                let Tab::Content(idx) = self.active_tab;
                let msg = self
                    .content_view_mut(idx)
                    .map(|cv| cv.handle_key(&chord))
                    .filter(|m| !matches!(m, SubViewMessage::Unhandled));
                if let Some(msg) = msg {
                    let _ = self.process_sub_view_message(msg);
                    self.drain_content_cursor_closes(idx);
                    self.sync_components();
                    return EditorRequest::None;
                }
            }
            // Chord matches a user-defined cmdline shortcut?
            // (`cmdline_shortcuts:` in tui.yaml; the default ships
            // `mc`/`mp` for cut/paste-node.)
            if let Some(cmd) = self.cmdline_shortcut_for_chord(&chord) {
                self.execute_cmdline(&cmd);
                self.sync_components();
                return EditorRequest::None;
            }
            // Chord completes a (rebound) tab-switch key, e.g. `tab.key:
            // "ctrl+k t"`. Plain digit tab keys are single-step and never
            // reach here.
            if let Some(tab) = self.tab_for_pressed(&chord) {
                self.set_active_tab(tab);
                self.sync_components();
                return EditorRequest::None;
            }
            // Chord didn't match — but if the accumulated chord is itself
            // a prefix of an even longer binding (e.g. `gl` → `glm`/`glp`),
            // keep stashing so the next key can complete it. Without this
            // branch the dispatcher would top out at 2-char chords.
            if self
                .keybindings
                .global
                .bindings
                .values()
                .any(|b| b.is_prefix(&chord))
                || self
                    .keybindings
                    .common
                    .bindings
                    .values()
                    .any(|b| b.is_prefix(&chord))
                || self
                    .keybindings
                    .content
                    .bindings
                    .values()
                    .any(|b| b.is_prefix(&chord))
                || self.cmdline_shortcut_chord_prefix(&chord)
                || self.tab_switch_is_prefix(&chord)
                || matches!(self.active_tab, Tab::Content(idx)
                    if self.content_view(idx).is_some_and(|cv| cv.yaml_action_chord_prefix(&chord)))
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

        // Shortcut menu intercepts all keys while open.
        if self.shortcut_menu.is_open() {
            self.handle_shortcut_menu_key(key);
            self.sync_components();
            return EditorRequest::None;
        }

        // Generic option menu (a `type: option_menu` action) intercepts keys
        // while open.
        if self.option_menu.is_open() {
            self.handle_option_menu_key(key);
            self.sync_components();
            return EditorRequest::None;
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
                    // Closing the popup is not enough: the adapter's
                    // login waits for an answer, holding the auth lock,
                    // so it has to be told the form is gone.
                    let view_index = popup.view_index();
                    self.adapter_creds_popup = None;
                    self.spawn_cancel_credentials(view_index);
                }
                CredsKeyOutcome::Submit { values } => {
                    let view_index = popup.view_index();
                    self.spawn_submit_credentials(view_index, values);
                }
                CredsKeyOutcome::Consumed => {}
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
                    let target = self.query_var_popup.as_ref().map(|p| p.target().clone());
                    self.query_var_popup = None;
                    if let Some(target) = target {
                        self.apply_query_with_vars(target, values);
                    }
                }
                QueryVarKeyOutcome::Consumed => {}
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

        // Sort menu intercepts all keys. Nothing is applied until Enter —
        // an adapter-side sort would otherwise reload per keystroke.
        if let Some(popup) = &mut self.sort_menu_popup {
            let outcome = popup.handle_key(key, &self.keybindings);
            match outcome {
                SortMenuOutcome::Consumed => {}
                SortMenuOutcome::Cancelled => self.sort_menu_popup = None,
                SortMenuOutcome::Applied(sort) => {
                    self.sort_menu_popup = None;
                    self.apply_sort_spec(sort);
                }
            }
            self.sync_components();
            return EditorRequest::None;
        }

        // Content file-picker popup (e.g. Taiga attachment upload) —
        // intercepts every key while open. The picker handles its own
        // Esc/submit via `FilePickerEvent`.
        if matches!(self.active_tab, Tab::Content(_)) && self.content_file_picker_popup.is_some() {
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
                        popup.column_types,
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
        {
            let Tab::Content(idx) = self.active_tab;
            // SQ-8d: pre-fill the Postgres-table shortcut cache for the
            // currently-focused table so `build_view_claims` can register
            // global apply-on-chord handlers for them. No-op when the
            // focus isn't on a Postgres table or the cache is already
            // populated for that table.
            self.ensure_node_script_shortcuts_loaded(idx);
            // Same idea for `:script`-menu shortcuts: pre-fill the cache for
            // the focused level so `build_view_claims` registers run-on-chord
            // handlers. No-op off a script-capable level / when cached.
            self.ensure_script_shortcuts_loaded(idx);
            if let Some(cv) = self.content_view_mut(idx) {
                let msg = cv.handle_key(key);
                // A `mark_read_on_reach_end` hook may have armed during the
                // keypress (cursor reached the newest unread row). Drain it
                // here so it dispatches alongside the key's own message,
                // keeping the selection-changed side effects intact.
                let mark_read = cv.take_pending_mark_read();
                match msg {
                    SubViewMessage::Unhandled => {
                        // Fall through to global/chords. Still drain any
                        // cursor closes the view queued during dispatch.
                        if let Some(req) = mark_read {
                            self.process_view_request(req);
                        }
                        self.drain_content_cursor_closes(idx);
                    }
                    other => {
                        let result = self.process_sub_view_message(other);
                        if let Some(req) = mark_read {
                            self.process_view_request(req);
                        }
                        self.drain_content_cursor_closes(idx);
                        self.sync_components();
                        return result;
                    }
                }
            }
        }

        let mode = action::input_mode(
            self.script_menu.is_open(),
            false, // trackings uses its own fuzzy path
            false,
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
            let prefix_global = self
                .keybindings
                .global
                .bindings
                .values()
                .any(|b| b.is_prefix(key));
            let prefix_common = self
                .keybindings
                .common
                .bindings
                .values()
                .any(|b| b.is_prefix(key));
            let prefix_tab = match self.active_tab {
                // Content chords (e.g. `zm` → TreeCollapseAll) live in
                // `content.bindings`. Without this, `z` would never be
                // stashed as a pending key on a Content tab and the chord
                // would silently break into two single-key dispatches.
                // YAML `actions:` chords (e.g. `al` → new channel) live in
                // the ContentView's own keymaps instead, so consult it too.
                Tab::Content(idx) => {
                    self.keybindings
                        .content
                        .bindings
                        .values()
                        .any(|b| b.is_prefix(key))
                        || self
                            .content_view(idx)
                            .is_some_and(|cv| cv.yaml_action_chord_prefix(key))
                }
            };
            let prefix_cmdline = self.cmdline_shortcut_chord_prefix(key);
            let prefix_tab_switch = self.tab_switch_is_prefix(key);
            if prefix_global || prefix_common || prefix_tab || prefix_cmdline || prefix_tab_switch {
                self.pending_key = Some(key.to_string());
                return EditorRequest::None;
            }
        }

        // Tab switch: resolve the pressed key against each visible tab's
        // effective switch binding (its `tab.key` override, else the
        // autonumber digit). Resolved here at global priority — after view
        // delegation, so a view that binds the same key on its own tab (or
        // an open form consuming the input) still wins. Additionally, the
        // visible tabs own *every* plain digit (`1`..`9`, then `0`): an
        // unmapped digit is swallowed so it can't fall through to another
        // single-key action, keeping the digit namespace reserved.
        if mode == action::InputMode::Normal {
            if let Some(tab) = self.tab_for_pressed(key) {
                self.set_active_tab(tab);
                self.sync_components();
                return EditorRequest::None;
            }
            let is_plain_digit =
                key.len() == 1 && key.chars().next().is_some_and(|c| c.is_ascii_digit());
            if is_plain_digit {
                self.sync_components();
                return EditorRequest::None;
            }
        }

        let action = action::resolve_key(key, mode, &self.keybindings, false);

        // If the action is Noop, try favorites then cmdline shortcuts.
        // Chord-prefix detection has already run above.
        if action == Action::Noop && mode == action::InputMode::Normal {
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
        match action {
            Action::Global(g) => self.handle_global_action(g),
            Action::Common(c) => self.handle_common_action(c),
            Action::Content(_) | Action::Window(_) | Action::QueryMenu(_) => {
                // Not produced by `resolve_key` (these reach the App via the
                // chain interceptor in Phase 2). Routed centrally through
                // `dispatch_chained_action` once that path lands; until then
                // they're a no-op so the match stays exhaustive.
                let _ = self.dispatch_chained_action(action.clone());
                EditorRequest::None
            }
            Action::Escape => {
                self.dispatch_escape();
                EditorRequest::None
            }
            // Text-input / form actions are only produced in Popup / Fuzzy /
            // FilterForm input modes, which every active popup intercepts
            // (and returns) before this dispatch path runs — so they never
            // reach here. Listed explicitly to keep the match exhaustive.
            Action::InsertChar(_)
            | Action::Backspace
            | Action::CursorLeft
            | Action::CursorRight
            | Action::Submit
            | Action::Toggle
            | Action::Reset
            | Action::Form(_)
            | Action::Blocked
            | Action::Noop => EditorRequest::None,
        }
    }

    /// Look up an action chain for `key`, walking ChildDef → ViewDef →
    /// global. Returns `Some(Some(chain))` to run, `Some(None)` when
    /// disabled at a scope (consume key without running anything), and
    /// `None` when no scope defines the binding (caller should fall
    /// through to ordinary key handling).
    fn resolve_action_chain(&self, key: &str) -> Option<Option<Vec<Action>>> {
        let mut scopes: Vec<&crate::action::ActionChains> = Vec::new();
        let Tab::Content(idx) = self.active_tab;
        if let Some(cv) = self.content_view(idx) {
            scopes.extend(cv.action_chain_scopes());
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
            || self.sort_menu_popup.is_some()
            || self.adapter_creds_popup.is_some()
            || self.query_var_popup.is_some()
            || self.content_action_popup.is_some()
            || self.content_file_picker_popup.is_some()
            || self.content_form_popup.is_some()
            || self.link_popup.is_some()
            || self.config_picker_popup.is_some()
            || self.shortcut_menu.is_open()
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
                let Tab::Content(idx) = self.active_tab;
                let Some(cv) = self.content_view_mut(idx) else {
                    return Err(format!("window.{w} on broken content tab"));
                };
                let msg = cv.dispatch_window_action(w);
                self.process_sub_view_message(msg);
                self.drain_content_cursor_closes(idx);
                Ok(())
            }
            Action::Content(c) => {
                let Tab::Content(idx) = self.active_tab;
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

    // -----------------------------------------------------------------------
    // Action handlers
    // -----------------------------------------------------------------------

    fn handle_global_action(&mut self, action: GlobalAction) -> EditorRequest {
        match action {
            GlobalAction::Quit => self.should_quit = true,
            GlobalAction::TabNext => {
                self.set_active_tab(self.tab_layout.next(self.active_tab));
            }
            GlobalAction::TabPrev => {
                self.set_active_tab(self.tab_layout.prev(self.active_tab));
            }
            GlobalAction::SubtabNext => return self.cycle_active_subtab(true),
            GlobalAction::SubtabPrev => return self.cycle_active_subtab(false),
            GlobalAction::DismissNotifications => self.dismiss_notifications(),
            GlobalAction::ShowNotifications => return self.open_notifications_editor(),
            GlobalAction::ShowLastError => return self.open_last_error_editor(),
            GlobalAction::ShortcutMenu => self.open_shortcut_menu(),
            GlobalAction::ToggleFullscreen => self.fullscreen = !self.fullscreen,
            GlobalAction::LinkMark => self.link_mark_current(),
            GlobalAction::LinkPaste => self.link_paste_current(),
            GlobalAction::LinkOpenPopup => self.link_open_popup(),
            GlobalAction::LinkJumpBack => self.link_jump_back(),
            GlobalAction::LinkJumpForward => self.link_jump_forward(),
        }
        EditorRequest::None
    }

    /// Cycle the active content tab's subtab forward/backward, routing the
    /// resulting [`SubViewMessage`] through the same handler as the per-view
    /// YAML switch keys (so a fresh subtab auto-loads). A no-op when the tab
    /// has a single view.
    fn cycle_active_subtab(&mut self, forward: bool) -> EditorRequest {
        let Tab::Content(idx) = self.active_tab;
        let msg = self
            .content_view_mut(idx)
            .and_then(|cv| cv.cycle_subtab(forward));
        match msg {
            Some(m) => self.process_sub_view_message(m),
            None => EditorRequest::None,
        }
    }

    /// Open the shortcut menu (default `ctrl+y`).
    ///
    /// Collects two row sets: the *context* rows from the focused pane's
    /// live keymap (exactly what would fire right now), and the *all* rows
    /// projected from every content tab's leaf maps. The menu opens in the
    /// configured [`crate::config::ShortcutScope`].
    fn open_shortcut_menu(&mut self) {
        use crate::keymap::{
            KeyScope, ShortcutRow, build_leaf_maps_for, leaf_scope_label, shortcut_rows_with,
        };

        // Generic per-tab switch rows built from the active layout, so
        // every configured tab is listed by its real name.
        let switch_rows = self.tab_switch_rows();

        // Context: the live keymap of the currently focused content view,
        // plus its keyless actions, prefixed with the generic tab switches.
        let Tab::Content(idx) = self.active_tab;
        let mut context: Vec<ShortcutRow> = switch_rows.clone();
        context.extend(
            self.content_view(idx)
                .map(|cv| cv.context_shortcut_rows())
                .unwrap_or_default(),
        );

        // All: every configured shortcut across all content tabs and levels.
        let kb = &self.config.keybindings;
        let mut all: Vec<ShortcutRow> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for row in switch_rows {
            if seen.insert((row.scope.clone(), row.name.clone(), row.keys.clone())) {
                all.push(row);
            }
        }
        for cv in self.content_views_iter() {
            // When a tab has several subtabs (views), fold the subtab name into
            // the scope path so pane-scoped shortcuts that flatten to the same
            // `Pane(tab, profile)` scope — e.g. Jira's `tickets` vs `bookmarks`
            // both offering `fuzzy filter` — become distinct, individually
            // labelled rows (`Jira › tickets › fuzzy filter`) instead of two
            // indistinguishable `Jira › fuzzy filter` entries. Mirrors the same
            // folding in `all_node_shortcut_rows`.
            let multi_subtab = cv.view_defs.len() > 1;
            for leaf in build_leaf_maps_for(&cv.tab_name, &cv.view_defs, kb) {
                // Tag each row with the scope the shortcut is actually active
                // in — not the leaf it was enumerated in. Global and tab-wide
                // claims live in every leaf, so labelling by leaf made them
                // survive dedup once per drilldown level. Labelling by their
                // real scope collapses them to a single row.
                let rows = shortcut_rows_with(&leaf.keymap, |claim| match &claim.scope {
                    KeyScope::Global => "Global".to_string(),
                    KeyScope::Tab(_) => cv.tab_name.clone(),
                    KeyScope::Pane(_, _) => match claim.source.subtab_view() {
                        Some(view) if multi_subtab => {
                            let mut parts = Vec::with_capacity(leaf.child_path.len() + 1);
                            parts.push(view.to_string());
                            parts.extend(leaf.child_path.iter().cloned());
                            leaf_scope_label(&cv.tab_name, &parts)
                        }
                        _ => leaf_scope_label(&cv.tab_name, &leaf.child_path),
                    },
                });
                for row in rows {
                    if seen.insert((row.scope.clone(), row.name.clone(), row.keys.clone())) {
                        all.push(row);
                    }
                }
            }
            // Node `shortcuts:` and unbound adapter actions dispatch through
            // the node-action path, not the pane keymap, so the leaf maps
            // above miss them entirely. Add them across every declared level
            // so the "All tabs" / "Unbound" scopes list bindable adapter
            // actions (e.g. `toggle-tracking`) from every tab.
            for row in cv.all_node_shortcut_rows() {
                if seen.insert((row.scope.clone(), row.name.clone(), row.keys.clone())) {
                    all.push(row);
                }
            }
        }

        // Refreshing an open menu (e.g. right after adding, deleting or
        // restoring a binding) keeps the current scope and carries the live
        // fuzzy filter across the rebuild; a fresh open uses the configured
        // scope and an empty query.
        if self.shortcut_menu.is_open() {
            self.shortcut_menu.refresh(context, all);
        } else {
            self.shortcut_menu
                .open(context, all, self.config.shortcut_menu.default_scope);
        }
    }

    /// Dispatch a key while the shortcut menu is open. In execute mode +
    /// context scope, [`ShortcutMenuMessage::Execute`] replays the selected
    /// row's key through the normal top-level [`Self::handle_key`] pipeline,
    /// so the action runs exactly as if the user had pressed it.
    fn handle_shortcut_menu_key(&mut self, key: &str) {
        use crate::components::shortcut_menu::ShortcutMenuMessage;
        match self.shortcut_menu.handle_key(key) {
            ShortcutMenuMessage::Execute(k) => {
                self.handle_key(&k);
            }
            ShortcutMenuMessage::AddBinding {
                row,
                binding,
                overwrite,
            } => {
                self.add_binding_from_menu(row, binding, overwrite);
            }
            ShortcutMenuMessage::SetBindings { row, values } => {
                self.set_bindings_from_menu(row, values);
            }
            ShortcutMenuMessage::RestoreDefault { row } => {
                self.restore_binding_from_menu(row);
            }
            ShortcutMenuMessage::DeleteTagged { rows } => {
                self.delete_tagged_from_menu(rows);
            }
            ShortcutMenuMessage::RestoreTagged { rows } => {
                self.restore_tagged_from_menu(rows);
            }
            ShortcutMenuMessage::ResolveConflictApply {
                row,
                binding,
                items,
                overwrite,
            } => {
                self.resolve_conflict_apply(row, binding, items, overwrite);
            }
            ShortcutMenuMessage::ResolveRestoreBatchApply { rows, items } => {
                self.apply_restore_batch(rows, items);
            }
            ShortcutMenuMessage::BindTagged {
                rows,
                binding,
                overwrite,
            } => {
                self.bind_tagged_from_menu(rows, binding, overwrite);
            }
            ShortcutMenuMessage::ResolveBindBatchApply {
                rows,
                binding,
                items,
                overwrite,
            } => {
                self.apply_bind_batch(rows, binding, items, overwrite);
            }
            ShortcutMenuMessage::Unhandled
            | ShortcutMenuMessage::Handled
            | ShortcutMenuMessage::Closed => {}
        }
    }

    /// Deadline at which the which-key popup should reveal itself, if armed.
    /// Read by the main loop so it can `sleep_until` it.
    pub fn which_key_deadline(&self) -> Option<std::time::Instant> {
        self.which_key_deadline
    }

    /// Keep the which-key popup in sync with [`Self::pending_key`]. Called
    /// after every keypress: no pending chord closes it; a fresh prefix arms
    /// the reveal timer (once the delay elapses the main loop reveals it via
    /// [`Self::reveal_which_key`]); a chord that steps deeper while the popup
    /// is already open refilters it live.
    pub fn reconcile_which_key(&mut self) {
        let Some(prefix) = self.pending_key.clone() else {
            self.which_key.close();
            self.which_key_deadline = None;
            return;
        };
        if !self.config.which_key.enabled || !self.which_key_prefix_allowed(&prefix) {
            self.which_key.close();
            self.which_key_deadline = None;
            return;
        }
        if self.which_key.is_open() {
            // Already shown — narrow the list to the deeper prefix. An empty
            // result means the pending chord is a lone unshared prefix; keep
            // the (now empty) popup closed rather than show a blank panel.
            let rows = self.which_key_candidates(&prefix);
            if rows.is_empty() {
                self.which_key.close();
            } else {
                self.which_key.open(prefix, rows);
            }
        } else if self.which_key_deadline.is_none() {
            self.which_key_deadline = Some(
                std::time::Instant::now()
                    + std::time::Duration::from_millis(self.config.which_key.delay_ms),
            );
        }
    }

    /// The reveal timer fired: open the popup for the current pending prefix.
    /// Returns whether anything changed (drives a repaint). A vanished or
    /// candidate-less prefix is a no-op.
    pub fn reveal_which_key(&mut self) -> bool {
        self.which_key_deadline = None;
        let Some(prefix) = self.pending_key.clone() else {
            return false;
        };
        if !self.config.which_key.enabled || !self.which_key_prefix_allowed(&prefix) {
            return false;
        }
        let rows = self.which_key_candidates(&prefix);
        if rows.is_empty() {
            return false;
        }
        self.which_key.open(prefix, rows);
        true
    }

    /// Is a pending chord `prefix` eligible for the popup? True when the
    /// allowlist is empty (all prefixes), else when `prefix` starts with one
    /// of the configured prefix sequences (matched step-wise).
    fn which_key_prefix_allowed(&self, prefix: &str) -> bool {
        which_key_prefix_allowed(&self.config.which_key.prefixes, prefix)
    }

    /// Every binding active in the focused pane whose sequence strictly
    /// continues `prefix`, as `(action name, full combo)` pairs. Drawn from
    /// the same sources the chord dispatcher resolves against: the global
    /// keybindings, the focused content view's live keymap + keyless actions
    /// (which folds in the common section), the generic tab-switch keys, and
    /// user `cmdline_shortcuts`. Sorted by combo for stable order.
    fn which_key_candidates(&self, prefix: &str) -> Vec<(String, String)> {
        // (name, keys-field). The keys field is the shortcut-row surface form
        // where alternatives are joined by " / "; split back out in the filter.
        let mut sources: Vec<(String, String)> = Vec::new();
        // Global section: dispatched at App level (e.g. a rebound `gl`), so it
        // never reaches the content view's keymap. Name via the action's slug.
        for (action, binding) in &self.keybindings.global.bindings {
            sources.push((action.to_string(), binding.0.join(" / ")));
        }
        for r in self.tab_switch_rows() {
            sources.push((r.name, r.keys));
        }
        let Tab::Content(idx) = self.active_tab;
        if let Some(cv) = self.content_view(idx) {
            for r in cv.context_shortcut_rows() {
                sources.push((r.name, r.keys));
            }
        }
        for (key, cmd) in &self.config.cmdline_shortcuts {
            sources.push((cmd.clone(), key.clone()));
        }

        which_key_filter(sources, prefix)
    }

    /// Resolve the binding conflicts the user confirmed (y): drop every
    /// colliding alternative listed in `items` from its owning shortcut, then
    /// bind `binding` on `row`. Items are grouped by source so a shortcut with
    /// two colliding alternatives loses both in a single write. All writes are
    /// comment-preserving; each file is re-read fresh before its edit so
    /// same-file edits compose, and every touched file is reloaded in-process.
    fn resolve_conflict_apply(
        &mut self,
        row: crate::keymap::ShortcutRow,
        binding: String,
        items: Vec<crate::components::shortcut_menu::ConflictItem>,
        overwrite: bool,
    ) {
        let Some(source) = row.source.clone() else {
            self.notify_error("This shortcut is read-only".to_string());
            return;
        };
        // A read-only conflict can't be freed — refuse rather than leave a
        // shadowed binding behind. (The prompt already declines to confirm in
        // this case; this guards the app path too.)
        if let Some(ro) = items.iter().find(|i| !i.removable) {
            self.notify_error(format!(
                "'{binding}' conflicts with read-only '{}' — not applied",
                ro.name
            ));
            return;
        }

        // 1. Drop the colliding alternatives, grouped by conflicting source so
        //    each file is edited once even if it owns several collisions.
        let mut touched: Vec<std::path::PathBuf> = Vec::new();
        let mut by_source: Vec<(crate::keymap::KeySource, Vec<String>, Vec<String>)> = Vec::new();
        for item in &items {
            if let Some(entry) = by_source.iter_mut().find(|(s, _, _)| *s == item.source) {
                entry.2.push(item.drop.clone());
            } else {
                by_source.push((
                    item.source.clone(),
                    item.current.clone(),
                    vec![item.drop.clone()],
                ));
            }
        }
        for (other_source, current, drops) in &by_source {
            // DB-stored shortcuts (saved query / `:script` menu / Postgres
            // table script) keep their chord in the `query_shortcut` table,
            // not YAML — free them via the repository, then skip the file path.
            match self.free_db_shortcut(other_source) {
                Ok(true) => continue,
                Ok(false) => {}
                Err(e) => {
                    self.notify_error(e);
                    return;
                }
            }
            let (loc, path) = match self.resolve_binding_target(other_source) {
                Ok(v) => v,
                Err(e) => {
                    self.notify_error(e);
                    return;
                }
            };
            // Single-slot bindings (a per-node `shortcuts:` map key, a subtab
            // `key`, the query `menu_key`, a `preview.keybinding`, or a child
            // keybinding override) carry no alternatives list to trim — freeing
            // the key means deleting the whole entry line, not rewriting a
            // `key:` value.
            let is_slot = matches!(
                other_source,
                crate::keymap::KeySource::NodeShortcut { .. }
                    | crate::keymap::KeySource::YamlSubtab { .. }
                    | crate::keymap::KeySource::YamlMenuKey { .. }
                    | crate::keymap::KeySource::YamlPreviewKey { .. }
                    | crate::keymap::KeySource::YamlChildKeybinding { .. }
                    | crate::keymap::KeySource::AppActionChain { .. }
                    | crate::keymap::KeySource::PaneSearchJump { .. }
            );
            let res = if is_slot {
                self.remove_binding_file(&path, &loc)
            } else {
                let new: Vec<String> = current
                    .iter()
                    .filter(|b| !drops.contains(b))
                    .cloned()
                    .collect();
                self.edit_binding_file(&path, &loc, &new)
            };
            if let Err(e) = res {
                self.notify_error(e);
                return;
            }
            if !touched.contains(&path) {
                touched.push(path);
            }
        }

        // 2. Bind the new key on the target action. A DB-stored target writes
        //    the chord to the `query_shortcut` table (single chord, replaces);
        //    a YAML target edits its file (read fresh: it may be one just
        //    edited above).
        match self.set_db_shortcut(&source, &binding) {
            Ok(true) => {}
            Ok(false) if matches!(source, crate::keymap::KeySource::NodeShortcut { .. }) => {
                // Per-node `shortcuts:` target: add `<binding>: <action>`,
                // reading fresh so it composes with any key freed above.
                let crate::keymap::KeySource::NodeShortcut { action, .. } = &source else {
                    unreachable!()
                };
                let action = action.clone();
                let current = Self::row_bindings(&row);
                let target = if overwrite {
                    vec![binding.clone()]
                } else {
                    let mut t = current.clone();
                    if !t.contains(&binding) {
                        t.push(binding.clone());
                    }
                    t
                };
                match self.rewrite_node_shortcut(&source, &action, &current, &target) {
                    Ok(path) => {
                        if !touched.contains(&path) {
                            touched.push(path);
                        }
                    }
                    Err(e) => {
                        self.notify_error(e);
                        return;
                    }
                }
            }
            Ok(false) => {
                let (loc, path) = match self.resolve_binding_target(&source) {
                    Ok(v) => v,
                    Err(e) => {
                        self.notify_error(e);
                        return;
                    }
                };
                let values = if overwrite {
                    vec![binding.clone()]
                } else {
                    let mut v = Self::row_bindings(&row);
                    if !v.iter().any(|b| b == &binding) {
                        v.push(binding.clone());
                    }
                    v
                };
                if let Err(e) = self.edit_binding_file(&path, &loc, &values) {
                    self.notify_error(e);
                    return;
                }
                if !touched.contains(&path) {
                    touched.push(path);
                }
            }
            Err(e) => {
                self.notify_error(e);
                return;
            }
        }

        // 3. Reload every touched file, then refresh the menu.
        for p in &touched {
            let _ = self.reload_config(p);
        }
        self.notify(format!("Rebound '{binding}' to {}", row.name));
        self.open_shortcut_menu();
    }

    /// Read `path`, apply `set_binding` at `location` with `values`, and write
    /// it back. Returns a user-facing error string on any failure. Does not
    /// reload — callers batch reloads when several files change.
    fn edit_binding_file(
        &self,
        path: &std::path::Path,
        location: &crate::config::keybinding_edit::BindingLocation,
        values: &[String],
    ) -> Result<(), String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
        let edited = crate::config::keybinding_edit::set_binding(location, &text, values)
            .map_err(|e| format!("Edit failed: {e}"))?;
        self.validate_config_text(path, &edited)
            .map_err(|e| format!("Change rejected — would break {}: {e}", path.display()))?;
        std::fs::write(path, &edited).map_err(|e| format!("Cannot write {}: {e}", path.display()))
    }

    /// Read `path`, remove the located entry line entirely (comment-preserving)
    /// and write it back. Used for per-node `shortcuts:` collisions, whose map
    /// key *is* the binding — dropping it means deleting the whole line, not
    /// rewriting a `key:` value. Returns a user-facing error string on failure.
    fn remove_binding_file(
        &self,
        path: &std::path::Path,
        location: &crate::config::keybinding_edit::BindingLocation,
    ) -> Result<(), String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
        let edited = crate::config::keybinding_edit::remove_binding(location, &text)
            .map_err(|e| format!("Edit failed: {e}"))?;
        self.validate_config_text(path, &edited)
            .map_err(|e| format!("Change rejected — would break {}: {e}", path.display()))?;
        std::fs::write(path, &edited).map_err(|e| format!("Cannot write {}: {e}", path.display()))
    }

    /// Rewrite a per-node `shortcuts:` block so the keys mapping to `action`
    /// become exactly `target` (each entry is `key: action`). Keys in
    /// `current` but not `target` are removed; keys in `target` but not
    /// `current` are inserted with the action verb as their value, creating
    /// the `shortcuts:` map/block if it is absent or empty.
    ///
    /// This is the write path for adapter-declared actions surfaced by the
    /// menu without a YAML binding: their map key *is* the chord and the value
    /// is the action verb — the reverse of every `key:`-valued binding, so the
    /// generic [`Self::edit_binding_file`] cannot express it. Reads the file
    /// fresh (so it composes with prior same-file edits) and returns the
    /// touched path; the caller reloads.
    fn rewrite_node_shortcut(
        &self,
        source: &crate::keymap::KeySource,
        action: &str,
        current: &[String],
        target: &[String],
    ) -> Result<std::path::PathBuf, String> {
        use crate::config::keybinding_edit::{remove_binding, set_binding_in_optional_map};

        let (base, path) = self.resolve_binding_target(source)?;
        let mut text = std::fs::read_to_string(&path)
            .map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
        // Drop keys that should no longer map to this action.
        for k in current.iter().filter(|k| !target.contains(k)) {
            let mut loc = base.clone();
            loc.entry = k.clone();
            match remove_binding(&loc, &text) {
                Ok(t) => text = t,
                // An already-absent key (e.g. never persisted) is not an error.
                Err(e) if e.contains("not found") => {}
                Err(e) => return Err(format!("Edit failed: {e}")),
            }
        }
        // Insert the newly-wanted keys, each pointing at the action verb.
        for k in target.iter().filter(|k| !current.contains(k)) {
            let mut loc = base.clone();
            loc.entry = k.clone();
            text = set_binding_in_optional_map(&loc, &text, &[action.to_string()])
                .map_err(|e| format!("Edit failed: {e}"))?;
        }
        self.validate_config_text(&path, &text)
            .map_err(|e| format!("Change rejected — would break {}: {e}", path.display()))?;
        std::fs::write(&path, &text)
            .map_err(|e| format!("Cannot write {}: {e}", path.display()))?;
        Ok(path)
    }

    /// Reload an already-written config `path` in-process, surface `ok_msg` and
    /// refresh the open menu. The twin of [`Self::write_reload_and_refresh`]
    /// for callers that have written the file themselves (e.g.
    /// [`Self::rewrite_node_shortcut`]).
    fn reload_note_refresh(&mut self, path: &std::path::Path, ok_msg: String) {
        match self.reload_config(path) {
            Ok(_) => {
                self.notify(ok_msg);
                self.open_shortcut_menu();
            }
            Err(e) => self.notify_error(format!("Saved, but reload failed: {e}")),
        }
    }

    /// Resolve the editable [`BindingLocation`] and owning file path behind a
    /// shortcut-menu row, or an error string to surface if the row is
    /// read-only / not locatable / its file cannot be found.
    fn resolve_binding_target(
        &self,
        source: &crate::keymap::KeySource,
    ) -> Result<
        (
            crate::config::keybinding_edit::BindingLocation,
            std::path::PathBuf,
        ),
        String,
    > {
        use crate::config::keybinding_edit::{EditTarget, locate_binding};

        let location = locate_binding(source).ok_or("This shortcut is not editable here")?;
        let path = match &location.target {
            EditTarget::TuiYaml => crate::app::config_edit::config_root()
                .ok_or("Cannot locate config dir")?
                .join("tui.yaml"),
            EditTarget::ViewFile { view } => self
                .content_views_iter()
                .find_map(|cv| {
                    cv.view_defs
                        .iter()
                        .any(|vd| &vd.name == view)
                        .then(|| cv.source_path.clone())
                        .flatten()
                })
                .ok_or_else(|| format!("No config file found for view '{view}'"))?,
            EditTarget::TabFile { tab } => self
                .content_views_iter()
                .find_map(|cv| {
                    (cv.tab_name == *tab)
                        .then(|| cv.source_path.clone())
                        .flatten()
                })
                .ok_or_else(|| format!("No config file found for tab '{tab}'"))?,
        };
        Ok((location, path))
    }

    /// Write `edited` YAML to `path`, reload it in-process and refresh the
    /// open shortcut menu, surfacing `ok_msg` on success. Every failure mode
    /// notifies the user.
    fn write_reload_and_refresh(&mut self, path: &std::path::Path, edited: &str, ok_msg: String) {
        if let Err(e) = self.validate_config_text(path, edited) {
            self.notify_error(format!(
                "Change rejected — would break {}: {e}",
                path.display()
            ));
            return;
        }
        if let Err(e) = std::fs::write(path, edited) {
            self.notify_error(format!("Cannot write {}: {e}", path.display()));
            return;
        }
        match self.reload_config(path) {
            Ok(_) => {
                self.notify(ok_msg);
                // Refresh so the change shows in the menu immediately.
                self.open_shortcut_menu();
            }
            Err(e) => self.notify_error(format!("Saved, but reload failed: {e}")),
        }
    }

    /// The current bindings of a menu row (`"a / ctrl+k l"` → `["a", "ctrl+k l"]`).
    fn row_bindings(row: &crate::keymap::ShortcutRow) -> Vec<String> {
        row.keys
            .split(" / ")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// Add a newly-recorded `binding` (surface form, steps space-joined) as an
    /// alternative to the shortcut behind `row`, writing it to the owning
    /// config file comment-preservingly and reloading in-process.
    ///
    /// Conflicts (scope-overlapping claims, including built-in globals and
    /// prefix collisions) abort the write and are surfaced instead — the
    /// interactive resolve-the-conflict prompt lands in a later phase. Success
    /// and every failure mode notify the user; the menu stays open.
    fn add_binding_from_menu(
        &mut self,
        row: crate::keymap::ShortcutRow,
        binding: String,
        overwrite: bool,
    ) {
        use crate::config::keybinding_edit::set_binding;
        use crate::config::keybindings::KeyBinding;

        let Some(source) = row.source.clone() else {
            self.notify_error("This shortcut is read-only".to_string());
            return;
        };
        let Some(scope) = row.key_scope.clone() else {
            self.notify_error("This shortcut has no editable scope".to_string());
            return;
        };
        let scope = self.repair_pane_scope(&source, scope);

        // Ctrl-U (overwrite) replaces the row's bindings with exactly the new
        // one; Ctrl-N (add) appends it as an alternative. Overwrite is how a
        // terminal key (`f`) is promoted to a chord (`f f`) without leaving the
        // shadowing single key behind, so it deliberately drops the old value.
        let values = if overwrite {
            vec![binding.clone()]
        } else {
            let mut v = Self::row_bindings(&row);
            if v.iter().any(|b| b == &binding) {
                self.notify(format!("'{binding}' is already bound here"));
                return;
            }
            v.push(binding.clone());
            v
        };

        // Conflict check against every live claim across all content tabs. A
        // collision raises the resolve-the-conflict prompt instead of writing.
        let claims = self.all_live_claims();
        let proposed = KeyBinding(vec![binding.clone()]);
        let conflicts = crate::keymap::binding_conflicts(&proposed, &scope, &claims, Some(&source));
        if !conflicts.is_empty() {
            let items = self.build_conflict_items(&conflicts, &claims);
            self.shortcut_menu
                .show_conflicts(row.clone(), binding.clone(), items, overwrite);
            return;
        }

        // Per-node `shortcuts:` binding: the map key is the chord and its
        // value is the action verb, so we can't append a chord to a `key:`
        // list — we add a `<binding>: <action>` entry (keeping any existing
        // keys; several keys may map to the same action).
        if let crate::keymap::KeySource::NodeShortcut { action, .. } = &source {
            let action = action.clone();
            let current = Self::row_bindings(&row);
            // Overwrite replaces the key(s) mapping to this action with just the
            // new chord; add keeps the existing keys and appends.
            let target = if overwrite {
                vec![binding.clone()]
            } else {
                let mut t = current.clone();
                t.push(binding.clone());
                t
            };
            match self.rewrite_node_shortcut(&source, &action, &current, &target) {
                Ok(path) => {
                    self.reload_note_refresh(&path, format!("Bound '{binding}' to {}", row.name))
                }
                Err(e) => self.notify_error(e),
            }
            return;
        }

        // DB-stored shortcut: chord lives in the `query_shortcut` table (single
        // chord — Ctrl+N replaces, there is no alternatives list to append to).
        match self.set_db_shortcut(&source, &binding) {
            Ok(true) => {
                self.notify(format!("Bound '{binding}' to {}", row.name));
                self.open_shortcut_menu();
                return;
            }
            Ok(false) => {}
            Err(e) => {
                self.notify_error(e);
                return;
            }
        }

        let (location, path) = match self.resolve_binding_target(&source) {
            Ok(v) => v,
            Err(e) => {
                self.notify_error(e);
                return;
            }
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                self.notify_error(format!("Cannot read {}: {e}", path.display()));
                return;
            }
        };
        match set_binding(&location, &text, &values) {
            Ok(edited) => self.write_reload_and_refresh(
                &path,
                &edited,
                format!("Bound '{binding}' to {}", row.name),
            ),
            Err(e) => self.notify_error(format!("Edit failed: {e}")),
        }
    }

    /// Overwrite the shortcut behind `row` with exactly `values` — the delete
    /// path (Ctrl+D). An empty `values` writes the disable form `[]`, so the
    /// action's last/only binding is removed ("gone", built-ins included);
    /// otherwise the remaining alternatives are written. Deleting can never
    /// introduce a conflict, so no conflict check runs.
    fn set_bindings_from_menu(&mut self, row: crate::keymap::ShortcutRow, values: Vec<String>) {
        use crate::config::keybinding_edit::set_binding;

        let Some(source) = row.source.clone() else {
            self.notify_error("This shortcut is read-only".to_string());
            return;
        };
        let ok_msg = if values.is_empty() {
            format!("Disabled '{}'", row.name)
        } else {
            format!("Updated bindings for '{}'", row.name)
        };

        // Per-node `shortcuts:` binding: the remaining keys must map to the
        // action verb, so rewrite the block to exactly `values` (empty =>
        // every key line removed) rather than writing a bogus `key: []`.
        if let crate::keymap::KeySource::NodeShortcut { action, .. } = &source {
            let action = action.clone();
            let current = Self::row_bindings(&row);
            match self.rewrite_node_shortcut(&source, &action, &current, &values) {
                Ok(path) => self.reload_note_refresh(&path, ok_msg),
                Err(e) => self.notify_error(e),
            }
            return;
        }

        // DB-stored shortcut: a single chord in `query_shortcut`. Empty values
        // (`[]`) means disable → unset the row; otherwise replace with the one
        // remaining chord. No YAML file is touched.
        if self.resolve_db_shortcut(&source).is_some() {
            let res = match values.last() {
                Some(chord) => self.set_db_shortcut(&source, chord),
                None => self.free_db_shortcut(&source),
            };
            match res {
                Ok(_) => {
                    self.notify(ok_msg);
                    self.open_shortcut_menu();
                }
                Err(e) => self.notify_error(e),
            }
            return;
        }

        let (location, path) = match self.resolve_binding_target(&source) {
            Ok(v) => v,
            Err(e) => {
                self.notify_error(e);
                return;
            }
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                self.notify_error(format!("Cannot read {}: {e}", path.display()));
                return;
            }
        };
        match set_binding(&location, &text, &values) {
            Ok(edited) => self.write_reload_and_refresh(&path, &edited, ok_msg),
            Err(e) => self.notify_error(format!("Edit failed: {e}")),
        }
    }

    /// Restore a built-in shortcut to its compiled-in default (Ctrl+R) by
    /// removing its `tui.yaml` override entry. Only meaningful for built-ins;
    /// the menu suppresses Ctrl+R for view actions (which have no default).
    fn restore_binding_from_menu(&mut self, row: crate::keymap::ShortcutRow) {
        use crate::config::keybinding_edit::remove_binding;

        let Some(source) = row.source.clone() else {
            self.notify_error("This shortcut is read-only".to_string());
            return;
        };
        if !source.has_compiled_default() {
            self.notify_error(
                "Only built-in and tab-switch shortcuts have a default to restore".to_string(),
            );
            return;
        }
        let (location, path) = match self.resolve_binding_target(&source) {
            Ok(v) => v,
            Err(e) => {
                self.notify_error(e);
                return;
            }
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                self.notify_error(format!("Cannot read {}: {e}", path.display()));
                return;
            }
        };
        match remove_binding(&location, &text) {
            Ok(edited) => self.write_reload_and_refresh(
                &path,
                &edited,
                format!("Restored default for '{}'", row.name),
            ),
            // No override entry to drop — a tab-switch key already on its
            // autonumber digit, or a built-in already at its compiled default.
            Err(e) if e.contains("not found") => {
                self.notify(format!("'{}' is already at its default", row.name))
            }
            Err(e) => self.notify_error(format!("Edit failed: {e}")),
        }
    }

    /// Repair a context row's [`KeyScope::Pane`] whose tab name is the empty
    /// placeholder the runtime keymap files context-scope claims under (the
    /// real tab was never plumbed through `Pane::build_claims`). Recovers the
    /// tab from the content view that owns this binding's subtab view, so the
    /// conflict check overlaps correctly against tab-wide built-ins. Non-Pane
    /// scopes and already-named Panes pass through unchanged.
    fn repair_pane_scope(
        &self,
        source: &crate::keymap::KeySource,
        scope: crate::keymap::KeyScope,
    ) -> crate::keymap::KeyScope {
        use crate::keymap::{KeyScope, TabRef};
        let KeyScope::Pane(tab, profile) = scope else {
            return scope;
        };
        if !tab.0.is_empty() {
            return KeyScope::Pane(tab, profile);
        }
        let real = source
            .subtab_view()
            .and_then(|view| {
                self.content_views_iter()
                    .find(|cv| cv.view_defs.iter().any(|vd| vd.name == view))
            })
            .map(|cv| cv.tab_name.clone());
        KeyScope::Pane(real.map(TabRef::new).unwrap_or(tab), profile)
    }

    /// Every live [`KeyClaim`] across all content tabs — the full set the
    /// shortcut-menu conflict check runs against. Folds together each view's
    /// leaf keymaps, the generic tab-switch keys (kept out of leaf maps), and
    /// the per-node `shortcuts:` entries (which override built-ins in leaves,
    /// so they must be added explicitly or a real binding would be invisible).
    fn all_live_claims(&self) -> Vec<crate::keymap::KeyClaim> {
        use crate::keymap::{KeyClaim, build_leaf_maps_for};
        let kb = &self.config.keybindings;
        let mut claims: Vec<KeyClaim> = Vec::new();
        for cv in self.content_views_iter() {
            for leaf in build_leaf_maps_for(&cv.tab_name, &cv.view_defs, kb) {
                claims.extend(leaf.keymap.claims);
            }
        }
        claims.extend(self.tab_switch_claims());
        for cv in self.content_views_iter() {
            claims.extend(crate::keymap::node_shortcut_claims(
                &cv.tab_name,
                &cv.view_defs,
            ));
        }
        claims
    }

    /// Build one prompt [`ConflictItem`] per distinct `(source, colliding
    /// binding)`. A built-in claim is folded into every leaf, so the same
    /// collision arrives many times — this dedups it. `claims` is used to
    /// recover each conflicting shortcut's full current bindings (for the
    /// drop-then-rewrite) and its surface form.
    fn build_conflict_items(
        &self,
        conflicts: &[crate::keymap::BindingConflict],
        claims: &[crate::keymap::KeyClaim],
    ) -> Vec<crate::components::shortcut_menu::ConflictItem> {
        use crate::components::shortcut_menu::ConflictItem;
        let mut items: Vec<ConflictItem> = Vec::new();
        for c in conflicts {
            let current: Vec<String> = claims
                .iter()
                .find(|cl| cl.source == c.source)
                .map(|cl| cl.key.0.clone())
                .unwrap_or_default();
            let drop = current
                .iter()
                .find(|b| crate::config::keybindings::binding_steps(b) == c.existing_seq)
                .cloned()
                .unwrap_or_else(|| c.existing_seq.join(" "));
            if items
                .iter()
                .any(|it| it.source == c.source && it.drop == drop)
            {
                continue;
            }
            items.push(ConflictItem {
                source: c.source.clone(),
                current,
                drop,
                name: c.source.action_name(),
                removable: crate::config::keybinding_edit::locate_binding(&c.source).is_some()
                    || self.resolve_db_shortcut(&c.source).is_some(),
            });
        }
        items
    }

    /// The compiled-in default binding for a source that has one (the four
    /// built-in sections, plus tab-switch keys whose default is the positional
    /// autonumber digit). Returns `None` for sources with no default — the
    /// same set [`KeySource::has_compiled_default`] rejects.
    fn compiled_default_binding(
        &self,
        source: &crate::keymap::KeySource,
    ) -> Option<crate::config::keybindings::KeyBinding> {
        use crate::config::keybindings::{
            CommonAction, ContentAction, GlobalAction, KeyBinding, KeyBindingSection, WindowAction,
        };
        use crate::keymap::KeySource;
        match source {
            KeySource::Global(a) => KeyBindingSection::<GlobalAction>::default()
                .bindings
                .get(a)
                .cloned(),
            KeySource::Common(a) => KeyBindingSection::<CommonAction>::default()
                .bindings
                .get(a)
                .cloned(),
            KeySource::Content(a) => KeyBindingSection::<ContentAction>::default()
                .bindings
                .get(a)
                .cloned(),
            KeySource::Window(a) => KeyBindingSection::<WindowAction>::default()
                .bindings
                .get(a)
                .cloned(),
            KeySource::TabSwitch { tab } => {
                let t = self.tab_layout.tabs().iter().copied().find(|&t| {
                    let Tab::Content(idx) = t;
                    self.content_view(idx)
                        .map(|cv| cv.tab_name == *tab)
                        .unwrap_or(false)
                })?;
                self.tab_layout
                    .digit_for(t)
                    .map(|d| KeyBinding(vec![d.to_string()]))
            }
            _ => None,
        }
    }

    /// Clear a row's binding(s) (`[]`) without reloading — the shared write
    /// half of both single-row delete and batch delete. Returns the config
    /// file(s) touched so the caller can batch a single reload. DB-stored
    /// shortcuts are freed via the repository and touch no file.
    fn disable_binding(
        &mut self,
        row: &crate::keymap::ShortcutRow,
    ) -> Result<Vec<std::path::PathBuf>, String> {
        let source = row
            .source
            .clone()
            .ok_or_else(|| "This shortcut is read-only".to_string())?;

        // Per-node `shortcuts:` binding: remove every key line mapping to the
        // action (the map key is the chord — there's no `key: []` to write).
        if let crate::keymap::KeySource::NodeShortcut { action, .. } = &source {
            let action = action.clone();
            let current = Self::row_bindings(row);
            let path = self.rewrite_node_shortcut(&source, &action, &current, &[])?;
            return Ok(vec![path]);
        }
        // DB-stored shortcut: unset the `query_shortcut` row; no YAML file.
        if self.resolve_db_shortcut(&source).is_some() {
            self.free_db_shortcut(&source)?;
            return Ok(Vec::new());
        }
        let (location, path) = self.resolve_binding_target(&source)?;
        self.edit_binding_file(&path, &location, &[])?;
        Ok(vec![path])
    }

    /// Batch Ctrl+D: disable every tagged row. Deleting can never introduce a
    /// conflict, so each row is cleared straight away; touched files reload
    /// once at the end and the menu refreshes.
    fn delete_tagged_from_menu(&mut self, rows: Vec<crate::keymap::ShortcutRow>) {
        let mut touched: Vec<std::path::PathBuf> = Vec::new();
        let mut done = 0usize;
        let mut errors: Vec<String> = Vec::new();
        for row in &rows {
            match self.disable_binding(row) {
                Ok(paths) => {
                    done += 1;
                    for p in paths {
                        if !touched.contains(&p) {
                            touched.push(p);
                        }
                    }
                }
                Err(e) => errors.push(format!("{}: {e}", row.name)),
            }
        }
        for p in &touched {
            let _ = self.reload_config(p);
        }
        if errors.is_empty() {
            self.notify(format!("Disabled {done} shortcut(s)"));
        } else {
            self.notify_error(format!(
                "Disabled {done}, {} failed: {}",
                errors.len(),
                errors.join("; ")
            ));
        }
        self.open_shortcut_menu();
    }

    /// Aggregated conflict items for restoring every row in `rows` to its
    /// compiled default. Each restored default is checked against the claims
    /// of shortcuts *not* being restored (the restored ones are being cleared,
    /// so they can't collide). Deduplicated across rows by `(source, drop)`.
    /// Empty means the whole batch applies without a prompt.
    fn restore_batch_conflicts(
        &self,
        rows: &[crate::keymap::ShortcutRow],
    ) -> Vec<crate::components::shortcut_menu::ConflictItem> {
        use crate::keymap::{KeyClaim, KeySource};
        let claims = self.all_live_claims();
        // The sources being restored are having their current bindings dropped,
        // so they are not collision targets for one another.
        let restoring: Vec<KeySource> = rows.iter().filter_map(|r| r.source.clone()).collect();
        let others: Vec<KeyClaim> = claims
            .iter()
            .filter(|cl| !restoring.contains(&cl.source))
            .cloned()
            .collect();
        let mut items: Vec<crate::components::shortcut_menu::ConflictItem> = Vec::new();
        for row in rows {
            let Some(source) = row.source.clone() else {
                continue;
            };
            let Some(default) = self.compiled_default_binding(&source) else {
                continue;
            };
            let Some(scope) = row.key_scope.clone() else {
                continue;
            };
            let scope = self.repair_pane_scope(&source, scope);
            let conflicts =
                crate::keymap::binding_conflicts(&default, &scope, &others, Some(&source));
            for it in self.build_conflict_items(&conflicts, &claims) {
                if items
                    .iter()
                    .any(|x| x.source == it.source && x.drop == it.drop)
                {
                    continue;
                }
                items.push(it);
            }
        }
        items
    }

    /// Batch Ctrl+E: restore every tagged row to its compiled default. Rows
    /// with no default are skipped. A clean batch applies immediately;
    /// otherwise an aggregated y/n conflict prompt is raised (resolved by
    /// [`Self::apply_restore_batch`]).
    fn restore_tagged_from_menu(&mut self, rows: Vec<crate::keymap::ShortcutRow>) {
        let restorable: Vec<crate::keymap::ShortcutRow> = rows
            .into_iter()
            .filter(|r| {
                r.source
                    .as_ref()
                    .map(|s| s.has_compiled_default())
                    .unwrap_or(false)
            })
            .collect();
        if restorable.is_empty() {
            self.notify("None of the tagged shortcuts have a default to restore".to_string());
            return;
        }
        let items = self.restore_batch_conflicts(&restorable);
        if items.is_empty() {
            self.apply_restore_batch(restorable, Vec::new());
        } else {
            self.shortcut_menu.show_restore_conflicts(restorable, items);
        }
    }

    /// Apply a (possibly conflict-resolved) batch restore: drop every colliding
    /// binding in `items` (grouped by source, comment-preservingly), then
    /// restore each row in `rows` to its default by removing its override
    /// entry. Refuses if any collision is with a read-only binding. Every
    /// touched file reloads once at the end and the menu refreshes.
    fn apply_restore_batch(
        &mut self,
        rows: Vec<crate::keymap::ShortcutRow>,
        items: Vec<crate::components::shortcut_menu::ConflictItem>,
    ) {
        use crate::keymap::KeySource;
        if let Some(ro) = items.iter().find(|i| !i.removable) {
            self.notify_error(format!(
                "Restore collides with read-only '{}' — not applied",
                ro.name
            ));
            return;
        }
        let mut touched: Vec<std::path::PathBuf> = Vec::new();

        // 1. Drop colliding bindings, grouped by source so each file is edited
        //    once even if it owns several collisions.
        let mut by_source: Vec<(KeySource, Vec<String>, Vec<String>)> = Vec::new();
        for item in &items {
            if let Some(entry) = by_source.iter_mut().find(|(s, _, _)| *s == item.source) {
                entry.2.push(item.drop.clone());
            } else {
                by_source.push((
                    item.source.clone(),
                    item.current.clone(),
                    vec![item.drop.clone()],
                ));
            }
        }
        for (other_source, current, drops) in &by_source {
            match self.free_db_shortcut(other_source) {
                Ok(true) => continue,
                Ok(false) => {}
                Err(e) => {
                    self.notify_error(e);
                    return;
                }
            }
            let (loc, path) = match self.resolve_binding_target(other_source) {
                Ok(v) => v,
                Err(e) => {
                    self.notify_error(e);
                    return;
                }
            };
            let is_slot = matches!(
                other_source,
                KeySource::NodeShortcut { .. }
                    | KeySource::YamlSubtab { .. }
                    | KeySource::YamlMenuKey { .. }
                    | KeySource::YamlPreviewKey { .. }
                    | KeySource::YamlChildKeybinding { .. }
                    | KeySource::AppActionChain { .. }
                    | KeySource::PaneSearchJump { .. }
            );
            let res = if is_slot {
                self.remove_binding_file(&path, &loc)
            } else {
                let new: Vec<String> = current
                    .iter()
                    .filter(|b| !drops.contains(b))
                    .cloned()
                    .collect();
                self.edit_binding_file(&path, &loc, &new)
            };
            if let Err(e) = res {
                self.notify_error(e);
                return;
            }
            if !touched.contains(&path) {
                touched.push(path);
            }
        }

        // 2. Restore each row: drop its override entry (read fresh so it
        //    composes with any drop above in the same file).
        let mut done = 0usize;
        for row in &rows {
            let Some(source) = row.source.clone() else {
                continue;
            };
            let (location, path) = match self.resolve_binding_target(&source) {
                Ok(v) => v,
                Err(e) => {
                    self.notify_error(e);
                    return;
                }
            };
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    self.notify_error(format!("Cannot read {}: {e}", path.display()));
                    return;
                }
            };
            match crate::config::keybinding_edit::remove_binding(&location, &text) {
                Ok(edited) => {
                    if let Err(e) = self.validate_config_text(&path, &edited) {
                        self.notify_error(format!(
                            "Change rejected — would break {}: {e}",
                            path.display()
                        ));
                        return;
                    }
                    if let Err(e) = std::fs::write(&path, &edited) {
                        self.notify_error(format!("Cannot write {}: {e}", path.display()));
                        return;
                    }
                    if !touched.contains(&path) {
                        touched.push(path);
                    }
                    done += 1;
                }
                // Already at its default — nothing to drop.
                Err(e) if e.contains("not found") => done += 1,
                Err(e) => {
                    self.notify_error(format!("Edit failed: {e}"));
                    return;
                }
            }
        }

        for p in &touched {
            let _ = self.reload_config(p);
        }
        self.notify(format!("Restored {done} default(s)"));
        self.open_shortcut_menu();
    }

    /// Write `binding` onto a single row without a conflict check or reload —
    /// the shared write half of both the batch bind and its conflict-resolved
    /// apply. `overwrite` replaces the row's bindings with exactly `binding`;
    /// otherwise it is appended as an alternative (a no-op if already present).
    /// Returns the file(s) touched so the caller can batch one reload; DB
    /// shortcuts apply through the repository and touch no file.
    fn write_binding_for_row(
        &mut self,
        row: &crate::keymap::ShortcutRow,
        binding: &str,
        overwrite: bool,
    ) -> Result<Vec<std::path::PathBuf>, String> {
        use crate::keymap::KeySource;
        let source = row
            .source
            .clone()
            .ok_or_else(|| "This shortcut is read-only".to_string())?;

        // Per-node `shortcuts:` binding: the map key is the chord, its value the
        // action verb, so we rewrite the block rather than a `key:` list.
        if let KeySource::NodeShortcut { action, .. } = &source {
            let action = action.clone();
            let current = Self::row_bindings(row);
            let target = if overwrite {
                vec![binding.to_string()]
            } else {
                let mut t = current.clone();
                if !t.iter().any(|b| b == binding) {
                    t.push(binding.to_string());
                }
                t
            };
            let path = self.rewrite_node_shortcut(&source, &action, &current, &target)?;
            return Ok(vec![path]);
        }
        // DB-stored shortcut: a single chord in `query_shortcut` (no list to
        // append to — the chord is replaced regardless of `overwrite`).
        if self.resolve_db_shortcut(&source).is_some() {
            self.set_db_shortcut(&source, binding)?;
            return Ok(Vec::new());
        }
        let values = if overwrite {
            vec![binding.to_string()]
        } else {
            let mut v = Self::row_bindings(row);
            if !v.iter().any(|b| b == binding) {
                v.push(binding.to_string());
            }
            v
        };
        let (location, path) = self.resolve_binding_target(&source)?;
        self.edit_binding_file(&path, &location, &values)?;
        Ok(vec![path])
    }

    /// Aggregated conflicts for binding `binding` on every row in `rows`.
    /// `Err` means the batch is impossible up front: two tagged rows share a
    /// scope, so the same key would shadow between them (unresolvable). `Ok`
    /// carries the collisions with shortcuts *not* in the batch, deduplicated
    /// by `(source, drop)`; empty means the batch applies without a prompt.
    fn bind_batch_conflicts(
        &self,
        rows: &[crate::keymap::ShortcutRow],
        binding: &str,
    ) -> Result<Vec<crate::components::shortcut_menu::ConflictItem>, String> {
        use crate::config::keybindings::KeyBinding;
        use crate::keymap::{KeyClaim, KeySource};
        let claims = self.all_live_claims();
        // The rows being (re)bound are not collision targets for one another
        // via their *old* bindings; their intra-batch overlap is handled below.
        let rebinding: Vec<KeySource> = rows.iter().filter_map(|r| r.source.clone()).collect();
        let others: Vec<KeyClaim> = claims
            .iter()
            .filter(|cl| !rebinding.contains(&cl.source))
            .cloned()
            .collect();
        let proposed = KeyBinding(vec![binding.to_string()]);

        // Intra-batch guard: every tagged row gets the *same* key, so any two
        // whose scopes overlap would collide with each other — which dropping a
        // third binding can't fix. Refuse with a clear message. (The intended
        // use is one action across different tabs, whose scopes never overlap.)
        let describe = |row: &crate::keymap::ShortcutRow| -> String {
            let path = if row.scope.is_empty() {
                row.name.clone()
            } else {
                format!("{} › {}", row.scope, row.name)
            };
            if row.keys.is_empty() {
                format!("{path} (unbound)")
            } else {
                format!("{path} [{}]", row.keys)
            }
        };
        let mut scoped: Vec<(String, crate::keymap::KeyScope, KeySource)> = Vec::new();
        for row in rows {
            let (Some(source), Some(scope)) = (row.source.clone(), row.key_scope.clone()) else {
                continue;
            };
            let scope = self.repair_pane_scope(&source, scope);
            // Two rows only truly collide when their scopes overlap *and* they
            // don't each belong to a different subtab of the same tab. The
            // `Pane` scope only tracks the tab, so sibling subtabs (e.g. Jira's
            // `tickets` vs `bookmarks` view) flatten to the same scope yet are
            // never active simultaneously — the same guard `binding_conflicts`
            // applies at dispatch time.
            let clash = scoped.iter().find(|(_, s, src)| {
                if !s.overlaps_with(&scope) {
                    return false;
                }
                match (src.subtab_view(), source.subtab_view()) {
                    (Some(a), Some(b)) if a != b => false,
                    _ => true,
                }
            });
            if let Some((other_desc, _, _)) = clash {
                return Err(format!(
                    "These two tagged shortcuts share a scope, so they can't both use '{binding}':\n  • {}\n  • {}",
                    other_desc,
                    describe(row)
                ));
            }
            scoped.push((describe(row), scope, source));
        }

        let mut items: Vec<crate::components::shortcut_menu::ConflictItem> = Vec::new();
        for row in rows {
            let (Some(source), Some(scope)) = (row.source.clone(), row.key_scope.clone()) else {
                continue;
            };
            let scope = self.repair_pane_scope(&source, scope);
            let conflicts =
                crate::keymap::binding_conflicts(&proposed, &scope, &others, Some(&source));
            for it in self.build_conflict_items(&conflicts, &claims) {
                if items
                    .iter()
                    .any(|x| x.source == it.source && x.drop == it.drop)
                {
                    continue;
                }
                items.push(it);
            }
        }
        Ok(items)
    }

    /// Batch Ctrl+N/Ctrl+U: bind the recorded `binding` on every tagged row.
    /// Refuses if two tagged rows share a scope; a clean batch applies
    /// immediately, otherwise an aggregated y/n conflict prompt is raised
    /// (resolved by [`Self::apply_bind_batch`]).
    fn bind_tagged_from_menu(
        &mut self,
        rows: Vec<crate::keymap::ShortcutRow>,
        binding: String,
        overwrite: bool,
    ) {
        let bindable: Vec<crate::keymap::ShortcutRow> =
            rows.into_iter().filter(|r| r.source.is_some()).collect();
        if bindable.is_empty() {
            self.notify("None of the tagged shortcuts are editable".to_string());
            return;
        }
        match self.bind_batch_conflicts(&bindable, &binding) {
            Err(e) => self.notify_error(e),
            Ok(items) if items.is_empty() => {
                self.apply_bind_batch(bindable, binding, Vec::new(), overwrite)
            }
            Ok(items) => self
                .shortcut_menu
                .show_bind_conflicts(bindable, binding, overwrite, items),
        }
    }

    /// Apply a (possibly conflict-resolved) batch bind: drop every colliding
    /// binding in `items` (grouped by source, comment-preservingly), then bind
    /// `binding` on every row in `rows`. Refuses if any collision is read-only.
    /// Every touched file reloads once at the end and the menu refreshes.
    fn apply_bind_batch(
        &mut self,
        rows: Vec<crate::keymap::ShortcutRow>,
        binding: String,
        items: Vec<crate::components::shortcut_menu::ConflictItem>,
        overwrite: bool,
    ) {
        use crate::keymap::KeySource;
        if let Some(ro) = items.iter().find(|i| !i.removable) {
            self.notify_error(format!(
                "Bind collides with read-only '{}' — not applied",
                ro.name
            ));
            return;
        }
        let mut touched: Vec<std::path::PathBuf> = Vec::new();

        // 1. Drop colliding bindings, grouped by source (each file edited once).
        let mut by_source: Vec<(KeySource, Vec<String>, Vec<String>)> = Vec::new();
        for item in &items {
            if let Some(entry) = by_source.iter_mut().find(|(s, _, _)| *s == item.source) {
                entry.2.push(item.drop.clone());
            } else {
                by_source.push((
                    item.source.clone(),
                    item.current.clone(),
                    vec![item.drop.clone()],
                ));
            }
        }
        for (other_source, current, drops) in &by_source {
            match self.free_db_shortcut(other_source) {
                Ok(true) => continue,
                Ok(false) => {}
                Err(e) => {
                    self.notify_error(e);
                    return;
                }
            }
            let (loc, path) = match self.resolve_binding_target(other_source) {
                Ok(v) => v,
                Err(e) => {
                    self.notify_error(e);
                    return;
                }
            };
            let is_slot = matches!(
                other_source,
                KeySource::NodeShortcut { .. }
                    | KeySource::YamlSubtab { .. }
                    | KeySource::YamlMenuKey { .. }
                    | KeySource::YamlPreviewKey { .. }
                    | KeySource::YamlChildKeybinding { .. }
                    | KeySource::AppActionChain { .. }
                    | KeySource::PaneSearchJump { .. }
            );
            let res = if is_slot {
                self.remove_binding_file(&path, &loc)
            } else {
                let new: Vec<String> = current
                    .iter()
                    .filter(|b| !drops.contains(b))
                    .cloned()
                    .collect();
                self.edit_binding_file(&path, &loc, &new)
            };
            if let Err(e) = res {
                self.notify_error(e);
                return;
            }
            if !touched.contains(&path) {
                touched.push(path);
            }
        }

        // 2. Bind each row (read fresh so it composes with any drop above).
        let mut done = 0usize;
        let mut errors: Vec<String> = Vec::new();
        for row in &rows {
            match self.write_binding_for_row(row, &binding, overwrite) {
                Ok(paths) => {
                    done += 1;
                    for p in paths {
                        if !touched.contains(&p) {
                            touched.push(p);
                        }
                    }
                }
                Err(e) => errors.push(format!("{}: {e}", row.name)),
            }
        }

        for p in &touched {
            let _ = self.reload_config(p);
        }
        if errors.is_empty() {
            self.notify(format!("Bound '{binding}' on {done} shortcut(s)"));
        } else {
            self.notify_error(format!(
                "Bound {done}, {} failed: {}",
                errors.len(),
                errors.join("; ")
            ));
        }
        self.open_shortcut_menu();
    }

    /// True iff some `cmdline_shortcuts:` key is a strict step-prefix of the
    /// accumulated `key` sequence — i.e. `key` should be stashed and waited
    /// on (e.g. `m` for `mc`/`mp`). Compared per-step so the space-form and
    /// legacy concatenation both resolve.
    fn cmdline_shortcut_chord_prefix(&self, key: &str) -> bool {
        let pressed = binding_steps(key);
        self.config.cmdline_shortcuts.keys().any(|k| {
            let ks = binding_steps(k);
            ks.len() > pressed.len() && ks[..pressed.len()] == pressed[..]
        })
    }

    /// The command bound to a fully-pressed cmdline-shortcut sequence, if
    /// any. Matched per-step (via [`binding_steps`]) so the accumulated
    /// space-joined chord matches a config key written either way.
    fn cmdline_shortcut_for_chord(&self, chord: &str) -> Option<String> {
        let pressed = binding_steps(chord);
        self.config
            .cmdline_shortcuts
            .iter()
            .find(|(k, _)| binding_steps(k) == pressed)
            .map(|(_, v)| v.clone())
    }

    /// Clear notification bar, sticky notification, and the most recent
    /// query-error banner. Shared by `GlobalAction::DismissNotifications`
    /// and the `:dismiss-notifications` cmdline command.
    fn dismiss_notifications(&mut self) {
        self.notification_bar.clear();
        self.alert_bar.clear();
        self.event_notices.clear();
        self.notification = None;
        self.set_query_error(None);
    }

    /// `:jump <Tab>` — programmatic tab switch.
    ///
    /// The tab name is matched case-insensitively against each content
    /// tab's `tab_name`. Used by scripts (via the script-output relay) and
    /// by users from the `:` cmdline. Unknown tab => modal error, no
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
                self.modal_message = Some(format!(":jump — unknown tab '{head}'"));
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
    /// `:tree-find "Tasks" id:<uuid>`. The query is adapter-defined;
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
                 :tree-find \"Tasks\" id:<uuid>"
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

        Self::treefind_trace(
            "command",
            format!("tab={tab_name:?} view={view_name:?} query={query:?}"),
        );
        if let Err(e) = self.tree_find_in_tab(tab_idx, view_name, query) {
            Self::treefind_trace("command", format!("tree_find_in_tab error: {e}"));
            self.modal_message = Some(format!(":tree-find — {e}"));
        }
    }

    /// Queue a tree-find `query` on content tab `tab_idx`, optionally
    /// switching to `view_name` first, then force a fresh reload. The
    /// queued query fires when the load lands (see the
    /// `LoadMsg::ContentItems` handler), so the search runs against an
    /// up-to-date snapshot — parity with the legacy `:reload-tasks` that
    /// preceded `:focus-task`.
    ///
    /// Shared by `:tree-find` and the `tasks/<uuid>` / `tracking/<uuid>`
    /// link reroute (via the adapter `id:<uuid>` exact-id escape, which is
    /// robust against the lazily-ingested tree). Returns `Err(message)`
    /// (the caller prefixes its own context) when the tab is in an error
    /// state, the named view is unknown, or the active view is not a tree.
    pub(crate) fn tree_find_in_tab(
        &mut self,
        tab_idx: usize,
        view_name: Option<String>,
        query: String,
    ) -> Result<(), String> {
        self.set_active_tab(Tab::Content(tab_idx));

        let pane_id = {
            let cv = match &mut self.content_views[tab_idx] {
                ContentSlot::Working(cv) => cv,
                ContentSlot::Broken { name, errors, .. } => {
                    return Err(format!(
                        "tab '{name}' is in an error state: {}",
                        errors.first().cloned().unwrap_or_default()
                    ));
                }
            };
            if let Some(v) = view_name {
                if let Err(available) = cv.switch_to_view_by_name(&v) {
                    return Err(format!(
                        "unknown view '{v}' (available: {})",
                        available.join(", ")
                    ));
                }
            }
            if !cv.active_view_is_tree() {
                return Err(
                    "the active view isn't a tree (use :focus-node for flat views)".to_string(),
                );
            }
            let pane_id = cv.active_pane_id();
            cv.active_pane_mut().queue_pending_tree_find(query);
            pane_id
        };

        self.spawn_content_load(tab_idx, pane_id);
        Ok(())
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
        let (vars_prefilled, target, name_str) = match parse_query_apply_args(raw_args) {
            Ok(parsed) => parsed,
            Err(msg) => {
                self.modal_message = Some(format!(":query apply — {msg}"));
                return;
            }
        };
        if name_str.is_empty() {
            self.modal_message =
                Some(":query apply expects [--var k=v]* [-t <Tab>[:<view>]] <name>".to_string());
            return;
        }

        // ── 2. Resolve target tab + view ─────────────────────────────────
        if let Some((tab_name, view_name)) = target {
            let tab_idx = self
                .content_views
                .iter()
                .position(|slot| slot.tab_name().eq_ignore_ascii_case(&tab_name));
            let Some(tab_idx) = tab_idx else {
                self.modal_message =
                    Some(format!(":query apply — '{tab_name}' is not a content tab"));
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
        let Tab::Content(tab_idx) = self.active_tab;

        // ── 3. Pull fresh DB saved queries before lookup ─────────────────
        self.reload_content_saved_queries(tab_idx);

        // ── 4. Look up saved query + pane, hand off to dispatcher ───────
        let (raw_query, saved_name, kind, pane_id) = {
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
            (sq.query, sq.name, sq.kind, cv.active_pane_id())
        };

        let target = crate::components::query_var_popup::QueryVarPopupTarget {
            tab_idx,
            pane_id,
            raw_query,
            saved_name: Some(saved_name),
            kind,
        };
        // CLI path: only popup when required vars are missing. Scripts
        // that pre-fill all `--var` flags get a popup-free apply.
        self.start_query_apply(target, vars_prefilled, false);
    }

    /// `:query edit <name>` — open the body file for `<name>` in the store
    /// that holds it in the external editor. Operates on the currently
    /// active content tab. Modal-errors when the active tab isn't a content
    /// tab, the adapter doesn't expose a filesystem-backed store, or the
    /// query doesn't exist.
    fn query_edit_command(&mut self, name: &str) {
        let view_index = match self.current_content_view_index_or_modal("edit") {
            Some(idx) => idx,
            None => return,
        };
        let kind = self.content_query_kind(view_index, name);
        let Some((path, suffix)) = self.query_body_path_or_modal("edit", view_index, name, kind)
        else {
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
            suffix,
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
    ///
    /// Always an adapter-native body: an extended document is created from
    /// the query menu (`++Name`), which can hand the editor a working
    /// template. The name still has to be free in *both* stores, since they
    /// share one namespace.
    fn query_new_command(&mut self, name: &str) {
        let view_index = match self.current_content_view_index_or_modal("new") {
            Some(idx) => idx,
            None => return,
        };
        match self.existing_query_kind(view_index, name) {
            Ok(Some(kind)) => {
                self.modal_message = Some(format!(
                    ":query new — '{name}' already exists ({kind}, use :query edit to modify)"
                ));
                return;
            }
            Ok(None) => {}
            Err(e) => {
                self.modal_message = Some(format!(":query new — could not check names: {e}"));
                return;
            }
        }
        let Some((path, suffix)) =
            self.query_body_path_or_modal("new", view_index, name, QueryKind::Saved)
        else {
            return;
        };
        let session = crate::edit_session::SavedQueryEditSession::new(
            path,
            view_index,
            name.to_string(),
            suffix,
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
        let scope = match self
            .content_view(view_index)
            .map(|cv| cv.query_scope.clone())
        {
            Some(s) => s,
            None => {
                self.modal_message = Some(":query delete — active tab has no scope".to_string());
                return;
            }
        };
        let kind = self.content_query_kind(view_index, name);
        self.delete_content_query(view_index, &scope, name, kind);
        self.reload_content_saved_queries(view_index);
        self.notify(format!("Deleted saved query '{name}'"));
    }

    /// Return the active content tab's slot index, or set
    /// `modal_message` and return `None` when the active tab isn't a
    /// content tab / is in an error state.
    fn current_content_view_index_or_modal(&mut self, sub: &str) -> Option<usize> {
        let Tab::Content(tab_idx) = self.active_tab;
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

    /// Look up the on-disk path and editor suffix for query `<name>` of
    /// `kind` in the active content view's adapter store. Returns `None`
    /// (and sets `modal_message`) when the adapter exposes no such store, or
    /// its store returns `None` from `path()` (opaque storage).
    fn query_body_path_or_modal(
        &mut self,
        sub: &str,
        view_index: usize,
        name: &str,
        kind: QueryKind,
    ) -> Option<(std::path::PathBuf, String)> {
        let cv = self.content_view(view_index)?;
        let Some(adapter) = cv.adapter.as_ref() else {
            self.modal_message = Some(format!(":query {sub} — this tab has no adapter"));
            return None;
        };
        let found = match kind {
            QueryKind::Saved => adapter
                .saved_query_store()
                .map(|s| (s.path(name), adapter.query_body_suffix().to_string())),
            QueryKind::Extended => adapter.extended_query_store().map(|s| {
                (
                    s.path(name),
                    not_yet_done_content::EXTENDED_QUERY_SUFFIX.to_string(),
                )
            }),
        };
        let Some((path, suffix)) = found else {
            self.modal_message = Some(format!(
                ":query {sub} — adapter '{}' has no {kind}-query store",
                adapter.adapter_type()
            ));
            return None;
        };
        let Some(path) = path else {
            self.modal_message = Some(format!(
                ":query {sub} — adapter '{}' stores queries opaquely (no file path)",
                adapter.adapter_type()
            ));
            return None;
        };
        Some((path, suffix))
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
        // An extended document declares its variables across several branch
        // queries, each in the adapter's own language — only the executor can
        // collect them, and a document that doesn't even parse must say so
        // here rather than fail silently on the load that follows.
        let vars = match target.kind {
            QueryKind::Saved => adapter.query_variables(&target.raw_query),
            QueryKind::Extended => {
                match not_yet_done_extended_query::document_variables(
                    &target.raw_query,
                    adapter.as_ref(),
                ) {
                    Ok((vars, warnings)) => {
                        for w in warnings {
                            self.notify(w.to_string());
                        }
                        vars
                    }
                    Err(e) => {
                        self.modal_message = Some(format!(":query apply — {e}"));
                        return;
                    }
                }
            }
        };
        let any_required_missing = vars
            .iter()
            .any(|v| v.default.is_none() && !prefilled.contains_key(&v.name));
        let needs_popup = !vars.is_empty() && (force_popup || any_required_missing);
        if !needs_popup {
            self.apply_query_with_vars(target, prefilled);
            return;
        }
        let title = match &target.saved_name {
            Some(n) => format!("Query: {n}"),
            None => "Query variables".to_string(),
        };
        self.query_var_popup = Some(crate::components::query_var_popup::QueryVarPopup::new(
            Arc::clone(&self.shared_theme),
            title,
            target,
            vars,
            prefilled,
        ));
    }

    /// Apply a saved query with the given variable bindings: stamp the
    /// pane's query+vars, then hand the load off to the ordinary async
    /// path (`spawn_content_load`).
    ///
    /// This MUST NOT block the main task. Applying a saved query can
    /// trigger an interactive login (e.g. the calendar's headless-browser
    /// 2FA) that takes seconds to minutes; the previous implementation
    /// `block_on`-ned the whole fetch on the main task and froze all input
    /// and rendering until it finished (or the browser was closed).
    /// `spawn_content_load` runs the fetch off-task and delivers the
    /// result via `LoadMsg::ContentItems` — which also drives the
    /// `expand_depth` auto-expand cascade — so the UI stays live and a
    /// later query change can supersede this one.
    pub fn apply_query_with_vars(
        &mut self,
        target: crate::components::query_var_popup::QueryVarPopupTarget,
        vars: std::collections::HashMap<String, String>,
    ) {
        let tab_idx = target.tab_idx;
        let pane_id = target.pane_id;

        {
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
            if cv.adapter.is_none() {
                self.modal_message = Some(":query apply — this tab has no adapter".to_string());
                return;
            }
            // Stamp the query+vars onto the target pane; `root_load_request`
            // (read by `spawn_content_load`) returns them via `active_query`
            // / `active_query_vars`.
            cv.set_query_for_pane_with_vars(
                pane_id,
                target.raw_query.clone(),
                target.saved_name.clone(),
                vars,
                target.kind,
            );
        }

        self.spawn_content_load(tab_idx, pane_id);
    }
}

/// Which-key allowlist test: is a pending chord `prefix` eligible for the
/// popup given the configured `prefixes`? Empty list → every prefix passes.
/// Otherwise the pending chord must *start with* one of the configured
/// prefix sequences (compared step-wise, so `g l` matches the entry `g`).
fn which_key_prefix_allowed(prefixes: &[String], prefix: &str) -> bool {
    if prefixes.is_empty() {
        return true;
    }
    let steps = binding_steps(prefix);
    prefixes.iter().any(|p| {
        let ps = binding_steps(p);
        !ps.is_empty() && steps.len() >= ps.len() && steps[..ps.len()] == ps[..]
    })
}

/// Filter `(name, keys-field)` candidate rows down to the bindings that
/// strictly continue `prefix`. `keys-field` is the shortcut-row surface
/// form (alternatives joined by `" / "`); each alternative is checked
/// independently and the *matching* combos are returned as `(name, combo)`,
/// de-duplicated and sorted by combo for stable display.
fn which_key_filter(sources: Vec<(String, String)>, prefix: &str) -> Vec<(String, String)> {
    let pre = binding_steps(prefix);
    if pre.is_empty() {
        return Vec::new();
    }
    let mut rows: Vec<(String, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (name, keys) in sources {
        for alt in keys.split(" / ") {
            let alt = alt.trim();
            if alt.is_empty() {
                continue;
            }
            let steps = binding_steps(alt);
            if steps.len() > pre.len()
                && steps[..pre.len()] == pre[..]
                && seen.insert((name.clone(), alt.to_string()))
            {
                rows.push((name.clone(), alt.to_string()));
            }
        }
    }
    rows.sort_by(|a, b| a.1.cmp(&b.1));
    rows
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
    let mut vars: std::collections::HashMap<String, String> = std::collections::HashMap::new();
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
            // Quote-aware, like `:tree-find` — a tab whose display name has
            // spaces is addressed as `-t "My Tab"`, and the quotes must be
            // stripped so the name matches `tab_name()` (which has none).
            let (tgt, tail) = split_leading_token(after);
            let tgt = tgt.trim();
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
/// token may itself contain spaces — e.g. `"Analytics DB" id:42` →
/// (`Analytics DB`, `id:42`). Unquoted tokens split on the first
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

/// May the view at `view_index` put a credential form in front of the user?
///
/// The active tab always may — the user is looking at it. Beyond that, a tab
/// that loads eagerly (`adapter.manual_connect: false`) may too: it starts
/// connecting the moment the app comes up, without the user ever visiting it,
/// so deferring its login to the first tab switch would park it on an answer
/// nobody can see. A `manual_connect` tab has no such problem — its load only
/// ever starts from a `reload` the user pressed on that very tab.
///
/// `popup_owner` is the view a form is already open for, if any. It is never
/// taken away: that login waits for its answer, and dropping its form is the
/// one thing that would leave it hanging forever. The loser keeps its
/// `NeedsCreds` on the view and is replayed when its tab is opened.
fn credential_form_allowed(
    active_view: usize,
    view_index: usize,
    manual_connect: bool,
    popup_owner: Option<usize>,
) -> bool {
    if popup_owner.is_some_and(|owner| owner != view_index) {
        return false;
    }
    active_view == view_index || !manual_connect
}

/// Title for the credential form. A credential script names what it is asking
/// for ("Unlock the password store") but not for which tab — and the form can
/// belong to a tab the user is not looking at — so the tab name leads.
fn credential_form_title(header: Option<&str>, tab_name: Option<&str>) -> String {
    match (header, tab_name) {
        (Some(h), Some(tab)) => format!("{tab}: {h}"),
        (Some(h), None) => h.to_string(),
        (None, Some(tab)) => format!("Login: {tab}"),
        (None, None) => "Login".into(),
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

/// Render notification-log records as one `[timestamp] message` block, oldest
/// first regardless of which bar they came from. Entries flagged `true` came
/// from the loud top alert bar and carry a `!` marker so they stay
/// distinguishable. Continuation lines of a multi-line message are indented so
/// entry boundaries stay readable. Empty input yields an empty string.
fn format_notification_log<'a>(
    records: impl Iterator<
        Item = (
            &'a crate::components::notification_bar::NotificationRecord,
            bool,
        ),
    >,
) -> String {
    let mut entries: Vec<_> = records.collect();
    entries.sort_by_key(|(r, _)| r.at);

    let mut out = String::new();
    for (record, alert) in entries {
        let stamp = record.at.format("%Y-%m-%d %H:%M:%S");
        let marker = if alert { "! " } else { "" };
        if record.message.is_empty() {
            out.push_str(&format!("[{stamp}] {marker}\n"));
            continue;
        }
        for (i, line) in record.message.lines().enumerate() {
            if i == 0 {
                out.push_str(&format!("[{stamp}] {marker}{line}\n"));
            } else {
                out.push_str(&format!("    {line}\n"));
            }
        }
    }
    out
}

/// The right-aligned hint both notification bars render, built from the live
/// bindings of the two actions that act on them: dismiss, and open the log in
/// the editor. Rebinding either key therefore updates the hint, and an unbound
/// action simply drops out of it.
fn notification_bar_hint(
    gkb: &crate::config::keybindings::KeyBindingSection<GlobalAction>,
) -> String {
    [
        (GlobalAction::DismissNotifications, "dismiss"),
        (GlobalAction::ShowNotifications, "open"),
    ]
    .iter()
    .filter_map(|(action, label)| {
        // `display_label` already brackets the key(s), e.g. `[Z]`.
        let binding = gkb.get(action)?;
        Some(format!("{} {label}", binding.display_label()))
    })
    .collect::<Vec<_>>()
    .join("  ")
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
            Tab::Content(_) => crate::edit_session::SessionScope::Content,
        };
        let session = crate::edit_session::ErrorViewSession::new(text, scope);
        self.open_session(Box::new(session))
    }

    /// Open the notification log in `$EDITOR` (read-only) — both bars' messages
    /// merged chronologically, so nothing the short bottom bar pushed out (or a
    /// `Z` dismissed) is lost. Falls back to a notification when nothing has
    /// been shown yet.
    fn open_notifications_editor(&mut self) -> EditorRequest {
        let text = self.notification_log_text();
        if text.is_empty() {
            self.notify("No notifications yet".to_string());
            return EditorRequest::None;
        }
        let scope = match self.active_tab {
            Tab::Content(_) => crate::edit_session::SessionScope::Content,
        };
        let session = crate::edit_session::NotificationViewSession::new(text, scope);
        self.open_session(Box::new(session))
    }

    /// Both bars' notification logs merged into one text block — see
    /// [`format_notification_log`].
    fn notification_log_text(&self) -> String {
        format_notification_log(
            self.notification_bar
                .history()
                .iter()
                .map(|r| (r, false))
                .chain(self.alert_bar.history().iter().map(|r| (r, true))),
        )
    }

    fn set_active_tab(&mut self, tab: Tab) {
        // Sort-hint mode is bound to the previously active view; cancel
        // on tab switch so we don't strand the user in a tab-mismatched
        // popup.
        if self.sort_hint_phase.is_active() {
            self.cancel_sort_hint_mode();
        }
        // A pending cut (the generic mark/paste-move clipboard) is local to
        // the tab it was made on — you can't paste a marked node into a
        // different adapter's tree. Leaving that tab aborts the cut.
        if self.active_tab != tab && self.content_marked_node.take().is_some() {
            self.notify("Cut cancelled".to_string());
        }
        self.active_tab = tab;
        {
            let Tab::Content(idx) = tab;
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
                // Live-tick coalescing: if this tab accrued background ticks
                // while hidden, run exactly one refresh now against the current
                // state so its live cells (e.g. a grouped tree's ticking
                // durations) are up to date the instant it becomes visible.
                if self.pending_live_refresh.remove(&idx) {
                    self.spawn_live_refresh(idx);
                }
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
    /// visible tabs in order, each as `icon key name`, keyed by the tab's
    /// autonumber digit (`1`..`9`, then `0`).
    ///
    /// A view whose adapter reports unread rows ([`ContentView::has_unread`])
    /// additionally gets its unread marker in front of the icon and carries
    /// its emphasis patch along, so a background chat tab announces new
    /// messages in the bar itself.
    fn build_main_tab_labels(&self) -> Vec<crate::tabs::MainTab> {
        self.tab_layout
            .tabs()
            .iter()
            .map(|&tab| {
                let key = self.tab_switch_key(tab);
                let (label, unread) = match tab {
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
                        let view = self
                            .content_views
                            .get(idx)
                            .and_then(|s| s.as_view())
                            .filter(|cv| cv.has_unread());
                        let marker = view.map(|cv| cv.unread_tab_marker()).unwrap_or_default();
                        (
                            crate::tabs::tab_label_with_marker(marker, &icon, &key, &name),
                            view.map(|cv| cv.unread_tab_style()),
                        )
                    }
                };
                crate::tabs::MainTab { tab, label, unread }
            })
            .collect()
    }

    /// The key hint that selects `tab` for the tab bar: the first entry of
    /// its effective switch binding — the `tab.key` override when set,
    /// otherwise the autonumber digit (`1`..`9`, then `0`). Empty when the
    /// tab has no switch key (disabled, or an 11th+ tab with no digit).
    fn tab_switch_key(&self, tab: Tab) -> String {
        self.tab_switch_entries(tab)
            .into_iter()
            .next()
            .unwrap_or_default()
    }

    /// The surface-form binding entries that switch to `tab`: the view
    /// file's `tab.key` override when present (possibly `[]` → disabled),
    /// otherwise the single positional autonumber digit. Empty when the tab
    /// has no switch key (disabled, or an 11th+ tab with no digit).
    fn tab_switch_entries(&self, tab: Tab) -> Vec<String> {
        let Tab::Content(idx) = tab;
        match self.content_view(idx).and_then(|cv| cv.tab_key_override()) {
            Some(kb) => kb.0.clone(),
            None => self
                .tab_layout
                .digit_for(tab)
                .map(|d| vec![d.to_string()])
                .unwrap_or_default(),
        }
    }

    /// The effective tab-switch [`KeyBinding`] for `tab` (override or
    /// autonumber digit), for matching pressed keys against.
    fn tab_switch_binding(&self, tab: Tab) -> crate::config::keybindings::KeyBinding {
        crate::config::keybindings::KeyBinding(self.tab_switch_entries(tab))
    }

    /// Resolve a fully-pressed key / chord (surface form) to the tab it
    /// switches to, honouring per-tab `tab.key` overrides and falling back
    /// to autonumber digits.
    fn tab_for_pressed(&self, key: &str) -> Option<Tab> {
        self.tab_layout
            .tabs()
            .iter()
            .copied()
            .find(|&tab| self.tab_switch_binding(tab).matches(key))
    }

    /// Whether `key` is a strict prefix of some tab's effective switch
    /// binding — a chorded tab key still waiting to be completed.
    fn tab_switch_is_prefix(&self, key: &str) -> bool {
        self.tab_layout
            .tabs()
            .iter()
            .copied()
            .any(|tab| self.tab_switch_binding(tab).is_prefix(key))
    }

    /// Global-scope [`KeyClaim`]s for the visible tabs' effective switch
    /// bindings, so the shortcut-menu conflict check catches a rebind that
    /// collides with another tab's key (or with a global). Tabs with no
    /// switch key (disabled / no digit) contribute nothing.
    fn tab_switch_claims(&self) -> Vec<crate::keymap::KeyClaim> {
        use crate::keymap::{KeyClaim, KeyScope, KeySource};
        self.tab_layout
            .tabs()
            .iter()
            .filter_map(|&tab| {
                let Tab::Content(idx) = tab;
                let name = self.content_views.get(idx)?.tab_name().to_string();
                let binding = self.tab_switch_binding(tab);
                if binding.0.is_empty() {
                    return None;
                }
                Some(KeyClaim::handler(
                    binding,
                    KeyScope::Global,
                    KeySource::TabSwitch { tab: name },
                ))
            })
            .collect()
    }

    /// Generic tab-switch shortcut rows — one per visible tab, from the
    /// active [`TabLayout`], keyed by its effective switch binding (the
    /// `tab.key` override or the autonumber digit) and labelled with the
    /// tab's real name. Editable from the shortcut menu (Ctrl+N / Ctrl+D /
    /// Ctrl+R) via the [`KeySource::TabSwitch`] source.
    ///
    /// [`KeySource::TabSwitch`]: crate::keymap::KeySource::TabSwitch
    fn tab_switch_rows(&self) -> Vec<crate::keymap::ShortcutRow> {
        use crate::keymap::{KeyScope, KeySource};
        self.tab_layout
            .tabs()
            .iter()
            .filter_map(|&tab| {
                let Tab::Content(idx) = tab;
                let name = self.content_views.get(idx)?.tab_name().to_string();
                let entries = self.tab_switch_entries(tab);
                Some(crate::keymap::ShortcutRow {
                    name: format!("Switch to {name}"),
                    keys: entries.join(" / "),
                    scope: "Global".to_string(),
                    source: Some(KeySource::TabSwitch { tab: name }),
                    key_scope: Some(KeyScope::Global),
                })
            })
            .collect()
    }

    /// Open / close the adapter credentials popup based on a fresh status.
    ///
    /// Acts for the active tab and for eager background tabs (see
    /// [`credential_form_allowed`]). There is one popup slot, so whoever asks
    /// first keeps it — a second adapter asking meanwhile is not dropped, its
    /// `NeedsCreds` stays on the view and is replayed when its tab is opened.
    fn react_to_adapter_status(
        &mut self,
        view_index: usize,
        status: &not_yet_done_content::AdapterStatus,
    ) {
        match status {
            not_yet_done_content::AdapterStatus::NeedsCreds {
                fields,
                header,
                error,
            } => {
                let Tab::Content(active) = self.active_tab;
                let manual = self
                    .content_view(view_index)
                    .is_none_or(|cv| cv.manual_connect);
                let owner = self.adapter_creds_popup.as_ref().map(|p| p.view_index());
                if !credential_form_allowed(active, view_index, manual, owner) {
                    return;
                }
                let tab_name = self.content_view(view_index).map(|cv| cv.tab_name.clone());
                let title = credential_form_title(header.as_deref(), tab_name.as_deref());
                let already_open = self.adapter_creds_popup.as_ref().is_some_and(|p| {
                    p.view_index() == view_index
                        && p.is_open()
                        && p.shows(&title, fields, error.as_deref())
                });
                if !already_open {
                    let mut popup = crate::components::adapter_creds_popup::AdapterCredsPopup::new(
                        Arc::clone(&self.shared_theme),
                        title,
                        view_index,
                        fields.clone(),
                    );
                    if let Some(e) = error {
                        popup.set_error(e.clone());
                    }
                    self.adapter_creds_popup = Some(popup);
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

        // Content tabs need their table rebuilt so the header reflects
        // the current overlay.
        {
            let Tab::Content(idx) = self.active_tab;
            if let Some(cv) = self.content_view_mut(idx) {
                cv.rebuild_table();
            }
        }

        self.tab_bar.set_active_tab(self.active_tab);
        let main_tab_labels = self.build_main_tab_labels();
        self.tab_bar.set_main_tab_labels(main_tab_labels);
        let subtab_labels: Vec<(usize, Vec<(String, bool)>)> = self
            .content_views_indexed()
            .map(|(idx, cv)| (idx, cv.subtab_labels()))
            .collect();
        for (idx, labels) in subtab_labels {
            self.tab_bar.set_content_sub_tabs(idx, labels);
        }

        // Action bar lives on each view; push state in. "Tracking active"
        // is a global highlight — OR the in-memory active set across every
        // open adapter (the task/tracking adapters report it; remote
        // adapters default to `false`). Routed through the adapter contract
        // so the App no longer queries the tracking repo directly.
        let tracking_active = self
            .content_views_iter()
            .any(|cv| cv.adapter.as_ref().is_some_and(|a| a.has_active_tracking()));
        let session = self.pending_session.as_ref();
        let session_label = |scope: crate::edit_session::SessionScope| -> Option<&str> {
            session.filter(|s| s.scope() == scope).map(|s| s.label())
        };
        let content_active_editor =
            session_label(crate::edit_session::SessionScope::Content).map(|s| s.to_string());
        // Action id of the focused content editor + of any open action picker
        // popup — let a modal `custom` action (Taiga `convert`) light up its
        // top-bar hint across both phases of its menu→editor flow.
        let content_editor_action_id = session
            .filter(|s| s.scope() == crate::edit_session::SessionScope::Content)
            .map(|s| s.action_id().to_string())
            .filter(|s| !s.is_empty());
        let content_action_popup_id = self
            .content_action_popup
            .as_ref()
            .map(|p| p.action_id.clone());
        let cut_active = self.content_marked_node.is_some();
        // Active sources owned by the App (popups / detached scripts) — pushed
        // into the focused content view so its hint resolver can light up the
        // matching top-bar shortcut while the affordance is open.
        let confirm_active = matches!(
            self.pending_confirmation,
            Some((
                _,
                PendingConfirmation::DeleteContentNode { .. }
                    | PendingConfirmation::InvokeNodeAction { .. }
            ))
        );
        let column_config_active = self.column_config_popup.is_some();
        let script_active = self.detached_script.is_some();
        // App-global activatable shortcuts (the shortcut menu today) surface in
        // the content action bar alongside the view's own activatable hints —
        // their key binding and active surface are App-owned, so we resolve
        // `active` here and hand finished hints to the view. Derived from the
        // exhaustive `GlobalAction::placement()` (BarPlacement::Active), so a
        // newly Active-placed global appears automatically once bound.
        let global_action_hints: Vec<crate::components::action_bar::ActionHint> =
            crate::config::keybindings::global_active_hints(&self.keybindings.global)
                .into_iter()
                .map(|(action, key, desc)| {
                    let active = match action {
                        GlobalAction::ShortcutMenu => self.shortcut_menu.is_open(),
                        _ => false,
                    };
                    crate::components::action_bar::ActionHint { key, desc, active }
                })
                .collect();
        {
            let Tab::Content(idx) = self.active_tab;
            if let Some(cv) = self.content_view_mut(idx) {
                cv.sync_action_bar(
                    content_active_editor.as_deref(),
                    content_editor_action_id.as_deref(),
                    content_action_popup_id.as_deref(),
                    tracking_active,
                    cut_active,
                    confirm_active,
                    column_config_active,
                    script_active,
                    global_action_hints,
                );
            }
        }

        {
            let Tab::Content(idx) = self.active_tab;
            // Leading app-global hints (quit, cycle tabs) are derived from the
            // exhaustive `GlobalAction::placement()` — see
            // `keybindings::global_status_hints`. Content-specific hints and
            // the sort mode follow, so the fixed global frame stays first.
            let mut hints: Vec<(String, String)> =
                crate::config::keybindings::global_status_hints(&self.keybindings.global);
            if let Some(cv) = self.content_view(idx) {
                for (k, v) in cv.status_bar_hints() {
                    hints.push((k, v));
                }
            }
            hints.push((
                self.keybindings.common.label(&CommonAction::SortMode),
                "sort".to_string(),
            ));
            self.status_bar.set_custom_hints(hints);
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

    fn process_sub_view_message(&mut self, msg: SubViewMessage) -> EditorRequest {
        match msg {
            SubViewMessage::Request(req) => self.process_view_request(req),
            SubViewMessage::SelectionChanged(_) => EditorRequest::None,
            SubViewMessage::FuzzyStateChanged { .. } => {
                // No live emitter remains after the legacy Trackings tab
                // removal; kept for SubViewMessage exhaustiveness.
                EditorRequest::None
            }
            SubViewMessage::SearchStateChanged { .. } => {
                // Search state now lives on views; handled via Searchable trait.
                EditorRequest::None
            }
            SubViewMessage::EditorOpened(_) | SubViewMessage::EditorClosed => EditorRequest::None,
            SubViewMessage::ActionBarHints(_) | SubViewMessage::StatusBarHints(_) => {
                EditorRequest::None
            }
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
            ViewRequest::OpenColumnConfig => {
                self.open_column_config_popup();
                EditorRequest::None
            }
            ViewRequest::PersistCardMode { view_index } => {
                self.persist_card_mode(view_index);
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
            ViewRequest::FetchContentPreview {
                view_index,
                pane_id,
                cache_key,
                node_id,
                action_id,
            } => {
                let adapter = self
                    .content_view(view_index)
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
                            view_index,
                            pane_id,
                            cache_key,
                            text,
                        });
                    }
                });
                EditorRequest::None
            }
            ViewRequest::OpenContentEditor {
                view_index,
                pane_id,
                node_id,
                action_id,
                label,
                editor_profile,
                commit_on_save,
            } => {
                self.open_content_editor(
                    view_index,
                    pane_id,
                    node_id,
                    action_id,
                    label,
                    editor_profile,
                    commit_on_save,
                );
                EditorRequest::None
            }
            ViewRequest::SpawnContentLoad {
                view_index,
                pane_id,
            } => {
                self.spawn_content_load(view_index, pane_id);
                EditorRequest::None
            }
            ViewRequest::ReloadContentCurrentLevel {
                view_index,
                pane_id,
            } => {
                self.reload_content_pane_current_level(view_index, pane_id);
                EditorRequest::None
            }
            ViewRequest::ForceReloadContent {
                view_index,
                pane_id,
            } => {
                self.spawn_content_reload(view_index, pane_id);
                EditorRequest::None
            }
            ViewRequest::TreeFindStart {
                view_index,
                pane_id,
                query,
            } => {
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
            ViewRequest::ApplyContentSavedQuery {
                view_index,
                pane_id,
                query,
                name,
                kind,
            } => {
                let target = crate::components::query_var_popup::QueryVarPopupTarget {
                    tab_idx: view_index,
                    pane_id,
                    raw_query: query,
                    saved_name: Some(name),
                    kind,
                };
                self.start_query_apply(target, std::collections::HashMap::new(), true);
                EditorRequest::None
            }
            ViewRequest::DrillDown {
                view_index,
                pane_id,
                node_id,
                node_label: _,
                child_node_type,
            } => {
                self.spawn_content_drill_down(view_index, pane_id, node_id, child_node_type);
                EditorRequest::None
            }
            ViewRequest::ExpandTreeNode {
                view_index,
                pane_id,
                parent_path,
                parent_node_id,
                child_node_type,
                page_size,
                page,
                append,
            } => {
                self.spawn_tree_expand(
                    view_index,
                    pane_id,
                    parent_path,
                    parent_node_id,
                    child_node_type,
                    page_size,
                    page,
                    append,
                );
                EditorRequest::None
            }
            ViewRequest::EagerExpandSubtree {
                view_index,
                pane_id,
            } => {
                // Fuzzy filter opened on an eager tree: pull the whole subtree
                // so the filter matches across collapsed / unpaged branches.
                self.spawn_subtree_load(view_index, pane_id, u32::MAX);
                EditorRequest::None
            }
            ViewRequest::DriveTreeAutoExpand {
                view_index,
                pane_id,
            } => {
                // `zr` armed the pane's unbounded-depth override; pump the
                // same cascade a fresh load uses so every node unfolds.
                self.drive_tree_auto_expand(view_index, pane_id);
                EditorRequest::None
            }
            ViewRequest::ExpandTreeNodeMulti {
                view_index,
                pane_id,
                parent_path,
                parent_node_id,
                child_node_types,
                page_size,
            } => {
                if let Some(cv) = self.content_view_mut(view_index) {
                    cv.begin_tree_multi_load(
                        pane_id,
                        parent_path.clone(),
                        child_node_types.clone(),
                    );
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
            ViewRequest::OpenContentQueryEditor {
                view_index,
                pane_id: _,
                save_name,
                is_new,
                kind,
            } => self.open_content_query_editor(view_index, save_name, is_new, kind),
            ViewRequest::OpenAdapterQueryEditor {
                view_index,
                pane_id,
                parent_node_id,
            } => self.open_adapter_query_editor(view_index, pane_id, parent_node_id),
            ViewRequest::OpenNodeScriptsMenu {
                view_index,
                pane_id,
                node_id,
            } => self.open_node_scripts_menu(view_index, pane_id, node_id),
            ViewRequest::RunNodeScript {
                view_index,
                pane_id,
                node_id,
                script,
            } => self.run_node_script(view_index, pane_id, node_id, script),
            ViewRequest::RunAdapterQuery {
                view_index,
                pane_id,
                node_id,
                query,
                page,
                cursor,
            } => {
                self.spawn_adapter_query(view_index, pane_id, node_id, query, Some(page), cursor);
                EditorRequest::None
            }
            ViewRequest::CloseAdapterCursor {
                view_index,
                cursor_id,
            } => {
                self.spawn_close_adapter_cursor(view_index, cursor_id);
                EditorRequest::None
            }
            ViewRequest::RunAdapterDbScript {
                view_index,
                pane_id,
                source_node_id,
                source_label,
                database,
                sql,
            } => {
                self.run_adapter_db_script(
                    view_index,
                    pane_id,
                    source_node_id,
                    source_label,
                    database,
                    sql,
                );
                EditorRequest::None
            }
            ViewRequest::OpenAdapterDbScriptEditor {
                view_index,
                pane_id,
                database,
                script,
                in_place,
            } => {
                self.open_adapter_db_script_editor(view_index, pane_id, database, script, in_place)
            }
            ViewRequest::ConfirmDeleteContentNode {
                view_index,
                pane_id,
                node_id,
                action_name,
                confirm,
            } => {
                self.confirm_delete_content_node(
                    view_index,
                    pane_id,
                    node_id,
                    action_name,
                    confirm,
                );
                EditorRequest::None
            }
            ViewRequest::ConfirmInvokeNodeAction {
                view_index,
                pane_id,
                node_id,
                action_name,
                prompt,
            } => {
                self.confirm_invoke_node_action(view_index, pane_id, node_id, action_name, prompt);
                EditorRequest::None
            }
            ViewRequest::InvokeContainerAction {
                view_index,
                pane_id,
                action_name,
            } => {
                self.spawn_invoke_container_action(view_index, pane_id, action_name);
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
            ViewRequest::EditNodeScript {
                view_index,
                pane_id,
                node_id,
                script,
                is_new,
            } => self.edit_node_script(view_index, pane_id, node_id, script, is_new),
            ViewRequest::DeleteNodeScript {
                view_index,
                pane_id,
                node_id,
                script,
            } => {
                self.delete_node_script(view_index, pane_id, node_id, script);
                EditorRequest::None
            }
            ViewRequest::PromptNodeScriptShortcut {
                view_index,
                pane_id: _,
                node_id,
                script,
            } => {
                self.prompt_node_script_shortcut(view_index, node_id, script);
                EditorRequest::None
            }
            ViewRequest::ClearNodeScriptShortcut {
                view_index,
                pane_id: _,
                node_id,
                script,
            } => {
                self.clear_node_script_shortcut(view_index, node_id, script);
                EditorRequest::None
            }
            ViewRequest::ExecuteContentAction {
                view_index,
                pane_id,
                node_id,
                action_id,
            } => {
                self.open_content_action_popup(view_index, pane_id, node_id, action_id);
                EditorRequest::None
            }
            ViewRequest::InvokeNodeAction {
                view_index,
                pane_id,
                node_id,
                action_name,
            } => {
                self.spawn_invoke_node_action(
                    view_index,
                    pane_id,
                    node_id,
                    action_name,
                    false,
                    None,
                );
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
            ViewRequest::CreateContentChild {
                view_index,
                pane_id,
                parent_node_id,
                child_node_type,
                action_id,
                label,
                editor_profile,
                commit_on_save,
            } => {
                let adapter = self
                    .content_view(view_index)
                    .and_then(|cv| cv.adapter.as_ref())
                    .map(Arc::clone);
                let Some(adapter) = adapter else {
                    self.notify("No adapter available".to_string());
                    return EditorRequest::None;
                };
                let reload = Some(crate::edit_session::ReloadTarget {
                    view_index,
                    pane_id,
                });
                let result = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        // `None` parent → create on the adapter's root container
                        // (empty tree / un-drilled root level). Resolve its id
                        // here, in the same async context that builds the session.
                        let node_id = match parent_node_id {
                            Some(id) => id,
                            None => adapter
                                .root()
                                .await
                                .map_err(|e| e.to_string())?
                                .id()
                                .to_string(),
                        };
                        let nav = crate::edit_session::NavContext {
                            view_index,
                            parent_node_id: node_id.clone(),
                            child_node_type,
                        };
                        crate::edit_session::NodeActionEditSession::new(
                            adapter,
                            node_id,
                            action_id,
                            label,
                            Some(nav),
                            reload,
                            editor_profile,
                            commit_on_save,
                        )
                        .await
                        .map_err(|e| e.to_string())
                    })
                });
                match result {
                    Ok(session) => self.open_session(Box::new(session)),
                    Err(e) => {
                        self.notify_error(format!("Failed to create content child: {e}"));
                        EditorRequest::None
                    }
                }
            }
            ViewRequest::SaveContentQuery {
                view_index,
                scope: _,
                name,
                query,
            } => {
                let kind = self.content_query_kind(view_index, &name);
                self.save_content_query_body(view_index, &name, &query, kind);
                self.reload_content_saved_queries(view_index);
                EditorRequest::None
            }
            ViewRequest::DeleteContentQuery {
                view_index,
                scope,
                name,
            } => {
                // The merged menu list is the authority on a name's kind:
                // it is what the user just picked the entry from.
                let kind = self.content_query_kind(view_index, &name);
                self.delete_content_query(view_index, &scope, &name, kind);
                self.reload_content_saved_queries(view_index);
                self.notify(format!("Deleted query '{name}'"));
                EditorRequest::None
            }
            ViewRequest::SetDefaultContentQuery { view_index, name } => {
                self.set_default_content_query(view_index, &name);
                EditorRequest::None
            }
            ViewRequest::PromptContentQueryShortcut {
                view_index,
                scope,
                name,
                query,
            } => {
                let kind = self.content_query_kind(view_index, &name);
                self.save_content_query_body(view_index, &name, &query, kind);
                self.reload_content_saved_queries(view_index);
                self.modal_message = Some(format!(
                    "Press a shortcut key for '{}'\n\nEsc to cancel",
                    name
                ));
                self.awaiting_favorite_shortcut = Some(PendingFavorite {
                    scope,
                    name,
                    query,
                    kind,
                });
                EditorRequest::None
            }
            ViewRequest::ClearContentQueryShortcut {
                view_index,
                scope,
                name,
            } => {
                self.clear_content_query_shortcut(view_index, &scope, &name);
                self.reload_content_saved_queries(view_index);
                self.notify(format!("Cleared shortcut for '{name}'"));
                EditorRequest::None
            }
            ViewRequest::RenameContentQuery {
                view_index,
                scope,
                old_name,
                new_name,
            } => {
                self.rename_content_query(view_index, &scope, &old_name, &new_name);
                self.reload_content_saved_queries(view_index);
                EditorRequest::None
            }
            ViewRequest::OpenScriptMenuForNode {
                view_index,
                pane_id,
                scope,
                default_field,
            } => {
                use crate::config::view_config::ScriptScope;
                match scope {
                    ScriptScope::Node => self.open_script_menu_for_content(view_index, pane_id),
                    ScriptScope::FilteredSet => {
                        self.open_script_menu_for_content_batch(view_index, pane_id)
                    }
                    ScriptScope::Table => {
                        self.open_script_menu_for_content_table(view_index, pane_id, default_field)
                    }
                }
                EditorRequest::None
            }
            ViewRequest::RunScriptShortcut {
                view_index,
                pane_id,
                name,
            } => self.run_script_shortcut(view_index, pane_id, name),
            ViewRequest::OpenOptionMenuForNode {
                view_index,
                pane_id,
                config,
            } => {
                self.open_option_menu_for_content(view_index, pane_id, config);
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
    /// The pane's currently-active query text — the active override, or the
    /// view's default query if none — normalized to `None` when empty. This
    /// is the same filter string `root_load_request` hands `list`; a
    /// set-scoped adapter action receives it via [`ActionContext::query`] so
    /// it can act on the visible set rather than the whole universe.
    fn pane_active_query(
        &self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
    ) -> Option<String> {
        let cv = self.content_view(view_index)?;
        let pane = cv.find_pane(pane_id)?;
        let q = pane.current_query_text(&cv.view_defs);
        let q = q.trim();
        if q.is_empty() {
            None
        } else {
            Some(q.to_string())
        }
    }

    fn spawn_invoke_node_action(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        node_id: String,
        action_name: String,
        confirmed: bool,
        value: Option<String>,
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
        // Pass the pane's active query so a set-scoped action (e.g.
        // `restore-all`) acts only on the visible set, never the whole
        // universe. Resolved here (needs `&self`) before the spawn.
        let query = self.pane_active_query(view_index, pane_id);
        tokio::spawn(async move {
            // Captured before `value` moves into the context below — used to
            // gate the form-popup reroute (a value-carrying invocation is an
            // option-menu/confirm path, never a form).
            let has_value = value.is_some();
            let ctx = not_yet_done_content::ActionContext {
                marked,
                confirmed,
                query,
                // Frontend-sourced value for value-accepting actions (e.g.
                // an `option_menu` toggle hands over the chosen option id).
                value,
                // Typed free-text is sourced only by the option-menu mutation
                // path ([`App::spawn_option_menu_mutation`]); this generic
                // per-node dispatch carries none.
                text: None,
            };
            // Capture the node's label + type alongside the dispatch so a
            // `mark-move` can populate the clipboard without re-fetching.
            let outcome: not_yet_done_content::Result<
                Option<(
                    not_yet_done_content::ActionDispatch,
                    String,
                    not_yet_done_content::NodeType,
                )>,
            > = async {
                let node = adapter.get_by_id(&node_id_for_task).await?;
                // Form-collecting row actions can't go through `invoke_action`
                // (there is no form dispatch): route them to the popup /
                // `execute` path against this row's node id, mirroring the
                // container path. `confirmed` re-invokes and value/option-menu
                // dispatches never carry a form spec, so guard on the plain
                // first invocation only.
                if !confirmed && !has_value {
                    let wants_form = adapter
                        .actions_for_type(node.node_type())
                        .into_iter()
                        .find(|a| a.id == action_name_for_task)
                        .is_some_and(|a| {
                            matches!(
                                a.input,
                                not_yet_done_content::InputSpec::Form { .. }
                                    | not_yet_done_content::InputSpec::ColumnForm
                            )
                        });
                    if wants_form {
                        let _ = tx.send(LoadMsg::OpenContentActionPopup {
                            view_index,
                            pane_id,
                            node_id: node_id_for_task.clone(),
                            action_id: action_name_for_task.clone(),
                        });
                        return Ok(None);
                    }
                }
                let label = node.label().to_string();
                let node_type = node.node_type().clone();
                let dispatch = node.invoke_action(&action_name_for_task, &ctx).await?;
                Ok(Some((dispatch, label, node_type)))
            }
            .await;
            let (result, node_label, node_type) = match outcome {
                // Rerouted to the form popup — nothing to dispatch here.
                Ok(None) => return,
                Ok(Some((dispatch, label, node_type))) => {
                    (Ok(dispatch), Some(label), Some(node_type))
                }
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

    /// Invoke an adapter action on the pane's *container* (the adapter
    /// `root()`), not on the selected row. Used by `actions:` entries
    /// flagged `on_container: true` (e.g. trackings `restore all`), which
    /// must fire even at the un-drilled flat root where no row — and no
    /// `parent:` target — is addressable.
    ///
    /// We resolve `adapter.root()`, invoke the action with
    /// `confirmed: false`, and route the dispatch through the same
    /// `NodeActionDispatched` → `handle_node_action_dispatched` path as a
    /// per-row action. The dispatch's `node_id` is the root's id, so a
    /// returned `Confirm` re-invokes correctly on the root via
    /// `spawn_invoke_node_action` (which `get_by_id`s the root again).
    fn spawn_invoke_container_action(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
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
        // The container action scopes to the pane's active query (see
        // `spawn_invoke_node_action`): resolve it before the spawn.
        let query = self.pane_active_query(view_index, pane_id);
        tokio::spawn(async move {
            // Resolve the container (root) node first, then decide the path.
            let root = match adapter.root().await {
                Ok(node) => node,
                Err(e) => {
                    let _ = tx.send(LoadMsg::NodeActionDispatched {
                        view_index,
                        pane_id,
                        node_id: String::new(),
                        action_name: action_name_for_task.clone(),
                        result: Err(format!("Action '{action_name_for_task}': {e}")),
                        node_label: None,
                        node_type: None,
                    });
                    return;
                }
            };
            let node_id = root.id().to_string();
            // Form-collecting container actions can't go through
            // `invoke_action` (there is no form dispatch): route them to the
            // popup / `execute` path against the root node id instead, exactly
            // like a per-row form action. Every other container action keeps
            // the `invoke_action` → `ActionDispatch` dispatch (Confirm, mark /
            // paste-move, Reload, …).
            let wants_form = adapter
                .actions_for_type(root.node_type())
                .into_iter()
                .find(|a| a.id == action_name_for_task)
                .is_some_and(|a| {
                    matches!(
                        a.input,
                        not_yet_done_content::InputSpec::Form { .. }
                            | not_yet_done_content::InputSpec::ColumnForm
                    )
                });
            if wants_form {
                let _ = tx.send(LoadMsg::OpenContentActionPopup {
                    view_index,
                    pane_id,
                    node_id,
                    action_id: action_name_for_task,
                });
                return;
            }

            let label = root.label().to_string();
            let node_type = root.node_type().clone();
            let ctx = not_yet_done_content::ActionContext {
                query,
                ..Default::default()
            };
            let (result, node_label, node_type) =
                match root.invoke_action(&action_name_for_task, &ctx).await {
                    Ok(dispatch) => (Ok(dispatch), Some(label), Some(node_type)),
                    Err(e) => (
                        Err(format!("Action '{action_name_for_task}': {e}")),
                        None,
                        None,
                    ),
                };
            let _ = tx.send(LoadMsg::NodeActionDispatched {
                view_index,
                pane_id,
                node_id,
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
                // Re-marking the already-marked node toggles the cut off
                // (e.g. two `C` in a row on the same channel). A cut never
                // deletes — it only ever cancels here or relocates on paste.
                if self
                    .content_marked_node
                    .as_ref()
                    .is_some_and(|m| m.node_id == node_id)
                {
                    self.content_marked_node = None;
                    self.notify("Cut cancelled".to_string());
                    return;
                }
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
        // A `Reload` dispatch means the adapter mutated state (e.g.
        // removing a bookmark) — invalidate sibling subtabs so they
        // re-load on next switch, mirroring the `ContentActionDone` path.
        if matches!(dispatch, not_yet_done_content::ActionDispatch::Reload) {
            if let Some(cv) = self.content_view_mut(view_index) {
                cv.invalidate_sibling_subtabs();
            }
        }
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
                    AuthInvalidate::Session => {
                        "Session invalidated, re-authenticating…".to_string()
                    }
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

    fn open_content_action_popup(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        node_id: String,
        action_id: String,
    ) {
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
                    adapter
                        .actions_for_type(node.node_type())
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
                        node.execute(&action_id, not_yet_done_content::ActionInput::None)
                            .await
                    }
                    .await;
                    // `OpenExternal` takes its own message so the app can hand
                    // the file to the OS viewer instead of reloading the pane.
                    if let Ok(not_yet_done_content::ActionOutcome::OpenExternal {
                        target,
                        message,
                    }) = &outcome
                    {
                        let _ = tx.send(LoadMsg::ContentOpenExternal {
                            target: target.clone(),
                            message: message.clone(),
                        });
                        return;
                    }
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
                    FilePickerStyle, SelectListStyle, SelectListStyleType, TextInputStyle,
                    TextInputStyleType,
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
                    .set_style(
                        TextInputStyleType::Title,
                        Style::default().fg(primary).bg(panel_bg),
                    )
                    .set_style(
                        TextInputStyleType::Input,
                        Style::default().fg(text_high).bg(panel_bg),
                    )
                    .placeholder_color(dim);
                let text_active = TextInputStyle::new()
                    .prefix_color(accent)
                    .set_style(
                        TextInputStyleType::Title,
                        Style::default()
                            .fg(accent)
                            .bg(input_bg)
                            .add_modifier(Modifier::BOLD),
                    )
                    .set_style(
                        TextInputStyleType::Input,
                        Style::default().fg(text_high).bg(input_bg),
                    )
                    .placeholder_color(dim);

                let list_inactive = SelectListStyle::default()
                    .prefix_color(primary)
                    .placeholder_color(dim)
                    .set_style(
                        SelectListStyleType::Item,
                        Style::default().fg(text_high).bg(panel_bg),
                    )
                    .set_style(
                        SelectListStyleType::ItemSelected,
                        Style::default().fg(text_high).bg(input_bg),
                    )
                    .set_style(
                        SelectListStyleType::ItemCursor,
                        Style::default().fg(text_high).bg(panel_bg),
                    )
                    .set_style(
                        SelectListStyleType::ItemCursorSelected,
                        Style::default().fg(text_high).bg(input_bg),
                    )
                    .set_style(
                        SelectListStyleType::FilterInput,
                        Style::default().fg(dim).bg(panel_bg),
                    )
                    .set_style(
                        SelectListStyleType::Footer,
                        Style::default().fg(dim).bg(panel_bg),
                    );
                let list_active = SelectListStyle::default()
                    .prefix_color(accent)
                    .placeholder_color(dim)
                    .set_style(
                        SelectListStyleType::Item,
                        Style::default().fg(text_high).bg(input_bg),
                    )
                    .set_style(
                        SelectListStyleType::ItemSelected,
                        Style::default().fg(text_high).bg(panel_bg),
                    )
                    .set_style(
                        SelectListStyleType::ItemCursor,
                        Style::default()
                            .fg(accent)
                            .bg(input_bg)
                            .add_modifier(Modifier::BOLD),
                    )
                    .set_style(
                        SelectListStyleType::ItemCursorSelected,
                        Style::default()
                            .fg(accent)
                            .bg(panel_bg)
                            .add_modifier(Modifier::BOLD),
                    )
                    .set_style(
                        SelectListStyleType::FilterInput,
                        Style::default().fg(text_high).bg(input_bg),
                    )
                    .set_style(
                        SelectListStyleType::FilterCursor,
                        Style::default()
                            .fg(input_bg)
                            .bg(accent)
                            .add_modifier(Modifier::BOLD),
                    )
                    .set_style(
                        SelectListStyleType::Footer,
                        Style::default().fg(dim).bg(input_bg),
                    );

                let picker_style = FilePickerStyle::new()
                    .with_text_input_inactive(text_inactive)
                    .with_text_input_active(text_active)
                    .with_select_list_inactive(list_inactive)
                    .with_select_list_active(list_active)
                    .with_panel_bg(panel_bg)
                    .with_title_style(
                        Style::default()
                            .fg(accent)
                            .bg(panel_bg)
                            .add_modifier(Modifier::BOLD),
                    )
                    .with_help_keys_style(
                        Style::default()
                            .fg(primary)
                            .bg(panel_bg)
                            .add_modifier(Modifier::BOLD),
                    )
                    .with_help_labels_style(Style::default().fg(dim).bg(panel_bg))
                    .with_paste_error_style(
                        Style::default()
                            .fg(theme.error())
                            .bg(panel_bg)
                            .add_modifier(Modifier::BOLD),
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
                self.open_content_form_popup(
                    adapter, view_index, pane_id, node_id, action_id, fields, None,
                );
            }
            not_yet_done_content::InputSpec::ColumnForm => {
                // The fields come from *this view's* `source: custom` columns —
                // the front-end's own typed config — not from the backend
                // schema. So a column that has never been written still gets an
                // input, and its YAML `kind:` supplies the `value_type` the TUI
                // sends on submit, letting the store bootstrap the column on
                // first write (type-on-first-write). No `describe_columns`
                // round-trip, no separate "define a column" step.
                let custom = self
                    .content_view(view_index)
                    .and_then(|cv| {
                        cv.find_pane(pane_id)
                            .map(|p| p.custom_column_fields(&cv.view_defs))
                    })
                    .unwrap_or_default();
                if custom.is_empty() {
                    self.notify("No custom columns configured in this view".to_string());
                    return;
                }
                let column_types = custom
                    .iter()
                    .map(|c| (c.key.clone(), c.value_type.clone()))
                    .collect();
                let fields = custom
                    .into_iter()
                    .map(|c| not_yet_done_content::FormFieldSpec::text(c.key, c.label).optional())
                    .collect();
                self.open_content_form_popup(
                    adapter,
                    view_index,
                    pane_id,
                    node_id,
                    action_id,
                    fields,
                    Some(column_types),
                );
            }
        }
    }

    /// Shared tail for the two form-shaped input specs
    /// ([`InputSpec::Form`](not_yet_done_content::InputSpec::Form) and
    /// [`InputSpec::ColumnForm`](not_yet_done_content::InputSpec::ColumnForm)):
    /// prefill from the node (edit flow, static `default` fallback inside the
    /// popup) and open the generic form popup.
    fn open_content_form_popup(
        &mut self,
        adapter: Arc<dyn not_yet_done_content::ContentAdapter>,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        node_id: String,
        action_id: String,
        fields: Vec<not_yet_done_content::FormFieldSpec>,
        column_types: Option<std::collections::HashMap<String, String>>,
    ) {
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
        // Resolve the per-action layout/style (columns, field bar, inline vs
        // dropdown selects) from the pane's `ActionDef.form` block — matched by
        // the adapter-side action `id`, the same identifier the popup carries.
        // Absent (or no matching action) → the driver's classic single-column,
        // no-bar, dropdown defaults, so existing forms are unchanged.
        let form_cfg = self.content_view(view_index).and_then(|cv| {
            let pane = cv.find_pane(pane_id)?;
            let vd = cv.view_defs.get(pane.view_def_index())?;
            vd.actions
                .iter()
                .find(|a| a.id.as_deref() == Some(action_id.as_str()))
                .and_then(|a| a.form.clone())
        });
        let form_options = Self::build_form_options(form_cfg.as_ref(), &fields);
        let popup = ContentFormPopup::new(
            action_id.clone(),
            fields,
            &prefill,
            &self.theme,
            &form_options,
        );
        self.content_form_popup = Some(ContentFormPopupState {
            popup,
            view_index,
            pane_id,
            node_id,
            action_id,
            column_types,
        });
    }

    /// Translate a per-action [`ActionFormConfig`] (view YAML) into the
    /// driver-level [`FormOptions`]. Absent config → today's defaults
    /// (1 column, no bar, dropdown selects). `column_assignment` lists field
    /// **keys** per column; they're resolved to per-field column indices
    /// (`column_of`) against the actual `fields`, with unlisted fields left in
    /// column 0.
    fn build_form_options(
        cfg: Option<&crate::config::view_config::ActionFormConfig>,
        fields: &[not_yet_done_content::FormFieldSpec],
    ) -> not_yet_done_ratatui::FormOptions {
        use crate::config::view_config::SelectStyleConfig;
        use not_yet_done_ratatui::{FormOptions, SelectStyle};

        let mut opts = FormOptions::default();
        let Some(cfg) = cfg else {
            return opts;
        };
        if let Some(c) = cfg.columns {
            opts.columns = c;
        }
        if let Some(b) = cfg.field_bar {
            opts.field_bar = b;
        }
        if let Some(s) = cfg.select_style {
            opts.select_style = match s {
                SelectStyleConfig::Inline => SelectStyle::Inline,
                SelectStyleConfig::Dropdown => SelectStyle::Dropdown,
            };
        }
        if let Some(assign) = &cfg.column_assignment {
            let mut column_of = vec![0usize; fields.len()];
            for (col_idx, keys) in assign.iter().enumerate() {
                for key in keys {
                    if let Some(fi) = fields.iter().position(|f| &f.key == key) {
                        column_of[fi] = col_idx;
                    }
                }
            }
            opts.column_of = column_of;
        }
        opts
    }

    /// Fetch the columns the adapter *describes* for each given node type
    /// off-thread (3b) and route them back via [`LoadMsg::ContentColumnSchema`]
    /// so their backend-authoritative types merge into column rendering. Fired
    /// after a content load. An empty schema (the common case — no custom
    /// columns) is dropped rather than sent, so a view without described
    /// columns triggers no extra rebuilds.
    fn refresh_column_schema(&self, view_index: usize, node_types: Vec<String>) {
        if node_types.is_empty() {
            return;
        }
        let Some(adapter) = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone)
        else {
            return;
        };
        let tx = self.load_tx.clone();
        for node_type in node_types {
            let adapter = Arc::clone(&adapter);
            let tx = tx.clone();
            tokio::spawn(async move {
                let schema = adapter.describe_columns(&node_type).await;
                if schema.is_empty() {
                    return;
                }
                let _ = tx.send(LoadMsg::ContentColumnSchema {
                    view_index,
                    node_type,
                    schema,
                });
            });
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
                node.execute(&action_id, not_yet_done_content::ActionInput::Files(paths))
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
        column_types: Option<std::collections::HashMap<String, String>>,
    ) {
        let adapter = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone);
        let Some(adapter) = adapter else {
            self.notify("No adapter available".to_string());
            return;
        };
        // A `ColumnForm` carries per-field types (from the view YAML `kind:`),
        // so it submits typed cells the backend can bootstrap on first write;
        // a plain `Form` submits the untyped value map.
        let input = match column_types {
            Some(types) => not_yet_done_content::ActionInput::ColumnForm(
                values
                    .into_iter()
                    .map(|(key, value)| {
                        let value_type = types
                            .get(&key)
                            .cloned()
                            .unwrap_or_else(|| "text".to_string());
                        not_yet_done_content::ColumnCellInput {
                            key,
                            value,
                            value_type,
                        }
                    })
                    .collect(),
            ),
            None => not_yet_done_content::ActionInput::Form(values),
        };
        let tx = self.load_tx.clone();
        let vi = view_index;
        let pid = pane_id;
        let aid_for_msg = action_id.clone();
        tokio::spawn(async move {
            let outcome = async {
                let mut node = adapter.get_by_id(&node_id).await?;
                node.execute(&action_id, input).await
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

    /// Build a `NodeActionEditSession` off-thread and open it in `$EDITOR`.
    /// Shared by the `OpenContentEditor` ViewRequest and the
    /// `OpenContentEditorForAction` LoadMsg — the latter is the Picker→editor
    /// chaining produced by [`ActionOutcome::OpenEditor`], where a menu step
    /// selects the target action id and then opens its type-specific editor
    /// on the same node.
    fn open_content_editor(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        node_id: String,
        action_id: String,
        label: String,
        editor_profile: Option<String>,
        commit_on_save: bool,
    ) {
        let adapter = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone);
        let Some(adapter) = adapter else {
            self.notify("No adapter available".to_string());
            return;
        };
        // Reject a second open while one is already up *or* still
        // loading. `open_session` has its own busy guard, but that
        // only catches the window after the detached child exists;
        // this closes the gap while the off-thread prepare runs.
        if self.editor_busy() {
            self.notify("Editor is already open".to_string());
            return;
        }
        // Build the session off-thread. Its `prepare` does the
        // network-heavy metadata/comment fetches that previously ran
        // under a `block_on` on the render thread — a dead connection
        // there froze the whole TUI. Now the ready session arrives via
        // `LoadMsg::EditorSessionReady` and is opened from
        // `handle_load_msg`; the wait is bounded by the adapter's own
        // request timeout, and the UI stays responsive throughout.
        let reload = Some(crate::edit_session::ReloadTarget {
            view_index,
            pane_id,
        });
        self.editor_load_token = self.editor_load_token.wrapping_add(1);
        let token = self.editor_load_token;
        self.notification_bar.set_keyed(
            EDITOR_LOADING_SLOT,
            crate::components::notification_bar::NoticeClass::Message,
            format!("⏳ Opening editor: {label}…"),
        );
        self.editor_loading = true;
        let tx = self.load_tx.clone();
        tokio::spawn(async move {
            let result = crate::edit_session::NodeActionEditSession::new(
                adapter,
                node_id.clone(),
                action_id,
                label,
                None,
                reload,
                editor_profile,
                commit_on_save,
            )
            .await
            .map(|s| Box::new(s) as Box<dyn crate::edit_session::EditSession>)
            .map_err(|e| e.to_string());
            let _ = tx.send(LoadMsg::EditorSessionReady {
                node_id,
                token,
                result,
            });
        });
    }

    fn execute_content_action(
        &mut self,
        view_index: usize,
        pane_id: crate::views::content_view::PaneId,
        node_id: String,
        action_id: String,
        value: String,
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
        tokio::spawn(async move {
            // `open_editor` short-circuits the ContentActionDone path: an
            // `ActionOutcome::OpenEditor` is a menu step (e.g. Taiga's
            // convert target picker) that must chain into a type-specific
            // editor on the same node rather than notify + reload.
            let mut open_editor: Option<(String, String)> = None;
            let result = match adapter.get_by_id(&node_id).await {
                Err(e) => Err(format!("Action failed: {action_id}: {e}")),
                Ok(mut node) => {
                    match node
                        .execute(&action_id, not_yet_done_content::ActionInput::Picked(value))
                        .await
                    {
                        Ok(not_yet_done_content::ActionOutcome::Done { message }) => {
                            Ok(message.unwrap_or_else(|| format!("{action_id} executed")))
                        }
                        Ok(not_yet_done_content::ActionOutcome::OpenEditor { action_id: next }) => {
                            // Resolve the editor label from the type-level
                            // action set (the picked value is a real action id
                            // present in `actions_for_type`); fall back to the id.
                            let label = adapter
                                .actions_for_type(node.node_type())
                                .into_iter()
                                .find(|a| a.id == next)
                                .map(|a| a.label)
                                .unwrap_or_else(|| next.clone());
                            open_editor = Some((next, label));
                            Ok(String::new())
                        }
                        Ok(_) => Ok(format!("{action_id} executed")),
                        Err(e) => Err(format!("Action failed: {action_id}: {e}")),
                    }
                }
            };
            if let Some((next, label)) = open_editor {
                let _ = tx.send(LoadMsg::OpenContentEditorForAction {
                    view_index: vi,
                    pane_id: pid,
                    node_id,
                    action_id: next,
                    label,
                });
            } else {
                let _ = tx.send(LoadMsg::ContentActionDone {
                    view_index: vi,
                    pane_id: pid,
                    result,
                });
            }
        });
    }

    fn handle_common_action(&mut self, action: CommonAction) -> EditorRequest {
        match action {
            CommonAction::ListNext => {
                if let Some(table) = self.active_table_mut() {
                    table.handle_nav(tuirealm::command::Cmd::Move(
                        tuirealm::command::Direction::Down,
                    ));
                }
            }
            CommonAction::ListPrev => {
                if let Some(table) = self.active_table_mut() {
                    table.handle_nav(tuirealm::command::Cmd::Move(
                        tuirealm::command::Direction::Up,
                    ));
                }
            }
            CommonAction::ListFirst => {
                if let Some(table) = self.active_table_mut() {
                    table.handle_nav(tuirealm::command::Cmd::GoTo(
                        tuirealm::command::Position::Begin,
                    ));
                }
            }
            CommonAction::ListLast => {
                if let Some(table) = self.active_table_mut() {
                    table.handle_nav(tuirealm::command::Cmd::GoTo(
                        tuirealm::command::Position::End,
                    ));
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
            CommonAction::FuzzyFilterOpen => {}
            CommonAction::FuzzyFilterAccept => {}
            CommonAction::FuzzyFilterClear => {}
            CommonAction::FuzzyFilterCancel => {}
            CommonAction::SearchOpen => {}
            CommonAction::SearchNext => {}
            CommonAction::SearchPrev => {}
            CommonAction::SavedFilterSelect => {}
            CommonAction::FormFilter => {
                // Deprecated — was a separate edit/create popup; the unified
                // query menu (q) now covers create/edit/delete/shortcut.
            }
            CommonAction::ColumnConfig => {
                self.open_column_config_popup();
            }
            CommonAction::FormClose => {}
            CommonAction::FavoriteToggle => {
                // Handled before action resolution when popup is open.
            }
            CommonAction::CommandLineOpen => {
                use crate::views::HasCmdline;
                let Tab::Content(idx) = self.active_tab;
                if let Some(cv) = self.content_view_mut(idx) {
                    cv.cmdline_open();
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
            CommonAction::SortMenu => {
                self.open_sort_menu();
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

    // -----------------------------------------------------------------------
    // Saved filter popup
    // -----------------------------------------------------------------------

    fn open_column_config_popup(&mut self) {
        use crate::components::column_config_popup::ColumnConfigPopup;

        // All tabs are content tabs now; ask the view for the active level's
        // configured columns.
        let Tab::Content(idx) = self.active_tab;
        let (config, entries) = match self
            .content_view(idx)
            .and_then(|cv| cv.column_config_entries())
        {
            Some(pair) => pair,
            None => {
                self.notify("This level has no configurable columns".to_string());
                return;
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
        let Tab::Content(idx) = self.active_tab;
        let Some(cv) = self.content_view_mut(idx) else {
            return;
        };
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

    /// Write a content tab's per-level card-mode map to the settings store.
    /// Mirrors [`Self::apply_column_config`]: one JSON row per tab (level key
    /// → on/off), deleted once the map is empty so a full reset back to the
    /// configured defaults leaves no residue.
    fn persist_card_mode(&mut self, view_index: usize) {
        let settings = Arc::clone(&self.settings_repo);
        let Some(cv) = self.content_view(view_index) else {
            return;
        };
        let key = format!("card_mode:{}", cv.tab_name);
        let value = serde_json::to_string(cv.card_mode_overrides()).unwrap_or_default();
        let empty = cv.card_mode_overrides().is_empty();
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

    fn active_table_mut(&mut self) -> Option<&mut DataTable> {
        let Tab::Content(idx) = self.active_tab;
        self.content_view_mut(idx)
            .map(|cv| &mut cv.active_pane_mut().table)
    }

    /// Open a URL with the configured link opener (default `xdg-open`),
    /// detached so the TUI never blocks on the launched browser. The opener
    /// string is split on whitespace so leading flags work; the URL is the
    /// final argument. Stdio is nulled and the child is placed in its own
    /// process group so it can outlive this session cleanly.
    fn open_link_in_browser(&mut self, url: &str) {
        match self.spawn_link_opener(url) {
            Ok(()) => self.notify(format!("Opening {url}")),
            Err(e) => self.notify(e),
        }
    }

    /// Open a local file (or URL) with the configured link opener. Used when
    /// an adapter action downloaded something (e.g. Stoat image attachments)
    /// and returned [`ActionOutcome::OpenExternal`]: the file is handed to the
    /// platform viewer, whose sibling-navigation then reaches the other files
    /// the action wrote into the same directory. The caller has already shown
    /// the adapter's status message, so a success here is silent; only a
    /// spawn failure is surfaced.
    fn open_external(&mut self, target: &str) {
        if let Err(e) = self.spawn_link_opener(target) {
            self.notify_error(e);
        }
    }

    /// Link-hop landed on an image URL: rather than opening the URL in the
    /// browser, download every image linked from the active pane into one
    /// temp directory and open the picked one in the OS viewer, whose
    /// sibling-navigation then pages through the rest. Downloads route through
    /// the pane's adapter (image hosts commonly sit behind auth); if the
    /// adapter declines (`download_asset` unsupported) or the fetch fails, the
    /// URL falls back to the browser.
    fn open_image_link(&mut self, url: &str) {
        let Tab::Content(idx) = self.active_tab;
        let adapter = self
            .content_view(idx)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone);
        let Some(adapter) = adapter else {
            self.open_link_in_browser(url);
            return;
        };
        // All image links in the pane, with the picked one guaranteed present
        // and opened first.
        let mut all = self
            .content_view(idx)
            .map(|cv| cv.active_pane().image_link_urls())
            .unwrap_or_default();
        let picked = url.to_string();
        if !all.iter().any(|u| u == &picked) {
            all.insert(0, picked.clone());
        }
        let tx = self.load_tx.clone();
        tokio::spawn(async move {
            match download_images_to_temp(adapter.as_ref(), &all, &picked).await {
                Ok((path, count)) => {
                    let message = if count > 1 {
                        Some(format!("Opening image ({count} downloaded)"))
                    } else {
                        Some("Opening image".to_string())
                    };
                    let _ = tx.send(LoadMsg::ContentOpenExternal {
                        target: path.to_string_lossy().into_owned(),
                        message,
                    });
                }
                // Adapter declined or the download failed: hand the URL to the
                // browser instead.
                Err(_) => {
                    let _ = tx.send(LoadMsg::ContentOpenExternal {
                        target: picked.clone(),
                        message: Some(format!("Opening {picked}")),
                    });
                }
            }
        });
    }

    /// Act on a [`Reminder`] fired for `view_index`: look up that tab's
    /// `reminder:` block (the watcher only subscribes for enabled tabs, but
    /// re-check so a mid-session config reload can't fire a stale command) and
    /// run its `command`. The reminder's fields are handed to the command as
    /// environment, never spliced into the string — see [`run_reminder_command`].
    ///
    /// [`Reminder`]: not_yet_done_content::Reminder
    /// [`run_reminder_command`]: Self::run_reminder_command
    fn handle_adapter_reminder(
        &mut self,
        view_index: usize,
        reminder: not_yet_done_content::Reminder,
    ) {
        let Some(cv) = self.content_view(view_index) else {
            return;
        };
        let command = match cv.reminder.as_ref() {
            Some(r) if r.enabled => r.command.clone(),
            _ => {
                reminder_trace(format_args!(
                    "TUI received reminder {:?} but reminder disabled/absent → no command",
                    reminder.title
                ));
                return;
            }
        };
        reminder_trace(format_args!(
            "TUI received reminder {:?} (lead={}min) → running command",
            reminder.title, reminder.lead_minutes
        ));
        if let Err(e) = self.run_reminder_command(&command, &reminder) {
            reminder_trace(format_args!("run_reminder_command FAILED: {e}"));
            self.notify_error(format!("Reminder command failed: {e}"));
        }
    }

    /// Run a tab's reminder `command` via `sh -c`, detached, exporting the
    /// reminder's fields as `NYD_REMINDER_*` environment variables. Passing
    /// them as env (not string interpolation) means an event title can never
    /// be interpreted as shell — the command sees plain data. `NYD_REMINDER_UNTIL`
    /// carries the item's end instant (empty when it has none), so a command can
    /// keep a notification up until the moment is over. Stdio is nulled
    /// and the child joins a fresh process group so it outlives this session
    /// cleanly, exactly like [`spawn_link_opener`](Self::spawn_link_opener).
    fn run_reminder_command(
        &self,
        command: &str,
        reminder: &not_yet_done_content::Reminder,
    ) -> Result<(), String> {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        if command.trim().is_empty() {
            return Err("empty command".to_string());
        }
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .env("NYD_REMINDER_ID", &reminder.id)
            .env("NYD_REMINDER_TITLE", &reminder.title)
            .env(
                "NYD_REMINDER_DETAIL",
                reminder.detail.as_deref().unwrap_or(""),
            )
            .env("NYD_REMINDER_WHEN", &reminder.when)
            .env(
                "NYD_REMINDER_UNTIL",
                reminder.until.as_deref().unwrap_or(""),
            )
            .env(
                "NYD_REMINDER_LEAD_MINUTES",
                reminder.lead_minutes.to_string(),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd.process_group(0);

        cmd.spawn().map(|_child| ()).map_err(|e| e.to_string())
    }

    /// Spawn the configured link opener (default `xdg-open`) on `target`,
    /// detached. The opener string is split on whitespace so leading flags
    /// work; `target` is the final argument. Stdio is nulled and the child is
    /// placed in its own process group so it outlives this session cleanly.
    /// Returns a user-facing error string on misconfiguration or spawn
    /// failure; the caller owns the success notification.
    fn spawn_link_opener(&self, target: &str) -> Result<(), String> {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        let template = self.config.navigation.link_opener.trim();
        let mut parts = template.split_whitespace();
        let Some(program) = parts.next() else {
            return Err("No link opener configured".to_string());
        };
        let args: Vec<&str> = parts.collect();

        let mut cmd = Command::new(program);
        cmd.args(&args)
            .arg(target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Detach into a fresh process group so xdg-open (and the app it execs)
        // never shares this TUI's controlling terminal or pipes.
        cmd.process_group(0);

        cmd.spawn()
            .map(|_child| ())
            .map_err(|e| format!("Failed to open: {e}"))
    }

    /// Load the per-content-tab card-mode map (one JSON row per tab, mapping
    /// level key → on/off) so a mode the user toggled is restored before the
    /// tab's first render. An unparsable row is ignored — the levels then
    /// follow their `card.default`.
    pub fn load_card_mode_for(&mut self, view_index: usize) {
        let Some(tab_name) = self.content_view(view_index).map(|cv| cv.tab_name.clone()) else {
            return;
        };
        let settings = Arc::clone(&self.settings_repo);
        let key = format!("card_mode:{tab_name}");
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async { settings.get(&key).await })
        });
        if let Ok(Some(value)) = result {
            if let Ok(map) = serde_json::from_str::<std::collections::HashMap<String, bool>>(&value)
            {
                if !map.is_empty() {
                    if let Some(cv) = self.content_view_mut(view_index) {
                        cv.set_card_mode_overrides(map);
                    }
                }
            }
        }
    }

    /// Load column configuration from DB for a single slot: the
    /// per-content-tab column overrides (one JSON row per tab, mapping level
    /// key → visible column keys in order). An unparsable row is ignored —
    /// the view then just shows its YAML defaults.
    pub fn load_column_config_for(&mut self, view_index: usize) {
        let Some(tab_name) = self.content_view(view_index).map(|cv| cv.tab_name.clone()) else {
            return;
        };
        let settings = Arc::clone(&self.settings_repo);
        let key = format!("content_columns:{tab_name}");
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async { settings.get(&key).await })
        });
        if let Ok(Some(value)) = result {
            if let Ok(map) =
                serde_json::from_str::<std::collections::HashMap<String, Vec<String>>>(&value)
            {
                if !map.is_empty() {
                    if let Some(cv) = self.content_view_mut(view_index) {
                        cv.set_column_overrides(map);
                    }
                }
            }
        }
    }

    /// Pre-fill a content view's saved sort spec from its adapter's
    /// persistence layer. Runs from [`Self::wire_content_view`], before
    /// the view's first content load fires.
    pub fn load_content_sort_state(&mut self, view_index: usize) {
        use crate::views::SortableView;
        let Some((adapter, scope)) = self.content_view(view_index).and_then(|cv| {
            cv.adapter
                .as_ref()
                .map(|a| (Arc::clone(a), cv.query_scope.clone()))
        }) else {
            return;
        };
        let res = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { adapter.load_view_sort(&scope).await })
        });
        if let Ok(sort) = res {
            if !sort.is_empty() {
                if let Some(cv) = self.content_view_mut(view_index) {
                    SortableView::set_current_sort(cv, sort);
                }
            }
        }
    }

    /// Persist the current sort state of a content view through its
    /// adapter. Called by the sort-mode handler when the user changes
    /// a column's direction.
    pub fn save_content_sort(&self, view_index: usize) {
        use crate::views::SortableView;
        let Some(cv) = self.content_view(view_index) else {
            return;
        };
        let Some(adapter) = cv.adapter.as_ref() else {
            return;
        };
        let adapter = Arc::clone(adapter);
        let scope = cv.query_scope.clone();
        let sort = SortableView::current_sort(cv).to_vec();
        let _ = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { adapter.save_view_sort(&scope, &sort).await })
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
            SortHintPhase::WaitingForColumn {
                target,
                labels,
                columns,
                input,
            } => {
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
                (
                    Some(*target),
                    HeaderOverlay::PickColumn {
                        labels: map,
                        input_len,
                    },
                )
            }
            SortHintPhase::WaitingForDirection {
                target, column_id, ..
            } => (
                Some(*target),
                HeaderOverlay::PickDirection {
                    column_key: column_id.clone(),
                },
            ),
        };

        // Clear all overlays first, then set the target.
        for cv in self.content_views_iter_mut() {
            cv.header_overlay = HeaderOverlay::None;
        }
        if let Some(target) = target {
            match target {
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
    /// map from the active view's [`SortableView::columns`].
    /// No-op for views that expose no sortable columns.
    pub fn enter_sort_hint_mode(&mut self) {
        use crate::views::SortableView;
        let Tab::Content(idx) = self.active_tab;
        let (target, columns) = match self.content_view(idx) {
            Some(cv) => (SortTarget::Content(idx), SortableView::columns(cv)),
            None => return,
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
            SortHintPhase::WaitingForColumn {
                target,
                labels,
                columns,
                mut input,
            } => {
                if key.chars().count() != 1 {
                    self.sort_hint_phase = SortHintPhase::WaitingForColumn {
                        target,
                        labels,
                        columns,
                        input,
                    };
                    return;
                }
                let ch = key.chars().next().unwrap();
                input.push(ch);
                let still_matching: usize =
                    labels.iter().filter(|(_, l)| l.starts_with(&input)).count();
                if still_matching == 0 {
                    self.notify(format!("No sort column for '{}'", input));
                    return;
                }
                if let Some((col_idx, _)) = labels.iter().find(|(_, l)| *l == input) {
                    let col = &columns[*col_idx];
                    self.sort_hint_phase = SortHintPhase::WaitingForDirection {
                        target,
                        column_id: col.key.clone(),
                        column_name: col.display_label().to_string(),
                    };
                    return;
                }
                self.sort_hint_phase = SortHintPhase::WaitingForColumn {
                    target,
                    labels,
                    columns,
                    input,
                };
            }
            SortHintPhase::WaitingForDirection {
                target,
                column_id,
                column_name,
            } => {
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
                            target,
                            column_id,
                            column_name,
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

        self.commit_sort(target, new_sort);
        self.notify(descr);
    }

    /// Store a sort spec on the target view, persist it, and reload when it
    /// actually changed. The single write path: the `S` hint mode and the
    /// sort menu (`c s`) are two UI paths onto this one function.
    /// Returns whether the view's sort changed.
    fn commit_sort(
        &mut self,
        target: SortTarget,
        new_sort: Vec<not_yet_done_content::SortKey>,
    ) -> bool {
        use crate::views::SortableView;

        match target {
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
                changed
            }
        }
    }

    /// Open the sort menu for the active tab — the whole sort spec as one
    /// editable list. Same source of columns as [`Self::enter_sort_hint_mode`],
    /// so a view without sortable columns says so instead of showing an
    /// empty popup.
    fn open_sort_menu(&mut self) {
        use crate::components::sort_menu::SortMenu;
        use crate::views::SortableView;

        let Tab::Content(idx) = self.active_tab;
        let Some((columns, current)) = self.content_view(idx).map(|cv| {
            (
                SortableView::columns(cv),
                SortableView::current_sort(cv).to_vec(),
            )
        }) else {
            return;
        };
        if columns.is_empty() {
            self.notify("No sortable columns".to_string());
            return;
        }
        self.sort_menu_popup = Some(SortMenu::new(
            Arc::clone(&self.shared_theme),
            &columns,
            &current,
            &self.keybindings,
        ));
    }

    /// Apply a full sort spec produced by the sort menu. Unlike
    /// [`Self::apply_sort`] this replaces the spec wholesale — the menu
    /// already shows every column, so what it hands over *is* the sort.
    fn apply_sort_spec(&mut self, sort: Vec<not_yet_done_content::SortKey>) {
        use crate::views::SortableView;
        use not_yet_done_content::SortDirection;

        let Tab::Content(idx) = self.active_tab;
        let labels: Vec<(String, String)> = self
            .content_view(idx)
            .map(|cv| {
                SortableView::columns(cv)
                    .into_iter()
                    .map(|c| {
                        let label = c.display_label().to_string();
                        (c.key, label)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let descr = if sort.is_empty() {
            "Sort cleared".to_string()
        } else {
            let parts: Vec<String> = sort
                .iter()
                .map(|k| {
                    let label = labels
                        .iter()
                        .find(|(key, _)| *key == k.column)
                        .map(|(_, l)| l.as_str())
                        .unwrap_or(k.column.as_str());
                    let dir = match k.direction {
                        SortDirection::Asc => "asc",
                        SortDirection::Desc => "desc",
                    };
                    format!("{} ({})", label, dir)
                })
                .collect();
            format!("Sort by {}", parts.join(", "))
        };
        // Silent when Enter changed nothing — closing the menu unchanged is
        // not an event worth a notification.
        if self.commit_sort(SortTarget::Content(idx), sort) {
            self.notify(descr);
        }
    }

    // ── Saved queries / favorites ──────────────────────────────────

    /// Write or delete the `default_query:{scope}` settings row. `None`
    /// clears the default.
    ///
    /// The value carries the owning store's kind (`saved:Name`), so that
    /// applying the default on start goes to one store instead of probing
    /// both; [`DefaultQuery::from_setting`] still reads the bare names
    /// written before kinds existed.
    fn persist_default_query(&self, scope: &str, default: Option<&DefaultQuery>) {
        let repo = Arc::clone(&self.settings_repo);
        let key = format!("default_query:{scope}");
        let value = default.map(|d| d.to_setting());
        let _ = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                match value {
                    Some(v) => repo.set(&key, &v).await,
                    None => repo.delete(&key).await,
                }
            })
        });
    }

    /// Toggle the default saved query for a content view, keyed on the
    /// view's `query_scope`. Selecting the current default clears it; the
    /// default is applied automatically on app start (it beats the
    /// last-active filter restore).
    fn set_default_content_query(&mut self, view_index: usize, name: &str) {
        let Some(cv) = self.content_view(view_index) else {
            return;
        };
        let scope = cv.query_scope.clone();
        let current = cv.default_saved_query.clone();
        let new = if current.as_deref() == Some(name) {
            None
        } else {
            Some(name.to_string())
        };
        // The kind is recorded so a startup resolution can go straight to
        // the owning store; the merged menu list stays the authority when
        // both are listed anyway.
        let kind = cv.query_kind_of(name);
        let encoded = new.as_deref().map(|n| DefaultQuery {
            kind,
            name: n.to_string(),
        });
        self.persist_default_query(&scope, encoded.as_ref());
        if let Some(cv) = self.content_view_mut(view_index) {
            cv.default_saved_query = new.clone();
        }
        match new {
            Some(n) => self.notify(format!("Default query: {n}")),
            None => self.notify("Default query cleared".to_string()),
        }
    }

    /// Reload saved queries for a single content view and merge with the
    /// YAML defaults.
    ///
    /// Bodies come from *both* adapter-managed stores — adapter-native
    /// saved queries and extended query documents — merged into one list,
    /// because names are unique across the two and the user is not meant to
    /// tell them apart; each entry remembers its `kind` so the loader knows
    /// which way to execute it. Shortcuts come from the `query_shortcut`
    /// table scoped to this view. An adapter without a store (Postgres, plus
    /// any adapter that opts out) yields an empty list — view-YAML `default:`
    /// is the only fallback then.
    fn reload_content_saved_queries(&mut self, view_index: usize) {
        let cv = match self.content_view(view_index) {
            Some(cv) => cv,
            None => return,
        };
        let scope = cv.query_scope.clone();
        let adapter = cv.adapter.clone();
        let shortcut_repo = Arc::clone(&self.query_shortcut_repo);
        let settings_repo = Arc::clone(&self.settings_repo);

        type ReloadResult = (
            Vec<crate::views::content_view::MergedSavedQuery>,
            Option<String>,
            Option<String>,
        );
        let (entries, default_query, load_error): ReloadResult =
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    // Only the name is kept: the setting's `kind` is a hint
                    // written at save time, while the merged list below is the
                    // authority on which store a name actually lives in today
                    // (see `ContentView::query_kind_of`).
                    let default_query = settings_repo
                        .get(&format!("default_query:{scope}"))
                        .await
                        .ok()
                        .flatten()
                        .map(|v| DefaultQuery::from_setting(&v).name);
                    let Some(adapter) = adapter.as_ref() else {
                        return (Vec::new(), default_query, None);
                    };
                    let Some(store) = adapter.saved_query_store() else {
                        return (Vec::new(), default_query, None);
                    };
                    let names = match store.list().await {
                        Ok(n) => n,
                        Err(e) => {
                            return (
                                Vec::new(),
                                default_query,
                                Some(format!("could not list saved queries: {e}")),
                            );
                        }
                    };
                    // A single malformed row (e.g. a text-encoded `id`)
                    // fails the whole scope's decode; surface it instead of
                    // silently dropping every shortcut in this scope.
                    let (shortcut_map, mut load_error): (
                        std::collections::HashMap<String, String>,
                        Option<String>,
                    ) = match shortcut_repo.list_by_scope(&scope).await {
                        Ok(rows) => (
                            rows.into_iter().map(|m| (m.name, m.shortcut)).collect(),
                            None,
                        ),
                        Err(e) => (
                            std::collections::HashMap::new(),
                            Some(format!("could not load query shortcuts: {e}")),
                        ),
                    };
                    let mut out = Vec::with_capacity(names.len());
                    for name in names {
                        let body = match store.load(&name).await {
                            Ok(b) => b,
                            Err(_) => continue,
                        };
                        let shortcut = shortcut_map.get(&name).cloned();
                        out.push(crate::views::content_view::MergedSavedQuery {
                            name,
                            query: body,
                            shortcut,
                            kind: QueryKind::Saved,
                        });
                    }
                    // The extended store rides alongside, not instead: an
                    // adapter has one only if it has a saved-query store, and
                    // a listing failure there must not lose the saved queries
                    // already collected.
                    if let Some(extended) = adapter.extended_query_store() {
                        match extended.list().await {
                            Ok(names) => {
                                for name in names {
                                    let Ok(body) = extended.load(&name).await else {
                                        continue;
                                    };
                                    let shortcut = shortcut_map.get(&name).cloned();
                                    out.push(crate::views::content_view::MergedSavedQuery {
                                        name,
                                        query: body,
                                        shortcut,
                                        kind: QueryKind::Extended,
                                    });
                                }
                            }
                            Err(e) => {
                                load_error
                                    .get_or_insert(format!("could not list extended queries: {e}"));
                            }
                        }
                    }
                    (out, default_query, load_error)
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
                for entry in &entries {
                    let (name, Some(sc)) = (&entry.name, &entry.shortcut) else {
                        continue;
                    };
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
        // A DB/store load error would otherwise vanish (empty list ==
        // "no shortcuts"), which is exactly what hid the text-uuid bug.
        // Surface it once per distinct message.
        if let Some(err) = load_error {
            let msg = format!("{}: {}", scope, err);
            if self.warned_saved_query_conflicts.insert(msg.clone()) {
                self.notify(msg);
            }
        }
    }

    /// Populate `ContentView::node_script_shortcuts` for the
    /// currently-focused Postgres table (SQ-8d). Cache miss → one
    /// indexed `query_shortcut` lookup keyed on the table's NodeRef
    /// scope. Cache hits short-circuit. Called once per content-tab
    /// keypress; insulated from non-Postgres adapters by the cheap
    /// `adapter_type()`/`target_node_script_node_id()` checks.
    pub fn ensure_node_script_shortcuts_loaded(&mut self, view_index: usize) {
        let Some(cv) = self.content_view(view_index) else {
            return;
        };
        let Some(adapter) = cv.adapter.as_ref() else {
            return;
        };
        if !adapter.capabilities().supports_node_query_editor {
            return;
        }
        let Some(node_id) = cv.target_node_script_node_id() else {
            return;
        };
        if cv.node_script_shortcuts.contains_key(&node_id) {
            return;
        }
        let scope = crate::app::node_actions::node_script_scope(
            adapter.adapter_type(),
            adapter.instance_id(),
            &node_id,
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
            cv.node_script_shortcuts.insert(node_id, entries);
        }
    }

    /// Populate `ContentView::script_shortcuts` for the focused level's
    /// script scope (`script:<tab>/<view…>`). Cache miss → one indexed
    /// `query_shortcut` lookup. No-op when the level offers no `type:
    /// script` action (so no scope) or the cache is already populated.
    /// Called once per content-tab keypress, symmetric with
    /// [`Self::ensure_node_script_shortcuts_loaded`].
    pub fn ensure_script_shortcuts_loaded(&mut self, view_index: usize) {
        let Some(cv) = self.content_view(view_index) else {
            return;
        };
        let Some(scope) = cv.focused_script_scope() else {
            return;
        };
        if cv.script_shortcuts.contains_key(&scope) {
            return;
        }
        let repo = Arc::clone(&self.query_shortcut_repo);
        let scope_owned = scope.clone();
        let entries: Vec<(String, String)> = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                repo.list_by_scope(&scope_owned)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|m| (m.name, m.shortcut))
                    .collect()
            })
        });
        if let Some(cv) = self.content_view_mut(view_index) {
            cv.script_shortcuts.insert(scope, entries);
        }
    }

    /// Persist a captured key chord for a `:script`-menu script into the
    /// `query_shortcut` table, then drop the cached scope entry so the
    /// next keypress refetches and the new claim goes live.
    pub fn bind_script_shortcut(&mut self, coords: ScriptShortcutCoords, chord: &str) {
        let shortcut_repo = Arc::clone(&self.query_shortcut_repo);
        let scope = coords.scope.clone();
        let name = coords.name.clone();
        let chord_owned = chord.to_string();
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                // Script rows share the query_shortcut table but are resolved
                // through their scope, so the kind column stays at its default.
                .block_on(async {
                    shortcut_repo
                        .set(&scope, &name, QueryKind::Saved.as_str(), &chord_owned)
                        .await
                })
        });
        if let Err(e) = result {
            self.notify_error(format!("Failed to persist shortcut: {e}"));
        }
        if let Some(cv) = self.content_view_mut(coords.view_index) {
            cv.script_shortcuts.remove(&coords.scope);
        }
    }

    /// Conflict description for binding `shortcut` to the saved query
    /// `name` in `scope`, or `None` when the key is free. Content-view
    /// scopes route through the keymap-based check so a saved-query
    /// shortcut can never shadow any key active in its tab (the
    /// `j`-shadows-list-navigation class of bug).
    fn favorite_shortcut_conflict(
        &self,
        scope: &str,
        name: &str,
        shortcut: &str,
    ) -> Option<String> {
        self.content_views_indexed()
            .find(|(_, cv)| cv.query_scope == scope)
            .and_then(|(_, cv)| cv.saved_query_shortcut_conflict(&self.keybindings, name, shortcut))
    }

    fn is_shortcut_taken(&self, shortcut: &str) -> bool {
        self.keybindings
            .global
            .bindings
            .values()
            .any(|b| b.matches(shortcut))
    }

    /// Bind `shortcut` to the saved query `name` in `scope`: write the
    /// body to the adapter store and the key chord to the DB, then reload.
    /// Returns `Err` with a user-facing message when the scope matches no
    /// content view or the DB write fails — the caller must not report
    /// success blindly (a swallowed `set` error here is exactly what made
    /// a failed bind look like it worked).
    fn add_favorite(&mut self, favorite: PendingFavorite, shortcut: String) -> Result<(), String> {
        let PendingFavorite {
            scope,
            name,
            query,
            kind,
        } = favorite;
        // Content view scope — body in adapter store, shortcut in DB.
        let target_idx = self
            .content_views_indexed()
            .find(|(_, cv)| cv.query_scope == scope)
            .map(|(idx, _)| idx);
        let Some(idx) = target_idx else {
            return Err(format!("no content view matches scope '{scope}'"));
        };
        self.save_content_query_body(idx, &name, &query, kind);
        let shortcut_repo = Arc::clone(&self.query_shortcut_repo);
        let scope_owned = scope.to_string();
        let name_owned = name.clone();
        let shortcut_owned = shortcut.clone();
        let set_result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                // The row records the kind so a shortcut press can go
                // straight to the owning store.
                shortcut_repo
                    .set(&scope_owned, &name_owned, kind.as_str(), &shortcut_owned)
                    .await
            })
        });
        if let Err(e) = set_result {
            return Err(format!("could not save shortcut: {e}"));
        }
        self.reload_content_saved_queries(idx);
        Ok(())
    }

    /// Which store owns the query named `name` in this view, according to
    /// the merged menu list. Unknown names read as `Saved` — that is a body
    /// typed into the editor, which is always adapter-native.
    fn content_query_kind(&self, view_index: usize, name: &str) -> QueryKind {
        self.content_view(view_index)
            .map(|cv| cv.query_kind_of(name))
            .unwrap_or_default()
    }

    /// Write `body` to the store `kind` names on the active adapter of the
    /// view at `view_index`. No-op if the adapter doesn't expose that store
    /// (e.g. Postgres, which has neither).
    fn save_content_query_body(&self, view_index: usize, name: &str, body: &str, kind: QueryKind) {
        let Some(cv) = self.content_view(view_index) else {
            return;
        };
        let adapter = cv.adapter.clone();
        let name_owned = name.to_string();
        let body_owned = body.to_string();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let Some(adapter) = adapter.as_ref() else {
                    return;
                };
                let _ = match kind {
                    QueryKind::Saved => match adapter.saved_query_store() {
                        Some(store) => store.save(&name_owned, &body_owned).await,
                        None => return,
                    },
                    QueryKind::Extended => match adapter.extended_query_store() {
                        Some(store) => store.save(&name_owned, &body_owned).await,
                        None => return,
                    },
                };
            })
        });
    }

    /// Delete a query body plus its shortcut row. `kind` picks the store:
    /// deleting from the wrong one would leave the entry in the menu while
    /// reporting success.
    fn delete_content_query(&self, view_index: usize, scope: &str, name: &str, kind: QueryKind) {
        let Some(cv) = self.content_view(view_index) else {
            return;
        };
        let adapter = cv.adapter.clone();
        let shortcut_repo = Arc::clone(&self.query_shortcut_repo);
        let scope_owned = scope.to_string();
        let name_owned = name.to_string();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                if let Some(adapter) = adapter.as_ref() {
                    match kind {
                        QueryKind::Saved => {
                            if let Some(store) = adapter.saved_query_store() {
                                let _ = store.delete(&name_owned).await;
                            }
                        }
                        QueryKind::Extended => {
                            if let Some(store) = adapter.extended_query_store() {
                                let _ = store.delete(&name_owned).await;
                            }
                        }
                    }
                }
                let _ = shortcut_repo.unset(&scope_owned, &name_owned).await;
            })
        });
    }

    /// Remove the keyboard shortcut bound to a saved content query,
    /// leaving the query body itself in place.
    fn clear_content_query_shortcut(&self, _view_index: usize, scope: &str, name: &str) {
        let shortcut_repo = Arc::clone(&self.query_shortcut_repo);
        let scope_owned = scope.to_string();
        let name_owned = name.to_string();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let _ = shortcut_repo.unset(&scope_owned, &name_owned).await;
            })
        });
    }

    fn rename_content_query(&self, view_index: usize, scope: &str, old_name: &str, new_name: &str) {
        let Some(cv) = self.content_view(view_index) else {
            return;
        };
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
                let _ = shortcut_repo
                    .rename(&scope_owned, &old_owned, &new_owned)
                    .await;
            })
        });
    }

    // ── DB-stored shortcut editing (query_shortcut table) ─────────────

    /// Resolve a DB-backed shortcut source (saved query / `:script`-menu /
    /// Postgres-table script) to the `query_shortcut` row it edits and the
    /// content view whose chord cache must be invalidated, or `None` for any
    /// other (YAML-backed or built-in) source. Clones everything so no borrow
    /// on `self` leaks out — the caller then mutates through the repository.
    fn resolve_db_shortcut(&self, source: &crate::keymap::KeySource) -> Option<DbShortcutTarget> {
        use crate::keymap::KeySource;
        match source {
            KeySource::SavedQueryShortcut { view, name } => {
                let (idx, cv) = self
                    .content_views_indexed()
                    .find(|(_, cv)| cv.active_view_name() == *view)?;
                Some(DbShortcutTarget {
                    scope: cv.query_scope.clone(),
                    name: name.clone(),
                    view_index: idx,
                    invalidate: DbShortcutInvalidate::SavedQuery,
                })
            }
            KeySource::ScriptShortcut { scope, name } => {
                let (idx, _) = self
                    .content_views_indexed()
                    .find(|(_, cv)| cv.focused_script_scope().as_deref() == Some(scope.as_str()))?;
                Some(DbShortcutTarget {
                    scope: scope.clone(),
                    name: name.clone(),
                    view_index: idx,
                    invalidate: DbShortcutInvalidate::ScriptScope(scope.clone()),
                })
            }
            KeySource::NodeScriptShortcut { node_id, script } => {
                let (idx, cv) = self
                    .content_views_indexed()
                    .find(|(_, cv)| cv.node_script_shortcuts.contains_key(node_id))?;
                let adapter = cv.adapter.as_ref()?;
                // Same string `node_actions::node_script_scope` builds, so
                // the row this resolves to is the one the bind path wrote.
                let scope = crate::app::node_actions::node_script_scope(
                    adapter.adapter_type(),
                    adapter.instance_id(),
                    node_id,
                );
                Some(DbShortcutTarget {
                    scope,
                    name: script.clone(),
                    view_index: idx,
                    invalidate: DbShortcutInvalidate::NodeScript(node_id.clone()),
                })
            }
            _ => None,
        }
    }

    /// Drop the cached chord-claim behind a DB shortcut so the next keypress
    /// rebuilds the keymap from the changed `query_shortcut` state.
    fn invalidate_db_shortcut(&mut self, target: &DbShortcutTarget) {
        match &target.invalidate {
            DbShortcutInvalidate::SavedQuery => {
                self.reload_content_saved_queries(target.view_index)
            }
            DbShortcutInvalidate::ScriptScope(scope) => {
                if let Some(cv) = self.content_view_mut(target.view_index) {
                    cv.script_shortcuts.remove(scope);
                }
            }
            DbShortcutInvalidate::NodeScript(node) => {
                if let Some(cv) = self.content_view_mut(target.view_index) {
                    cv.node_script_shortcuts.remove(node);
                }
            }
        }
    }

    /// Clear the chord of a DB-stored shortcut (unset its `query_shortcut`
    /// row), leaving the query/script body in place. Returns `Ok(true)` when
    /// `source` was DB-backed and the row was unset, `Ok(false)` when it is a
    /// YAML/built-in source the caller must handle itself, or `Err` on DB
    /// failure. Mirrors `clear_content_query_shortcut` / `clear_postgres_…`.
    fn free_db_shortcut(&mut self, source: &crate::keymap::KeySource) -> Result<bool, String> {
        let Some(target) = self.resolve_db_shortcut(source) else {
            return Ok(false);
        };
        let repo = Arc::clone(&self.query_shortcut_repo);
        let (scope, name) = (target.scope.clone(), target.name.clone());
        let res = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async { repo.unset(&scope, &name).await })
        });
        res.map_err(|e| format!("could not clear shortcut: {e}"))?;
        self.invalidate_db_shortcut(&target);
        Ok(true)
    }

    /// Bind (or replace — a DB shortcut has a single chord, no alternatives
    /// list) the chord of a DB-stored shortcut. Returns `Ok(true)` when
    /// `source` was DB-backed and the row was set, `Ok(false)` when the caller
    /// must edit YAML instead, or `Err` on DB failure.
    fn set_db_shortcut(
        &mut self,
        source: &crate::keymap::KeySource,
        chord: &str,
    ) -> Result<bool, String> {
        let Some(target) = self.resolve_db_shortcut(source) else {
            return Ok(false);
        };
        let repo = Arc::clone(&self.query_shortcut_repo);
        let (scope, name, chord_owned) =
            (target.scope.clone(), target.name.clone(), chord.to_string());
        let res = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                // Script row — see `bind_script_shortcut` on the kind.
                .block_on(async {
                    repo.set(&scope, &name, QueryKind::Saved.as_str(), &chord_owned)
                        .await
                })
        });
        res.map_err(|e| format!("could not save shortcut: {e}"))?;
        self.invalidate_db_shortcut(&target);
        Ok(true)
    }

    // ── Command line (:) ──────────────────────────────────────────────

    fn execute_cmdline(&mut self, cmd: &str) {
        let args: Vec<&str> = cmd.trim().split_whitespace().collect();
        if args.is_empty() {
            return;
        }

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
                self.modal_message = Some(":dismiss-notifications takes no arguments".to_string());
                return;
            }
            self.dismiss_notifications();
            return;
        }

        // `:reload` / `:refresh` — re-fetch the active content pane at its
        // current level. Primarily for `commands`-mode scripts that mutate
        // the underlying data (e.g. a period equalizer): the script returns
        // `:reload` once its synchronous writes are done, so the refresh
        // can't race ahead of them.
        if args[0] == "reload" || args[0] == "refresh" {
            if args.len() > 1 {
                self.modal_message = Some(format!(":{} takes no arguments", args[0]));
                return;
            }
            let Tab::Content(view_index) = self.active_tab;
            if let Some(pane_id) = self.content_view(view_index).map(|cv| cv.active_pane_id()) {
                self.reload_content_pane_current_level(view_index, pane_id);
            }
            return;
        }

        if args[0] == "jump" {
            if args.len() != 2 {
                self.modal_message =
                    Some(":jump expects one argument, e.g. :jump Trackings:tree".to_string());
                return;
            }
            self.jump_command(args[1]);
            return;
        }

        if args[0] == "focus-node" {
            // Everything after the command name is target + path; path may
            // contain `|` and other shell-active chars, so we hand the whole
            // rest off to `focus_node_command` unsplit.
            let rest = cmd
                .trim()
                .splitn(2, char::is_whitespace)
                .nth(1)
                .unwrap_or("");
            if rest.trim().is_empty() {
                self.modal_message =
                    Some(":focus-node expects <Tab>[:<view>] /col|pattern".to_string());
                return;
            }
            self.focus_node_command(rest.trim());
            return;
        }

        if args[0] == "tree-find" {
            // Everything after the command name is target + query; the
            // tab name may be quoted and the query may contain `:` etc,
            // so hand the whole rest off to `tree_find_command` unsplit.
            let rest = cmd
                .trim()
                .splitn(2, char::is_whitespace)
                .nth(1)
                .unwrap_or("");
            if rest.trim().is_empty() {
                self.modal_message = Some(":tree-find expects <Tab>[:<view>] <query>".to_string());
                return;
            }
            self.tree_find_command(rest.trim());
            return;
        }

        // DSF-5: `:db-script <sub>` namespace (mirrors `:query <sub>`).
        // Subcommands target the focused content pane's selected row;
        // see `db_script_command` for the per-subcommand contract.
        if args[0] == "db-script" {
            let sub = args.get(1).copied().unwrap_or("");
            let rest = cmd
                .trim()
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
            let rest = cmd
                .trim()
                .splitn(3, char::is_whitespace)
                .nth(2)
                .unwrap_or("")
                .trim();
            match sub {
                "apply" => {
                    if rest.is_empty() {
                        self.modal_message =
                            Some(":query apply expects [-t <Tab>[:<view>]] <name>".to_string());
                        return;
                    }
                    self.query_apply_command(rest);
                }
                "edit" => {
                    if rest.is_empty() {
                        self.modal_message = Some(":query edit expects <name>".to_string());
                        return;
                    }
                    self.query_edit_command(rest);
                }
                "new" => {
                    if rest.is_empty() {
                        self.modal_message = Some(":query new expects <name>".to_string());
                        return;
                    }
                    self.query_new_command(rest);
                }
                "delete" => {
                    if rest.is_empty() {
                        self.modal_message = Some(":query delete expects <name>".to_string());
                        return;
                    }
                    self.query_delete_command(rest);
                }
                "" => {
                    self.modal_message = Some(
                        ":query expects a subcommand (apply | edit | new | delete)".to_string(),
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
                self.modal_message = Some(format!(":{} takes no arguments", args[0]));
                return;
            }
            let Tab::Content(view_index) = self.active_tab;
            self.spawn_invalidate_auth(view_index, kind);
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
                    if stdout.is_empty() {
                        format!(":{cmd} — done")
                    } else {
                        stdout
                    }
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

    fn execute_confirmation(&mut self, confirmation: PendingConfirmation) {
        match confirmation {
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
            PendingConfirmation::DeleteContentNode {
                view_index,
                pane_id,
                node_id,
                action_name,
            } => {
                self.delete_content_node_now(view_index, pane_id, node_id, action_name);
            }
            PendingConfirmation::InvokeNodeAction {
                view_index,
                pane_id,
                node_id,
                action_name,
            } => {
                // Re-invoke the same action on the same node, now with
                // `confirmed: true`, so the adapter does the work instead of
                // returning another `Confirm`.
                self.spawn_invoke_node_action(
                    view_index,
                    pane_id,
                    node_id,
                    action_name,
                    true,
                    None,
                );
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

    /// Refresh `link_refs` from the link table. Cheap — a single `list_all`
    /// scan. Called on startup and after every mutation that adds or
    /// removes a link row. Also syncs the snapshot held by views that
    /// render the `links` column.
    pub fn reload_link_refs(&mut self) {
        let repo = Arc::clone(&self.link_repo);
        let rows = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async move { repo.list_all().await.unwrap_or_default() })
        });
        let mut set = HashSet::with_capacity(rows.len() * 2);
        for row in rows {
            set.insert(row.source_ref);
            set.insert(row.target_ref);
        }
        self.link_refs = set;
        // Push the fresh snapshot down to views that render the column
        // without going through App on every rebuild.
        for slot in self.content_views.iter_mut() {
            if let Some(cv) = slot.as_view_mut() {
                cv.set_link_refs(&self.link_refs);
            }
        }
    }

    /// Returns `true` if an editor is currently live — an external process,
    /// the builtin pane — or if a previous commit is still being processed
    /// in the background. All three states reject a new editor open.
    pub fn editor_busy(&self) -> bool {
        self.detached_editor.is_some()
            || self.builtin_editor.is_some()
            || self.commit_in_flight
            || self.editor_loading
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

    /// True while any content adapter is in a `Busy` state whose banner is
    /// actually shown — the only banner whose text advances purely with
    /// wall-clock time. A tab with `load_banner: off` has nothing to repaint,
    /// so it must not keep the loop awake for a second counter nobody sees.
    fn has_live_banner(&self) -> bool {
        self.content_views_indexed()
            .any(|(_, cv)| cv.is_busy() && cv.load_banner_route() != LoadBannerRoute::Off)
    }

    /// Resolve where this view's load banner goes: its own `tab.load_banner`
    /// if the view file set one, else the global `notifications.load_banner`.
    /// Re-run on every wiring, so an edited config takes effect on reload.
    fn apply_load_banner_route(&mut self, view_index: usize) {
        let global = self.config.notifications.load_banner;
        if let Some(cv) = self.content_view_mut(view_index) {
            cv.set_load_banner_default(global);
        }
    }

    /// Keep the global load slot in step with the tabs that route their load
    /// banner there (`load_banner: global`). Called once per frame: the slot
    /// is derived state, and re-deriving it beats subscribing to every status
    /// transition that could invalidate it — a missed transition would leave
    /// a counter ticking forever on a tab that finished long ago.
    ///
    /// All globally routed tabs share **one** slot: a single tab names itself
    /// (`"Jira — Loading… 40 % (3s)"`, since the global bar cannot say which
    /// tab is meant), several collapse into one counter. The bar therefore
    /// never spends more than one line on loads, whatever is going on.
    pub fn sync_load_banners(&mut self) {
        let live: Vec<(String, LoadBanner)> = self
            .content_views_indexed()
            .filter_map(|(_, cv)| cv.global_load_banner().map(|b| (cv.tab_name.clone(), b)))
            .collect();
        let text = match live.len() {
            0 => None,
            1 => Some(format!("{} — {}", live[0].0, live[0].1.text)),
            n => {
                let oldest = live
                    .iter()
                    .map(|(_, b)| b.started_at_unix_ms)
                    .min()
                    .unwrap_or_default();
                Some(collapsed_load_banner(n, oldest))
            }
        };
        // Same fallback as a prominent `notify` action: a user who switched the
        // loud top strip off gets the counter in the bottom bar instead of
        // losing it. Clearing the other bar covers a config reload that flips
        // `alert_enabled` while a load is in flight.
        let (target, other) = if self.config.notifications.alert_enabled {
            (&mut self.alert_bar, &mut self.notification_bar)
        } else {
            (&mut self.notification_bar, &mut self.alert_bar)
        };
        other.clear_keyed(LOAD_BANNER_SLOT);
        match text {
            Some(t) => target.set_keyed(
                LOAD_BANNER_SLOT,
                crate::components::notification_bar::NoticeClass::Load,
                t,
            ),
            None => target.clear_keyed(LOAD_BANNER_SLOT),
        }
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
        let Tab::Content(idx) = self.active_tab;
        if let Some(cv) = self.content_view_mut(idx) {
            return cv.refit_tables_if_needed();
        }
        false
    }

    pub fn needs_periodic_tick(&self) -> bool {
        self.has_live_banner() || self.detached_editor.is_some() || self.detached_script.is_some()
    }

    // -----------------------------------------------------------------------
    // Detached script polling
    // -----------------------------------------------------------------------

    /// Poll the marker file written by the most recently launched
    /// detached script. When found, surface captured output (if any)
    /// in a [`ScriptOutputSession`].
    /// Returns `true` when a detached script finished and its output was
    /// processed this tick; `false` while none is pending or still running.
    pub fn poll_detached_script(&mut self) -> bool {
        let Some(ref script) = self.detached_script else {
            return false;
        };
        if !script.is_done() {
            return false;
        }

        let output_path = script.output_path.clone();
        let output = script.read_output();
        let capture = script.capture;
        let emits_commands = script.emits_commands;
        let output_suffix = script.output_suffix.clone();
        self.detached_script = None;

        if emits_commands {
            // Re-use the same JSON-commands handler the background path uses;
            // it re-reads the file itself. Must run *before* we delete the
            // file below — deleting first would make it silently find no
            // commands.
            self.run_script_output_commands(&output_path);
        } else if capture {
            if let Some(content) = output.filter(|s| !s.trim().is_empty()) {
                let session = crate::edit_session::ScriptOutputSession::new(content)
                    .with_suffix(output_suffix);
                let _ = self.open_session(Box::new(session));
            } else {
                self.notify("Script finished (no output)".to_string());
            }
        }

        // Remove the marker/output file only now — the commands handler
        // above re-reads it from disk, so deleting earlier would drop them.
        let _ = std::fs::remove_file(&output_path);

        true
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read text from the system clipboard. Returns `None` when the
/// `clipboard` feature is off or no text is available.
#[cfg(feature = "clipboard")]
fn clipboard_text() -> Option<String> {
    arboard::Clipboard::new()
        .ok()
        .and_then(|mut c| c.get_text().ok())
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

/// Load content views from YAML files in `~/.config/not_yet_done/views/`.
/// Each file becomes one [`ContentSlot`]: `Working` if the YAML loaded,
/// validated, and an adapter (or fallback) bound; `Broken` if the YAML
/// is invalid (parse/validate failure). Tab indices stay stable so the
/// user sees a labeled tab for the broken file with an in-app error
/// panel instead of the process exiting.
/// Build the [`TabLayout`] for the current config + loaded content
/// views. Returns the layout plus an optional hard-error message (a
/// duplicate tab name) for the caller to surface as a startup modal; on
/// that error the layout falls back to all tabs so the app still runs.
/// Soft issues (unknown tab names in `tabs.order`) are logged, not
/// returned.
fn build_tab_layout(
    tabs_cfg: &crate::config::TabsConfig,
    content_views: &[ContentSlot],
) -> (TabLayout, Option<String>) {
    // All tabs are content tabs, in slot order.
    let mut available: Vec<(String, Tab)> = Vec::with_capacity(content_views.len());
    for (idx, slot) in content_views.iter().enumerate() {
        available.push((slot.tab_name().to_string(), Tab::Content(idx)));
    }

    match crate::tabs::resolve_tab_layout(tabs_cfg, &available, content_views.len(), |w| {
        not_yet_done_content::http_log::log_error("tab_layout", &w)
    }) {
        Ok(layout) => (layout, None),
        Err(hard) => {
            not_yet_done_content::http_log::log_error("tab_layout", &hard);
            (TabLayout::all_tabs(content_views.len()), Some(hard))
        }
    }
}

fn load_content_views(
    theme: &Arc<Theme>,
    keybindings: &crate::config::keybindings::KeyBindingConfig,
    editors: &crate::config::editor::EditorsConfig,
    factories: std::collections::HashMap<String, Box<dyn not_yet_done_content::AdapterFactory>>,
    host_ctx: &not_yet_done_content::HostContext,
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
        .filter(|p| {
            p.extension()
                .is_some_and(|ext| ext == "yaml" || ext == "yml")
        })
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
            Err(e) => {
                eprintln!("Warning: {}: {e}", path.display());
                continue;
            }
        };
        // Heuristic: a view-config has top-level `tab` AND `adapter` keys.
        // Files without both (e.g. adapter credentials like jira-adapter.yaml)
        // are skipped silently.
        let raw: serde_yaml::Value = match serde_yaml::from_str(&yaml) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let is_view_config = raw.get("tab").is_some() && raw.get("adapter").is_some();
        if !is_view_config {
            continue;
        }

        // YAML-parse failure: take the file's stem as a fallback tab name
        // (the actual `tab.name` is unreadable).
        let mut config: ViewFileConfig = match serde_yaml::from_str(&yaml) {
            Ok(c) => c,
            Err(e) => {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("?")
                    .to_string();
                loaded.push((
                    path.clone(),
                    Loaded::Broken {
                        name,
                        errors: vec![format!("YAML parse error: {e}")],
                    },
                ));
                continue;
            }
        };

        // Fill tree-continuation levels that omit `columns:` from the level
        // above them, before validation reads them (so `tree_label` etc.
        // resolve against the inherited set).
        config.inherit_tree_columns();
        // Propagate inheritable per-row actions/shortcuts (marked `inherit`)
        // down tree-continuation levels, likewise before validation.
        config.inherit_tree_actions();

        match config.validate(keybindings, editors) {
            Ok(()) => loaded.push((path.clone(), Loaded::Ok(config))),
            Err(errors) => loaded.push((
                path.clone(),
                Loaded::Broken {
                    name: config.tab.name.clone(),
                    errors,
                },
            )),
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
                let adapter: Option<Arc<dyn not_yet_done_content::ContentAdapter>> = match factories
                    .get(&config.adapter.adapter_type)
                {
                    None => {
                        init_error = Some(format!(
                            "no adapter factory registered for type '{}'",
                            config.adapter.adapter_type
                        ));
                        None
                    }
                    Some(factory) => {
                        let adapter_config =
                            config.adapter.config_inline.as_ref().cloned().or_else(|| {
                                config.adapter.config.as_ref().and_then(|cfg_path| {
                                    let resolved = if std::path::Path::new(cfg_path).is_absolute() {
                                        std::path::PathBuf::from(cfg_path)
                                    } else {
                                        path_ref
                                            .parent()
                                            .unwrap_or(std::path::Path::new("."))
                                            .join(cfg_path)
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
                            Some(cfg) => {
                                match factory.create(
                                    config.adapter.effective_instance_id(),
                                    &cfg,
                                    host_ctx,
                                ) {
                                    Ok(a) => Some(Arc::from(a)),
                                    Err(e) => {
                                        init_error = Some(e.to_string());
                                        None
                                    }
                                }
                            }
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
    use super::{
        App, credential_form_allowed, credential_form_title, format_notification_log,
        image_temp_filename, notification_bar_hint, parse_query_apply_args,
        render_payload_template, split_leading_token, which_key_filter, which_key_prefix_allowed,
    };

    #[test]
    fn the_active_tab_may_always_ask_for_credentials() {
        assert!(credential_form_allowed(2, 2, true, None));
        assert!(credential_form_allowed(2, 2, false, None));
    }

    #[test]
    fn an_eager_background_tab_may_ask_at_startup() {
        // manual_connect: false — it connects without the user visiting it,
        // so its form has to be visible from wherever the user is.
        assert!(credential_form_allowed(0, 3, false, None));
    }

    #[test]
    fn a_manual_connect_background_tab_stays_quiet() {
        // Its load only starts from a `reload` pressed on that tab, so the
        // form waits until the tab is opened.
        assert!(!credential_form_allowed(0, 3, true, None));
    }

    #[test]
    fn an_open_form_is_never_taken_from_another_view() {
        assert!(!credential_form_allowed(1, 1, false, Some(4)));
        assert!(!credential_form_allowed(0, 3, false, Some(4)));
        // …but the owner may keep refreshing its own form (new error, new
        // round of a multi-step script).
        assert!(credential_form_allowed(0, 4, false, Some(4)));
    }

    #[test]
    fn credential_form_title_leads_with_the_tab() {
        assert_eq!(
            credential_form_title(Some("Password store locked"), Some("Kimai")),
            "Kimai: Password store locked"
        );
        assert_eq!(credential_form_title(None, Some("Kimai")), "Login: Kimai");
        assert_eq!(
            credential_form_title(Some("Password store locked"), None),
            "Password store locked"
        );
        assert_eq!(credential_form_title(None, None), "Login");
    }

    /// Build a log record at a fixed local time so the rendered stamp is
    /// deterministic.
    fn record(
        hour: u32,
        minute: u32,
        message: &str,
    ) -> crate::components::notification_bar::NotificationRecord {
        use chrono::TimeZone;
        crate::components::notification_bar::NotificationRecord {
            at: chrono::Local
                .with_ymd_and_hms(2026, 8, 3, hour, minute, 0)
                .unwrap(),
            message: message.to_string(),
        }
    }

    #[test]
    fn notification_log_merges_both_bars_chronologically() {
        let bottom = [record(9, 5, "second"), record(9, 20, "fourth")];
        let top = [record(9, 0, "first"), record(9, 10, "third")];
        let text = format_notification_log(
            bottom
                .iter()
                .map(|r| (r, false))
                .chain(top.iter().map(|r| (r, true))),
        );
        assert_eq!(
            text,
            "[2026-08-03 09:00:00] ! first\n\
             [2026-08-03 09:05:00] second\n\
             [2026-08-03 09:10:00] ! third\n\
             [2026-08-03 09:20:00] fourth\n"
        );
    }

    #[test]
    fn notification_log_indents_continuation_lines() {
        let rec = [record(9, 0, "headline\ndetail")];
        let text = format_notification_log(rec.iter().map(|r| (r, false)));
        assert_eq!(text, "[2026-08-03 09:00:00] headline\n    detail\n");
    }

    #[test]
    fn notification_log_is_empty_without_records() {
        assert!(format_notification_log(std::iter::empty()).is_empty());
    }

    #[test]
    fn bar_hint_names_the_bound_keys() {
        let gkb = crate::config::keybindings::KeyBindingSection::<
            crate::config::keybindings::GlobalAction,
        >::default();
        assert_eq!(notification_bar_hint(&gkb), "[Z] dismiss  [f10] open");
    }

    #[test]
    fn form_options_default_when_no_config() {
        let fields = vec![not_yet_done_content::FormFieldSpec::text("a", "A")];
        let opts = App::build_form_options(None, &fields);
        assert_eq!(opts, not_yet_done_ratatui::FormOptions::default());
    }

    #[test]
    fn form_options_resolve_column_assignment_to_indices() {
        use crate::config::view_config::{ActionFormConfig, SelectStyleConfig};
        let fields = vec![
            not_yet_done_content::FormFieldSpec::text("name", "Name"),
            not_yet_done_content::FormFieldSpec::text("description", "Desc"),
            not_yet_done_content::FormFieldSpec::text("extra", "Extra"),
        ];
        let cfg = ActionFormConfig {
            columns: Some(2),
            // `extra` is deliberately unlisted → stays in column 0.
            column_assignment: Some(vec![vec!["name".into()], vec!["description".into()]]),
            field_bar: Some(true),
            select_style: Some(SelectStyleConfig::Inline),
        };
        let opts = App::build_form_options(Some(&cfg), &fields);
        assert_eq!(opts.columns, 2);
        assert!(opts.field_bar);
        assert_eq!(opts.select_style, not_yet_done_ratatui::SelectStyle::Inline);
        assert_eq!(opts.column_of, vec![0, 1, 0]);
    }

    #[test]
    fn render_payload_template_substitutes_string_and_number_fields() {
        let payload = serde_json::json!({ "number": "42", "who": "alice" });
        assert_eq!(
            render_payload_template("Authenticator: tap {number} ({who})", &payload),
            "Authenticator: tap 42 (alice)"
        );
        // A JSON number renders without quotes.
        let numeric = serde_json::json!({ "n": 7 });
        assert_eq!(render_payload_template("code {n}", &numeric), "code 7");
    }

    #[test]
    fn render_payload_template_leaves_unknown_placeholders_and_handles_empty() {
        let payload = serde_json::json!({ "a": "x" });
        assert_eq!(
            render_payload_template("{a} then {missing}", &payload),
            "x then {missing}"
        );
        // Non-object payloads (null, array) simply yield the template verbatim.
        assert_eq!(
            render_payload_template("no fields", &serde_json::Value::Null),
            "no fields"
        );
    }

    #[test]
    fn image_temp_filename_prefixes_index_and_keeps_basename() {
        assert_eq!(
            image_temp_filename(3, "https://cdn.test/a/b/photo.png"),
            "03_photo.png"
        );
    }

    #[test]
    fn image_temp_filename_drops_query_and_sanitizes() {
        assert_eq!(
            image_temp_filename(0, "https://cdn.test/pic name.jpg?tok=1"),
            "00_pic_name.jpg"
        );
    }

    #[test]
    fn image_temp_filename_appends_extension_when_missing() {
        assert_eq!(
            image_temp_filename(1, "https://cdn.test/blob"),
            "01_blob.img"
        );
    }

    #[test]
    fn split_leading_token_quoted_tab_name_with_spaces() {
        // The `:tree-find "Analytics DB" id:42` case: a quoted tab name
        // keeps its spaces, the rest is the query.
        let (tok, rest) = split_leading_token(r#""Analytics DB" id:42"#);
        assert_eq!(tok, "Analytics DB");
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

    #[test]
    fn query_apply_strips_quotes_from_target_tab() {
        // Regression: `query apply -t "Tasks" All` must resolve the tab as
        // `Tasks`, not `"Tasks"` — the quotes are shell-style grouping and
        // must be stripped so it matches `tab_name()`. (goto_task.py emits
        // the quoted form because a tab name can contain spaces.)
        let (_vars, target, name) = parse_query_apply_args(r#"-t "Tasks" All"#).expect("parses");
        assert_eq!(target, Some(("Tasks".to_string(), None)));
        assert_eq!(name, "All");
    }

    #[test]
    fn query_apply_unquoted_target_with_view() {
        // Unquoted `tab:view` still splits into tab + view. (A quoted tab
        // name and a `:view` can't be combined — the closing quote ends the
        // token before the colon; same limitation as `:tree-find`.)
        let (_vars, target, name) =
            parse_query_apply_args("-t Postgres:items My Query").expect("parses");
        assert_eq!(
            target,
            Some(("Postgres".to_string(), Some("items".to_string())))
        );
        assert_eq!(name, "My Query");
    }

    #[test]
    fn which_key_empty_allowlist_permits_every_prefix() {
        assert!(which_key_prefix_allowed(&[], "g"));
        assert!(which_key_prefix_allowed(&[], "ctrl+k"));
    }

    #[test]
    fn which_key_allowlist_matches_step_wise() {
        let list = vec!["g".to_string(), "z".to_string()];
        // First step in the allowlist — a deeper pending chord still matches.
        assert!(which_key_prefix_allowed(&list, "g"));
        assert!(which_key_prefix_allowed(&list, "g l"));
        assert!(which_key_prefix_allowed(&list, "z"));
        // Not listed → rejected.
        assert!(!which_key_prefix_allowed(&list, "f"));
    }

    #[test]
    fn which_key_allowlist_supports_multi_step_entries() {
        let list = vec!["g l".to_string()];
        assert!(which_key_prefix_allowed(&list, "g l"));
        // `g` alone is a prefix of the entry, not a continuation of it.
        assert!(!which_key_prefix_allowed(&list, "g"));
        assert!(!which_key_prefix_allowed(&list, "g m"));
    }

    #[test]
    fn which_key_filter_keeps_only_strict_continuations() {
        let sources = vec![
            ("New channel".to_string(), "a l".to_string()),
            ("Archive".to_string(), "a a".to_string()),
            // Exact match, not a continuation → dropped (nothing left to type).
            ("Fuzzy".to_string(), "a".to_string()),
            // Different prefix → dropped.
            ("Delete".to_string(), "d".to_string()),
        ];
        let rows = which_key_filter(sources, "a");
        // Sorted by combo: "a a" before "a l".
        assert_eq!(
            rows,
            vec![
                ("Archive".to_string(), "a a".to_string()),
                ("New channel".to_string(), "a l".to_string()),
            ]
        );
    }

    #[test]
    fn which_key_filter_splits_binding_alternatives() {
        // One shortcut with two alternatives; only the one continuing the
        // prefix survives.
        let sources = vec![("Jump".to_string(), "g g / ctrl+home".to_string())];
        let rows = which_key_filter(sources, "g");
        assert_eq!(rows, vec![("Jump".to_string(), "g g".to_string())]);
    }
}
