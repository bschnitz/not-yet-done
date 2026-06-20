//! Generic option-menu orchestration — open, async option load, key dispatch.
//!
//! Drives a `type: option_menu` action end to end without the adapter knowing
//! the host: the menu config ([`OptionMenuConfig`]) names an options *source*,
//! a node *marker* field, and a *toggle* action. The App fetches the options
//! via [`ContentAdapter::list_values`], reads the node's currently-selected
//! values from its marker metadata field, opens the popup with those pre-marked,
//! and on each Enter dispatches the toggle action with the chosen value in
//! `ActionContext.value`. The adapter decides assign-vs-unassign from the node's
//! own membership and returns an [`ActionDispatch`] — nonsense values come back
//! as `ActionDispatch::Error`.
//!
//! [`ContentAdapter::list_values`]: not_yet_done_content::ContentAdapter::list_values
//! [`ActionDispatch`]: not_yet_done_content::ActionDispatch

use std::sync::Arc;

use crate::app::App;
use crate::app::ContentSlot;
use crate::app::LoadMsg;
use crate::components::option_menu::{OptionMenuEntry, OptionMenuMessage};
use crate::config::view_config::OptionMenuConfig;
use crate::views::content_view::PaneId;

/// Dispatch context for the currently open option menu. Set when the menu
/// opens; consulted on every toggle and when the popup closes.
#[derive(Debug, Clone)]
pub struct OptionMenuTarget {
    pub view_index: usize,
    pub pane_id: PaneId,
    /// Node the toggle action acts on (the selected row at open time).
    pub node_id: String,
    /// Adapter action invoked on toggle (e.g. `toggle-tag`).
    pub toggle_action: String,
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
            toggle_action: config.toggle.clone(),
            title: config.title.clone().unwrap_or_else(|| "Options".to_string()),
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
                // The marker field holds the comma-separated stable ids
                // currently set on the node (e.g. `tag_ids`). Absent or empty
                // → nothing selected.
                let selected = node
                    .metadata()
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
                    .unwrap_or_default();
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
                },
                Err(e) => LoadMsg::OptionMenuItems {
                    view_index,
                    pane_id,
                    items: Vec::new(),
                    selected_values: Vec::new(),
                    error: Some(format!("Failed to load options: {e}")),
                },
            };
            let _ = tx.send(msg);
        });
    }

    /// Build the popup entries from the loaded options + current selection and
    /// open the menu. A load error is surfaced and the menu stays closed.
    pub fn open_option_menu_popup(
        &mut self,
        view_index: usize,
        pane_id: PaneId,
        items: Vec<not_yet_done_content::ValueOption>,
        selected_values: Vec<String>,
        error: Option<String>,
    ) {
        if let Some(err) = error {
            self.notify_error(err);
            self.option_menu_target = None;
            return;
        }
        // Stale guard: a newer open (or a tab switch) may have superseded the
        // target this load was spawned for.
        let title = match &self.option_menu_target {
            Some(t) if t.view_index == view_index && t.pane_id == pane_id => t.title.clone(),
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
            .open(title, &entries, &self.keybindings.tag_menu);
    }

    /// Dispatch a key while the option menu is open. Enter toggles the focused
    /// option (dispatched async with the value); Esc closes the menu.
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
        }
    }
}
