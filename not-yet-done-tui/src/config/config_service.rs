use std::fs;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};

use super::keybindings::KeyBindingConfig;

pub struct TuiConfigService;

impl TuiConfigService {
    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .expect("Could not determine config directory")
            .join("not_yet_done")
            .join("tui-keybindings.yaml")
    }

    pub fn load() -> Result<KeyBindingConfig> {
        let path = Self::config_path();

        if !path.exists() {
            let default = KeyBindingConfig::default();
            Self::save(&default)
                .with_context(|| format!("Failed to create default keybinding config at {}", path.display()))?;
            return Ok(default);
        }

        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read keybinding config at {}", path.display()))?;

        let config: KeyBindingConfig = serde_yaml::from_str(&content)
            .with_context(|| "Failed to parse tui-keybindings.yaml")?;

        Ok(config)
    }

    fn save(config: &KeyBindingConfig) -> Result<()> {
        let path = Self::config_path();

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create config directory {}", parent.display()))?;
        }

        let yaml = serde_yaml::to_string(config)
            .context("Failed to serialize keybinding config")?;

        let mut file = fs::File::create(&path)
            .with_context(|| format!("Failed to create config file at {}", path.display()))?;

        file.write_all(yaml.as_bytes())
            .with_context(|| "Failed to write keybinding config")?;

        Ok(())
    }
}
