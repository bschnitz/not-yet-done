use async_trait::async_trait;
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ColumnTrait, Condition, DatabaseConnection, EntityTrait,
    QueryFilter, Set,
};
use shaku::Component;
use uuid::Uuid;

use crate::entity::tracking::{self, ActiveModel};
use crate::error::AppError;
use crate::filter::{ColumnRegistry, FilterExpr};

#[async_trait]
pub trait TrackingRepository: shaku::Interface {
    /// Insert a new tracking entry
    async fn insert(
        &self,
        task_id: Uuid,
        started_at: chrono::DateTime<chrono::Utc>,
        predecessor_id: Option<Uuid>,
    ) -> Result<tracking::Model, AppError>;

    /// Find the single active (not deleted, no ended_at) tracking for a task
    async fn find_active_for_task(
        &self,
        task_id: Uuid,
    ) -> Result<Option<tracking::Model>, AppError>;

    /// Find all active trackings across all tasks
    async fn find_all_active(&self) -> Result<Vec<tracking::Model>, AppError>;

    /// Stop a tracking by setting ended_at and marking as deleted
    async fn stop(
        &self,
        id: Uuid,
        ended_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<tracking::Model, AppError>;

    async fn find_in_range(
        &self,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
        task_id: Option<Uuid>,
    ) -> Result<Vec<tracking::Model>, AppError>;

    /// Find all non-deleted, completed trackings that overlap with [start, end],
    /// excluding the tracking with the given id.
    async fn find_overlapping(
        &self,
        start: chrono::DateTime<chrono::Utc>,
        end: chrono::DateTime<chrono::Utc>,
        exclude_id: Uuid,
    ) -> Result<Vec<tracking::Model>, AppError>;

    /// Soft-delete a tracking without changing started_at/ended_at
    async fn soft_delete_keeping_times(&self, id: Uuid) -> Result<(), AppError>;

    /// Insert a completed tracking with explicit end time and optional predecessor
    async fn insert_with_end(
        &self,
        task_id: Uuid,
        started_at: chrono::DateTime<chrono::Utc>,
        ended_at: chrono::DateTime<chrono::Utc>,
        predecessor_id: Option<Uuid>,
    ) -> Result<tracking::Model, AppError>;

    /// Find a tracking by id (including deleted)
    async fn find_by_id(&self, id: Uuid) -> Result<Option<tracking::Model>, AppError>;

    /// Find all non-deleted trackings, newest first.
    async fn find_all(&self) -> Result<Vec<tracking::Model>, AppError>;

    /// Find *every* tracking, deleted or not, newest first. This is the
    /// unfiltered universe — callers that want only the live set apply a
    /// `deleted = false` query filter on top. Adapters load this so the
    /// query is the single, replaceable filter rather than stacking on a
    /// baked-in `deleted = false`.
    async fn find_all_including_deleted(&self) -> Result<Vec<tracking::Model>, AppError>;

    /// Set deleted = false on a tracking.
    async fn undelete(&self, id: Uuid) -> Result<(), AppError>;

    /// Find all trackings (including deleted) that have the given predecessor_id.
    async fn find_by_predecessor(
        &self,
        predecessor_id: Uuid,
    ) -> Result<Vec<tracking::Model>, AppError>;

    /// Hard-delete a tracking from the database.
    async fn hard_delete(&self, id: Uuid) -> Result<(), AppError>;

    /// Find trackings matching a filter expression.
    /// Supports tracking columns + `description` (maps to joined task.description).
    async fn find_filtered(&self, expr: &FilterExpr) -> Result<Vec<tracking::Model>, AppError>;
}

#[derive(Component)]
#[shaku(interface = TrackingRepository)]
pub struct TrackingRepositoryImpl {
    #[shaku(default)]
    db: Option<DatabaseConnection>,
}

impl TrackingRepositoryImpl {
    /// Update the task's last_tracked_at to the given timestamp if it's newer.
    async fn update_last_tracked_at(
        db: &DatabaseConnection,
        task_id: Uuid,
        at: chrono::DateTime<chrono::Utc>,
    ) {
        use crate::entity::task;
        use sea_orm::sea_query::Expr;
        // Only update if the new timestamp is newer than the current one.
        let _ = task::Entity::update_many()
            .col_expr(task::Column::LastTrackedAt, Expr::value(Some(at)))
            .filter(task::Column::Id.eq(task_id))
            .filter(
                sea_orm::Condition::any()
                    .add(task::Column::LastTrackedAt.is_null())
                    .add(task::Column::LastTrackedAt.lt(at)),
            )
            .exec(db)
            .await;
    }
}

#[async_trait]
impl TrackingRepository for TrackingRepositoryImpl {
    async fn insert(
        &self,
        task_id: Uuid,
        started_at: chrono::DateTime<chrono::Utc>,
        predecessor_id: Option<Uuid>,
    ) -> Result<tracking::Model, AppError> {
        let db = self.db.as_ref().expect("DB not initialized");
        let model = ActiveModel {
            task_id: Set(task_id),
            started_at: Set(started_at),
            predecessor_id: Set(predecessor_id),
            ..ActiveModel::new()
        };
        let result = model.insert(db).await?;
        // Update task.last_tracked_at.
        Self::update_last_tracked_at(db, task_id, started_at).await;
        Ok(result)
    }

