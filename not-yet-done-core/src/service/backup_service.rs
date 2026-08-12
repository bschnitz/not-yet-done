use async_trait::async_trait;
use chrono::Utc;
use shaku::Component;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::ConfigServiceImpl;
use crate::error::CoreError;

#[async_trait]
pub trait BackupService: shaku::Interface {
    async fn create_backup(&self) -> Result<String, CoreError>;
    async fn list_backups(&self) -> Result<Vec<String>, CoreError>;
    async fn restore_backup(&self, filename: &str) -> Result<String, CoreError>;
}

#[derive(Component)]
#[shaku(interface = BackupService)]
pub struct BackupServiceImpl;

impl BackupServiceImpl {
    fn extract_db_path(db_url: &str) -> Result<PathBuf, CoreError> {
        if db_url.starts_with("sqlite://") {
            let path_str = db_url
                .strip_prefix("sqlite://")
                .ok_or_else(|| CoreError::NotFileBasedDatabase)?;

            let path_str = path_str
                .split('?')
                .next()
                .ok_or_else(|| CoreError::NotFileBasedDatabase)?;

            let path = Path::new(path_str);

            if !path.exists() {
                return Err(CoreError::DatabaseFileNotFound(path.to_path_buf()));
            }

            if !path.is_file() {
                return Err(CoreError::NotFileBasedDatabase);
            }

            Ok(path.to_path_buf())
        } else {
            Err(CoreError::NotFileBasedDatabase)
        }
    }

    fn generate_backup_filename(original_name: &str) -> String {
        let timestamp = Utc::now().format("%Y%m%d-%H%M%S");
        format!("{}-{}", timestamp, original_name)
    }

    /// Back up the database at an explicit `db_url`, instead of the one in the
    /// core config.
    ///
    /// *Why it exists:* after the DB-split the task domain lives in its own
    /// `tasks.db`, separate from the legacy core DB. The `nyd-t` CLI operates
    /// on that task DB and backs *it* up, while the trait's [`create_backup`]
    /// (used by the TUI's daily backup) still targets the config DB. Both share
    /// the same backup directory, timestamp scheme and retention policy.
    pub async fn create_backup_at(&self, db_url: &str) -> Result<String, CoreError> {
        let config_service = ConfigServiceImpl::new();
        let config = config_service
            .get_config()
            .await
            .map_err(|e| CoreError::BackupFailed(format!("Failed to get config: {}", e)))?;

        let db_path = Self::extract_db_path(db_url)?;

        let original_filename = db_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| CoreError::BackupFailed("Invalid database filename".to_string()))?;

        let backup_filename = Self::generate_backup_filename(original_filename);
        let backup_path = config.backup.directory.join(&backup_filename);

        fs::copy(&db_path, &backup_path)
            .map_err(|e| CoreError::BackupFailed(format!("Failed to copy database file: {}", e)))?;

        Self::cleanup_old_backups(&config.backup.directory, config.backup.max_count)?;

        Ok(backup_path.to_string_lossy().to_string())
    }

    /// Restore a backup file into an explicit `db_url`, instead of the one in
    /// the core config. Counterpart to [`create_backup_at`].
    pub async fn restore_backup_at(
        &self,
        db_url: &str,
        filename: &str,
    ) -> Result<String, CoreError> {
        let config_service = ConfigServiceImpl::new();
        let config = config_service
            .get_config()
            .await
            .map_err(|e| CoreError::BackupFailed(format!("Failed to get config: {}", e)))?;

        let db_path = Self::extract_db_path(db_url)?;
        let backup_path = config.backup.directory.join(filename);

        if !backup_path.exists() {
            return Err(CoreError::BackupFailed(format!(
                "Backup file not found: {}",
                filename
            )));
        }

        if !backup_path.is_file() {
            return Err(CoreError::BackupFailed(format!(
                "Backup is not a file: {}",
                filename
            )));
        }

        fs::copy(&backup_path, &db_path).map_err(|e| {
            CoreError::BackupFailed(format!("Failed to restore database file: {}", e))
        })?;

        Ok(db_path.to_string_lossy().to_string())
    }

    /// Create a backup if none exists for today. Returns the path if a new
    /// backup was created, or None if one already existed.
    pub async fn ensure_daily_backup(&self) -> Result<Option<String>, CoreError> {
        let today_prefix = Utc::now().format("%Y%m%d").to_string();
        let backups = self.list_backups().await?;
        if backups.iter().any(|b| b.starts_with(&today_prefix)) {
            return Ok(None);
        }
        self.create_backup().await.map(Some)
    }

    fn cleanup_old_backups(backup_dir: &Path, max_count: usize) -> Result<(), CoreError> {
        if max_count == 0 {
            return Ok(());
        }

        let mut entries: Vec<_> = fs::read_dir(backup_dir)
            .map_err(|e| {
                CoreError::BackupFailed(format!("Failed to read backup directory: {}", e))
            })?
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().is_file())
            .collect();

        entries.sort_by_key(|entry| {
            entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });

        while entries.len() > max_count {
            if let Some(oldest) = entries.first() {
                fs::remove_file(oldest.path()).map_err(|e| {
                    CoreError::BackupFailed(format!("Failed to remove old backup: {}", e))
                })?;
                entries.remove(0);
            } else {
                break;
            }
        }

        Ok(())
    }
}

#[async_trait]
impl BackupService for BackupServiceImpl {
    async fn create_backup(&self) -> Result<String, CoreError> {
        let config_service = ConfigServiceImpl::new();
        let db_url = config_service
            .get_database_url()
            .await
            .map_err(|e| CoreError::BackupFailed(format!("Failed to get database URL: {}", e)))?;
        self.create_backup_at(&db_url).await
    }

    async fn list_backups(&self) -> Result<Vec<String>, CoreError> {
        let config_service = ConfigServiceImpl::new();
        let config = config_service
            .get_config()
            .await
            .map_err(|e| CoreError::BackupFailed(format!("Failed to get config: {}", e)))?;

        let mut backups: Vec<_> = fs::read_dir(&config.backup.directory)
            .map_err(|e| {
                CoreError::BackupFailed(format!("Failed to read backup directory: {}", e))
            })?
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().is_file())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();

        backups.sort();
        Ok(backups)
    }

    async fn restore_backup(&self, filename: &str) -> Result<String, CoreError> {
        let config_service = ConfigServiceImpl::new();
        let db_url = config_service
            .get_database_url()
            .await
            .map_err(|e| CoreError::BackupFailed(format!("Failed to get database URL: {}", e)))?;
        self.restore_backup_at(&db_url, filename).await
    }
}
