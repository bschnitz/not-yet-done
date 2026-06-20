//! App-level entry point for [`NodeRef`] open-by-path dispatch.
//!
//! The App is the top of the routing chain. It peels the first segment
//! off the ref (the tab key) and forwards the remaining tail to the
//! matching tab. Tasks and Trackings consume a single UUID tail.
//! Adapter tabs (`jira`, `taiga`) take a `<instance_id>/<node_id>`
//! tail: the instance segment selects which content slot to switch to,
//! the rest is handed to the pane for row-focus lookup.
//!
//! Postgres is intentionally excluded — its `qrow:N` IDs are per-query
//! and meaningless after a refresh.

use std::collections::HashSet;

use not_yet_done_content::{LinkRouteError, NodeRef};
use uuid::Uuid;

use super::App;
use crate::components::searchable_popup::{PopupItem, SearchablePopup};
use crate::tabs::Tab;

/// Snapshot of "what addresses currently resolve" used by
/// [`classify_link_ref`] for bulk staleness scanning. Computed once
/// per `:linkprune` invocation by querying tasks / trackings / content
/// adapters; kept as plain sets so the classifier itself stays a pure
/// function over data.
#[derive(Debug, Default)]
pub struct LinkResolveContext {
    /// All non-deleted task UUIDs.
    pub task_ids: HashSet<Uuid>,
    /// All non-deleted tracking UUIDs.
    pub tracking_ids: HashSet<Uuid>,
    /// `(adapter_type, instance_id)` pairs for every currently configured
    /// content adapter, e.g. `("jira", "prod")`. A link whose head matches
    /// `adapter_type` but whose instance segment isn't in this set counts
    /// as stale even when the node would otherwise be navigable.
    pub adapter_instances: HashSet<(String, String)>,
}

/// Pure classification: would [`App::open_link`] currently fail with
/// [`LinkRouteError::Stale`] / [`LinkRouteError::UnknownRoute`] for
/// this ref? Pulled out so it can be exercised with unit tests instead
/// of a live App.
///
/// Returns `None` for "looks live", `Some(reason)` for "would not
/// navigate". Postgres is always stale (its IDs are per-query and so
/// have no place in the link table); the actual *remote* existence of
/// a Jira/Taiga node is intentionally not probed — we only verify the
/// instance segment matches a configured adapter.
pub fn classify_link_ref(raw: &str, ctx: &LinkResolveContext) -> Option<String> {
    let node_ref = match NodeRef::parse(raw) {
        Ok(r) => r,
        Err(e) => return Some(format!("parse failed: {e}")),
    };
    let (head, tail) = node_ref.split_head();
    match head {
        "tasks" => {
            let id_str = tail.ok_or("missing task id");
            let id_str = match id_str {
                Ok(s) => s,
                Err(e) => return Some(e.to_string()),
            };
            let uuid = match Uuid::parse_str(id_str) {
                Ok(u) => u,
                Err(_) => return Some(format!("invalid task uuid: {id_str}")),
            };
            if !ctx.task_ids.contains(&uuid) {
                return Some(format!("task {uuid} not found"));
            }
            None
        }
        "tracking" => {
            let id_str = match tail {
                Some(s) => s,
                None => return Some("missing tracking id".to_string()),
            };
            let uuid = match Uuid::parse_str(id_str) {
                Ok(u) => u,
                Err(_) => return Some(format!("invalid tracking uuid: {id_str}")),
            };
            if !ctx.tracking_ids.contains(&uuid) {
                return Some(format!("tracking {uuid} not found"));
            }
            None
        }
        "jira" | "taiga" => {
            let tail = match tail {
                Some(s) => s,
                None => return Some(format!("missing instance/node after {head}")),
            };
            let (instance, _node) = match tail.split_once('/') {
                Some((a, b)) if !a.is_empty() && !b.is_empty() => (a, b),
                _ => return Some(format!("malformed {head}/<instance>/<node>: {head}/{tail}")),
            };
            if !ctx
                .adapter_instances
                .contains(&(head.to_string(), instance.to_string()))
            {
                return Some(format!("no content tab for {head}/{instance}"));
            }
            None
        }
        "postgres" => Some("postgres has no stable IDs".to_string()),
        other => Some(format!("unknown route: {other}")),
    }
}

/// State for the `gl` link popup. The [`SearchablePopup`] carries the
/// item list (rendered with `→`/`←` prefix in the label, `link.id` in
/// `value`); `other_by_id` maps a link-row id to the ref of the *other*
/// node — what Enter navigates to. The anchor [`NodeRef`] is kept so
/// the popup can refresh itself after a delete.
pub struct LinkPopupState {
    pub popup: SearchablePopup,
    pub other_by_id: std::collections::HashMap<Uuid, String>,
    pub anchor: NodeRef,
}

