// not-yet-done-task-core/src/repository/task_repository.rs

use async_trait::async_trait;
use sea_orm::prelude::Expr;
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait,
    QueryFilter, Set,
};
use shaku::Component;
use uuid::Uuid;

use std::collections::HashMap;

use crate::entity::task::{self, ActiveModel, TaskColumnRegistry, TaskStatus};
use crate::entity::task_project;
use crate::error::AppError;
use crate::filter::{FilterBuilder, FilterExpr};
use crate::recur::Rule;

// ---------------------------------------------------------------------------
// Batch operation types
// ---------------------------------------------------------------------------

/// A single operation in a batch of tree mutations.
#[derive(Debug, Clone)]
pub enum TaskOp {
    Insert {
        description: String,
        parent_id: Option<Uuid>,
        status: TaskStatus,
        priority: i32,
    },
    Update {
        id: Uuid,
        description: Option<String>,
        status: Option<TaskStatus>,
        priority: Option<i32>,
        parent_id: Option<Option<Uuid>>,
        deleted: Option<bool>,
    },
    Delete {
        id: Uuid,
    },
}

// ---------------------------------------------------------------------------
// Path helpers (public for testing)
// ---------------------------------------------------------------------------

/// Short ID: first 8 hex chars of a UUID.
pub fn short_id(id: Uuid) -> String {
    id.to_string()[..8].to_string()
}

/// Compute the path for a task given a map of id → parent_id.
/// Returns `/<short-root>/.../<short-self>/`.
pub fn compute_path(id: Uuid, parents: &HashMap<Uuid, Option<Uuid>>) -> String {
    let mut chain = vec![id];
    let mut current = parents.get(&id).copied().flatten();
    // Walk up to root, guard against cycles.
    let mut seen = std::collections::HashSet::new();
    seen.insert(id);
    while let Some(pid) = current {
        if !seen.insert(pid) {
            break;
        } // cycle guard
        chain.push(pid);
        current = parents.get(&pid).copied().flatten();
    }
    chain.reverse();
    let mut path = String::from("/");
    for nid in chain {
        path.push_str(&short_id(nid));
        path.push('/');
    }
    path
}

fn is_open(status: &TaskStatus) -> bool {
    matches!(status, TaskStatus::Todo | TaskStatus::InProgress)
}

/// Path of a new task: parent's path + own short id.
async fn child_path(
    db: &DatabaseConnection,
    id: Uuid,
    parent_id: Option<Uuid>,
) -> Result<String, AppError> {
    Ok(if let Some(pid) = parent_id {
        let parent = task::Entity::find_by_id(pid).one(db).await?;
        let parent_path = parent
            .and_then(|p| p.path)
            .unwrap_or_else(|| format!("/{}/", short_id(pid)));
        format!("{}/{}/", parent_path.trim_end_matches('/'), short_id(id))
    } else {
        format!("/{}/", short_id(id))
    })
}

