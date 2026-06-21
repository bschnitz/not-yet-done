//! Self-contained database bootstrap for the task domain.
//!
//! [`open`] connects to a DSN, syncs the task-domain schema, and builds a
//! [`TaskDomainModule`](crate::module::TaskDomainModule) over the connection,
//! returning the resolved services a host needs. This is what lets the
//! in-process Tasks/Trackings content adapters be **self-contained** (Phase C4
//! of the DB-split): they receive a `database:` DSN in their config and call
//! [`open`] themselves, instead of having concrete services threaded in from
//! the App (the pre-C4 `CoreHandle` model).
//!
//! Scope: schema **sync** only — this creates the current schema on a fresh
//! database. Upgrading a *legacy*-schema database is deliberately not done
//! here; that path stays in `not_yet_done_core::db::connect` for the
//! shared-database era, and the DB-split data migration (C6) moves
//! already-current rows into a fresh task database.

use std::sync::Arc;

use sea_orm::{Database, DbErr};
use shaku::HasComponent;

use crate::module::TaskDomainModule;
use crate::repository::{
    ProjectRepositoryImpl, ProjectRepositoryImplParameters, TagRepositoryImpl,
    TagRepositoryImplParameters, TaskRepositoryImpl, TaskRepositoryImplParameters,
    TrackingRepository, TrackingRepositoryImpl, TrackingRepositoryImplParameters,
};
use crate::service::{TagService, TaskService, TrackingService};

/// The task-domain services resolved over one database connection.
///
/// Cloneable (`Arc` handles) so the caller can share them across the adapter
/// and its background bridges.
#[derive(Clone)]
pub struct TaskDomain {
    pub task_service: Arc<dyn TaskService>,
    pub tracking_repo: Arc<dyn TrackingRepository>,
    /// High-level tracking operations (split/move with gravity, overlap and
    /// future guards) the repository alone doesn't encapsulate. The Trackings
    /// adapter's `split`/`move` actions delegate to it.
    pub tracking_service: Arc<dyn TrackingService>,
    pub tag_service: Arc<dyn TagService>,
}

/// Connect to `dsn`, sync the task-domain schema into it, and resolve the
/// domain services over the connection.
///
/// A fresh SQLite file (`sqlite://…?mode=rwc`) is created and populated with
/// the current schema on first open. See the module docs for the legacy-schema
/// caveat.
pub async fn open(dsn: &str) -> Result<TaskDomain, DbErr> {
    let db = Database::connect(dsn).await?;
    db.get_schema_registry("not_yet_done_task_core::entity::*")
        .sync(&db)
        .await?;

    let module = TaskDomainModule::builder()
        .with_component_parameters::<TaskRepositoryImpl>(TaskRepositoryImplParameters {
            db: Some(db.clone()),
        })
        .with_component_parameters::<ProjectRepositoryImpl>(ProjectRepositoryImplParameters {
            db: Some(db.clone()),
        })
        .with_component_parameters::<TagRepositoryImpl>(TagRepositoryImplParameters {
            db: Some(db.clone()),
        })
        .with_component_parameters::<TrackingRepositoryImpl>(TrackingRepositoryImplParameters {
            db: Some(db.clone()),
        })
        .build();

    Ok(TaskDomain {
        task_service: module.resolve(),
        tracking_repo: module.resolve(),
        tracking_service: module.resolve(),
        tag_service: module.resolve(),
    })
}
