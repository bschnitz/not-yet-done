use std::fs;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};

use super::theme_config::ThemeConfig;

pub struct TuiThemeService;

impl TuiThemeService {
    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .expect("Could not determine config directory")
            .join("not_yet_done")
            .join("tui-theme.yaml")
    }

    /// Load theme from `tui-theme.yaml`.
    /// If the file does not exist, writes the default theme and returns it.
    pub fn load() -> Result<ThemeConfig> {
        let path = Self::config_path();

        if !path.exists() {
            let default = ThemeConfig::default();
            Self::save(&default)
                .with_context(|| format!(
                    "Failed to create default theme config at {}",
                    path.display()
                ))?;
            return Ok(default);
        }

        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read theme config at {}", path.display()))?;

        let config: ThemeConfig = serde_yaml::from_str(&content)
            .with_context(|| "Failed to parse tui-theme.yaml — check your hex colour values")?;

        Ok(config)
    }

    fn save(config: &ThemeConfig) -> Result<()> {
        let path = Self::config_path();

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!(
                    "Failed to create config directory {}",
                    parent.display()
                ))?;
        }

        let yaml = serde_yaml::to_string(config)
            .context("Failed to serialize theme config")?;

        let mut file = fs::File::create(&path)
            .with_context(|| format!("Failed to create theme file at {}", path.display()))?;

        file.write_all(yaml.as_bytes())
            .context("Failed to write theme config")?;

        Ok(())
    }
}
