//! Edit session for the YAML-based tag-form (create + edit).
//!
//! Wraps the host-agnostic helpers in [`not_yet_done_task_core::service`]
//! (template, parse, normalize, error annotation) and dispatches to
//! [`TagService`] on commit. On validation/service failure we render
//! the error as a `# ERROR:` block at the top of the buffer and ask
//! the App to reopen the editor.
//!
//! Two modes:
//!   - `Create`: blank template, name+project come from the form.
//!   - `EditGlobal { id }` / `EditProject { id, project_name }`:
//!     pre-filled template; the `project:` field is ignored on save
//!     so a tag's scope never changes via the edit form.

use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use not_yet_done_task_core::repository::{TagStyle, TagStylePatch};
use not_yet_done_task_core::service::{
    annotate_error, edit_global_template, edit_project_template, new_tag_template,
    normalize, parse_draft, strip_error_block, TagService, TaskService,
};

use crate::views::content_view::PaneId;

use super::{CommitOutcome, EditSession, FollowUp, SessionScope};

#[derive(Debug, Clone)]
pub enum TagFormMode {
    Create,
    EditGlobal { id: String },
    EditProject { id: String },
}

pub struct TagFormSession {
    tag_service: Arc<dyn TagService>,
    mode: TagFormMode,
    template: String,
    label: String,
    /// On successful Create, also assign the new tag to this task via
    /// the given service. `None` means create-only. Ignored for Edit
    /// modes since editing doesn't touch task↔tag relations.
    assign_after_create: Option<(Uuid, Arc<dyn TaskService>)>,
    /// When the session was opened from a content/adapter tab, the pane to
    /// reload on a successful commit (so its tag columns re-render). When
    /// `None` the commit refreshes the native Tasks tab instead.
    content_reload: Option<(usize, PaneId)>,
}

impl TagFormSession {
    pub async fn edit(
        tag_service: Arc<dyn TagService>,
        id: String,
        content_reload: Option<(usize, PaneId)>,
    ) -> Result<Self, String> {
        if let Some(rest) = id.strip_prefix("global-tag:") {
            let uuid = uuid::Uuid::parse_str(rest)
                .map_err(|_| format!("invalid uuid in {id}"))?;
            let list = tag_service.list_global().await.map_err(|e| e.to_string())?;
            let tag = list
                .into_iter()
                .find(|t| t.id == uuid)
                .ok_or_else(|| format!("tag not found: {id}"))?;
            let template = edit_global_template(&tag);
            let label = format!("edit tag {}", tag.name);
            return Ok(Self {
                tag_service,
                mode: TagFormMode::EditGlobal { id },
                template,
                label,
                assign_after_create: None,
                content_reload,
            });
        }
        if let Some(_rest) = id.strip_prefix("project-tag:") {
            // We need the project-tag model + its project name. The
            // service's `list_all` returns both — cheaper than a
            // dedicated by-id-with-project endpoint we'd otherwise
            // have to add.
            let items = tag_service.list_all().await.map_err(|e| e.to_string())?;
            for item in items {
                if let not_yet_done_task_core::service::TagItem::Project { tag, project_name } = item {
                    if format!("project-tag:{}", tag.id) == id {
                        let template = edit_project_template(&tag, &project_name);
                        let label = format!("edit tag {}", tag.name);
                        return Ok(Self {
                            tag_service,
                            mode: TagFormMode::EditProject { id },
                            template,
                            label,
                            assign_after_create: None,
                            content_reload,
                        });
                    }
                }
            }
            return Err(format!("tag not found: {id}"));
        }
        Err(format!("unknown tag id format: {id}"))
    }

