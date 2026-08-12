//! Generic option-menu orchestration — open, async option load, key dispatch,
//! and the create/rename/delete mutations.
//!
//! Drives a `type: option_menu` action end to end without the adapter knowing
//! the host: the menu config ([`OptionMenuConfig`]) names an options *source*,
//! a node *marker* field, a *toggle* action, and (optionally) *create*,
//! *rename* and *delete* actions. The App fetches the options via
//! [`ContentAdapter::list_values`], reads the node's currently-selected values
//! from its marker metadata field, opens the popup with those pre-marked, and:
//!
//! - on each Enter dispatches the toggle action with the chosen value in
//!   `ActionContext.value`;
//! - on a create/rename submit dispatches the configured action with the typed
//!   text in `ActionContext.text` (rename also carries the focused id in
//!   `ActionContext.value`), then re-fetches the option list so the menu and
//!   the pane reflect the change;
//! - on a confirmed delete dispatches the delete action with the focused id.
//!
//! The adapter decides assign-vs-unassign / accepts-or-rejects and returns an
//! [`ActionDispatch`] — nonsense values come back as `ActionDispatch::Error`,
//! which surfaces as a non-fatal notification while the menu stays open.
//!
//! [`ContentAdapter::list_values`]: not_yet_done_content::ContentAdapter::list_values
//! [`ActionDispatch`]: not_yet_done_content::ActionDispatch

use std::sync::Arc;

use crate::app::App;
use crate::app::ContentSlot;
use crate::app::LoadMsg;
use crate::components::option_menu::{
    OptionMenuCaps, OptionMenuEntry, OptionMenuMessage, OptionMenuVerb,
};
use crate::config::view_config::OptionMenuConfig;
use crate::views::content_view::PaneId;

/// Dispatch context for the currently open option menu. Set when the menu
/// opens; consulted on every toggle/mutation and when the popup closes.
#[derive(Debug, Clone)]
pub struct OptionMenuTarget {
    pub view_index: usize,
    pub pane_id: PaneId,
    /// Node the toggle action acts on (the selected row at open time).
    pub node_id: String,
    /// `list_values` source key (re-fetched after a create/rename/delete).
    pub source: String,
    /// Hidden node metadata field holding the selected option ids.
    pub marker: String,
    /// Adapter action invoked on toggle (e.g. `toggle-tag`).
    pub toggle_action: String,
    /// Adapter action invoked on create (text → `ActionContext.text`), if wired.
    pub create_action: Option<String>,
    /// Adapter action invoked on rename (focused id → `value`, name → `text`).
    pub rename_action: Option<String>,
    /// Adapter action invoked on delete (focused id → `value`), if wired.
    pub delete_action: Option<String>,
    /// Popup title (resolved from config or the action name).
    pub title: String,
}

impl App {
    /// Open the option menu from a `type: option_menu` action. Resolves the
    /// focused pane's selected node, pins the menu to it, and kicks off the
    /// async option load. The popup opens once the options arrive
    /// ([`Self::open_option_menu_popup`]).
    pub fn open_option_menu_for_content(
        &mut self,
        view_index: usize,
        pane_id: PaneId,
        config: OptionMenuConfig,
    ) {
        let node_id = {
            let Some(ContentSlot::Working(cv)) = self.content_views.get(view_index) else {
                self.notify("Content view is unavailable".to_string());
                return;
            };
            let Some(pane) = cv.find_pane(pane_id) else {
                self.notify("Pane not found".to_string());
                return;
            };
            let Some(item) = pane.selected_item() else {
                self.notify("No row selected".to_string());
                return;
            };
            item.id.clone()
        };

        self.option_menu_target = Some(OptionMenuTarget {
            view_index,
            pane_id,
            node_id: node_id.clone(),
            source: config.source.clone(),
            marker: config.marker.clone(),
            toggle_action: config.toggle.clone(),
            create_action: config.create.clone(),
            rename_action: config.rename.clone(),
            delete_action: config.delete.clone(),
            title: config
                .title
                .clone()
                .unwrap_or_else(|| "Options".to_string()),
        });

        self.spawn_option_menu_load(view_index, pane_id, node_id, config.source, config.marker);
    }

