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
use crate::service::{ProjectService, TagService, TaskService, TrackingService};

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
    /// Project listing + CRUD (with cascade delete). Backs the Projects
    /// adapter (`project:root` / `project:item`).
    pub project_service: Arc<dyn ProjectService>,
}

/// The per-host default task DSN: `<data-local>/not_yet_done/tasks.db`
/// (e.g. `~/.local/share/not_yet_done/tasks.db`) as a `mode=rwc` SQLite DSN,
/// so a fresh file is created on first open. Falls back to the temp dir when
/// no data-local dir is known. Creates the parent directory if missing.
///
/// *Why it lives here:* the task DB's location is domain knowledge shared by
/// every consumer of the core — the in-process Tasks/Trackings content
/// adapters **and** the standalone `nyd-t` CLI. Keeping it in the core means
/// both resolve the *same* file when no explicit `database:` DSN is set,
/// instead of each carrying its own copy that could drift apart.
pub fn default_task_dsn() -> String {
    let dir = dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("not_yet_done");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("tasks.db");
    format!("sqlite://{}?mode=rwc", path.display())
}

/// Connect to `dsn`, sync the task-domain schema into it, and build the DI
/// module over the connection.
///
/// A fresh SQLite file (`sqlite://…?mode=rwc`) is created and populated with
/// the current schema on first open. See the module docs for the legacy-schema
/// caveat.
///
/// Returns the [`TaskDomainModule`] itself for callers that resolve services
/// ad hoc — the `nyd-t` CLI dispatches each subcommand to a freshly-resolved
/// service rather than holding the [`TaskDomain`] bundle. [`open`] builds on
/// this and resolves the bundle for adapter use.
pub async fn open_module(dsn: &str) -> Result<TaskDomainModule, DbErr> {
    let db = Database::connect(dsn).await?;
    db.get_schema_registry("not_yet_done_task_core::entity::*")
        .sync(&db)
        .await?;

    Ok(TaskDomainModule::builder()
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
        .build())
}

/// Connect to `dsn`, sync the task-domain schema into it, and resolve the
/// domain services over the connection.
///
/// A fresh SQLite file (`sqlite://…?mode=rwc`) is created and populated with
/// the current schema on first open. See the module docs for the legacy-schema
/// caveat.
pub async fn open(dsn: &str) -> Result<TaskDomain, DbErr> {
    let module = open_module(dsn).await?;

    Ok(TaskDomain {
        task_service: module.resolve(),
        tracking_repo: module.resolve(),
        tracking_service: module.resolve(),
        tag_service: module.resolve(),
        project_service: module.resolve(),
    })
}
