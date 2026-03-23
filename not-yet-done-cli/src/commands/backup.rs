use tusks::tusks;

#[tusks()]
pub mod cli {
    pub use crate::cli as parent_;

    #[command(about = "Create a backup of the database")]
    pub fn create() -> u8 {
        let result = crate::run_async(|module| async move {
            use shaku::HasComponent;
            use not_yet_done_core::service::BackupService;
            let service: &dyn BackupService = module.resolve_ref();
            service.create_backup().await
        });
        match result {
            Ok(path) => { println!("✓ Backup erstellt: {}", path); 0 }
            Err(e)   => { eprintln!("Error: {e}"); 1 }
        }
    }

    /// List all available backups
    #[command(about = "List all available backups")]
    pub fn list() -> u8 {
        let result = crate::run_async(|module| async move {
            use shaku::HasComponent;
            use not_yet_done_core::service::BackupService;
            let service: &dyn BackupService = module.resolve_ref();
            service.list_backups().await
        });
        match result {
            Ok(backups) if backups.is_empty() => {
                println!("No backups found.");
                0
            }
            Ok(backups) => {
                for backup in backups {
                    println!("{}", backup);
                }
                0
            }
            Err(e) => { eprintln!("Error: {e}"); 1 }
        }
    }

    /// Restore database from a backup file
    #[command(about = "Restore database from a backup file")]
    pub fn restore(
        #[arg(help = "Backup filename (e.g., 20260323-185627-nyd.db)")] filename: String,
    ) -> u8 {
        let result = crate::run_async(|module| async move {
            use shaku::HasComponent;
            use not_yet_done_core::service::BackupService;
            let service: &dyn BackupService = module.resolve_ref();
            service.restore_backup(&filename).await
        });
        match result {
            Ok(path) => {
                println!("✓ Datenbank wiederhergestellt: {}", path);
                0
            }
            Err(e) => { eprintln!("Error: {e}"); 1 }
        }
    }
}
