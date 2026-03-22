use tusks::tusks;

#[tusks()]
#[command(about = "Manage time tracking")]
pub mod cli {
    pub use crate::cli as parent_;

    /// Start tracking time for a task.
    ///
    /// By default all other active trackings are stopped before starting the new one.
    /// Use --parallel to keep existing trackings running. Note that each task can only
    /// have one active tracking at a time — starting a task that is already being tracked
    /// will return an error regardless of --parallel.
    pub fn start(
        #[arg(help = "Task ID to start tracking")] task_id: String,
        #[arg(
            long,
            help = "Keep other tasks' active trackings running instead of stopping them"
        )]
        parallel: bool,
    ) -> u8 {
        let result = crate::run_async(|module| async move {
            use shaku::HasComponent;
            use not_yet_done_core::service::TrackingService;
            use sea_orm::prelude::Uuid;
            let task_id = Uuid::parse_str(&task_id)
                .map_err(|_| not_yet_done_core::error::AppError::InvalidId(task_id))?;
            let service: &dyn TrackingService = module.resolve_ref();
            service.start(task_id, parallel).await
        });
        match result {
            Ok(tracking) => {
                println!("✓ Tracking started: [{}] started at {}", tracking.id, tracking.started_at);
                0
            }
            Err(e) => { eprintln!("Error: {e}"); 1 }
        }
    }
}
