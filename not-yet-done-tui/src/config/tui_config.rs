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
    /// Curated top-level tab order. See [`TabsConfig`]. Empty by default,
    /// which shows every configured tab in its natural slot order.
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
    /// Behaviour of the shortcut menu (opened via `global.shortcut_menu`,
    /// default `ctrl+y`). See [`ShortcutMenuConfig`].
    #[serde(default)]
    pub shortcut_menu: ShortcutMenuConfig,
    /// "Which-key" style popup that previews the possible completions of a
    /// half-typed chord (e.g. after `g` it lists `gl`, `gm`, …). Off by
    /// default. See [`WhichKeyConfig`].
    #[serde(default)]
    pub which_key: WhichKeyConfig,
    /// Inline terminal graphics in markdown bodies. See [`ImagesConfig`].
    #[serde(default)]
    pub images: ImagesConfig,
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
            shortcut_menu: Default::default(),
            which_key: Default::default(),
            images: Default::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// ShortcutMenuConfig — the shortcut/action menu (default key ctrl+y)
// ---------------------------------------------------------------------------

/// Which shortcuts the menu lists when it opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutScope {
    /// Only the shortcuts active in the current tab + drilldown level.
    #[default]
    Context,
    /// Every configured shortcut across all tabs and levels.
    All,
    /// Only actions that currently have no binding (across all tabs) — the
    /// menu's "give me a key" view: select one and record a binding with
    /// Ctrl+N.
    Unbound,
}

/// Behaviour of the shortcut menu.
///
/// ```yaml
/// shortcut_menu:
///   execute_on_enter: false   # Enter only closes (reference mode)
///   default_scope: context    # context | all | unbound
///   toggle_key: tab           # cycle this view -> all tabs -> unbound
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutMenuConfig {
    /// When `true`, pressing Enter on a row closes the menu and replays
    /// that row's key through the normal dispatch pipeline, running the
    /// action. Only meaningful in [`ShortcutScope::Context`] (keys from
    /// other tabs are contextless). Default `false` — reference only.
    #[serde(default)]
    pub execute_on_enter: bool,
    /// Scope the menu opens in. Default [`ShortcutScope::Context`].
    #[serde(default)]
    pub default_scope: ShortcutScope,
    /// Key that toggles between context and all scope while the popup is
    /// open. Default `tab`.
    #[serde(default = "default_shortcut_toggle_key")]
    pub toggle_key: String,
}

fn default_shortcut_toggle_key() -> String {
    "tab".to_string()
}

impl Default for ShortcutMenuConfig {
    fn default() -> Self {
        Self {
            execute_on_enter: false,
            default_scope: ShortcutScope::default(),
            toggle_key: default_shortcut_toggle_key(),
        }
    }
}

// ---------------------------------------------------------------------------
// WhichKeyConfig — the chord-completion preview popup
// ---------------------------------------------------------------------------

/// A "which-key" style popup that appears while a multi-step chord is
/// half-typed and lists every binding that continues the pressed prefix.
/// It is purely informational — keys still flow through the normal chord
/// dispatch, so completing the chord runs its action and an unmapped key
/// aborts it (closing the popup).
///
/// ```yaml
/// which_key:
///   enabled: true        # off by default
///   delay_ms: 300        # wait this long after the prefix before showing
///   prefixes: [g, z]     # only these first steps trigger it (empty = all)
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhichKeyConfig {
    /// Master switch. Default `false` — the popup never appears unless the
    /// user opts in.
    #[serde(default)]
    pub enabled: bool,
    /// How long the pending chord must sit before the popup pops up, in
    /// milliseconds. A short delay keeps fluently-typed chords from flashing
    /// the popup. Default `300`.
    #[serde(default = "default_which_key_delay_ms")]
    pub delay_ms: u64,
    /// Allowlist of chord prefixes that may trigger the popup. Each entry is
    /// itself a key sequence (`g`, `z`, or even `g l`); the popup shows only
    /// when the pending chord starts with one of them. Empty means *every*
    /// prefix is eligible. Default empty.
    #[serde(default)]
    pub prefixes: Vec<String>,
}

fn default_which_key_delay_ms() -> u64 {
    300
}

impl Default for WhichKeyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            delay_ms: default_which_key_delay_ms(),
            prefixes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavigationConfig {
    #[serde(default = "default_jump_chars")]
    pub jump_chars: String,
    /// Command used by link-hop (`f`) to open a picked URL. The URL is
    /// appended as the final argument; the string is split on whitespace so
    /// extra flags work (e.g. `firefox --new-tab`). Default: `xdg-open`.
    #[serde(default = "default_link_opener")]
    pub link_opener: String,
}

fn default_jump_chars() -> String {
    "abcdefghijklmnopqrstuvwxyz".to_string()
}

