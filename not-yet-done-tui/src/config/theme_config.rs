use serde::{Deserialize, Serialize};

use super::color::HexColor;

/// All theme colours, fully configurable via `tui-theme.yaml`.
///
/// Each field maps directly to a named constant in the old `Theme` struct.
/// The YAML file uses `#rrggbb` hex strings:
///
/// ```yaml
/// name: Teal Dark
/// bg:          "#121212"
/// surface:     "#1e1e1e"
/// surface_2:   "#282828"
/// primary:     "#4db6ac"
/// primary_dim: "#26a69a"
/// on_primary:  "#002522"
/// accent:      "#ffca28"
/// accent_dim:  "#ffb300"
/// text_high:   "#e6e6e6"
/// text_med:    "#9e9e9e"
/// text_dim:    "#616161"
/// success:     "#66bb6a"
/// error:       "#ef5350"
/// warning:     "#ffa726"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeConfig {
    /// Human-readable name shown in error messages / future theme picker
    #[serde(default = "default_name")]
    pub name: String,

    // ── Surfaces ──────────────────────────────────────────────────────────
    #[serde(default = "d_bg")]
    pub bg: HexColor,
    #[serde(default = "d_surface")]
    pub surface: HexColor,
    #[serde(default = "d_surface_2")]
    pub surface_2: HexColor,

    // ── Primary (teal) ────────────────────────────────────────────────────
    #[serde(default = "d_primary")]
    pub primary: HexColor,
    #[serde(default = "d_primary_dim")]
    pub primary_dim: HexColor,
    #[serde(default = "d_on_primary")]
    pub on_primary: HexColor,

    // ── Accent (amber) ────────────────────────────────────────────────────
    #[serde(default = "d_accent")]
    pub accent: HexColor,
    #[serde(default = "d_accent_dim")]
    pub accent_dim: HexColor,

    // ── Text ──────────────────────────────────────────────────────────────
    #[serde(default = "d_text_high")]
    pub text_high: HexColor,
    #[serde(default = "d_text_med")]
    pub text_med: HexColor,
    #[serde(default = "d_text_dim")]
    pub text_dim: HexColor,

    // ── Status ────────────────────────────────────────────────────────────
    #[serde(default = "d_success")]
    pub success: HexColor,
    #[serde(default = "d_error")]
    pub error: HexColor,
    #[serde(default = "d_warning")]
    pub warning: HexColor,

    // ── Secondary / Tertiary accents ────────────────────────────────────
    #[serde(default = "d_secondary")]
    pub secondary: HexColor,
    #[serde(default = "d_tertiary")]
    pub tertiary: HexColor,

    // ── Tree ─────────────────────────────────────────────────────────────
    #[serde(default = "d_tree_connector")]
    pub tree_connector: HexColor,

    // ── Tracking taskpath ────────────────────────────────────────────────
    /// Color of the separator string between path segments in the
    /// `taskpath` column of the trackings views.
    #[serde(default = "d_taskpath_separator")]
    pub taskpath_separator: HexColor,

    // ── Grouping ─────────────────────────────────────────────────────────
    /// Color of group-header rows and the grand-total footer in grouped
    /// content views (M3 `group_by`). A row introducing a date bucket or a
    /// category, plus the totals line, render in this accent so they stand
    /// out from the data rows. Defaults to the same hue as `accent`.
    #[serde(default = "d_group_header")]
    pub group_header: HexColor,

    // ── Unread ───────────────────────────────────────────────────────────
    /// Color of the unread highlight in chat-style adapters (Stoat): the
    /// name of a channel/category that holds unread messages, plus the
    /// header line of an unread message in the message list, render in this
    /// accent and are prefixed with the configurable `unread_marker` glyph.
    /// Defaults to a vivid blue so unread items pop against the dim tree.
    #[serde(default = "d_unread")]
    pub unread: HexColor,

    // ── Card mode ────────────────────────────────────────────────────────
    /// Color of the frame glyphs around a card (`card:` mode in a content
    /// view). Dim by default so the frame structures the list without
    /// competing with the values inside it. A view may override it per level
    /// via `card.border_style`.
    #[serde(default = "d_card_border")]
    pub card_border: HexColor,
    /// Color of the field labels inside a card. Overridable per level via
    /// `card.label_style`.
    #[serde(default = "d_card_label")]
    pub card_label: HexColor,

    // ── Tab bar ─────────────────────────────────────────────────────────
    #[serde(default = "d_tab_active")]
    pub tab_active: HexColor,
    #[serde(default = "d_tab_active_bg")]
    pub tab_active_bg: HexColor,
    #[serde(default = "d_sub_tab_active")]
    pub sub_tab_active: HexColor,
    #[serde(default = "d_sub_tab_active_bg")]
    pub sub_tab_active_bg: HexColor,

    // ── Toolbar ──────────────────────────────────────────────────────────
    #[serde(default = "d_toolbar_bg")]
    pub toolbar_bg: HexColor,

    // ── Form ───────────────────────────────────────────────────────────────
    #[serde(default = "d_focused_bg")]
    pub focused_bg: HexColor,
    #[serde(default = "d_form_bg")]
    pub form_bg: HexColor,
    /// Fine-grained theming for the generic spec-driven form popup
    /// (`InputSpec::Form` actions). Every role is optional and falls back to a
    /// mapped app-theme colour when unset (see [`crate::ui::Theme`] `form_*`
    /// accessors), so an empty `form:` block reproduces the classic look.
    #[serde(default)]
    pub form: FormThemeConfig,

    /// Fine-grained theming for the built-in vim editor pane (editor profiles
    /// with `builtin: true`). Every role is optional and falls back to a mapped
    /// app-theme colour when unset (see [`crate::ui::Theme`] `vim_*`
    /// accessors), so an empty `vim:` block reproduces the classic look.
    #[serde(default)]
    pub vim: VimThemeConfig,

    // ── Alert bar ────────────────────────────────────────────────────────
    /// Foreground + background of the prominent alert bar — the optional
    /// notification strip beneath the top chrome that carries important
    /// messages (e.g. the Microsoft Authenticator number to tap during an
    /// interactive sign-in). Default: bold black text on the warm app orange
    /// field (the same orange used elsewhere as an accent) — prominent and
    /// highly legible without a harsh neon hue. Override both in
    /// `tui-theme.yaml` to taste.
    #[serde(default = "d_alert_fg")]
    pub alert_fg: HexColor,
    #[serde(default = "d_alert_bg")]
    pub alert_bg: HexColor,
}

