//! Hook-triggered view scripts — the same scripts the `:script` menu
//! runs, started by an event instead of a key press.
//!
//! Three pieces:
//!   - [`ScriptHook`]: the vocabulary — `reload` (after the rows landed)
//!     and `load` (before they reach the table), stored per `(scope, name)`
//!     in the `script_hook` table next to the script's key chord — see
//!     [`crate::app::script::ScriptContext::shortcut_scope`] for the scope
//!     string both share.
//!   - The picker ([`ScriptHookPicker`]), opened with Ctrl+H on a selected
//!     entry in the script menu.
//!   - Two firing seams, both in the `LoadMsg::ContentItems` handler:
//!     [`App::run_load_hooks`] on the rows *before* they are handed to the
//!     pane, and [`App::fire_reload_hooks`] once they have landed.
//!
//! **Why two hooks and not one.** They have different contracts and only
//! one of them can be moved. A `reload` hook sees the settled view — the
//! cursor, the filtered rows — and may emit commands (`:reload`, `:jump`)
//! that presuppose a table to act on; what it changes in the data it has to
//! write out and pull back in with a second load. A `load` hook has no
//! cursor (nothing is selected yet) and may emit no commands at all — it
//! would be emitting them into the middle of the load that is running it —
//! but it can hand back a patch that lands in the rows before anyone sees
//! them, which costs no second load and never shows a stale value. Colours
//! (`highlights`) and the row order (`order`) ride that same answer and are
//! therefore `load`-hook privileges too: after the table exists there is
//! nothing left to paint and nothing left to reorder, so a `reload` hook
//! naming either key is refused
//! ([`crate::app::script::load_only_key_rejection`]).
//!
//! **Loop protection.** A hook script may itself ask for a reload — that
//! is the point of the `commands` mode — so the trigger has to be able to
//! tell "the user reloaded" from "a script reloaded". The answer rides
//! with the load: [`App::load_hook_depth`] is the ambient depth every
//! newly spawned load is stamped with, and it is raised for exactly as
//! long as a hook run (plus every command it emits) is executing. Loads
//! arriving with a depth of [`HOOK_MAX_DEPTH`] or more do not fire hooks,
//! so a reload *caused by* a reload script runs no reload scripts. The
//! guard deliberately sits on the load and not on the command name: a
//! script emitting `:jump`, a query command or anything else that ends in
//! a fetch would otherwise slip past a `:reload`-only check.

use std::collections::HashMap;
use std::sync::Arc;

use not_yet_done_content::{MetadataField, NodeSummary};

use crate::app::editor::parse_script_mode;
use crate::app::{App, EditorRequest};
use crate::components::searchable_popup::{PopupItem, SearchablePopup};
use crate::config::highlight::StyleResolver;
use crate::config::keybindings::ScriptMenuAction;
use crate::views::content_highlights::ScriptHighlights;
use crate::views::content_view::PaneId;

/// Events a view script can be bound to. Unknown values read from the
/// database parse to `None` and are ignored, so a binding written by a
/// newer version never breaks an older one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptHook {
    /// Run right after the pane's rows have (re)loaded — including the
    /// first load when the view opens, and a drill-down into a level.
    Reload,
    /// Run on the freshly loaded rows *before* they reach the pane, with
    /// the chance to patch them (see [`App::run_load_hooks`]).
    Load,
}

impl ScriptHook {
    pub const ALL: &'static [ScriptHook] = &[ScriptHook::Reload, ScriptHook::Load];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reload => "reload",
            Self::Load => "load",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "reload" => Some(Self::Reload),
            "load" => Some(Self::Load),
            _ => None,
        }
    }

    /// One-line explanation shown next to the entry in the picker.
    pub fn description(self) -> &'static str {
        match self {
            Self::Reload => "after the view's rows (re)loaded",
            Self::Load => "on the rows before they reach the table",
        }
    }
}

/// Hook depth at which hooks stop firing. `1` means: a load caused by a
/// hook script runs no further hooks.
pub const HOOK_MAX_DEPTH: u8 = 1;

/// Open picker over [`ScriptHook::ALL`], plus a "no hook" entry. Bound to
/// one `(scope, name)` — the same key the script's chord is stored under.
pub struct ScriptHookPicker {
    pub popup: SearchablePopup,
    scope: String,
    name: String,
}