fn default_link_opener() -> String {
    "xdg-open".to_string()
}

impl Default for NavigationConfig {
    fn default() -> Self {
        Self {
            jump_chars: default_jump_chars(),
            link_opener: default_link_opener(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationsConfig {
    /// Maximum number of lines for the notification area.
    #[serde(default = "default_notification_max_lines")]
    pub max_lines: u16,

    /// How many messages the bottom notification bar shows at once. Once the
    /// bar holds this many, the next message pushes the oldest one out, so the
    /// newest is always on screen (set it to `1` for a single-message bar).
    /// Dropped messages stay readable via the `show_notifications` action.
    /// `0` = unlimited. Does not affect the top alert bar.
    #[serde(default = "default_notification_max_messages")]
    pub max_messages: u16,

    /// How many past notifications each bar keeps for the `show_notifications`
    /// editor view. The log survives both the display cap and a dismiss.
    /// `0` = unlimited.
    #[serde(default = "default_notification_history_limit")]
    pub history_limit: u16,

    /// Whether the prominent top alert bar is active. When `true` (default),
    /// `type: notify` actions flagged `prominent: true` render in the loud top
    /// bar (theme `alert_fg`/`alert_bg`); when `false`, they fall back to the
    /// ordinary bottom notification bar, so a user who dislikes the top strip
    /// can switch it off without touching any view config.
    #[serde(default = "default_alert_enabled")]
    pub alert_enabled: bool,

    /// Maximum number of lines for the top alert bar.
    #[serde(default = "default_alert_max_lines")]
    pub alert_max_lines: u16,

    /// Where the load banner of a tab that is currently fetching appears.
    /// Overridable per view file via `tab.load_banner`, so a single slow tab
    /// may be loud without making every tab loud.
    #[serde(default)]
    pub load_banner: LoadBannerRoute,
}

/// Where a tab's load banner is shown ([`NotificationsConfig::load_banner`]).
///
/// The default differs from the one for auth prompts on purpose. An MFA
/// challenge *must* be global — otherwise the user, sitting in another tab,
/// never learns that something is waiting for them. A load counter is the
/// opposite: it resolves on its own, so from another tab it is pure noise.
/// Hence `tab` by default, with `global` available for the one tab whose
/// loads are slow enough to be worth watching from elsewhere.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoadBannerRoute {
    /// Inside the loading tab, on its own banner line — visible only while
    /// that tab is in front. The default.
    #[default]
    Tab,
    /// On the global bar shared by all tabs, prefixed with the tab's name
    /// (`"Jira — Loading… 40 % (3s)"`) since the surface itself no longer
    /// says which tab is meant. Several tabs loading at once collapse into
    /// one counter rather than one line each. Falls back to the bottom
    /// notification bar when [`NotificationsConfig::alert_enabled`] is off,
    /// exactly as a prominent `notify` action does.
    Global,
    /// Nowhere. The load still happens and errors still surface; only the
    /// progress line is suppressed.
    Off,
}

fn default_notification_max_lines() -> u16 {
    5
}

fn default_notification_max_messages() -> u16 {
    5
}

fn default_notification_history_limit() -> u16 {
    200
}

fn default_alert_enabled() -> bool {
    true
}

fn default_alert_max_lines() -> u16 {
    3
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            max_lines: default_notification_max_lines(),
            max_messages: default_notification_max_messages(),
            history_limit: default_notification_history_limit(),
            alert_enabled: default_alert_enabled(),
            alert_max_lines: default_alert_max_lines(),
            load_banner: LoadBannerRoute::default(),
        }
    }
}

/// Inline terminal graphics: pictures drawn between the text lines of a
/// `markdown: true` column (chat screenshots, pasted images).
///
/// Whether anything is actually drawn depends on the terminal: at startup the
/// TUI asks it which graphics protocol it speaks (kitty, sixel, iTerm2) and
/// falls back to halfblocks. A terminal that answers nothing keeps the plain
/// `[image: …]` text, exactly as with `enabled: false`.
///
/// ```yaml
/// images:
///   enabled: true
///   max_height: 20
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImagesConfig {
    /// Master switch. `false` skips the startup capability query altogether,
    /// so nothing is downloaded and every image stays a text placeholder.
    #[serde(default = "default_images_enabled")]
    pub enabled: bool,

    /// Tallest a single picture may get, in terminal rows. Anything larger is
    /// scaled down (aspect preserved) so one screenshot can't push a whole
    /// conversation off the screen.
    #[serde(default = "default_images_max_height")]
    pub max_height: u16,
}

fn default_images_enabled() -> bool {
    true
}

fn default_images_max_height() -> u16 {
    20
}

impl Default for ImagesConfig {
    fn default() -> Self {
        Self {
            enabled: default_images_enabled(),
            max_height: default_images_max_height(),
        }
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
