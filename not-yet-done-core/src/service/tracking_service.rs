use async_trait::async_trait;
use shaku::Component;
use std::sync::Arc;
use uuid::Uuid;

use crate::entity::tracking;
use crate::error::AppError;
use crate::repository::TrackingRepository;

#[async_trait]
pub trait TrackingService: shaku::Interface {
    /// Start tracking a task.
    ///
    /// If `parallel` is false (default), all other active trackings are stopped first.
    /// Returns an error if the task already has an active tracking.
    async fn start(
        &self,
        task_id: Uuid,
        parallel: bool,
    ) -> Result<tracking::Model, AppError>;
}

#[derive(Component)]
#[shaku(interface = TrackingService)]
pub struct TrackingServiceImpl {
    #[shaku(inject)]
    tracking_repository: Arc<dyn TrackingRepository>,
}

#[async_trait]
impl TrackingService for TrackingServiceImpl {
    async fn start(
        &self,
        task_id: Uuid,
        parallel: bool,
    ) -> Result<tracking::Model, AppError> {
        // Guard: task must not already have an active tracking
        if let Some(_) = self.tracking_repository.find_active_for_task(task_id).await? {
            return Err(AppError::TrackingAlreadyActive(task_id));
        }

        let now = chrono::Utc::now();

        if !parallel {
            // Stop all other active trackings
            let active = self.tracking_repository.find_all_active().await?;
            for t in active {
                if t.task_id != task_id {
                    self.tracking_repository.stop(t.id, now).await?;
                }
            }
        }

        self.tracking_repository.insert(task_id, now, None).await
    }
}
