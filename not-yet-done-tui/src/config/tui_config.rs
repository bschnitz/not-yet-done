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
    /// The grouped shortcut overview (opened via `global.shortcut_overview`).
    /// See [`ShortcutOverviewConfig`].
    #[serde(default)]
    pub shortcut_overview: ShortcutOverviewConfig,
    /// Settings shared by every floating popup. See [`PopupsConfig`].
    #[serde(default)]
    pub popups: PopupsConfig,
    /// "Which-key" style popup that previews the possible completions of a
    /// half-typed chord (e.g. after `g` it lists `gl`, `gm`, …). Off by
    /// default. See [`WhichKeyConfig`].
    #[serde(default)]
    pub which_key: WhichKeyConfig,
    /// Inline terminal graphics in markdown bodies. See [`ImagesConfig`].
    #[serde(default)]
    pub images: ImagesConfig,
    /// What the pointer does. Colours live in [`ThemeConfig`]; this is
    /// behaviour. See [`MouseConfig`].
    #[serde(default)]
    pub mouse: MouseConfig,
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
            script: Default::default(),
            notifications: Default::default(),
            navigation: Default::default(),
            tabs: Default::default(),
            cmdline_shortcuts: default_cmdline_shortcuts(),
            shortcut_menu: Default::default(),
            shortcut_overview: Default::default(),
            popups: Default::default(),
            which_key: Default::default(),
            images: Default::default(),
            mouse: Default::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// MouseConfig — pointer behaviour (colours live in ThemeConfig)
// ---------------------------------------------------------------------------

/// What one notch of the wheel does over a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WheelMode {
    /// Pan the viewport and leave the cursor on its row — how a scroll wheel
    /// behaves everywhere else. The cursor only comes along when the window
    /// would otherwise leave it behind.
    #[default]
    View,
    /// Move the cursor and let the viewport follow, exactly as holding `j` or
    /// `k` does. This was the only behaviour before panning existed.
    Cursor,
}

/// Behaviour of the pointer.
///
/// ```yaml
/// mouse:
///   highlight_press: false
///   wheel: view # or: cursor
///   wheel_rows: 3
/// ```
///
/// Kept out of the `mouse` cargo feature on purpose, like the matching colour
/// block: a `tui.yaml` written against a build with mouse support must still
/// parse against one without.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MouseConfig {
    /// Tint the cell the left button went down on while nothing has been
    /// dragged yet.
    ///
    /// Off by default: almost every press turns out to be a click, and the
    /// highlight then flashes for a frame and reads as a stray cursor rather
    /// than as a selection. Turn it on to see where a drag is anchored.
    #[serde(default)]
    pub highlight_press: bool,

    /// Whether the wheel pans the view or walks the cursor. See [`WheelMode`].
    #[serde(default)]
    pub wheel: WheelMode,

    /// How far one notch of the wheel goes: rows in a normal table, physical
    /// lines in a smooth-scrolling one (the chat), cursor steps in
    /// [`WheelMode::Cursor`].
    #[serde(default = "default_wheel_rows")]
    pub wheel_rows: usize,
}

fn default_wheel_rows() -> usize {
    3
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self {
            highlight_press: false,
            wheel: WheelMode::default(),
            wheel_rows: default_wheel_rows(),
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
// ShortcutOverviewConfig — the grouped keyboard-shortcut popup
// ---------------------------------------------------------------------------

/// The shortcut overview: every shortcut of the current context, grouped —
/// "General" first, then one section per [`WhichKeyGroup`].
///
/// ```yaml
/// shortcut_overview:
///   min_width: 50   # narrowest the popup body may get, in cells
///   max_width: 80   # widest it may get
/// ```
///
/// Both are unset by default: the popup is then sized by its content alone,
/// as every other popup is.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShortcutOverviewConfig {
    /// How wide the popup body is at least, in cells. Keeps a short section
    /// (or a view with few shortcuts) from collapsing into a narrow column
    /// that re-flows every time the overview is opened somewhere else. Unset
    /// by default.
    #[serde(default)]
    pub min_width: Option<u16>,
    /// How wide the popup body may grow, in cells. Long shortcut names are
    /// truncated at this width instead of stretching the popup across the
    /// terminal. Unset by default. A [`min_width`](Self::min_width) larger
    /// than this one wins — the popup never ends up narrower than its
    /// minimum.
    #[serde(default)]
    pub max_width: Option<u16>,
}

// ---------------------------------------------------------------------------
// PopupsConfig — settings shared by every floating popup
// ---------------------------------------------------------------------------

/// Chrome settings that apply to every popup at once.
///
/// ```yaml
/// popups:
///   hint_width: 50   # how far the key hints may widen a popup
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PopupsConfig {
    /// How many cells the bottom key-hint line may widen a popup to before it
    /// wraps to another row instead. Raise it for wide terminals (fewer hint
    /// rows, wider popups), lower it to keep popups narrow. Default `50`.
    #[serde(default = "default_popup_hint_width")]
    pub hint_width: u16,
}

fn default_popup_hint_width() -> u16 {
    crate::ui::panel_chrome::DEFAULT_HINT_WIDTH_CAP as u16
}

impl Default for PopupsConfig {
    fn default() -> Self {
        Self {
            hint_width: default_popup_hint_width(),
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
///   groups:              # optional naming/folding per chord prefix
///     - prefix: o
///       title: "Open ..."
///       collapse_in_bars: true
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
    /// Named chord groups. Independent of [`Self::prefixes`] — naming a group
    /// here neither restricts nor widens which prefixes pop the menu, it only
    /// gives the group a heading and, optionally, folds its keys away in the
    /// bars. Default empty.
    #[serde(default)]
    pub groups: Vec<WhichKeyGroup>,
}

/// One named chord group: everything bound under `prefix`.
///
/// The [`title`](Self::title) replaces the bare `✦ o…` heading of the
/// which-key popup, and [`collapse_in_bars`](Self::collapse_in_bars) trades
/// the group's individual entries in the action and status bars for a single
/// `o Open …` one. Both are inert while `which_key.enabled` is `false` —
/// without the popup there would be nothing left to discover the folded keys
/// with.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WhichKeyGroup {
    /// The chord prefix this group covers, written exactly as it is bound
    /// (`o`, `g l`, `ctrl+k`).
    pub prefix: String,
    /// Free-form name for the group. Shown as the popup heading and as the
    /// label of the collapsed bar entry. Optional — without it the popup
    /// keeps its `✦ o…` heading and a collapsed group reads `o …`.
    #[serde(default)]
    pub title: Option<String>,
    /// Fold the group in the bars: every hint, favorite and script shortcut
    /// under `prefix` is dropped and one `prefix title` entry takes the place
    /// of the first of them. A binding that *is* the bare prefix key (a view
    /// that uses plain `o` for an action of its own) never opens the group's
    /// menu and therefore keeps its entry. Default `false`.
    #[serde(default)]
    pub collapse_in_bars: bool,
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
            groups: Vec::new(),
        }
    }
}