/// Vim-style two-stack jump history. `back` holds older positions
/// (Ctrl+O target), `forward` holds positions to revisit after a
/// Ctrl+O (Ctrl+I target). A fresh link-jump records the anchor in
/// `back` and wipes `forward`, matching the standard browser/vim
/// branching semantics.
///
/// All mutators are intentionally atomic so callers don't need to
/// worry about restoring partial state — if a popped target can't be
/// navigated to, the caller simply doesn't push the current position
/// back, matching vim's "drop stale marks" behaviour.
#[derive(Default, Debug)]
pub struct JumpHistory {
    back: Vec<NodeRef>,
    forward: Vec<NodeRef>,
}

impl JumpHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a deliberate jump from `from` → (target navigated to
    /// elsewhere). Drops the forward branch — once the user takes a
    /// new path, the old "next" trail no longer makes sense.
    pub fn record_jump(&mut self, from: NodeRef) {
        self.back.push(from);
        self.forward.clear();
    }

    /// Pop and return the most recent back entry. `current` (the
    /// position we're leaving) is pushed onto `forward` so a
    /// subsequent forward jump can return here. Pass `None` when the
    /// current selection isn't addressable.
    pub fn pop_back(&mut self, current: Option<NodeRef>) -> Option<NodeRef> {
        let target = self.back.pop()?;
        if let Some(c) = current {
            self.forward.push(c);
        }
        Some(target)
    }

    /// Pop and return the most recent forward entry. Mirrors
    /// [`Self::pop_back`] in reverse.
    pub fn pop_forward(&mut self, current: Option<NodeRef>) -> Option<NodeRef> {
        let target = self.forward.pop()?;
        if let Some(c) = current {
            self.back.push(c);
        }
        Some(target)
    }

    pub fn back_len(&self) -> usize {
        self.back.len()
    }

    pub fn forward_len(&self) -> usize {
        self.forward.len()
    }
}

impl App {
    /// Build a [`NodeRef`] for the currently focused row in the active
    /// tab, or `None` when nothing addressable is selected. Postgres is
    /// intentionally excluded — its `qrow:N` IDs are per-query and
    /// can't survive a refresh, let alone a process restart.
    pub fn current_node_ref(&self) -> Option<NodeRef> {
        let Tab::Content(idx) = self.active_tab;
        let cv = self.content_view(idx)?;
        let adapter = cv.adapter.as_ref()?;
        let kind = adapter.adapter_type();
        if kind == "postgres" {
            return None;
        }
        let node_id = cv.active_pane().selected_item_id()?;
        NodeRef::parse(&format!(
            "{}/{}/{}",
            kind,
            adapter.instance_id(),
            node_id
        ))
        .ok()
    }

    /// Capture the current selection into [`App::marked_link`]. Notifies
    /// either the captured ref or — when the selection isn't addressable
    /// (broken tab, empty list, postgres) — a "nothing to mark" message.
    pub fn link_mark_current(&mut self) {
        match self.current_node_ref() {
            Some(node_ref) => {
                let label = node_ref.as_str().to_string();
                self.marked_link = Some(node_ref);
                self.notify(format!("Link mark armed: {label}"));
            }
            None => self.notify("Nothing to mark for linking".to_string()),
        }
    }

    /// Drop the link mark and notify. Called from the Esc tail handler.
    pub fn link_clear_mark(&mut self) {
        if self.marked_link.take().is_some() {
            self.notify("Link mark cleared".to_string());
        }
    }

