use std::future::Future;
use std::sync::Arc;
use std::sync::OnceLock;
use tusks::tusks;

use not_yet_done_core::db;
use not_yet_done_core::module::AppModule;
use not_yet_done_core::repository::{
    ProjectRepositoryImpl, ProjectRepositoryImplParameters,
    TagRepositoryImpl, TagRepositoryImplParameters,
    TaskRepositoryImpl, TaskRepositoryImplParameters,
    TrackingRepositoryImpl, TrackingRepositoryImplParameters,
};

mod commands;

static MODULE: OnceLock<Arc<AppModule>> = OnceLock::new();

pub fn run_async<F, Fut, T>(f: F) -> T
where
    F: FnOnce(Arc<AppModule>) -> Fut,
    Fut: Future<Output = T>,
{
    let module = MODULE.get().expect("AppModule nicht initialisiert").clone();
    tokio::runtime::Runtime::new()
        .expect("tokio Runtime konnte nicht erstellt werden")
        .block_on(f(module))
}

#[tusks(root)]
#[command(name = "nyd", about = "not-yet-done — deine Todo-App")]
pub mod cli {
    #[command(about = "Task-Verwaltung")]
    pub use crate::commands::task::cli as task;

    #[command(about = "Projekt-Verwaltung")]
    pub use crate::commands::project::cli as project;


    #[command(about = "Tag-Verwaltung")]
    pub use crate::commands::tag::cli as tag;

    #[command(about = "Datenbankoperationen")]
    pub use crate::commands::db::cli as db;

    #[command(about = "Time tracking")]
    pub use crate::commands::track::cli as track;
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let sync_schema = args.windows(2).any(|w| w[0] == "db" && w[1] == "sync");

    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "sqlite://nyd.db?mode=rwc".to_string());

    let db = tokio::runtime::Runtime::new()
        .expect("tokio Runtime konnte nicht erstellt werden")
        .block_on(async { db::connect(&db_url, sync_schema).await });

    let db = match db {
        Ok(db) => db,
        Err(e) => {
            eprintln!("Datenbankverbindung fehlgeschlagen: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let module = Arc::new(
        AppModule::builder()
            .with_component_parameters::<TaskRepositoryImpl>(
                TaskRepositoryImplParameters { db: Some(db.clone()) },
            )
            .with_component_parameters::<ProjectRepositoryImpl>(
                ProjectRepositoryImplParameters { db: Some(db.clone()) },
            )
            .with_component_parameters::<TrackingRepositoryImpl>(
                TrackingRepositoryImplParameters { db: Some(db.clone()) },
            )
            .with_component_parameters::<TagRepositoryImpl>(
                TagRepositoryImplParameters { db: Some(db) },
            )
            .build(),
    );

    MODULE.set(module).unwrap_or_else(|_| panic!("MODULE bereits gesetzt"));

    std::process::ExitCode::from(cli::exec_cli().unwrap_or(0))
}
