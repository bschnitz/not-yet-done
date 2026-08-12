use async_trait::async_trait;
use shaku::Component;
use std::sync::Arc;
use uuid::Uuid;

use crate::entity::task;
use crate::error::AppError;
use crate::filter::FilterExpr;
use crate::repository::{ProjectRepository, TagRepository, TaskRepository};

#[async_trait]
pub trait TaskService: shaku::Interface {
    async fn add_task(
        &self,
        description: String,
        project: Option<String>,
        parent_id: Option<String>,
        tag: Option<String>,
        status: Option<task::TaskStatus>,
        priority: Option<i32>,
    ) -> Result<task::Model, AppError>;
    async fn list_tasks(&self, project: Option<String>) -> Result<Vec<task::Model>, AppError>;
    /// Like [`list_tasks`](Self::list_tasks) but returns the full universe
    /// including soft-deleted tasks. Eager adapters load this so a saved
    /// query is the single, replaceable filter (the shipped default
    /// `deleted = false` hides deleted rows; a query that drops it surfaces
    /// them).
    async fn list_tasks_including_deleted(
        &self,
        project: Option<String>,
    ) -> Result<Vec<task::Model>, AppError>;
    async fn list_filtered(&self, expr: &FilterExpr) -> Result<Vec<task::Model>, AppError>;
    async fn list_filtered_with_options(
        &self,
        expr: &FilterExpr,
        options: &crate::filter::query_filter::QueryOptions,
    ) -> Result<Vec<task::Model>, AppError>;
    async fn get_subtree(
        &self,
        root_id: Uuid,
        last_tracked_since: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Vec<task::Model>, AppError>;
    async fn delete_task(&self, id: Uuid) -> Result<(), AppError>;
    async fn delete_task_recursive(&self, id: Uuid) -> Result<usize, AppError>;
    async fn undelete_last(&self) -> Result<usize, AppError>;
    async fn edit_task(
        &self,
        id: Uuid,
        description: Option<String>,
        add_project: Option<String>,
        remove_project: Option<String>,
        add_tag: Option<String>,
        remove_tag: Option<String>,
    ) -> Result<task::Model, AppError>;
    async fn update_task(
        &self,
        id: Uuid,
        description: Option<String>,
        status: Option<task::TaskStatus>,
        priority: Option<i32>,
        parent_id: Option<Option<Uuid>>,
        deleted: Option<bool>,
    ) -> Result<task::Model, AppError>;

    /// Batch fetch tags for many tasks. Returns map task_id → tags
    /// (mix of global + project tags). Empty vec for tagless tasks.
    async fn load_tags_for_tasks(
        &self,
        task_ids: &[Uuid],
    ) -> Result<std::collections::HashMap<Uuid, Vec<crate::repository::ResolvedTag>>, AppError>;
}

#[derive(Component)]
#[shaku(interface = TaskService)]
pub struct TaskServiceImpl {
    #[shaku(inject)]
    task_repository: Arc<dyn TaskRepository>,
    #[shaku(inject)]
    project_repository: Arc<dyn ProjectRepository>,
    #[shaku(inject)]
    tag_repository: Arc<dyn TagRepository>,
}

#[async_trait]
impl TaskService for TaskServiceImpl {
    async fn add_task(
        &self,
        description: String,
        project: Option<String>,
        parent_id: Option<String>,
        tag: Option<String>,
        status: Option<task::TaskStatus>,
        priority: Option<i32>,
    ) -> Result<task::Model, AppError> {
        let parent_uuid = if let Some(p) = parent_id {
            Some(Uuid::parse_str(&p).map_err(|_| AppError::InvalidId(p))?)
        } else {
            None
        };

        let task = self
            .task_repository
            .insert(description, parent_uuid, status, priority)
            .await?;

        if let Some(proj_ref) = project {
            let project = self.resolve_project(&proj_ref).await?;
            self.task_repository
                .assign_project(task.id, project.id)
                .await?;
        }

        if let Some(tag_ref) = tag {
            let resolved = self.resolve_tag(task.id, &tag_ref).await?;
            match resolved {
                crate::repository::ResolvedTag::Global(t) => {
                    self.task_repository
                        .assign_global_tag(task.id, t.id)
                        .await?
                }
                crate::repository::ResolvedTag::Project(t) => {
                    self.task_repository
                        .assign_project_tag(task.id, t.id)
                        .await?
                }
            }
        }

        Ok(task)
    }

    async fn list_tasks(&self, project: Option<String>) -> Result<Vec<task::Model>, AppError> {
        let project_id = if let Some(proj_ref) = project {
            let p = self.resolve_project(&proj_ref).await?;
            Some(p.id)
        } else {
            None
        };
        self.task_repository.find_all(project_id).await
    }

    async fn list_tasks_including_deleted(
        &self,
        project: Option<String>,
    ) -> Result<Vec<task::Model>, AppError> {
        let project_id = if let Some(proj_ref) = project {
            let p = self.resolve_project(&proj_ref).await?;
            Some(p.id)
        } else {
            None
        };
        self.task_repository
            .find_all_including_deleted(project_id)
            .await
    }

    /// Return all tasks matching the given filter expression.
    ///
    /// Unlike `list_tasks`, this method does not apply any implicit filters
    /// (e.g. `deleted = false`) — the caller is responsible for including
    /// all desired conditions in the expression.
    async fn list_filtered(&self, expr: &FilterExpr) -> Result<Vec<task::Model>, AppError> {
        self.task_repository.find_filtered(expr).await
    }

    async fn list_filtered_with_options(
        &self,
        expr: &FilterExpr,
        options: &crate::filter::query_filter::QueryOptions,
    ) -> Result<Vec<task::Model>, AppError> {
        self.task_repository
            .find_filtered_with_options(expr, options)
            .await
    }

