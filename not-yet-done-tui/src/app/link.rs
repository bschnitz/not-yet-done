//! App-level entry point for [`NodeRef`] open-by-path dispatch.
//!
//! Every link ref has the same three-part shape, whatever produced it:
//! `<adapter_type>/<instance_id>/<node_id>`. The first two segments
//! pick the content slot, the rest is the adapter's own node id and
//! stays opaque to the host (it may itself contain slashes).
//!
//! Nothing here knows any adapter by name. What the host needs to
//! route a link, the adapter states through the trait:
//!
//! - [`AdapterCapabilities::unstable_node_ids`](not_yet_done_content::AdapterCapabilities::unstable_node_ids)
//!   — ids that don't survive a reload can't be linked at all, so such
//!   rows are neither markable nor followable.
//! - [`ContentAdapter::locate_node_path`] — where a node lives in the
//!   tree, so following a link can expand a subtree that isn't loaded
//!   yet. Optional: adapters that don't implement it simply lose deep
//!   links (a target that happens to be on screen still works).
//! - [`ContentAdapter::get_by_id`] — used by `:linkprune` to tell
//!   "target is gone" from "target just isn't loaded".

use std::collections::HashSet;

use not_yet_done_content::{LinkRouteError, NodeRef};
use uuid::Uuid;

use super::App;
use crate::components::searchable_popup::{PopupItem, SearchablePopup};
use crate::tabs::Tab;

/// Split a link ref into `(adapter_type, instance_id, node_id)`.
///
/// The node id is everything after the second segment and keeps its
/// slashes — composite ids (`<db>/schemas/<s>/tables/<t>`, a Jira
/// comment under its issue) are one opaque unit to the host.
pub fn split_link_addr(node_ref: &NodeRef) -> Result<(&str, &str, &str), String> {
    let raw = node_ref.as_str();
    let mut parts = raw.splitn(3, '/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(kind), Some(instance), Some(node_id))
            if !kind.is_empty() && !instance.is_empty() && !node_id.is_empty() =>
        {
            Ok((kind, instance, node_id))
        }
        _ => Err(format!(
            "expected <adapter>/<instance>/<node_id>, got {raw}"
        )),
    }
}

/// Snapshot of "what addresses currently resolve" used by
/// [`classify_link_ref`] for bulk staleness scanning. Computed once
/// per `:linkprune` invocation; kept as plain data so the classifier
/// itself stays a pure function.
#[derive(Debug, Default)]
pub struct LinkResolveContext {
    /// `(adapter_type, instance_id)` pairs for every currently configured
    /// content adapter, e.g. `("jira", "prod")`. A ref whose first two
    /// segments aren't in this set has no tab to open.
    pub adapter_instances: HashSet<(String, String)>,
    /// Adapter types that declared
    /// [`AdapterCapabilities::unstable_node_ids`](not_yet_done_content::AdapterCapabilities::unstable_node_ids)
    /// — their ids are positional or per-query, so any stored ref to
    /// them is meaningless by construction.
    pub unstable_id_adapters: HashSet<String>,
    /// Refs the host *proved* dead, mapped to the reason. Only positive
    /// evidence lands here (the owning adapter answered "not found"), so
    /// an unreachable server or an adapter that can't verify never costs
    /// a link.
    pub dead_refs: std::collections::HashMap<String, String>,
}

/// Pure classification: would [`App::open_link`] currently fail with
/// [`LinkRouteError::Stale`] / [`LinkRouteError::UnknownRoute`] for
/// this ref? Pulled out so it can be exercised with unit tests instead
/// of a live App.
///
/// Returns `None` for "looks live", `Some(reason)` for "would not
/// navigate". Conservative by design: a ref counts live unless its
/// shape is broken, no configured adapter owns it, its adapter has no
/// stable ids, or that adapter positively reported the node gone.
pub fn classify_link_ref(raw: &str, ctx: &LinkResolveContext) -> Option<String> {
    let node_ref = match NodeRef::parse(raw) {
        Ok(r) => r,
        Err(e) => return Some(format!("parse failed: {e}")),
    };
    let (kind, instance, _node_id) = match split_link_addr(&node_ref) {
        Ok(parts) => parts,
        Err(e) => return Some(e),
    };
    if ctx.unstable_id_adapters.contains(kind) {
        return Some(format!("{kind} has no stable node ids"));
    }
    if !ctx
        .adapter_instances
        .contains(&(kind.to_string(), instance.to_string()))
    {
        return Some(format!("no content tab for {kind}/{instance}"));
    }
    ctx.dead_refs.get(raw).cloned()
}