    async fn find_active_for_task(
        &self,
        task_id: Uuid,
    ) -> Result<Option<tracking::Model>, AppError> {
        let db = self.db.as_ref().expect("DB not initialized");
        Ok(tracking::Entity::find()
            .filter(tracking::Column::TaskId.eq(task_id))
            .filter(tracking::Column::Deleted.eq(false))
            .filter(tracking::Column::EndedAt.is_null())
            .one(db)
            .await?)
    }

    async fn find_all_active(&self) -> Result<Vec<tracking::Model>, AppError> {
        let db = self.db.as_ref().expect("DB not initialized");
        Ok(tracking::Entity::find()
            .filter(tracking::Column::Deleted.eq(false))
            .filter(tracking::Column::EndedAt.is_null())
            .all(db)
            .await?)
    }

    async fn stop(
        &self,
        id: Uuid,
        ended_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<tracking::Model, AppError> {
        let db = self.db.as_ref().expect("DB not initialized");
        let tracking = tracking::Entity::find_by_id(id)
            .one(db)
            .await?
            .ok_or(AppError::TrackingNotFound(id))?;
        let task_id = tracking.task_id;
        let mut model: tracking::ActiveModel = tracking.into();
        model.ended_at = Set(Some(ended_at));
        let result = model.update(db).await?;
        // Update task.last_tracked_at.
        Self::update_last_tracked_at(db, task_id, ended_at).await;
        Ok(result)
    }

    async fn find_in_range(
        &self,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
        task_id: Option<Uuid>,
    ) -> Result<Vec<tracking::Model>, AppError> {
        let db = self.db.as_ref().expect("DB not initialized");

        let mut query = tracking::Entity::find()
            .filter(tracking::Column::Deleted.eq(false))
            .filter(tracking::Column::StartedAt.lt(to))
            .filter(
                Condition::any()
                    .add(tracking::Column::EndedAt.gt(from))
                    .add(tracking::Column::EndedAt.is_null()),
            );

        if let Some(tid) = task_id {
            query = query.filter(tracking::Column::TaskId.eq(tid));
        }

        Ok(query.all(db).await?)
    }

    async fn find_overlapping(
        &self,
        start: chrono::DateTime<chrono::Utc>,
        end: chrono::DateTime<chrono::Utc>,
        exclude_id: Uuid,
    ) -> Result<Vec<tracking::Model>, AppError> {
        let db = self.db.as_ref().expect("DB not initialized");
        Ok(tracking::Entity::find()
            .filter(tracking::Column::Deleted.eq(false))
            .filter(tracking::Column::Id.ne(exclude_id))
            .filter(tracking::Column::StartedAt.lt(end))
            .filter(
                Condition::any()
                    .add(tracking::Column::EndedAt.gt(start))
                    .add(tracking::Column::EndedAt.is_null()),
            )
            .all(db)
            .await?)
    }

    async fn soft_delete_keeping_times(&self, id: Uuid) -> Result<(), AppError> {
        let db = self.db.as_ref().expect("DB not initialized");
        let tracking = tracking::Entity::find_by_id(id)
            .one(db)
            .await?
            .ok_or(AppError::TrackingNotFound(id))?;
        let mut model: tracking::ActiveModel = tracking.into();
        model.deleted = Set(true);
        model.update(db).await?;
        Ok(())
    }

    async fn insert_with_end(
        &self,
        task_id: Uuid,
        started_at: chrono::DateTime<chrono::Utc>,
        ended_at: chrono::DateTime<chrono::Utc>,
        predecessor_id: Option<Uuid>,
    ) -> Result<tracking::Model, AppError> {
        let db = self.db.as_ref().expect("DB not initialized");
        let model = ActiveModel {
            task_id: Set(task_id),
            started_at: Set(started_at),
            ended_at: Set(Some(ended_at)),
            predecessor_id: Set(predecessor_id),
            ..ActiveModel::new()
        };
        Ok(model.insert(db).await?)
    }

    async fn find_by_id(&self, id: Uuid) -> Result<Option<tracking::Model>, AppError> {
        let db = self.db.as_ref().expect("DB not initialized");
        Ok(tracking::Entity::find_by_id(id).one(db).await?)
    }

    async fn find_all(&self) -> Result<Vec<tracking::Model>, AppError> {
        use sea_orm::QueryOrder;
        let db = self.db.as_ref().expect("DB not initialized");
        Ok(tracking::Entity::find()
            .filter(tracking::Column::Deleted.eq(false))
            .order_by_desc(tracking::Column::StartedAt)
            .all(db)
            .await?)
    }

    async fn find_all_including_deleted(&self) -> Result<Vec<tracking::Model>, AppError> {
        use sea_orm::QueryOrder;
        let db = self.db.as_ref().expect("DB not initialized");
        Ok(tracking::Entity::find()
            .order_by_desc(tracking::Column::StartedAt)
            .all(db)
            .await?)
    }

    async fn find_filtered(&self, expr: &FilterExpr) -> Result<Vec<tracking::Model>, AppError> {
        use sea_orm::{JoinType, QueryOrder, QuerySelect};

        let db = self.db.as_ref().expect("DB not initialized");
        let resolved = crate::filter::tree_ops::resolve_tree_operators(expr, db).await?;
        let registry = TrackingWithTaskRegistry;
        let condition = crate::filter::FilterBuilder::new(&registry).build(&resolved)?;

        Ok(tracking::Entity::find()
            .join(
                JoinType::LeftJoin,
                tracking::Entity::belongs_to(crate::entity::task::Entity)
                    .from(tracking::Column::TaskId)
                    .to(crate::entity::task::Column::Id)
                    .into(),
            )
            .filter(condition)
            .order_by_desc(tracking::Column::StartedAt)
            .all(db)
            .await?)
    }

    async fn undelete(&self, id: Uuid) -> Result<(), AppError> {
        let db = self.db.as_ref().expect("DB not initialized");
        let tracking = tracking::Entity::find_by_id(id)
            .one(db)
            .await?
            .ok_or(AppError::TrackingNotFound(id))?;
        let mut model: tracking::ActiveModel = tracking.into();
        model.deleted = Set(false);
        model.update(db).await?;
        Ok(())
    }

    async fn find_by_predecessor(
        &self,
        predecessor_id: Uuid,
    ) -> Result<Vec<tracking::Model>, AppError> {
        let db = self.db.as_ref().expect("DB not initialized");
        Ok(tracking::Entity::find()
            .filter(tracking::Column::PredecessorId.eq(predecessor_id))
            .all(db)
            .await?)
    }

    async fn hard_delete(&self, id: Uuid) -> Result<(), AppError> {
        use sea_orm::ModelTrait;
        let db = self.db.as_ref().expect("DB not initialized");
        if let Some(tracking) = tracking::Entity::find_by_id(id).one(db).await? {
            tracking.delete(db).await?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Composite column registry for tracking queries with task join
// ---------------------------------------------------------------------------

/// Resolves both tracking and task columns for JOIN queries.
///
/// - Unqualified columns (no table prefix) resolve to tracking fields,
///   except `description` which maps to `task.description` for backward compat.
/// - Columns prefixed with `t.` or `task.` resolve to task fields.
/// - Columns prefixed with `tracking.` resolve to tracking fields.
struct TrackingWithTaskRegistry;

impl TrackingWithTaskRegistry {
    const TRACKING_COLS: &'static [&'static str] = &[
        "id",
        "task_id",
        "predecessor_id",
        "started_at",
        "ended_at",
        "deleted",
        "created_at",
    ];
    const TASK_COLS: &'static [&'static str] = &[
        "id",
        "description",
        "status",
        "deleted",
        "deleted_at",
        "priority",
        "parent_id",
        "created_at",
        "updated_at",
        "last_tracked_at",
        "path",
    ];
}

impl ColumnRegistry for TrackingWithTaskRegistry {
    fn resolve(&self, table: Option<&str>, column: &str) -> Option<sea_orm::sea_query::ColumnRef> {
        use sea_orm::sea_query::{Alias, ColumnName, ColumnRef, IntoIden, TableName};

        let qualified = |tbl: &str, col: &str| -> ColumnRef {
            ColumnRef::Column(ColumnName(
                Some(TableName(None, Alias::new(tbl).into_iden())),
                Alias::new(col).into_iden(),
            ))
        };

        match table {
            // Explicitly qualified as task column.
            Some("t") | Some("task") => {
                if Self::TASK_COLS.contains(&column) {
                    Some(qualified("task", column))
                } else {
                    None
                }
            }
            // Explicitly qualified as tracking column.
            Some("tracking") => {
                if Self::TRACKING_COLS.contains(&column) {
                    Some(qualified("tracking", column))
                } else {
                    None
                }
            }
            // Unqualified: tracking fields first, then `description` as task fallback.
            None | Some("") => {
                if Self::TRACKING_COLS.contains(&column) {
                    Some(qualified("tracking", column))
                } else if column == "description" {
                    // Backward compat: unqualified description → task.description.
                    Some(qualified("task", "description"))
                } else if column == "path" {
                    // Tree queries: unqualified path → task.path.
                    Some(qualified("task", "path"))
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}