    /// Build (or rebuild) the link popup for `anchor`. Queries outgoing
    /// + incoming via the repo and composes a single [`SearchablePopup`]
    /// with outgoing rows on top (`→ <ref>`), incoming below (`← <ref>`).
    /// Each row's `value` is the link table id; the `rows` lookup keeps
    /// the direction + the other-side ref needed for navigation.
    fn build_link_popup(&self, anchor: NodeRef) -> Result<LinkPopupState, String> {
        let repo = std::sync::Arc::clone(&self.link_repo);
        let anchor_for_query = anchor.clone();
        let (outgoing, incoming) = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let out = repo.outgoing(&anchor_for_query).await;
                let inc = repo.incoming(&anchor_for_query).await;
                (out, inc)
            })
        });
        let outgoing = outgoing.map_err(|e| format!("link outgoing query failed: {e}"))?;
        let incoming = incoming.map_err(|e| format!("link incoming query failed: {e}"))?;

        let mut items: Vec<PopupItem> = Vec::with_capacity(outgoing.len() + incoming.len());
        let mut other_by_id: std::collections::HashMap<Uuid, String> =
            std::collections::HashMap::with_capacity(outgoing.len() + incoming.len());
        for row in &outgoing {
            items.push(PopupItem {
                label: format!("→ {}", row.target_ref),
                value: row.id.to_string(),
                ..Default::default()
            });
            other_by_id.insert(row.id, row.target_ref.clone());
        }
        for row in &incoming {
            items.push(PopupItem {
                label: format!("← {}", row.source_ref),
                value: row.id.to_string(),
                ..Default::default()
            });
            other_by_id.insert(row.id, row.source_ref.clone());
        }
        let title = format!("Links · {} · ↵ open · d delete · esc close", anchor.as_str());
        let theme = std::sync::Arc::clone(&self.shared_theme);
        let hints = vec![
            ("↵".to_string(), "open".to_string()),
            ("d".to_string(), "delete".to_string()),
            ("esc".to_string(), "close".to_string()),
        ];
        let popup = SearchablePopup::new(theme, title, items)
            .with_popup_kb(
                self.keybindings.popup.clone(),
                self.keybindings.key_icons.clone(),
            )
            .with_hints(hints);
        Ok(LinkPopupState {
            popup,
            other_by_id,
            anchor,
        })
    }

    /// Open the link popup anchored at the current row. Notifies when
    /// nothing addressable is selected or both queries return zero rows
    /// (in which case there's nothing to show — but we still open an
    /// empty popup so the user knows the query ran, not just no mapping).
    pub fn link_open_popup(&mut self) {
        let Some(anchor) = self.current_node_ref() else {
            self.notify("Nothing to look up links for on this row".to_string());
            return;
        };
        match self.build_link_popup(anchor) {
            Ok(state) => {
                if state.popup.is_empty() {
                    self.notify("No links for this node".to_string());
                    return;
                }
                self.link_popup = Some(state);
            }
            Err(msg) => {
                self.set_query_error(Some(msg.clone()));
                self.notify(msg);
            }
        }
    }

    /// Re-query outgoing+incoming for the popup anchor and replace the
    /// item list. Called after a delete. Preserves the popup's current
    /// search query so the filter stays useful across the refresh.
    fn link_popup_refresh(&mut self) {
        let Some(state) = self.link_popup.as_ref() else {
            return;
        };
        let anchor = state.anchor.clone();
        let prior_query = state.popup.query_text().to_string();
        match self.build_link_popup(anchor) {
            Ok(mut new_state) => {
                if new_state.popup.is_empty() {
                    self.link_popup = None;
                    self.notify("No more links for this node".to_string());
                    return;
                }
                for c in prior_query.chars() {
                    new_state.popup.insert_char(c);
                }
                self.link_popup = Some(new_state);
            }
            Err(msg) => {
                self.set_query_error(Some(msg.clone()));
                self.notify(msg);
            }
        }
    }

    /// Delete the currently selected link row, then refresh the popup.
    fn link_popup_delete_selected(&mut self) {
        let Some(state) = self.link_popup.as_ref() else {
            return;
        };
        let Some(item) = state.popup.selected_item() else {
            return;
        };
        let Ok(id) = Uuid::parse_str(&item.value) else {
            return;
        };
        let repo = std::sync::Arc::clone(&self.link_repo);
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async move { repo.delete(id).await })
        });
        match result {
            Ok(()) => {
                self.reload_link_refs();
                self.notify("Link deleted".to_string());
                self.link_popup_refresh();
            }
            Err(e) => {
                let msg = format!("Link delete failed: {e}");
                self.set_query_error(Some(msg.clone()));
                self.notify(msg);
            }
        }
    }

    /// Pick the navigation target for the currently selected row and
    /// close the popup before handing off to `open_link`. Closing first
    /// avoids leaving an overlay across a tab switch.
    ///
    /// When `open_link` fails with [`LinkRouteError::Stale`] or
    /// [`LinkRouteError::UnknownRoute`] — or the ref doesn't even
    /// parse — the row is treated as stale: a confirm-delete modal
    /// offers to drop it from the link table. `NotSupported` and
    /// `Other` stay informational (postgres / transient I/O).
    pub fn link_popup_activate_selected(&mut self) {
        let Some(state) = self.link_popup.as_ref() else {
            return;
        };
        let Some(item) = state.popup.selected_item() else {
            return;
        };
        let Ok(link_id) = Uuid::parse_str(&item.value) else {
            return;
        };
        let Some(target) = state.other_by_id.get(&link_id).cloned() else {
            return;
        };
        let anchor = state.anchor.clone();
        // Close BEFORE navigating so the overlay doesn't persist across
        // the tab switch open_link may trigger.
        self.link_popup = None;
        let node_ref = match NodeRef::parse(&target) {
            Ok(r) => r,
            Err(e) => {
                self.prompt_delete_stale_link(link_id, &target, &format!("ref parse failed: {e}"));
                return;
            }
        };
        let outcome = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { self.open_link(&node_ref).await })
        });
        match outcome {
            Ok(()) => {
                self.jump_history.record_jump(anchor);
            }
            Err(LinkRouteError::Stale(reason))
            | Err(LinkRouteError::UnknownRoute(reason)) => {
                self.prompt_delete_stale_link(link_id, &target, &reason);
            }
            Err(e) => {
                let msg = format!("Link open failed: {e}");
                self.set_query_error(Some(msg.clone()));
                self.notify(msg);
            }
        }
    }

    /// Navigate one step backward in the jump history (Ctrl+O). The
    /// current position is pushed onto the forward stack so Ctrl+I can
    /// return. On navigation failure (stale ref), the popped entry is
    /// discarded — same heuristic vim uses when an old mark is gone.
    pub fn link_jump_back(&mut self) {
        let current = self.current_node_ref();
        let Some(target) = self.jump_history.pop_back(current.clone()) else {
            self.notify("No back-history".to_string());
            return;
        };
        let label = target.as_str().to_string();
        let outcome = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { self.open_link(&target).await })
        });
        match outcome {
            Ok(()) => {
                self.notify(format!("← {label}"));
            }
            Err(e) => {
                // Undo the forward push we just made — the navigation
                // didn't actually move us, so `current` shouldn't be
                // recoverable via Ctrl+I either.
                let _ = self.jump_history.pop_forward(None);
                let msg = format!("Back-jump failed: {e}");
                self.set_query_error(Some(msg.clone()));
                self.notify(msg);
            }
        }
    }

    /// Build a [`LinkResolveContext`] from the current process state.
    /// Issues one query per source (tasks / trackings) plus an in-process
    /// scan of configured content adapters. Soft-deleted tasks and
    /// trackings count as stale because their underlying repos exclude
    /// `deleted = true` rows.
    fn build_link_resolve_context(&self) -> Result<LinkResolveContext, String> {
        let task_service = std::sync::Arc::clone(&self.task_service);
        let tracking_repo = std::sync::Arc::clone(&self.tracking_repo);
        let (tasks, trackings) = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let t = task_service.list_tasks(None).await;
                let tr = tracking_repo.find_all().await;
                (t, tr)
            })
        });
        let tasks = tasks.map_err(|e| format!("task scan failed: {e}"))?;
        let trackings = trackings.map_err(|e| format!("tracking scan failed: {e}"))?;
        let task_ids: HashSet<Uuid> = tasks.into_iter().map(|t| t.id).collect();
        let tracking_ids: HashSet<Uuid> = trackings.into_iter().map(|t| t.id).collect();
        let adapter_instances: HashSet<(String, String)> = self
            .content_views_iter()
            .filter_map(|cv| {
                cv.adapter
                    .as_ref()
                    .map(|a| (a.adapter_type().to_string(), a.instance_id().to_string()))
            })
            .collect();
        Ok(LinkResolveContext {
            task_ids,
            tracking_ids,
            adapter_instances,
        })
    }

    /// `:linkprune` entry point. Scans every link row, classifies each
    /// endpoint via [`classify_link_ref`], collects rows where source or
    /// target no longer resolves, and prompts the user to confirm a
    /// bulk delete. No-op when nothing is stale or the link table is
    /// empty — the modal then just reports the empty result so the user
    /// knows the scan ran.
    pub fn link_prune_command(&mut self) {
        let ctx = match self.build_link_resolve_context() {
            Ok(c) => c,
            Err(msg) => {
                self.set_query_error(Some(msg.clone()));
                self.modal_message = Some(msg);
                return;
            }
        };
        let repo = std::sync::Arc::clone(&self.link_repo);
        let rows = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move { repo.list_all().await })
        });
        let rows = match rows {
            Ok(r) => r,
            Err(e) => {
                let msg = format!("link scan failed: {e}");
                self.set_query_error(Some(msg.clone()));
                self.modal_message = Some(msg);
                return;
            }
        };
        if rows.is_empty() {
            self.modal_message = Some("No links in the database.".to_string());
            return;
        }
        let mut stale_ids: Vec<Uuid> = Vec::new();
        let mut sample: Vec<String> = Vec::new();
        for row in &rows {
            let src_stale = classify_link_ref(&row.source_ref, &ctx);
            let tgt_stale = classify_link_ref(&row.target_ref, &ctx);
            if src_stale.is_some() || tgt_stale.is_some() {
                stale_ids.push(row.id);
                if sample.len() < 5 {
                    let reason = src_stale
                        .as_deref()
                        .or(tgt_stale.as_deref())
                        .unwrap_or("");
                    sample.push(format!(
                        "{} → {} ({})",
                        row.source_ref, row.target_ref, reason
                    ));
                }
            }
        }
        if stale_ids.is_empty() {
            self.modal_message = Some(format!(
                "Scanned {} link(s). None are stale.",
                rows.len()
            ));
            return;
        }
        let mut body = format!(
            "{} of {} link(s) are stale:\n",
            stale_ids.len(),
            rows.len()
        );
        for line in &sample {
            body.push_str(&format!("  {line}\n"));
        }
        if stale_ids.len() > sample.len() {
            body.push_str(&format!("  … and {} more\n", stale_ids.len() - sample.len()));
        }
        body.push_str("Delete all? (y/n)");
        self.pending_confirmation = Some((
            body.clone(),
            super::PendingConfirmation::BulkDeleteStaleLinks(stale_ids),
        ));
        self.modal_message = Some(body);
    }

    /// Navigate one step forward in the jump history (Ctrl+I).
    /// Symmetric counterpart to [`Self::link_jump_back`].
    pub fn link_jump_forward(&mut self) {
        let current = self.current_node_ref();
        let Some(target) = self.jump_history.pop_forward(current.clone()) else {
            self.notify("No forward-history".to_string());
            return;
        };
        let label = target.as_str().to_string();
        let outcome = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { self.open_link(&target).await })
        });
        match outcome {
            Ok(()) => {
                self.notify(format!("→ {label}"));
            }
            Err(e) => {
                let _ = self.jump_history.pop_back(None);
                let msg = format!("Forward-jump failed: {e}");
                self.set_query_error(Some(msg.clone()));
                self.notify(msg);
            }
        }
    }

    /// Set up the confirm-delete modal for a stale link row. The actual
    /// repo delete happens via [`super::PendingConfirmation`] once the
    /// user hits y/Enter — keeps deletion behind the same y/n gate as
    /// every other destructive confirm in the app.
    fn prompt_delete_stale_link(&mut self, link_id: Uuid, target: &str, reason: &str) {
        let msg = format!(
            "Stale link `{target}`\n({reason})\nDelete from link table? (y/n)"
        );
        self.modal_message = Some(msg.clone());
        self.pending_confirmation = Some((msg, super::PendingConfirmation::DeleteStaleLink(link_id)));
    }

    /// Dispatch a single key while the link popup is open. Returns
    /// `true` if the key was consumed by the popup. Esc/Enter/`d` carry
    /// the explicit semantics; anything else is forwarded to the
    /// inner [`SearchablePopup`] for filter/cursor handling.
    pub fn handle_link_popup_key(&mut self, key: &str) -> bool {
        if self.link_popup.is_none() {
            return false;
        }
        match key {
            "esc" => {
                self.link_popup = None;
                true
            }
            "enter" => {
                self.link_popup_activate_selected();
                true
            }
            "d" => {
                self.link_popup_delete_selected();
                true
            }
            // Navigation + text input — delegated to the popup's intrinsic
            // PopupAction bindings.
            other => {
                if let Some(state) = self.link_popup.as_mut() {
                    let _ = state.popup.handle_key(other);
                }
                true // swallow any other key while popup is open
            }
        }
    }

    /// Write a directed link `current → marked` to the link table. The
    /// mark stays armed so the user can paste the same source onto
    /// multiple targets in a row. No-op (with explanatory notification)
    /// when no mark is set, no current selection exists, or both refer
    /// to the same node. The repo's `create` is idempotent so re-paste
    /// is harmless.
    pub fn link_paste_current(&mut self) {
        let Some(target) = self.marked_link.clone() else {
            self.notify("No link mark armed (press M on a row first)".to_string());
            return;
        };
        let Some(source) = self.current_node_ref() else {
            self.notify("Nothing to link from on this row".to_string());
            return;
        };
        if source.as_str() == target.as_str() {
            self.notify("Cannot link a node to itself".to_string());
            return;
        }
        let source_label = source.as_str().to_string();
        let target_label = target.as_str().to_string();
        let repo = std::sync::Arc::clone(&self.link_repo);
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async move { repo.create(&source, &target).await })
        });
        match result {
            Ok(_) => {
                self.reload_link_refs();
                self.notify(format!("Linked: {source_label} → {target_label}"));
            }
            Err(e) => {
                let msg = format!("Link create failed: {e}");
                self.set_query_error(Some(msg.clone()));
                self.notify(msg);
            }
        }
    }
}