/// Ask `adapter` whether `node_id` still exists. `Some(reason)` means
/// "provably gone", `None` means "live, or the adapter couldn't tell".
///
/// The distinction is the whole point: `:linkprune` deletes rows, so it
/// must only act on a definite answer. A node counts gone when the
/// adapter reports [`ContentError::NotFound`] or hands back a node
/// flagged `deleted` in its metadata (the local adapters keep
/// soft-deleted rows but mark them). Every other error — expired
/// session, unreachable host, adapter that doesn't implement lookup by
/// id — keeps the link, exactly as an unconfigured adapter does.
async fn probe_ref_dead(
    adapter: &dyn not_yet_done_content::ContentAdapter,
    node_id: &str,
) -> Option<String> {
    match adapter.get_by_id(node_id).await {
        Ok(node) => {
            let deleted = node
                .metadata()
                .fields
                .iter()
                .any(|f| f.key == "deleted" && f.value == "true");
            deleted.then(|| format!("{node_id} is deleted"))
        }
        Err(not_yet_done_content::ContentError::NotFound(_)) => {
            Some(format!("{node_id} not found"))
        }
        Err(_) => None,
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
    /// tab, or `None` when nothing addressable is selected — including
    /// rows of an adapter that declared `unstable_node_ids`, whose ids
    /// can't survive a refresh let alone a process restart.
    pub fn current_node_ref(&self) -> Option<NodeRef> {
        let Tab::Content(idx) = self.active_tab;
        let cv = self.content_view(idx)?;
        let adapter = cv.adapter.as_ref()?;
        let kind = adapter.adapter_type();
        if adapter.capabilities().unstable_node_ids {
            return None;
        }
        let node_id = cv.active_pane().selected_item_id()?;
        NodeRef::parse(&format!("{}/{}/{}", kind, adapter.instance_id(), node_id)).ok()
    }

    /// Capture the current selection into [`App::marked_link`]. Notifies
    /// either the captured ref or — when the selection isn't addressable
    /// (broken tab, empty list, an adapter with `unstable_node_ids`) — a
    /// "nothing to mark" message.
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
        let title = format!(
            "Links · {} · ↵ open · d delete · esc close",
            anchor.as_str()
        );
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
            tokio::runtime::Handle::current().block_on(async move { repo.delete(id).await })
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
    /// offers to drop it from the link table. `NotSupported` and `Other`
    /// stay informational — an unstable-id adapter, a target that just
    /// isn't loaded, or transient I/O must never cost a good link.
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
            tokio::runtime::Handle::current().block_on(async { self.open_link(&node_ref).await })
        });
        match outcome {
            Ok(()) => {
                self.jump_history.record_jump(anchor);
            }
            Err(LinkRouteError::Stale(reason)) | Err(LinkRouteError::UnknownRoute(reason)) => {
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
            tokio::runtime::Handle::current().block_on(async { self.open_link(&target).await })
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

    /// Build a [`LinkResolveContext`] for the given link refs by asking
    /// each ref's *owning* adapter whether the node still exists.
    ///
    /// Ownership comes straight from the ref: `<adapter_type>/<instance>`
    /// selects the adapter, the rest is its node id. Only refs that have
    /// such an adapter and stable ids get probed, each through
    /// [`ContentAdapter::get_by_id`] — see [`probe_ref_dead`] for why
    /// only a definite "gone" counts. Probes run concurrently because a
    /// remote adapter may answer in its own time and `:linkprune` blocks
    /// the UI while it scans.
    fn build_link_resolve_context(&self, refs: &[&str]) -> LinkResolveContext {
        use not_yet_done_content::ContentAdapter;

        let adapter_instances: HashSet<(String, String)> = self
            .content_views_iter()
            .filter_map(|cv| {
                cv.adapter
                    .as_ref()
                    .map(|a| (a.adapter_type().to_string(), a.instance_id().to_string()))
            })
            .collect();
        let unstable_id_adapters: HashSet<String> = self
            .content_views_iter()
            .filter_map(|cv| cv.adapter.as_ref())
            .filter(|a| a.capabilities().unstable_node_ids)
            .map(|a| a.adapter_type().to_string())
            .collect();

        // Pair each distinct ref with the adapter that owns it. Refs
        // without an owner, or owned by an adapter whose ids don't
        // survive a reload, are classified without probing.
        let mut targets: Vec<(String, String, std::sync::Arc<dyn ContentAdapter>)> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        for raw in refs {
            if !seen.insert(raw) {
                continue;
            }
            let Ok(node_ref) = NodeRef::parse(raw) else {
                continue;
            };
            let Ok((kind, instance, node_id)) = split_link_addr(&node_ref) else {
                continue;
            };
            if unstable_id_adapters.contains(kind) {
                continue;
            }
            let owner = self.content_views_iter().find_map(|cv| {
                let a = cv.adapter.as_ref()?;
                (a.adapter_type() == kind && a.instance_id() == instance)
                    .then(|| std::sync::Arc::clone(a))
            });
            if let Some(adapter) = owner {
                targets.push((raw.to_string(), node_id.to_string(), adapter));
            }
        }

        let dead_refs = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                let mut set = tokio::task::JoinSet::new();
                for (raw, node_id, adapter) in targets {
                    set.spawn(async move {
                        let verdict = probe_ref_dead(adapter.as_ref(), &node_id).await;
                        (raw, verdict)
                    });
                }
                let mut dead = std::collections::HashMap::new();
                while let Some(joined) = set.join_next().await {
                    // A panicking probe must not take the scan down; an
                    // unanswered ref simply stays live.
                    if let Ok((raw, Some(reason))) = joined {
                        dead.insert(raw, reason);
                    }
                }
                dead
            })
        });

        LinkResolveContext {
            adapter_instances,
            unstable_id_adapters,
            dead_refs,
        }
    }

    /// `:linkprune` entry point. Scans every link row, classifies each
    /// endpoint via [`classify_link_ref`], collects rows where source or
    /// target no longer resolves, and prompts the user to confirm a
    /// bulk delete. No-op when nothing is stale or the link table is
    /// empty — the modal then just reports the empty result so the user
    /// knows the scan ran.
    pub fn link_prune_command(&mut self) {
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
        // Resolve only the refs this table actually uses, by probing the
        // adapters that own the task/tracking data.
        let refs: Vec<&str> = rows
            .iter()
            .flat_map(|r| [r.source_ref.as_str(), r.target_ref.as_str()])
            .collect();
        let ctx = self.build_link_resolve_context(&refs);
        let mut stale_ids: Vec<Uuid> = Vec::new();
        let mut sample: Vec<String> = Vec::new();
        for row in &rows {
            let src_stale = classify_link_ref(&row.source_ref, &ctx);
            let tgt_stale = classify_link_ref(&row.target_ref, &ctx);
            if src_stale.is_some() || tgt_stale.is_some() {
                stale_ids.push(row.id);
                if sample.len() < 5 {
                    let reason = src_stale.as_deref().or(tgt_stale.as_deref()).unwrap_or("");
                    sample.push(format!(
                        "{} → {} ({})",
                        row.source_ref, row.target_ref, reason
                    ));
                }
            }
        }
        if stale_ids.is_empty() {
            self.modal_message = Some(format!("Scanned {} link(s). None are stale.", rows.len()));
            return;
        }
        let mut body = format!("{} of {} link(s) are stale:\n", stale_ids.len(), rows.len());
        for line in &sample {
            body.push_str(&format!("  {line}\n"));
        }
        if stale_ids.len() > sample.len() {
            body.push_str(&format!(
                "  … and {} more\n",
                stale_ids.len() - sample.len()
            ));
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
            tokio::runtime::Handle::current().block_on(async { self.open_link(&target).await })
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
        let msg = format!("Stale link `{target}`\n({reason})\nDelete from link table? (y/n)");
        self.modal_message = Some(msg.clone());
        self.pending_confirmation =
            Some((msg, super::PendingConfirmation::DeleteStaleLink(link_id)));
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
    /// Follow a link ref: switch to the content slot named by
    /// `<adapter_type>/<instance_id>` and put the cursor on `<node_id>`.
    ///
    /// Adapter-agnostic in every step. The node id is handed to the
    /// adapter untouched, and reaching a row that isn't on screen goes
    /// through [`ContentAdapter::locate_node_path`] — so an adapter gets
    /// deep links by implementing one method, and loses nothing else by
    /// not implementing it.
    pub async fn open_link(&mut self, node_ref: &NodeRef) -> Result<(), LinkRouteError> {
        let (kind, instance, node_id) = split_link_addr(node_ref).map_err(LinkRouteError::Stale)?;
        let (kind, node_id) = (kind.to_string(), node_id.to_string());

        let (slot_idx, adapter) = self
            .content_views_indexed()
            .find_map(|(i, cv)| {
                let a = cv.adapter.as_ref()?;
                (a.adapter_type() == kind && a.instance_id() == instance)
                    .then(|| (i, std::sync::Arc::clone(a)))
            })
            .ok_or_else(|| {
                LinkRouteError::UnknownRoute(format!("no content tab for {kind}/{instance}"))
            })?;

        // Refs to per-query ids can't be honoured — the row they named
        // is long gone even if the string still parses.
        if adapter.capabilities().unstable_node_ids {
            return Err(LinkRouteError::NotSupported(format!(
                "{kind} has no stable node ids"
            )));
        }

        self.set_active_tab(Tab::Content(slot_idx));

        // Cheap path: the row is already among the loaded ones.
        if let Some(cv) = self.content_view_mut(slot_idx) {
            if cv.active_pane_mut().focus_item_by_id(&node_id) {
                return Ok(());
            }
        }

        // Not loaded — ask the adapter where the node lives so the
        // ancestors can be expanded on the way to it.
        match adapter.locate_node_path(&node_id).await {
            Ok(Some(path)) if !path.is_empty() => self.reveal_tree_path(slot_idx, path, &node_id),
            Ok(_) => {
                // No path: either the adapter can't locate nodes, or the
                // node is gone. Only the second may offer a prune, so
                // ask before blaming the link.
                Err(self.classify_unreachable(&adapter, &kind, &node_id).await)
            }
            Err(e) => Err(LinkRouteError::Other(format!(
                "{kind} could not locate {node_id}: {e}"
            ))),
        }
    }

    /// Turn "couldn't reach the node" into the right error kind: a
    /// definite `NotFound` from the adapter is [`LinkRouteError::Stale`]
    /// (the UI then offers to drop the row), everything else stays
    /// [`LinkRouteError::NotSupported`] so a link to a node that merely
    /// isn't loaded never gets deleted.
    async fn classify_unreachable(
        &self,
        adapter: &std::sync::Arc<dyn not_yet_done_content::ContentAdapter>,
        kind: &str,
        node_id: &str,
    ) -> LinkRouteError {
        match probe_ref_dead(adapter.as_ref(), node_id).await {
            Some(reason) => LinkRouteError::Stale(reason),
            None => LinkRouteError::NotSupported(format!(
                "{node_id} is not among the loaded rows and {kind} can't locate it"
            )),
        }
    }

    /// Drive the tree to `path` (root → … → target) and land the cursor
    /// on its last segment, reusing the tree-find expand walker: seed
    /// the pane with a single synthetic hit, then let the normal
    /// `NeedTreeExpand` → `TreeChildren` → re-poll chain do the work.
    fn reveal_tree_path(
        &mut self,
        view_index: usize,
        path: Vec<String>,
        label: &str,
    ) -> Result<(), LinkRouteError> {
        let pane_id = {
            let Some(cv) = self.content_view_mut(view_index) else {
                return Err(LinkRouteError::Other("content tab vanished".into()));
            };
            if !cv.active_view_is_tree() {
                return Err(LinkRouteError::NotSupported(format!(
                    "{label} is not on the current page (flat view — no path to expand)"
                )));
            }
            let pane_id = cv.active_pane_id();
            let pane = cv.active_pane_mut();
            // The query string is only status-bar decoration here; the
            // hit we inject is what the walker follows.
            pane.tree_find_begin(format!("id:{label}"));
            pane.tree_find_complete(
                vec![not_yet_done_content::TreeFindHit {
                    path,
                    label: label.to_string(),
                    space_key: String::new(),
                }],
                false,
            );
            pane_id
        };
        self.drive_tree_find_chain(view_index, pane_id);
        Ok(())
    }
}

#[cfg(test)]
mod classify_tests {
    use super::{LinkResolveContext, classify_link_ref};

    /// `instances` are the configured `(adapter_type, instance_id)` pairs,
    /// `unstable` the adapter types whose ids don't survive a reload,
    /// `dead` the refs an adapter positively reported gone.
    fn ctx_with(
        instances: &[(&str, &str)],
        unstable: &[&str],
        dead: &[(&str, &str)],
    ) -> LinkResolveContext {
        LinkResolveContext {
            adapter_instances: instances
                .iter()
                .map(|(k, i)| (k.to_string(), i.to_string()))
                .collect(),
            unstable_id_adapters: unstable.iter().map(|s| s.to_string()).collect(),
            dead_refs: dead
                .iter()
                .map(|(r, why)| (r.to_string(), why.to_string()))
                .collect(),
        }
    }

    #[test]
    fn known_instance_is_live() {
        let ctx = ctx_with(&[("jira", "prod")], &[], &[]);
        assert!(classify_link_ref("jira/prod/PROJ-1", &ctx).is_none());
    }

    #[test]
    fn unknown_instance_is_stale() {
        let ctx = ctx_with(&[("jira", "prod")], &[], &[]);
        let reason = classify_link_ref("jira/staging/PROJ-1", &ctx).unwrap();
        assert!(reason.contains("no content tab"), "reason={reason}");
    }

    #[test]
    fn instance_of_another_adapter_type_is_stale() {
        // adapter_type segregation: an instance configured for taiga must
        // not satisfy a jira link's instance check (or vice versa).
        let ctx = ctx_with(&[("taiga", "dev")], &[], &[]);
        let reason = classify_link_ref("jira/dev/PROJ-1", &ctx).unwrap();
        assert!(reason.contains("no content tab"), "reason={reason}");
    }

    #[test]
    fn any_adapter_type_routes_without_a_whitelist() {
        // The classifier knows no adapter names — a freshly added adapter
        // is linkable the moment it has a configured instance.
        let ctx = ctx_with(&[("sqlite", "local"), ("confluence", "wiki")], &[], &[]);
        assert!(classify_link_ref("sqlite/local/main/tables/t", &ctx).is_none());
        assert!(classify_link_ref("confluence/wiki/12345", &ctx).is_none());
    }

    #[test]
    fn composite_node_id_keeps_its_slashes() {
        let ctx = ctx_with(&[("taiga", "dev")], &[], &[]);
        assert!(classify_link_ref("taiga/dev/task:42/comment/7", &ctx).is_none());
    }

    #[test]
    fn missing_node_segment_is_stale() {
        let ctx = ctx_with(&[("jira", "prod")], &[], &[]);
        let reason = classify_link_ref("jira/prod", &ctx).unwrap();
        assert!(reason.contains("expected"), "reason={reason}");
    }

    #[test]
    fn unstable_ids_are_always_stale() {
        // Even with a configured instance: per-query ids can't be stored.
        let ctx = ctx_with(&[("postgres", "main")], &["postgres"], &[]);
        let reason = classify_link_ref("postgres/main/qrow:1", &ctx).unwrap();
        assert!(reason.contains("no stable node ids"), "reason={reason}");
    }

    #[test]
    fn probed_dead_ref_is_stale() {
        let ctx = ctx_with(
            &[("tasks", "local")],
            &[],
            &[("tasks/local/abc", "abc is deleted")],
        );
        let reason = classify_link_ref("tasks/local/abc", &ctx).unwrap();
        assert!(reason.contains("deleted"), "reason={reason}");
    }

    #[test]
    fn unprobed_ref_stays_live() {
        // The adapter couldn't verify (network, no lookup support) — the
        // link survives rather than being pruned on a guess.
        let ctx = ctx_with(&[("tasks", "local")], &[], &[]);
        assert!(classify_link_ref("tasks/local/abc", &ctx).is_none());
    }

    #[test]
    fn unconfigured_adapter_is_stale() {
        let ctx = LinkResolveContext::default();
        let reason = classify_link_ref("nope/inst/whatever", &ctx).unwrap();
        assert!(reason.contains("no content tab"), "reason={reason}");
    }

    #[test]
    fn empty_string_is_stale() {
        let ctx = LinkResolveContext::default();
        let reason = classify_link_ref("", &ctx).unwrap();
        assert!(reason.contains("parse failed"), "reason={reason}");
    }

    #[test]
    fn default_context_accepts_nothing() {
        let ctx = LinkResolveContext::default();
        assert!(ctx.adapter_instances.is_empty());
        assert!(classify_link_ref("jira/prod/PROJ-1", &ctx).is_some());
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
        let t = h
            .pop_forward(Some(r("tasks/aaaaaaaa-1111-1111-1111-111111111111")))
            .unwrap();
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
        assert!(
            h.pop_back(Some(r("tasks/aaaaaaaa-1111-1111-1111-111111111111")))
                .is_none()
        );
        assert!(
            h.pop_forward(Some(r("tasks/aaaaaaaa-1111-1111-1111-111111111111")))
                .is_none()
        );
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
mod addr_tests {
    use super::split_link_addr;
    use not_yet_done_content::NodeRef;

    /// The previous version of these tests exercised a *copy* of the
    /// routing match kept in the test module. That copy is why the ref
    /// format could drift from what `current_node_ref` produced without
    /// a single test failing — so the address split is now tested
    /// through the same function the router calls.
    fn split(raw: &str) -> Result<(String, String, String), String> {
        let r = NodeRef::parse(raw).map_err(|e| e.to_string())?;
        split_link_addr(&r).map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string()))
    }

    #[test]
    fn splits_adapter_instance_and_node_id() {
        let (kind, instance, node_id) = split("jira/prod/PROJ-1").unwrap();
        assert_eq!(
            (kind.as_str(), instance.as_str(), node_id.as_str()),
            ("jira", "prod", "PROJ-1")
        );
    }

    #[test]
    fn node_id_keeps_its_own_slashes() {
        // Composite ids are one opaque unit: only the first two
        // separators belong to the host.
        let (_, _, node_id) = split("taiga/dev/task:42/comment/7").unwrap();
        assert_eq!(node_id, "task:42/comment/7");
        let (_, _, node_id) = split("postgres/main/db/schemas/public/tables/t").unwrap();
        assert_eq!(node_id, "db/schemas/public/tables/t");
    }

    #[test]
    fn missing_node_segment_is_an_error() {
        assert!(split("jira/prod").is_err());
    }

    #[test]
    fn bare_adapter_head_is_an_error() {
        assert!(split("tasks").is_err());
    }

    #[test]
    fn uuid_node_ids_need_no_special_case() {
        // Task refs go through the very same split — the old two-segment
        // `tasks/<uuid>` form no longer exists anywhere.
        let (kind, instance, node_id) =
            split("tasks/local/4f7c0b2e-1a55-4f5b-9e93-2b8e0a4fb111").unwrap();
        assert_eq!(kind, "tasks");
        assert_eq!(instance, "local");
        assert_eq!(node_id, "4f7c0b2e-1a55-4f5b-9e93-2b8e0a4fb111");
    }
}
