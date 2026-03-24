use std::fs;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::keybindings::KeyBindingConfig;
use super::layout::LayoutConfig;
use super::theme_config::ThemeConfig;

// ---------------------------------------------------------------------------
// TuiConfig — top-level, owns all sub-configs
// ---------------------------------------------------------------------------

/// Deserialises from `~/.config/not_yet_done/tui.yaml`:
///
/// ```yaml
/// keybindings:
///   global:
///     quit: q
///     tab_tasks: "2"
///     ...
///   tasks:
///     view_list: l
///     form_add: a
///     ...
///
/// theme:
///   name: Teal Dark
///   bg: "#121212"
///   ...
///
/// layout:
///   tasks:
///     split:
///       type: vertical
///       horizontal-threshold: 120
///       vertical-threshold: 80
///       order: [view, form]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TuiConfig {
    #[serde(default)]
    pub keybindings: KeyBindingConfig,
    #[serde(default)]
    pub theme:       ThemeConfig,
    #[serde(default)]
    pub layout:      LayoutConfig,
}

// ---------------------------------------------------------------------------
// TuiConfigService
// ---------------------------------------------------------------------------

pub struct TuiConfigService;

impl TuiConfigService {
    pub fn config_path() -> PathBuf {
        dirs::config_dir()
            .expect("Could not determine config directory")
            .join("not_yet_done")
            .join("tui.yaml")
    }

    /// Load `tui.yaml`. If the file does not exist, writes defaults and returns them.
    pub fn load() -> Result<TuiConfig> {
        let path = Self::config_path();

        if !path.exists() {
            let default = TuiConfig::default();
            Self::save(&default).with_context(|| {
                format!("Failed to write default tui.yaml at {}", path.display())
            })?;
            return Ok(default);
        }

        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;

        let config: TuiConfig = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse {}", path.display()))?;

        Ok(config)
    }

    fn save(config: &TuiConfig) -> Result<()> {
        let path = Self::config_path();

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create config directory {}", parent.display())
            })?;
        }

        let yaml = serde_yaml::to_string(config)
            .context("Failed to serialize tui.yaml")?;

        fs::File::create(&path)
            .with_context(|| format!("Failed to create {}", path.display()))?
            .write_all(yaml.as_bytes())
            .context("Failed to write tui.yaml")?;

        Ok(())
    }
}