    async fn get_subtree(
        &self,
        root_id: Uuid,
        last_tracked_since: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Vec<task::Model>, AppError> {
        self.task_repository
            .find_subtree(root_id, last_tracked_since)
            .await
    }

    async fn delete_task(&self, id: Uuid) -> Result<(), AppError> {
        self.task_repository.soft_delete(id).await
    }

    async fn delete_task_recursive(&self, id: Uuid) -> Result<usize, AppError> {
        self.task_repository.soft_delete_recursive(id).await
    }

    async fn undelete_last(&self) -> Result<usize, AppError> {
        self.task_repository.undelete_last().await
    }

    async fn edit_task(
        &self,
        id: Uuid,
        description: Option<String>,
        add_project: Option<String>,
        remove_project: Option<String>,
        add_tag: Option<String>,
        remove_tag: Option<String>,
    ) -> Result<task::Model, AppError> {
        let task = if let Some(desc) = description {
            self.task_repository.update_description(id, desc).await?
        } else {
            self.task_repository.find_by_id(id).await?
        };

        if let Some(proj_ref) = add_project {
            let project = self.resolve_project(&proj_ref).await?;
            self.task_repository.assign_project(id, project.id).await?;
        }

        if let Some(proj_ref) = remove_project {
            let project = self.resolve_project(&proj_ref).await?;
            self.task_repository
                .unassign_project(id, project.id)
                .await?;
        }

        if let Some(tag_ref) = add_tag {
            let resolved = self.resolve_tag(id, &tag_ref).await?;
            match resolved {
                crate::repository::ResolvedTag::Global(t) => {
                    self.task_repository.assign_global_tag(id, t.id).await?
                }
                crate::repository::ResolvedTag::Project(t) => {
                    self.task_repository.assign_project_tag(id, t.id).await?
                }
            }
        }

        if let Some(tag_ref) = remove_tag {
            let resolved = self.resolve_tag(id, &tag_ref).await?;
            match resolved {
                crate::repository::ResolvedTag::Global(t) => {
                    self.task_repository.unassign_global_tag(id, t.id).await?
                }
                crate::repository::ResolvedTag::Project(t) => {
                    self.task_repository.unassign_project_tag(id, t.id).await?
                }
            }
        }

        Ok(task)
    }

    async fn update_task(
        &self,
        id: Uuid,
        description: Option<String>,
        status: Option<task::TaskStatus>,
        priority: Option<i32>,
        parent_id: Option<Option<Uuid>>,
        deleted: Option<bool>,
    ) -> Result<task::Model, AppError> {
        self.task_repository
            .update_task(id, description, status, priority, parent_id, deleted)
            .await
    }

    async fn load_tags_for_tasks(
        &self,
        task_ids: &[Uuid],
    ) -> Result<std::collections::HashMap<Uuid, Vec<crate::repository::ResolvedTag>>, AppError>
    {
        self.task_repository.find_tags_by_task_ids(task_ids).await
    }
}

impl TaskServiceImpl {
    /// Resolve a project reference string (name or UUID).
    async fn resolve_project(
        &self,
        proj_ref: &str,
    ) -> Result<crate::entity::project::Model, AppError> {
        if let Ok(id) = uuid::Uuid::parse_str(proj_ref) {
            self.project_repository.find_by_id(id).await
        } else {
            self.project_repository
                .find_by_name(proj_ref)
                .await?
                .ok_or_else(|| AppError::ProjectNotFound(Uuid::nil()))
        }
    }

    async fn resolve_tag(
        &self,
        task_id: Uuid,
        tag_ref: &str,
    ) -> Result<crate::repository::ResolvedTag, AppError> {
        use crate::repository::ResolvedTag;

        // Prefix format: "global-tag:<uuid>" or "project-tag:<uuid>"
        if let Some(rest) = tag_ref.strip_prefix("global-tag:") {
            let id = Uuid::parse_str(rest).map_err(|_| AppError::InvalidId(rest.to_string()))?;
            let tag = self.tag_repository.find_global_by_id(id).await?;
            return Ok(ResolvedTag::Global(tag));
        }
        if let Some(rest) = tag_ref.strip_prefix("project-tag:") {
            let id = Uuid::parse_str(rest).map_err(|_| AppError::InvalidId(rest.to_string()))?;
            let tag = self.tag_repository.find_project_tag_by_id(id).await?;
            return Ok(ResolvedTag::Project(tag));
        }

        // Plain UUID — try global first, then project
        if let Ok(id) = Uuid::parse_str(tag_ref) {
            if let Ok(tag) = self.tag_repository.find_global_by_id(id).await {
                return Ok(ResolvedTag::Global(tag));
            }
            if let Ok(tag) = self.tag_repository.find_project_tag_by_id(id).await {
                return Ok(ResolvedTag::Project(tag));
            }
            return Err(AppError::TagNotFound(tag_ref.to_string()));
        }

        // Name resolution: check project tags of the task first
        let project_ids = self
            .task_repository
            .find_project_ids_for_task(task_id)
            .await?;
        let project_matches = self
            .tag_repository
            .find_project_tags_by_name(tag_ref, &project_ids)
            .await?;

        if project_matches.len() > 1 {
            return Err(AppError::AmbiguousTag(tag_ref.to_string()));
        }
        if let Some(tag) = project_matches.into_iter().next() {
            return Ok(ResolvedTag::Project(tag));
        }

        // Then global
        if let Some(tag) = self.tag_repository.find_global_by_name(tag_ref).await? {
            return Ok(ResolvedTag::Global(tag));
        }

        Err(AppError::TagNotFound(tag_ref.to_string()))
    }
}
