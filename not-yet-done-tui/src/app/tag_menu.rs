//! `:tag` menu lifecycle — open, render-state, key dispatch, and
//! transitions into the [`TagFormSession`] editor flow.
//!
//! Mirrors the saved-query-menu wiring in `views/tasks_view.rs` but
//! lives at App level: the menu is global, callable from any tab.

use uuid::Uuid;

use crate::app::App;
use crate::app::EditorRequest;
use crate::components::tag_menu::{TagMenuEntry, TagMenuMessage};
use crate::edit_session::TagFormSession;
use crate::tabs::Tab;
use not_yet_done_core::repository::ResolvedTag;
use not_yet_done_core::service::TagItem;

impl App {
    /// Build a menu entry list from the current tag database and open
    /// the popup. Global tags first, then project tags (each suffixed
    /// with their project name).
    pub fn open_tag_menu(&mut self) {
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
                let session = TagFormSession::create_with_name(
                    self.tag_service.clone(),
                    &name,
                    assign_to.map(|tid| (tid, self.task_service.clone())),
                );
                self.open_session(Box::new(session))
            }
            TagMenuMessage::EditExisting { id, label: _ } => {
                let svc = self.tag_service.clone();
                let result = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current()
                        .block_on(async move { TagFormSession::edit(svc, id).await })
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
    /// the user isn't on the Tasks tab or has no task selected.
    fn tag_assign_target(&self) -> Option<Uuid> {
        if self.active_tab != Tab::Tasks {
            return None;
        }
        self.tasks_view.selected_id()
    }

    fn toggle_tag_assignment(&mut self, tag_id: &str, label: &str) {
        let Some(task_id) = self.tag_assign_target() else {
            self.notify("Tag assignment needs a selected task on the Tasks tab".to_string());
            return;
        };

        let assigned = self
            .tasks_view
            .state
            .task_tags
            .get(&task_id)
            .map(|tags| tags.iter().any(|rt| tag_matches_id(rt, tag_id)))
            .unwrap_or(false);

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
                let ids: Vec<Uuid> =
                    self.tasks_view.state.task_rows.iter().map(|t| t.id).collect();
                self.spawn_load_task_tags(ids);
            }
            Err(e) => self.notify_error(format!("Failed to toggle tag: {e}")),
        }
    }

    fn delete_tag(&mut self, id: &str, label: &str) {
        let svc = self.tag_service.clone();
        let id_str = id.to_string();
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move { svc.delete(id_str).await })
        });
        match result {
            Ok(()) => self.notify(format!("Deleted tag {label}")),
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