impl App {
    /// All hook bindings registered for `scope`, as `(script name, hook)`.
    /// Empty on a DB error — a hook that cannot be read must not take the
    /// view down with it.
    fn hooks_for_scope(&self, scope: &str) -> Vec<(String, String)> {
        let repo = Arc::clone(&self.script_hook_repo);
        let scope = scope.to_string();
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async move { repo.list_by_scope(&scope).await.unwrap_or_default() })
        })
        .into_iter()
        .map(|m| (m.name, m.hook))
        .collect()
    }

    /// Map of `script name → hook`, for decorating the script menu.
    pub(super) fn hook_labels_for_scope(
        &self,
        scope: &str,
    ) -> std::collections::HashMap<String, String> {
        self.hooks_for_scope(scope).into_iter().collect()
    }

    /// Persist (or clear, when `hook` is `None`) the hook of one script.
    fn store_script_hook(&mut self, scope: &str, name: &str, hook: Option<ScriptHook>) {
        let repo = Arc::clone(&self.script_hook_repo);
        let scope_owned = scope.to_string();
        let name_owned = name.to_string();
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                match hook {
                    Some(h) => repo
                        .set(&scope_owned, &name_owned, h.as_str())
                        .await
                        .map(|_| ()),
                    None => repo.unset(&scope_owned, &name_owned).await,
                }
            })
        });
        match result {
            Ok(()) => match hook {
                Some(h) => self.notify(format!("Script '{name}' now runs {}", h.description())),
                None => self.notify(format!("Hook removed from script '{name}'")),
            },
            Err(e) => self.notify_error(format!("Failed to store hook for '{name}': {e}")),
        }
    }

    /// Drop a script's hook binding without a message — used when the
    /// script file itself is deleted, so no orphan row keeps pointing at
    /// a script that is gone.
    pub(super) fn clear_script_hook(&mut self, scope: &str, name: &str) {
        let repo = Arc::clone(&self.script_hook_repo);
        let scope = scope.to_string();
        let name = name.to_string();
        let _ = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async move { repo.unset(&scope, &name).await })
        });
    }

    /// Ctrl+H in the script menu: offer the hooks this script can be bound
    /// to. Interactive and capture modes are rejected up front — a hook
    /// fires while a load lands, where there is nobody to hand a terminal
    /// or an editor to.
    pub(super) fn open_script_hook_picker(&mut self, scope: String, name: String, path: &str) {
        if let Some(reason) = hook_mode_rejection(path) {
            self.notify_error(format!("Cannot hook '{name}': {reason}"));
            return;
        }
        let current = self
            .hooks_for_scope(&scope)
            .into_iter()
            .find(|(n, _)| n == &name)
            .map(|(_, h)| h);
        let mut items = vec![PopupItem {
            label: "none".to_string(),
            value: String::new(),
            marked: current.is_none(),
            suffix: Some("run manually only".to_string()),
        }];
        items.extend(ScriptHook::ALL.iter().map(|h| PopupItem {
            label: h.as_str().to_string(),
            value: h.as_str().to_string(),
            marked: current.as_deref() == Some(h.as_str()),
            suffix: Some(h.description().to_string()),
        }));
        let popup = SearchablePopup::new(
            Arc::clone(&self.shared_theme),
            format!("Hook · {name}"),
            items,
        )
        .with_popup_kb(
            self.keybindings.popup.clone(),
            self.keybindings.key_icons.clone(),
        )
        .with_hints(vec![
            (
                self.keybindings.script_menu.label(&ScriptMenuAction::Run),
                "bind".into(),
            ),
            (
                self.keybindings.script_menu.label(&ScriptMenuAction::Close),
                "cancel".into(),
            ),
        ]);
        self.script_hook_picker = Some(ScriptHookPicker { popup, scope, name });
    }

    /// Dispatch one keypress while the hook picker is open.
    pub fn handle_script_hook_picker_key(&mut self, key: &str) {
        let kb = self.keybindings.script_menu.clone();
        if kb
            .get(&ScriptMenuAction::Close)
            .is_some_and(|b| b.matches(key))
        {
            self.script_hook_picker = None;
            return;
        }
        if kb
            .get(&ScriptMenuAction::Run)
            .is_some_and(|b| b.matches(key))
        {
            let Some(picker) = self.script_hook_picker.take() else {
                return;
            };
            let Some(item) = picker.popup.selected_item() else {
                return;
            };
            let hook = ScriptHook::parse(&item.value);
            if hook.is_none() && !item.value.is_empty() {
                return;
            }
            self.store_script_hook(&picker.scope, &picker.name, hook);
            return;
        }
        if let Some(picker) = &mut self.script_hook_picker {
            picker.popup.handle_key(key);
        }
    }

    /// Run every script bound to [`ScriptHook::Load`] on the rows a load
    /// just produced, letting each one patch them before the pane — and
    /// with it the user — ever sees them.
    ///
    /// The rows are handed over in the `scope: table` payload shape and come
    /// back as `{"cells": {"<row id>": {"<column key>": <value>}}}`. Sparse
    /// by design: a script answers for what it computed, not for the table.
    ///
    /// A script may also answer with `highlights` in the same addressing;
    /// those are returned rather than applied, because they belong beside the
    /// rows on the pane and not in them (see [`ScriptHighlights`]).
    ///
    /// And it may answer with `order` — the row ids in the order it wants
    /// them shown. That one is applied here, on the rows themselves (see
    /// [`apply_row_order`]), and only while the pane carries no sort of the
    /// user's own.
    ///
    /// Runs synchronously, like the reload hook, and for the same reason —
    /// the rows in `items` are on their way to
    /// [`ContentView::set_items_for_pane`](crate::views::content_view::ContentView::set_items_for_pane)
    /// in this very message handler, and there is nothing to hand a patch to
    /// afterwards. It costs no *extra* stall either: nothing is painted
    /// between the two seams anyway, so a reload hook's runtime already sits
    /// in front of the first frame that shows the new rows.
    ///
    /// Several scripts on one level run in name order, each seeing what the
    /// previous one wrote — the payload is rebuilt from the patched rows.
    pub(super) fn run_load_hooks(
        &mut self,
        view_index: usize,
        pane_id: PaneId,
        hook_depth: u8,
        items: &mut [NodeSummary],
    ) -> ScriptHighlights {
        // Same provenance guard as the reload hook: rows fetched *because* a
        // hook asked for them run no hooks of their own. A load hook cannot
        // trigger a load itself (its commands are refused), so this is not
        // loop protection here — it keeps the two hooks from doubling up on
        // the round trip a reload hook already paid for.
        let mut highlights = ScriptHighlights::default();
        if hook_depth >= HOOK_MAX_DEPTH {
            return highlights;
        }
        if self.hook_runs_in_flight.contains(&(view_index, pane_id)) {
            return highlights;
        }
        let Some(scope) = self
            .content_view(view_index)
            .and_then(|cv| cv.pane_script_scope(pane_id))
        else {
            return highlights;
        };
        let mut names: Vec<String> = self
            .hooks_for_scope(&scope)
            .into_iter()
            .filter(|(_, hook)| ScriptHook::parse(hook) == Some(ScriptHook::Load))
            .map(|(name, _)| name)
            .collect();
        if names.is_empty() {
            return highlights;
        }
        names.sort();

        self.hook_runs_in_flight.insert((view_index, pane_id));
        for name in names {
            self.run_load_hook_script(view_index, pane_id, &name, items, &mut highlights);
        }
        self.hook_runs_in_flight.remove(&(view_index, pane_id));
        highlights
    }

    /// One load-hook run: build the payload from the rows as they stand,
    /// execute, apply what came back.
    fn run_load_hook_script(
        &mut self,
        view_index: usize,
        pane_id: PaneId,
        name: &str,
        items: &mut [NodeSummary],
        highlights: &mut ScriptHighlights,
    ) {
        let Some(ctx) = self.build_load_hook_ctx(view_index, pane_id, items) else {
            return;
        };
        let path = ctx.scripts_dir().join(name);
        if !path.exists() {
            return;
        }
        if let Some(reason) = hook_mode_rejection(&path.to_string_lossy()) {
            self.notify_error(format!("Hook script '{name}' not run: {reason}"));
            return;
        }
        let Some(answer) = self.run_script_for_output(&ctx, &path) else {
            return;
        };
        // Commands would have to be executed into the middle of the load that
        // is running this script — refused rather than quietly dropped, so a
        // script bound to the wrong hook says so instead of half-working.
        if answer
            .get("commands")
            .and_then(|c| c.as_array())
            .is_some_and(|c| !c.is_empty())
        {
            self.notify_error(format!(
                "Load hook '{name}' returned commands — ignored (only `cells` runs before the rows land)"
            ));
        }
        match apply_cell_patch(items, &answer) {
            Ok(patch) => {
                if patch.unknown > 0 {
                    self.notify_error(format!(
                        "Load hook '{name}': {} row id(s) not in this load — ignored",
                        patch.unknown
                    ));
                }
            }
            Err(e) => self.notify_error(format!("Load hook '{name}': {e}")),
        }
        self.apply_script_order(view_index, pane_id, name, items, &answer);
        self.collect_script_highlights(view_index, name, items, &answer, highlights);
    }

    /// Read the answer's `order` and put the rows in it — unless the user
    /// has sorted this pane.
    ///
    /// A script order and a column sort are the same decision made twice, and
    /// the hook runs on *every* load, so a script that always won would make
    /// `c s` look broken on its level. The user's sort therefore wins, and the
    /// script order is what an unsorted pane falls back to. Said out loud
    /// rather than dropped quietly: a script whose order goes unused should
    /// read as refused, not as ineffective.
    fn apply_script_order(
        &mut self,
        view_index: usize,
        pane_id: PaneId,
        name: &str,
        items: &mut [NodeSummary],
        answer: &serde_json::Value,
    ) {
        if answer.get("order").is_none() {
            return;
        }
        let user_sorted = self
            .content_view(view_index)
            .and_then(|cv| cv.find_pane(pane_id))
            .is_some_and(|pane| !pane.current_sort().is_empty());
        if user_sorted {
            self.notify(format!(
                "Load hook '{name}': `order` ignored — this view is sorted by column (clear the sort to let the script order)"
            ));
            return;
        }
        match apply_row_order(items, answer) {
            Ok(out) => {
                if out.unknown > 0 {
                    self.notify_error(format!(
                        "Load hook '{name}': {} ordered row id(s) not in this load — ignored",
                        out.unknown
                    ));
                }
                if out.duplicate > 0 {
                    self.notify_error(format!(
                        "Load hook '{name}': {} row id(s) named twice in `order` — first mention kept",
                        out.duplicate
                    ));
                }
            }
            Err(e) => self.notify_error(format!("Load hook '{name}': {e}")),
        }
    }

    /// Read the answer's `highlights` and fold them into `into`.
    ///
    /// The styles are resolved here rather than at paint time: this is the
    /// last place that has the view file's `styles:` table in reach, and a
    /// name that resolves to nothing should be reported once per load, not
    /// swallowed once per frame.
    fn collect_script_highlights(
        &mut self,
        view_index: usize,
        name: &str,
        items: &[NodeSummary],
        answer: &serde_json::Value,
        into: &mut ScriptHighlights,
    ) {
        if answer.get("highlights").is_none() {
            return;
        }
        let Some(cv) = self.content_view(view_index) else {
            return;
        };
        let theme = Arc::clone(&cv.theme);
        let styles = cv.highlight_styles.clone();
        let resolver = StyleResolver::new(&styles, theme.styles(), &theme);

        match ScriptHighlights::parse(answer, &resolver) {
            Ok((parsed, warnings)) => {
                for w in warnings {
                    self.notify_error(format!("Load hook '{name}': {w}"));
                }
                let unknown = parsed.unknown_rows(items);
                if unknown > 0 {
                    self.notify_error(format!(
                        "Load hook '{name}': {unknown} highlighted row id(s) not in this load — ignored"
                    ));
                }
                into.merge(parsed);
            }
            Err(e) => self.notify_error(format!("Load hook '{name}': {e}")),
        }
    }

    /// Run every script bound to [`ScriptHook::Reload`] on the pane whose
    /// load just landed. `hook_depth` is the depth the finished load
    /// carried; see the module docs for how it stops a trigger loop.
    pub(super) fn fire_reload_hooks(&mut self, view_index: usize, pane_id: PaneId, hook_depth: u8) {
        if hook_depth >= HOOK_MAX_DEPTH {
            return;
        }
        // Re-entrancy guard. The synchronous runner cannot re-enter on its
        // own — it blocks the main loop — but a hook that ends in a load
        // for the *same* pane must not be able to stack up either.
        if self.hook_runs_in_flight.contains(&(view_index, pane_id)) {
            return;
        }
        let Some(scope) = self
            .content_view(view_index)
            .and_then(|cv| cv.pane_script_scope(pane_id))
        else {
            return;
        };
        let mut names: Vec<String> = self
            .hooks_for_scope(&scope)
            .into_iter()
            .filter(|(_, hook)| ScriptHook::parse(hook) == Some(ScriptHook::Reload))
            .map(|(name, _)| name)
            .collect();
        if names.is_empty() {
            return;
        }
        // Stable order: the menu lists scripts by name, so hooks run in the
        // order the user sees them.
        names.sort();

        self.hook_runs_in_flight.insert((view_index, pane_id));
        let previous_depth = self.load_hook_depth;
        // Everything spawned from here on — the script's own commands, the
        // reload `run_script` does after a batch/table run — is stamped one
        // level deeper and therefore fires no further hooks.
        self.load_hook_depth = hook_depth + 1;
        for name in names {
            self.run_hook_script(view_index, pane_id, &name);
        }
        self.load_hook_depth = previous_depth;
        self.hook_runs_in_flight.remove(&(view_index, pane_id));
    }

    /// One hook run: rebuild the payload context exactly as the manual run
    /// would ([`App::run_script_shortcut`] does the same for a chord) and
    /// execute the script.
    fn run_hook_script(&mut self, view_index: usize, pane_id: PaneId, name: &str) {
        use crate::config::view_config::ScriptScope;
        let Some((scope, default_field)) = self
            .content_view(view_index)
            .and_then(|cv| cv.pane_script_action(pane_id))
        else {
            return;
        };
        let ctx = match scope {
            ScriptScope::Node => self.build_content_node_ctx(view_index, pane_id),
            ScriptScope::FilteredSet => self.build_content_batch_ctx(view_index, pane_id),
            ScriptScope::Table => self.build_content_table_ctx(view_index, pane_id, default_field),
        };
        let Some(ctx) = ctx else {
            return;
        };
        let path = ctx.scripts_dir().join(name);
        // A hook whose script was deleted behind the app's back is not an
        // error the user needs on every single load — the menu shows the
        // binding, and re-binding clears it.
        if !path.exists() {
            return;
        }
        if let Some(reason) = hook_mode_rejection(&path.to_string_lossy()) {
            self.notify_error(format!("Hook script '{name}' not run: {reason}"));
            return;
        }
        // A hook fires with no cursor intent behind it, so the script's own
        // `# scope:` matters most here: a reload hook that maintains a column
        // wants the rows that just landed, not whatever row the cursor sits on.
        let ctx = self.apply_script_scope_header(ctx, &path);
        let request = self.run_script(&ctx, &path.to_string_lossy());
        // The modes a hook is allowed to use never open an editor; a
        // request here would mean the header changed after binding.
        if !matches!(request, EditorRequest::None) {
            self.notify_error(format!(
                "Hook script '{name}' asked for an editor — ignored (hooks run unattended)"
            ));
        }
    }
}

