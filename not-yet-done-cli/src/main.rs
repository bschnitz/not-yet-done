use std::future::Future;
use std::sync::Arc;
use std::sync::OnceLock;
use tusks::tusks;

use not_yet_done_core::config::ConfigServiceImpl;
use not_yet_done_core::db;
use not_yet_done_task_core::module::TaskDomainModule;
use not_yet_done_task_core::repository::{
    ProjectRepositoryImpl, ProjectRepositoryImplParameters, TagRepositoryImpl,
    TagRepositoryImplParameters, TaskRepositoryImpl, TaskRepositoryImplParameters,
    TrackingRepositoryImpl, TrackingRepositoryImplParameters,
};

mod adapter_cli;
mod adapter_connect;
mod adapter_query;
mod cli_config;
mod commands;
mod config_auth;
mod config_gen;
mod config_template;
mod thunderbird;

static MODULE: OnceLock<Arc<TaskDomainModule>> = OnceLock::new();

/// Point the CLI's diagnostics at the same rotating log the TUI writes, read
/// from the same `config.yaml`.
///
/// The CLI is where a TUI failure gets reproduced -- headless, with the same
/// adapters and the same session -- so it is the last place that should be
/// logging into the void. It used to: only the TUI called `configure`, and a
/// CLI run wrote nothing unless `NYD_DEBUG` was set by hand, which one only
/// remembers to do *after* the interesting run.
///
/// First thing in `main`, before the adapter dispatch below, because that is
/// the path that talks to the network. Read-only and silent: no config, or an
/// unreadable one, simply leaves logging off rather than failing the command,
/// and the interactive "shall I create one?" branch of `get_config` is
/// deliberately not taken -- installing a log file is no reason to stop and
/// ask a question.
fn install_logging() {
    let Ok(config) = ConfigServiceImpl::read_config() else {
        return;
    };
    not_yet_done_content::http_log::configure(not_yet_done_content::http_log::LogSettings {
        enabled: config.logging.enabled,
        directory: config.logging.directory.clone(),
        retention_days: config.logging.retention_days as i64,
        verbose: config.logging.verbose,
    });
}

pub fn run_async<F, Fut, T>(f: F) -> T
where
    F: FnOnce(Arc<TaskDomainModule>) -> Fut,
    Fut: Future<Output = T>,
{
    let module = MODULE
        .get()
        .expect("TaskDomainModule not initialized")
        .clone();
    tokio::runtime::Runtime::new()
        .expect("failed to create the tokio runtime")
        .block_on(f(module))
}

// The CLI is, by design, a thin generic front-end over the ContentAdapter
// protocol (Block D): tasks, trackings *and* projects are reached as adapter
// instances (`nyd tasks …`, `nyd projects do create …`), and the terse
// everyday forms live as `cli.yaml` aliases (see `cli_config::DEFAULT_ALIASES`),
// not as hard-coded subcommands. The only built-in `tusks` commands left are
// the two that have no adapter path yet:
//   * `tag`    — tag CRUD with fg/bg/symbol styling; the task adapter only
//                exposes tags as a value source + per-task mutation, so the
//                standalone management UI still lives here.
//   * `backup` — backup/list/restore of the legacy core DB; stays until D4
//                turns it into a `tasks.db` adapter action.
// The former `task` / `project` / `track` / `query` / `db sync` commands were
// removed once their adapter replacements landed (D3b-1..3); those names are
// now free for adapter instances or aliases (see `adapter_cli::BUILTIN_COMMANDS`).
#[tusks(root)]
#[command(name = "nyd", about = "not-yet-done — deine Todo-App")]
pub mod cli {
    #[command(about = "Tag-Verwaltung")]
    pub use crate::commands::tag::cli as tag;

    #[command(about = "Backup erstellen")]
    pub use crate::commands::backup::cli as backup;
}

fn main() -> std::process::ExitCode {
    install_logging();

    let args: Vec<String> = std::env::args().collect();

    // Generic adapter front-end (Block D): if the first argument names a
    // configured adapter instance, drive the ContentAdapter protocol directly
    // and skip the legacy task-core path entirely (no nyd.db connection needed).
    if let Some(code) = adapter_cli::try_dispatch(&args) {
        return code;
    }

    let config_service = ConfigServiceImpl::new();

    let db_url = tokio::runtime::Runtime::new()
        .expect("failed to create the tokio runtime")
        .block_on(async { config_service.get_database_url().await });

    let db_url = match db_url {
        Ok(url) => url,
        Err(e) => {
            eprintln!("Configuration error: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    // The remaining built-in commands (`tag`, `backup`) operate on an existing
    // core DB; schema sync was only ever triggered by the now-removed
    // `db sync`, so connect without it.
    let db = tokio::runtime::Runtime::new()
        .expect("failed to create the tokio runtime")
        .block_on(async { db::connect(&db_url, false).await });

    let db = match db {
        Ok(db) => db,
        Err(e) => {
            eprintln!("Database connection failed: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let module = Arc::new(
        TaskDomainModule::builder()
            .with_component_parameters::<TaskRepositoryImpl>(TaskRepositoryImplParameters {
                db: Some(db.clone()),
            })
            .with_component_parameters::<ProjectRepositoryImpl>(ProjectRepositoryImplParameters {
                db: Some(db.clone()),
            })
            .with_component_parameters::<TrackingRepositoryImpl>(TrackingRepositoryImplParameters {
                db: Some(db.clone()),
            })
            .with_component_parameters::<TagRepositoryImpl>(TagRepositoryImplParameters {
                db: Some(db),
            })
            .build(),
    );

    MODULE
        .set(module)
        .unwrap_or_else(|_| panic!("MODULE already set"));

    std::process::ExitCode::from(cli::exec_cli().unwrap_or(0))
}
