use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub database: DatabaseConfig,
    #[serde(default)]
    pub backup: BackupConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupConfig {
    #[serde(default = "default_max_count")]
    pub max_count: usize,
    #[serde(default = "default_backup_directory")]
    pub directory: PathBuf,
}

fn default_max_count() -> usize {
    10
}

fn default_backup_directory() -> PathBuf {
    let data_dir = dirs::data_local_dir()
        .expect("Could not determine data directory")
        .join("not_yet_done");
    data_dir.join("backups")
}

impl BackupConfig {
    pub fn ensure_directory_exists(&self) -> Result<(), std::io::Error> {
        fs::create_dir_all(&self.directory)?;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.max_count == 0 {
            return Err("max_count must be greater than 0".to_string());
        }
        Ok(())
    }
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            max_count: default_max_count(),
            directory: default_backup_directory(),
        }
    }
}

impl Config {
    pub fn with_defaults() -> Self {
        let db_path = Self::default_db_path();
        let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
        Self {
            database: DatabaseConfig { url: db_url },
            backup: BackupConfig::default(),
        }
    }

    fn default_db_path() -> PathBuf {
        let db_dir = dirs::data_local_dir()
            .expect("Could not determine data directory")
            .join("not_yet_done");

        db_dir.join("nyd.db")
    }

    fn ensure_db_directory_exists() -> Result<(), std::io::Error> {
        let db_path = Self::default_db_path();
        let parent = db_path.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "No parent directory")
        })?;

        fs::create_dir_all(parent)?;
        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::ensure_db_directory_exists().expect("Failed to create database directory");
        let config = Self::with_defaults();
        config
            .backup
            .ensure_directory_exists()
            .expect("Failed to create backup directory");
        config
            .backup
            .validate()
            .expect("Invalid backup configuration");
        config
    }
}