/// What one [`apply_cell_patch`] run did.
#[derive(Debug, Default, PartialEq, Eq)]
struct PatchOutcome {
    /// Cells written into a row.
    applied: usize,
    /// Row ids the answer named that this load does not contain.
    unknown: usize,
}

/// Apply a load hook's answer to the rows.
///
/// Shape: `{"cells": {"<row id>": {"<column key>": <value>}}}`. A missing
/// `cells` key is not an error — a script that found nothing to change says
/// so by leaving it out (or by writing `{}`).
///
/// Values may be strings, numbers or booleans; all three land as the text a
/// cell holds, so a script can answer `4.0` for a `kind: number` column
/// without quoting it. `null` clears the cell.
///
/// A column the row does not carry yet is **added**, not skipped: a computed
/// column has no value on a row nothing was ever stored for, and that is
/// precisely the row a load hook exists to fill. Added fields are
/// non-editable — the next load recomputes them, so a typed-in value would
/// vanish without explanation.
fn apply_cell_patch(
    items: &mut [NodeSummary],
    answer: &serde_json::Value,
) -> Result<PatchOutcome, String> {
    let Some(cells) = answer.get("cells") else {
        return Ok(PatchOutcome::default());
    };
    let cells = cells
        .as_object()
        .ok_or_else(|| "`cells` must be an object keyed by row id".to_string())?;
    // Owns its keys: the rows are patched through the same slice the index
    // was built from, so a borrow of their ids could not survive it.
    let index: HashMap<String, usize> = items
        .iter()
        .enumerate()
        .map(|(i, item)| (item.id.clone(), i))
        .collect();

    let mut out = PatchOutcome::default();
    for (row_id, columns) in cells {
        let Some(&i) = index.get(row_id.as_str()) else {
            out.unknown += 1;
            continue;
        };
        let columns = columns
            .as_object()
            .ok_or_else(|| format!("cells['{row_id}'] must be an object keyed by column"))?;
        for (key, value) in columns {
            let text = match value {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                serde_json::Value::Null => String::new(),
                _ => {
                    return Err(format!(
                        "cells['{row_id}']['{key}'] must be a string, number, bool or null"
                    ));
                }
            };
            let fields = &mut items[i].metadata.fields;
            match fields.iter_mut().find(|f| f.key == *key) {
                Some(field) => field.value = text,
                None => fields.push(MetadataField {
                    key: key.clone(),
                    value: text,
                    display_label: key.clone(),
                    editable: false,
                    allowed_values: None,
                }),
            }
            out.applied += 1;
        }
    }
    Ok(out)
}

