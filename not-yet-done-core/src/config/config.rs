use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub database: DatabaseConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
}

impl Config {
    pub fn with_default_db_url() -> Self {
        let db_path = Self::default_db_path();
        let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
        Self {
            database: DatabaseConfig { url: db_url },
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
        Self::with_default_db_url()
    }
}
