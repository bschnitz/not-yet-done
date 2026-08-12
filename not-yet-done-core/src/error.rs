// not-yet-done-core/src/error.rs

use thiserror::Error;

/// App-shell error type. The task domain owns its own richer
/// [`not_yet_done_task_core::error::AppError`]; this slim variant covers
/// only what the shell repositories (link / settings / query_shortcut)
/// and the backup service actually surface, so that core
/// no longer needs to depend on task-core (C3 of the DB-split).
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("Database error: {0}")]
    Database(#[from] sea_orm::DbErr),

    #[error("Database is not a file-based database")]
    NotFileBasedDatabase,

    #[error("Database file not found: {0:?}")]
    DatabaseFileNotFound(std::path::PathBuf),

    #[error("Backup failed: {0}")]
    BackupFailed(String),
}
