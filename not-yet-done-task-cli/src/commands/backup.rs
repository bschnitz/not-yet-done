use tusks::tusks;

/// Default retention for `nyd-t backup create`: keep the newest N timestamped
/// copies of *this* database (suffix-aware, so unrelated DBs in the same dir
/// are untouched). Matches the in-process adapter/TUI default so backups made
/// from either front-end prune to the same bound.
const MAX_BACKUPS: usize = 10;

#[tusks()]
pub mod cli {
    pub use crate::cli as parent_;
    use not_yet_done_task_core::backup;

    #[command(about = "Create a backup of the tasks/trackings database")]
    pub fn create() -> u8 {
        // A backup is a plain timestamped file copy — synchronous, no DB
        // connection needed. It targets the *task* DB this invocation operates
        // on (`tasks_dsn`), via task-core's own backup module, so `nyd-t` no
        // longer reaches into the legacy `not-yet-done-core` app shell.
        let dir = backup::default_backup_dir();
        match backup::create_backup_at(crate::tasks_dsn(), &dir, super::MAX_BACKUPS) {
            Ok(path) => {
                println!("✓ Backup erstellt: {}", path);
                0
            }
            Err(e) => {
                eprintln!("Error: {e}");
                1
            }
        }
    }

    /// List all available backups
    #[command(about = "List all available backups")]
    pub fn list() -> u8 {
        let dir = backup::default_backup_dir();
        match backup::list_backups(&dir) {
            Ok(backups) if backups.is_empty() => {
                println!("No backups found.");
                0
            }
            Ok(backups) => {
                for b in backups {
                    println!("{b}");
                }
                0
            }
            Err(e) => {
                eprintln!("Error: {e}");
                1
            }
        }
    }

    /// Restore database from a backup file
    #[command(about = "Restore database from a backup file")]
    pub fn restore(
        #[arg(help = "Backup filename (e.g., 20260323-185627-tasks.db)")] filename: String,
    ) -> u8 {
        // Restore into the *task* DB this invocation operates on.
        let dir = backup::default_backup_dir();
        match backup::restore_backup_at(crate::tasks_dsn(), &dir, &filename) {
            Ok(path) => {
                println!("✓ Datenbank wiederhergestellt: {}", path);
                0
            }
            Err(e) => {
                eprintln!("Error: {e}");
                1
            }
        }
    }
}