#[async_trait]
pub trait TaskRepository: shaku::Interface {
    async fn insert(
        &self,
        description: String,
        parent_id: Option<Uuid>,
        status: Option<task::TaskStatus>,
        priority: Option<i32>,
    ) -> Result<task::Model, AppError>;
    async fn find_all(&self, project_id: Option<Uuid>) -> Result<Vec<task::Model>, AppError>;
    /// Find *every* task, deleted or not. This is the unfiltered universe —
    /// callers that want only the live set apply a `deleted = false` query
    /// filter on top. Eager adapters load this so the query is the single,
    /// replaceable filter rather than stacking on a baked-in `deleted = false`
    /// (see also [`TrackingRepository::find_all_including_deleted`]).
    async fn find_all_including_deleted(
        &self,
        project_id: Option<Uuid>,
    ) -> Result<Vec<task::Model>, AppError>;
    async fn find_by_id(&self, id: Uuid) -> Result<task::Model, AppError>;
    async fn soft_delete(&self, id: Uuid) -> Result<(), AppError>;
    /// Soft-delete a task and all its descendants. All affected tasks get the
    /// same `deleted_at` timestamp so the operation can be undone as a group.
    /// Only tasks that are not already deleted are affected.
    async fn soft_delete_recursive(&self, id: Uuid) -> Result<usize, AppError>;
    /// Undo the most recent delete operation by restoring all tasks that share
    /// the latest `deleted_at` timestamp.
    async fn undelete_last(&self) -> Result<usize, AppError>;
    async fn update_description(
        &self,
        id: Uuid,
        description: String,
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
    /// Replace the task's time window (`not_before` / `due`) as a whole.
    /// `None` clears the bound. No validation here — the service checks the
    /// bounds against each other.
    async fn set_window(
        &self,
        id: Uuid,
        not_before: Option<chrono::DateTime<chrono::Utc>>,
        due: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<task::Model, AppError>;
    /// Replace the task's recurrence rule; `None` ends the series at this
    /// task. A task that gets a rule and has no `series_id` starts its own
    /// series. No validation here — the service parses the rule.
    async fn set_recur(&self, id: Uuid, recur: Option<String>) -> Result<task::Model, AppError>;
    async fn assign_project(&self, task_id: Uuid, project_id: Uuid) -> Result<(), AppError>;
    async fn unassign_project(&self, task_id: Uuid, project_id: Uuid) -> Result<(), AppError>;
    async fn soft_delete_by_project(&self, project_id: Uuid) -> Result<(), AppError>;
    async fn assign_global_tag(&self, task_id: Uuid, tag_id: Uuid) -> Result<(), AppError>;
    async fn unassign_global_tag(&self, task_id: Uuid, tag_id: Uuid) -> Result<(), AppError>;
    async fn assign_project_tag(&self, task_id: Uuid, tag_id: Uuid) -> Result<(), AppError>;
    async fn unassign_project_tag(&self, task_id: Uuid, tag_id: Uuid) -> Result<(), AppError>;
    async fn find_project_ids_for_task(&self, task_id: Uuid) -> Result<Vec<Uuid>, AppError>;

    /// Batch fetch tags for many tasks at once. Returns a map from
    /// task id → list of tags (mix of global + project tags). Empty
    /// vec for tasks with no tags. Avoids N+1 in list views.
    async fn find_tags_by_task_ids(
        &self,
        task_ids: &[Uuid],
    ) -> Result<std::collections::HashMap<Uuid, Vec<crate::repository::ResolvedTag>>, AppError>;

    /// Return all tasks matching the given filter expression.
    ///
    /// The expression is compiled entirely to SQL.  Only columns of the `task`
    /// entity may be referenced; unknown column names produce
    /// [`AppError::FilterError`].
    async fn find_filtered(&self, expr: &FilterExpr) -> Result<Vec<task::Model>, AppError>;

    /// Like `find_filtered`, but applies query options (e.g. include_ancestors).
    async fn find_filtered_with_options(
        &self,
        expr: &FilterExpr,
        options: &crate::filter::query_filter::QueryOptions,
    ) -> Result<Vec<task::Model>, AppError>;

    /// Return a task and all its descendants. When `last_tracked_since` is
    /// given, only leaf tasks (those without children) that were tracked at
    /// or after that timestamp are included; ancestor tasks on the path to
    /// those leaves are always included.
    async fn find_subtree(
        &self,
        root_id: Uuid,
        last_tracked_since: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Vec<task::Model>, AppError>;

    /// Execute a batch of tree operations in order, then rebuild paths for
    /// all affected tasks. Returns the created/updated models.
    async fn apply_batch(&self, ops: Vec<TaskOp>) -> Result<Vec<task::Model>, AppError>;

    /// Rebuild the `path` column for all tasks. Called after schema migration
    /// or data import.
    async fn rebuild_all_paths(&self) -> Result<usize, AppError>;
}

#[derive(Component)]
#[shaku(interface = TaskRepository)]
pub struct TaskRepositoryImpl {
    #[shaku(default)]
    db: Option<DatabaseConnection>,
}

#[async_trait]
impl TaskRepository for TaskRepositoryImpl {
    async fn insert(
        &self,
        description: String,
        parent_id: Option<Uuid>,
        status: Option<task::TaskStatus>,
        priority: Option<i32>,
    ) -> Result<task::Model, AppError> {
        let db = self.db.as_ref().expect("DB not initialized");
        let mut model = ActiveModel {
            description: Set(description),
            parent_id: Set(parent_id),
            ..ActiveModel::new()
        };
        if let Some(s) = status {
            model.status = Set(s);
        }
        if let Some(p) = priority {
            model.priority = Set(p);
        }
        model.path = Set(Some(child_path(db, model.id.clone().unwrap(), parent_id).await?));
        Ok(model.insert(db).await?)
    }

    async fn find_all(&self, project_id: Option<Uuid>) -> Result<Vec<task::Model>, AppError> {
        use crate::entity::task::Column;
        use sea_orm::QuerySelect;
        let db = self.db.as_ref().expect("DB not initialized");

        let query = task::Entity::find().filter(Column::Deleted.eq(false));

        if let Some(pid) = project_id {
            use crate::entity::task_project::Column as TpCol;
            use sea_orm::JoinType;
            return Ok(query
                .join(
                    JoinType::InnerJoin,
                    task::Entity::belongs_to(crate::entity::task_project::Entity)
                        .from(Column::Id)
                        .to(TpCol::TaskId)
                        .into(),
                )
                .filter(TpCol::ProjectId.eq(pid))
                .all(db)
                .await?);
        }

        Ok(query.all(db).await?)
    }

    async fn find_all_including_deleted(
        &self,
        project_id: Option<Uuid>,
    ) -> Result<Vec<task::Model>, AppError> {
        use crate::entity::task::Column;
        use sea_orm::QuerySelect;
        let db = self.db.as_ref().expect("DB not initialized");

        // Same as `find_all` but without the implicit `deleted = false` —
        // the full task universe, deleted rows included.
        let query = task::Entity::find();

        if let Some(pid) = project_id {
            use crate::entity::task_project::Column as TpCol;
            use sea_orm::JoinType;
            return Ok(query
                .join(
                    JoinType::InnerJoin,
                    task::Entity::belongs_to(crate::entity::task_project::Entity)
                        .from(Column::Id)
                        .to(TpCol::TaskId)
                        .into(),
                )
                .filter(TpCol::ProjectId.eq(pid))
                .all(db)
                .await?);
        }

        Ok(query.all(db).await?)
    }

    async fn find_by_id(&self, id: Uuid) -> Result<task::Model, AppError> {
        let db = self.db.as_ref().expect("DB not initialized");
        task::Entity::find_by_id(id)
            .one(db)
            .await?
            .ok_or(AppError::TaskNotFound(id))
    }

    async fn soft_delete(&self, id: Uuid) -> Result<(), AppError> {
        let db = self.db.as_ref().expect("DB not initialized");
        let task = self.find_by_id(id).await?;
        let mut model: ActiveModel = task.into();
        let now = chrono::Utc::now();
        model.deleted = Set(true);
        model.deleted_at = Set(Some(now));
        model.updated_at = Set(now);
        model.update(db).await?;
        Ok(())
    }

    async fn soft_delete_recursive(&self, id: Uuid) -> Result<usize, AppError> {
        use crate::entity::task::Column;

        let db = self.db.as_ref().expect("DB not initialized");
        let now = chrono::Utc::now();

        // Collect all descendant IDs (BFS).
        let mut to_delete = vec![id];
        let mut queue = vec![id];
        while let Some(parent) = queue.pop() {
            let children: Vec<task::Model> = task::Entity::find()
                .filter(Column::ParentId.eq(Some(parent)))
                .filter(Column::Deleted.eq(false))
                .all(db)
                .await?;
            for child in children {
                to_delete.push(child.id);
                queue.push(child.id);
            }
        }

        if to_delete.is_empty() {
            return Ok(0);
        }

        let count = to_delete.len();
        task::Entity::update_many()
            .col_expr(Column::Deleted, Expr::value(true))
            .col_expr(Column::DeletedAt, Expr::value(now))
            .col_expr(Column::UpdatedAt, Expr::value(now))
            .filter(Column::Id.is_in(to_delete))
            .filter(Column::Deleted.eq(false))
            .exec(db)
            .await?;

        Ok(count)
    }

    async fn undelete_last(&self) -> Result<usize, AppError> {
        use crate::entity::task::Column;
        use sea_orm::QueryOrder;

        let db = self.db.as_ref().expect("DB not initialized");

        // Find the most recent deleted_at timestamp.
        let latest = task::Entity::find()
            .filter(Column::Deleted.eq(true))
            .filter(Column::DeletedAt.is_not_null())
            .order_by_desc(Column::DeletedAt)
            .one(db)
            .await?;

        let Some(latest_task) = latest else {
            return Ok(0);
        };
        let Some(ts) = latest_task.deleted_at else {
            return Ok(0);
        };

        let result = task::Entity::update_many()
            .col_expr(Column::Deleted, Expr::value(false))
            .col_expr(
                Column::DeletedAt,
                Expr::value(Option::<chrono::DateTime<chrono::Utc>>::None),
            )
            .col_expr(Column::UpdatedAt, Expr::value(chrono::Utc::now()))
            .filter(Column::DeletedAt.eq(ts))
            .exec(db)
            .await?;

        Ok(result.rows_affected as usize)
    }

    async fn update_description(
        &self,
        id: Uuid,
        description: String,
    ) -> Result<task::Model, AppError> {
        let db = self.db.as_ref().expect("DB not initialized");
        let task = self.find_by_id(id).await?;
        let mut model: ActiveModel = task.into();
        model.description = Set(description);
        model.updated_at = Set(chrono::Utc::now());
        Ok(model.update(db).await?)
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
        let db = self.db.as_ref().expect("DB not initialized");
        let task = self.find_by_id(id).await?;
        let parent_changed = parent_id.is_some() && parent_id != Some(task.parent_id);
        let before = task.clone();
        let mut model: ActiveModel = task.into();
        if let Some(d) = description {
            model.description = Set(d);
        }
        if let Some(s) = status {
            model.status = Set(s);
        }
        if let Some(p) = priority {
            model.priority = Set(p);
        }
        if let Some(pid) = parent_id {
            model.parent_id = Set(pid);
        }
        if let Some(del) = deleted {
            model.deleted = Set(del);
        }
        model.updated_at = Set(chrono::Utc::now());
        let updated = model.update(db).await?;
        self.advance_series(db, &before, &updated).await?;

        // Rebuild paths for this task and all descendants when parent changes.
        if parent_changed {
            self.rebuild_subtree_paths(db, id).await?;
        }

        // Re-fetch to get the updated path.
        if parent_changed {
            Ok(self.find_by_id(id).await?)
        } else {
            Ok(updated)
        }
    }

    async fn set_window(
        &self,
        id: Uuid,
        not_before: Option<chrono::DateTime<chrono::Utc>>,
        due: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<task::Model, AppError> {
        let db = self.db.as_ref().expect("DB not initialized");
        let task = self.find_by_id(id).await?;
        let mut model: ActiveModel = task.into();
        model.not_before = Set(not_before);
        model.due = Set(due);
        model.updated_at = Set(chrono::Utc::now());
        Ok(model.update(db).await?)
    }

    async fn set_recur(&self, id: Uuid, recur: Option<String>) -> Result<task::Model, AppError> {
        let db = self.db.as_ref().expect("DB not initialized");
        let task = self.find_by_id(id).await?;
        let series_id = task.series_id.or(recur.as_ref().map(|_| task.id));
        let mut model: ActiveModel = task.into();
        model.recur = Set(recur);
        model.series_id = Set(series_id);
        model.updated_at = Set(chrono::Utc::now());
        Ok(model.update(db).await?)
    }

    async fn assign_project(&self, task_id: Uuid, project_id: Uuid) -> Result<(), AppError> {
        let db = self.db.as_ref().expect("DB not initialized");
        let join = task_project::ActiveModel {
            task_id: Set(task_id),
            project_id: Set(project_id),
        };
        use sea_orm::ActiveModelTrait;
        join.insert(db).await?;
        Ok(())
    }

    async fn unassign_project(&self, task_id: Uuid, project_id: Uuid) -> Result<(), AppError> {
        let db = self.db.as_ref().expect("DB not initialized");
        task_project::Entity::delete_many()
            .filter(task_project::Column::TaskId.eq(task_id))
            .filter(task_project::Column::ProjectId.eq(project_id))
            .exec(db)
            .await?;
        Ok(())
    }

    async fn soft_delete_by_project(&self, project_id: Uuid) -> Result<(), AppError> {
        use crate::entity::task::Column;
        let db = self.db.as_ref().expect("DB not initialized");

        let task_ids: Vec<Uuid> = task_project::Entity::find()
            .filter(task_project::Column::ProjectId.eq(project_id))
            .all(db)
            .await?
            .into_iter()
            .map(|tp| tp.task_id)
            .collect();

        if task_ids.is_empty() {
            return Ok(());
        }

        task::Entity::update_many()
            .col_expr(task::Column::Deleted, Expr::value(true))
            .col_expr(task::Column::UpdatedAt, Expr::value(chrono::Utc::now()))
            .filter(Column::Id.is_in(task_ids))
            .exec(db)
            .await?;

        Ok(())
    }

    async fn assign_global_tag(&self, task_id: Uuid, tag_id: Uuid) -> Result<(), AppError> {
        use crate::entity::task_global_tag;
        let db = self.db.as_ref().expect("DB not initialized");
        task_global_tag::ActiveModel {
            task_id: Set(task_id),
            global_tag_id: Set(tag_id),
        }
        .insert(db)
        .await?;
        Ok(())
    }

    async fn unassign_global_tag(&self, task_id: Uuid, tag_id: Uuid) -> Result<(), AppError> {
        use crate::entity::task_global_tag;
        let db = self.db.as_ref().expect("DB not initialized");
        task_global_tag::Entity::delete_many()
            .filter(task_global_tag::Column::TaskId.eq(task_id))
            .filter(task_global_tag::Column::GlobalTagId.eq(tag_id))
            .exec(db)
            .await?;
        Ok(())
    }

    async fn assign_project_tag(&self, task_id: Uuid, tag_id: Uuid) -> Result<(), AppError> {
        use crate::entity::task_project_tag;
        let db = self.db.as_ref().expect("DB not initialized");
        task_project_tag::ActiveModel {
            task_id: Set(task_id),
            project_tag_id: Set(tag_id),
        }
        .insert(db)
        .await?;
        Ok(())
    }

    async fn unassign_project_tag(&self, task_id: Uuid, tag_id: Uuid) -> Result<(), AppError> {
        use crate::entity::task_project_tag;
        let db = self.db.as_ref().expect("DB not initialized");
        task_project_tag::Entity::delete_many()
            .filter(task_project_tag::Column::TaskId.eq(task_id))
            .filter(task_project_tag::Column::ProjectTagId.eq(tag_id))
            .exec(db)
            .await?;
        Ok(())
    }

    async fn find_project_ids_for_task(&self, task_id: Uuid) -> Result<Vec<Uuid>, AppError> {
        let db = self.db.as_ref().expect("DB not initialized");
        Ok(task_project::Entity::find()
            .filter(task_project::Column::TaskId.eq(task_id))
            .all(db)
            .await?
            .into_iter()
            .map(|tp| tp.project_id)
            .collect())
    }

    async fn find_tags_by_task_ids(
        &self,
        task_ids: &[Uuid],
    ) -> Result<std::collections::HashMap<Uuid, Vec<crate::repository::ResolvedTag>>, AppError>
    {
        use crate::entity::{global_tag, project_tag, task_global_tag, task_project_tag};
        use crate::repository::ResolvedTag;
        use std::collections::HashMap;

        let mut out: HashMap<Uuid, Vec<ResolvedTag>> = HashMap::new();
        if task_ids.is_empty() {
            return Ok(out);
        }
        let db = self.db.as_ref().expect("DB not initialized");

        let g_links = task_global_tag::Entity::find()
            .filter(task_global_tag::Column::TaskId.is_in(task_ids.to_vec()))
            .all(db)
            .await?;
        if !g_links.is_empty() {
            let g_ids: Vec<Uuid> = g_links
                .iter()
                .map(|l| l.global_tag_id)
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            let g_tags = global_tag::Entity::find()
                .filter(global_tag::Column::Id.is_in(g_ids))
                .all(db)
                .await?;
            let g_by_id: HashMap<Uuid, global_tag::Model> =
                g_tags.into_iter().map(|t| (t.id, t)).collect();
            for link in g_links {
                if let Some(t) = g_by_id.get(&link.global_tag_id) {
                    out.entry(link.task_id)
                        .or_default()
                        .push(ResolvedTag::Global(t.clone()));
                }
            }
        }

        let p_links = task_project_tag::Entity::find()
            .filter(task_project_tag::Column::TaskId.is_in(task_ids.to_vec()))
            .all(db)
            .await?;
        if !p_links.is_empty() {
            let p_ids: Vec<Uuid> = p_links
                .iter()
                .map(|l| l.project_tag_id)
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            let p_tags = project_tag::Entity::find()
                .filter(project_tag::Column::Id.is_in(p_ids))
                .all(db)
                .await?;
            let p_by_id: HashMap<Uuid, project_tag::Model> =
                p_tags.into_iter().map(|t| (t.id, t)).collect();
            for link in p_links {
                if let Some(t) = p_by_id.get(&link.project_tag_id) {
                    out.entry(link.task_id)
                        .or_default()
                        .push(ResolvedTag::Project(t.clone()));
                }
            }
        }

        Ok(out)
    }

    async fn find_filtered(&self, expr: &FilterExpr) -> Result<Vec<task::Model>, AppError> {
        let db = self.db.as_ref().expect("DB not initialized");
        let resolved = crate::filter::tree_ops::resolve_tree_operators(expr, db).await?;
        let condition = FilterBuilder::new(&TaskColumnRegistry).build(&resolved)?;
        Ok(task::Entity::find().filter(condition).all(db).await?)
    }

    async fn find_filtered_with_options(
        &self,
        expr: &FilterExpr,
        options: &crate::filter::query_filter::QueryOptions,
    ) -> Result<Vec<task::Model>, AppError> {
        let mut results = self.find_filtered(expr).await?;

        if options.include_ancestors && !results.is_empty() {
            let db = self.db.as_ref().expect("DB not initialized");
            let result_ids: std::collections::HashSet<Uuid> =
                results.iter().map(|t| t.id).collect();

            // Collect all ancestor IDs by walking parent_id chains.
            let mut ancestor_ids: std::collections::HashSet<Uuid> =
                std::collections::HashSet::new();
            for task in &results {
                let mut current = task.parent_id;
                while let Some(pid) = current {
                    if result_ids.contains(&pid) || !ancestor_ids.insert(pid) {
                        break;
                    }
                    // Look up the parent to continue the chain.
                    if let Ok(parent) = task::Entity::find_by_id(pid).one(db).await {
                        current = parent.and_then(|p| p.parent_id);
                    } else {
                        break;
                    }
                }
            }

            // Remove IDs already in results.
            let missing: Vec<Uuid> = ancestor_ids.difference(&result_ids).copied().collect();
            if !missing.is_empty() {
                let ancestors: Vec<task::Model> = task::Entity::find()
                    .filter(task::Column::Id.is_in(missing))
                    .all(db)
                    .await?;
                results.extend(ancestors);
            }
        }

        Ok(results)
    }

    async fn find_subtree(
        &self,
        root_id: Uuid,
        last_tracked_since: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Vec<task::Model>, AppError> {
        use crate::entity::task::Column;

        let db = self.db.as_ref().expect("DB not initialized");

        // Look up the root to get its path prefix.
        let root = self.find_by_id(root_id).await?;
        let root_path = root
            .path
            .clone()
            .unwrap_or_else(|| format!("/{}/", short_id(root_id)));

        // Find all tasks whose path starts with the root's path (includes root itself).
        let all: Vec<task::Model> = task::Entity::find()
            .filter(Column::Path.starts_with(&root_path))
            .filter(Column::Deleted.eq(false))
            .all(db)
            .await?;

        if last_tracked_since.is_none() {
            return Ok(all);
        }
        let since = last_tracked_since.unwrap();

        // Build a set of IDs and a parent lookup.
        let id_set: std::collections::HashSet<Uuid> = all.iter().map(|t| t.id).collect();
        let children: std::collections::HashSet<Uuid> = all
            .iter()
            .filter_map(|t| t.parent_id)
            .filter(|pid| id_set.contains(pid))
            .collect();

        // A leaf is a task that is not a parent of any other task in the subtree.
        // Keep leaves that match the filter, plus all ancestors on the path to them.
        let mut keep: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
        for t in &all {
            let is_leaf = !children.contains(&t.id);
            if !is_leaf {
                continue;
            }
            let dominated = t.last_tracked_at.map(|lta| lta >= since).unwrap_or(false);
            if !dominated {
                continue;
            }
            // Keep this leaf and all ancestors up to root.
            keep.insert(t.id);
            let mut current = t.parent_id;
            while let Some(pid) = current {
                if !keep.insert(pid) {
                    break;
                }
                current = all.iter().find(|a| a.id == pid).and_then(|a| a.parent_id);
            }
        }

        Ok(all.into_iter().filter(|t| keep.contains(&t.id)).collect())
    }

    async fn apply_batch(&self, ops: Vec<TaskOp>) -> Result<Vec<task::Model>, AppError> {
        let db = self.db.as_ref().expect("DB not initialized");
        let mut results = Vec::new();

        for op in ops {
            match op {
                TaskOp::Insert {
                    description,
                    parent_id,
                    status,
                    priority,
                } => {
                    let mut model = ActiveModel {
                        description: Set(description),
                        parent_id: Set(parent_id),
                        ..ActiveModel::new()
                    };
                    model.status = Set(status);
                    model.priority = Set(priority);
                    // Path will be set by rebuild_all_paths at the end.
                    let task = model.insert(db).await?;
                    results.push(task);
                }
                TaskOp::Update {
                    id,
                    description,
                    status,
                    priority,
                    parent_id,
                    deleted,
                } => {
                    let task = self.find_by_id(id).await?;
                    let before = task.clone();
                    let mut model: ActiveModel = task.into();
                    if let Some(d) = description {
                        model.description = Set(d);
                    }
                    if let Some(s) = status {
                        model.status = Set(s);
                    }
                    if let Some(p) = priority {
                        model.priority = Set(p);
                    }
                    if let Some(pid) = parent_id {
                        model.parent_id = Set(pid);
                    }
                    if let Some(del) = deleted {
                        model.deleted = Set(del);
                    }
                    model.updated_at = Set(chrono::Utc::now());
                    let updated = model.update(db).await?;
                    self.advance_series(db, &before, &updated).await?;
                    results.push(updated);
                }
                TaskOp::Delete { id } => {
                    self.soft_delete(id).await?;
                }
            }
        }

        // Rebuild all paths after the batch.
        self.rebuild_all_paths().await?;

        Ok(results)
    }

    async fn rebuild_all_paths(&self) -> Result<usize, AppError> {
        let db = self.db.as_ref().expect("DB not initialized");
        // Load all tasks (including deleted) to build the parent map.
        let all: Vec<task::Model> = task::Entity::find().all(db).await?;
        let parents: HashMap<Uuid, Option<Uuid>> =
            all.iter().map(|t| (t.id, t.parent_id)).collect();

        let mut count = 0;
        for t in &all {
            let new_path = compute_path(t.id, &parents);
            if t.path.as_deref() != Some(&new_path) {
                task::Entity::update_many()
                    .col_expr(task::Column::Path, Expr::value(Some(new_path)))
                    .filter(task::Column::Id.eq(t.id))
                    .exec(db)
                    .await?;
                count += 1;
            }
        }
        Ok(count)
    }
}

// Private helper methods (not part of the trait).
impl TaskRepositoryImpl {
    /// Keeps a recurring series at one open task (see `docs/plan-recurring-tasks.md`).
    /// Closing an occurrence (done or cancelled) creates its successor;
    /// reopening it withdraws that successor again if nobody has touched it.
    /// Called wherever a status is written, so every path gets it.
    async fn advance_series(
        &self,
        db: &DatabaseConnection,
        before: &task::Model,
        after: &task::Model,
    ) -> Result<(), AppError> {
        if after.deleted {
            return Ok(());
        }
        match (is_open(&before.status), is_open(&after.status)) {
            (true, false) => self.spawn_successor(db, after).await,
            (false, true) => self.withdraw_successor(db, after).await,
            _ => Ok(()),
        }
    }

    async fn spawn_successor(
        &self,
        db: &DatabaseConnection,
        closed: &task::Model,
    ) -> Result<(), AppError> {
        use crate::entity::task::Column;
        use crate::recur::Subtasks;

        // The rule was validated when it was set; one that no longer parses
        // must not make the task impossible to complete.
        let Some(rule) = closed.recur.as_deref().and_then(|r| r.parse::<Rule>().ok()) else {
            return Ok(());
        };
        let window = rule.successor(closed.updated_at, closed.not_before, &chrono::Local);
        // One instant for every stamp of the new subtree: `updated_at ==
        // created_at` is how a reopen recognises a copy nobody has touched,
        // and the shared `created_at` tells a copy from a child added later.
        let now = chrono::Utc::now();
        let successor = ActiveModel {
            id: Set(Uuid::new_v4()),
            description: Set(closed.description.clone()),
            status: Set(TaskStatus::Todo),
            deleted: Set(false),
            deleted_at: Set(None),
            priority: Set(closed.priority),
            parent_id: Set(closed.parent_id),
            created_at: Set(now),
            updated_at: Set(now),
            last_tracked_at: Set(None),
            not_before: Set(Some(window.not_before)),
            due: Set(window.due),
            recur: Set(closed.recur.clone()),
            series_id: Set(Some(closed.series_id.unwrap_or(closed.id))),
            path: Set(None),
        };
        let root = self.insert_copy(db, closed.id, successor).await?;

        // Children's windows move with the mother's; without a window of her
        // own there is nothing to measure the move by, so they stay put.
        let shift = closed.not_before.map(|nb| window.not_before - nb);
        let mut pending = vec![(closed.id, root)];
        while let Some((old_parent, new_parent)) = pending.pop() {
            let children = task::Entity::find()
                .filter(Column::ParentId.eq(old_parent))
                .filter(Column::Deleted.eq(false))
                .all(db)
                .await?;
            for child in children {
                let copied = match rule.subtasks {
                    Subtasks::All => true,
                    Subtasks::Open => is_open(&child.status),
                    Subtasks::None => false,
                };
                if !copied {
                    continue;
                }
                // A copy is a fresh start: `in_progress` claims work begun.
                let status = match child.status {
                    _ if rule.reopen => TaskStatus::Todo,
                    TaskStatus::InProgress => TaskStatus::Todo,
                    ref s => s.clone(),
                };
                let moved = |at: Option<chrono::DateTime<chrono::Utc>>| match shift {
                    Some(d) => at.map(|t| t + d),
                    None => at,
                };
                let copy = ActiveModel {
                    id: Set(Uuid::new_v4()),
                    description: Set(child.description.clone()),
                    status: Set(status),
                    deleted: Set(false),
                    deleted_at: Set(None),
                    priority: Set(child.priority),
                    parent_id: Set(Some(new_parent)),
                    created_at: Set(now),
                    updated_at: Set(now),
                    last_tracked_at: Set(None),
                    not_before: Set(moved(child.not_before)),
                    due: Set(moved(child.due)),
                    // A series of its own would get a second open task.
                    recur: Set(None),
                    series_id: Set(None),
                    path: Set(None),
                };
                let id = self.insert_copy(db, child.id, copy).await?;
                pending.push((child.id, id));
            }
        }
        Ok(())
    }

    /// Inserts `copy` (its path computed here) with the tag and project
    /// links of `source`; returns the new id.
    async fn insert_copy(
        &self,
        db: &DatabaseConnection,
        source: Uuid,
        mut copy: ActiveModel,
    ) -> Result<Uuid, AppError> {
        use crate::entity::{task_global_tag, task_project_tag};

        let id = *copy.id.as_ref();
        let parent_id = *copy.parent_id.as_ref();
        copy.path = Set(Some(child_path(db, id, parent_id).await?));
        copy.insert(db).await?;

        for link in task_global_tag::Entity::find()
            .filter(task_global_tag::Column::TaskId.eq(source))
            .all(db)
            .await?
        {
            task_global_tag::ActiveModel { task_id: Set(id), global_tag_id: Set(link.global_tag_id) }
                .insert(db)
                .await?;
        }
        for link in task_project_tag::Entity::find()
            .filter(task_project_tag::Column::TaskId.eq(source))
            .all(db)
            .await?
        {
            task_project_tag::ActiveModel { task_id: Set(id), project_tag_id: Set(link.project_tag_id) }
                .insert(db)
                .await?;
        }
        for link in task_project::Entity::find()
            .filter(task_project::Column::TaskId.eq(source))
            .all(db)
            .await?
        {
            task_project::ActiveModel { task_id: Set(id), project_id: Set(link.project_id) }
                .insert(db)
                .await?;
        }
        Ok(id)
    }

    /// Removes the successor of a reopened occurrence — the newest later task
    /// of its series — together with its copied subtree, unless any of it has
    /// been edited, tracked or started, or a child was added to it. A touched
    /// successor stays, and the series has two open tasks.
    async fn withdraw_successor(
        &self,
        db: &DatabaseConnection,
        reopened: &task::Model,
    ) -> Result<(), AppError> {
        use crate::entity::task::Column;
        use crate::entity::{task_global_tag, task_project_tag, tracking};
        use sea_orm::{PaginatorTrait, QueryOrder};

        let Some(series_id) = reopened.series_id else {
            return Ok(());
        };
        let Some(successor) = task::Entity::find()
            .filter(Column::SeriesId.eq(series_id))
            .filter(Column::Id.ne(reopened.id))
            .filter(Column::Deleted.eq(false))
            .filter(Column::CreatedAt.gt(reopened.created_at))
            .order_by_desc(Column::CreatedAt)
            .one(db)
            .await?
        else {
            return Ok(());
        };
        if successor.status != TaskStatus::Todo {
            return Ok(());
        }

        // The whole subtree, parents before children, deleted ones included:
        // deleting a copied child is touching it too.
        let mut subtree = vec![successor.clone()];
        let mut next = 0;
        while next < subtree.len() {
            let children =
                task::Entity::find().filter(Column::ParentId.eq(subtree[next].id)).all(db).await?;
            subtree.extend(children);
            next += 1;
        }
        // A child added by hand has a later `created_at` than the copies.
        let untouched = subtree.iter().all(|t| {
            t.created_at == successor.created_at
                && t.updated_at == t.created_at
                && t.last_tracked_at.is_none()
                && !t.deleted
        });
        if !untouched {
            return Ok(());
        }
        let ids: Vec<Uuid> = subtree.iter().map(|t| t.id).collect();
        let trackings = tracking::Entity::find()
            .filter(tracking::Column::TaskId.is_in(ids.clone()))
            .count(db)
            .await?;
        if trackings > 0 {
            return Ok(());
        }

        // Deleted for good, not soft-deleted: `undelete_last` would bring it
        // back as a second open task.
        task_global_tag::Entity::delete_many()
            .filter(task_global_tag::Column::TaskId.is_in(ids.clone()))
            .exec(db)
            .await?;
        task_project_tag::Entity::delete_many()
            .filter(task_project_tag::Column::TaskId.is_in(ids.clone()))
            .exec(db)
            .await?;
        task_project::Entity::delete_many()
            .filter(task_project::Column::TaskId.is_in(ids.clone()))
            .exec(db)
            .await?;
        for id in ids.into_iter().rev() {
            task::Entity::delete_by_id(id).exec(db).await?;
        }
        Ok(())
    }

    /// Rebuild paths for a task and all its descendants.
    async fn rebuild_subtree_paths(
        &self,
        db: &DatabaseConnection,
        root_id: Uuid,
    ) -> Result<(), AppError> {
        // Load all tasks to compute paths.
        let all: Vec<task::Model> = task::Entity::find().all(db).await?;
        let parents: HashMap<Uuid, Option<Uuid>> =
            all.iter().map(|t| (t.id, t.parent_id)).collect();

        // Find all descendants via BFS.
        let mut to_update = vec![root_id];
        let mut queue = vec![root_id];
        while let Some(pid) = queue.pop() {
            for t in &all {
                if t.parent_id == Some(pid) && t.id != pid {
                    to_update.push(t.id);
                    queue.push(t.id);
                }
            }
        }

        for id in to_update {
            let new_path = compute_path(id, &parents);
            task::Entity::update_many()
                .col_expr(task::Column::Path, Expr::value(Some(new_path)))
                .filter(task::Column::Id.eq(id))
                .exec(db)
                .await?;
        }
        Ok(())
    }
}