/// Optional per-role overrides for the spec-driven form popup. Each `None`
/// falls back to the app theme (see the `form_*` accessors on
/// [`crate::ui::Theme`]). `field_bg`/`field_bg_idle` are the filled bars behind
/// a field; when both stay `None` no bar is drawn, matching the classic look.
///
/// ```yaml
/// form:
///   accent:    "#8ec07c"   # focused label / prefix / cursor
///   selected:  "#fabd2f"   # picked option / checked toggle
///   field_bg:  "#282828"   # bar behind the focused field
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FormThemeConfig {
    #[serde(default)]
    pub accent: Option<HexColor>,
    #[serde(default)]
    pub label_idle: Option<HexColor>,
    #[serde(default)]
    pub text: Option<HexColor>,
    #[serde(default)]
    pub text_idle: Option<HexColor>,
    #[serde(default)]
    pub placeholder: Option<HexColor>,
    #[serde(default)]
    pub selected: Option<HexColor>,
    #[serde(default)]
    pub hint: Option<HexColor>,
    #[serde(default)]
    pub error: Option<HexColor>,
    /// Fill behind the *focused* field (the bar). `None` → no bar.
    #[serde(default)]
    pub field_bg: Option<HexColor>,
    /// Fill behind *unfocused* fields. Usually `None`.
    #[serde(default)]
    pub field_bg_idle: Option<HexColor>,
    /// Fill behind the whole floating panel, used for the `event_form` look
    /// (a centred, borderless, content-sized panel — only when an action's
    /// `form.field_bar` is set). `None` → falls back to `form_bg`.
    #[serde(default)]
    pub panel_bg: Option<HexColor>,
}

/// Optional per-role overrides for the built-in vim editor pane. Each `None`
/// falls back to the app theme (see the `vim_*` accessors on
/// [`crate::ui::Theme`]), so an empty block keeps the pane looking like the
/// rest of the TUI.
///
/// ```yaml
/// vim:
///   bg:           "#1d2021"   # the whole pane
///   text:         "#ebdbb2"
///   selection_bg: "#504945"   # visual-mode selection
///   cursor_bg:    "#fabd2f"   # explicit cursor instead of the reverse trick
/// ```
///
/// The cursor is special: as long as both `cursor` and `cursor_bg` stay `None`
/// it is drawn by *reversing* whatever it sits on, which keeps it visible on
/// every background — including inside a selection. Setting either one turns
/// that off and paints the configured colours instead, so a fixed cursor colour
/// is a deliberate choice, not the default.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VimThemeConfig {
    /// Background of the whole pane, and the surface every other role sits on.
    #[serde(default)]
    pub bg: Option<HexColor>,
    /// Buffer text.
    #[serde(default)]
    pub text: Option<HexColor>,
    /// The character *under* the block cursor. `None` → reverse video.
    #[serde(default)]
    pub cursor: Option<HexColor>,
    /// The block cursor itself. `None` → reverse video.
    #[serde(default)]
    pub cursor_bg: Option<HexColor>,
    /// Line-number gutter.
    #[serde(default)]
    pub gutter: Option<HexColor>,
    /// The mode indicator (`-- INSERT --`).
    #[serde(default)]
    pub mode: Option<HexColor>,
    /// The rest of the status line.
    #[serde(default)]
    pub status: Option<HexColor>,
    /// The `:` command line and its messages.
    #[serde(default)]
    pub command_line: Option<HexColor>,
    /// Text inside a visual-mode selection.
    #[serde(default)]
    pub selection: Option<HexColor>,
    /// Fill behind a visual-mode selection.
    #[serde(default)]
    pub selection_bg: Option<HexColor>,
}

