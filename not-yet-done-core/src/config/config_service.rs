use std::fs;
use std::io::Write;
use std::path::PathBuf;

use crate::config::{Config, ConfigError};

pub struct ConfigServiceImpl;

impl ConfigServiceImpl {
    pub fn new() -> Self {
        Self
    }

    fn config_path() -> PathBuf {
        let config_dir = dirs::config_dir()
            .expect("Could not determine config directory");
        config_dir.join("not_yet_done").join("config.yaml")
    }

    fn load_config() -> Result<Config, ConfigError> {
        let path = Self::config_path();
        
        if !path.exists() {
            return Err(ConfigError::ConfigNotFound(
                path.to_string_lossy().to_string(),
            ));
        }

        let content = fs::read_to_string(&path)
            .map_err(|e| ConfigError::ReadError(e))?;

        let config: Config = serde_yaml::from_str(&content)
            .map_err(|e| ConfigError::ParseError(e))?;

        config.backup.ensure_directory_exists()
            .map_err(|e| ConfigError::DirectoryError(e))?;

        config.backup.validate()
            .map_err(|e| ConfigError::ValidationError(e.to_string()))?;

        Ok(config)
    }

    fn save_config(config: &Config) -> Result<(), ConfigError> {
        let path = Self::config_path();
        let parent = path.parent()
            .ok_or_else(|| ConfigError::DirectoryError(
                std::io::Error::new(std::io::ErrorKind::NotFound, "No parent directory")
            ))?;

        fs::create_dir_all(parent)
            .map_err(|e| ConfigError::DirectoryError(e))?;

        let yaml = serde_yaml::to_string(config)
            .map_err(|e| ConfigError::ParseError(e))?;

        let mut file = fs::File::create(&path)
            .map_err(|e| ConfigError::WriteError(e))?;

        file.write_all(yaml.as_bytes())
            .map_err(|e| ConfigError::WriteError(e))?;

        Ok(())
    }

    fn prompt_user_to_create_config() -> Result<bool, ConfigError> {
        println!("Configuration file not found: {}", Self::config_path().display());
        print!("Create configuration file with default settings? [Y/n]: ");
        
        let mut input = String::new();
        std::io::stdout().flush()
            .map_err(|e| ConfigError::ReadError(e))?;
        
        std::io::stdin().read_line(&mut input)
            .map_err(|e| ConfigError::ReadError(e))?;

        let input = input.trim().to_lowercase();
        
        Ok(input == "" || input == "y" || input == "yes")
    }
}

 impl ConfigServiceImpl {
    pub async fn get_database_url(&self) -> Result<String, ConfigError> {
        if let Ok(db_url) = std::env::var("DATABASE_URL") {
            return Ok(db_url);
        }

        let config = self.get_config().await?;
        Ok(config.database.url)
    }

    pub async fn get_config(&self) -> Result<Config, ConfigError> {
        if let Ok(_db_url) = std::env::var("DATABASE_URL") {
            return Ok(Config::default());
        }

        match Self::load_config() {
            Ok(config) => Ok(config),
            Err(e) if matches!(e.kind(), crate::config::error::ConfigErrorKind::NotFound) => {
                let create = Self::prompt_user_to_create_config()?;
                
                if !create {
                    return Err(ConfigError::CreationDeclined);
                }

                let default_config = Config::default();
                Self::save_config(&default_config)?;
                
                println!("Configuration file created at: {}", Self::config_path().display());
                
                Ok(default_config)
            }
            Err(e) => Err(e),
        }
    }
}