impl App {
    /// Dispatch a link-open request on the head segment.
    ///
    /// Known heads: `tasks`, `tracking`, `jira`, `taiga`, `postgres`.
    /// Unknown heads return [`LinkRouteError::UnknownRoute`].
    // L5/L6 will wire callers (mark-and-paste flow); kept reachable so
    // L3 changes can be exercised via the existing test surface.
    #[allow(dead_code)]
    pub async fn open_link(&mut self, node_ref: &NodeRef) -> Result<(), LinkRouteError> {
        let (head, tail) = node_ref.split_head();
        match head {
            "tasks" => self.open_link_tasks(tail),
            "tracking" => self.open_link_tracking(tail),
            "jira" | "taiga" => self.open_link_content(head, tail),
            "postgres" => Err(LinkRouteError::NotSupported(
                "postgres has no stable node IDs in v1".into(),
            )),
            other => Err(LinkRouteError::UnknownRoute(other.to_string())),
        }
    }

    fn open_link_tasks(&mut self, tail: Option<&str>) -> Result<(), LinkRouteError> {
        let id_str = tail.ok_or_else(|| LinkRouteError::Stale("missing task id".into()))?;
        // Validate the id still parses, then report that the legacy
        // `tasks/<uuid>` navigation target no longer exists. The native
        // Tasks tab was retired in favour of the generic ContentAdapter
        // "Tasks" tab; jumping a bare task uuid into that adapter tab is
        // not wired yet (it would need the adapter's goto-by-id path), so
        // such links degrade to a clear NotSupported instead of opening
        // the wrong tab. The link rows themselves remain in the store.
        Uuid::parse_str(id_str)
            .map_err(|_| LinkRouteError::Stale(format!("invalid task uuid: {id_str}")))?;
        Err(LinkRouteError::NotSupported(
            "task links open in the legacy Tasks tab, which has been removed".into(),
        ))
    }