// ---------------------------------------------------------------------------
// Default helpers — serde requires fn pointers, not closures
// ---------------------------------------------------------------------------

fn default_name() -> String {
    "Catppuccin Mocha".to_string()
}
fn hex(s: &str) -> HexColor {
    s.parse().expect("hardcoded hex is valid")
}

fn d_bg() -> HexColor {
    hex("#1e1e2e")
}
fn d_surface() -> HexColor {
    hex("#313244")
}
fn d_surface_2() -> HexColor {
    hex("#45475a")
}
fn d_primary() -> HexColor {
    hex("#cba6f7")
}
fn d_primary_dim() -> HexColor {
    hex("#f5c2e7")
}
fn d_on_primary() -> HexColor {
    hex("#1e1e2e")
}
fn d_accent() -> HexColor {
    hex("#f9e2af")
}
fn d_accent_dim() -> HexColor {
    hex("#fab387")
}
fn d_text_high() -> HexColor {
    hex("#cdd6f4")
}
fn d_text_med() -> HexColor {
    hex("#bac2de")
}
fn d_text_dim() -> HexColor {
    hex("#45475a")
}
fn d_success() -> HexColor {
    hex("#a6e3a1")
}
fn d_error() -> HexColor {
    hex("#f38ba8")
}
fn d_warning() -> HexColor {
    hex("#f9e2af")
}
fn d_secondary() -> HexColor {
    hex("#4d699b")
}
fn d_tertiary() -> HexColor {
    hex("#597b75")
}
fn d_tree_connector() -> HexColor {
    hex("#54546d")
}
fn d_taskpath_separator() -> HexColor {
    hex("#fab387")
}
fn d_group_header() -> HexColor {
    hex("#f9e2af")
}
fn d_unread() -> HexColor {
    hex("#89b4fa")
}
fn d_card_border() -> HexColor {
    // The same dim structural hue as the tree connectors — both draw
    // scaffolding, not content.
    hex("#54546d")
}
fn d_card_label() -> HexColor {
    hex("#7f849c")
}
fn d_tab_active() -> HexColor {
    hex("#B8BB26")
}
fn d_tab_active_bg() -> HexColor {
    hex("#79740E")
}
fn d_sub_tab_active() -> HexColor {
    hex("#D3869B")
}
fn d_sub_tab_active_bg() -> HexColor {
    hex("#8F3F71")
}
fn d_toolbar_bg() -> HexColor {
    hex("#252535")
}
fn d_focused_bg() -> HexColor {
    // Subtly lighter than `form_bg` so the focused field/row stands out
    // without a garish block. Overridable via the theme yaml.
    hex("#313244")
}
fn d_form_bg() -> HexColor {
    hex("#000000")
}
fn d_alert_fg() -> HexColor {
    // Bold black text on the orange field — maximum legibility, not garish.
    hex("#000000")
}
fn d_alert_bg() -> HexColor {
    // The warm app orange already used elsewhere (accent_dim, taskpath_separator),
    // here as the bar's fill.
    hex("#fab387")
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            name: default_name(),
            bg: d_bg(),
            surface: d_surface(),
            surface_2: d_surface_2(),
            primary: d_primary(),
            primary_dim: d_primary_dim(),
            on_primary: d_on_primary(),
            accent: d_accent(),
            accent_dim: d_accent_dim(),
            text_high: d_text_high(),
            text_med: d_text_med(),
            text_dim: d_text_dim(),
            success: d_success(),
            error: d_error(),
            warning: d_warning(),
            secondary: d_secondary(),
            tertiary: d_tertiary(),
            tree_connector: d_tree_connector(),
            taskpath_separator: d_taskpath_separator(),
            group_header: d_group_header(),
            unread: d_unread(),
            card_border: d_card_border(),
            card_label: d_card_label(),
            tab_active: d_tab_active(),
            tab_active_bg: d_tab_active_bg(),
            sub_tab_active: d_sub_tab_active(),
            sub_tab_active_bg: d_sub_tab_active_bg(),
            toolbar_bg: d_toolbar_bg(),
            focused_bg: d_focused_bg(),
            form_bg: d_form_bg(),
            form: FormThemeConfig::default(),
            vim: VimThemeConfig::default(),
            alert_fg: d_alert_fg(),
            alert_bg: d_alert_bg(),
        }
    }
}
