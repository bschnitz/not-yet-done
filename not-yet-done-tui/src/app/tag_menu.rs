//! `:tag` menu lifecycle — open, render-state, key dispatch, and
//! transitions into the [`TagFormSession`] editor flow.
//!
//! Mirrors the saved-query-menu wiring in `views/tasks_view.rs` but
//! lives at App level: the menu is global, callable from any tab.

use uuid::Uuid;

use crate::app::App;
use crate::app::ContentSlot;
use crate::app::EditorRequest;
use crate::components::tag_menu::{TagMenuEntry, TagMenuMessage};
use crate::edit_session::TagFormSession;
use crate::views::content_view::PaneId;
use not_yet_done_task_core::repository::ResolvedTag;
use not_yet_done_task_core::service::TagItem;

/// Where a tag-menu opened from a content/adapter tab assigns tags and
/// which pane to refresh once an assignment changes. Set by
/// [`App::open_tag_menu_for_content`]; consulted by `tag_assign_target`
/// and the toggle/create/delete paths so the generic tag menu works on a
/// ContentView the same way it does on the native Tasks tab.
#[derive(Debug, Clone, Copy)]
pub struct ContentTagTarget {
    /// Task the menu assigns/unassigns/creates tags against.
    pub task_id: Uuid,
    /// Originating content view + pane, reloaded after a change so the
    /// `tag_symbols` / `tag_names` columns reflect the new assignment.
    pub view_index: usize,
    pub pane_id: PaneId,
}

impl App {
    /// Open the tag menu from the cmdline (`:tag`). Clears any content-tab
    /// target, so the menu manages tags globally (create / edit / delete)
    /// without a node to assign against. Assignment needs a selected node
    /// and is reached via a content tab's `T` key
    /// ([`Self::open_tag_menu_for_content`]).
    pub fn open_tag_menu(&mut self) {
        self.content_tag_target = None;
        self.build_and_open_tag_menu();
    }

    /// Open the tag menu from a content/adapter tab (a `type: tag` action,
    /// e.g. the Tasks `T` key). Resolves the focused pane's selected
    /// node to a task id and pins the menu to it; the menu then assigns,
    /// creates and deletes tags exactly as on the native tab, refreshing
    /// this pane afterwards. Tags are a task concept, so a non-task node
    /// (id that isn't a task UUID) is rejected with a notice.
    pub fn open_tag_menu_for_content(&mut self, view_index: usize, pane_id: PaneId) {
        let node_id = {
            let Some(ContentSlot::Working(cv)) = self.content_views.get(view_index) else {
                self.notify("Content view is unavailable".to_string());
                return;
            };
            let Some(pane) = cv.find_pane(pane_id) else {
                self.notify("Pane not found".to_string());
                return;
            };
            // Tree-aware: the selected summary lives on the tree entry, not
            // in `pane.items` (depth-0 only).
            let Some(item) = pane.selected_item() else {
                self.notify("No row selected".to_string());
                return;
            };
            item.id.clone()
        };
        let Ok(task_id) = Uuid::parse_str(&node_id) else {
            self.notify("Tags can only be assigned to tasks".to_string());
            return;
        };
        self.content_tag_target = Some(ContentTagTarget {
            task_id,
            view_index,
            pane_id,
        });
        self.build_and_open_tag_menu();
    }