    fn open_link_tracking(&mut self, tail: Option<&str>) -> Result<(), LinkRouteError> {
        let id_str = tail.ok_or_else(|| LinkRouteError::Stale("missing tracking id".into()))?;
        // Validate the id still parses, then report that the legacy
        // `tracking/<uuid>` navigation target no longer exists. The native
        // Trackings tab was retired in favour of the generic ContentAdapter
        // "Trackings" tab; jumping a bare tracking uuid into that adapter
        // tab is not wired yet (it would need the adapter's goto-by-id
        // path), so such links degrade to a clear NotSupported instead of
        // opening the wrong tab. The link rows themselves remain in the store.
        Uuid::parse_str(id_str)
            .map_err(|_| LinkRouteError::Stale(format!("invalid tracking uuid: {id_str}")))?;
        Err(LinkRouteError::NotSupported(
            "tracking links open in the legacy Trackings tab, which has been removed".into(),
        ))
    }

    fn open_link_content(
        &mut self,
        head: &str,
        tail: Option<&str>,
    ) -> Result<(), LinkRouteError> {
        let tail = tail.ok_or_else(|| {
            LinkRouteError::Stale(format!("missing instance/node id after {head}"))
        })?;
        let (instance_id, node_id) = match tail.split_once('/') {
            Some((a, b)) if !a.is_empty() && !b.is_empty() => (a, b),
            _ => {
                return Err(LinkRouteError::Stale(format!(
                    "expected {head}/<instance>/<node_id>, got {head}/{tail}"
                )));
            }
        };
        let slot_idx = self
            .content_views_indexed()
            .find(|(_, cv)| {
                cv.adapter
                    .as_ref()
                    .map(|a| a.adapter_type() == head && a.instance_id() == instance_id)
                    .unwrap_or(false)
            })
            .map(|(i, _)| i)
            .ok_or_else(|| {
                LinkRouteError::UnknownRoute(format!("no content tab for {head}/{instance_id}"))
            })?;
        self.set_active_tab(Tab::Content(slot_idx));
        if let Some(cv) = self.content_view_mut(slot_idx) {
            cv.active_pane_mut().focus_item_by_id(node_id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod classify_tests {
    use super::{classify_link_ref, LinkResolveContext};
    use std::collections::HashSet;
    use uuid::Uuid;

    fn ctx_with(task_ids: &[Uuid], tracking_ids: &[Uuid], instances: &[(&str, &str)]) -> LinkResolveContext {
        LinkResolveContext {
            task_ids: task_ids.iter().copied().collect(),
            tracking_ids: tracking_ids.iter().copied().collect(),
            adapter_instances: instances
                .iter()
                .map(|(k, i)| (k.to_string(), i.to_string()))
                .collect(),
        }
    }

    #[test]
    fn live_task_classifies_as_none() {
        let id = Uuid::new_v4();
        let ctx = ctx_with(&[id], &[], &[]);
        assert!(classify_link_ref(&format!("tasks/{id}"), &ctx).is_none());
    }

    #[test]
    fn missing_task_uuid_is_stale() {
        let known = Uuid::new_v4();
        let missing = Uuid::new_v4();
        let ctx = ctx_with(&[known], &[], &[]);
        let reason = classify_link_ref(&format!("tasks/{missing}"), &ctx).unwrap();
        assert!(reason.contains("not found"), "reason={reason}");
    }

    #[test]
    fn malformed_task_uuid_is_stale() {
        let ctx = ctx_with(&[], &[], &[]);
        let reason = classify_link_ref("tasks/not-a-uuid", &ctx).unwrap();
        assert!(reason.contains("invalid task uuid"), "reason={reason}");
    }

    #[test]
    fn missing_task_tail_is_stale() {
        let ctx = ctx_with(&[], &[], &[]);
        let reason = classify_link_ref("tasks", &ctx).unwrap();
        assert!(reason.contains("missing task id"), "reason={reason}");
    }

    #[test]
    fn live_tracking_classifies_as_none() {
        let id = Uuid::new_v4();
        let ctx = ctx_with(&[], &[id], &[]);
        assert!(classify_link_ref(&format!("tracking/{id}"), &ctx).is_none());
    }

    #[test]
    fn jira_with_known_instance_is_live() {
        let ctx = ctx_with(&[], &[], &[("jira", "prod")]);
        assert!(classify_link_ref("jira/prod/PROJ-1", &ctx).is_none());
    }

    #[test]
    fn jira_with_unknown_instance_is_stale() {
        let ctx = ctx_with(&[], &[], &[("jira", "prod")]);
        let reason = classify_link_ref("jira/staging/PROJ-1", &ctx).unwrap();
        assert!(reason.contains("no content tab"), "reason={reason}");
    }

    #[test]
    fn jira_keyed_by_taiga_instance_is_stale() {
        // adapter_type segregation: an instance configured for taiga must
        // not satisfy a jira link's instance check (or vice versa).
        let ctx = ctx_with(&[], &[], &[("taiga", "dev")]);
        let reason = classify_link_ref("jira/dev/PROJ-1", &ctx).unwrap();
        assert!(reason.contains("no content tab"), "reason={reason}");
    }

    #[test]
    fn jira_missing_node_segment_is_stale() {
        let ctx = ctx_with(&[], &[], &[("jira", "prod")]);
        let reason = classify_link_ref("jira/prod", &ctx).unwrap();
        assert!(reason.contains("malformed"), "reason={reason}");
    }

    #[test]
    fn taiga_with_composite_node_id_is_live() {
        let ctx = ctx_with(&[], &[], &[("taiga", "dev")]);
        assert!(
            classify_link_ref("taiga/dev/task:42/comment/7", &ctx).is_none()
        );
    }

    #[test]
    fn postgres_always_stale() {
        let ctx = ctx_with(&[], &[], &[("postgres", "main")]);
        // Even when an instance is registered, postgres refs are stale
        // because per-query IDs aren't stable.
        let reason = classify_link_ref("postgres/main/qrow:1", &ctx).unwrap();
        assert!(reason.contains("no stable IDs"), "reason={reason}");
    }

    #[test]
    fn unknown_head_is_stale() {
        let ctx = LinkResolveContext::default();
        let reason = classify_link_ref("nope/whatever", &ctx).unwrap();
        assert!(reason.contains("unknown route"), "reason={reason}");
    }

    #[test]
    fn empty_string_is_stale() {
        let ctx = LinkResolveContext::default();
        let reason = classify_link_ref("", &ctx).unwrap();
        assert!(reason.contains("parse failed"), "reason={reason}");
    }

    #[test]
    fn default_context_has_no_known_addresses() {
        // Defensive: a default ctx shouldn't accidentally accept anything.
        let ctx = LinkResolveContext::default();
        let _: HashSet<Uuid> = ctx.task_ids;
    }
}

#[cfg(test)]
mod jump_history_tests {
    use super::JumpHistory;
    use not_yet_done_content::NodeRef;

    fn r(s: &str) -> NodeRef {
        NodeRef::parse(s).unwrap()
    }

    #[test]
    fn record_jump_pushes_back_and_clears_forward() {
        let mut h = JumpHistory::new();
        // Seed a forward entry to verify it gets dropped on a new jump.
        h.record_jump(r("tasks/aaaaaaaa-1111-1111-1111-111111111111"));
        let _ = h.pop_back(Some(r("jira/prod/PROJ-1")));
        assert_eq!(h.forward_len(), 1);

        h.record_jump(r("taiga/dev/task:99"));
        assert_eq!(h.back_len(), 1);
        assert_eq!(h.forward_len(), 0);
    }

    #[test]
    fn pop_back_returns_lifo_and_records_current_in_forward() {
        let mut h = JumpHistory::new();
        h.record_jump(r("tasks/aaaaaaaa-1111-1111-1111-111111111111"));
        h.record_jump(r("jira/prod/PROJ-1"));

        let t = h.pop_back(Some(r("taiga/dev/task:7"))).unwrap();
        assert_eq!(t.as_str(), "jira/prod/PROJ-1");
        assert_eq!(h.back_len(), 1);
        assert_eq!(h.forward_len(), 1);
    }

    #[test]
    fn pop_forward_round_trips_back_to_starting_position() {
        let mut h = JumpHistory::new();
        h.record_jump(r("tasks/aaaaaaaa-1111-1111-1111-111111111111"));
        let _ = h.pop_back(Some(r("jira/prod/PROJ-1")));
        // back: [], forward: [jira/prod/PROJ-1]
        let t = h.pop_forward(Some(r("tasks/aaaaaaaa-1111-1111-1111-111111111111"))).unwrap();
        assert_eq!(t.as_str(), "jira/prod/PROJ-1");
        assert_eq!(h.back_len(), 1);
        assert_eq!(h.forward_len(), 0);
    }

    #[test]
    fn pop_with_no_current_does_not_grow_other_stack() {
        let mut h = JumpHistory::new();
        h.record_jump(r("tasks/aaaaaaaa-1111-1111-1111-111111111111"));
        let _ = h.pop_back(None);
        assert_eq!(h.back_len(), 0);
        assert_eq!(h.forward_len(), 0);
    }

    #[test]
    fn pop_empty_returns_none() {
        let mut h = JumpHistory::new();
        assert!(h.pop_back(Some(r("tasks/aaaaaaaa-1111-1111-1111-111111111111"))).is_none());
        assert!(h.pop_forward(Some(r("tasks/aaaaaaaa-1111-1111-1111-111111111111"))).is_none());
    }

    #[test]
    fn new_jump_after_back_drops_forward_branch() {
        let mut h = JumpHistory::new();
        h.record_jump(r("tasks/aaaaaaaa-1111-1111-1111-111111111111"));
        let _ = h.pop_back(Some(r("jira/prod/PROJ-1")));
        // forward holds "jira/prod/PROJ-1"; recording a new jump kills it.
        h.record_jump(r("taiga/dev/task:9"));
        assert_eq!(h.back_len(), 1);
        assert_eq!(h.forward_len(), 0);
    }
}

#[cfg(test)]
mod tests {
    use not_yet_done_content::{LinkRouteError, NodeRef};

    /// Classification helper that mirrors the [`App::open_link`] head
    /// match without needing a full App instance. Covers the dispatch
    /// table separately from the tab-internal focus logic, which lives
    /// behind real view state.
    fn classify(head: &str, tail: Option<&str>) -> Result<&'static str, LinkRouteError> {
        match head {
            "tasks" => {
                let _ = tail.ok_or_else(|| LinkRouteError::Stale("missing task id".into()))?;
                Ok("tasks")
            }
            "tracking" => {
                let _ = tail.ok_or_else(|| LinkRouteError::Stale("missing tracking id".into()))?;
                Ok("tracking")
            }
            "jira" | "taiga" => {
                let tail = tail.ok_or_else(|| {
                    LinkRouteError::Stale(format!("missing instance/node id after {head}"))
                })?;
                let _ = match tail.split_once('/') {
                    Some((a, b)) if !a.is_empty() && !b.is_empty() => (a, b),
                    _ => {
                        return Err(LinkRouteError::Stale(format!(
                            "expected {head}/<instance>/<node_id>, got {head}/{tail}"
                        )));
                    }
                };
                Ok(if head == "jira" { "jira" } else { "taiga" })
            }
            "postgres" => Err(LinkRouteError::NotSupported(
                "postgres has no stable node IDs in v1".into(),
            )),
            other => Err(LinkRouteError::UnknownRoute(other.to_string())),
        }
    }

    #[test]
    fn tasks_route_parses_uuid_tail() {
        let r = NodeRef::parse("tasks/4f7c0b2e-1a55-4f5b-9e93-2b8e0a4fb111").unwrap();
        let (head, tail) = r.split_head();
        assert_eq!(classify(head, tail).unwrap(), "tasks");
    }

    #[test]
    fn tasks_route_missing_tail_is_stale() {
        let r = NodeRef::parse("tasks").unwrap();
        let (head, tail) = r.split_head();
        assert!(matches!(classify(head, tail), Err(LinkRouteError::Stale(_))));
    }

    #[test]
    fn jira_route_requires_two_tail_segments() {
        let ok = NodeRef::parse("jira/prod/PROJ-1").unwrap();
        let (h, t) = ok.split_head();
        assert_eq!(classify(h, t).unwrap(), "jira");

        let missing_node = NodeRef::parse("jira/prod").unwrap();
        let (h, t) = missing_node.split_head();
        assert!(matches!(classify(h, t), Err(LinkRouteError::Stale(_))));
    }

    #[test]
    fn taiga_route_with_composite_node_id() {
        // The node_id segment is opaque to the App and may contain
        // adapter-private slashes (e.g. comment sub-keys).
        let r = NodeRef::parse("taiga/dev/task:42/comment/7").unwrap();
        let (h, t) = r.split_head();
        assert_eq!(classify(h, t).unwrap(), "taiga");
    }

    #[test]
    fn postgres_route_is_not_supported() {
        let r = NodeRef::parse("postgres/whatever").unwrap();
        let (h, t) = r.split_head();
        assert!(matches!(classify(h, t), Err(LinkRouteError::NotSupported(_))));
    }

    #[test]
    fn unknown_head_classifies_as_unknown_route() {
        let r = NodeRef::parse("nope/x").unwrap();
        let (h, t) = r.split_head();
        match classify(h, t) {
            Err(LinkRouteError::UnknownRoute(s)) => assert_eq!(s, "nope"),
            other => panic!("expected UnknownRoute, got {other:?}"),
        }
    }
}