/// What one [`apply_row_order`] run did.
#[derive(Debug, Default, PartialEq, Eq)]
struct OrderOutcome {
    /// Rows the answer named and this load contains — the ones that moved to
    /// the front, in the order they were named.
    placed: usize,
    /// Row ids the answer named that this load does not contain.
    unknown: usize,
    /// Ids named more than once. The first mention decides; the repeats are
    /// counted so a script that built its list from two sources hears about
    /// it.
    duplicate: usize,
}

/// Put the rows in the order a load hook's answer asks for.
///
/// Shape: `{"order": ["<row id>", …]}`. Sparse like `cells`: the named rows
/// come first, in exactly that sequence, and everything the answer did not
/// mention keeps its relative order behind them. A script that only wants to
/// pull three rows to the top therefore names three ids, not the whole table.
///
/// The sort is stable, which is what makes the tail well-defined — the rows
/// the script had no opinion about stay in the order the adapter delivered
/// them.
fn apply_row_order(
    items: &mut [NodeSummary],
    answer: &serde_json::Value,
) -> Result<OrderOutcome, String> {
    let Some(order) = answer.get("order") else {
        return Ok(OrderOutcome::default());
    };
    let order = order
        .as_array()
        .ok_or_else(|| "`order` must be an array of row ids".to_string())?;

    let mut rank: HashMap<&str, usize> = HashMap::new();
    let mut out = OrderOutcome::default();
    for entry in order {
        let id = entry
            .as_str()
            .ok_or_else(|| "`order` entries must be row id strings".to_string())?;
        if rank.contains_key(id) {
            out.duplicate += 1;
            continue;
        }
        let next = rank.len();
        rank.insert(id, next);
    }
    let present: std::collections::HashSet<&str> =
        items.iter().map(|item| item.id.as_str()).collect();
    out.unknown = rank.keys().filter(|id| !present.contains(*id)).count();
    out.placed = rank.len() - out.unknown;

    // Unnamed rows sort behind every named one and, the sort being stable,
    // among themselves as they arrived.
    items.sort_by_key(|item| rank.get(item.id.as_str()).copied().unwrap_or(usize::MAX));
    Ok(out)
}