    /// Create variant that opens the editor pre-filled with `name`.
    /// Used by the menu's "type new name + Enter" flow.
    ///
    /// `assign_after_create`: when `Some`, the newly created tag is
    /// also assigned to the given task on successful commit. The menu
    /// uses this to honour the "Enter assigns to selected task"
    /// convention for the CreateNew path.
    pub fn create_with_name(
        tag_service: Arc<dyn TagService>,
        name: &str,
        assign_after_create: Option<(Uuid, Arc<dyn TaskService>)>,
        content_reload: Option<(usize, PaneId)>,
    ) -> Self {
        let template = new_tag_template().replace("name:\n", &format!("name: \"{}\"\n", name));
        Self {
            tag_service,
            mode: TagFormMode::Create,
            template,
            label: format!("new tag {name}"),
            assign_after_create,
            content_reload,
        }
    }
}

#[async_trait]
impl EditSession for TagFormSession {
    fn template(&self) -> &str {
        &self.template
    }

    fn suffix(&self) -> &str {
        ".yaml"
    }

    fn scope(&self) -> SessionScope {
        // Tag mgmt is App-level; pin to Tasks for the action bar.
        SessionScope::Tasks
    }

    fn label(&self) -> &str {
        &self.label
    }

    async fn commit(&mut self, text: &str) -> CommitOutcome {
        let stripped = strip_error_block(text);
        let draft = match parse_draft(&stripped) {
            Ok(d) => d,
            Err(e) => return CommitOutcome::Reopen { content: annotate_error(&stripped, &e) },
        };

        let name = match normalize(draft.name) {
            Some(n) => n,
            None => {
                return CommitOutcome::Cancelled {
                    message: Some("Tag edit aborted (empty name)".to_string()),
                };
            }
        };
        let style = TagStyle {
            fg_color: normalize(draft.fg_color),
            bg_color: normalize(draft.bg_color),
            symbol: normalize(draft.symbol),
        };
        let project = normalize(draft.project);

        // `result`: on Create success, also carries the new tag's
        // qualified id (`global-tag:<uuid>` / `project-tag:<uuid>`) so
        // we can auto-assign it to the target task afterwards.
        let result: Result<(String, Option<String>), _> = match &self.mode {
            TagFormMode::Create => {
                if let Some(proj) = project {
                    self.tag_service
                        .add_project_tag(name.clone(), style, proj)
                        .await
                        .map(|t| (
                            format!("✓ created project tag {}", t.name),
                            Some(format!("project-tag:{}", t.id)),
                        ))
                } else {
                    self.tag_service
                        .add_global(name.clone(), style)
                        .await
                        .map(|t| (
                            format!("✓ created global tag {}", t.name),
                            Some(format!("global-tag:{}", t.id)),
                        ))
                }
            }
            TagFormMode::EditGlobal { id } | TagFormMode::EditProject { id } => {
                let patch = TagStylePatch {
                    fg_color: Some(style.fg_color),
                    bg_color: Some(style.bg_color),
                    symbol: Some(style.symbol),
                };
                self.tag_service
                    .edit(id.clone(), Some(name.clone()), patch)
                    .await
                    .map(|_| (format!("✓ updated tag {name}"), None))
            }
        };

        match result {
            Ok((msg, new_tag_id)) => {
                // Auto-assign on Create when the menu passed a target task.
                let mut message = msg;
                if let (Some(tag_id), Some((task_id, task_svc))) =
                    (new_tag_id, self.assign_after_create.take())
                {
                    match task_svc
                        .edit_task(task_id, None, None, None, Some(tag_id), None)
                        .await
                    {
                        Ok(_) => message = format!("{message} (assigned)"),
                        // Tag was created, assign failed — report both; the
                        // reload below still surfaces the new tag.
                        Err(e) => message = format!("{message} (assign failed: {e})"),
                    }
                }
                // Content tab: reload the originating pane so its tag
                // columns re-render. Tag menus only open from content tabs
                // now, so `content_reload` is always set on an assign.
                if let Some((view_index, pane_id)) = self.content_reload {
                    CommitOutcome::FollowUp(FollowUp::ReloadContentPaneForTag {
                        view_index,
                        pane_id,
                        message,
                    })
                } else {
                    CommitOutcome::Done { message: Some(message) }
                }
            }
            Err(e) => CommitOutcome::Reopen {
                content: annotate_error(&stripped, &e.to_string()),
            },
        }
    }
}
