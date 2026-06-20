use std::fs;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::editor::EditorsConfig;
use super::keybindings::KeyBindingConfig;
use super::script::ScriptConfig;
use super::tabs::TabsConfig;
use super::theme_config::ThemeConfig;
use super::tracking::TrackingConfig;

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
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiConfig {
    #[serde(default)]
    pub keybindings: KeyBindingConfig,
    #[serde(default)]
    pub theme: ThemeConfig,
    #[serde(default)]
    pub editors: EditorsConfig,
    #[serde(default)]
    pub tracking: TrackingConfig,
    #[serde(default)]
    pub script: ScriptConfig,
    #[serde(default)]
    pub notifications: NotificationsConfig,
    #[serde(default)]
    pub navigation: NavigationConfig,
    /// Named tab constellations + which one is active. See [`TabsConfig`].
    /// Empty by default — the feature stays dormant and tabs keep the
    /// legacy fixed `1`..`6` keys until at least one constellation is
    /// defined.
    #[serde(default)]
    pub tabs: TabsConfig,
    /// Direct key → cmdline command bindings, bypassing the `:` prompt.
    /// Triggered only when no typed action is bound to the key, and
    /// before the chord-prefix fallback — so single-character keys can
    /// be safely shadowed without breaking `glm`/`glp` chord prefixes.
    ///
    /// ```yaml
    /// cmdline_shortcuts:
    ///   F2: "config tui"
    ///   "<c-comma>": "config"
    /// ```
    ///
    /// The value is passed verbatim to [`crate::app::App::execute_cmdline`],
    /// so anything that works after typing `:` works here.
    ///
    /// Built-in defaults (used when the field is absent from tui.yaml):
    ///   - `mc` → `cut-node` (mark task for moving)
    ///   - `mp` → `paste-node` (move cut task under current selection)
    /// Defining the field overrides the defaults completely — copy
    /// the entries you want to keep.
    ///
    /// Multi-character keys (`mc`, `mp`, …) are treated as chord
    /// sequences: the first character is stashed as a chord prefix,
    /// the next character completes it. So `mc` shadows the standalone
    /// key `m`; you can still use `m` for something else as long as no
    /// chord starting with `m` is bound.
    #[serde(default = "default_cmdline_shortcuts")]
    pub cmdline_shortcuts: std::collections::HashMap<String, String>,
}

/// Default shortcuts shipped with the app. See
/// [`TuiConfig::cmdline_shortcuts`] for the override contract.
fn default_cmdline_shortcuts() -> std::collections::HashMap<String, String> {
    let mut m = std::collections::HashMap::new();
    m.insert("mc".to_string(), "cut-node".to_string());
    m.insert("mp".to_string(), "paste-node".to_string());
    m
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            keybindings: Default::default(),
            theme: Default::default(),
            editors: Default::default(),
            tracking: Default::default(),
            script: Default::default(),
            notifications: Default::default(),
            navigation: Default::default(),
            tabs: Default::default(),
            cmdline_shortcuts: default_cmdline_shortcuts(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavigationConfig {
    #[serde(default = "default_jump_chars")]
    pub jump_chars: String,
}

fn default_jump_chars() -> String {
    "abcdefghijklmnopqrstuvwxyz".to_string()
}

impl Default for NavigationConfig {
    fn default() -> Self {
        Self { jump_chars: default_jump_chars() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationsConfig {
    /// Maximum number of lines for the notification area.
    #[serde(default = "default_notification_max_lines")]
    pub max_lines: u16,
}

fn default_notification_max_lines() -> u16 {
    5
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self { max_lines: default_notification_max_lines() }
    }
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

        let yaml = serde_yaml::to_string(config).context("Failed to serialize tui.yaml")?;

        fs::File::create(&path)
            .with_context(|| format!("Failed to create {}", path.display()))?
            .write_all(yaml.as_bytes())
            .context("Failed to write tui.yaml")?;

        Ok(())
    }
}