/// Why `path` cannot run as a hook, or `None` when it can. Hooks run
/// unattended while a load lands, so only the two modes that need neither
/// the terminal nor an editor are allowed: `background` and `commands`.
fn hook_mode_rejection(path: &str) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let mode = parse_script_mode(&content);
    if mode.is_interactive() {
        return Some("interactive scripts cannot run as a hook".to_string());
    }
    if mode.captures_output() {
        return Some("capture-mode scripts cannot run as a hook".to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_names_round_trip() {
        for hook in ScriptHook::ALL {
            assert_eq!(ScriptHook::parse(hook.as_str()), Some(*hook));
        }
    }

    /// A row written by a newer version must be ignored, not crash the
    /// load path.
    #[test]
    fn unknown_hook_names_are_ignored() {
        assert_eq!(ScriptHook::parse("on_startup"), None);
        assert_eq!(ScriptHook::parse(""), None);
    }

    fn row(id: &str, fields: &[(&str, &str)]) -> NodeSummary {
        NodeSummary {
            id: id.to_string(),
            label: format!("label of {id}"),
            node_type: not_yet_done_content::NodeType {
                type_id: "mock:issue".to_string(),
                mime_type: "text/plain".to_string(),
                syntax: None,
                file_extension: ".txt".to_string(),
                display_name: "Issue".to_string(),
            },
            metadata: not_yet_done_content::Metadata {
                fields: fields
                    .iter()
                    .map(|(k, v)| MetadataField {
                        key: k.to_string(),
                        value: v.to_string(),
                        display_label: k.to_string(),
                        editable: true,
                        allowed_values: None,
                    })
                    .collect(),
            },
            has_children: None,
        }
    }

    fn field(item: &NodeSummary, key: &str) -> Option<String> {
        item.metadata
            .fields
            .iter()
            .find(|f| f.key == key)
            .map(|f| f.value.clone())
    }

    #[test]
    fn patch_overwrites_an_existing_cell() {
        let mut items = vec![row("MOCK-1", &[("days", "1.00")])];
        let answer = serde_json::json!({"cells": {"MOCK-1": {"days": "2.50"}}});
        let out = apply_cell_patch(&mut items, &answer).unwrap();
        assert_eq!(out.applied, 1);
        assert_eq!(field(&items[0], "days").as_deref(), Some("2.50"));
    }

    /// The row a computed column exists for is exactly the row that has no
    /// stored value yet — patching it must add the field, not skip it.
    #[test]
    fn patch_adds_a_column_the_row_does_not_carry() {
        let mut items = vec![row("MOCK-1", &[])];
        let answer = serde_json::json!({"cells": {"MOCK-1": {"days": "0.25"}}});
        apply_cell_patch(&mut items, &answer).unwrap();
        assert_eq!(field(&items[0], "days").as_deref(), Some("0.25"));
        assert!(!items[0].metadata.fields[0].editable);
    }

    #[test]
    fn patch_accepts_numbers_bools_and_null() {
        let mut items = vec![row("MOCK-1", &[("n", "x"), ("b", "x"), ("z", "x")])];
        let answer = serde_json::json!({"cells": {"MOCK-1": {"n": 4.5, "b": true, "z": serde_json::Value::Null}}});
        apply_cell_patch(&mut items, &answer).unwrap();
        assert_eq!(field(&items[0], "n").as_deref(), Some("4.5"));
        assert_eq!(field(&items[0], "b").as_deref(), Some("true"));
        assert_eq!(field(&items[0], "z").as_deref(), Some(""));
    }

    #[test]
    fn patch_is_sparse_and_leaves_other_rows_alone() {
        let mut items = vec![
            row("MOCK-1", &[("days", "1.00")]),
            row("MOCK-2", &[("days", "3.00")]),
        ];
        let answer = serde_json::json!({"cells": {"MOCK-2": {"days": "4.00"}}});
        apply_cell_patch(&mut items, &answer).unwrap();
        assert_eq!(field(&items[0], "days").as_deref(), Some("1.00"));
        assert_eq!(field(&items[1], "days").as_deref(), Some("4.00"));
    }

    /// A script that found nothing to change answers without `cells` — that
    /// is a result, not a malformed answer.
    #[test]
    fn empty_answer_is_not_an_error() {
        let mut items = vec![row("MOCK-1", &[("days", "1.00")])];
        for answer in [serde_json::json!({}), serde_json::json!({"commands": []})] {
            let out = apply_cell_patch(&mut items, &answer).unwrap();
            assert_eq!(out, PatchOutcome::default());
        }
        assert_eq!(field(&items[0], "days").as_deref(), Some("1.00"));
    }

    #[test]
    fn rows_outside_this_load_are_counted_not_applied() {
        let mut items = vec![row("MOCK-1", &[("days", "1.00")])];
        let answer = serde_json::json!({"cells": {"GONE-9": {"days": "9.00"}}});
        let out = apply_cell_patch(&mut items, &answer).unwrap();
        assert_eq!(out.unknown, 1);
        assert_eq!(out.applied, 0);
    }

    #[test]
    fn malformed_shapes_are_rejected() {
        let mut items = vec![row("MOCK-1", &[])];
        for answer in [
            serde_json::json!({"cells": ["MOCK-1"]}),
            serde_json::json!({"cells": {"MOCK-1": "days"}}),
            serde_json::json!({"cells": {"MOCK-1": {"days": ["1.0"]}}}),
        ] {
            assert!(apply_cell_patch(&mut items, &answer).is_err());
        }
    }

    fn ids(items: &[NodeSummary]) -> Vec<&str> {
        items.iter().map(|i| i.id.as_str()).collect()
    }

    #[test]
    fn order_puts_the_named_rows_in_front_in_that_sequence() {
        let mut items = vec![row("A", &[]), row("B", &[]), row("C", &[])];
        let answer = serde_json::json!({"order": ["C", "A", "B"]});
        let out = apply_row_order(&mut items, &answer).unwrap();
        assert_eq!(out.placed, 3);
        assert_eq!(ids(&items), ["C", "A", "B"]);
    }

    /// The point of the sparse shape: name the three rows that matter and
    /// leave the rest of the table where the adapter put it.
    #[test]
    fn unnamed_rows_keep_their_relative_order_behind() {
        let mut items = vec![row("A", &[]), row("B", &[]), row("C", &[]), row("D", &[])];
        let answer = serde_json::json!({"order": ["C"]});
        let out = apply_row_order(&mut items, &answer).unwrap();
        assert_eq!(out.placed, 1);
        assert_eq!(ids(&items), ["C", "A", "B", "D"]);
    }

    #[test]
    fn ordered_ids_outside_this_load_are_counted_not_applied() {
        let mut items = vec![row("A", &[]), row("B", &[])];
        let answer = serde_json::json!({"order": ["GONE", "B"]});
        let out = apply_row_order(&mut items, &answer).unwrap();
        assert_eq!(out.unknown, 1);
        assert_eq!(out.placed, 1);
        assert_eq!(ids(&items), ["B", "A"]);
    }

    /// A list stitched together from two sources may name a row twice — the
    /// first mention is its place, the repeat is only counted.
    #[test]
    fn a_repeated_id_keeps_its_first_place() {
        let mut items = vec![row("A", &[]), row("B", &[]), row("C", &[])];
        let answer = serde_json::json!({"order": ["C", "A", "C"]});
        let out = apply_row_order(&mut items, &answer).unwrap();
        assert_eq!(out.duplicate, 1);
        assert_eq!(ids(&items), ["C", "A", "B"]);
    }

    #[test]
    fn an_answer_without_order_leaves_the_rows_alone() {
        let mut items = vec![row("A", &[]), row("B", &[])];
        for answer in [serde_json::json!({}), serde_json::json!({"cells": {}})] {
            let out = apply_row_order(&mut items, &answer).unwrap();
            assert_eq!(out, OrderOutcome::default());
        }
        assert_eq!(ids(&items), ["A", "B"]);
    }

    #[test]
    fn malformed_order_shapes_are_rejected() {
        let mut items = vec![row("A", &[])];
        for answer in [
            serde_json::json!({"order": {"A": 0}}),
            serde_json::json!({"order": [1, 2]}),
        ] {
            assert!(apply_row_order(&mut items, &answer).is_err());
        }
    }

    #[test]
    fn only_unattended_modes_may_hook() {
        let dir = std::env::temp_dir().join("nyd-hook-mode-test");
        std::fs::create_dir_all(&dir).unwrap();
        let cases = [
            ("bg.py", "# mode: background\n", false),
            ("cmd.py", "# mode: commands\n", false),
            ("plain.py", "print(1)\n", false), // defaults to background
            ("cap.py", "# mode: capture\n", true),
            ("int.py", "# mode: interactive\n", true),
            ("intcmd.py", "# mode: interactive+commands\n", true),
        ];
        for (file, body, rejected) in cases {
            let path = dir.join(file);
            std::fs::write(&path, body).unwrap();
            assert_eq!(
                hook_mode_rejection(&path.to_string_lossy()).is_some(),
                rejected,
                "{file}"
            );
        }
    }
}
