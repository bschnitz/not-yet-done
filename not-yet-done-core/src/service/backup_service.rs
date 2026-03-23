use async_trait::async_trait;
use shaku::Component;
use chrono::Utc;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::ConfigServiceImpl;
use crate::error::AppError;

#[async_trait]
pub trait BackupService: shaku::Interface {
    async fn create_backup(&self) -> Result<String, AppError>;
    async fn list_backups(&self) -> Result<Vec<String>, AppError>;
    async fn restore_backup(&self, filename: &str) -> Result<String, AppError>;
}

#[derive(Component)]
#[shaku(interface = BackupService)]
pub struct BackupServiceImpl;

impl BackupServiceImpl {
    fn extract_db_path(db_url: &str) -> Result<PathBuf, AppError> {
        if db_url.starts_with("sqlite://") {
            let path_str = db_url.strip_prefix("sqlite://")
                .ok_or_else(|| AppError::NotFileBasedDatabase)?;
            
            let path_str = path_str.split('?').next()
                .ok_or_else(|| AppError::NotFileBasedDatabase)?;
            
            let path = Path::new(path_str);
            
            if !path.exists() {
                return Err(AppError::DatabaseFileNotFound(path.to_path_buf()));
            }
            
            if !path.is_file() {
                return Err(AppError::NotFileBasedDatabase);
            }
            
            Ok(path.to_path_buf())
        } else {
            Err(AppError::NotFileBasedDatabase)
        }
    }

    fn generate_backup_filename(original_name: &str) -> String {
        let timestamp = Utc::now().format("%Y%m%d-%H%M%S");
        format!("{}-{}", timestamp, original_name)
    }

    fn cleanup_old_backups(backup_dir: &Path, max_count: usize) -> Result<(), AppError> {
        if max_count == 0 {
            return Ok(());
        }

        let mut entries: Vec<_> = fs::read_dir(backup_dir)
            .map_err(|e| AppError::BackupFailed(format!("Failed to read backup directory: {}", e)))?
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().is_file())
            .collect();

        entries.sort_by_key(|entry| {
            entry.metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });

        while entries.len() > max_count {
            if let Some(oldest) = entries.first() {
                fs::remove_file(oldest.path())
                    .map_err(|e| AppError::BackupFailed(format!("Failed to remove old backup: {}", e)))?;
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
    async fn create_backup(&self) -> Result<String, AppError> {
        let config_service = ConfigServiceImpl::new();

        let db_url = config_service.get_database_url().await
            .map_err(|e| AppError::BackupFailed(format!("Failed to get database URL: {}", e)))?;

        let db_path = Self::extract_db_path(&db_url)?;

        let config = config_service.get_config().await
            .map_err(|e| AppError::BackupFailed(format!("Failed to get config: {}", e)))?;

        let original_filename = db_path.file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| AppError::BackupFailed("Invalid database filename".to_string()))?;

        let backup_filename = Self::generate_backup_filename(original_filename);
        let backup_path = config.backup.directory.join(&backup_filename);

        fs::copy(&db_path, &backup_path)
            .map_err(|e| AppError::BackupFailed(format!("Failed to copy database file: {}", e)))?;

        Self::cleanup_old_backups(&config.backup.directory, config.backup.max_count)?;

        Ok(backup_path.to_string_lossy().to_string())
    }

    async fn list_backups(&self) -> Result<Vec<String>, AppError> {
        let config_service = ConfigServiceImpl::new();
        let config = config_service.get_config().await
            .map_err(|e| AppError::BackupFailed(format!("Failed to get config: {}", e)))?;

        let mut backups: Vec<_> = fs::read_dir(&config.backup.directory)
            .map_err(|e| AppError::BackupFailed(format!("Failed to read backup directory: {}", e)))?
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().is_file())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();

        backups.sort();
        Ok(backups)
    }

    async fn restore_backup(&self, filename: &str) -> Result<String, AppError> {
        let config_service = ConfigServiceImpl::new();

        let db_url = config_service.get_database_url().await
            .map_err(|e| AppError::BackupFailed(format!("Failed to get database URL: {}", e)))?;

        let db_path = Self::extract_db_path(&db_url)?;

        let config = config_service.get_config().await
            .map_err(|e| AppError::BackupFailed(format!("Failed to get config: {}", e)))?;

        let backup_path = config.backup.directory.join(filename);

        if !backup_path.exists() {
            return Err(AppError::BackupFailed(format!("Backup file not found: {}", filename)));
        }

        if !backup_path.is_file() {
            return Err(AppError::BackupFailed(format!("Backup is not a file: {}", filename)));
        }

        fs::copy(&backup_path, &db_path)
            .map_err(|e| AppError::BackupFailed(format!("Failed to restore database file: {}", e)))?;

        Ok(db_path.to_string_lossy().to_string())
    }
}