    /// Build a menu entry list from the current tag database and open
    /// the popup. Global tags first, then project tags (each suffixed
    /// with their project name).
    fn build_and_open_tag_menu(&mut self) {
        let svc = self.tag_service.clone();
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move { svc.list_all().await })
        });
        let items = match result {
            Ok(v) => v,
            Err(e) => {
                self.notify_error(format!("Failed to load tags: {e}"));
                return;
            }
        };

        let mut entries: Vec<TagMenuEntry> = items
            .into_iter()
            .map(|item| match item {
                TagItem::Global(t) => TagMenuEntry {
                    id: format!("global-tag:{}", t.id),
                    label: format_menu_label(&t.name, t.symbol.as_deref(), None),
                },
                TagItem::Project { tag, project_name } => TagMenuEntry {
                    id: format!("project-tag:{}", tag.id),
                    label: format_menu_label(&tag.name, tag.symbol.as_deref(), Some(&project_name)),
                },
            })
            .collect();
        entries.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));

        self.tag_menu.open(&entries, &self.keybindings.tag_menu);
    }

    /// Dispatch a key while the tag menu is open. Returns an
    /// [`EditorRequest`] when a menu action transitions into the
    /// external editor (Create / Edit).
    pub fn handle_tag_menu_key(&mut self, key: &str) -> EditorRequest {
        let msg = self.tag_menu.handle_key(key, &self.keybindings.tag_menu);
        match msg {
            TagMenuMessage::Unhandled | TagMenuMessage::Handled => EditorRequest::None,
            TagMenuMessage::Closed => EditorRequest::None,
            TagMenuMessage::ToggleAssign { id, label } => {
                self.toggle_tag_assignment(&id, &label);
                EditorRequest::None
            }
            TagMenuMessage::Delete { id, label } => {
                self.delete_tag(&id, &label);
                EditorRequest::None
            }
            TagMenuMessage::CreateNew { name } => {
                let assign_to = self.tag_assign_target();
                let content_reload = self.content_tag_reload();
                let session = TagFormSession::create_with_name(
                    self.tag_service.clone(),
                    &name,
                    assign_to.map(|tid| (tid, self.task_service.clone())),
                    content_reload,
                );
                self.open_session(Box::new(session))
            }
            TagMenuMessage::EditExisting { id, label: _ } => {
                let svc = self.tag_service.clone();
                let content_reload = self.content_tag_reload();
                let result = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current()
                        .block_on(async move { TagFormSession::edit(svc, id, content_reload).await })
                });
                match result {
                    Ok(session) => self.open_session(Box::new(session)),
                    Err(e) => {
                        self.notify_error(format!("Cannot edit tag: {e}"));
                        EditorRequest::None
                    }
                }
            }
        }
    }

    /// Return the task id eligible for tag assignment, or `None` when
    /// there is no eligible target. The target is set by
    /// [`Self::open_tag_menu_for_content`] from the selected content node
    /// (e.g. the Tasks `T` key).
    fn tag_assign_target(&self) -> Option<Uuid> {
        self.content_tag_target.as_ref().map(|target| target.task_id)
    }

    /// The content pane to reload after a session-based tag commit
    /// (create/edit), or `None` when the menu was opened from the native
    /// Tasks tab. Threaded into [`TagFormSession`].
    fn content_tag_reload(&self) -> Option<(usize, PaneId)> {
        self.content_tag_target
            .map(|t| (t.view_index, t.pane_id))
    }

    fn toggle_tag_assignment(&mut self, tag_id: &str, label: &str) {
        let Some(task_id) = self.tag_assign_target() else {
            self.notify("Tag assignment needs a selected task on the Tasks tab".to_string());
            return;
        };

        let assigned = self.tag_currently_assigned(task_id, tag_id);

        let svc = self.task_service.clone();
        let id_str = tag_id.to_string();
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                if assigned {
                    svc.edit_task(task_id, None, None, None, None, Some(id_str)).await
                } else {
                    svc.edit_task(task_id, None, None, None, Some(id_str), None).await
                }
            })
        });

        match result {
            Ok(_) => {
                let action = if assigned { "Unassigned" } else { "Assigned" };
                self.notify(format!("{action} tag {label}"));
                self.refresh_after_tag_change();
            }
            Err(e) => self.notify_error(format!("Failed to toggle tag: {e}")),
        }
    }

    /// Is `tag_id` (`global-tag:`/`project-tag:` form) currently on
    /// `task_id`? The content view holds no tag-relationship cache, so
    /// fetch this one task's tags live.
    fn tag_currently_assigned(&self, task_id: Uuid, tag_id: &str) -> bool {
        let svc = self.task_service.clone();
        let map = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async move { svc.load_tags_for_tasks(&[task_id]).await })
        });
        map.ok()
            .and_then(|m| m.get(&task_id).map(|tags| tags.to_vec()))
            .map(|tags| tags.iter().any(|rt| tag_matches_id(rt, tag_id)))
            .unwrap_or(false)
    }

    /// Refresh the content pane that owns the assignment after a tag
    /// change, so its `tag_symbols` / `tag_names` columns re-render. When
    /// the menu was opened globally (`:tag`, no target) there is nothing
    /// to refresh.
    fn refresh_after_tag_change(&mut self) {
        if let Some(target) = self.content_tag_target {
            self.reload_content_pane_current_level(target.view_index, target.pane_id);
        }
    }

    fn delete_tag(&mut self, id: &str, label: &str) {
        let svc = self.tag_service.clone();
        let id_str = id.to_string();
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move { svc.delete(id_str).await })
        });
        match result {
            Ok(()) => {
                self.notify(format!("Deleted tag {label}"));
                // The tag is gone from every task it was on, so re-render
                // the rows (drops it from the tag columns).
                self.refresh_after_tag_change();
            }
            Err(e) => self.notify_error(format!("Failed to delete tag: {e}")),
        }
    }
}

/// Match a resolved tag against the menu's stable id form
/// (`global-tag:<uuid>` / `project-tag:<uuid>`).
fn tag_matches_id(rt: &ResolvedTag, tag_id: &str) -> bool {
    if let Some(uuid_str) = tag_id.strip_prefix("global-tag:") {
        return matches!(rt, ResolvedTag::Global(g) if g.id.to_string() == uuid_str);
    }
    if let Some(uuid_str) = tag_id.strip_prefix("project-tag:") {
        return matches!(rt, ResolvedTag::Project(p) if p.id.to_string() == uuid_str);
    }
    false
}

fn format_menu_label(name: &str, symbol: Option<&str>, project: Option<&str>) -> String {
    let mut out = String::new();
    if let Some(s) = symbol {
        out.push_str(s);
        out.push(' ');
    }
    out.push_str(name);
    if let Some(p) = project {
        out.push_str(&format!(" (project: {p})"));
    }
    out
}
