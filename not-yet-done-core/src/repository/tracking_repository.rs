use async_trait::async_trait;
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ColumnTrait, DatabaseConnection,
    EntityTrait, QueryFilter, Set,
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
}