    /// Fetch the selectable options (`list_values(source)`) and the node's
    /// currently-selected values (its `marker` metadata field) off-thread,
    /// then hand them to the main loop via [`LoadMsg::OptionMenuItems`].
    fn spawn_option_menu_load(
        &self,
        view_index: usize,
        pane_id: PaneId,
        node_id: String,
        source: String,
        marker: String,
    ) {
        let adapter = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone);
        let Some(adapter) = adapter else {
            return;
        };
        let tx = self.load_tx.clone();
        tokio::spawn(async move {
            let outcome: not_yet_done_content::Result<(
                Vec<not_yet_done_content::ValueOption>,
                Vec<String>,
            )> = async {
                let items = adapter.list_values(&source).await?;
                let node = adapter.get_by_id(&node_id).await?;
                let selected = marker_values(node.as_ref(), &marker);
                Ok((items, selected))
            }
            .await;

            let msg = match outcome {
                Ok((items, selected_values)) => LoadMsg::OptionMenuItems {
                    view_index,
                    pane_id,
                    items,
                    selected_values,
                    error: None,
                    notice: None,
                    reload_pane: false,
                },
                Err(e) => LoadMsg::OptionMenuItems {
                    view_index,
                    pane_id,
                    items: Vec::new(),
                    selected_values: Vec::new(),
                    error: Some(format!("Failed to load options: {e}")),
                    notice: None,
                    reload_pane: false,
                },
            };
            let _ = tx.send(msg);
        });
    }

    /// Invoke a create/rename/delete option-store mutation, then re-fetch the
    /// option list so the menu rebuilds and the pane reloads. A single async
    /// task chains mutation → refetch, so the refresh never races the write.
    /// An adapter rejection (`ActionDispatch::Error`) surfaces as a non-fatal
    /// notice and leaves the (re-fetched) menu open.
    fn spawn_option_menu_mutation(
        &self,
        view_index: usize,
        pane_id: PaneId,
        node_id: String,
        action: String,
        value: Option<String>,
        text: Option<String>,
        source: String,
        marker: String,
    ) {
        let adapter = self
            .content_view(view_index)
            .and_then(|cv| cv.adapter.as_ref())
            .map(Arc::clone);
        let Some(adapter) = adapter else {
            return;
        };
        let tx = self.load_tx.clone();
        tokio::spawn(async move {
            // 1. Run the mutation.
            let mutate: not_yet_done_content::Result<not_yet_done_content::ActionDispatch> =
                async {
                    let node = adapter.get_by_id(&node_id).await?;
                    let ctx = not_yet_done_content::ActionContext {
                        value,
                        text,
                        ..Default::default()
                    };
                    node.invoke_action(&action, &ctx).await
                }
                .await;
            let notice = match mutate {
                Ok(not_yet_done_content::ActionDispatch::Error(e)) => Some(e),
                Ok(_) => None,
                Err(e) => Some(format!("{e}")),
            };
            let reload_pane = notice.is_none();

            // 2. Re-fetch the option list + the node's current selection.
            let refetch: not_yet_done_content::Result<(
                Vec<not_yet_done_content::ValueOption>,
                Vec<String>,
            )> = async {
                let items = adapter.list_values(&source).await?;
                let node = adapter.get_by_id(&node_id).await?;
                let selected = marker_values(node.as_ref(), &marker);
                Ok((items, selected))
            }
            .await;

            let msg = match refetch {
                Ok((items, selected_values)) => LoadMsg::OptionMenuItems {
                    view_index,
                    pane_id,
                    items,
                    selected_values,
                    error: None,
                    notice,
                    reload_pane,
                },
                Err(e) => LoadMsg::OptionMenuItems {
                    view_index,
                    pane_id,
                    items: Vec::new(),
                    selected_values: Vec::new(),
                    error: Some(format!("Failed to refresh options: {e}")),
                    notice,
                    reload_pane,
                },
            };
            let _ = tx.send(msg);
        });
    }

    /// Build the popup entries from the loaded options + current selection and
    /// open (or rebuild) the menu. `notice` is a non-fatal message shown while
    /// the menu stays open (e.g. a rejected mutation); `error` is a fatal load
    /// failure that closes the menu. `reload_pane` reloads the pane after a
    /// mutation changed the underlying data.
    #[allow(clippy::too_many_arguments)]
    pub fn open_option_menu_popup(
        &mut self,
        view_index: usize,
        pane_id: PaneId,
        items: Vec<not_yet_done_content::ValueOption>,
        selected_values: Vec<String>,
        error: Option<String>,
        notice: Option<String>,
        reload_pane: bool,
    ) {
        // A mutation changed the underlying data — reload the pane regardless
        // of the menu's open state.
        if reload_pane {
            self.reload_content_pane_current_level(view_index, pane_id);
        }
        if let Some(n) = notice {
            self.notify(n);
        }
        if let Some(err) = error {
            self.notify_error(err);
            self.option_menu_target = None;
            return;
        }
        // Stale guard: a newer open (or a tab switch) may have superseded the
        // target this load was spawned for.
        let (title, caps) = match &self.option_menu_target {
            Some(t) if t.view_index == view_index && t.pane_id == pane_id => (
                t.title.clone(),
                OptionMenuCaps {
                    create: t.create_action.is_some(),
                    rename: t.rename_action.is_some(),
                    delete: t.delete_action.is_some(),
                },
            ),
            _ => return,
        };

        let entries: Vec<OptionMenuEntry> = items
            .into_iter()
            .map(|opt| OptionMenuEntry {
                assigned: selected_values.contains(&opt.value),
                value: opt.value,
                label: opt.label,
            })
            .collect();

        self.option_menu
            .open(title, &entries, caps, &self.keybindings.tag_menu);
    }

    /// Dispatch a key while the option menu is open. Enter toggles the focused
    /// option; create/rename submit a typed name; delete confirms then removes;
    /// Esc closes the menu.
    pub fn handle_option_menu_key(&mut self, key: &str) {
        let msg = self.option_menu.handle_key(key, &self.keybindings.tag_menu);
        match msg {
            OptionMenuMessage::Unhandled | OptionMenuMessage::Handled => {}
            OptionMenuMessage::Closed => {
                self.option_menu_target = None;
            }
            OptionMenuMessage::Toggle { value, .. } => {
                let Some(target) = self.option_menu_target.clone() else {
                    return;
                };
                // The component already flipped the `★` marker; fire the
                // adapter toggle and let its `Reload` refresh the pane in the
                // background. The menu stays open for further toggles.
                self.spawn_invoke_node_action(
                    target.view_index,
                    target.pane_id,
                    target.node_id,
                    target.toggle_action,
                    false,
                    Some(value),
                );
            }
            OptionMenuMessage::Submit { verb, value, text } => {
                let Some(target) = self.option_menu_target.clone() else {
                    return;
                };
                let action = match verb {
                    OptionMenuVerb::Create => target.create_action.clone(),
                    OptionMenuVerb::Rename => target.rename_action.clone(),
                };
                let Some(action) = action else {
                    return;
                };
                self.spawn_option_menu_mutation(
                    target.view_index,
                    target.pane_id,
                    target.node_id,
                    action,
                    value,
                    Some(text),
                    target.source,
                    target.marker,
                );
            }
            OptionMenuMessage::Delete { value, .. } => {
                let Some(target) = self.option_menu_target.clone() else {
                    return;
                };
                let Some(action) = target.delete_action.clone() else {
                    return;
                };
                self.spawn_option_menu_mutation(
                    target.view_index,
                    target.pane_id,
                    target.node_id,
                    action,
                    Some(value),
                    None,
                    target.source,
                    target.marker,
                );
            }
        }
    }
}

/// Read a node's `marker` metadata field as a list of stable ids. The field
/// holds the comma-separated ids currently set on the node (e.g. `tag_ids`);
/// absent or empty → nothing selected.
fn marker_values(node: &dyn not_yet_done_content::Node, marker: &str) -> Vec<String> {
    node.metadata()
        .fields
        .iter()
        .find(|f| f.key == marker)
        .map(|f| {
            f.value
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}
