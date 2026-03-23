use async_trait::async_trait;
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ColumnTrait, DatabaseConnection,
    EntityTrait, QueryFilter, Set, Condition
};
use shaku::Component;
use uuid::Uuid;

use crate::entity::tracking::{self, ActiveModel};
use crate::error::AppError;

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
}

#[derive(Component)]
#[shaku(interface = TrackingRepository)]
pub struct TrackingRepositoryImpl {
    #[shaku(default)]
    db: Option<DatabaseConnection>,
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
        Ok(model.insert(db).await?)
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
        let mut model: tracking::ActiveModel = tracking.into();
        model.ended_at = Set(Some(ended_at));
        model.deleted = Set(true);
        Ok(model.update(db).await?)
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
            .one(db).await?
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
}
