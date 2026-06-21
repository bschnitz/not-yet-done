use std::future::Future;
use std::sync::Arc;
use std::sync::OnceLock;
use tusks::tusks;

use not_yet_done_task_core::bootstrap;
use not_yet_done_task_core::module::TaskDomainModule;

mod commands;
mod datetime;
mod offset;

static MODULE: OnceLock<Arc<TaskDomainModule>> = OnceLock::new();
static DSN: OnceLock<String> = OnceLock::new();

pub fn run_async<F, Fut, T>(f: F) -> T
where
    F: FnOnce(Arc<TaskDomainModule>) -> Fut,
    Fut: Future<Output = T>,
{
    let module = MODULE.get().expect("TaskDomainModule nicht initialisiert").clone();
    tokio::runtime::Runtime::new()
        .expect("tokio Runtime konnte nicht erstellt werden")
        .block_on(f(module))
}

/// The DSN of the task database this invocation is operating on. Resolved once
/// in `main` from `NYD_TASKS_DB` or the per-host default; the `backup` command
/// uses it so it backs up the *task* DB rather than the legacy core DB.
pub fn tasks_dsn() -> &'static str {
    DSN.get().expect("DSN nicht initialisiert")
}

// `nyd-t` is the dedicated tasks/trackings CLI: a first-class client of the
// native domain core (`not-yet-done-task-core`), not a generic adapter driver.
// It is the counterpart to `nyd` — `nyd` drives *foreign* systems (Jira,
// Confluence, Postgres, …) through the generic ContentAdapter protocol, while
// `nyd-t` owns everything that lives on our own core: tasks, time tracking,
// projects, tags, the database and its backups. The commands here produce
// typed, domain-shaped output (e.g. `track export`'s joined tracking+task JSON,
// `task tree`'s nested hierarchy) that scripts depend on.
#[tusks(root)]
#[command(name = "nyd-t", about = "not-yet-done — Tasks & Time Tracking CLI")]
pub mod cli {
    #[command(about = "Create, list, inspect, edit and delete tasks")]
    pub use crate::commands::task::cli as task;

    #[command(about = "Start, stop, summarise, export and reschedule time tracking")]
    pub use crate::commands::track::cli as track;

    #[command(about = "Manage projects that tasks can be assigned to")]
    pub use crate::commands::project::cli as project;

    #[command(about = "Manage tags (colours/symbols) for tasks, global or per project")]
    pub use crate::commands::tag::cli as tag;

    #[command(about = "Database operations (schema sync for the tasks/trackings DB)")]
    pub use crate::commands::db::cli as db;

    #[command(about = "Create, list and restore backups of the database")]
    pub use crate::commands::backup::cli as backup;
}

fn main() -> std::process::ExitCode {
    // Resolve the task DB this invocation operates on. Unlike the pre-rebuild
    // `nyd`, which read the *core* config's `database.url` (the legacy shared
    // `nyd.db`), `nyd-t` targets the split-out task database: `NYD_TASKS_DB`
    // when set, otherwise the per-host default (`<data-local>/not_yet_done/
    // tasks.db`) — the very same file the in-process Tasks/Trackings adapters
    // open when their config carries no explicit `database:` DSN.
    let dsn = std::env::var("NYD_TASKS_DB")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(bootstrap::default_task_dsn);

    // `bootstrap::open_module` connects and syncs the task-domain schema (a
    // fresh SQLite file is created and populated on first open), so there is no
    // separate `db sync` step to special-case here.
    let module = tokio::runtime::Runtime::new()
        .expect("tokio Runtime konnte nicht erstellt werden")
        .block_on(async { bootstrap::open_module(&dsn).await });

    let module = match module {
        Ok(module) => Arc::new(module),
        Err(e) => {
            eprintln!("Datenbankverbindung fehlgeschlagen: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    MODULE.set(module).unwrap_or_else(|_| panic!("MODULE bereits gesetzt"));
    DSN.set(dsn).unwrap_or_else(|_| panic!("DSN bereits gesetzt"));

    std::process::ExitCode::from(cli::exec_cli().unwrap_or(0))
}
