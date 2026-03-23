use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("Database error: {0}")]
    Database(#[from] sea_orm::DbErr),

    #[error("Task not found: {0}")]
    TaskNotFound(Uuid),

    #[error("Project not found: {0}")]
    ProjectNotFound(Uuid),

    #[error("Tag not found: {0}")]
    TagNotFound(String),

    #[error("Tag name '{name}' already exists as a global tag: [global-tag:{id}]")]
    DuplicateGlobalTag { name: String, id: Uuid },

    #[error("Tag name '{name}' already exists in this project: [project-tag:{id}]")]
    DuplicateProjectTag { name: String, id: Uuid },

    #[error("Tag name '{0}' is ambiguous: it exists in multiple projects. Please use the full ID: project-tag:<id> or global-tag:<id>")]
    AmbiguousTag(String),

    #[error("Tracking not found: {0}")]
    TrackingNotFound(Uuid),

    #[error("Task {0} already has an active tracking — stop it first before starting a new one")]
    TrackingAlreadyActive(Uuid),

    #[error("No active tracking found for task {0}")]
    NoActiveTracking(Uuid),

    #[error("Invalid ID: {0}")]
    InvalidId(String),

    #[error("Invalid color (expected hex string like #FF5733): {0}")]
    InvalidColor(String),

    #[error("Invalid status transition: {0}")]
    InvalidStatusTransition(String),

    #[error("No active trackings found")]
    NoActiveTrackingAny,

    #[error("Tracking {0} is still active — only completed trackings can be moved")]
    TrackingStillActive(Uuid),

    #[error("Moving tracking would place it in the future — use --allow-future to override")]
    TrackingInFuture,

    #[error("Tracking would overlap with another tracking of the same task")]
    OverlapSameTask,

    #[error("Tracking would overlap with existing trackings — use --allow-overlap to override")]
    OverlapOtherTask,

    #[error("No free slot found to place the tracking")]
    NoFreeSlot,
}