impl WhichKeyConfig {
    /// The title configured for exactly this pending prefix, if any. Steps
    /// are compared token-wise, so `g  l` in the config still matches the
    /// `g l` the dispatcher reports.
    pub fn title_for(&self, prefix: &str) -> Option<&str> {
        let steps: Vec<&str> = prefix.split_whitespace().collect();
        self.groups
            .iter()
            .find(|g| g.prefix.split_whitespace().collect::<Vec<_>>() == steps)
            .and_then(|g| g.title.as_deref())
    }

    /// The groups that fold their keys away in the bars — empty while the
    /// popup is switched off, so the bars stay complete.
    pub fn collapsing_groups(&self) -> &[WhichKeyGroup] {
        if self.enabled { &self.groups } else { &[] }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The documented `which_key` block has to survive the real deserializer —
    /// a renamed field would otherwise only surface as a silently ignored
    /// option in the user's config.
    #[test]
    fn which_key_block_from_the_docs_parses() {
        let yaml = "
which_key:
  enabled: true
  delay_ms: 300
  prefixes: [g, z]
  groups:
    - prefix: o
      title: Open ...
      collapse_in_bars: true
    - prefix: g l
";
        let config: TuiConfig = serde_yaml::from_str(yaml).expect("which_key block parses");
        let wk = &config.which_key;
        assert!(wk.enabled);
        assert_eq!(wk.delay_ms, 300);
        assert_eq!(wk.prefixes, vec!["g".to_string(), "z".to_string()]);
        assert_eq!(wk.groups.len(), 2);
        assert_eq!(wk.title_for("o"), Some("Open ..."));
        assert!(wk.groups[0].collapse_in_bars);
        // Prefix alone is enough: title and collapse_in_bars are optional.
        assert_eq!(wk.groups[1].prefix, "g l");
        assert_eq!(wk.groups[1].title, None);
        assert!(!wk.groups[1].collapse_in_bars);
    }

    /// The wheel block from the docs, and the defaults a `tui.yaml` that says
    /// nothing about the mouse has to keep: panning, three rows a notch.
    #[test]
    fn the_documented_mouse_block_parses_and_defaults_to_panning() {
        let bare: TuiConfig = serde_yaml::from_str("images:\n  enabled: false\n").unwrap();
        assert_eq!(bare.mouse.wheel, WheelMode::View);
        assert_eq!(bare.mouse.wheel_rows, 3);
        assert!(!bare.mouse.highlight_press);

        let config: TuiConfig = serde_yaml::from_str(
            "
mouse:
  highlight_press: false
  wheel: cursor
  wheel_rows: 5
",
        )
        .expect("mouse block parses");
        assert_eq!(config.mouse.wheel, WheelMode::Cursor);
        assert_eq!(config.mouse.wheel_rows, 5);

        // A block that names only one knob keeps the defaults for the rest.
        let partial: TuiConfig = serde_yaml::from_str("mouse:\n  wheel_rows: 1\n").unwrap();
        assert_eq!(partial.mouse.wheel, WheelMode::View);
        assert_eq!(partial.mouse.wheel_rows, 1);
    }

    /// Same guard for the width options: the overview's two are optional (no
    /// bound at all unless asked for), the shared hint cap has a default.
    #[test]
    fn the_documented_popup_widths_parse_with_their_defaults() {
        let yaml = "
shortcut_overview:
  min_width: 50
  max_width: 72
popups:
  hint_width: 0
";
        let config: TuiConfig = serde_yaml::from_str(yaml).expect("width blocks parse");
        assert_eq!(config.shortcut_overview.min_width, Some(50));
        assert_eq!(config.shortcut_overview.max_width, Some(72));
        assert_eq!(config.popups.hint_width, 0);

        // One bound on its own is a valid config — the other stays unset.
        let only_min: TuiConfig = serde_yaml::from_str("shortcut_overview:\n  min_width: 50\n")
            .expect("min alone parses");
        assert_eq!(only_min.shortcut_overview.min_width, Some(50));
        assert_eq!(only_min.shortcut_overview.max_width, None);

        let bare: TuiConfig = serde_yaml::from_str("{}").expect("empty config parses");
        assert_eq!(bare.shortcut_overview.min_width, None);
        assert_eq!(bare.shortcut_overview.max_width, None);
        assert_eq!(bare.popups.hint_width, 50);
    }
}
